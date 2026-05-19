# `duga` — Implementation-Grade Execution Plan

Bound strictly to **SPEC.md v1.2 (2026-05-10)**. Anything not pinned by the spec is marked **GAP(Gxx)** with a clarifying question. No invented architecture.

---

## 0. GAPs that must be answered before the affected code is merged

| # | Topic | Spec ref | Question |
|---|-------|----------|----------|
| **G1** | Tool-call concurrency in one assistant turn | SECT 5 (`for call in ...`) | Is sequential **mandated**, or illustrative? Affects event ordering, replay determinism, and `Workspace` aliasing. |
| **G2** | `EventSink::emit` semantics | SECT 6b ("non-blocking, fan-out") vs `async fn emit` | May `emit` `.await` a bounded send, or must it be wait-free? `MultiSink` queue bound? Drop policy on overflow? |
| **G3** | `LlmTokenDelta` <-> `LlmResponse` ordering | SECT 5.2, SECT 24, SECT 25 | Must every `LlmTokenDelta` for turn N have `seq` < `LlmResponse` for turn N? Required for replay reconstruction. |
| **G4** | Replay execution model | SECT 25 | Does v1.0 ship a `duga-replay` *binary* that re-runs JSONL, or only the format? |
| **G5** | `search` implementation | SECT 11 + SECT 16 | In-process `regex`/`grep-walker` over `cap_std`, or shell-out to `rg`? Spec says "matches ripgrep's default engine" but not "is ripgrep". |
| **G6** | `count_tokens` sync vs async | SECT 6a | Anthropic's exact tokenizer is HTTP. Should `count_tokens` be `async`, or is a local approximation acceptable? |
| **G7** | `Workspace` concurrent access | SECT 13, SECT 10 | `&Workspace` shared by tools — concurrent reads/writes through the same `Dir`? Linked to G1. |
| **G8** | `write` per-path mutex | SECT 11 (`write`) | Keyed on raw arg or canonicalized workspace-relative path? Map lifetime = agent run? |
| **G9** | Symlink default for `read`/`write` | SECT 14 | "By default not followed" — open fails, or symlink itself is read as a special object? |
| **G10** | Plugin cancellation mechanism | SECT 28, SECT 26 | Wasmtime **fuel** (deterministic) or **epoch interruption** (wall-clock)? |
| **G11** | `ShellSession` lifecycle | SECT 19 | How is a session created? Implicit on first `session=<uuid>` use, or explicit constructor? |
| **G12** | `ThinkLimits.max_tokens` accounting | SECT 12 | Counted on `thought` text, or on assistant tokens that produced the call, or both? |
| **G13** | Memory tokenizer source | SECT 21 | Does `Memory` hold `Arc<dyn LlmClient>`, or a separated `Arc<dyn Tokenizer>`? |
| **G14** | Redaction & `seq` stability | SECT 24.1, SECT 25 | Pre- or post-serialization redaction? Does `seq` remain stable across redacted vs non-redacted? |
| **G15** | Hot reload of plugins | not in spec | Confirmed post-MVP optional. SECT 30 changes sketched in Phase 14. |
| **G16** | Windows / macOS `RESOLVE_BENEATH` equivalent | SECT 14 | Confirm cap_std's defaults (`NtCreateFile`+`OBJ_DONT_REPARSE` on Windows, `O_NOFOLLOW`+manual checks on macOS) are the chosen mechanism. |
| **G17** | Provider conformance suite | SECT 6a | Mandated cross-provider conformance fixtures? |

Every code path that depends on a GAP carries a `// GAP(Gxx)` marker and is gated until resolved.

---

## 1. Cargo workspace tree

```
duga/
  Cargo.toml                        # virtual manifest, [workspace] only
  rust-toolchain.toml               # pinned channel
  deny.toml                         # cargo-deny: licenses, advisories, bans
  .cargo/config.toml                # per-target rustflags + linker
  crates/
    duga-types/                     # data only (no logic, no I/O, no tokio)
    duga-config/                    # YAML -> typed Config
    duga-events/                    # EventSink, MultiSink, JsonlSink, Redactor
    duga-llm/                       # LlmClient trait + ProviderRegistry
      duga-llm-openai/              # OpenAI + OpenRouter
      duga-llm-anthropic/           # Anthropic Messages API
      duga-llm-ollama/              # Ollama /api/chat
    duga-sandbox/                   # Workspace, Dir wrapper, exec, env scrub, BinaryRegistry
    duga-tools/                     # Tool trait + Dispatcher + schema pipeline
      duga-tools-builtin/           # shell, read, write, search, think
    duga-plugin-abi/                # WIT file + wit-bindgen output (host & guest)
    duga-plugin-host/               # wasmtime loader, WASI cap binder, Tool adapter
    duga-core/                      # AgentLoop, Memory, Summarizer trait, AgentError
    duga-tui/                       # ratatui TuiSink (feature-gated)
    duga-replay/                    # JSONL reader, MockLlm, MockToolDispatcher, replay bin
    duga-harness/                   # bin: ./harness -- composition root only
  plugins-examples/
    format-code/                    # wasm32-wasip2 example guest
  tests/
    golden/                         # *.jsonl replay fixtures
    e2e/                            # cargo test, no live LLM
  xtask/                            # cargo-xtask: build, package, cross
  ci/                               # GH Actions workflows
```

---

## 2. ASCII dependency graph (compile-time)

:: Still composing the full plan. Saving progress...

```
                       +------------------------+
                       |   duga-harness         |  bin (composition root)
                       +------------+-----------+
                                    | depends on every layer below
        +-----------+---------------+-------+----------------+---------------+
        v           v                       v                v               v
   duga-core   duga-tools-builtin    duga-plugin-host   duga-llm-*      duga-replay
        |           |                       |                |               |
        v           v                       v                v               v
   duga-tools  duga-tools             duga-tools        duga-llm       duga-events
        |       duga-sandbox          duga-plugin-abi      |               |
        v       duga-events           duga-events          v               v
   duga-events  duga-types            duga-sandbox      duga-events    duga-types
        |                             duga-types        duga-types
        v
   duga-types

   duga-config --> duga-types       (read by harness only; not by core)
   duga-plugin-abi --> (no internal deps; wit-bindgen only)
                       Used by:  duga-plugin-host (host bindings)
                                 plugins-examples/* (guest bindings)
```

