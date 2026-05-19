# EPIC-11: WASM Plugin System

**§SPEC:** §28–30  
**Labels:** `epic/wasm`  
**Crates:** `duga-plugin-abi`, `duga-plugin-host`, `plugins-examples/format-code`

## Goal
Implement the WASM Component Model plugin system: WIT definition, wit-bindgen host/guest bindings, wasmtime host with capability gating, plugin loading/validation, and an example guest plugin.

---

### TASK-11.1: WIT file + wit-bindgen generate host/guest bindings

- **§SPEC:** §28.1 (Plugin contract WIT)
- **Labels:** `layer/wasm`, `priority/critical`
- **Description:** Create `duga-plugin-abi` crate. Place the exact WIT file from SECT 28.1 in `wit/plugin.wit`. Configure `build.rs` to run `wit-bindgen` generating host bindings (behind `host` feature) and guest bindings (behind `guest` feature). The generated code must compile for both the host (`x86_64-unknown-linux-gnu`) and guest (`wasm32-wasip2`).
- **Files affected:**
  - `crates/duga-plugin-abi/Cargo.toml` (new)
  - `crates/duga-plugin-abi/build.rs` (new)
  - `crates/duga-plugin-abi/wit/plugin.wit` (new)
  - `crates/duga-plugin-abi/src/lib.rs` (re-exports)
- **Types involved:** WIT `tool` interface, generated `Tool` trait (guest), generated host types
- **Functions to implement:** `build.rs` wit-bindgen invocation
- **Dependencies:** (root task — no duga deps)
- **Implementation steps:**
  1. Create `plugin.wit` with exact content from SECT 28.1
  2. `build.rs`: use `wit_bindgen::generate!({ path: "wit", world: "plugin" })` or configure per-feature
  3. Feature-gate: `host` → `wit_bindgen::generate!({ path: "wit/plugin.wit", world: "plugin", generate_all })`; `guest` → same for guest
  4. Test: `cargo check -p duga-plugin-abi` and `cargo check -p duga-plugin-abi --features host`
- **Edge cases:**
  - wit-bindgen version must match wasmtime version → pin both exactly
  - Generated code uses `wasmtime` types for host → add wasmtime dep behind host feature
- **Definition of Done:** WIT compiles, host+guest bindings generated
- **Acceptance criteria:**
  - `cargo check -p duga-plugin-abi --features host` succeeds
  - Generated host trait has `fn info(&mut self) -> ToolInfo`, `fn execute(&mut self, inv: Invocation) -> Result<Outcome, String>`
- **Test plan:** unit: `cargo check` is the test
- **Estimated effort:** 5 hours

---

### TASK-11.2: Example guest plugin — format-code

- **§SPEC:** §28.1 (guest implementation)
- **Labels:** `layer/wasm`, `priority/critical`
- **Description:** Create `plugins-examples/format-code/` — a minimal wasm32-wasip2 plugin that implements the `tool` interface from TASK-11.1. Uses `wit-bindgen` guest bindings. The plugin exports `info()` returning name "format-code" and a simple schema (`{file: string}`), and `execute()` that validates the input JSON and returns a success outcome. This serves as the reference plugin and build test.
- **Files affected:**
  - `plugins-examples/format-code/Cargo.toml` (new)
  - `plugins-examples/format-code/src/lib.rs` (new)
- **Types involved:** `wit_bindgen::generate!({ exports })`, `ToolInfo`, `Invocation`, `Outcome`
- **Functions to implement:** `info()`, `execute()`
- **Dependencies:** TASK-11.1
- **Implementation steps:**
  1. Cargo.toml: `crate-type = ["cdylib"]`, target `wasm32-wasip2`, dep on `duga-plugin-abi` with `guest` feature
  2. Implement `Component` struct
  3. `impl tool::Host for Component`: `info()` → return ToolInfo, `execute()` → return Outcome
  4. Build: `cargo build --target wasm32-wasip2 --release`
  5. Verify: `file target/wasm32-wasip2/release/format_code.wasm` → WASM binary
