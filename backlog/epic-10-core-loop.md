# EPIC-10: Core ReAct Loop

**§SPEC:** §5, §26–27  
**Labels:** `epic/loop`  
**Crate:** `duga-core`

## Goal
Implement `AgentLoop::run()` — the single orchestration point: limit checks, cancellable LLM calls, memory bookkeeping, termination detection, sequential tool execution with retry, compression triggering.

---

### TASK-10.1: AgentLoop struct + constructor

- **§SPEC:** §4 (AgentConfig), §5 (loop context)
- **Labels:** `layer/loop`, `priority/critical`
- **Description:** Define `AgentLoop` struct holding all runtime state: `config: AgentConfig`, `memory: Memory`, `llm: Arc<dyn LlmClient>`, `tools: ToolDispatcher`, `events: Arc<dyn EventSink>`, `summarizer: Arc<dyn Summarizer>`, `cancel: CancellationToken`, `steps: usize`, `tool_calls: usize`. Implement `AgentLoop::new(...)` builder that takes all dependencies. Validate config: reject zero max_steps, zero max_runtime.
- **Files affected:**
  - `crates/duga-core/src/loop.rs` (new)
  - `crates/duga-core/src/lib.rs` (add module)
- **Types involved:** `AgentLoop`, `AgentConfig`, `Memory`, `LlmClient`, `ToolDispatcher`, `EventSink`, `Summarizer`, `CancellationToken`
- **Functions to implement:**
  - `AgentLoop::new(config, memory, llm, tools, events, summarizer, cancel) -> Self`
  - `AgentLoop::tool_ctx(&self, child_cancel: CancellationToken) -> ToolContext`
- **Dependencies:** TASK-1.7 (AgentConfig), TASK-6.1 (Memory), TASK-8.1 (LlmClient), TASK-3.3 (ToolDispatcher), TASK-7.2 (EventSink), TASK-6.5 (Summarizer)
- **Implementation steps:**
  1. Define `AgentLoop` struct
  2. `new()`: store all fields, initialize counters
  3. `tool_ctx()`: create `ToolContext` from `workspace`, child cancel token, event sink
  4. Add test with minimal constructor
- **Edge cases:**
  - `Arc` cloning: EventSink cloned for ToolContext; LlmClient cloned for Memory
  - Cancel token: `CancellationToken` is clonable; child tokens can be created
  - Workspace: stored as `Arc<Workspace>` in AgentLoop for sharing with ToolDispatcher
- **Definition of Done:** Struct compiles, constructor works
- **Accepted criteria:** `AgentLoop::new(...)` builds without panic
- **Test plan:** unit: Build with mocks
- **Estimated effort:** 4 hours

---

### TASK-10.2: AgentLoop::run — limit checks at top of loop

- **§SPEC:** §5 (limit checks — max_steps, max_tool_calls, max_runtime, cancellation)
- **Labels:** `layer/loop`, `priority/critical`
- **Description:** Implement the limit check section at the top of the `run()` loop. Before each LLM call, check: (1) `self.steps >= max_steps` → return `AgentError::MaxStepsReached`, (2) `self.tool_calls >= max_tool_calls` → return `AgentError::MaxToolCallsReached`, (3) elapsed > `max_runtime` → return `AgentError::Timeout`, (4) `cancel.is_cancelled()` → return `AgentError::Cancelled`.
- **Files affected:**
  - `crates/duga-core/src/loop.rs` (implement run)
- **Types involved:** `AgentLoop`, `AgentError`, `AgentLimits`
- **Functions to implement:**
  - `AgentLoop::run(&mut self, task: String) -> Result<String, AgentError>`
- **Dependencies:** TASK-10.1
- **Implementation steps:**
  1. `run()`: record `Instant::now()`, push user task to memory
  2. Enter `loop { ... }`
  3. Check 4 limits in order
  4. Return appropriate `AgentError`
  5. Test with limits set to trigger immediately
- **Edge cases:**
  - `max_steps: 0` → returns `MaxStepsReached` immediately (before any LLM call)
  - `max_runtime: 0s` → returns `Timeout` immediately
  - Cancel before entering loop → `Cancelled` immediately
  - Steps increment AFTER tool execution, so step 0 is the first iteration