**Verified properties:**
- `duga-core` does **not** depend on wasmtime, ratatui, reqwest, serde_yaml.
- `duga-types` is the only crate every layer imports; it is pure data.
- `duga-plugin-abi` is leaf; both host and guest depend on it (the only crate that crosses the host/guest boundary).
- `duga-tools-builtin` does not depend on `duga-core` (no upward edge).
- `duga-harness` is the only place where concrete providers, sinks, and the loop are stitched together.

## 3. Runtime ownership graph (who owns what at runtime)

```
                 main (duga-harness)
                       | owns
                       v
               +---------------+
               |  AgentLoop    |  (duga-core)
               +-------+-------+
                       | owns by value
   +-------------------+-------------------+
   | owns Arc<dyn LlmClient>               | owns ToolDispatcher
   v                                       v
LlmClient impl                       ToolDispatcher (duga-tools)
(in duga-llm-*)                            | owns Vec<Box<dyn Tool>>
                                           |
                                           +--> ShellTool, ReadTool, WriteTool, SearchTool, ThinkTool
                                           +--> WasmPluginAdapter (one per plugin)
                                                     | owns wasmtime::Component (Arc, shared)
                                                     | creates a fresh Store per call
                                                     v
                                              wasmtime::Store<Wasi>

AgentLoop also owns by value:
   - Memory                         (duga-core)        -- exclusive owner
   - CancellationToken              (held; child tokens passed to tools)
   - Arc<dyn EventSink>             (cloneable; passed by &dyn into LlmClient/Tool)
   - Arc<dyn Summarizer>            (cloneable; passed by &dyn into Memory::compress)
   - Arc<Workspace>                 -- Arc because both tools and ToolContext borrow

EventSink fan-out:
   Arc<MultiSink>
       |
       +-- Arc<JsonlSink>     (owns mpsc::Sender<Event>; writer task owns the file)
       +-- Arc<TuiSink>       (owns mpsc::Sender; render task owns terminal)
       +-- Arc<NullSink>      (no state)

Plugins:
   PluginRegistry owns: HashMap<String, Arc<wasmtime::Engine>> + Arc<Component>s.
   Each invocation: fresh Store<WasiCtx> built from per-plugin WasiCapabilities.
```

**Justification for every Arc/Mutex:**

| Type | Where | Why required | Alternatives rejected |
|------|-------|--------------|----------------------|
| `Arc<dyn LlmClient>` | AgentLoop | passed `&dyn` to Memory and inside `tokio::select!`; must outlive the borrow. | `Box<dyn>` would prevent sharing with Memory. |
| `Arc<dyn EventSink>` | AgentLoop | cloned into `ToolContext` (owned by tool call futures); `&dyn` cannot be stored in `ToolContext<'a>` across `.await`. | `&'a dyn` forces a lifetime parameter on `handle_tool_call` that conflicts with the loop borrow. |
| `Arc<dyn Summarizer>` | AgentLoop | same reason as EventSink; must outlive async boundaries. | -- |
| `Arc<Workspace>` | AgentLoop | needed by both `ToolDispatcher` (stores it) and Memory (for path normalization). | Rc is !Send; Box prevents sharing. |
| `Arc<wasmtime::Engine>` | PluginRegistry | Engine is expensive to create; shared across all invocations. Must be `Send + Sync` for concurrent use (G1). | Creating an Engine per-call is 200ms+ overhead. |
| `Mutex<HashMap<Uuid, ShellSession>>` | ShellTool | Sessions are mutated by `cd`/`export`/`unset` across concurrent tool calls (if G1 -> parallel). | `RwLock` adds no value since mutations dominate. |

**No `Arc<Mutex<Memory>>`**: Memory is owned exclusively by `AgentLoop::run`, never shared. No `Arc<Mutex<Event>>` either -- events flow through bounded channels, never shared state.

---

## 4. Async boundary graph

| Computation | Blocking (spawn_blocking) | Cancellation boundary |
|---|---|---|
| AgentLoop::run | None | CancellationToken |
| LlmClient::chat | DNS/TLS/HTTP (reqwest = async) | select! with cancel |
| ToolDispatcher::dispatch | | |
| ShellTool::execute | waitpid (tokio::process) | ToolContext.cancellation |
| ReadTool::execute | cap_std read (spawn_blocking) | same |
| WriteTool::execute | cap_std write (spawn_blocking) | same |
| SearchTool::execute | recursive walk (spawn_blocking) | same |
| ThinkTool::execute | none (pure CPU) | same |
| WasmPlugin::execute | wasmtime call (sync, fast) | fuel/epoch (G10) |
| Memory::compress | LlmClient::chat (async) | CancellationToken |
| EventSink::emit | bounded mpsc::send | none needed |

**Send / Sync requirements:**

| Type | Bound |
|------|-------|
| `dyn LlmClient` | Send + Sync |
| `dyn EventSink` | Send + Sync |
| `dyn Tool` | Send + Sync |
| `ToolContext<'a>` | Send (used inside `.await` in handle_tool_call) |
| `ToolResult` | Send |
| `Event` | Send |

---

## 5. Per-crate specification

### 5.1 `duga-types` -- `crates/duga-types/`

**Purpose:** every wire and on-the-loop datatype. **Zero logic, zero I/O, zero tokio.**

**Public API surface:**
- Message, Role, ContentBlock enum
- ToolCall (id: Uuid, tool: String, raw_args: serde_json::Value)
- ToolSchema (name, description, args_schema)
- ToolResult (tool_call_id, success, output, metadata, duration_ms, stdout_bytes, stderr_bytes, truncated)
- LlmCallOptions, LlmResponse, AssistantMessage, TokenUsage
- Event enum (all variants from SECT 24)
- AgentConfig, AgentLimits, AgentFeatures, OutputLimits, ThinkLimits
- SummaryMessage (content, pinned_facts)
- Seq(newtype u64)
- ToolError (with is_transient(): Timeout|Cancelled|Denied|InvalidArgs|Io|Plugin|OutputLimitExceeded)
- AgentError (MaxStepsReached|MaxToolCallsReached|Timeout|Cancelled|ContextOverflow|SummarizerFailed)

**Dependencies:** serde (derive), serde_json, schemars v0.8, uuid (v4), time, thiserror.

**What should NOT be public:** Raw field access that breaks invariants. Use builder methods where needed.

### 5.2 `duga-config` -- `crates/duga-config/`

**Purpose:** YAML -> typed Config. ~ expansion via $HOME (no shell). Validates allowed_binaries existence, warns on secret-pattern matches (SECT 20).

