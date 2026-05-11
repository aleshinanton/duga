# EPIC-1: Foundation

**§SPEC:** §4, §6–10, §27  
**Label:** `epic/foundation`  
**Crate:** `duga-types` (all tasks)

## Goal
Establish the `duga-types` crate with every wire and on-the-loop datatype. Zero logic, zero I/O, zero tokio. Every other crate depends on this.

---

### TASK-1.1: Workspace crate scaffold + Cargo.toml + lib.rs

- **§SPEC:** §4, §6, §9
- **Labels:** `layer/foundation`, `priority/critical`
- **Description:** Create the workspace Cargo.toml (virtual manifest), `rust-toolchain.toml` pinning the Rust channel, `.cargo/config.toml` with per-target rustflags, and the `crates/duga-types/` directory with `Cargo.toml` and `src/lib.rs`. Declare all dependencies: `serde` (derive), `serde_json`, `schemars` v0.8+ (uuid feature), `uuid` v1 (v4 + serde), `time` 0.3 (serde), `thiserror` 1.
- **Files affected:**
  - `Cargo.toml` (workspace root)
  - `rust-toolchain.toml`
  - `.cargo/config.toml`
  - `crates/duga-types/Cargo.toml`
  - `crates/duga-types/src/lib.rs`
- **Types involved:** None yet — infrastructure only.
- **Functions to implement:** None — infrastructure only.
- **Dependencies:** (root task — no dependencies)
- **Implementation steps:**
  1. Write workspace-level `Cargo.toml` with `[workspace]`, `resolver = "2"`, `members = ["crates/*", "crates/duga-llm/*", "crates/duga-tools-builtin", "plugins-examples/*", "xtask"]`
  2. Write `rust-toolchain.toml` pinning stable channel
  3. Write `.cargo/config.toml` with `[target.x86_64-unknown-linux-musl]` linker and rustflags
  4. Write `deny.toml` (empty skeleton, populated in EPIC-15)
  5. Write `crates/duga-types/Cargo.toml` with all dependencies as specified in PLAN.md §5.1
  6. Write `crates/duga-types/src/lib.rs` with `//! duga-types: ...` module doc
  7. Run `cargo check -p duga-types` — must succeed
- **Edge cases:**
  - `crates/duga-llm/*` glob must match subdirectories but not files; test with `cargo metadata`
  - Pinned toolchain must be available in CI — use `stable` initially, pin to specific version later
- **Definition of Done:**
  - `cargo check -p duga-types` succeeds
  - `cargo metadata` shows correct workspace members
  - `cargo clippy -p duga-types -- -D warnings` clean
- **Acceptance criteria:**
  - Workspace compiles the `duga-types` crate with no warnings
- **Test plan:**
  - unit: `cargo check` is the test
  - integration: none
- **Estimated effort:** 2 hours

---

### TASK-1.2: Message, Role, ContentBlock types

- **§SPEC:** §6a (AssistantMessage fields), §21 (Memory message format)
- **Labels:** `layer/foundation`, `priority/critical`
- **Description:** Define the `Message` enum, `Role` enum (`System`, `User`, `Assistant`, `Tool`), and `ContentBlock` enum (text and tool-call variants). Messages are the universal communication format between the LLM, memory, and event system. All types must derive `Serialize`, `Deserialize`, `Clone`, `Debug`, `PartialEq`. Add `JsonSchema` derive from schemars on `ContentBlock` for schema generation.
- **Files affected:**
  - `crates/duga-types/src/message.rs` (new)
  - `crates/duga-types/src/lib.rs` (add `pub mod message;`)
- **Types involved:** `Message`, `Role`, `ContentBlock`
- **Functions to implement:**
  - `Message::new(role: Role, content: Vec<ContentBlock>) -> Self`
  - `Message::user(text: impl Into<String>) -> Self` (convenience)
  - `Message::assistant(text: Option<String>, tool_calls: Vec<ToolCall>) -> Self` (convenience)
  - `Message::system(text: impl Into<String>) -> Self` (convenience)
  - `Message::tool(tool_call_id: Uuid, output: String) -> Self` (convenience)
  - `impl Display for Role`
- **Dependencies:** TASK-1.1
- **Implementation steps:**
  1. Define `Role` enum with `System`, `User`, `Assistant`, `Tool` variants + serde rename_all = "lowercase"
  2. Define `ContentBlock` enum: `Text { text: String }`, `ToolCall(ToolCall)` (forward-declare ToolCall from TASK-1.3 as opaque if needed)
  3. Define `Message` struct with `role: Role`, `content: Vec<ContentBlock>`, optional `name: Option<String>`, optional `pinned: bool`
  4. Implement convenience constructors
  5. Add `impl Display for Role` with lowercase output
  6. Add `#[derive(JsonSchema)]` on `ContentBlock` (skip `ToolCall` variant initially, add in TASK-1.3)
