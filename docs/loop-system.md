# Loop System — Loop-Agnostic Core Architecture

## Overview

The loop system is the **pluggable execution strategy** layer.  The LLM
selects which strategy to use by invoking the `delegate` built-in tool.
No separate classifier, no extra latency, no new architectural layer.

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

## Available Loops

| ID | Name | Purpose |
|----|------|---------|
| `simple_react` | Simple ReAct | **Default entry point.** Direct tool calls, lookups, single-step tasks. |
| `problem_solving` | Problem Solving | Plan → Execute → Audit cycle. For code generation and multi-step reasoning. |
| `verification` | Verification | Generate N independent answers, then vote. For factual accuracy. |
| `decomposition` | Decomposition | Break into subtasks, solve separately, merge results. |
| `search` | Search | Query → Search → Evaluate → Refine. For information retrieval. |

## How Delegation Works

1. The system prompt includes an "Available Strategies" section listing enabled loops.
2. The LLM can emit a `delegate` tool call at any point during execution:
   ```json
   {"tool": "delegate", "raw_args": {"loop": "problem_solving", "reason": "multi-step code generation"}}
   ```
3. `SimpleReActLoop` intercepts the call **before** tool dispatch.
4. It looks up the target loop in the `LoopRegistry`, validates depth limits, and runs it.
5. The delegated loop's result becomes the final answer.

## Loop Contract

Every loop implements the `Loop` trait:

```rust
pub trait Loop: Send + Sync {
    fn id(&self) -> &'static str;          // e.g. "problem_solving"
    fn name(&self) -> &'static str;        // e.g. "Problem Solving"
    fn description(&self) -> &'static str; // when to select (injected into system prompt)
    fn run<'a>(
        &'a self,
        task: String,
        ctx: &'a mut LoopContext<'a>,
    ) -> LoopRunFuture<'a>;
}
```

**`LoopContext`** bundles all runtime dependencies as borrowed references:
config, memory, LLM client, tool dispatcher, workspace, event sink,
summarizer, cancellation token, and the `LoopRegistry` for recursive delegation.

## Adding a New Loop

1. Implement `Loop` trait (see `crates/duga-core/src/loops/simple_react.rs` for reference).
2. Register in `crates/duga-core/src/loops/mod.rs`:
   ```rust
   pub mod my_new_loop;
   pub use my_new_loop::MyNewLoop;

   // In register_default_loops():
   let _ = registry.register(Box::new(MyNewLoop));
   ```
3. Add `"my_new_loop"` to `agent.loop.enabled_loops` in your config YAML.
4. The loop automatically appears in the system prompt and `delegate` tool schema.

Nothing else changes. Bot, dispatcher, system prompt, and `delegate` tool
are all **unaffected**. No code outside `duga-core` needs modification.

## Config

```yaml
agent:
  loop:
    enabled_loops:                 # only listed ids are active
      - problem_solving
      - verification
      - decomposition
      - search
    max_refinement_iterations: 3   # used by loops that need iteration caps
    max_delegation_depth: 2        # prevent infinite delegation chains
```

- `simple_react` is always the entry point (hardcoded, not configurable)
- `max_delegation_depth: 0` disables delegation entirely
- Empty `enabled_loops` means the delegate tool reports "no loops available"

## Design Guidelines

**When to create a new loop:**
- A structured workflow significantly outperforms ad-hoc ReAct for a class of tasks
- The workflow requires its own control flow (iterations, phases, voting)
- You want the LLM to choose this workflow explicitly via `delegate`

**When NOT to:**
- A simple prompt in the system message is sufficient
- The behavior is already handled by `simple_react` + tools
- The "strategy" is just a different system prompt (use the `delegate` tool to pass specialized context instead)

## Migration Guide

### `AgentLoop` → `SimpleReActLoop`

Before:
```rust
let mut agent = AgentLoop::new(config, memory, summarizer, llm, tools, workspace, sink);
let result = agent.run(task, cancellation).await;
```

After:
```rust
let mut runtime = build_agent(&config, llm, tools, workspace, sinks, None)?;
let mut ctx = LoopContext {
    config: &config.agent,
    memory: &mut runtime.memory,
    llm: &runtime.llm,
    tools: &runtime.dispatcher,
    workspace: &runtime.workspace,
    event_sink: &runtime.event_sink,
    summarizer: &runtime.summarizer,
    cancellation: &cancellation,
    registry: &runtime.registry,
    max_refinement_iterations: config.agent.loop_config.max_refinement_iterations,
    max_delegation_depth: config.agent.loop_config.max_delegation_depth,
    delegation_depth: 0,
};
let result = SimpleReActLoop.run(task, &mut ctx).await;
```

### `AgentRunResult` → `LoopResult`

`LoopResult` adds a `loop_id: String` field recording which loop produced the result.
All other fields (`message`, `steps`, `tool_calls`) are identical.

### `agent.restore_history(history)` → `runtime.memory_mut().restore_history(history)`

The `restore_history` method now lives on `Memory` directly, exposed via
`BuiltRuntime::memory_mut()`.