- **Edge cases:**
  - `execute()` receives pre-validated JSON — but the plugin should still handle parse errors gracefully
  - Plugin name must not collide with built-in tools
- **Definition of Done:** Plugin builds for wasm32-wasip2
- **Acceptance criteria:**
  - `cargo build --target wasm32-wasip2 --release` produces .wasm file
  - .wasm file is a valid component model module (tested in TASK-11.4)
- **Test plan:** build test only (no runtime test until TASK-11.5)
- **Estimated effort:** 4 hours

---

### TASK-11.3: WasmPluginAdapter implements Tool

- **§SPEC:** §28 (Plugins implement Tool), §29 (WASI capabilities)
- **Labels:** `layer/wasm`, `priority/critical`
- **Description:** Implement `WasmPluginAdapter` in `duga-plugin-host` that wraps a wasmtime `Component` and implements the `Tool` trait. The adapter stores: `name`, `description`, `args_schema_json`, `component: Arc<wasmtime::component::Component>`, `engine: Arc<wasmtime::Engine>`, `capabilities: WasiCapabilities`. `execute()` creates a fresh `Store` per invocation, calls the plugin's `execute` export, and maps the result to `ToolResult`.
- **Files affected:**
  - `crates/duga-plugin-host/Cargo.toml` (new)
  - `crates/duga-plugin-host/src/adapter.rs` (new)
  - `crates/duga-plugin-host/src/lib.rs` (new)
- **Types involved:** `WasmPluginAdapter`, `Tool`, `ToolResult`, `ToolError`, `wasmtime::Engine`, `wasmtime::component::Component`
- **Functions to implement:**
  - `WasmPluginAdapter::new(name, desc, schema, component, engine, capabilities) -> Self`
  - `impl Tool for WasmPluginAdapter`
- **Dependencies:** TASK-11.1 (host bindings), TASK-3.1 (Tool trait), TASK-3.5 (ErasedTool)
- **Implementation steps:**
  1. Create `duga-plugin-host` crate
  2. Define `WasiCapabilities` struct (workspace_fs, network, process_spawn — all bool, default all false)
  3. Implement `WasmPluginAdapter`
  4. `execute()`:
      - Create `wasmtime::Store` with WASI context from capabilities
      - Preopen workspace dir at FD 3 via `wasi::preopens` in linker
      - Serialize args to JSON string
      - Call plugin `execute(invocation { args_json, workspace_root })`
      - Parse `Outcome` → build `ToolResult`
  5. Handle errors: plugin panics (trap) → `ToolError::Plugin`, plugin returns Err → `ToolError::Plugin`
- **Edge cases:**
  - `execute()` is synchronous (wasmtime call blocks) — fast (<1ms) for typical plugins; no spawn_blocking needed
  - Store per call: ensures isolation, but costs ~50-100µs per creation
  - Workspace preopen: wasmtime_wasi's `preopened_dir` must point to the workspace Dir
- **Definition of Done:** Adapter compiles and works
- **Acceptance criteria:**
  - Plugin execute returns ToolResult with correct output
  - Plugin trap → `ToolError::Plugin("plugin trapped: ...")`
- **Test plan:** unit: Load format-code wasm, call execute
- **Estimated effort:** 8 hours

---

### TASK-11.4: load_plugins — enumerate + validate .wasm files

- **§SPEC:** §30 (Plugin Loading)
- **Labels:** `layer/wasm`, `priority/critical`
- **Description:** Implement `load_plugins(dir: &Path, config: &PluginConfig) -> Result<Vec<Box<dyn Tool>>, PluginError>`. Enumerate `*.wasm` files in the plugin directory (non-recursive). Validate each is a WASI Preview 2 component (attempt `Component::new(&engine, &wasm_bytes)` — rejects core modules). Instantiate component, call `info()` export to get name/description/schema. Check name collision with built-in tools → warn + skip. Return Vec of `WasmPluginAdapter` as `Box<dyn Tool>`.
- **Files affected:**
  - `crates/duga-plugin-host/src/loader.rs` (new)
  - `crates/duga-plugin-host/src/lib.rs` (add module)