- **Edge cases:**
  - Assistant message with both text AND tool calls (text is optional)
  - Tool message must reference a valid `tool_call_id` (Uuid)
  - System message before any user message — allowed per OpenAI/Anthropic convention
- **Definition of Done:**
  - `cargo build -p duga-types` succeeds
  - `cargo test -p duga-types` passes
  - `cargo clippy -p duga-types -- -D warnings` clean
  - No `unwrap()` in non-test code
- **Acceptance criteria:**
  - `Message::user("hello")` creates valid user message
  - `Message::assistant(Some("text".into()), vec![])` creates assistant message
  - Serialize/deserialize roundtrip: `serde_json::to_string(msg)` → `serde_json::from_str` → same message
- **Test plan:**
  - unit: Test each convenience constructor, test Display for Role, test serialize/deserialize roundtrip for all message types
  - integration: none
- **Estimated effort:** 4 hours

---

### TASK-1.3: ToolCall, ToolSchema, AssistantMessage types

- **§SPEC:** §6 (ToolCall), §7 (ToolSchema from trait), §6a (AssistantMessage)
- **Labels:** `layer/foundation`, `priority/critical`
- **Description:** Define `ToolCall { id: Uuid, tool: String, raw_args: Value }` per SECT 6. Define `ToolSchema { name: String, description: String, args_schema: Value }` — the serialized JSON Schema for one tool. Define `AssistantMessage { text: Option<String>, tool_calls: Vec<ToolCall> }` per SECT 6a. All types derive `Serialize`, `Deserialize`, `Clone`, `Debug`, `PartialEq`, `JsonSchema`.
- **Files affected:**
  - `crates/duga-types/src/tool_call.rs` (new)
  - `crates/duga-types/src/tool_schema.rs` (new)
  - `crates/duga-types/src/lib.rs` (add modules)
  - `crates/duga-types/src/message.rs` (update `ContentBlock::ToolCall(ToolCall)`)
- **Types involved:** `ToolCall`, `ToolSchema`, `AssistantMessage`
- **Functions to implement:**
  - `ToolCall::new(tool: impl Into<String>, raw_args: Value) -> Self` (generates Uuid)
  - `ToolSchema::new(name: impl Into<String>, description: impl Into<String>, args_schema: Value) -> Self`
  - `AssistantMessage::is_termination(&self) -> bool` (true when `tool_calls.is_empty()`)
- **Dependencies:** TASK-1.1, TASK-1.2
- **Implementation steps:**
  1. Create `tool_call.rs`: define `ToolCall` with `id: Uuid`, `tool: String`, `raw_args: Value`
  2. Implement `ToolCall::new()` that auto-generates `Uuid::new_v4()`
  3. Create `tool_schema.rs`: define `ToolSchema`
  4. Update `message.rs`: add `ContentBlock::ToolCall(ToolCall)` variant, backfill `JsonSchema` derive
  5. Create `AssistantMessage` struct in `message.rs` or separate file
  6. Implement `is_termination()`
  7. Add serde tests
