# EPIC-26: Loop-Agnostic Core — Agent Loop System Redesign

**§SPEC:** §5 (Core Loop) — redesign  
**Labels:** `epic/loop`, `epic/core`  
**Crate:** `duga-core` (primary), `duga-types`, `duga-config`, `duga-runtime`, `duga-harness`, `duga-tools-builtin`

## Goal

Redesign the core agent loop system to be fully loop-agnostic. Any loop type can be added without touching the dispatcher, bot, classifier prompt, or output validation.

Instead of a separate meta-loop classifier (extra LLM call), loop selection is handled by the main LLM through a **`delegate` built-in tool**. The LLM decides inline whether to switch strategies — no new architectural layer, no extra latency, no separate model config.

---

## Architecture

```
Bot → SimpleReActLoop.run(task)
        │
        ├─ think / analyze task
        ├─ tools (shell, read, edit, write, search)
        ├─ delegate to another loop?  ← LLM decides via tool call
        │      │
        │      └─ lookup loop in registry → run it → return
        │
        └─ return final answer
```

### How it works

1. The LLM receives the task plus a system prompt listing available loops as strategies
2. During normal tool execution, the LLM may emit a `delegate` tool call:
   ```json
   {"tool": "delegate", "raw_args": {"loop": "problem_solving", "reason": "multi-step code generation"}}
   ```
3. The `DelegateTool` looks up the loop in the `LoopRegistry`, runs it with the current conversation state, and returns its result as the tool output
4. SimpleReActLoop detects the delegate result and returns it as the final answer (the delegated loop already produced the complete response)

No separate classifier. No `Core` dispatcher. The LLM is the classifier.

---

## Loop Registry

Core maintains a registry of available loops. Each loop is a self-contained unit registered at startup.

Registry maps: `loop_type_id → Box<dyn Loop>`

The system prompt's "Available Strategies" section is auto-generated from the registry — each loop provides its own `name`, `description`, and selection guidance. No manual prompt editing when adding new loops.

---

## Loop Contract

Every loop must implement:

```rust
trait Loop: Send + Sync {
    fn id(&self) -> &'static str;         // e.g. "simple_react"
    fn name(&self) -> &'static str;       // e.g. "Simple ReAct"
    fn description(&self) -> &'static str; // when to select (used in system prompt)
    async fn run(&self, task: String, ctx: &LoopContext<'_>) -> Result<LoopResult, AgentError>;
}
```

Nothing else. The loop that delegates does not care what happens inside `run()`.

### LoopContext

```rust
struct LoopContext<'a> {
    config: &'a AgentConfig,
    memory: &'a mut Memory,
    llm: &'a Arc<dyn LlmClient>,
    tools: &'a Arc<ToolDispatcher>,
    workspace: &'a Workspace,
    event_sink: &'a Arc<dyn EventSink>,
    summarizer: &'a Arc<dyn Summarizer>,
    cancellation: &'a CancellationToken,
    registry: &'a LoopRegistry,        // so loops can recursively delegate
    max_refinement_iterations: u32,
}
```

---

## Delegate Tool

A built-in tool registered alongside `think`, `shell`, etc.:

```rust
#[derive(Deserialize, JsonSchema)]
struct DelegateArgs {
    /// Which loop to hand control to
    loop: String,
    /// Why this loop was chosen (for logs and observability)
    reason: String,
}
```

When invoked:
1. Look up `args.loop` in `LoopContext.registry`
2. If not found or not enabled → return error as tool feedback (LLM sees it and can choose another)
3. Run `selected_loop.run(task, ctx).await`
4. Return the result output as tool output

The delegating loop (SimpleReActLoop) detects that the `delegate` tool produced the final answer and propagates it as its own result.

### System Prompt Integration

Registration auto-generates a prompt section:

```
## Available Strategies

You have access to a `delegate` tool that hands control to a
specialized loop for complex tasks. Use it when your current
approach isn't optimal.

Available loop types:
- problem_solving — Plan → execute → audit for code generation,
  multi-step reasoning, and structured tasks.
- verification — Generate multiple independent answers then vote.
  For factual accuracy and verification tasks.
- decomposition — Break into independent subtasks, solve separately,
  merge results. For large compound tasks.
- search — Query → search → evaluate → refine. For information
  retrieval and codebase exploration.

If the task doesn't need a specialized strategy, proceed directly
with your normal tools (think, shell, read, edit, write, search).
```

Only enabled loops appear. Adding a new loop automatically adds its description.

---

## Config

```yaml
agent:
  # ... existing limits, features, output, think ...

  loop:
    enabled_loops:                  # only registered loops listed here are active
      - problem_solving
      - verification
      - decomposition
      - search
    max_refinement_iterations: 3    # used by loops that need iteration cap
    max_delegation_depth: 2         # prevent infinite delegation chains
```

- `simple_react` is always the entry point (the bot always calls `simple_react`)
- Other loops are only reachable via `delegate`
- `max_delegation_depth` prevents Loop A → Loop B → Loop A → ... infinite chains

---

## Currently Planned Loops

Register in this order as implemented:

| ID | Name | Purpose |
|----|------|---------|
| `simple_react` | Simple ReAct | Direct tool calls, lookups, single-step tasks (current behavior — always the entry loop) |
| `problem_solving` | Problem Solving | Plan → execute → audit, code generation and multi-step reasoning |
| `verification` | Verification | Multiple independent answers → vote, factual accuracy tasks |
| `decomposition` | Decomposition | Split → solve subtasks → merge, large compound tasks |
| `search` | Search | Query → search → evaluate → refine, RAG / information retrieval |

Future (not yet scoped):

| ID | Name | Purpose |
|----|------|---------|
| `tree_of_thought` | Tree of Thought | Parallel branch exploration with pruning |
| `swarm` | Swarm | Multi-agent generator → critic → synthesizer |

---

## Extensibility Rules

Adding a new loop:
1. Implement the `Loop` trait
2. Register it in the `LoopRegistry` at startup
3. Add its `id` to `enabled_loops` in config when ready

Nothing else changes. Bot, dispatcher, system prompt, and `delegate` tool are all unaffected. The loop appears in the strategy list automatically.

---

## Rollout Plan (phased, zero-downtime)

| Phase | What | Behavior Change |
|-------|------|----------------|
| **1. Interface + Registry** | Define `Loop` trait, `LoopContext`, `LoopRegistry`. Extract existing ReAct logic into `SimpleReActLoop`. No `delegate` tool yet. | None — same behavior, different code path |
| **2. Delegate tool** | Register `DelegateTool` in builtin tools. Wire registry lookup. SimpleReActLoop propagates delegate results as final answer. | None — `delegate` exists but no other loops registered |
| **3. Add loops one by one** | Implement → register → add to `enabled_loops` → test | Each loop is opt-in via config |
| **4. Enable in staging** | Enable one alternate loop. Monitor LLM delegation decisions in logs. | LLM may delegate when appropriate |
| **5. Production** | Enable all tested loops. | Full loop-agnostic routing |

---

### TASK-26.1: Define Loop trait, LoopContext, LoopResult types

