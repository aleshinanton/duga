# EPIC-3: Tool Trait System

**§SPEC:** §7–8, §10  
**Labels:** `epic/tools`  
**Crate:** `duga-tools`

## Goal
Define the `Tool` trait, `ToolContext`, `ToolDispatcher`, and the schema registry. This is the abstraction layer between the agent loop and all tool implementations (built-in and WASM plugins).

---

### TASK-3.1: Tool trait definition

- **§SPEC:** §7 (Typed Tool Arguments)
- **Labels:** `layer/tools`, `priority/critical`
- **Description:** Define the `Tool` trait in `duga-tools`. The trait has: `type Args: DeserializeOwned + JsonSchema`, `fn name(&self) -> &str`, `fn description(&self) -> &str`, `fn retryable(&self) -> bool` (default false), `async fn execute(&self, ctx: ToolContext<'_>, args: Self::Args) -> Result<ToolResult, ToolError>`, and `fn json_schema(&self) -> Value` (auto-generated from `Self::Args` via schemars). Also implement a blanket `fn tool_schema(&self) -> ToolSchema` that calls `name()`, `description()`, `json_schema()`.
- **Files affected:**
  - `crates/duga-tools/Cargo.toml` (new)
  - `crates/duga-tools/src/lib.rs` (new)
  - `crates/duga-tools/src/tool.rs` (new)
- **Types involved:** `Tool` (trait), `ToolContext` (from TASK-3.2)
- **Functions to implement:**
  - `trait Tool: Send + Sync { ... }`
  - `fn json_schema(&self) -> Value` — provided method using `schemars::schema_for!`
  - `fn tool_schema(&self) -> ToolSchema` — provided method
- **Dependencies:** TASK-1.3 (ToolSchema), TASK-1.4 (ToolResult), TASK-1.5 (ToolError)
- **Implementation steps:**
  1. Create `crates/duga-tools/Cargo.toml` with deps: `duga-types`, `duga-events`, `duga-sandbox`, `serde_json`, `jsonschema`, `schemars`
  2. Create `tool.rs`: define `Tool` trait with associated type + methods
  3. Implement `json_schema()` default: `schemars::schema_for!(Self::Args)` → serialize to `Value`
  4. Implement `tool_schema()` default: build `ToolSchema` from name/desc/schema
  5. Forward-declare `ToolContext` (from TASK-3.2) — use `use crate::context::ToolContext;`
  6. Ensure trait is object-safe enough for `Box<dyn Tool>` — but note that `type Args` makes it NOT object-safe for dispatch. The dispatcher handles this (TASK-3.4).
- **Edge cases:**
  - `type Args` is an associated type — prevents `Box<dyn Tool>` from calling `execute` directly. The dispatcher must use enum-based dispatch or type-erased wrapper. Document this constraint.
  - `schemars::schema_for!` is generic over `T: JsonSchema` — works at trait impl level, not trait object level. Each concrete tool's `json_schema()` uses its own `Self::Args`.
- **Definition of Done:**
  - Trait compiles
  - `cargo clippy` clean
- **Acceptance criteria:**
  - Trait has all methods from SECT 7
  - `retryable()` default returns `false`
  - `tool_schema()` returns `ToolSchema` with name, description, args_schema
- **Test plan:**
  - unit: Test with a minimal Tool impl (no-op) — verify tool_schema() returns correct data
- **Estimated effort:** 4 hours

---

### TASK-3.2: ToolContext struct

- **§SPEC:** §10 (ToolContext), §26 (Cancellation)
- **Labels:** `layer/tools`, `priority/critical`
- **Description:** Define `ToolContext<'a>` struct with fields: `workspace: &'a Workspace`, `cancellation: CancellationToken`, `event_sink: &'a dyn EventSink`. This is the only context tools receive — no mutable memory, no orchestration control, no prompt injection access. The struct must be `Send` (used inside `.await` in `handle_tool_call`).
- **Files affected:**
  - `crates/duga-tools/src/context.rs` (new)
  - `crates/duga-tools/src/lib.rs` (add module)
- **Types involved:** `ToolContext<'a>`
- **Functions to implement:**
  - `ToolContext::new(workspace: &'a Workspace, cancellation: CancellationToken, event_sink: &'a dyn EventSink) -> Self`
  - `ToolContext::is_cancelled(&self) -> bool`