**Public:** Config struct, SandboxConfig, WorkspaceConfig, EnvironmentConfig, MemoryConfig, PluginConfig, WasiCapabilities. fn load(path: &Path) -> Result<Config, ConfigError>.

**Deps:** serde_yaml, duga-types, which, cap_std, tracing.

### 5.3 `duga-llm` -- `crates/duga-llm/`

**Purpose:** the **only** provider boundary (SECT 6a). Trait, error type, and registry.

**Public:**
- trait LlmClient: Send + Sync { async fn chat(...), fn count_tokens(...) }
- ProviderRegistry, ProviderFactory trait
- LlmError

**Deps:** duga-types, duga-events, async-trait.

### 5.4 `duga-llm-openai/anthropic/ollama` -- provider implementations

Each provides a ProviderFactory. Deps: duga-llm, reqwest, tokio, serde_json, tracing.

### 5.5 `duga-events` -- `crates/duga-events/`

**Purpose:** EventSink trait, concrete sinks, SeqAllocator, Redactor.

**Public:**
- trait EventSink: Send + Sync { async fn emit(&self, event: &mut Event); }
- SeqAllocator(AtomicU64) -- monotonic per run
- JsonlSink (spawns writer task, mpsc::UnboundedSender)
- MultiSink (Vec<Arc<dyn EventSink>>)
- NullSink
- Redactor { patterns: Vec<Regex> }

**Deps:** duga-types, serde_json, tokio, regex, tracing.

### 5.6 `duga-sandbox` -- `crates/duga-sandbox/`

**Purpose:** Workspace (cap_std wrapper), ShellSession, BinaryRegistry, env scrubber, run_captured().

**Public:**
- Workspace { root_dir: cap_std::fs::Dir, root_path: PathBuf }
- BinaryRegistry (resolve at startup, exec only by absolute path)
- ShellSession (id, cwd, env) + SessionCommand enum (Cd, Export, Unset, Pwd, Spawn)
- SanitizedEnv
- async fn run_captured(binary, args, workspace, session, env, limits, cancellation) -> Result<ToolResult, ToolError>

**Deps:** duga-types, cap_std, tokio, which, tracing.

### 5.7 `duga-tools` -- `crates/duga-tools/`

**Purpose:** Tool trait, ToolDispatcher, schema validation pipeline (SECT 8).

**Public:**
- ToolDispatcher { tools, schema_index }
  - fn schemas() -> Vec<ToolSchema>
  - async fn dispatch(&self, call: &ToolCall, ctx: ToolContext) -> Result<ToolResult, ToolError>
  - fn get(&self, name: &str) -> Option<&dyn Tool>

**Schema pipeline (inside dispatch):**
1. Parse: validate call.raw_args is valid JSON
2. Validate: run jsonschema against tool's schema
3. Deserialize: serde_json::from_value::<T::Args>(validated_value)
4. Execute: tool.execute(ctx, args).await

**Deps:** duga-types, duga-events, duga-sandbox, jsonschema, serde_json.

### 5.8 `duga-tools-builtin` -- `crates/duga-tools-builtin/`

**Purpose:** shell, read, write, search, think -- the five required tools.

**Public structs:** ShellTool, ReadTool, WriteTool, SearchTool, ThinkTool.

**Key details:**
- ShellTool: uses Sandbox::run_captured + ShellSession::classify for session-state commands
- ReadTool: cap_std read, NUL-byte sniffing (first 8 KiB), spawn_blocking
- WriteTool: atomic write via tempfile + rename, per-path mutex
- SearchTool: walkdir + regex in spawn_blocking (GAP G5)
- ThinkTool: no-op, output = thought echoed back

**Deps:** duga-tools, duga-sandbox, duga-types, walkdir, regex, tempfile.

### 5.9 `duga-plugin-abi` -- `crates/duga-plugin-abi/`

**Purpose:** WIT file (duga:plugin@0.1.0) + wit-bindgen generated bindings for host and guest.

**Structure:**
- wit/plugin.wit (exact content from SECT 28.1)
- src/lib.rs (empty, re-exports)
- Features: host ("wit-bindgen/host"), guest ("wit-bindgen/rust")

**Deps:** wit-bindgen only.

### 5.10 `duga-plugin-host` -- `crates/duga-plugin-host/`

**Purpose:** wasmtime loader, WASI capability gating, Tool adapter.

**Public:**
- PluginRegistry { engine: Arc<wasmtime::Engine>, plugins: Vec<WasmPluginAdapter> }
- WasmPluginAdapter implements Tool
- fn load_plugins(dir, config, workspace) -> Result<PluginRegistry, PluginError>

