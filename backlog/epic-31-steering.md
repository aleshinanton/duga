# EPIC-31: Steering — Dynamic Mid-Loop Guidance Injection

**Labels:** `epic/steering`, `epic/loop`, `epic/telegram`  
**Crates:** `duga-core` (primary), `duga-events`, `duga-config`, `duga-telegram-bot`  
**Depends on:** EPIC-26 (Loop-Agnostic Core), EPIC-16 (Telegram Bot), EPIC-23 (Task Anchoring)

## Goal

Add **steering** — dynamically injecting guidance, observations, or constraints into the LLM context **during** a loop run, not just at startup via the system prompt.

Current architecture is purely **reactive**: `LLM → Response → Execute Tools → Push Results → LLM → ...` with no modification possible in between. Steering adds a proactive component, allowing human users, tools, or the loop itself to inject guidance at well-defined checkpoints.

---

## What Changes

### Architecture Before
```
LLM → Response → Execute Tools → Push Results → LLM → ...
           no modification possible in between
```

### Architecture After
```
LLM → Response → Execute Tools → Push Results → [STEER] → LLM → ...
                                          inject guidance here
```

### Injection Points (matching current `run_simple_react` structure)

```
POINT 0: Before each LLM call (check_limits + compress + check_steer)
POINT 1: After LLM response (delegation intercept — existing)
POINT 2: After each tool call result (check_steer, reprompt BUFFERED)
POINT 3: End of step — before next iteration (check_steer, reprompt ALLOWED)
```

### Steering Types

| Type           | Source              | When                     | Example                                              |
|----------------|---------------------|--------------------------|------------------------------------------------------|
| **Human steer**| Telegram user       | Mid-loop, async          | "try a different approach", "use axum instead"       |
| **Tool steer** | Tool execution      | After each tool call     | "search returned 500 results — narrow your query"    |
| **Self-steer** | Loop logic          | Between iterations       | "repeated same error 3 times — pivot"                |
| **Policy steer**| Config/rules       | Before each iteration    | "you have 2 steps remaining — prioritize"            |
| **External steer**| Webhook           | Async                    | External system injects guidance                     |

---

## Key Design Decisions (from design doc)

1. **Channels (mpsc) instead of shared state** — async-safe, non-blocking `try_recv()`, FIFO ordering, no mutex contention
2. **Two-pass drain at each checkpoint** — all context events applied first; exactly one highest-priority control event returned. Cancel > ForceComplete > Reprompt
3. **Reprompt buffered at POINT 2** — avoids inconsistent message history (assistant with N tool_calls but only k < N results)
4. **Reprompt is a control event, not a context event** — changes loop control flow, not just memory
5. **SteeringReceiver is OWNED by the loop** — matches receiver's lifetime (one loop run); avoids lifetime entanglement with `<'a>`
6. **Self-steering bypasses the channel** — calls `apply_context_event()` directly to avoid conflating human-originated and machine-originated steering

---

## Development Rules

### 1. Step-by-step implementation with validation at every gate

Each task must be implemented and **validated** before moving to the next. Never batch untested changes across tasks.

**Per-task gates:**
1. **Write the code** — minimal, focused diff for the task scope
2. **Compile check** — `cargo check -p <crate>` must pass with zero warnings
3. **Write tests** — see Testing Requirements below
4. **Run the test suite** — `cargo test -p <crate>` must pass (all existing + new tests)
5. **Run workspace tests** — `cargo test --workspace` must pass before merging the task
6. **Commit** — one commit per completed task with the task ID in the message

### 2. Never break existing functionality

This epic adds steering to a **live, working loop system**. Every existing test, every existing code path, and every existing user interaction must continue to work identically when no steering is configured.

**Hard rules:**
- `steer: None` (the default) must be **zero-overhead** — `check_steer()` returns `SteerAction::Continue` immediately with no side effects
- All existing `cargo test` assertions must pass verbatim after each task
- No changes to the `Loop` trait signature
- No changes to `Memory`, `Event`, or `LoopResult` public APIs (only additive fields/enum variants)
- `LoopContext::child()` must continue to work identically for child loops that get `steer: None`

### 3. Regression guardrails

Before starting each task, run the full test suite to establish a baseline:
```bash
cargo test --workspace 2>&1 | tail -5  # record passing count
```

After completing the task, the passing count must be **equal or greater** than the baseline. A drop means something broke.

**Critical regression paths to re-verify after every task:**
- `cargo test -p duga-core -- simple_react` — the main loop still works
- `cargo test -p duga-core -- memory` — memory compression/eviction still correct
- `cargo test -p duga-core -- loop_context` — delegation depth tracking correct
- `cargo test -p duga-events` — event serialization/deserialization intact
- `cargo test -p duga-telegram-bot` — bot message handling intact

### 4. Task dependency order is mandatory

Follow the dependency graph strictly:
```
TASK-31.1 → TASK-31.2 → TASK-31.3 → TASK-31.4 → TASK-31.5
                                                    ↓
                                              TASK-31.6 → TASK-31.7 → TASK-31.8
                                                    ↓
                         TASK-31.9 ─────────────────┘
                         TASK-31.10 ────────────────┘
                         TASK-31.11 ────────────────┘
                         TASK-31.12 ────────────────┘
```

Tasks 31.9–31.12 can be done in any order after 31.4, but each requires 31.4's `check_steer()` integration to exist first.

---

## Testing Requirements

### Every task must ship with tests — no exceptions

| Task | Minimum Tests Required | Test File |
|------|----------------------|-----------|
| TASK-31.1 | `SteeringApplied` JSON roundtrip (2 cases: cancel + guidance) | `duga-events/src/lib.rs` tests |
| TASK-31.2 | Channel send/drain roundtrip, priority (Cancel > ForceComplete > Reprompt), buffering, `is_active()` lifecycle, empty drain, multiple context events, `apply_context_event` event emission | `duga-core/src/steering.rs` `#[cfg(test)]` |
| TASK-31.3 | `child()` inherits `steer_limits`, `child()` sets `steer: None`, existing `LoopContext` tests pass | `duga-core/src/loop_context.rs` tests |
| TASK-31.4 | Cancel at POINT 0 terminates, Complete at POINT 3 returns forced answer, Reprompt at POINT 0 re-calls LLM, Cancel during tool execution (POINT 2), no-steer zero-overhead, existing simple_react tests pass | `duga-core/src/loops/simple_react.rs` tests |
| TASK-31.5 | All unit tests from §Implementation Steps, plus boundary cases: channel full drain with mixed events, sender dropped mid-loop, duplicate Reprompt buffering | `duga-core/src/steering.rs` tests |
| TASK-31.6 | Channel lifecycle (create → store → run → clear → verify `is_active()`), sender cleared on error exit, sender cleared on normal exit | `duga-telegram-bot/src/session.rs` tests |
| TASK-31.7 | Active sender → message steered, inactive sender → new task started, race (sender dropped between `is_active()` and `send()`) → graceful fallback | `duga-telegram-bot/src/bot.rs` tests |
| TASK-31.8 | Manual end-to-end only (Telegram UI interactions are not easily unit-testable) | — |
| TASK-31.9 | 3 consecutive errors → self-steer fired, 2 errors + 1 success → counter reset, `SteeringApplied` emitted with `source: "self-diagnosis"` | `duga-core/src/loops/simple_react.rs` tests |
| TASK-31.10 | Config deserialization with/without `steering` section, `repeat: false` injects once, `repeat: true` injects every step, backward compat (no `steering` key) | `duga-types/src/config.rs` tests |
| TASK-31.11 | Tool returns `steering_hint` → pushed to memory, tool returns no hint → no extra event, `SteeringApplied` emitted with `source: "tool"` | `duga-core/src/loops/simple_react.rs` tests |
| TASK-31.12 | All 4 specialized loops: `check_steer()` with `steer: None` returns `Continue` (zero-overhead), existing loop tests pass unchanged | Existing loop test files |