- **Types involved:** `PluginRegistry`, `WasmPluginAdapter`, `PluginError`
- **Functions to implement:**
  - `pub fn load_plugins(dir: &Path, config: &PluginConfig, workspace: &Workspace) -> Result<Vec<Box<dyn Tool>>, PluginError>`
  - `PluginError` enum: `NotAComponent(PathBuf)`, `InfoFailed(PathBuf, String)`, `NameCollision(String)`, `LoadFailed(PathBuf, anyhow::Error)`
- **Dependencies:** TASK-11.3, TASK-2.1
- **Implementation steps:**
  1. Enumerate `dir/*.wasm` via `std::fs::read_dir`
  2. For each: read bytes, `Component::new(&engine, &bytes)` — on error → `PluginError::NotAComponent`
  3. Create linker with WASI, instantiate, call `info()`
  4. Check name against built-in list (passed as parameter or static set)
  5. Build `WasmPluginAdapter`, push to vec
  6. Log warnings for skipped/collided plugins
- **Edge cases:**
  - Empty plugin dir → empty vec (no error)
  - Dir doesn't exist → `PluginError::LoadFailed`
  - Non-component WASM (.wasm core module) → rejected by Component::new
  - File is not WASM → `Component::new` fails → `NotAComponent`
- **Definition of Done:** Plugins load correctly
- **Acceptance criteria:**
  - load_plugins finds format-code.wasm, returns WasmPluginAdapter
  - Core .wasm module → PluginError::NotAComponent
  - Name collision → warning + skip
  - Empty dir → empty vec
- **Test plan:** unit: Create test dir with format-code.wasm + bad file; test loading
- **Estimated effort:** 5 hours

---

### TASK-11.5: Plugin instantiation + info() call

- **§SPEC:** §30 (Loading flow steps 3–4), §28.1 (info() export)
- **Labels:** `layer/wasm`, `priority/critical`
- **Description:** Within `load_plugins()`, after creating the Component, instantiate it with a WASI context, call the `info()` export, and extract `name`, `description`, `args_schema`. The `args_schema` is a JSON string — parse it to `serde_json::Value`. If `info()` fails (trap or returns error), return `PluginError::InfoFailed`.
- **Files affected:**
  - `crates/duga-plugin-host/src/loader.rs` (enhance)