- **Dependencies:** TASK-2.1 (Workspace), TASK-1.1 (duga-types), EPIC-7 (EventSink trait — forward-declare in duga-events)
- **Implementation steps:**
  1. Create `context.rs`: define `ToolContext<'a>` with three public fields
  2. Implement `new()` constructor
  3. Implement `is_cancelled()` → `self.cancellation.is_cancelled()`
  4. Add `unsafe impl Send for ToolContext<'_> {}` if needed — `&dyn EventSink: Send + Sync` ensures this
  5. Ensure `CancellationToken` from `tokio_util::sync::CancellationToken` is used
- **Edge cases:**
  - `ToolContext` carries references — lifetime is tied to the loop's scope
  - Cancel token is clonable; child tokens can be created for per-tool cancellation
  - No `Clone` for ToolContext — it's passed by ownership, not shared
- **Definition of Done:**
  - Struct compiles
  - `cargo clippy` clean
- **Acceptance criteria:**
  - `ToolContext::is_cancelled()` returns false when token not cancelled
  - `ToolContext::is_cancelled()` returns true after token is cancelled
  - Struct is `Send`
- **Test plan:**
  - unit: Test is_cancelled with CancellationToken, test Send trait is satisfied
- **Estimated effort:** 3 hours

---

### TASK-3.3: ToolDispatcher::new + schemas + get

- **§SPEC:** §8 (Schema Validation Pipeline — registration side)
- **Labels:** `layer/tools`, `priority/critical`
- **Description:** Implement `ToolDispatcher` that owns a `Vec<Box<dyn Tool>>` (but dispatch is done via a type-erased wrapper or enum — see TASK-3.4). Implement `ToolDispatcher::new(tools: Vec<Box<dyn Tool>>) -> Self`, `fn schemas(&self) -> Vec<ToolSchema>`, and `fn get(&self, name: &str) -> Option<&dyn Tool>`. The dispatcher is the single registry for all tools.
- **Files affected:**
  - `crates/duga-tools/src/dispatcher.rs` (new)
  - `crates/duga-tools/src/lib.rs` (add module)
- **Types involved:** `ToolDispatcher`, `Tool` trait
- **Functions to implement:**
  - `ToolDispatcher::new() -> Self` (starts empty)
  - `ToolDispatcher::register(&mut self, tool: Box<dyn Tool>)` — validate name uniqueness
  - `ToolDispatcher::schemas(&self) -> Vec<ToolSchema>`
  - `ToolDispatcher::get(&self, name: &str) -> Option<&dyn Tool>`
  - `ToolDispatcher::names(&self) -> Vec<&str>`
- **Dependencies:** TASK-3.1 (Tool trait)
- **Implementation steps:**
  1. Define `ToolDispatcher` with `tools: Vec<Box<dyn Tool>>` and `schema_cache: Vec<ToolSchema>`
  2. Implement `register()` — push tool, update cache, check for duplicate names
  3. Implement `schemas()` — return clone of cache
  4. Implement `get()` — linear search by name
  5. Implement `names()` — iterate tools and collect names
  6. Add unit tests with a mock tool
- **Edge cases:**
  - Duplicate tool name → `register()` returns error or panics? → Return `Result<(), ToolRegistryError>` with `DuplicateName` variant
  - Empty dispatcher → `schemas()` returns empty vec, `get("anything")` returns None
  - Tool names must be alphanumeric + underscore? → Document but don't enforce strictly
- **Definition of Done:**
  - `cargo build` succeeds
  - `cargo test` passes
  - `cargo clippy` clean
- **Acceptance criteria:**
  - Register 2 tools, `schemas()` returns vec of 2 ToolSchemas
  - `get("write")` returns Some when registered, None otherwise
  - `names()` returns correct names
  - Duplicate name → `Err(ToolRegistryError::DuplicateName)`
- **Test plan:**
  - unit: Implement a minimal `MockTool` for testing; test registration, lookup, duplicate detection
- **Estimated effort:** 4 hours

---

### TASK-3.4: ToolDispatcher::dispatch - tool resolution + error mapping

- **§SPEC:** §8 (dispatch flow — resolve tool), §27 (error propagation)
- **Labels:** `layer/tools`, `priority/critical`
- **Description:** Implement the first stage of `dispatch()`: given a `ToolCall`, resolve the tool by name from the registry, map unknown names to `ToolError::InvalidArgs` with a helpful message listing available tools. This is part 1 of the schema pipeline — tool resolution, before JSON parsing. The full pipeline (parse → validate → deserialize → execute) is completed in EPIC-9.
- **Files affected:**
  - `crates/duga-tools/src/dispatcher.rs` (append method body)