### Coverage targets

- **New code**: 100% of new code paths must have a test that exercises them
- **Modified code**: all branches added to existing functions must be covered
- **Error paths**: every `Err(...)` return and every `unreachable!()` must have a test that hits it
- **Channel states**: sender alive, sender dropped, receiver empty, receiver with pending events, receiver with buffered reprompt — all must be tested

### Test naming convention

Use descriptive test names that document the scenario:
```rust
#[tokio::test]
async fn cancel_wins_over_force_complete_when_both_injected() { }

#[tokio::test]
async fn reprompt_buffered_at_point_2_flushed_at_point_3() { }

#[tokio::test]
async fn steer_none_has_zero_overhead_identical_to_baseline() { }
```

### Test infrastructure reused from existing codebase

- `CapturingEventSink` from `duga-core/src/testing.rs` — capture events for assertion
- `MockTool` / `MockLlm` from existing test modules — script LLM responses and tool behavior
- `RecordingSink` from `duga-events/src/lib.rs` tests — verify event emission order
- `DummyClient` from `duga-llm` — token counting without real API calls

### Running tests during development

```bash
# After each code change:
cargo check -p duga-core -p duga-events -p duga-telegram-bot

# Run affected crate tests:
cargo test -p duga-core
cargo test -p duga-events

# Full workspace before committing:
cargo test --workspace
```

---

## Files Summary

| File | Change |
|------|--------|
| `crates/duga-core/src/steering.rs` (NEW) | `SteeringContextEvent`, `SteeringControlEvent`, `SteeringEvent`, `SteeringSender`, `SteeringReceiver`, `SteerLimits`, `SteerAction`, `check_steer()`, `apply_context_event()` |
| `crates/duga-core/src/loop_context.rs` | Add `steer`, `steer_limits` fields; adapt `child()` |
| `crates/duga-core/src/loops/simple_react.rs` | Call `check_steer()` at POINTS 0, 2, 3 |
| `crates/duga-core/src/loops/*.rs` | Add `check_steer()` calls to all other loops (Phase 4) |
| `crates/duga-events/src/lib.rs` | Add `Event::SteeringApplied` variant |
| `crates/duga-telegram-bot/src/session.rs` | Add `steer_tx: SteeringSender` to session state |
| `crates/duga-telegram-bot/src/runtime.rs` | Create channel pair, give sender to session, receiver to `LoopContext` |
| `crates/duga-telegram-bot/src/bot.rs` | Modify `handle_message` to steer active loops; add cancel button |
| `crates/duga-telegram-bot/src/render.rs` | Steering visual indicators (optional) |
| `crates/duga-config/src/config.rs` | Policy steer config fields (Phase 3) |

---

## Task Breakdown

---

### TASK-31.1: Add SteeringApplied event variant to duga-events

- **Labels:** `layer/events`, `priority/critical`
- **Description:** Add a `SteeringApplied` variant to the `Event` enum in `duga-events`. This event is emitted whenever any steering event is applied (guidance, task reset, limit adjustment, reprompt, cancel, force complete). It carries a `source` tag for observability (e.g. `"human"`, `"self-diagnosis"`, `"tool"`, `"policy"`) and a `kind` tag (e.g. `"guidance"`, `"cancel"`, `"reprompt"`).
- **Files affected:**
  - `crates/duga-events/src/lib.rs`
- **Types involved:** `Event`
- **Functions to implement:**
  - `Event::SteeringApplied { source: String, kind: String }` — new variant
- **Dependencies:** None
- **Implementation steps:**
  1. Add variant to `Event` enum:
     ```rust
     SteeringApplied {
         source: String,
         kind: String,
     },
     ```
  2. Ensure variant roundtrips through JSON serialization (add test)
  3. Ensure `Redactor` doesn't redact this event's fields (`source` and `kind` are not sensitive)
  4. Add test: serialize/deserialize roundtrip preserves both fields
  5. `cargo test -p duga-events` passes
- **Edge cases:**
  - `source` and `kind` are arbitrary strings — no enum validation needed
  - Event is small (~2 fields) — minimal JSONL overhead
- **Definition of Done:** `Event::SteeringApplied` variant exists, roundtrips, and passes tests
- **Acceptance criteria:**
  - `serde_json::to_string(&Event::SteeringApplied { source: "human".into(), kind: "cancel".into() })` produces valid JSON with `"type": "steering_applied"`
  - Deserialized event preserves both `source` and `kind` fields exactly
- **Test plan:**
  - unit: JSON roundtrip with `source = "human"`, `kind = "cancel"`
  - unit: JSON roundtrip with `source = "self-diagnosis"`, `kind = "guidance"`
  - unit: Verify variant tag is `"steering_applied"` in JSON output
- **Estimated effort:** 1 hour
- **Depends on:** None

---

### TASK-31.2: Add steering types module to duga-core

- **Labels:** `layer/core`, `priority/critical`
- **Description:** Create `crates/duga-core/src/steering.rs` with all steering types. This includes the three event enums (`SteeringContextEvent`, `SteeringControlEvent`, `SteeringEvent`), the channel pair (`SteeringSender`, `SteeringReceiver`), the limits struct (`SteerLimits`), and the action enum (`SteerAction`). The module provides the `check_steer()` and `apply_context_event()` functions that are called at injection points. All types must implement `Clone` and `Debug` where appropriate.
- **Files affected:**
  - `crates/duga-core/src/steering.rs` (NEW)
  - `crates/duga-core/src/lib.rs` (add `pub mod steering;`)