- **Definition of Done:** All limit checks work
- **Acceptance criteria:**
  - `max_steps: 1` → first iteration runs, second returns MaxStepsReached
  - `max_tool_calls: 0` with tool response → MaxToolCallsReached
  - Cancel token triggered → Cancelled
- **Test plan:** unit: MockLlm returning tool calls; set tight limits; verify error returns
- **Estimated effort:** 4 hours

---

### TASK-10.3: AgentLoop::run — LLM call with cancellation

- **§SPEC:** §5 (LLM call section — tokio::select!), §26 (Cancellation)
- **Labels:** `layer/loop`, `priority/critical`
- **Description:** Implement the LLM call inside `run()` using `tokio::select!` with `biased` polling: branch 1 checks cancellation, branch 2 calls `self.llm.chat(...)`. Pass `self.memory.messages()`, `self.tools.schemas()`, `LlmCallOptions { streaming: config.streaming }`, and `&*self.events`. On cancellation winning → return `AgentError::Cancelled`. On LLM error → return `LlmError` → map to `AgentError`? No — spec says loop continues. LLM errors are appended as tool results? Wait — `LlmError` is returned by the LLM call, not a tool. The spec loops: `r = self.llm.chat(...) => r?`. The `?` propagates LlmError. So LlmError terminates the loop via `?`. But the error taxonomy says LlmError is NOT fatal — contradiction. Resolve: `run()` returns `Result<String, AgentError>`, but `llm.chat()` returns `Result<LlmResponse, LlmError>`. Map `LlmError` to `AgentError::SummarizerFailed`? No. Simpler: wrap `LlmError` in a new `AgentError::LlmError(LlmError)`. 
- **Files affected:**
  - `crates/duga-core/src/loop.rs` (LLM call)
  - `crates/duga-types/src/error.rs` (add AgentError::LlmError variant)
- **Types involved:** `AgentError::LlmError`, `LlmClient`, `CancellationToken`
- **Functions to implement:** (inline in run)
- **Dependencies:** TASK-10.2, TASK-8.1
- **Implementation steps:**
  1. Add `AgentError::LlmError(LlmError)` variant (or map to existing)
  2. In run loop: `tokio::select! { biased; _ = cancel.cancelled() => return Err(Cancelled); r = llm.chat(...) => r.map_err(AgentError::LlmError)? }`
  3. Test with cancellation, test with LLM error
- **Edge cases:**
  - LLM call hangs indefinitely → cancellation must win (biased select ensures this)
  - LLM returns empty tool_calls and no text → termination condition (TASK-10.5) handles this → empty text = return Ok("")
- **Definition of Done:** LLM call cancellable
- **Acceptance criteria:**
  - Cancel during LLM call → function returns Cancelled within 100ms
  - LLM returns error → function returns LlmError
- **Test plan:** unit: MockLlm that delays; cancel token; verify Cancelled
- **Estimated effort:** 4 hours

---

### TASK-10.4: AgentLoop::run — memory bookkeeping

- **§SPEC:** §5 (Memory bookkeeping section)
- **Labels:** `layer/loop`, `priority/critical`
- **Description:** After LLM response: push the full `AssistantMessage` to memory via `self.memory.push_assistant()`. This happens even when streaming has already emitted token deltas. Memory is always updated from the final complete message, never from deltas.
- **Files affected:**
  - `crates/duga-core/src/loop.rs` (memory section)
- **Types involved:** `Memory::push_assistant`, `AssistantMessage`
- **Functions to implement:** (inline in run)
- **Dependencies:** TASK-10.3, TASK-6.2
- **Implementation steps:**
  1. `self.memory.push_assistant(response.message.clone())`
  2. Test: verify memory has assistant message after LLM response
- **Definition of Done:** Memory updated
- **Accepted criteria:** After LLM response, memory.messages() includes the assistant message
- **Test plan:** unit: MockLlm; verify memory state
- **Estimated effort:** 1 hour

---

### TASK-10.5: AgentLoop::run — termination condition