- **Types involved:** `Component`, `Store`, `ToolInfo`, `PluginError`
- **Dependencies:** TASK-11.4
- **Implementation steps:**
  1. Build `wasmtime_wasi::WasiCtxBuilder` with minimal capabilities
  2. Create Store, instantiate component with WASI
  3. Get `tool` interface from instance
  4. Call `interface.call_info(&mut store)` → parse ToolInfo
  5. Parse `args_schema` string → JSON Value (validate it's valid JSON Schema)
  6. Return WasmPluginAdapter
- **Edge cases:**
  - `args_schema` is invalid JSON → `PluginError::InfoFailed`
  - `args_schema` is valid JSON but not valid JSON Schema → allow (host validates at dispatch time)
  - `name` is empty string → reject
- **Definition of Done:** info() called successfully
- **Estimated effort:** 4 hours

---

### TASK-11.6: WASI capability gating per plugin config

- **§SPEC:** §29 (WASI Capability Matrix)
- **Labels:** `layer/wasm`, `priority/critical`
- **Description:** Enforce WASI capabilities per plugin config. The per-plugin `WasiCapabilities` struct controls which WASI interfaces are available: workspace filesystem (always if `workspace_fs: true`), network (denied by default), process spawn (denied by default), randomness (always), clocks (always). Build the `WasiCtx` and `Linker` accordingly. If a plugin attempts to use a denied capability → wasmtime traps → `ToolError::Plugin`.
- **Files affected:**
  - `crates/duga-plugin-host/src/capabilities.rs` (new)
  - `crates/duga-plugin-host/src/adapter.rs` (use capabilities)
- **Types involved:** `WasiCapabilities`, `WasiCtx`, `Linker`
- **Functions to implement:**
  - `WasiCapabilities::build_ctx(&self, workspace_dir: Dir) -> WasiCtx`
  - `WasiCapabilities::configure_linker(&self, linker: &mut Linker)`
- **Dependencies:** TASK-11.5
- **Implementation steps:**
  1. Define `WasiCapabilities` struct with `workspace_fs`, `network`, `process_spawn`, `randomness`, `clocks` (all bool, defaults match SECT 29)
  2. `build_ctx()`: create WasiCtx with preopened workspace dir at FD 3 if workspace_fs
  3. `configure_linker()`: add WASI functions based on allowed capabilities
  4. If capability denied and plugin calls it → wasmtime trap → `ToolError::Plugin`
  5. Test: plugin that tries to open /etc/passwd → denied
- **Edge cases:**
  - Network capability default is false, but clock + randomness are true → these are WASI capabilities too
  - Plugin uses `wasi:sockets` → denied unless explicitly configured (not in default matrix)
- **Definition of Done:** Capabilities enforced
- **Acceptance criteria:**
  - workspace_fs: true → plugin can read/write workspace
  - workspace_fs: false → plugin cannot access filesystem
  - network: false (default) → plugin network calls trap
  - process_spawn: false (default) → plugin spawn calls trap
- **Test plan:** unit: Load plugin with different cap configs, test allowed/denied operations
- **Estimated effort:** 6 hours

---

### TASK-11.7: Plugin execute with fresh Store per call

- **§SPEC:** §28 (isolated — separate Store per invocation)
- **Labels:** `layer/wasm`, `priority/critical`
- **Description:** In `WasmPluginAdapter::execute()`, create a fresh `Store<WasiCtx>` for each call. Build the WASI context from the plugin's capabilities + workspace. Pass the `Invocation { args_json, workspace_root }` to the plugin's `execute` export. Parse the `Outcome` and convert to `ToolResult`. Apply the same `OutputLimits` and timeout as built-in tools (but plugin output is just a string, not stdout/stderr).
- **Files affected:**
  - `crates/duga-plugin-host/src/adapter.rs` (execute impl)
- **Types involved:** `Store`, `Outcome`, `ToolResult`, `OutputLimits`
- **Dependencies:** TASK-11.6
- **Implementation steps:**
  1. `execute()`: create Store from capabilities
  2. Build linker, instantiate component
  3. Get `tool` interface, call `call_execute(&mut store, invocation)`
  4. Parse `Result<Outcome, String>`:
      - Ok(outcome) → `ToolResult { success: outcome.success, output: outcome.output, metadata: parse outcome.metadata_json, ... }`
      - Err(e) → `ToolError::Plugin(e)`
  5. Enforce OutputLimits: truncate output if too long
  6. Enforce timeout: `tokio::time::timeout` wrapping the sync wasmtime call in `spawn_blocking`
- **Edge cases:**
  - `metadata_json` is invalid JSON → set metadata to `json!({"raw": metadata_json})`
  - Plugin execution takes >1s → timeout triggers (wasmtime fuel may limit this too — GAP G10)
  - Fresh Store per call: prevents state leakage between invocations
- **Definition of Done:** Execute works with fresh Store
- **Acceptance criteria:**
  - Plugin returns Outcome { success: true, output: "formatted", metadata_json: "..." } → correct ToolResult
  - Plugin returns Err → `ToolError::Plugin`
  - OutputLimits enforced on plugin output
- **Test plan:** unit: Load format-code, call execute with various inputs
- **Estimated effort:** 5 hours

---

### TASK-11.8: Plugin capability enforcement tests

- **§SPEC:** §29 (capability matrix enforcement)
- **Labels:** `layer/wasm`, `priority/critical`
- **Description:** Create tests that verify each WASI capability restriction. Create a test plugin that attempts each disallowed operation (read /etc/passwd, open socket, spawn process) and verify it traps with a clear error. Create a test plugin that uses allowed operations (workspace read/write, random, clock) and verify it succeeds.
- **Files affected:**
  - `crates/duga-plugin-host/tests/capability_tests.rs` (new)
  - `plugins-examples/test-caps/` (new test plugin)
- **Types involved:** `WasiCapabilities`, `WasmPluginAdapter`
- **Dependencies:** TASK-11.7
- **Implementation steps:**
  1. Create test-caps plugin with multiple exports testing each capability
  2. Test workspace_fs=true: plugin writes/reads file → success
  3. Test workspace_fs=false: plugin open fails → trap
  4. Test network=false: plugin HTTP request fails → trap
  5. Test process_spawn=false: plugin Command fails → trap
- **Acceptance criteria:** All capability tests pass
- **Test plan:** integration tests as described
- **Estimated effort:** 5 hours

---

### TASK-11.9: Plugin cancellation — fuel or epoch

- **§SPEC:** §26 (Cancellation), §28 (plugin cancellation), GAP G10
- **Labels:** `layer/wasm`, `priority/high`
- **Description:** Implement plugin cancellation. GAP G10: use wasmtime fuel metering (deterministic, but may not interrupt infinite loops) as the primary mechanism, with epoch interruption (wall-clock based, non-deterministic) as an optional fallback. Set fuel to a generous limit (e.g., 1 billion fuel units = ~10M instructions). When `ToolContext.cancellation.is_cancelled()`, either let fuel run out or trigger epoch increment.
- **Files affected:**
  - `crates/duga-plugin-host/src/adapter.rs` (fuel config)
- **Types involved:** `wasmtime::StoreLimits`, `CancellationToken`
- **Dependencies:** TASK-11.7, TASK-3.2 (ToolContext.cancellation)
- **Implementation steps:**
  1. Configure `StoreLimits` with fuel: `engine.config().consume_fuel(true); store.set_fuel(1_000_000_000)`
  2. On cancellation → `store.set_fuel(0)` (plugin will trap on next fuel check)
  3. Alternative: use epoch interrupt + `engine.increment_epoch()` from cancellation task
  4. Test with infinite-loop plugin
- **Edge cases:**
  - Fuel metering adds ~5% overhead — acceptable for security
  - Plugin in host function call (e.g., WASI file read) → fuel not consumed during host call; epoch interrupt works though
  - Infinite loop without any imports (pure computation) → fuel catches this
- **Definition of Done:** Cancellation stops plugins
- **Acceptance criteria:**
  - Cancel token triggered during plugin execute → plugin stops within 100ms
  - Normal plugin execution unaffected by fuel metering
- **Test plan:** unit: Infinite-loop plugin, cancel after 100ms
- **Estimated effort:** 5 hours

---

### TASK-11.10: Plugin name collision with built-in tools

- **§SPEC:** §30 (Loading flow step 5 — name collision)
- **Labels:** `layer/wasm`, `priority/high`
- **Description:** In `load_plugins()`, after calling `info()` and getting the plugin's name, check against the set of built-in tool names (`["read", "write", "shell", "search", "think"]`). If collision → emit `tracing::warn!` and skip the plugin (do not register). The built-in tool always takes precedence.
- **Files affected:**
  - `crates/duga-plugin-host/src/loader.rs` (add collision check)
- **Types involved:** `PluginError::NameCollision`
- **Dependencies:** TASK-11.5
- **Implementation steps:**
  1. Define `BUILTIN_NAMES: &[&str] = &["read", "write", "shell", "search", "think"]`
  2. After `info()`: check `tool_info.name` against built-in list
  3. If collision → warn + continue (skip)
  4. Also check against already-loaded plugin names → warn + skip
  5. Test with a plugin named "read"
- **Edge cases:**
  - Case-sensitive comparison
  - Trailing/leading whitespace in plugin name → reject or trim? → Reject with warning
- **Definition of Done:** Name collision detected and handled
- **Accepted criteria:**
  - Plugin named "read" → skipped with warning
  - Plugin named "format-code" → registered (no collision)
  - Two plugins named "duplicate" → second skipped
- **Test plan:** unit: Load plugin with colliding name
- **Estimated effort:** 3 hours