- **Types involved:** `ToolDispatcher`, `ToolCall`, `ToolContext`, `ToolResult`, `ToolError`
- **Functions to implement:**
  - `ToolDispatcher::dispatch(&self, call: &ToolCall, ctx: ToolContext<'_>) -> Result<ToolResult, ToolError>` — stub that resolves tool and delegates to per-tool dispatch (full impl in TASK-9.5)
- **Dependencies:** TASK-3.3 (ToolDispatcher::get), TASK-3.2 (ToolContext)
- **Implementation steps:**
  1. Implement `dispatch()` as `async fn`
  2. Call `self.get(&call.tool)` → if None, return `Err(ToolError::InvalidArgs(format!("Unknown tool '{}'. Available: {}", call.tool, self.names().join(", "))))`
  3. For now (until TASK-9.5), panic with "not implemented" for the found case — this task is about error handling for unknown tools
  4. Add unit test
- **Edge cases:**
  - Case sensitivity: tool names should be case-sensitive (spec doesn't specify, match exactly)
  - Tool name with leading/trailing spaces → handled at ToolCall construction (or reject in validation)
  - Error message must be LLM-friendly — clear, actionable, no internal paths
- **Definition of Done:**
  - `dispatch()` returns `InvalidArgs` for unknown tools
  - `cargo test` passes
  - `cargo clippy` clean
- **Acceptance criteria:**
  - `dispatch(ToolCall { tool: "nonexistent", .. }, ctx)` → `Err(ToolError::InvalidArgs("Unknown tool 'nonexistent'. Available: read, write, shell"))`
  - Error message includes list of available tool names
- **Test plan:**
  - unit: Test with empty dispatcher → error includes "Available: ", test with populated dispatcher → error lists names
- **Estimated effort:** 3 hours

---

### TASK-3.5: Schema validation pipeline — parse JSON + validate schema + deserialize

- **§SPEC:** §8 (full validation pipeline)
- **Labels:** `layer/tools`, `priority/critical`
- **Description:** Implement the complete schema validation pipeline inside `ToolDispatcher::dispatch()`: (1) validate `call.raw_args` is structurally valid JSON (it already is — it's a `Value`), (2) validate against the tool's JSON Schema using the `jsonschema` crate, (3) deserialize into the tool's `Args` type. Each step maps errors to `ToolError::InvalidArgs` with human-readable messages. This is the "type-safe firewall" between untrusted LLM output and tool execution.
- **Files affected:**
  - `crates/duga-tools/src/dispatcher.rs` (append pipeline)
  - `crates/duga-tools/src/schema.rs` (new — schema validation helpers)
- **Types involved:** `ToolDispatcher`, `Tool`, `ToolError::InvalidArgs`
- **Functions to implement:**
  - `fn validate_schema(schema: &Value, args: &Value) -> Result<(), Vec<String>>` — runs jsonschema, collects errors
  - `fn deserialize_args<T: DeserializeOwned>(value: Value) -> Result<T, String>`
- **Dependencies:** TASK-3.4, TASK-3.1 (json_schema from tool)
- **Implementation steps:**
  1. Create `schema.rs`: implement `validate_schema()` using `jsonschema::options().build(schema)?.validate(args)`
  2. Map jsonschema errors to human-readable strings: `format!("{}: {}", instance_path, kind)`
  3. Implement `deserialize_args()` using `serde_json::from_value`
  4. In `dispatch()`, after resolving tool: `validate_schema(tool.json_schema(), call.raw_args)` → on error, `Err(ToolError::InvalidArgs("Invalid args: " + joined_errors))`
  5. Then `deserialize_args::<T::Args>()` → on error, `Err(ToolError::InvalidArgs("Deserialization failed: " + e))`
  6. Note: cannot call `deserialize_args` generically because `T::Args` is not known at the trait-object level. This is the known limitation of the `Tool` trait design. *Wait — this is the core dispatch problem.* See implementation decision below.
- **Implementation decision — dispatch strategy:**
  Because `Tool` has `type Args`, `Box<dyn Tool>` cannot dispatch to `execute`. Two approaches exist:
  **(A) Macro-based enum dispatch** — define an internal enum of all tool variants, match on tool name, call typed tool directly.
  **(B) Type-erased wrapper** — wrap each tool in an `ErasedTool` that stores the schema validation + execute as boxed closures.
  For MVP, use approach **(B)** — it's simpler and doesn't require a centralized enum. The `ErasedTool` stores: `name`, `description`, `schema: Value`, `retryable: bool`, and `execute: Box<dyn Fn(Value, ToolContext) -> Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send>> + Send + Sync>`.
  This task implements the `ErasedTool` wrapper and modifies `ToolDispatcher` to store `Vec<ErasedTool>` instead of `Vec<Box<dyn Tool>>`.
- **Files affected (revised):**
  - `crates/duga-tools/src/erased.rs` (new — `ErasedTool` wrapper)
  - `crates/duga-tools/src/dispatcher.rs` (revised to use `ErasedTool`)
- **Functions to implement:**
  - `ErasedTool::erase<T: Tool + 'static>(tool: T) -> Self`
  - Inside `erase()`: capture `schema`, `retryable`, build closure that deserializes + calls `tool.execute`
  - `ToolDispatcher::register_erased(&mut self, tool: ErasedTool)`
  - Updated `dispatch()`: get `ErasedTool` by name, validate schema, call erased execute closure
- **Dependencies:** TASK-3.4 (dispatch stub)
- **Edge cases:**
  - JSON parse can't fail on `Value` (it's already parsed) — skip parse step
  - Schema validation with jsonschema may have its own errors → wrap in `InvalidArgs`
  - Deserialization error messages from serde can be cryptic → improve with `.to_string()`
  - The erased execute closure must be `Send` because dispatch is async
- **Definition of Done:**
  - Full pipeline works with a mock tool
  - Invalid schema → `ToolError::InvalidArgs` with clear message
  - Invalid types → `ToolError::InvalidArgs`
  - Valid args → tool.execute is called
  - `cargo clippy` clean
- **Acceptance criteria:**
  - `dispatch(ToolCall { tool: "mock", raw_args: json!({"required_field": missing}), ... })` → `Err(ToolError::InvalidArgs("Invalid args: /: missing required properties: required_field"))`
  - `dispatch(ToolCall { tool: "mock", raw_args: json!({"required_field": "ok"}), ... })` → calls tool.execute
- **Test plan:**
  - unit: Test with a mock tool that has a known schema; test missing field, type mismatch, valid args; test serde deserialization failure
- **Estimated effort:** 6 hours

---

### TASK-3.6: ToolResult::from_outcome builder + duration measurement

- **§SPEC:** §9 (ToolResult duration_ms), §5.1 (handle_tool_call pattern)
- **Labels:** `layer/tools`, `priority/critical`
- **Description:** Implement a helper in `duga-tools` that wraps `ToolResult::from_outcome()` (defined in TASK-1.4) with convenience for the dispatch context. Specifically, a function that takes an `Instant` started before tool execution, the `ToolCall`, and the `Result<ToolResult, ToolError>`, and produces a `ToolResult` with correct `tool_call_id` and `duration_ms`. This is used in `handle_tool_call` (EPIC-10).
- **Files affected:**
  - `crates/duga-tools/src/result.rs` (new)
- **Types involved:** `ToolResult`, `ToolError`, `ToolCall`
- **Functions to implement:**
  - `fn make_tool_result(call: &ToolCall, outcome: Result<ToolResult, ToolError>, started: Instant) -> ToolResult`
- **Dependencies:** TASK-1.4 (ToolResult::from_outcome)
- **Implementation steps:**
  1. Create `result.rs`
  2. Implement `make_tool_result()`: calls `ToolResult::from_outcome(call.id, &call.tool, outcome, started)`
  3. This is a thin wrapper; primary logic is in `ToolResult::from_outcome` (TASK-1.4)
- **Edge cases:**
  - None — pure delegation
- **Definition of Done:**
  - Function compiles and works
- **Acceptance criteria:**
  - `make_tool_result(&call, Ok(success_result), start)` → `ToolResult { success: true, ... }`
  - `make_tool_result(&call, Err(ToolError::Timeout), start)` → `ToolResult { success: false, ... }`
  - `duration_ms` > 0
- **Test plan:**
  - unit: Test both success and error paths, verify duration is measured
- **Estimated effort:** 2 hours