- **§SPEC:** §5 (termination condition — no tool calls = answer)
- **Labels:** `layer/loop`, `priority/critical`
- **Description:** After memory bookkeeping, check termination: if `response.message.tool_calls.is_empty()`, emit `Event::FinalResponse` and return `Ok(response.message.text.unwrap_or_default())`. Free-form text accompanying tool calls does NOT terminate the loop.
- **Files affected:**
  - `crates/duga-core/src/loop.rs` (termination)
- **Types involved:** `AssistantMessage::is_termination()`, `Event::FinalResponse`
- **Functions to implement:** (inline in run)
- **Dependencies:** TASK-10.4, TASK-1.3 (is_termination)
- **Implementation steps:**
  1. `if response.message.tool_calls.is_empty() { self.events.emit(Event::FinalResponse).await; return Ok(response.message.text.unwrap_or_default()); }`
  2. Test
- **Edge cases:**
  - Text is None, tool_calls empty → return Ok("") (albeit unusual)
  - Text is Some(""), tool_calls empty → return Ok("")
- **Definition of Done:** Loop terminates correctly
- **Acceptance criteria:**
  - LLM returns text only → loop returns Ok(text)
  - LLM returns text + tool calls → loop continues (tool calls executed)
- **Test plan:** unit: MockLlm returning text-only; verify return value
- **Estimated effort:** 2 hours

---

### TASK-10.6: AgentLoop::handle_tool_call — dispatch + retry loop

- **§SPEC:** §5.1 (handle_tool_call), §27 (error retry logic)
- **Labels:** `layer/loop`, `priority/critical`
- **Description:** Implement `handle_tool_call()` as specified in SECT 5.1: (1) emit `ToolCallStarted`, (2) enter retry loop: dispatch via `self.tools.dispatch()`, on transient error (`Io` or `Plugin`) and `attempt < retry_on_error` → increment attempt and retry, (3) on non-transient or exhausted retries → break, (4) build `ToolResult` via `from_outcome`, (5) emit `ToolCallFinished`, (6) return ToolResult. GAP G1: calls are executed sequentially within a turn (`for call in ...`).
- **Files affected:**
  - `crates/duga-core/src/loop.rs` (handle_tool_call)
- **Types involved:** `ToolCall`, `ToolResult`, `ToolError::is_transient`, `EventSink`
- **Functions to implement:**
  - `AgentLoop::handle_tool_call(&mut self, call: ToolCall) -> ToolResult` (async)
- **Dependencies:** TASK-3.4 (ToolDispatcher::dispatch), TASK-1.4 (from_outcome), TASK-7.1 (events)
- **Implementation steps:**
  1. Emit `Event::ToolCallStarted { id: call.id, tool: call.tool.clone(), args: call.raw_args.clone() }`
  2. Record `Instant::now()`
  3. Retry loop: `let mut attempt = 0; loop { match self.tools.dispatch(&call, ctx).await { Ok(r) => break Ok(r), Err(e) if e.is_transient() && attempt < self.config.limits.retry_on_error => { attempt += 1; continue; }, Err(e) => break Err(e) } }`
  4. Build `ToolResult::from_outcome(call.id, &call.tool, outcome, started)`
  5. Emit `Event::ToolCallFinished { id, tool, success: result.success, output: result.output.clone(), duration_ms: result.duration_ms }`
  6. Return result
- **Edge cases:**
  - `retry_on_error: 0` → no retries (transient errors fail immediately)
  - Tool execution panics → catch and return ToolError::Plugin (or let panic propagate to loop)
  - Dispatcher returns error with non-transient kind → break immediately
- **Definition of Done:** Retry logic works
- **Acceptance criteria:**
  - Transient error (Io) → retried up to retry_on_error times
  - Non-transient error (InvalidArgs) → not retried
  - After max retries → error returned as ToolResult with success=false
- **Test plan:** unit: MockToolDispatcher that returns Io error N times then Ok; verify retry count
- **Estimated effort:** 5 hours

---

### TASK-10.7: AgentLoop::run — tool execution + result push + counter increment

- **§SPEC:** §5 (Tool execution section)
- **Labels:** `layer/loop`, `priority/critical`
- **Description:** In `run()`, after LLM response, iterate `response.message.tool_calls` sequentially: for each call, `self.handle_tool_call(call).await` → `self.memory.push_tool_result(result)` → `self.tool_calls += 1`. GAP G1 resolved as sequential execution.
- **Files affected:**
  - `crates/duga-core/src/loop.rs` (tool execution loop)