- **§SPEC:** §5 (redesign), Loop Contract
- **Labels:** `layer/loop`, `priority/critical`
- **Description:** Define the `Loop` trait, `LoopContext` struct, and `LoopResult` struct in `duga-core`. The `Loop` trait has four required items: `id()`, `name()`, `description()`, and `run()`. `LoopContext` bundles all runtime dependencies (borrowed, not owned). `LoopResult` is the unified return type replacing `AgentRunResult`.
- **Files affected:**
  - `crates/duga-core/src/loop_trait.rs` (new)
  - `crates/duga-core/src/loop_context.rs` (new)
  - `crates/duga-core/src/loop_result.rs` (new)
  - `crates/duga-core/src/lib.rs` (add modules)
- **Types involved:** `Loop`, `LoopContext<'a>`, `LoopResult`, `Event::LoopDelegated`
- **Functions to implement:**
  - `trait Loop`: `fn id(&self) -> &'static str; fn name(&self) -> &'static str; fn description(&self) -> &'static str; async fn run(&self, task: String, ctx: &LoopContext<'_>) -> Result<LoopResult, AgentError>`
  - `LoopContext<'a>`: holds `&'a AgentConfig`, `&'a mut Memory`, `&'a Arc<dyn LlmClient>`, `&'a Arc<ToolDispatcher>`, `&'a Workspace`, `&'a Arc<dyn EventSink>`, `&'a Arc<dyn Summarizer>`, `&'a CancellationToken`, `&'a LoopRegistry`, `max_refinement_iterations: u32`, `max_delegation_depth: u32`, `delegation_depth: u32`
  - `LoopResult`: struct with `message: AssistantMessage`, `steps: u32`, `tool_calls: u32`, `loop_id: String`
  - `Event::LoopDelegated { from: String, to: String, reason: String, depth: u32 }` — add this variant to the Event enum now so T26.5 can emit it