- **Edge cases:**
  - `ToolCall::raw_args` is opaque `Value` — no schema enforcement here (that's TASK-3.5)
  - `AssistantMessage.text` may be `None` when only tool calls exist
  - `AssistantMessage.text` may be `Some("")` — treat as no text
- **Definition of Done:**
  - All types compile with required derives
  - Serialize/deserialize roundtrip passes for all types
  - `cargo clippy` clean
- **Acceptance criteria:**
  - `AssistantMessage { text: None, tool_calls: vec![] }.is_termination()` returns `true`
  - `AssistantMessage { text: None, tool_calls: vec![call] }.is_termination()` returns `false`
- **Test plan:**
  - unit: Test `ToolCall::new` generates UUID, test `is_termination` for both cases, test JSON roundtrip
- **Estimated effort:** 3 hours

---

### TASK-1.4: ToolResult struct + builder

- **§SPEC:** §9 (ToolResult fields)
- **Labels:** `layer/foundation`, `priority/critical`
- **Description:** Define `ToolResult` struct with all fields from SECT 9: `tool_call_id`, `success`, `output`, `metadata`, `duration_ms`, `stdout_bytes`, `stderr_bytes`, `truncated`. Implement `ToolResult::from_outcome()` as a builder that constructs a `ToolResult` from a `Result<ToolResult, ToolError>` outcome plus timing. Implement `Display` for human-readable feedback.
- **Files affected:**
  - `crates/duga-types/src/tool_result.rs` (new)
  - `crates/duga-types/src/lib.rs` (add module)
- **Types involved:** `ToolResult`, `ToolError` (from TASK-1.5 — forward-declare)
- **Functions to implement:**
  - `ToolResult::builder() -> ToolResultBuilder`
  - `ToolResultBuilder::tool_call_id(...) -> &mut Self`
  - `ToolResultBuilder::success(...) -> &mut Self`
  - `ToolResultBuilder::output(...) -> &mut Self`
  - `ToolResultBuilder::metadata(...) -> &mut Self`
  - `ToolResultBuilder::duration_ms(...) -> &mut Self`
  - `ToolResultBuilder::stdout_bytes(...) -> &mut Self`
  - `ToolResultBuilder::stderr_bytes(...) -> &mut Self`
  - `ToolResultBuilder::truncated(...) -> &mut Self`
  - `ToolResultBuilder::build() -> ToolResult`
  - `ToolResult::from_outcome(tool_call_id: Uuid, tool_name: &str, outcome: Result<ToolResult, ToolError>, started: Instant) -> ToolResult`
  - `impl Display for ToolResult` — produces human-readable output
- **Dependencies:** TASK-1.1, TASK-1.3 (needs Uuid), TASK-1.5 (needs ToolError)
- **Implementation steps:**
  1. Define `ToolResult` struct with all 9 fields from SECT 9
  2. Implement `ToolResultBuilder` with fluent API
  3. Implement `from_outcome()` — on Ok: return inner; on Err: build ToolResult with `success: false`, `output = e.to_string()`
  4. Implement `Display` — formats as "tool_call_id: ...\noutput: ...\nduration: ...ms"
  5. Add serde roundtrip test
- **Edge cases:**
  - `metadata` is `Value` — may be `Null` for tools without metadata
  - `stdout_bytes` / `stderr_bytes` are zero for non-process tools (think, read, write)
  - `from_outcome` must measure duration as `started.elapsed().as_millis()` — not Instant::now() inside the function
- **Definition of Done:**
  - `cargo build -p duga-types` succeeds
  - `cargo test -p duga-types` passes
  - Builder all-fields test passes
  - `from_outcome` success and error paths tested
- **Acceptance criteria:**
  - `ToolResult::from_outcome(id, "bash", Ok(result), start)` → `ToolResult { success: true, ... }`
  - `ToolResult::from_outcome(id, "bash", Err(ToolError::Timeout), start)` → `ToolResult { success: false, output: "process timed out", ... }`
  - Builder produces all default field values when not set (defaults: empty strings, 0 duration/bytes, false truncated)
- **Test plan:**
  - unit: Test builder all-fields, builder defaults, from_outcome Ok, from_outcome Err, Display formatting
- **Estimated effort:** 4 hours

---

### TASK-1.5: ToolError enum + is_transient() + Display

- **§SPEC:** §27 (Error model)
- **Labels:** `layer/foundation`, `priority/critical`
- **Description:** Define `ToolError` enum with all 7 variants from SECT 27: `Timeout`, `Cancelled`, `Denied(String)`, `InvalidArgs(String)`, `Io(std::io::Error)`, `Plugin(String)`, `OutputLimitExceeded`. Implement `is_transient(&self) -> bool` (true for `Io` and `Plugin` only). Implement `Display` producing human-readable tool feedback. Derive `thiserror::Error` for `Display` + `std::error::Error`. Derive `Serialize`/`Deserialize` (note: `std::io::Error` is NOT serializable — serialize it as its Display string).
- **Files affected:**
  - `crates/duga-types/src/error.rs` (new)
  - `crates/duga-types/src/lib.rs` (add module)
- **Types involved:** `ToolError`, `AgentError` (from TASK-1.6)
- **Functions to implement:**
  - `ToolError::is_transient(&self) -> bool`
  - `impl Display for ToolError` (via thiserror)
  - Custom `Serialize`/`Deserialize` for `ToolError::Io` — serialize as `{"Io": "<display string>"}`, deserialize back as opaque Io error
- **Dependencies:** TASK-1.1
- **Implementation steps:**
  1. Derive `thiserror::Error` on `ToolError` with `#[error("...")]` messages for each variant
  2. Implement `is_transient()` — match `Io(_) | Plugin(_) => true`, `_ => false`
  3. Implement custom `Serialize` for ToolError using `serde::Serialize` — tag each variant
  4. Implement custom `Deserialize` for ToolError — for `Io`, reconstruct as `std::io::Error::new(std::io::ErrorKind::Other, string)`
  5. Add tests for serialize/deserialize roundtrip of each variant (except Io — test approximate reconstruction)
- **Edge cases:**
  - `ToolError::Io` roundtrip: deserialized Io kind is always `Other` — this is intentional, only the message matters for replay
  - `ToolError::Plugin` is retryable — same semantics as Io
  - `Display` messages must be suitable as LLM tool feedback: clear, actionable, no stack traces
- **Definition of Done:**
  - `cargo build` succeeds
  - All 7 variants compile and serialize/deserialize
  - `is_transient` returns correct values for each
  - `cargo clippy` clean
- **Acceptance criteria:**
  - `ToolError::Timeout.is_transient()` → `false`
  - `ToolError::Io(io_err).is_transient()` → `true`
  - `ToolError::Plugin("boom".into()).is_transient()` → `true`
  - `ToolError::Denied("not allowed".into()).to_string()` → `"denied: not allowed"` (or similar)
  - Serialize `ToolError::Timeout` → `"{\"Timeout\":null}"`, deserialize back → `ToolError::Timeout`
- **Test plan:**
  - unit: Test `is_transient` for all 7 variants, test Display output, test serialize/deserialize for all variants, verify Io reconstruction preserves the message
- **Estimated effort:** 4 hours

---

### TASK-1.6: AgentError enum + Display

- **§SPEC:** §27 (AgentError terminates loop)
- **Labels:** `layer/foundation`, `priority/critical`
- **Description:** Define `AgentError` enum with variants: `MaxStepsReached`, `MaxToolCallsReached`, `Timeout`, `Cancelled`, `ContextOverflow`, `SummarizerFailed(String)`. These errors terminate the agent loop (unlike `ToolError` which becomes feedback). Derive `thiserror::Error`, `Serialize`, `Deserialize`, `Clone`, `Debug`, `PartialEq`.
- **Files affected:**
  - `crates/duga-types/src/error.rs` (append to existing)
- **Types involved:** `AgentError`
- **Functions to implement:** None beyond derives.
- **Dependencies:** TASK-1.1
- **Implementation steps:**
  1. Add `AgentError` enum after `ToolError` in `error.rs`
  2. Derive `thiserror::Error` with meaningful messages (e.g., `#[error("max steps ({0}) reached")]`)
  3. Add `Serialize`/`Deserialize` derives
  4. Add unit tests
- **Edge cases:**
  - `AgentError` is distinct from `ToolError` — no shared variants, different purpose
  - `SummarizerFailed` carries a String, not the original error — the summarizer's Display is captured
- **Definition of Done:**
  - All derives compile
  - Serialize/deserialize roundtrip
  - `cargo clippy` clean
- **Acceptance criteria:**
  - `AgentError::MaxStepsReached.to_string()` contains "max steps"
  - `AgentError::ContextOverflow.to_string()` contains "context overflow"
- **Test plan:**
  - unit: Test Display for each variant, test JSON roundtrip
- **Estimated effort:** 2 hours

---

### TASK-1.7: AgentConfig, AgentLimits, AgentFeatures, OutputLimits, ThinkLimits

- **§SPEC:** §4 (AgentConfig, AgentLimits, AgentFeatures), §12 (ThinkLimits), §18 (OutputLimits)
- **Labels:** `layer/foundation`, `priority/critical`
- **Description:** Define all configuration limit structs. `AgentLimits { max_steps, max_tool_calls, max_runtime: Duration, retry_on_error }`, `AgentFeatures { streaming }`, `OutputLimits { max_stdout_bytes, max_stderr_bytes, max_combined_bytes }`, `ThinkLimits { max_calls, max_tokens }`, `AgentConfig { limits, features, output, think }`. All derive `Serialize`, `Deserialize`, `Clone`, `Debug`, `PartialEq`, `JsonSchema`. Add `Default` impls with reasonable safe defaults (max_steps=50, max_tool_calls=100, max_runtime=10min, retry=2, output=4MB each, think=8 calls/4096 tokens, streaming=false).
- **Files affected:**
  - `crates/duga-types/src/config.rs` (new)
  - `crates/duga-types/src/lib.rs` (add module)
- **Types involved:** `AgentConfig`, `AgentLimits`, `AgentFeatures`, `OutputLimits`, `ThinkLimits`
- **Functions to implement:**
  - `impl Default for AgentLimits`
  - `impl Default for AgentFeatures`
  - `impl Default for OutputLimits`
  - `impl Default for ThinkLimits`
  - `impl Default for AgentConfig`
- **Dependencies:** TASK-1.1
- **Implementation steps:**
  1. Define `OutputLimits` struct
  2. Define `ThinkLimits` struct
  3. Define `AgentLimits` struct with `max_runtime: Duration` — add serde helper for human-readable duration ("10m", "120s")
  4. Define `AgentFeatures` struct
  5. Define `AgentConfig` struct composing all four
  6. Implement `Default` for each with documented values
  7. Implement custom `Serialize`/`Deserialize` for `Duration` via a serde module (parse "10m", "120s", "1h")
  8. Add unit tests
- **Edge cases:**
  - `Duration` serialization: must support "10m" (minutes), "120s" (seconds), "1h" (hours), "500ms" (milliseconds)
  - `max_runtime: 0` means "no limit"? → No, spec says bounded. Default must be positive. Reject zero in config validation (TASK-12.3).
  - `max_steps: 0` → agent terminates immediately after first limit check
- **Definition of Done:**
  - All structs compile with derives
  - Default impls produce expected values
  - JSON roundtrip with Duration serde works
  - `cargo clippy` clean
- **Acceptance criteria:**
  - `AgentConfig::default().limits.max_steps` → `50`
  - `serde_json::from_str::<AgentLimits>(r#"{"max_steps":10,"max_tool_calls":20,"max_runtime":"5m","retry_on_error":1}"#)` succeeds
  - Invalid duration format → deserialization error
- **Test plan:**
  - unit: Test Default values, test Duration serde (valid formats), test Duration serde rejection (invalid), test full AgentConfig roundtrip
- **Estimated effort:** 5 hours

---

### TASK-1.8: LlmCallOptions, LlmResponse, TokenUsage, SummaryMessage

- **§SPEC:** §6a (LlmCallOptions, LlmResponse, TokenUsage), §21–22 (SummaryMessage)
- **Labels:** `layer/foundation`, `priority/critical`
- **Description:** Define remaining data types used across the system. `LlmCallOptions { streaming: bool }`, `LlmResponse { message: AssistantMessage, usage: TokenUsage }`, `TokenUsage { prompt: u32, completion: u32 }`, `SummaryMessage { content: String, pinned_facts: Vec<String> }`. Also define `Seq(u64)` newtype for monotonic sequence numbers (used in EPIC-7).
- **Files affected:**
  - `crates/duga-types/src/llm.rs` (new)
  - `crates/duga-types/src/summary.rs` (new)
  - `crates/duga-types/src/seq.rs` (new)
  - `crates/duga-types/src/lib.rs` (add modules)
- **Types involved:** `LlmCallOptions`, `LlmResponse`, `TokenUsage`, `SummaryMessage`, `Seq`
- **Functions to implement:**
  - `Seq::next() -> Seq` (increment by 1, return new value — used by `SeqAllocator` in EPIC-7)
  - `impl Display for Seq`
  - `SummaryMessage::new(content: String) -> Self`
  - `SummaryMessage::with_pinned_facts(content: String, pinned_facts: Vec<String>) -> Self`
- **Dependencies:** TASK-1.1, TASK-1.3 (uses `AssistantMessage`)
- **Implementation steps:**
  1. Create `llm.rs`: define `TokenUsage`, `LlmCallOptions`, `LlmResponse`
  2. Create `summary.rs`: define `SummaryMessage`
  3. Create `seq.rs`: define `Seq(u64)` with `Serialize`, `Deserialize`, `Clone`, `Copy`, `Debug`, `PartialEq`, `PartialOrd`
  4. Implement `Seq::next()` (takes `&self` or `&mut self` — use value `self` → `Seq(self.0 + 1)` for pure functional)
  5. Add unit tests
- **Edge cases:**
  - `Seq` overflow: `u64::MAX + 1` — practically impossible (1 event per nanosecond = 585 years), do not handle
  - `SummaryMessage.pinned_facts` is empty vec by default — `with_pinned_facts` sets it
  - `TokenUsage.prompt` or `completion` may be zero (e.g., empty prompt, no completion tokens consumed)
- **Definition of Done:**
  - All types compile
  - JSON roundtrip for each
  - `cargo clippy` clean
- **Acceptance criteria:**
  - `Seq(0).next()` → `Seq(1)`
  - `SummaryMessage::new("summary").pinned_facts` → `vec![]`
  - `SummaryMessage::with_pinned_facts("s", vec!["fact".into()]).pinned_facts` → `vec!["fact"]`
- **Test plan:**
  - unit: Test Seq next and ordering, test SummaryMessage constructors, test JSON roundtrip for all four types
- **Estimated effort:** 3 hours