- **Types involved:** `SteeringContextEvent`, `SteeringControlEvent`, `SteeringEvent`, `SteeringSender`, `SteeringReceiver`, `SteerLimits`, `SteerAction`
- **Functions to implement:**
  - `SteeringSender::new(tx)` — constructor
  - `SteeringSender::is_active() -> bool` — checks `!tx.is_closed()`
  - `SteeringSender::inject(&self, event: SteeringEvent) -> Result<(), AgentError>`
  - `SteeringSender::guide(&self, text: &str) -> Result<(), AgentError>` — convenience for `InjectGuidance`
  - `SteeringSender::cancel(&self, reason: &str) -> Result<(), AgentError>` — convenience for `Cancel`
  - `SteeringReceiver::new(rx)` — constructor
  - `SteeringReceiver::drain(&mut self) -> (Vec<SteeringContextEvent>, Option<SteeringControlEvent>)` — two-pass drain with priority
  - `check_steer(ctx: &mut LoopContext, allow_reprompt: bool) -> Result<SteerAction, AgentError>` — full checkpoint logic
  - `apply_context_event(ctx: &mut LoopContext, event: SteeringContextEvent) -> Result<(), AgentError>` — apply a single context event to memory/limits
- **Dependencies:** TASK-31.1 (Event::SteeringApplied), `LoopContext`, `Memory`, `AgentError`
- **Implementation steps:**
  1. Define `SteeringContextEvent` enum with four variants: `InjectGuidance`, `ResetTask`, `AdjustLimits`, `InjectToolResult`
  2. Define `SteeringControlEvent` enum with three variants: `Cancel`, `ForceComplete`, `Reprompt`
  3. Define `SteeringEvent` enum wrapping either `Context` or `Control`
  4. Define `SteeringSender` wrapping `mpsc::UnboundedSender<SteeringEvent>` with convenience methods
  5. Define `SteeringReceiver` wrapping `mpsc::UnboundedReceiver<SteeringEvent>` + `pending_reprompt: Option<SteeringControlEvent>`
  6. Define `SteerLimits` struct with `remaining_steps: Option<u32>`, `remaining_tool_calls: Option<u32>`
  7. Define `SteerAction` enum: `Continue`, `Cancel(String)`, `Complete(String)`, `Reprompt`
  8. Implement `SteeringReceiver::drain()` with two-pass logic and Cancel > ForceComplete > Reprompt priority
  9. Implement `check_steer()` — drains receiver, applies context events, returns control action (buffers Reprompt at POINT 2)
  10. Implement `apply_context_event()` — mutates memory or steer_limits, emits `SteeringApplied` event
  11. `cargo check -p duga-core` passes