**Loading flow (SECT 30):**
1. Enumerate dir/*.wasm
2. Validate component model (reject core modules)
3. Instantiate with WASI context from WasiCapabilities
4. Call info() -> register
5. Name collision with built-in -> warn and skip

**Cancellation:** GAP G10 (fuel or epoch).

**Deps:** duga-plugin-abi (host), duga-tools, duga-sandbox, wasmtime (component-model, wasi-preview2).

### 5.11 `duga-core` -- `crates/duga-core/`

**Purpose:** AgentLoop, Memory, Summarizer trait.

**Public:**
- AgentLoop { run(&mut self, task: String) -> Result<String, AgentError> }
- Memory { push_user, push_assistant, push_tool_result, messages, over_budget, compress }
- trait Summarizer: Send + Sync { async fn summarize(&self, messages: &[Message]) -> Result<SummaryMessage> }
- Core loop (SECT 5): check limits -> LLM call (cancellable) -> push assistant -> if no tools, return -> execute tools sequentially -> push results -> compress if over budget

**Deps:** duga-types, duga-llm, duga-tools, duga-events, duga-sandbox, tokio, tokio-util, tracing.

### 5.12 `duga-tui` -- `crates/duga-tui/` (feature-gated)

TuiSink via ratatui + crossterm. Feature-gated behind `tui`.

### 5.13 `duga-replay` -- `crates/duga-replay/`

JSONL reader, MockLlm, MockToolDispatcher, replay binary.
- fn read_jsonl(path: &Path) -> Result<Vec<StoredEvent>, ReplayError>
- MockLlm(VecDeque<LlmResponse>) -- deterministic
- MockToolDispatcher -- returns stored ToolResults by call id

### 5.14 `duga-harness` -- `crates/duga-harness/` (bin)

Composition root. Zero business logic.
1. Parse CLI args (clap)
2. Config::load
3. Workspace::open, BinaryRegistry::new, SanitizedEnv::build
4. Build ProviderRegistry + LlmClient
5. Build builtin tools + load plugins
6. Build ToolDispatcher
7. Build EventSinks -> MultiSink
8. Setup CancellationToken (sigint)
9. Build Memory + Summarizer
10. Build AgentLoop -> run(task)



---

## 6. Complete dependency table

| Crate | Deps | Features |
|-------|------|----------|
| duga-types | serde, serde_json, schemars, uuid, time, thiserror | -- |
| duga-config | serde_yaml, duga-types, which, cap_std, tracing | -- |
| duga-events | duga-types, tokio, regex, serde_json, tracing | -- |
| duga-llm | duga-types, duga-events, async-trait | -- |
| duga-llm-openai | duga-llm, reqwest, tokio, serde_json, tracing | -- |
| duga-llm-anthropic | duga-llm, reqwest, tokio, serde_json, tracing | -- |
| duga-llm-ollama | duga-llm, reqwest, tokio, serde_json, tracing | -- |
| duga-sandbox | duga-types, cap_std, tokio, which, tracing | -- |
| duga-tools | duga-types, duga-events, duga-sandbox, serde_json, jsonschema | -- |
| duga-tools-builtin | duga-tools, duga-sandbox, duga-types, walkdir, regex, tempfile | -- |
| duga-plugin-abi | wit-bindgen | host, guest |
| duga-plugin-host | duga-plugin-abi (host), duga-tools, duga-sandbox, wasmtime | -- |
| duga-core | duga-types, duga-llm, duga-tools, duga-events, tokio, tokio-util | -- |
| duga-tui | ratatui, crossterm, duga-events | tui |
| duga-replay | duga-core, duga-types, duga-events, clap, serde_json | -- |
| duga-harness | all of the above, clap | tui (optional) |

---

## 7. Dependency evaluation (key choices)

| Crate | Version | Why | Rejected alternatives |
|-------|---------|-----|----------------------|
| tokio | 1.40+ (full) | Standard async runtime. tokio::process::Command required by spec (SECT 17). | smol, async-std: smaller but missing process::Command parity. |
| tokio-util | 0.7+ | CancellationToken (SECT 26). | Roll-your-own token: spec explicitly says CancellationToken. |
| serde | 1.0+ (derive) | Industry standard. Required by spec for all serialization. | miniserde, rkyv: lack JsonSchema support. |
| schemars | 0.8+ | JSON Schema derive required by spec (SECT 7). | jsonschema (validation only, no derive). Both are complementary. |
| reqwest | 0.12+ | Async HTTP. Best-in-class streaming SSE support. | hyper (too low-level), ureq (sync only). |
| wasmtime | 25+ (component-model) | Spec mandate (SECT 28). | wasmi (interpreter, no component model). |
| wit-bindgen | 0.30+ | Spec mandate (SECT 28, WIT contract). | cargo-component (build tool, not library). |
| cap_std | 3.x | Spec mandate (SECT 13). | openat2 raw syscalls (not portable). |
| tracing | 0.1+ | Structured observability. Non-blocking, async-aware. | log (sync, no spans); slog (less ecosystem). |
| clap | 4.x | CLI arg parsing. | Manual arg parsing (error-prone). |
| jsonschema | 0.18+ | JSON Schema validation (SECT 8). | Hand-rolled validation (not spec-compliant). |
| walkdir | 2.5+ | Recursive search for search tool. | ignore (heavier), jwalk (overkill for single-thread). |
| ratatui | 0.28+ | TUI (optional). | crossterm alone (too low-level). |

---

## 8. Error taxonomy

| Error type | Layer | Retryable? | Fatal? | Replay-serializable? |
|---|---|---|---|---|
| ToolError::Io(String) | duga-tools/duga-sandbox | Yes (is_transient) | No -> tool feedback | Yes |
| ToolError::Plugin(String) | duga-plugin-host | Yes (is_transient) | No -> tool feedback | Yes |
| ToolError::Timeout | duga-tools | No | No -> tool feedback | Yes |
| ToolError::Cancelled | duga-tools | No | No -> tool feedback | Yes |
| ToolError::InvalidArgs(String) | duga-tools | No | No -> tool feedback | Yes |
| ToolError::Denied(String) | duga-sandbox | No | No -> tool feedback | Yes |
| ToolError::OutputLimitExceeded | duga-sandbox | No | No -> tool feedback | Yes |
| AgentError::MaxStepsReached | duga-core | -- | Yes (loop returns) | Yes |
| AgentError::MaxToolCallsReached | duga-core | -- | Yes | Yes |
| AgentError::Timeout | duga-core | -- | Yes | Yes |
| AgentError::Cancelled | duga-core | -- | Yes | Yes |
| AgentError::ContextOverflow | duga-core | -- | Yes | Yes |
| AgentError::SummarizerFailed(String) | duga-core | -- | Yes | Yes |
| LlmError | duga-llm-* | No | No (loop continues) | N/A (internal) |
| ConfigError | duga-config | -- | Yes (startup) | N/A |
| PluginError | duga-plugin-host | -- | Yes (startup) | N/A |
| ReplayError | duga-replay | -- | Yes | N/A |

**Propagation rule:** Every ToolError variant has a Display impl producing human-readable tool feedback. The loop appends this as a tool result message. AgentError variants terminate the loop.

---

## 9. Memory + backpressure strategy

### Event channel -- JsonlSink

**Design:** mpsc::UnboundedSender<JsonlMessage>.

**Justification:** Event emission must not block the loop (GAP G2). Total events per run is bounded by max_steps * (1 + max_tool_calls + 1). With max_steps=50, max_tool_calls=100 -> ~5,100 events. Each event is a few KB -> ~50MB total worst case. This fits in memory comfortably.

**Risk:** If max_steps is set very high (e.g. 10,000), events could grow to ~10GB. Mitigation: document this; future optimization could switch to bounded + try_send + warn on overflow.

### Memory -- owned by loop

**Ownership:** Exclusive by AgentLoop. No sharing, mutex, or channels.

**Budget check:** Memory::over_budget() is O(n) over recent_messages. Memory::compress spawns an LLM call (via Summarizer).

### Tool output truncation

OutputLimits enforced in duga-sandbox::run_captured:
- Read stdout/stderr with .take(max_bytes)
- After both streams complete, check combined total
- If exceeded, truncate formatted output, set ToolResult::truncated = true

### Replay file growth

Single file per run. Path determined at startup (replay-{timestamp}.jsonl). Self-describing (SECT 25).

---

## 10. WASM plugin system detail

### Build pipeline
```
cd plugins-examples/format-code/
cargo build --target wasm32-wasip2 --release
# Output: target/wasm32-wasip2/release/format_code.wasm

cd duga/
cargo build --release
# Output: target/release/harness
```

### WIT binding generation flow
```
duga-plugin-abi/wit/plugin.wit --[wit-bindgen]--> duga-plugin-abi/src/bindings.rs
  | (host feature)                                       | (host mode)
  v                                                      v
wasmtime::component::Component                    duga::plugin@0.1.0::tool trait
+ Component::new(&engine, &bytes)                 implemented by WasmPluginAdapter
  |
  | (guest feature -> used by plugin crates)
  v
macros to export the tool interface (info(), execute())
```

### ABI versioning strategy
- WIT package: duga:plugin@0.1.0. Bumped on breaking changes.
- Schema version: header comment in plugin.wit with changelog.
- Host rejects plugins with mismatched WIT major version.
- wasmtime provides native compatibility check.

### Hot reload (post-MVP, Phase 15)
Required changes to SECT 30:
1. PluginRegistry gains watch(dir).await (inotify/kqueue on plugin dir)
2. On file change: re-validate, re-compile, replace Arc<Component> atomically
3. Active tool executions using old component finish unmolested (held Arc)
4. Add PluginConfig.watch: bool
5. Risk: Component::new is 100-500ms -- hot-reload stalls dispatch. Mitigation: background pre-validation.

---

## 11. Testing strategy

### Unit tests

| Scope | Crate | Mock targets | What to test |
|---|---|---|---|
| Loop invariants | duga-core | MockLlm + MockToolDispatcher | steps, tool_calls, cancellation, retry, memory budget |
| Retry logic | duga-core | MockTool (returns Io N times then Ok) | retry_on_error count; non-transient not retried |
| Schema pipeline | duga-tools | -- | Valid JSON -> Ok, missing field -> InvalidArgs |
| Memory | duga-core | MockSummarizer | push, over_budget, compress, overflow dropping |
| Event redaction | duga-events | -- | Redactor patterns match; no false positives |
| ShellSession | duga-sandbox | -- | classify cd/export/unset/pwd; apply mutations |
| BinaryRegistry | duga-sandbox | which stub | known -> resolved; unknown -> Denied |
| Replay | duga-replay | MockLlm + MockToolDispatcher | roundtrip: run, emit, write JSONL, read back, replay, verify |

### Integration tests (in tests/e2e/)

Each test builds a minimal AgentLoop with fixtures, uses deterministic fake clock (tokio::time::pause + tokio::time::advance).

### E2E smoke test

**Scenario:** Agent receives:
> "Write a Rust function fibonacci(n: u64) -> u64 with unit tests. Ensure cargo test passes."

**Expected replay sequence:**
1. LlmRequest -> LlmResponse([write])
2. write(src/fib.rs, content = "fn fibonacci...")
3. LlmRequest -> LlmResponse([read])
4. read(Cargo.toml)
5. LlmRequest -> LlmResponse([write])
6. write(Cargo.toml, content = "...")
7. LlmRequest -> LlmResponse([shell])
8. shell(["cargo", "test"]) -> success, "test result: ok"
9. LlmRequest -> LlmResponse(text = "Done!")

Test uses MockLlm + MockToolDispatcher pre-loaded with this exact script.

---

## 12. Implementation phases

| Phase | Days | SPEC Sections | Crate(s) introduced | Checkpoint |
|---|---|---|---|---|
| P1 Primitives | 2 | SECT 4, 6-10 | duga-types, duga-events | ToolError, ToolResult, Event compile; NullSink works |
| P2 Config + sandbox | 2 | SECT 13-16, 18, 20, 32 | duga-config, duga-sandbox | Config::load working; Workspace::open; BinaryRegistry; run_captured |
| P3 ShellSession + env | 1 | SECT 17, 19-20 | (extends duga-sandbox) | ShellSession classify + apply; SanitizedEnv |
| P4 Tool trait + dispatcher | 2 | SECT 7-8, 10 | duga-tools | ToolDispatcher validates schema pipeline |
| P5 Built-in tools | 3 | SECT 11-12, 16-17 | duga-tools-builtin | shell, read, write, search, think compile and test |
| P6 Memory | 2 | SECT 21-23 | (extends duga-core) | Memory push, over_budget, compress, overflow rules |
| P7 LLM trait + providers | 2 | SECT 6a, 32 | duga-llm, duga-llm-openai, -anthropic, -ollama | LlmClient trait; ProviderRegistry; one provider tested |
| P8 Stream + redaction | 1 | SECT 5.2, 24.1 | (extends duga-events) | JsonlSink writes valid JSONL; Redactor passes |
| P9 Core loop | 3 | SECT 5, 26-27 | duga-core | AgentLoop with mocks; retry, cancellation, limits tested |
| P10 WASM plugin ABI | 2 | SECT 28.1, 28 | duga-plugin-abi, duga-plugin-host | WIT compiles; load_plugins tests; plugin executes |
| P11 Harness + CLI | 1 | SECT 32-33 | duga-harness | ./harness --config config.yaml runs end-to-end |
| P12 Replay | 1 | SECT 25 | duga-replay | read_jsonl -> MockLlm + MockToolDispatcher roundtrip |
| P13 TUI (optional) | 2 | SECT 6b | duga-tui | TuiSink renders live event stream |
| P14 Static builds + CI | 2 | SECT 33 | xtask, ci/ | Static binaries for Linux(musl), macOS, Windows |
| P15 Hot reload (post-MVP) | 2 | SECT 30 | (extends duga-plugin-host) | watch + atomic component swap |

**Total approx 26 days for MVP (P1-P12).**

---

## 13. Static build strategy

### Linux (x86_64-unknown-linux-musl)
```toml
# .cargo/config.toml
[target.x86_64-unknown-linux-musl]
linker = "rust-lld"
rustflags = ["-C", "target-feature=+crt-static"]

[target.aarch64-unknown-linux-musl]
linker = "rust-lld"
```

wasmtime bundles its own musl libm for cross targets.

### macOS (aarch64-apple-darwin / x86_64-apple-darwin)
```
[target.aarch64-apple-darwin]
rustflags = ["-C", "link-arg=-dead_strip"]
```
Use cargo-zigbuild for cross.

### Windows (x86_64-pc-windows-msvc)
```
[target.x86_64-pc-windows-msvc]
rustflags = ["-C", "target-feature=+crt-static"]
```

### Cross-compilation (xtask)
cargo xtask dist --target x86_64-unknown-linux-musl:
1. Install cross or use cargo-zigbuild
2. Build --release with crt-static
3. Strip symbols
4. Output: target/<target>/release/harness

---

## 14. CI matrix

```yaml
strategy:
  matrix:
    target:
      - x86_64-unknown-linux-musl
      - aarch64-unknown-linux-musl
      - x86_64-apple-darwin
      - aarch64-apple-darwin
      - x86_64-pc-windows-msvc
steps:
  - name: Build
    run: cargo xtask build --target ${{ matrix.target }}
  - name: Unit tests (host target only)
    if: matrix.target == 'x86_64-unknown-linux-gnu'
    run: cargo test --workspace
  - name: WASM plugin build test
    run: cargo build --target wasm32-wasip2 --manifest-path plugins-examples/format-code/Cargo.toml
  - name: Dist
    run: cargo xtask dist --target ${{ matrix.target }}
```

---

## 15. Replay JSONL example (fibonacci E2E)

```jsonl
{"seq":1,"ts":"2026-05-10T18:30:00.000Z","event":"LoopIteration","step":1}
{"seq":2,"ts":"2026-05-10T18:30:00.001Z","event":"LlmRequest","messages":[{"role":"system","content":[{"text":"You are a coding assistant..."}]},{"role":"user","content":[{"text":"Write a Rust function fibonacci..."}]}],"tools":[{"name":"read","description":"Read a file","args_schema":{...}},{"name":"write","description":"Write a file","args_schema":{...}},{"name":"shell","description":"Execute a command","args_schema":{...}}],"opts":{"streaming":true}}
{"seq":3,"ts":"2026-05-10T18:30:01.000Z","event":"LlmTokenDelta","text":"I'll write the fibonacci function..."}
{"seq":5,"ts":"2026-05-10T18:30:02.000Z","event":"LlmResponse","message":{"text":"I'll write the fibonacci function...","tool_calls":[{"id":"550e8400-e29b-41d4-a716-446655440000","tool":"write","raw_args":{"path":"src/fib.rs","content":"fn fibonacci(n: u64) -> u64 { ... }"}}]},"usage":{"prompt":120,"completion":85}}
{"seq":6,"ts":"2026-05-10T18:30:02.000Z","event":"ToolCallStarted","id":"550e8400-...","tool":"write","args":{"path":"src/fib.rs","content":"<REDACTED>"}}
{"seq":7,"ts":"2026-05-10T18:30:02.010Z","event":"ToolCallFinished","id":"550e8400-...","tool":"write","success":true,"output":"Wrote 210 bytes to src/fib.rs","duration_ms":10}
```

---

## 16. Schema validation pipeline example

```rust
// Inside ToolDispatcher::dispatch:
async fn dispatch(&self, call: &ToolCall, ctx: ToolContext<'_>) -> Result<ToolResult, ToolError> {
    // Step 1: Resolve tool
    let tool = self.get(&call.tool).ok_or_else(|| ToolError::InvalidArgs(
        format!("Unknown tool '{}'. Available: {}", call.tool, self.names().join(", "))
    ))?;
    // Step 2: Validate schema (jsonschema crate)
    let schema = tool.json_schema();
    let validation = jsonschema::validate(&schema, &call.raw_args);
    if let Err(errors) = validation {
        let msg = errors.map(|e| format!("{}: {}", e.instance_path, e.kind)).collect::<Vec<_>>().join("; ");
        return Err(ToolError::InvalidArgs(format!("Invalid args: {}", msg)));
    }
    // Step 3: Deserialize
    let args = serde_json::from_value::<T::Args>(call.raw_args.clone())
        .map_err(|e| ToolError::InvalidArgs(format!("Deserialization failed: {}", e)))?;
    // Step 4: Execute
    tool.execute(ctx, args).await
}
```

---

## 17. Top 5 project risks

| Risk | Impact | Likelihood | Mitigation |
|---|---|---|---|
| cap_std incompatibility with wasmtime WASI | Plugin FS breaks | Medium | Test plugin FS in integration tests early (P10). Use wasmtime_wasi::Dir directly for plugin FD 3. |
| MacOS/Windows cap_std != Linux openat2 | Path traversal on non-Linux | Medium | Use cap_std abstractions exclusively. Run CI on all 3 OS. File cap_std issues. |
| LLM provider streaming API divergence | SSE normalization bugs | High | Each duga-llm-* crate has own SSE parser; test each independently. |
| Wasmtime + cap_std + tokio cancellation | Plugin cancellation broken | Medium | P10 tests must include forced plugin cancellation. Benchmark fuel vs epoch (G10). |
| Memory budget drift after compression | Hysteresis loop | Low | Document as best-effort. SECT 22.1 overflow rules provide hard safety valve. |

---

## 18. Cargo.toml fragments

### Virtual workspace
```toml
[workspace]
resolver = "2"
members = [
    "crates/*",
    "crates/duga-llm/*",
    "crates/duga-tools-builtin",
    "plugins-examples/*",
    "xtask",
]
```

### duga-types/Cargo.toml
```toml
[package]
name = "duga-types"
version = "0.1.0"
edition = "2021"

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
schemars = { version = "0.8", features = ["uuid"] }
uuid = { version = "1", features = ["v4", "serde"] }
time = { version = "0.3", features = ["serde"] }
thiserror = "1"
```

### duga-core/Cargo.toml
```toml
[package]
name = "duga-core"
version = "0.1.0"
edition = "2021"

[dependencies]
duga-types = { path = "../duga-types" }
duga-llm = { path = "../duga-llm" }
duga-tools = { path = "../duga-tools" }
duga-events = { path = "../duga-events" }
duga-sandbox = { path = "../duga-sandbox" }
tokio = { version = "1", features = ["process", "time", "sync"] }
tokio-util = { version = "0.7", features = ["cancellation"] }
tracing = "0.1"
```

### duga-plugin-abi/Cargo.toml
```toml
[package]
name = "duga-plugin-abi"
version = "0.1.0"
edition = "2021"

[features]
host = ["wit-bindgen/host"]
guest = ["wit-bindgen/rust"]

[dependencies]
wit-bindgen = { version = "0.30", default-features = false, optional = true }

[build-dependencies]
anyhow = "1"
wit-bindgen = { version = "0.30", default-features = false }
```

---

## End of Plan


---

## 19. Phase checkpoints & completion tracking

### Rules for marking phases complete

Each phase has a concrete **checkpoint** — a binary acceptance gate. A phase is **DONE** only when its checkpoint passes. No partial credit.

**Checkpoint format for every phase:**
```
Phase N: <NAME>
  Checkpoint: <single command or test name>
  GAPs resolved: [Gxx, Gyy]
  Depends on: [P<N>]
  Signal: <path to a marker file or git tag>
```

### Checkpoint markers

After a phase is done, the developer creates an empty marker file and commits it:

```
touch .phase/P1 && git add .phase/P1 && git commit -m "P1 done: Primitives"
```

The `.phase/` directory is committed to the repo. This makes the progress machine-parseable and reviewable.

### Phase-by-phase checkpoints

```
Phase P1: Primitives
  Checkpoint: cargo test -p duga-types -p duga-events -- --quiet
  GAPs: [G6, G13, G14]  (affect count_tokens sync, Memory trait, redaction seq)
  Depends on: (none)
  Signal: .phase/P1
  What passes:
    - ToolError, ToolResult, Event, Seq, ToolSchema compile
    - NullSink compiles and discards events without panic
    - Event serializes/deserializes to JSON
    - ToolType::is_transient() returns correct values
    - SeqAllocator returns monotonic increasing seqs

Phase P2: Config + sandbox
  Checkpoint: cargo test -p duga-config -p duga-sandbox -- --quiet
  GAPs: [G5, G16]
  Depends on: [P1]
  Signal: .phase/P2
  What passes:
    - Config::load("test.yaml") parses the full spec-compliant YAML (SECT 32)
    - ~ expansion works (uses $HOME, never shell)
    - allowed_binaries with nonexistent path => ConfigError
    - Workspace::open creates cap_std::fs::Dir
    - Workspace::resolve rejects absolute paths and ".."
    - BinaryRegistry::new resolves "cargo" -> /usr/bin/cargo
    - BinaryRegistry::resolve("nonexistent") => BinaryError::NotFound
    - run_captured executes echo hello, returns correct ToolResult
    - run_captured applies OutputLimits (truncates oversized output)

Phase P3: ShellSession + env
  Checkpoint: cargo test -p duga-sandbox -- shell_session --quiet
  GAPs: [G11]
  Depends on: [P2]
  Signal: .phase/P3
  What passes:
    - ShellSession::new(cwd) initializes with correct cwd and empty env
    - classify("cd /tmp") => Cd("/tmp")
    - classify("export FOO=bar") => Export("FOO", "bar")
    - classify("unset FOO") => Unset("FOO")
    - classify("pwd") => Pwd
    - classify("cargo test") => Spawn(["cargo", "test"])
    - apply(Cd) updates session.cwd
    - apply(Export) updates session.env
    - apply(Unset) removes from session.env
    - apply(Pwd) returns session.cwd as string
    - SanitizedEnv respects allowed list
    - SanitizedEnv warns on *_TOKEN patterns
    - SanitizedEnv sets PATH to /usr/bin:/bin
    - Secret-pattern vars allowed via explicit config produce startup warning

Phase P4: Tool trait + dispatcher
  Checkpoint: cargo test -p duga-tools -- --quiet
  GAPs: [G2, G7]
  Depends on: [P1, P2]
  Signal: .phase/P4
  What passes:
    - ToolDispatcher::new accepts Vec<Box<dyn Tool>>
    - ToolDispatcher::schemas returns correct ToolSchema for each tool
    - ToolDispatcher::get("name") returns Some or None
    - Schema validation pipeline (SECT 8):
      - Valid JSON + valid schema => Ok
      - Missing required field => ToolError::InvalidArgs
      - Type mismatch => ToolError::InvalidArgs
      - Unknown tool name => ToolError::InvalidArgs
    - ToolContext::new builds correctly with workspace, cancellation, events
    - dispatch returns ToolResult for successful tool execution

Phase P5: Built-in tools
  Checkpoint: cargo test -p duga-tools-builtin -- --quiet
    AND: cargo test --test e2e -- write_read_roundtrip
  GAPs: [G5, G8, G9]
  Depends on: [P3, P4]
  Signal: .phase/P5
  What passes:
    - ShellTool executes "echo hello" and returns output
    - ShellTool with session tracks cwd via "cd"
    - ShellTool with session tracks env via "export"
    - ShellTool with nonexistent binary => ToolError::Denied
    - ReadTool reads a text file
    - ReadTool with offset skips bytes correctly
    - ReadTool with limit stops at limit bytes
    - ReadTool on binary file (NUL within first 8KiB) returns hex summary
    - ReadTool on nonexistent path => ToolError::Io
    - WriteTool writes content atomically
    - WriteTool concurrent writes to same path are serialized
    - WriteTool on path outside workspace => ToolError::Denied
    - SearchTool finds regex matches in workspace
    - SearchTool with literal=true treats query as literal
    - SearchTool with max_results caps output
    - ThinkTool echoes thought back as output

Phase P6: Memory
  Checkpoint: cargo test -p duga-core -- memory --quiet
  GAPs: [G12, G13]
  Depends on: [P1, P7 (for tokenizer)]
  Signal: .phase/P6
  What passes:
    - Memory::new initializes with system messages, max_tokens, ratio
    - push_user appends user message
    - push_assistant appends assistant message
    - push_tool_result appends tool result message
    - messages() returns system + summary + recent in correct order
    - over_budget() returns false when under threshold
    - over_budget() returns true when over threshold
    - compress() replaces summary, retains newest half of recent
    - Summary overflow rules (SECT 22.1):
      - drops oldest non-pinned messages
      - returns ContextOverflow if even system+summary doesn't fit
    - Pinned task facts survive compression unchanged

Phase P7: LLM trait + providers
  Checkpoint: cargo test -p duga-llm -- --quiet
    AND: cargo test -p duga-llm-openai -- --quiet --test integration -- test_connect   (requires OPENAI_API_KEY)
  GAPs: [G6, G17]
  Depends on: [P1, P4]
  Signal: .phase/P7
  What passes:
    - LlmClient trait compiles with Send + Sync
    - ProviderRegistry::register and ProviderRegistry::build work
    - ProviderRegistry::build with "openai/gpt-4o" returns Ok
    - ProviderRegistry::build with "unknown/foo" returns Err
    - count_tokens returns non-zero for a non-empty message list
    - Integration test: real chat call returns LlmResponse (with API key)
    - Streaming: Event::LlmTokenDelta emitted during streaming calls

Phase P8: Stream + redaction
  Checkpoint: cargo test -p duga-events -- --quiet
  GAPs: [G2, G14]
  Depends on: [P1, P7]
  Signal: .phase/P8
  What passes:
    - JsonlSink writes valid JSONL to temp file
    - JsonlSink output can be read back with serde_json::from_str
    - Seq numbers are monotonic and attached to each event
    - Redactor replaces matching patterns with "<REDACTED>"
    - Redactor leaves non-matching fields unchanged
    - Redactor applies to ToolCallStarted.args and ToolCallFinished.output
    - Redactor applies to LlmRequest.messages string fields
    - Redactor does NOT match on field names, only string values
    - MultiSink fans out to all child sinks

Phase P9: Core loop
  Checkpoint: cargo test -p duga-core -- --quiet
    AND: ./tests/e2e/loop_tests.sh  (or cargo test --test e2e -- loop)
  GAPs: [G1, G3]
  Depends on: [P4, P5, P6, P7, P8]
  Signal: .phase/P9
  What passes:
    - AgentLoop::run with MockLlm that returns tool calls then answer
    - AgentLoop terminates with MaxStepsReached when steps exceed limit
    - AgentLoop terminates with MaxToolCallsReached when tool_calls exceed limit
    - AgentLoop terminates with Timeout when wall clock exceeds max_runtime
    - AgentLoop terminates with Cancelled when token is cancelled
    - Retry: transient error is retried retry_on_error times then fails
    - Retry: non-transient error is NOT retried
    - LLM call cancellation works via select! (biased)
    - Memory compression triggers when over_budget
    - Events emitted in correct order (LoopIteration, LlmRequest, etc.)
    - Streaming: LlmTokenDelta emitted before LlmResponse (GAP G3)
    - FinalResponse emitted on successful termination
    - ToolContext.cancellation propagates to running tools when cancelled

Phase P10: WASM plugin ABI
  Checkpoint: cd plugins-examples/format-code && cargo build --target wasm32-wasip2 --release
    AND: cargo test -p duga-plugin-host -- --quiet
  GAPs: [G10]
  Depends on: [P4, P5]
  Signal: .phase/P10
  What passes:
    - WIT file compiles with wit-bindgen (host and guest features)
    - Guest plugin builds for wasm32-wasip2
    - Host rejects core (non-component) WASM modules => PluginError::NotAComponent
    - Host loads valid .wasm component, calls info(), registers tool
    - Host rejects plugin whose name collides with built-in (warning + skip)
    - Plugin execution returns ToolResult matching its outcome
    - Plugin gets workspace-root string correctly
    - WASI capabilities are enforced:
      - Workspace filesystem: allowed
      - Network: denied (PluginError if attempted)
      - Process spawn: denied (PluginError if attempted)
    - Cancellation cancels running plugin (fuel or epoch)

Phase P11: Harness + CLI
  Checkpoint: cargo build --release && ./harness --help
    AND: ./harness --config test.yaml "echo hello"  (dry run without real LLM)
  GAPs: [G1-G17 resolution required for production]
  Depends on: [P2, P7, P9, P10]
  Signal: .phase/P11
  What passes:
    - ./harness --help prints usage
    - ./harness --config config.yaml "task" parses config, builds loop, runs
    - ./harness with missing --config => ConfigError with clear message
    - ./harness with nonexistent config path => ConfigError
    - ./harness with invalid model string => ConfigError
    - ./harness writes replay.jsonl to configured path
    - Signal handling (Ctrl+C) cancels running agent

Phase P12: Replay
  Checkpoint: cargo test -p duga-replay -- --quiet
    AND: cargo run --bin duga-replay -- --input test.jsonl --substitute-llm
  GAPs: [G4]
  Depends on: [P8, P9]
  Signal: .phase/P12
  What passes:
    - read_jsonl parses a valid JSONL file into StoredEvent sequence
    - MockLlm::new(events) replays stored LlmResponse responses in order
    - MockToolDispatcher returns stored ToolResults by tool_call_id
    - Replaying a recorded session without live LLM produces same FinalResponse
    - Replay with substitute_tools=true works (no real tool execution)
    - ReplayRoundtrip: run -> emit -> write JSONL -> read -> replay -> same output
    - Replay detects corrupted JSONL and returns ReplayError

Phase P13: TUI (optional)
  Checkpoint: cargo build --features tui
    AND: ./harness --config config.yaml --tui "task"  (visual check)
  Depends on: [P8, P11]
  Signal: .phase/P13

Phase P14: Static builds + CI
  Checkpoint: cargo xtask dist --target x86_64-unknown-linux-musl
    AND: cargo xtask dist --target aarch64-apple-darwin
    AND: cargo xtask dist --target x86_64-pc-windows-msvc
    AND: CI pipeline green for all 5 targets
  GAPs: [G16]
  Depends on: [P11]
  Signal: .phase/P14
  What passes:
    - Linux musl binary: file target/x86_64-unknown-linux-musl/release/harness => "ELF... statically linked"
    - macOS binary: lipo -info => "x86_64" or "arm64"
    - Windows binary: not a console app that needs MSVC redist
    - CI pipeline: all 5 targets build in < 30 minutes
    - cargo deny passes (no unlicensed deps, no advisories)

Phase P15: Hot reload (post-MVP, optional)
  Checkpoint: cargo test -p duga-plugin-host -- hot_reload --quiet
  GAPs: [G15]
  Depends on: [P10]
  Signal: .phase/P15
  What passes:
    - PluginRegistry::watch(dir) detects new .wasm file
    - PluginRegistry::watch detects modified .wasm file
    - New plugin becomes available for next tool dispatch
    - Active execution using old component continues uninterrupted
    - Invalid .wasm files produce warning, not crash
    - Hot reload can be disabled via config

### Completion progress file

The developer maintains a `.phase/completed` file with the phases done and date:

```
# Phase completion log
P1 2026-05-11 # Primitives: ToolError, ToolResult, Event compile
P2 2026-05-13 # Config + sandbox: Workspace, BinaryRegistry, run_captured
...
```

### GAP resolution tracking

Each GAP is tracked in a `.phase/gaps.md` file with status:

```
# GAP resolution log
G1: OPEN - Tool-call concurrency. Awaiting decision on sequential vs parallel.
G2: RESOLVED 2026-05-11 - EventSink::emit uses unbounded mpsc, never blocks.
G3: OPEN - LlmTokenDelta ordering. Awaiting spec clarification.
...
```

When a GAP affects a phase, the phase cannot be marked done until the GAP is resolved.