- **Dependencies:** TASK-1.3 (AssistantMessage), TASK-1.6 (AgentError), TASK-3.3 (ToolDispatcher), TASK-6.1 (Memory), TASK-8.1 (LlmClient), TASK-7.1 (Event enum)
- **Implementation steps:**
  1. Define `LoopResult` struct — superset of current `AgentRunResult` with `loop_id` field
  2. Define `LoopContext<'a>` — bundle of borrowed runtime dependencies, including both `max_delegation_depth` (constant from config) and `delegation_depth` (current depth, incremented on delegation)
  3. Define `Loop` trait with async `run()` method
  4. Ensure trait is object-safe (`Send + Sync` bounds)
  5. Add `Event::LoopDelegated` variant to the Event enum (so dependent tasks don't wait for T26.13)
  6. Add unit test: verify trait compiles with mock implementation
- **Edge cases:**
  - `LoopContext` includes both `max_delegation_depth` and `delegation_depth` — the interceptor compares them, `max` from config, `depth` incremented per delegation
  - `&mut Memory` in `LoopContext` — only one loop runs at a time, intentional. Delegation must happen inline (no concurrent loops)
  - Loop trait methods return `&'static str` — implementors use string literals
  - `LoopContext.registry` is `&'a LoopRegistry` (borrowed from the `Arc<LoopRegistry>` owned by `BuiltRuntime`)
- **Definition of Done:**
  - `Loop` trait compiles, `LoopContext` compiles, `LoopResult` serializes/deserializes
  - `Event::LoopDelegated` variant exists and roundtrips to JSON
  - `cargo check -p duga-core -p duga-types` passes
- **Acceptance criteria:**
  - Mock `Loop` impl compiles and runs
  - `LoopResult` JSON roundtrip works
  - `Event::LoopDelegated` serializes/deserializes correctly
- **Test plan:**
  - unit: Mock loop implementing trait, verify id/name/description
  - unit: `LoopResult` serialize/deserialize roundtrip
  - unit: `Event::LoopDelegated` JSON roundtrip
- **Estimated effort:** 4 hours

---

### TASK-26.2: Define LoopRegistry with system prompt generation

- **§SPEC:** §5 (redesign), Loop Registry
- **Labels:** `layer/loop`, `priority/critical`
- **Description:** Implement `LoopRegistry` mapping `loop_type_id → Box<dyn Loop>`. Methods: `register()`, `get()`, and `build_strategies_prompt()` which auto-generates the "Available Strategies" section of the system prompt from registered + enabled loops. No classifier logic — the registry is purely a lookup structure.
- **Files affected:**
  - `crates/duga-core/src/loop_registry.rs` (new)
  - `crates/duga-core/src/lib.rs` (add module)
- **Types involved:** `LoopRegistry`, `Loop` trait
- **Functions to implement:**
  - `LoopRegistry::new() -> Self`
  - `LoopRegistry::register(&mut self, loop_impl: Box<dyn Loop>) -> Result<(), LoopRegistryError>` — rejects duplicate `id`
  - `LoopRegistry::get(&self, id: &str) -> Option<&dyn Loop>`
  - `LoopRegistry::is_enabled(&self, id: &str, enabled_ids: &[String]) -> bool`
  - `LoopRegistry::build_strategies_prompt(&self, enabled_ids: &[String]) -> String` — builds the "Available Strategies" prompt block
- **Dependencies:** TASK-26.1 (Loop trait)
- **Implementation steps:**
  1. Define `LoopRegistryError` enum
  2. Define `LoopRegistry` with `HashMap<String, Box<dyn Loop>>` internally
  3. Implement `register()` with duplicate check
  4. Implement `get()` and `is_enabled()`
  5. Implement `build_strategies_prompt()` — formats each enabled loop's description:
     ```
     - {id} — {description}
     ```
  6. Add unit tests
- **Edge cases:**
  - Duplicate `id` on register → return error
  - `enabled_ids` references unknown id → silently ignore
  - Registry populated at startup, never modified at runtime
  - Prompt must include the `delegate` tool instructions + enabled loop list
- **Definition of Done:**
  - Registry compiles, all methods work
  - `build_strategies_prompt` output contains correct loop descriptions
  - `cargo test -p duga-core -- loop_registry` passes
- **Acceptance criteria:**
  - Register 3 mock loops, `get("id2")` returns correct loop
  - `build_strategies_prompt(&["id1", "id3"])` contains id1 and id3 but NOT id2
  - Duplicate register returns error
- **Test plan:**
  - unit: empty registry, single register, multiple, duplicate, get existing/missing, build_strategies_prompt content verification
- **Estimated effort:** 3 hours

---

### TASK-26.3: Extract existing ReAct loop into SimpleReActLoop

- **§SPEC:** §5 (current loop logic)
- **Labels:** `layer/loop`, `priority/critical`
- **Description:** Take the current `AgentLoop::run()` implementation and extract it into a `SimpleReActLoop` struct that implements the `Loop` trait. Zero behavior change: every existing test must pass. `SimpleReActLoop` is always the entry point — bots call `simple_react.run(task, ctx)`. Later tasks add delegation awareness.
- **Files affected:**
  - `crates/duga-core/src/loops/simple_react.rs` (new)
  - `crates/duga-core/src/loops/mod.rs` (new)
  - `crates/duga-core/src/agent_loop.rs` (deprecated, kept for backward compat)
- **Types involved:** `SimpleReActLoop`, `Loop` trait, `LoopContext`, `LoopResult`
- **Functions to implement:**
  - `SimpleReActLoop` — empty struct (stateless, all state in `LoopContext`)
  - `impl Loop for SimpleReActLoop` — `id = "simple_react"`, `name = "Simple ReAct"`, `description = "Direct tool calls for single-step and straightforward tasks. Uses think, shell, read, edit, write, and search tools. When a task requires structured planning, multi-step reasoning, verification, decomposition, or information retrieval, delegate to the appropriate specialized loop."`
  - `run()` contains the exact logic from current `AgentLoop::run()`, adapted to use `LoopContext` fields
- **Dependencies:** TASK-26.1 (Loop trait), TASK-10.x (existing AgentLoop logic)
- **Implementation steps:**
  1. Create `crates/duga-core/src/loops/` directory
  2. Create `simple_react.rs` with `SimpleReActLoop`
  3. Implement `Loop` trait
  4. Copy `AgentLoop::run()` logic → `SimpleReActLoop::run()`, adapting field access:
     - `self.config` → `ctx.config`
     - `self.memory` → `ctx.memory` (`&mut Memory`)
     - `self.llm` → `ctx.llm`
     - etc.
  5. Copy helper methods as private module functions
  6. Port all existing `AgentLoop` tests to use `SimpleReActLoop` + `LoopContext`
  7. All existing tests pass with identical assertions
- **Edge cases:**
  - `&mut Memory` in `LoopContext` — current `AgentLoop` owns `self.memory`, refactor to borrow
  - Task anchoring, tool reset, event emission all work identically via `LoopContext`
- **Definition of Done:**
  - All existing `AgentLoop` tests pass via `SimpleReActLoop`
  - Same assertions: result message, steps, tool_calls, cancellation, retry
  - `cargo test -p duga-core` passes
- **Acceptance criteria:**
  - `run_returns_terminal_assistant_message` passes on `SimpleReActLoop`
  - `run_executes_tools_and_feeds_results_to_next_step` passes
  - `cancellation_before_run_is_fatal` passes
  - Memory compression and retry tests pass
- **Test plan:**
  - unit: Duplicate all existing `AgentLoop` tests in `simple_react.rs` test module, adapted to `LoopContext` + `SimpleReActLoop`
- **Estimated effort:** 8 hours

---

### TASK-26.4: Implement DelegateTool (schema-only, for LLM visibility)

- **§SPEC:** §5 (redesign), Delegate Tool
- **Labels:** `layer/loop`, `layer/tools-builtin`, `priority/critical`
- **Description:** Implement `DelegateTool` as a built-in tool registered for **schema purposes only**. The LLM sees it in the tool list and can emit `delegate` tool calls, but the actual delegation logic lives in `SimpleReActLoop` (TASK-26.5) which intercepts the call before dispatching. The tool's `execute()` is a no-op that returns an error. The key responsibility of this task is making the tool's `description()` dynamically list which loops are available.
- **Files affected:**
  - `crates/duga-tools-builtin/src/delegate.rs` (new)
  - `crates/duga-tools-builtin/src/lib.rs` (register DelegateTool)
- **Types involved:** `DelegateTool`, `DelegateArgs`, `Tool` trait
- **Functions to implement:**
  - `DelegateArgs { loop: String, reason: String }` — derive `Deserialize`, `JsonSchema`. The `loop` field's schema description lists currently enabled loops: `"One of: problem_solving, verification"`
  - `DelegateTool::new(enabled_loops: Vec<String>) -> Self` — stores the list, formats a dynamic cached description string
  - `impl Tool for DelegateTool` — `name = "delegate"`, `description` returns the cached `String` as `&str`
  - `execute()`: returns `ToolError::InvalidArgs("delegate is handled by the agent loop — this tool cannot be called directly")` — this path should never be reached because `SimpleReActLoop` intercepts before dispatch
- **Dependencies:** TASK-26.2 (LoopRegistry — needed to know valid loop ids for schema), TASK-3.1 (Tool trait)
- **Implementation steps:**
  1. Define `DelegateArgs` with serde + JsonSchema. Dynamically set the `loop` field's schema `description` to `"One of: {enabled list}"`
  2. Define `DelegateTool` struct holding `enabled_ids: Vec<String>` and `cached_description: String`
  3. Format description at construction: `"Hand control to a specialized loop. Available: {list}. Use when your current approach isn't optimal."`
  4. Implement `Tool` trait: `name="delegate"`, `description` returns `&self.cached_description`, `retryable=false`
  5. `execute()` returns an error — the loop intercepts delegate before dispatch (T26.5)
  6. Register in the dispatcher after `DelegateTool::new()` (ordering handled in T26.7)
  7. Note: `description()` can't return `&'static str` — use a cached `String` field. The `Tool` trait's `description()` returns `&str` which can borrow from `self`.
- **Edge cases:**
  - `enabled_loops` is empty → description says `"No specialized loops available. Use your standard tools."`; schema lists no options
  - `simple_react` appears in `enabled_loops` → filter it out (it's the entry point, not a delegation target). Add config validation warning per T26.6.
  - LLM emits delegate for a loop not listed → the intercept in T26.5 catches it and returns helpful error
- **Definition of Done:**
  - `DelegateTool` compiles, implements `Tool`, registers in dispatcher
  - Schema description dynamically lists enabled loops (or "none available")
  - `execute()` returns error (guard against accidental dispatch)
  - `cargo test -p duga-tools-builtin` passes
- **Acceptance criteria:**
  - `delegate` appears in `ToolDispatcher::schemas()` with correct name and dynamic description
  - `enabled_loops: ["problem_solving", "verification"]` → description contains both ids
  - `enabled_loops: []` → description says "No specialized loops available"
  - Calling `delegate` through dispatcher directly returns error
- **Test plan:**
  - unit: DelegateTool schema with empty enabled list → description says "no loops"
  - unit: DelegateTool schema with 2 enabled loops → description lists both
  - unit: Dispatcher dispatch of delegate returns ToolError
  - unit: `simple_react` filtered from enabled list in description
- **Estimated effort:** 3 hours

---

### TASK-26.5: Wire delegation interception into SimpleReActLoop

- **§SPEC:** §5 (redesign), Architecture
- **Labels:** `layer/loop`, `priority/critical`
- **Description:** Update `SimpleReActLoop` to intercept `delegate` tool calls BEFORE dispatching. This is the only code that does actual delegation — `DelegateTool` (T26.4) is just a schema placeholder. When the LLM returns a response containing `delegate` in `tool_calls`: (1) intercept before the `for call in tool_calls` loop (or check at the top), (2) validate `ctx.delegation_depth < ctx.max_delegation_depth` — if at limit, push error to memory and let LLM continue without delegation, (3) look up the target loop in `ctx.registry`, (4) if not found or not enabled, push error to memory, (5) build a new `LoopContext` with `delegation_depth + 1`, (6) run `target_loop.run(task, &child_ctx)`, (7) push the result as tool output to memory, (8) emit `Event::LoopDelegated`, (9) return the delegated result as the final answer (skip remaining tool calls). If `delegate` was the ONLY tool call and it succeeded, this becomes the final answer. If there were other tool calls alongside `delegate`, delegate is handled first — if successful, skip the rest.
- **Files affected:**
  - `crates/duga-core/src/loops/simple_react.rs` (add delegation intercept)
- **Types involved:** `SimpleReActLoop`, `LoopContext`, `LoopRegistry`, `Event::LoopDelegated`
- **Functions to implement:**
  - `try_delegate(call: &ToolCall, task: &str, ctx: &LoopContext) -> Result<Option<LoopResult>, AgentError>` — returns `Some(result)` if delegation succeeded, `None` if it wasn't a delegate call, or pushes error to memory and returns `None` if delegation failed
- **Dependencies:** TASK-26.3 (SimpleReActLoop), TASK-26.4 (DelegateTool registered), TASK-26.1 (Event::LoopDelegated)
- **Implementation steps:**
  1. In `SimpleReActLoop::run()`, after the LLM returns a response with tool calls:
     a. Before the `for call in tool_calls` loop, scan for a `delegate` tool call
     b. If found:
        - Check `ctx.delegation_depth >= ctx.max_delegation_depth` → push error tool result to memory: `"max delegation depth ({max}) reached"` → continue loop normally (don't abort)
        - Look up `call.raw_args["loop"]` in `ctx.registry.get()`
        - If not found or not enabled → push error tool result: `"unknown loop 'X'. Available: [...]"` → continue
        - Build child `LoopContext`: borrow same fields (`memory`, `llm`, `tools`, etc.) but with `delegation_depth: ctx.delegation_depth + 1`
        - Run `target_loop.run(task.clone(), &child_ctx).await`
        - Push result output as tool result to memory
        - Emit `Event::LoopDelegated { from: "simple_react", to: target_id, reason, depth: ctx.delegation_depth + 1 }`
        - Return the `LoopResult` from the target loop as the final answer (terminate `SimpleReActLoop::run()`)
     c. If no delegate call found → proceed with normal tool execution loop
  2. `max_delegation_depth: 0` means the depth check always fails → delegate is effectively disabled, no code-path change needed
  3. No changes to limit checks, compression, or cancellation logic
- **Edge cases:**
  - LLM calls `delegate` + 2 other tools in the same `tool_calls` array → delegate is handled first; if it succeeds, remaining tools are skipped and the delegated result is returned
  - LLM calls `delegate` but target loop fails → the target loop's error result is pushed to memory; return it as final answer (the LLM's decision to delegate was committed)
  - `max_delegation_depth: 0` → check `ctx.delegation_depth >= 0` is true on first call → push error, continue
  - Delegate succeeded but output is empty → treat as normal result, return empty answer
  - Task anchoring: the target loop receives the same `task` string; the original task anchor is already in memory
  - Memory is `&mut` in `LoopContext` — the child context re-borrows the same memory. Since execution is sequential (the parent loop yields while child runs), this is safe.
- **Definition of Done:**
  - Delegating to another loop via `delegate` tool call produces correct final answer
  - Failed delegation (depth limit, unknown loop) pushes error to memory, run continues
  - Depth limit enforced per `max_delegation_depth`
  - `Event::LoopDelegated` emitted with correct from/to/reason/depth
  - `cargo test -p duga-core` passes
- **Acceptance criteria:**
  - Mock LLM emits `delegate { loop: "problem_solving" }` → target loop runs, its result returned as final answer
  - Mock LLM emits `delegate` + `shell` in same turn → delegate result returned, shell skipped
  - `max_delegation_depth: 0` → delegate intercepted, error pushed to memory, LLM continues without delegation
  - `max_delegation_depth: 1` → first delegation works, second fails with depth error
  - Unknown loop id → error pushed to memory, run continues
  - `Event::LoopDelegated` emitted
- **Test plan:**
  - unit: Mock LLM emits single `delegate` → target loop runs, result returned
  - unit: Mock LLM emits `delegate` after other tools → delegate result used, remaining skipped
  - unit: Depth limit hit → error in memory, no delegation
  - unit: Unknown loop → error in memory, run continues
  - unit: `Event::LoopDelegated` emission verification via RecordingSink
  - unit: Delegated loop runs with correct `delegation_depth` in its `LoopContext`
- **Estimated effort:** 6 hours

---

### TASK-26.6: Add loop config to AgentConfig and Config

- **§SPEC:** §32 (Config), Config section of loop redesign
- **Labels:** `layer/config`, `priority/high`
- **Description:** Add `LoopConfig` struct to `AgentConfig`. Fields: `enabled_loops: Vec<String>`, `max_refinement_iterations: u32`, `max_delegation_depth: u32`. Provide sensible defaults. Wire into `Config` YAML deserialization. Add validation: `enabled_loops` may be empty (meaning only `simple_react` available, delegate tool returns error for all loops), `max_delegation_depth` must be ≥ 0.
- **Files affected:**
  - `crates/duga-types/src/config.rs` (add `LoopConfig`, add `loop: LoopConfig` field to `AgentConfig`)
  - `crates/duga-config/src/config.rs` (add validation)
- **Types involved:** `LoopConfig`, `AgentConfig`
- **Functions to implement:**
  - `LoopConfig` struct with serde derives
  - `impl Default for LoopConfig`
  - Config validation
- **Dependencies:** TASK-1.7 (AgentConfig)
- **Implementation steps:**
  1. Define `LoopConfig`:
     ```rust
     pub struct LoopConfig {
         #[serde(default)]
         pub enabled_loops: Vec<String>,
         #[serde(default = "default_max_refinement_iterations")]
         pub max_refinement_iterations: u32,
         #[serde(default = "default_max_delegation_depth")]
         pub max_delegation_depth: u32,
     }
     ```
  2. Defaults: `enabled_loops: vec![]`, `max_refinement_iterations: 3`, `max_delegation_depth: 2`
  3. Add `#[serde(default)] loop: LoopConfig` to `AgentConfig`
  4. Validation:
     - `max_refinement_iterations == 0` → error (must be at least 1)
     - `max_delegation_depth > 10` → warning (suspiciously high)
  5. Add unit tests
- **Edge cases:**
  - Backward compat: existing configs without `loop:` block work (serde default)
  - `enabled_loops` empty → delegate tool always returns error with "no loops enabled"
  - Duplicate ids in `enabled_loops` → warn, deduplicate
- **Definition of Done:**
  - Config deserialization includes loop config
  - Existing configs work without `loop:` block
  - `cargo test -p duga-types -p duga-config` passes
- **Acceptance criteria:**
  - `AgentConfig::default().loop_config.max_delegation_depth` → `2`
  - YAML with `loop:` block parses correctly
  - YAML without `loop:` block uses defaults
  - `max_refinement_iterations: 0` → validation error
- **Test plan:**
  - unit: `LoopConfig::default()` values
  - unit: Full YAML deserialize with loop block
  - unit: Minimal YAML without loop block (backward compat)
  - unit: Validation errors
- **Estimated effort:** 3 hours

---

### TASK-26.7: Wire loop system into harness, runtime, and bot frontends

- **§SPEC:** §33 (Build/Harness), Architecture
- **Labels:** `layer/runtime`, `layer/frontend`, `priority/critical`
- **Description:** Update `duga-runtime::build_agent()` to construct a `LoopRegistry`, register enabled loops, build `SimpleReActLoop`, and wire `DelegateTool` into the tool dispatcher. Update `BuiltRuntime` to hold the registry. Update harness and telegram bot to construct and pass the registry. Keep backward compatibility via deprecated `AgentLoop` wrapper that delegates to `SimpleReActLoop`.
- **Files affected:**
  - `crates/duga-runtime/src/agent.rs` (build_agent update, BuiltRuntime)
  - `crates/duga-runtime/src/tools.rs` (register DelegateTool)
  - `crates/duga-runtime/src/lib.rs` (exports)
  - `crates/duga-harness/src/main.rs` (use new wiring)
  - `crates/duga-telegram-bot/src/` (any AgentLoop → new wiring)
- **Types involved:** `LoopRegistry`, `SimpleReActLoop`, `DelegateTool`, `BuiltRuntime`, `AgentLoop` (deprecated)
- **Functions to implement:**
  - `build_loop_registry(enabled_ids: &[String]) -> LoopRegistry` — registers all loops, returns registry
  - Update `build_agent` / `build_core` to accept `LoopRegistry` and register `DelegateTool`
  - `builtin_tools_with_delegate(registry: Arc<LoopRegistry>, enabled: &[String], max_depth: u32) -> Vec<Box<dyn Tool>>`
- **Dependencies:** TASK-26.3 (SimpleReActLoop), TASK-26.4 (DelegateTool), TASK-26.5 (delegation awareness), TASK-26.6 (config)
- **Implementation steps:**
  1. In `duga-runtime/src/tools.rs`: after `build_dispatcher()` builds the dispatcher and registers builtin tools, register `DelegateTool`:
     ```rust
     let dispatcher = build_dispatcher(&config, workspace)?;  // registers shell, read, etc.
     let registry = build_loop_registry(&config);              // builds LoopRegistry
     let delegate = DelegateTool::new(config.loop.enabled_loops.clone());
     dispatcher.register_erased(ErasedTool::erase(delegate))?; // added after builtin tools
     ```
     The ordering matters: `LoopRegistry` is built after config is loaded, and `DelegateTool` needs the enabled loop list at construction time.
  2. In `duga-runtime/src/agent.rs`:
     a. Create `build_loop_registry()` — register `SimpleReActLoop` + any enabled specialized loops from config
     b. Update `build_agent()` to accept and store `Arc<LoopRegistry>`
     c. `BuiltRuntime` gains `registry: Arc<LoopRegistry>`
     d. Keep `build_agent` returning `AgentLoop` (deprecated wrapper) for backward compat
  3. In harness main: build registry, pass to agent construction
  4. In telegram bot: same migration
- **Edge cases:**
  - No loops enabled beyond `simple_react` → `DelegateTool` registered but always returns error → LLM learns not to use it
  - Registry must outlive all tool calls → `Arc<LoopRegistry>` shared
  - `ToolContext` must carry registry reference for `DelegateTool::execute()`
- **Definition of Done:**
  - `cargo build --workspace` succeeds
  - `cargo test --workspace` passes
  - Harness runs end-to-end with same behavior
- **Acceptance criteria:**
  - `./harness "echo hello"` → same output as before
  - `enabled_loops: ["problem_solving"]` → delegate tool schema includes "problem_solving" in description
  - `enabled_loops: []` → delegate tool returns error when called
- **Test plan:**
  - integration: Harness with test config, verify output matches pre-refactor behavior
  - unit: `build_loop_registry` returns correct loops
  - unit: `DelegateTool` registered in dispatcher schemas
- **Estimated effort:** 6 hours

---

### TASK-26.8: Implement ProblemSolving loop

- **§SPEC:** §5 (redesign), Planned Loops
- **Labels:** `layer/loop`, `priority/normal`
- **Description:** Implement the `problem_solving` loop. Plan → Execute → Audit pattern for code generation and multi-step reasoning. Flow: (1) Planning: LLM creates step-by-step plan, (2) Execution: each step executed via tool calls, (3) Audit: LLM reviews output against plan, identifies gaps, re-enters planning if needed (up to `max_refinement_iterations`). Stateless — all state in `LoopContext`.
- **Files affected:**
  - `crates/duga-core/src/loops/problem_solving.rs` (new)
  - `crates/duga-core/src/loops/mod.rs` (add module)
- **Types involved:** `ProblemSolvingLoop`, `Loop` trait, `LoopContext`
- **Functions to implement:**
  - `ProblemSolvingLoop` — empty struct
  - `impl Loop` — `id = "problem_solving"`, `name = "Problem Solving"`, `description = "Plan → execute → audit cycle for code generation, multi-step reasoning, and tasks requiring structured thinking. Use when the task involves generating complex code, designing architecture, or solving multi-stage problems."`
  - Private helpers: `plan_phase()`, `execute_plan()`, `audit_phase()`
- **Dependencies:** TASK-26.1 (Loop trait), TASK-26.3 (SimpleReActLoop — reuse internal tool execution)
- **Implementation steps:**
  1. `plan_phase()`: LLM call prompting step-by-step plan, store as pinned messages
  2. `execute_plan()`: for each step, run mini ReAct cycle (reuse `SimpleReActLoop` internals)
  3. `audit_phase()`: LLM reviews output vs plan, if gaps found re-enter planning (up to `max_refinement_iterations`)
  4. Result includes accumulated output from all steps
- **Edge cases:**
  - Plan generation fails → return error with explanation
  - Plan has 0 steps → treat as single-step, execute directly
  - `max_refinement_iterations` reached → return best result with note
  - Plan step fails → continue to next step, report in audit
- **Definition of Done:** Compiles, implements `Loop` trait, Plan→Execute→Audit cycle works with mock LLM
- **Acceptance criteria:**
  - Multi-step plan created, executed, audited
  - Refinement respects iteration cap
  - Audit detects gaps → re-plans
- **Test plan:** unit with mock LLM returning plan + execution + audit responses
- **Estimated effort:** 10 hours

---

### TASK-26.9: Implement Verification loop

- **§SPEC:** §5 (redesign), Planned Loops
- **Labels:** `layer/loop`, `priority/normal`
- **Description:** Implement the `verification` loop. Generates N independent answers via separate reasoning paths, then votes for the best one. Flow: (1) Run N answer generations (each a mini ReAct cycle with varied prompts), (2) Voting: present all answers to LLM, ask it to select best or synthesize consensus, (3) Return selected/consensus answer. Answer count configurable via `max_refinement_iterations`.
- **Files affected:**
  - `crates/duga-core/src/loops/verification.rs` (new)
  - `crates/duga-core/src/loops/mod.rs` (add module)
- **Types involved:** `VerificationLoop`, `Loop` trait, `LoopContext`
- **Functions to implement:**
  - `VerificationLoop` — empty struct
  - `impl Loop` — `id = "verification"`, `name = "Verification"`, `description = "Generates multiple independent answers then votes for the most accurate one. For factual questions, verification tasks, and correctness-critical work."`
  - Private helpers: `generate_answer()`, `vote()`
- **Dependencies:** TASK-26.1 (Loop trait), TASK-26.3 (SimpleReActLoop)
- **Implementation steps:**
  1. `generate_answer(i)`: runs mini ReAct cycle with prompt "Attempt {i}/{n}. Focus on accuracy."
  2. All generations share same `LoopContext.memory` (conversation accumulates)
  3. `vote()`: single LLM call presenting all answers, asking for best + reasoning
  4. Return selected answer
- **Edge cases:**
  - `max_refinement_iterations: 1` → single generation, no voting
  - All answers agree → return consensus
  - All answers disagree → vote selects best, note disagreement
  - One generation fails → skip it, continue with remaining
- **Definition of Done:** Compiles, multi-generation + voting works with mock LLM
- **Acceptance criteria:**
  - 3 independent answers generated, best selected via vote
  - Consensus case: agreed answer returned
  - Conflict case: selected answer returned with disagreement noted
- **Test plan:** unit with mock LLM returning varied answers + vote response
- **Estimated effort:** 8 hours

---

### TASK-26.10: Implement Decomposition loop

- **§SPEC:** §5 (redesign), Planned Loops
- **Labels:** `layer/loop`, `priority/normal`
- **Description:** Implement the `decomposition` loop. Breaks a large compound task into independent subtasks, solves each separately, then merges results. Flow: (1) Decompose: LLM produces subtask list, (2) Solve: each subtask solved independently (reuses `SimpleReActLoop` logic), (3) Merge: LLM combines all subtask results into final answer.
- **Files affected:**
  - `crates/duga-core/src/loops/decomposition.rs` (new)
  - `crates/duga-core/src/loops/mod.rs` (add module)
- **Types involved:** `DecompositionLoop`, `Loop` trait, `LoopContext`
- **Functions to implement:**
  - `DecompositionLoop` — empty struct
  - `impl Loop` — `id = "decomposition"`, `name = "Decomposition"`, `description = "Breaks a large task into independent subtasks, solves each separately, then merges results. For compound tasks where sub-problems can be solved independently."`
  - Private helpers: `decompose()`, `solve_subtask()`, `merge_results()`
- **Dependencies:** TASK-26.1 (Loop trait), TASK-26.3 (SimpleReActLoop)
- **Implementation steps:**
  1. `decompose()`: LLM call → JSON subtask list `[{title, description}]`
  2. `solve_subtask()`: for each, run mini ReAct cycle, collect result
  3. `merge_results()`: LLM call → synthesize all results into final answer
  4. Cap subtask count at `max_refinement_iterations`
- **Edge cases:**
  - Task can't be decomposed → fall back to direct execution
  - Subtask fails → mark incomplete, continue, report in merge
  - Single subtask → degenerate, similar to SimpleReAct
- **Definition of Done:** Compiles, Decompose→Solve→Merge works with mock LLM
- **Acceptance criteria:**
  - 3 subtasks created, solved independently, merged
  - Empty decomposition → fallback to direct execution
  - Failed subtask → remaining complete, merge notes failure
- **Test plan:** unit with mock LLM returning subtasks + subtask solutions + merge response
- **Estimated effort:** 8 hours

---

### TASK-26.11: Implement Search loop (RAG workflow)

- **§SPEC:** §5 (redesign), Planned Loops
- **Labels:** `layer/loop`, `priority/normal`
- **Description:** Implement the `search` loop for Retrieval-Augmented Generation. Flow: (1) Query formulation: convert task into search queries, (2) Search + Read: use `search` and `read` tools, (3) Evaluate: assess result sufficiency, (4) Refine: reformulate if insufficient, (5) Answer: synthesize findings. Iterates up to `max_refinement_iterations` search-refine cycles.
- **Files affected:**
  - `crates/duga-core/src/loops/search_loop.rs` (new)
  - `crates/duga-core/src/loops/mod.rs` (add module)
- **Types involved:** `SearchLoop`, `Loop` trait, `LoopContext`
- **Functions to implement:**
  - `SearchLoop` — empty struct
  - `impl Loop` — `id = "search"`, `name = "Search"`, `description = "Query → search → evaluate → refine cycle for information retrieval. For finding information in the workspace, codebase exploration, document search, or RAG workflows."`
  - Private helpers: `formulate_query()`, `execute_search()`, `evaluate_results()`, `synthesize_findings()`
- **Dependencies:** TASK-26.1 (Loop trait), TASK-26.3 (SimpleReActLoop), TASK-4.7 (SearchTool), TASK-4.1 (ReadTool)
- **Implementation steps:**
  1. `formulate_query()`: LLM call → specific search queries
  2. `execute_search()`: call `search` + `read` tools via `LoopContext.tools`
  3. `evaluate_results()`: LLM call → "sufficient?" + refinement suggestions
  4. `synthesize_findings()`: LLM call → final answer from findings
  5. Repeat from step 2 if insufficient (up to `max_refinement_iterations`)
- **Edge cases:**
  - Search finds nothing → reformulate with different terms
  - Results sufficient on first try → skip refinement
  - Refinement limit reached → return best-effort answer
- **Definition of Done:** Compiles, Query→Search→Evaluate→Refine cycle works with mock tools
- **Acceptance criteria:**
  - "find auth logic" → formulates query, searches, reads, answers
  - Empty results → reformulates and retries
  - `max_refinement_iterations: 1` → no refinement
- **Test plan:** unit with mock tools returning search results + mock LLM for evaluation
- **Estimated effort:** 8 hours

---

### TASK-26.12: Register all loops and integration tests

- **§SPEC:** §5 (redesign), Rollout phases 3–4
- **Labels:** `layer/loop`, `layer/testing`, `priority/high`
- **Description:** Register all implemented loops in `LoopRegistry` at startup. Create integration tests: (1) LLM delegates to each loop type, (2) delegation chains (A → B), (3) delegation depth limit, (4) unknown loop handling, (5) extensibility — registering a new loop auto-adds it to delegate tool schema and system prompt.
- **Files affected:**
  - `crates/duga-core/src/loops/mod.rs` (re-export all loops)
  - `crates/duga-core/tests/loop_integration.rs` (new)
  - `crates/duga-core/tests/delegation_e2e.rs` (new)
- **Types involved:** `LoopRegistry`, all loop types, `DelegateTool`, `SimpleReActLoop`
- **Functions to implement:**
  - `register_default_loops(registry: &mut LoopRegistry)` — registers ProblemSolving, Verification, Decomposition, Search
  - Integration test helpers
- **Dependencies:** TASK-26.4 (DelegateTool), TASK-26.5 (delegation awareness), TASK-26.8–26.11 (loop impls)
- **Implementation steps:**
  1. `register_default_loops()` called at startup
  2. Integration tests:
     a. Mock LLM emits `delegate { loop: "problem_solving" }` → problem_solving runs → result returned
     b. Mock LLM emits `delegate { loop: "verification" }` → verification runs
     c. Delegate → unknown loop → error in memory → LLM retries with correct id
     d. Depth limit: delegate chain of 3 with `max_depth: 2` → third returns error
     e. Extensibility: register mock loop → verify in delegate tool schema and system prompt
  3. `cargo test -p duga-core` passes
- **Edge cases:**
  - All loops registered but none enabled → delegate always fails, LLM learns this
  - Delegation chain: SimpleReAct → ProblemSolving → Verification (depth 2) works if `max_depth >= 2`
- **Definition of Done:**
  - All 5 loops registered + delegate tool functional
  - Integration tests pass for delegation to each loop type
  - Extensibility verified
- **Acceptance criteria:**
  - Delegate to each loop type works
  - Unknown loop → retry works
  - Depth limit enforced
  - New loop registration → auto-appears in prompt/schema
- **Test plan:**
  - integration: Mock LLM + real LoopRegistry + mock target loops
  - integration: Depth limit scenarios
  - integration: Extensibility — register mock, check prompt, execute via delegate
- **Estimated effort:** 8 hours

---

### TASK-26.13: System prompt auto-generation and observability

- **§SPEC:** §5 (redesign), System Prompt Integration, §31 (Observability)
- **Labels:** `layer/loop`, `layer/observability`, `priority/high`
- **Description:** Integrate loop strategy descriptions into the system prompt automatically via `LoopRegistry::build_strategies_prompt()`. The prompt section is injected at agent construction time. Add tracing spans for loop execution and delegation: `#[instrument]` on each loop's `run()` and on the delegation intercept in `SimpleReActLoop`. Log delegation chains at INFO level showing which loop ran, depth, and reason.
- **Files affected:**
  - `crates/duga-runtime/src/agent.rs` (inject strategies prompt into system prompt)
  - `crates/duga-core/src/loops/simple_react.rs` (add tracing spans)
  - `crates/duga-core/src/loops/*.rs` (add tracing spans to each loop's `run()`)
- **Types involved:** `LoopRegistry`, system prompt construction, tracing
- **Functions to implement:**
  - `build_system_prompt(base: &str, registry: &LoopRegistry, enabled: &[String]) -> String` — appends strategies section from `build_strategies_prompt()`
  - Tracing: `#[instrument]` on each loop's `run()` — fields: `loop_id`, `delegation_depth`, `task` (snipped)
  - Tracing: `#[instrument]` on the delegation intercept — fields: `from_loop`, `to_loop`, `reason`, `depth`
- **Dependencies:** TASK-26.2 (LoopRegistry), TASK-26.5 (delegation awareness), TASK-26.6 (config)
- **Implementation steps:**
  1. In `duga-runtime/src/agent.rs`: after building base system prompt, append `\n\n## Available Strategies\n\n{registry.build_strategies_prompt(&config.loop.enabled_loops)}`
  2. When `enabled_loops` is empty, the prompt says: `"No specialized loops available. Proceed with your standard tools (think, shell, read, edit, write, search)."`
  3. Add `#[tracing::instrument(skip(ctx), fields(loop_id = %self.id(), depth = ctx.delegation_depth))]` to each loop's `run()`
  4. Add `#[tracing::instrument(skip(ctx), fields(from = "simple_react", to = %target_id, reason = %reason, depth = ctx.delegation_depth + 1))]` to the delegation intercept
  5. Log at INFO: `"Delegating to {target_id} (depth {new_depth}): {reason}"` on successful delegation; `"Delegation blocked: max depth reached"` on depth limit
  6. Note: `Event::LoopDelegated` variant was already added in TASK-26.1 — emit it from the intercept here
- **Edge cases:**
  - System prompt grows with many enabled loops → cap at reasonable length (each loop description is ~1 sentence, total strategies section ~15 lines max)
  - Tracing fields must not leak full task text → use `task = %task.chars().take(80).collect::<String>()` or the `task` field as `%task_snippet`
  - `delegation_depth` in logs must reflect the *new* depth (after increment)
- **Definition of Done:**
  - System prompt includes auto-generated strategies section (injected at agent construction)
  - Tracing spans capture loop execution and delegation chains
  - INFO-level logs show delegation decisions for production monitoring
- **Acceptance criteria:**
  - System prompt with 2 enabled loops contains both in strategies section
  - System prompt with 0 enabled loops contains "no specialized loops" note
  - Tracing logs show: `loop_id`, `depth`, delegation `from`/`to`/`reason`
- **Test plan:**
  - unit: `build_system_prompt` with various enabled_loops configs
  - unit: Tracing span content verification (tracing-test crate)
  - unit: Empty enabled_loops → prompt says "no specialized loops"
- **Estimated effort:** 3 hours

---

### TASK-26.14: Documentation and migration guide

- **§SPEC:** Architecture, Config, Rollout
- **Labels:** `layer/docs`, `priority/normal`
- **Description:** Write comprehensive documentation. Cover: (1) Architecture — how the loop system works, delegation flow, (2) Adding a new loop — step-by-step with code example, (3) Config reference — all `loop.*` fields, (4) Migration guide — `AgentLoop` → `SimpleReActLoop`, `AgentRunResult` → `LoopResult`, (5) Loop design guidelines — when to create a new loop vs use an existing one, (6) `delegate` tool usage guidance for the LLM. Update `docs/architecture.md` §5.
- **Files affected:**
  - `docs/architecture.md` (update §5)
  - `docs/loop-system.md` (new)
  - `crates/duga-core/README.md` (update)
- **Dependencies:** TASK-26.1 through TASK-26.13
- **Implementation steps:**
  1. Create `docs/loop-system.md` with all sections
  2. Update `docs/architecture.md` §5 — replace monolithic loop with new architecture
  3. Update `crates/duga-core/README.md` with loop system overview
- **Definition of Done:** All docs exist and are accurate
- **Acceptance criteria:** New contributor can add a loop by following the guide without reading source code
- **Estimated effort:** 4 hours

---

### TASK-26.15: Remove deprecated AgentLoop, AgentRunResult, and migrate AgentLoop API surface

- **§SPEC:** §5 (redesign), cleanup
- **Labels:** `layer/loop`, `priority/high`
- **Description:** Once all call sites use `SimpleReActLoop` and `LoopResult` (TASK-26.7), remove deprecated types. Delete `AgentLoop` struct and `crates/duga-core/src/agent_loop.rs`. Delete `AgentRunResult`. Remove deprecated re-exports. Migrate the `AgentLoop` API surface that callers depended on: `restore_history()` moves to `Memory` (it already operates on `self.memory`); `memory()` accessor moves to `BuiltRuntime` (which owns the `Memory` between runs). Update any remaining test references.
- **Files affected:**
  - `crates/duga-core/src/agent_loop.rs` (delete)
  - `crates/duga-core/src/lib.rs` (remove deprecated re-exports)
  - `crates/duga-core/src/memory.rs` (add `restore_history` public method if not already present)
  - `crates/duga-runtime/src/agent.rs` (remove deprecated `build_agent` wrapper; expose `memory()` on `BuiltRuntime`)
  - Any test files still referencing `AgentLoop` / `AgentRunResult`
- **Types involved:** `AgentLoop` (delete), `AgentRunResult` (delete)
- **Functions to implement:**
  - `Memory::restore_history(&mut self, messages: Vec<Message>)` — move from `AgentLoop::restore_history` (the current impl already delegates to `Memory::enforce_window()`)
  - `BuiltRuntime::memory(&self) -> &Memory` and `BuiltRuntime::memory_mut(&mut self) -> &mut Memory` — expose for callers that need pre-run history restoration
- **Dependencies:** TASK-26.7 (all consumers migrated), TASK-26.12 (integration tests on new system)
- **Implementation steps:**
  1. Move `AgentLoop::restore_history` logic into `Memory::restore_history` (the logic already operates purely on `self.memory` fields — push messages + enforce window)
  2. Add `memory()` and `memory_mut()` accessors to `BuiltRuntime`
  3. Update telegram bot: replace `agent.restore_history(history)` → `runtime.memory_mut().restore_history(history)`
  4. Grep for `AgentLoop` and `AgentRunResult` — confirm no non-test usage remains
  5. Delete `agent_loop.rs`
  6. Remove `pub mod agent_loop` and deprecated re-exports from `lib.rs`
  7. Remove deprecated `build_agent` wrapper from `duga-runtime`
  8. Update any core tests using `AgentLoop` → use `SimpleReActLoop` + `LoopContext`
  9. `cargo build --workspace` + `cargo test --workspace` pass
  10. `cargo clippy --workspace -- -D warnings` clean
- **Edge cases:**
  - Third-party code referencing `AgentLoop` → compile error with migration path in release notes. Acceptable — `AgentLoop` was never a stable public API.
  - `restore_history` behavior must remain identical: push messages, enforce window, log before/after counts
  - `memory()` on `BuiltRuntime` returns `&Memory` — read-only for callers that need token counts or message inspection
- **Definition of Done:**
  - `grep -r "AgentLoop" crates/` returns zero results (except docs/changelog)
  - `grep -r "AgentRunResult" crates/` returns zero results
  - `restore_history` available via `Memory`
  - All behavior preserved via `SimpleReActLoop` + `BuiltRuntime`
- **Estimated effort:** 4 hours

---

## Dependency Graph (internal to EPIC-26)

```
T26.1 (Loop trait, LoopContext, LoopResult, Event::LoopDelegated)
  ├── T26.2 (LoopRegistry — depends on T26.1)
  │     ├── T26.4 (DelegateTool schema-only — depends on T26.1 + T26.2)
  │     └── T26.13 (System prompt + observability — depends on T26.2 + T26.5 + T26.6)
  └── T26.3 (SimpleReActLoop — depends on T26.1)
        └── T26.5 (Delegation intercept — depends on T26.1 + T26.3 + T26.4)
              └── T26.7 (Wire into harness — depends on T26.3–T26.6)
                    └── T26.12 (Register all + integration — depends on T26.4 + T26.5 + T26.8–T26.11)
                          └── T26.15 (Remove deprecated — depends on T26.7 + T26.12)

T26.6 (Config — independent, can be done in parallel with T26.1–T26.3)

T26.8 (ProblemSolving — depends on T26.1 + T26.3)
T26.9 (Verification — depends on T26.1 + T26.3)
T26.10 (Decomposition — depends on T26.1 + T26.3)
T26.11 (Search — depends on T26.1 + T26.3)

T26.14 (Documentation — depends on T26.1–T26.13)
```

## Summary

| Task | Name | Est. Hours |
|------|------|------------|
| TASK-26.1 | Define Loop trait, LoopContext, LoopResult, Event::LoopDelegated | 4 |
| TASK-26.2 | Define LoopRegistry with prompt generation | 3 |
| TASK-26.3 | Extract SimpleReActLoop (zero behavior change) | 8 |
| TASK-26.4 | Implement DelegateTool (schema-only) | 3 |
| TASK-26.5 | Wire delegation interception into SimpleReActLoop | 6 |
| TASK-26.6 | Add loop config | 3 |
| TASK-26.7 | Wire loop system into harness/runtime/bot | 6 |
| TASK-26.8 | ProblemSolving loop | 10 |
| TASK-26.9 | Verification loop | 8 |
| TASK-26.10 | Decomposition loop | 8 |
| TASK-26.11 | Search loop (RAG) | 8 |
| TASK-26.12 | Register all loops + integration tests | 8 |
| TASK-26.13 | System prompt auto-generation + observability | 3 |
| TASK-26.14 | Documentation + migration guide | 4 |
| TASK-26.15 | Remove deprecated AgentLoop + migrate API surface | 4 |
| **Total** | | **86** |

## High-Risk Items

| Risk | Impact | Mitigation |
|------|--------|------------|
| `&mut Memory` in `LoopContext` — all loops share one conversation | Medium — verification loop generates multiple answers in same context, could confuse LLM | Intentional for v1. Future: `Memory::snapshot()` + `Memory::restore()` for isolated branches per generation. |
| `AgentRunResult` → `LoopResult` API break | Medium — all bot code must update result destructuring | `#[deprecated]` type alias during transition. Full removal in T26.15 after all consumers migrate. |
| `delegate` tool adds schema size | Low — one extra tool in dispatcher | Negligible. Tool schema is ~200 bytes. LLM already handles 6+ tools. |
| System prompt grows with many loops | Low — each loop adds ~1 sentence | Strategy section is ~3 lines + 1 line per enabled loop. Even with 10 loops it's under 20 lines. |
| Infinite delegation chains | Low — depth counter prevents this | `max_delegation_depth` enforces hard limit at `DelegateTool` level. Default 2 prevents runaway chains. |
| LLM never learns to delegate | Medium — model may never emit `delegate` tool call | Acceptable — `simple_react` handles any task. Delegation is an optimization. Can add a prompt nudge in system message: "For complex multi-step tasks, consider using the delegate tool." |

---

## Implementation Status (2026-05-21)

| Task | Name | Status | Notes |
|------|------|--------|-------|
| TASK-26.1 | Loop trait, LoopContext, LoopResult, Event::LoopDelegated | ✅ Done | |
| TASK-26.2 | LoopRegistry with prompt generation | ✅ Done | |
| TASK-26.3 | Extract SimpleReActLoop | ✅ Done | Zero behavior change |
| TASK-26.4 | DelegateTool (schema-only) | ✅ Done | Registered in dispatcher |
| TASK-26.5 | Delegation intercept in SimpleReActLoop | ✅ Done | Depth limiting, error handling |
| TASK-26.6 | LoopConfig in AgentConfig | ✅ Done | Backward-compatible defaults |
| TASK-26.7 | Wire into harness/runtime/bot | ✅ Done | All 3 frontends migrated |
| TASK-26.8 | ProblemSolving loop | ✅ Done | Plan→Execute→Audit with mock tests |
| TASK-26.9 | Verification loop | ✅ Done | N-generations + voting with mock tests |
| TASK-26.10 | Decomposition loop | ✅ Done | Decompose→Solve→Merge with mock tests |
| TASK-26.11 | Search loop (RAG) | ✅ Done | Query→Search→Evaluate→Refine with mock tests |
| TASK-26.12 | Register all + integration tests | ✅ Done | All loops registered, e2e tests migrated |
| TASK-26.13 | System prompt + observability | ✅ Done | LoopRegistry::build_strategies_prompt + #[instrument] |
| TASK-26.14 | Documentation + migration guide | ✅ Done | docs/loop-system.md, README, architecture updates |
| TASK-26.15 | Remove deprecated AgentLoop | ✅ Done | agent_loop.rs deleted, all consumers migrated |

**All 15 tasks complete. 49 tests pass.**