- **Edge cases:**
  - `drain()` receives `Reprompt` when `allow_reprompt = false` → buffer to `pending_reprompt`, do NOT apply
  - `drain()` receives both `Cancel` and `ForceComplete` → Cancel wins (Cancel > ForceComplete > Reprompt priority)
  - `drain()` receives `Reprompt` alongside context events → context events are still applied (they're guidance for the re-prompt)
  - Empty drain → `(vec![], None)` → `SteerAction::Continue`
  - `pending_reprompt` is flushed at the next checkpoint where `allow_reprompt = true`; it has lower priority than any newly-arrived control events
  - `InjectGuidance` with `as_system: true` → push as `Message::system()`; `as_system: false` → push as `Message::user()` with `[Steering from {source}]: {text}`
  - `ResetTask` → calls `ctx.memory.set_task_anchor(Some(new_task))` and pushes a user message explaining the reset
  - `AdjustLimits` → writes to `ctx.steer_limits`
  - `InjectToolResult` → calls `ctx.memory.push_tool_result()` — note: `push_tool_result` takes a `ToolResult`, but the event carries `tool_name` and `result` strings. We need a different approach. Use the existing `push_tool_result_raw` if it exists, or add a method that pushes a raw tool result by name + output string. **GAP**: Need a method on `Memory` to push a synthetic tool result. For now, create the result inline using `ToolResultBuilder` and `CallId::new()`.
- **Definition of Done:**
  - All steering types compile in `duga-core`
  - `check_steer()` correctly drains, prioritizes, and buffers
  - `apply_context_event()` correctly mutates memory/limits and emits events
  - `cargo check -p duga-core` passes
- **Acceptance criteria:**
  - Channel send/receive roundtrips correctly
  - Two-pass drain produces correct `(context_events, control_event)` pairs
  - Cancel always wins over ForceComplete in priority test
  - Reprompt buffered when `allow_reprompt = false`, applied when `allow_reprompt = true`
  - `is_active()` returns `false` after receiver is dropped
- **Test plan:**
  - unit: `SteeringSender` send + `SteeringReceiver` drain roundtrip
  - unit: Context event drained into context_events vec
  - unit: Control event drained into control_event option
  - unit: Cancel > ForceComplete priority
  - unit: ForceComplete > Reprompt priority
  - unit: Reprompt buffered on `allow_reprompt = false`
  - unit: Buffered reprompt flushed on next `allow_reprompt = true`
  - unit: `is_active()` before and after receiver drop
  - unit: `InjectGuidance` creates correct memory messages (system vs user)
  - unit: `ResetTask` sets task anchor correctly
  - unit: `AdjustLimits` writes to `steer_limits` correctly
  - unit: Empty receiver → drain returns `(vec![], None)`
  - unit: Multiple context events drained in one pass
  - unit: `apply_context_event` emits `Event::SteeringApplied`
- **Estimated effort:** 8 hours
- **Depends on:** TASK-31.1

---

### TASK-31.3: Add steer and steer_limits fields to LoopContext

- **Labels:** `layer/core`, `priority/critical`
- **Description:** Add two new fields to `LoopContext`: `steer: Option<SteeringReceiver>` (owned) and `steer_limits: Option<SteerLimits>` (owned). The `steer` field is the receiving end of the steering channel, owned by the loop run. The `steer_limits` field caches limit adjustments from `AdjustLimits` steering events. Adapt the `child()` method to pass `steer: None` (child loops don't get steering) but copy `steer_limits` (limits should propagate).
- **Files affected:**
  - `crates/duga-core/src/loop_context.rs`
- **Types involved:** `LoopContext<'a>`, `SteeringReceiver`, `SteerLimits`
- **Functions to implement:**
  - New fields in `LoopContext` struct
  - Adapt `child()` method
- **Dependencies:** TASK-31.2 (steering types)
- **Implementation steps:**
  1. Add fields to `LoopContext<'a>`:
     ```rust
     pub steer: Option<SteeringReceiver>,
     pub steer_limits: Option<SteerLimits>,
     ```
  2. In `child()`: set `steer: None` (child loops don't receive steering), copy `steer_limits`:
     ```rust
     steer_limits: self.steer_limits.clone(), // SteerLimits must implement Clone
     ```
  3. Update all existing `LoopContext` construction sites to set `steer: None, steer_limits: None`
     - `make_test_ctx()` in loop_context.rs tests
     - `make_ctx()` in simple_react.rs tests
     - `run_task_for_chat()` in telegram runtime
     - Any harness/CLI code building `LoopContext`
  4. Ensure `SteerLimits` implements `Clone` and `Debug`
  5. `cargo check -p duga-core -p duga-telegram-bot` passes
- **Edge cases:**
  - Existing code constructing `LoopContext` directly will fail to compile — must add `steer: None, steer_limits: None` to all sites
  - `child()` propagates `steer_limits` so that limits set via steering on the parent affect the child loop
  - `steer_limits` being `Option` means limit adjustments are additive (don't override default limits)
- **Definition of Done:**
  - All `LoopContext` construction sites updated
  - `cargo check --workspace` passes
  - `cargo test -p duga-core` passes (existing tests adapted)
- **Acceptance criteria:**
  - `LoopContext.steer: None` is the default (no steering channel)
  - `LoopContext.steer_limits: None` is the default (no limit overrides)
  - `child()` sets `steer: None` and copies `steer_limits`
- **Test plan:**
  - Update existing tests that build `LoopContext` to include new fields
  - Test: `child()` inherits `steer_limits` but not `steer`
  - Test: `child()` with `None` steer_limits produces `None`
- **Estimated effort:** 3 hours
- **Depends on:** TASK-31.2

---

### TASK-31.4: Integrate check_steer() into SimpleReActLoop

- **Labels:** `layer/loop`, `priority/critical`
- **Description:** Add `check_steer()` calls at three points in `run_simple_react()`:
  - **POINT 0**: Before each LLM call (after `check_limits` / `compress_if_needed`). `allow_reprompt = true`. On `Reprompt`, `continue 'outer` to re-call LLM with new guidance.
  - **POINT 2**: Inside the tool loop, after each `push_tool_result`. `allow_reprompt = false`. On `Reprompt`, `unreachable!()` (buffered instead). On `Cancel`/`Complete`, exit early.
  - **POINT 3**: At end of step (before next iteration). `allow_reprompt = true`. Flushes any buffered reprompt from POINT 2.

  The `'outer` loop label already exists on the `for step in 1..=max_steps` line. Use it for POINT 0 `Reprompt` continuation.

- **Files affected:**
  - `crates/duga-core/src/loops/simple_react.rs`
- **Types involved:** `SteerAction`, `check_steer()`
- **Functions to implement:**
  - Integration code at three injection points (match statements)
- **Dependencies:** TASK-31.2 (check_steer), TASK-31.3 (LoopContext fields)
- **Implementation steps:**
  1. **POINT 0** — After `compress_if_needed(ctx).await?`, before LLM call:
     ```rust
     match check_steer(ctx, true).await? {
         SteerAction::Cancel(reason) => {
             tracing::info!(%reason, "Steering cancelled run");
             return fatal(ctx, AgentError::Cancelled).await;
         }
         SteerAction::Complete(answer) => {
             tracing::info!("Steering force-completed run");
             return Ok(complete_from_steer(ctx, answer).await);
         }
         SteerAction::Reprompt => {
             tracing::info!("Steering reprompt — re-calling LLM");
             continue 'outer;
         }
         SteerAction::Continue => { /* normal flow */ }
     }
     ```
  2. **POINT 2** — After `ctx.memory.push_tool_result(result)`:
     ```rust
     match check_steer(ctx, false).await? {
         SteerAction::Cancel(reason) => return fatal(ctx, AgentError::Cancelled).await,
         SteerAction::Complete(answer) => return Ok(complete_from_steer(ctx, answer).await),
         SteerAction::Reprompt => unreachable!("Reprompt buffered at POINT 2"),
         SteerAction::Continue => { /* next tool */ }
     }
     ```
  3. **POINT 3** — At end of step, after tool loop, before `StepFinished` event:
     ```rust
     match check_steer(ctx, true).await? {
         SteerAction::Cancel(reason) => return fatal(ctx, AgentError::Cancelled).await,
         SteerAction::Complete(answer) => return Ok(complete_from_steer(ctx, answer).await),
         SteerAction::Reprompt => continue 'outer,
         SteerAction::Continue => { /* fall through to next step */ }
     }
     ```
  4. Implement helper: `complete_from_steer(ctx, answer: String) -> LoopResult` — pushes final message, emits `AgentFinished`, returns `LoopResult`
  5. Handle the `check_steer` import
  6. `cargo test -p duga-core -- simple_react` passes
- **Edge cases:**
  - `Cancel` at POINT 2 during tool chain: the assistant message with N tool_calls is already in memory, but only k < N results have been pushed. The LLM is NOT called again (we're cancelling), so the inconsistent state doesn't matter. The loop just exits.
  - `Complete` at POINT 2 during tool chain: same as Cancel — we skip remaining tool results and return the forced answer. The inconsistent message history is irrelevant since we're not calling LLM again.
  - `Reprompt` at POINT 0 or 3: the current LLM response (if any) is already in memory. The reprompt injects new guidance, and `continue 'outer` causes the loop to re-call LLM with the updated context. The LLM sees its previous response plus the new steering guidance and adjusts.
  - `Reprompt` at POINT 2: buffered. After all tool results are pushed, POINT 3 flushes it. The LLM then sees the complete tool chain plus steering guidance.
  - Steering receiver is `None`: `check_steer` returns `SteerAction::Continue` immediately — zero overhead.
- **Definition of Done:**
  - All three injection points added and compile
  - Existing simple_react tests still pass (steering is None by default)
  - New steering-specific tests pass
  - `cargo test -p duga-core -- simple_react` passes
- **Acceptance criteria:**
  - `Cancel` steering event at POINT 0 terminates loop with `AgentError::Cancelled`
  - `Complete` steering event at POINT 3 returns forced answer
  - `Reprompt` at POINT 0 causes `continue 'outer` (LLM re-called)
  - No steering (steer = None) has zero overhead — `check_steer` returns `Continue` immediately
  - Existing tests pass unchanged
- **Test plan:**
  - unit: Mock steering channel, inject Cancel at POINT 0 → run returns `AgentError::Cancelled`
  - unit: Mock steering channel, inject Complete at POINT 3 → run returns forced answer
  - unit: Mock steering channel, inject Reprompt at POINT 0 → LLM called twice (original + reprocessed)
  - unit: No steering channel → run behaves identically to before
  - unit: Cancel during tool execution at POINT 2 → loop exits with Cancelled
- **Estimated effort:** 6 hours
- **Depends on:** TASK-31.3

---

### TASK-31.5: Unit tests for steering infrastructure

- **Labels:** `layer/testing`, `priority/high`
- **Description:** Comprehensive unit tests for the steering system. This task covers tests that are too complex to fit in individual task descriptions, including: channel lifecycle tests, drain priority tests, loop integration tests with mock channels, event emission verification, and edge case scenarios.
- **Files affected:**
  - `crates/duga-core/src/steering.rs` (add `#[cfg(test)] mod tests`)
  - `crates/duga-core/src/loops/simple_react.rs` (add steering-specific tests in existing test module)
- **Types involved:** All steering types, mock channels, `CapturingEventSink`
- **Dependencies:** TASK-31.4
- **Implementation steps:**
  1. **Steering module unit tests:**
     - `send_and_drain_roundtrip()` — inject → drain → verify
     - `context_event_drained_to_context_events()` — InjectGuidance appears in context_events
     - `control_event_drained_to_control_event()` — Cancel appears in control_event
     - `cancel_wins_over_force_complete()` — both injected, Cancel returned
     - `force_complete_wins_over_reprompt()` — both injected, ForceComplete returned
     - `reprompt_buffered_when_allow_false()` — drain with allow=false buffers Reprompt
     - `buffered_reprompt_flushed_next_allow_true()` — buffered Reprompt appears on next drain with allow=true
     - `is_active_before_and_after_drop()` — sender is_active() before drop, not after
     - `empty_receiver_drain_returns_continue()` — no events → Continue
     - `multiple_context_events_drained_in_one_pass()` — 3 context events → all in vec
  2. **SimpleReActLoop steering integration tests:**
     - `cancel_at_point_0_terminates_loop()` — inject Cancel via channel, verify AgentError::Cancelled
     - `complete_at_point_3_returns_forced_answer()` — inject Complete, verify LoopResult with forced text
     - `reprompt_at_point_0_re_calls_llm()` — inject Reprompt, verify LLM called 2 times
     - `no_steer_has_zero_overhead()` — steer=None, verify identical behavior to before
     - `cancel_during_tool_execution()` — inject Cancel at POINT 2, verify early exit
  3. **Event emission tests:**
     - `apply_context_event_emits_steering_applied()` — verify Event::SteeringApplied emitted
     - `steer_action_cancel_emits_steering_applied()` — verify cancel event emitted
     - `steer_action_reprompt_emits_steering_applied()` — verify reprompt event emitted
  4. `cargo test -p duga-core` all pass
- **Edge cases:**
  - Tests must use `tokio::test` for async operations
  - Mock LLM needs to return different responses on successive calls (for reprompt tests)
  - Channel lifecycle: sender dropped before receiver → drain returns empty
- **Definition of Done:** All tests listed above pass
- **Acceptance criteria:** Test coverage of all steering paths: context events, control events, priority, buffering, loop integration
- **Estimated effort:** 6 hours
- **Depends on:** TASK-31.4

---

### TASK-31.6: Wire steering channel into Telegram runtime and session

- **Labels:** `layer/telegram`, `priority/critical`
- **Description:** Create the steering channel pair (`SteeringSender`, `SteeringReceiver`) when a loop run starts. Store the sender in the `ChatSessionState` so the bot can inject steering events mid-run. Pass the receiver to `LoopContext.steer` so the loop can check for steering at injection points. The channel must be created in `run_task_for_chat()` before the loop runs, and the sender must be cleaned up when the run completes.
- **Files affected:**
  - `crates/duga-telegram-bot/src/session.rs` — add `steer_tx: Option<SteeringSender>` to `ChatSessionState`
  - `crates/duga-telegram-bot/src/runtime.rs` — create channel, store sender in session, pass receiver to `LoopContext`
  - `crates/duga-core/src/lib.rs` — re-export `SteeringSender`, `SteeringReceiver`, `SteeringEvent`, etc.
- **Types involved:** `ChatSessionState`, `LoopContext`, `SteeringSender`, `SteeringReceiver`
- **Functions to implement:**
  - `ChatSessionState.steer_tx` field + setter/clearer
  - Channel creation in `run_task_for_chat()`
- **Dependencies:** TASK-31.3 (LoopContext fields), TASK-31.2 (steering types)
- **Implementation steps:**
  1. Add field to `ChatSessionState`:
     ```rust
     pub steer_tx: Option<SteeringSender>,
     ```
     Default to `None`.
  2. In `SessionManager::start_run()`:
     - After setting `session.active = true`, but before spawning the tokio task:
     - Create the channel: `let (tx, rx) = tokio::sync::mpsc::unbounded_channel();`
     - Create sender: `let sender = SteeringSender::new(tx);`
     - Create receiver: `let receiver = SteeringReceiver::new(rx);`
     - Store sender in session: `session.steer_tx = Some(sender);`
     - Pass receiver to the spawned task
  3. In `run_task_for_chat()`, accept a `steering_rx: SteeringReceiver` parameter:
     ```rust
     pub async fn run_task_for_chat(
         &self,
         chat_id: i64,
         task: String,
         bot: Bot,
         telegram_config: &TelegramConfig,
         _bot_logger: Arc<BotLogger>,
         session_manager: Arc<SessionManager>,
         cancellation: CancellationToken,
         steering_rx: Option<SteeringReceiver>,  // NEW parameter
     ) -> Result<String>
     ```
     Set `ctx.steer = steering_rx;` when building the `LoopContext`.
  4. When the run completes (in the tokio::spawn cleanup block in `start_run()`):
     - Set `session.steer_tx = None` to drop the sender
  5. `cargo check -p duga-telegram-bot` passes
- **Edge cases:**
  - `SessionManager::start_run()` must pass the receiver to `run_task_for_chat()` — the receiver is consumed by the loop
  - The sender is cloned for the bot handler to use (it needs a clone to inject events)
  - When the loop finishes (success or error), the receiver is dropped, and `sender.is_active()` returns `false`
  - The sender must be cleared from the session BEFORE the receiver is dropped to avoid a brief window where `is_active()` returns true but the loop is already exiting
  - Racing: between `is_active()` check and `send()`, the loop could finish. Handle the `send()` error gracefully.
- **Definition of Done:**
  - Channel created in `start_run()` and wired into `LoopContext`
  - Sender stored in session and accessible from bot handler
  - Sender cleared on run completion
  - Existing Telegram tests pass
- **Acceptance criteria:**
  - `session.steer_tx.is_some()` during an active run
  - `session.steer_tx.is_none()` after run completes
  - `steer_tx.is_active()` returns `true` during run, `false` after
- **Test plan:**
  - unit: Channel lifecycle test — create, store, clear, verify is_active
  - unit: Verify sender cleared after run completes (mock run)
- **Estimated effort:** 4 hours
- **Depends on:** TASK-31.3

---

### TASK-31.7: Modify Telegram handle_message to steer active loops

- **Labels:** `layer/telegram`, `priority/critical`
- **Description:** When a user sends a message while a loop is running, instead of starting a new run (or rejecting with "task already running"), inject the message as a `Reprompt` steering event into the running loop. This is the core human-steering interaction: the user can say "use axum instead" mid-loop and the LLM adjusts course without losing context.
- **Files affected:**
  - `crates/duga-telegram-bot/src/bot.rs` — modify `handle_message()`
  - `crates/duga-core/src/lib.rs` — ensure `SteeringSender`, `SteeringEvent`, etc. are accessible
- **Types involved:** `SessionManager`, `SteeringSender`, `SteeringEvent`, `SteeringControlEvent`
- **Functions to implement:**
  - Modified `handle_message()` — check for active loop, steer if active, new task if not
- **Dependencies:** TASK-31.6 (steering channel in session)
- **Implementation steps:**
  1. In `handle_message()`, after authorization and command check, before task processing:
     - Check if there's an active session: `if let Some(session) = session_manager.sessions.get(&chat_id_i64)`
     - Check if the session has a steering sender: `if let Some(ref steer_tx) = session.steer_tx`
     - Check if the sender is active: `if steer_tx.is_active()`
     - If all true: this message is a steer, not a new task
  2. The steer action: inject a single `Reprompt` control event:
     ```rust
     let result = steer_tx.inject(
         SteeringEvent::Control(
             SteeringControlEvent::Reprompt {
                 guidance: text.clone(),
             }
         )
     );
     ```
     **Important**: Only inject `Reprompt` — do NOT also inject `InjectGuidance`. The `Reprompt` control event already carries the guidance text. Injecting both would double-inject the message into memory.
  3. On success: acknowledge with a brief message like "🔄 Adjusting..."
  4. On error (channel closed — race between `is_active()` and `send()`): treat as a new task:
     ```rust
     // Channel closed — loop just finished. Start new task.
     start_new_task_as_normal(...)
     ```
  5. If no active session or no steering sender: proceed with normal new task flow
  6. `cargo check -p duga-telegram-bot` passes
- **Edge cases:**
  - **Race condition**: `is_active()` returns `true` but the loop finishes before `send()` completes. The `send()` returns an error. Handle gracefully: start a new task.
  - **Cancellation commands**: `/stop` or "stop"/"cancel" text should still call `session_manager.cancel()` which uses the `CancellationToken`, not the steering channel. Keep existing command handling before the steer check.
  - **First message of session**: No active loop → normal task creation.
  - **Empty message**: Skip steering, do nothing.
  - **Attachment-only message**: Steer with the attachment summary as guidance text.
  - **Multiple rapid steers**: Each is injected sequentially. The loop processes all pending events at the next checkpoint. The `drain()` method collects them all.
- **Definition of Done:**
  - User message during active run becomes a steer (Reprompt)
  - User message when no run is active starts a new run (existing behavior)
  - `/stop` during active run still cancels (priority over steer)
  - Race condition handled gracefully
- **Acceptance criteria:**
  - Message "use axum instead" during active run → injected as Reprompt
  - Message during idle → starts new run
  - `/stop` during active run → cancels (not steered)
  - Rapid steering messages → all processed at next checkpoint
- **Test plan:**
  - unit: Mock session with active sender, send message → verify steer injected
  - unit: Mock session with inactive sender → verify new task started
  - unit: Race condition: sender dropped between is_active() and send() → verify graceful fallback
  - manual: End-to-end with Telegram bot — start a task, send steering message mid-run
- **Estimated effort:** 5 hours
- **Depends on:** TASK-31.6

---

### TASK-31.8: Cancel button and /steer command in Telegram

- **Labels:** `layer/telegram`, `priority/high`
- **Description:** Add explicit steering UI to the Telegram bot. When a loop is running, show action buttons under the status message: `[Stop]`. Tapping "Stop" sends `SteeringControlEvent::Cancel`. Also add a `/steer <text>` command as an explicit way to inject guidance (alternative to plain text messages).
- **Files affected:**
  - `crates/duga-telegram-bot/src/bot.rs` — add `/steer` command handler, cancel button wiring
  - `crates/duga-telegram-bot/src/render.rs` — add inline keyboard markup to status messages
- **Types involved:** `InlineKeyboardMarkup`, `SteeringControlEvent::Cancel`, `SteeringEvent`
- **Dependencies:** TASK-31.7
- **Implementation steps:**
  1. **Cancel button**: When the renderer sends the initial "Running..." status message, include an inline keyboard:
     ```
     [ [🛑 Stop] ]
     ```
     The callback data for "Stop" is `"cancel_run"`.
  2. **Cancel button handler**: In `handle_callback_query()`, when the callback data is `"cancel_run"`:
     - Get the steering sender from the session
     - Inject `SteeringControlEvent::Cancel { reason: "User cancelled via button".into() }`
     - Also call `session_manager.cancel(chat_id)` to set the cancellation token (belt and suspenders)
     - Answer the callback with "⏹️ Stopping..."
  3. **`/steer` command**: Handle in `handle_command()`:
     ```
     /steer use axum instead of actix-web
     ```
     - Get the steering sender from session
     - Inject `Reprompt { guidance: rest_of_message }`
     - Reply: "🔄 Steering injected."
  4. Update the steering path in `handle_message()`: if the message is a `/steer` command, it's handled by the command handler. Plain text messages during active loop are still steered as described in TASK-31.7.
  5. `cargo check -p duga-telegram-bot` passes
- **Edge cases:**
  - Cancel button appears even after loop finishes: handler checks `is_active()` before injecting
  - `/steer` with no guidance text: reply with usage help
  - `/steer` when no loop is active: reply "No active run to steer. Start a task first."
- **Definition of Done:**
  - Cancel button appears during active runs
  - Tapping Cancel stops the loop
  - `/steer` command injects guidance into running loop
- **Acceptance criteria:**
  - Running loop shows `[🛑 Stop]` button
  - Tapping Stop → loop terminates with Cancel
  - `/steer try a different approach` → LLM receives "try a different approach" as Reprompt
  - `/steer` with no args → usage message
  - `/steer` when idle → error message
- **Test plan:**
  - manual: Telegram bot end-to-end — start task, tap Stop, verify cancellation
  - manual: Telegram bot — start task, send `/steer use X instead`, verify LLM adjusts
- **Estimated effort:** 4 hours
- **Depends on:** TASK-31.7

---

### TASK-31.9: Self-steering — loop detects repeated errors

- **Labels:** `layer/loop`, `layer/core`, `priority/medium`
- **Description:** Implement self-steering: when the loop detects a pattern of repeated errors (e.g., 3 consecutive tool call failures), it injects guidance into its own context via `apply_context_event()` directly (not through the external channel). This keeps human-originated and machine-originated steering separate for auditability. The steering event carries `source: "self-diagnosis"`.
- **Files affected:**
  - `crates/duga-core/src/loops/simple_react.rs` — add error tracking + self-steer logic
- **Types involved:** `SteeringContextEvent::InjectGuidance`, `apply_context_event()`
- **Dependencies:** TASK-31.4 (check_steer integrated), TASK-31.2 (apply_context_event)
- **Implementation steps:**
  1. Add error tracking in `run_simple_react()`:
     ```rust
     let mut consecutive_errors: u32 = 0;
     ```
  2. In the tool loop, when a tool returns an error:
     - Increment `consecutive_errors`
     - If `consecutive_errors >= 3`:
       ```rust
       apply_context_event(ctx, SteeringContextEvent::InjectGuidance {
           text: "You have failed 3 times in a row. Pivot to a different \
                  approach. Consider: (1) using a different tool, \
                  (2) breaking the problem down further, \
                  (3) explaining what's blocking you.".into(),
           as_system: true,
           source: "self-diagnosis".into(),
       }).await?;
       consecutive_errors = 0; // reset after injecting
       ```
  3. When a tool succeeds, reset `consecutive_errors = 0`
  4. This calls the same `apply_context_event()` used by `check_steer()`, so observability (event emission) and memory mutation behave identically
  5. `cargo test -p duga-core -- simple_react` passes
- **Edge cases:**
  - Self-steer event is emitted with `source: "self-diagnosis"` — distinct from `"human"` in logs
  - Self-steer injects as system message (`as_system: true`) for higher priority in LLM attention
  - Error counter resets on successful tool call — only consecutive errors trigger self-steer
  - Self-steer is complementary to, not replacing, the retry mechanism (TASK-10.4)
  - The self-steer guidance is injected BEFORE the next LLM call (it goes to memory at POINT 2)
- **Definition of Done:**
  - 3 consecutive errors trigger self-steer guidance injection
  - Self-steer events logged with `source: "self-diagnosis"`
  - Successful tool call resets error counter
- **Acceptance criteria:**
  - 3 failed tool calls in a row → `InjectGuidance` event in memory with "Pivot to a different approach"
  - `Event::SteeringApplied { source: "self-diagnosis", kind: "guidance" }` emitted
  - 2 failures + 1 success + 1 failure → counter is 1 (not 3)
- **Test plan:**
  - unit: Mock tools that fail 3 times → verify self-steer event in memory
  - unit: Mock tools that fail 2 times, succeed, fail 1 → verify no self-steer
  - unit: Verify `SteeringApplied` event emitted with correct source
- **Estimated effort:** 4 hours
- **Depends on:** TASK-31.4

---

### TASK-31.10: Policy steer — config-driven rules

- **Labels:** `layer/config`, `layer/steering`, `priority/medium`
- **Description:** Add a `steering` section to the agent config that allows defining policy rules. Each rule injects guidance before each iteration (POINT 0). Example rules: "prefer axum over actix", "you have {remaining} steps remaining", "always write tests". Rules are loaded from config at startup and applied as `InjectGuidance` context events at POINT 0.
- **Files affected:**
  - `crates/duga-types/src/config.rs` — add `SteeringConfig` and `SteeringRule`
  - `crates/duga-config/src/config.rs` — validation
  - `crates/duga-core/src/loops/simple_react.rs` — apply policy steers at POINT 0
- **Types involved:** `AgentConfig`, `SteeringConfig`, `SteeringRule`
- **Dependencies:** TASK-31.4
- **Implementation steps:**
  1. Define config types:
     ```rust
     #[derive(Debug, Clone, Deserialize)]
     pub struct SteeringRule {
         pub guidance: String,
         #[serde(default)]
         pub as_system: bool,
         #[serde(default)]
         pub repeat: bool, // if true, inject every iteration
     }
     
     #[derive(Debug, Clone, Default, Deserialize)]
     pub struct SteeringConfig {
         #[serde(default)]
         pub rules: Vec<SteeringRule>,
     }
     ```
  2. Add `steering: SteeringConfig` field to `AgentConfig` (with `#[serde(default)]`)
  3. In `run_simple_react()`, at POINT 0 (before LLM call), apply policy steers:
     ```rust
     for rule in &ctx.config.steering.rules {
         if rule.repeat || step == 1 {
             // Only apply once unless repeat is true
             // ...apply rule as InjectGuidance with source: "policy"...
         }
     }
     ```
  4. Note: since `LoopContext` fields are `&'a` references, the policy rules are accessed via `ctx.config`. No need to cache them.
  5. `cargo check -p duga-types -p duga-config -p duga-core` passes
- **Edge cases:**
  - `rules` is empty or `steering` section is missing → no policy steers (default)
  - Rule `guidance` can contain template variables? Not in MVP — plain string
  - `repeat: false` (default) → inject once on first iteration only
  - `repeat: true` → inject every iteration (use sparingly — consumes tokens)
  - Policy steers are applied internally (source: "policy"), not through the channel
  - Policy steers should be applied as system messages (`as_system: true`) for higher priority
- **Definition of Done:**
  - Config YAML can specify steering rules
  - Rules are injected as guidance at POINT 0
  - `repeat: false` rules inject once
  - Existing configs without `steering:` section work (backward compat)
- **Acceptance criteria:**
  - YAML: `agent.steering.rules: [{ guidance: "prefer axum", as_system: true }]` → injected at step 1
  - YAML without `steering` key → no errors, no policy steers
  - Rule with `repeat: true` → injected every step
- **Test plan:**
  - unit: Deserialize config with steering rules
  - unit: Verify rules injected at correct iteration
  - unit: Verify `repeat: false` only injects once
- **Estimated effort:** 4 hours
- **Depends on:** TASK-31.4

---

### TASK-31.11: Tool steer — tools return steering alongside results

- **Labels:** `layer/tools`, `layer/steering`, `priority/low`
- **Description:** Allow tools to return structured steering context events alongside their normal output. For example, the `search` tool could return "search returned 500 results — narrow your query" as a steering hint. This is an opt-in capability: tools that don't need steering are unaffected. The steering hint is pushed to memory as a user message after the tool result.
- **Files affected:**
  - `crates/duga-types/src/tool_result.rs` — add optional `steering_hint` field to `ToolResult`
  - `crates/duga-core/src/loops/simple_react.rs` — process steering hints after tool results (POINT 2)
  - `crates/duga-tools-builtin/src/search.rs` — example: search returns steering hint
- **Types involved:** `ToolResult`, `SteeringContextEvent::InjectGuidance`
- **Dependencies:** TASK-31.4, the `search` tool
- **Implementation steps:**
  1. Add field to `ToolResult`:
     ```rust
     pub steering_hint: Option<String>,
     ```
     Default to `None`. Add a builder method `steering_hint()`.
  2. In `run_simple_react()`, after `push_tool_result`, check for steering hint:
     ```rust
     if let Some(hint) = result.steering_hint.take() {
         apply_context_event(ctx, SteeringContextEvent::InjectGuidance {
             text: hint,
             as_system: false, // user-level, tool-originated
             source: "tool".into(),
         }).await?;
     }
     ```
  3. Example: update `search` tool to return a steering hint when results exceed 50 items:
     ```rust
     ToolResultBuilder::new()
         .steering_hint(Some(format!("Search returned {} results. Consider narrowing your query with more specific terms.", total)))
         // ...
     ```
  4. `cargo check --workspace` passes
- **Edge cases:**
  - Tool result serialized to JSONL: `steering_hint` field appears in stored events (good for observability)
  - `steering_hint` is `None` by default → no behavior change for existing tools
  - Tool-result steering is applied at POINT 2, same as other context events
  - The hint is injected AFTER the tool result, so the LLM sees the result first then the steering
  - Source is `"tool"` for auditability
- **Definition of Done:**
  - `ToolResult` has an optional `steering_hint` field
  - Loop processes steering hints from tool results
  - At least one built-in tool demonstrates the capability (search)
- **Acceptance criteria:**
  - Tool returning `steering_hint = "narrow query"` → hint appears in memory after tool result
  - `Event::SteeringApplied { source: "tool", kind: "guidance" }` emitted
  - Tool without steering hint → no extra event emitted
- **Test plan:**
  - unit: Mock tool returns `steering_hint`, verify it's pushed to memory
  - unit: Mock tool returns no `steering_hint`, verify no extra event
- **Estimated effort:** 3 hours
- **Depends on:** TASK-31.4

---

### TASK-31.12: Add steering to other loops (ProblemSolving, Verification, Decomposition, Search)

- **Labels:** `layer/loop`, `priority/medium`
- **Description:** Add `check_steer()` calls to all specialized loops at their natural injection points. Each loop has its own structure, so the injection points differ. The goal is that if a user steers a delegated loop, the steering is received at the top-level `SimpleReActLoop` (which owns the receiver) and the child loop inherits `steer_limits` but not the receiver itself. For now, steering to child loops goes through the parent loop's injection points.
- **Files affected:**
  - `crates/duga-core/src/loops/problem_solving.rs`
  - `crates/duga-core/src/loops/verification.rs`
  - `crates/duga-core/src/loops/decomposition.rs`
  - `crates/duga-core/src/loops/search_loop.rs`
- **Types involved:** `check_steer()`, `SteerAction`
- **Dependencies:** TASK-31.4
- **Implementation steps:**
  1. For **ProblemSolvingLoop** (`plan_phase`, `execute_plan`, `audit_phase`):
     - Add `check_steer(ctx, true)` at the start of each phase (before LLM calls)
     - Add `check_steer(ctx, false)` after tool calls in `execute_plan`
     - Add `check_steer(ctx, true)` at end of each iteration before next refinement
  2. For **VerificationLoop** (`generate_answer`, `vote`):
     - Add `check_steer(ctx, true)` before each generation and before voting
  3. For **DecompositionLoop** (`decompose`, `solve_subtask`, `merge_results`):
     - Add `check_steer(ctx, true)` before each LLM call
     - Add `check_steer(ctx, false)` after tool calls in `solve_subtask`
  4. For **SearchLoop** (`formulate_query`, `execute_search`, `evaluate_results`, `synthesize_findings`):
     - Add `check_steer(ctx, true)` before each LLM call
     - Add `check_steer(ctx, false)` after search/read tool calls
  5. Note: since child loops get `ctx.steer = None` and `ctx.steer_limits = Some(cloned)`, `check_steer()` will return `Continue` immediately but limit adjustments from the parent are respected.
  6. `cargo check -p duga-core` passes; existing loop tests pass
- **Edge cases:**
  - Child loop gets `steer = None` from `LoopContext::child()` → `check_steer()` no-ops. **Future enhancement**: allow child loops to have their own steering receivers for more granular control.
  - Limit adjustments from parent loop's steering propagate via `steer_limits` clone
  - Each specialized loop has different structure — injection points must be placed where they make sense for that loop's flow
- **Definition of Done:**
  - All four specialized loops have `check_steer()` calls at appropriate points
  - Existing tests for these loops pass unchanged (steer is None → no behavior change)
- **Acceptance criteria:**
  - `check_steer` compiles and is called in all four specialized loops
  - No behavior change when no steering is configured
  - `steer_limits` propagated to child loops
- **Test plan:**
  - Existing loop tests pass
- **Estimated effort:** 5 hours
- **Depends on:** TASK-31.4

---

## Dependency Graph

```
TASK-31.1 (Event::SteeringApplied)
    │
    └── TASK-31.2 (steering types module)
            │
            ├── TASK-31.3 (LoopContext fields)
            │       │
            │       ├── TASK-31.4 (SimpleReActLoop integration)
            │       │       │
            │       │       ├── TASK-31.5 (unit tests)
            │       │       ├── TASK-31.9 (self-steering)
            │       │       ├── TASK-31.10 (policy steer)
            │       │       ├── TASK-31.11 (tool steer)
            │       │       └── TASK-31.12 (other loops)
            │       │
            │       └── TASK-31.6 (telegram wiring)
            │               │
            │               └── TASK-31.7 (handle_message steer)
            │                       │
            │                       └── TASK-31.8 (cancel button + /steer)
            │
            └── (TASK-31.9, 31.10, 31.11 also depend on TASK-31.2 for apply_context_event)
```

## Epic Summary

| Phase | Task | Name | Est. Hours |
|-------|------|------|------------|
| 1 | TASK-31.1 | Add SteeringApplied event variant | 1 |
| 1 | TASK-31.2 | Add steering types module to duga-core | 8 |
| 1 | TASK-31.3 | Add steer/steer_limits fields to LoopContext | 3 |
| 1 | TASK-31.4 | Integrate check_steer() into SimpleReActLoop | 6 |
| 1 | TASK-31.5 | Unit tests for steering infrastructure | 6 |
| **Phase 1 Total** | | | **24 hours** |
| 2 | TASK-31.6 | Wire steering channel into Telegram runtime/session | 4 |
| 2 | TASK-31.7 | Modify handle_message to steer active loops | 5 |
| 2 | TASK-31.8 | Cancel button and /steer command | 4 |
| **Phase 2 Total** | | | **13 hours** |
| 3 | TASK-31.9 | Self-steering — repeated error detection | 4 |
| 3 | TASK-31.10 | Policy steer — config-driven rules | 4 |
| 3 | TASK-31.11 | Tool steer — tools return steering hints | 3 |
| **Phase 3 Total** | | | **11 hours** |
| 4 | TASK-31.12 | Add steering to other loops | 5 |
| **Phase 4 Total** | | | **5 hours** |
| **Grand Total** | | | **53 hours** |

---

## Key Implementation Notes

### Memory method needed for InjectToolResult

The `InjectToolResult` steering event needs a way to push a synthetic tool result to memory. The current `Memory::push_tool_result()` takes a `ToolResult` struct which requires a `CallId`. We can construct one:

```rust
SteeringContextEvent::InjectToolResult { tool_name, result } => {
    let tool_result = duga_types::tool_result::ToolResultBuilder::new()
        .tool_call_id(duga_types::tool_call::CallId::new())
        .success(true)
        .output(result)
        .build()
        .map_err(|e| AgentError::Internal(format!("Failed to build injected tool result: {e}")))?;
    ctx.memory.push_tool_result(tool_result);
    // ...
}
```

### LoopContext child() and steer

The `child()` method currently copies all fields via reborrowing. With the owned `steer` field, we need to decide whether to pass `steer` to children. **Decision**: child loops get `steer: None`. Steering is for the top-level loop only. In the future, if child loops need their own steering, they would get their own channel.

### Cancellation vs Steering Cancel

Two mechanisms exist to stop a running loop:
1. **CancellationToken** (existing) — set by `/stop` command → loop checks at every `check_limits()` call
2. **Steering Cancel** (new) — injected via channel → loop checks at `check_steer()` checkpoints

Both should work. When a user presses the Stop button:
- Inject `SteeringControlEvent::Cancel` for immediate effect at next checkpoint
- Also call `session_manager.cancel()` to set the cancellation token for the limit checks

### Event emission during steering

Every steering event emission should include:
- `SteeringApplied { source, kind }` — for observability
- `Error { message }` on cancel — for frontend display (existing `fatal()` does this)
- Normal lifecycle events continue as usual (StepStarted, StepFinished, etc.)