- **Types involved:** `ToolCall`, `ToolResult`, `Memory::push_tool_result`
- **Dependencies:** TASK-10.5, TASK-10.6, TASK-6.2
- **Implementation steps:**
  1. `for call in response.message.tool_calls { let result = self.handle_tool_call(call).await; self.memory.push_tool_result(result); self.tool_calls += 1; }`
  2. Test with multiple tool calls
- **Edge cases:**
  - Empty tool_calls → for loop body never runs (termination already handled)
  - Tool result with success=false → still pushed to memory (LLM should see the error)
  - Counter overflow → usize, practically impossible
- **Definition of Done:** Sequential execution with memory updates
- **Accepted criteria:**
  - LLM response with 2 tool calls → both executed, both results in memory
  - tool_calls counter incremented by 2
- **Test plan:** unit: MockLlm returning 2 tool calls; verify execution and memory
- **Estimated effort:** 3 hours

---

### TASK-10.8: AgentLoop::run — compression trigger + step increment

- **§SPEC:** §5 (compression section, step increment)
- **Labels:** `layer/loop`, `priority/critical`
- **Description:** After tool execution, check `self.memory.over_budget(tokenizer)`. If true: call `self.memory.compress(&*self.summarizer).await?` — on error, return `AgentError::SummarizerFailed`. Emit `Event::MemoryCompressed { before_tokens, after_tokens }`. Increment `self.steps += 1`.
- **Files affected:**
  - `crates/duga-core/src/loop.rs` (compression + step)
- **Types involved:** `Memory::compress`, `Memory::over_budget`, `Summarizer`, `Event::MemoryCompressed`
- **Dependencies:** TASK-10.7, TASK-6.5, TASK-6.4
- **Implementation steps:**
  1. `let before = self.memory.token_count(...)` (optional, for event)
  2. `if self.memory.over_budget(llm.as_ref()) { self.memory.compress(summarizer.as_ref()).await?; self.events.emit(Event::MemoryCompressed { before_tokens: before, after_tokens: self.memory.token_count(...) }).await; }`
  3. `self.steps += 1;`
  4. Test
- **Edge cases:**
  - Compress fails → return AgentError::SummarizerFailed
  - Compress reduces to exactly max_tokens → ok
  - Step overflow → not handled
- **Definition of Done:** Compression triggered, step incremented
- **Accepted criteria:**
  - Memory over budget → compress called, event emitted
  - Memory under budget → compress skipped
  - Step increments each iteration
- **Test plan:** unit: Set low max_tokens, push enough messages to trigger over_budget; verify compress called
- **Estimated effort:** 3 hours

---

### TASK-10.9: Core loop integration tests with mocks

- **§SPEC:** §5 (complete loop), §26–27 (cancellation + error)
- **Labels:** `layer/loop`, `priority/critical`
- **Description:** Create integration tests for the complete AgentLoop with MockLlm, MockToolDispatcher, MockSummarizer, and NullSink. Test scenarios: (1) single-turn answer (no tools), (2) multi-turn with tools, (3) max_steps reached, (4) max_tool_calls reached, (5) timeout, (6) cancellation, (7) retry on transient error, (8) no retry on non-transient, (9) compression trigger, (10) ContextOverflow. Use `tokio::test` with `time::pause()` for deterministic timing.
- **Files affected:**
  - `crates/duga-core/tests/loop_integration.rs` (new)
  - `crates/duga-core/tests/mocks.rs` (new — MockLlm, MockToolDispatcher, MockSummarizer)
- **Types involved:** `AgentLoop`, all mock types
- **Dependencies:** TASK-10.1 through TASK-10.8
- **Implementation steps:**
  1. Create `MockLlm` that returns scripted responses from a `VecDeque`
  2. Create `MockToolDispatcher` that returns pre-configured ToolResults
  3. Create `MockSummarizer` that returns "summary" string
  4. Test all 10 scenarios
  5. Verify events emitted in correct order
- **Acceptance criteria:** All 10 scenarios pass
- **Test plan:** integration tests as described
- **Estimated effort:** 8 hours
