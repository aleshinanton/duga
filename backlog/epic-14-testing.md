# EPIC-14: Testing Harness

**§SPEC:** §6a, §6b, §25 (mocks for testing)  
**Labels:** `epic/testing`  
**Crates:** `duga-core` (test utilities), `duga-replay` (mocks)

## Goal
Mock implementations for deterministic testing: MockTool, MockLlm, CapturingEventSink, and the E2E smoke test with the fibonacci scenario.

---

### TASK-14.1: MockTool — configurable responses

- **§SPEC:** §7 (Tool trait — mock implementation)
- **Labels:** `layer/testing`, `priority/critical`
- **Description:** Implement `MockTool` that implements `Tool<Args = serde_json::Value>`. Can be configured with a `Vec<(Value, Result<ToolResult, ToolError>)>` — a list of (expected_args, response) pairs. Also support an "always" mode that returns the same response regardless of args. Used for testing ToolDispatcher and AgentLoop without real tool execution.
- **Files affected:**
  - `crates/duga-core/src/testing.rs` (new — test utilities module)
  - Or `crates/duga-core/tests/common/mock_tool.rs` (test-only)
- **Types involved:** `MockTool`, `Tool`, `ToolResult`, `ToolError`
- **Functions to implement:**
  - `MockTool::new(name: &str, description: &str) -> Self`
  - `MockTool::with_response(mut self, expected_args: Value, response: Result<ToolResult, ToolError>) -> Self`
  - `MockTool::always(mut self, response: Result<ToolResult, ToolError>) -> Self`
  - `MockTool::with_retryable(mut self, retryable: bool) -> Self`
  - `impl Tool for MockTool`
- **Dependencies:** TASK-3.1, TASK-1.4
- **Implementation steps:**
  1. Define `MockTool` with `VecDeque<(Value, Result<ToolResult, ToolError>)>` or `Option<Result<ToolResult, ToolError>>` for always mode
  2. `execute()`: pop from queue or return always response
  3. Generate schema from `Value` type (generic object schema)
  4. Add tests: mock tool registrable in ToolDispatcher
- **Edge cases:**
  - Queue exhausted → return Io error (simulating unexpected call)
  - Always mode set → queue ignored
  - `retryable()` returns configured value
- **Definition of Done:** MockTool works
- **Accepted criteria:**
  - MockTool registered in ToolDispatcher
  - dispatch returns configured response
  - Retryable flag respected
- **Test plan:** unit: Test with ToolDispatcher
- **Estimated effort:** 4 hours

---

### TASK-14.2: MockLlm — scripted LlmResponse queue

- **§SPEC:** §6a (LlmClient — mock implementation)
- **Labels:** `layer/testing`, `priority/critical`
- **Description:** Implement `MockLlm` that implements `LlmClient`. Stores a `VecDeque<Result<LlmResponse, LlmError>>` — on each `chat()` call, pops and returns the next response. For `count_tokens()`, returns `messages.len() * 10` (or configurable multiplier). Optionally records received messages for assertion.
- **Files affected:**
  - `crates/duga-core/tests/common/mock_llm.rs` (test-only)
- **Types involved:** `MockLlm`, `LlmClient`, `LlmResponse`, `LlmError`
- **Functions to implement:**
  - `MockLlm::new(responses: Vec<Result<LlmResponse, LlmError>>) -> Self`
  - `MockLlm::with_token_multiplier(mut self, n: usize) -> Self`
  - `MockLlm::received_messages(&self) -> &[Vec<Message>]` (for assertion)
  - `impl LlmClient for MockLlm`
- **Dependencies:** TASK-8.1
- **Implementation steps:**
  1. Store `VecDeque` of responses + `Vec<Vec<Message>>` for received
  2. `chat()`: pop response, record messages, return response
  3. `count_tokens()`: `messages.len() * multiplier`
  4. Handle streaming: if `opts.streaming`, emit one LlmTokenDelta per word of the response text, then return full response
- **Edge cases:**
  - Queue exhausted → return `LlmError::ProviderError("mock exhausted")`
  - Streaming with no text → no token deltas emitted
- **Definition of Done:** MockLlm works
- **Accepted criteria:**
  - AgentLoop with MockLlm → runs with scripted responses
  - count_tokens returns expected values
  - Streaming mode emits token deltas
- **Test plan:** unit: Test with AgentLoop
- **Estimated effort:** 5 hours

---

### TASK-14.3: CapturingEventSink — in-memory event capture

- **§SPEC:** §6b (EventSink — test implementation)
- **Labels:** `layer/testing`, `priority/critical`
- **Description:** Implement `CapturingEventSink` that implements `EventSink` and stores all emitted events in a `Vec<Event>` behind a `Mutex`. Used in tests to verify event order and content. Provides `fn events(&self) -> Vec<Event>` for assertion.
- **Files affected:**
  - `crates/duga-core/tests/common/capturing_sink.rs` (test-only)
- **Types involved:** `CapturingEventSink`, `EventSink`, `Event`
- **Functions to implement:**
  - `CapturingEventSink::new() -> Self`
  - `CapturingEventSink::events(&self) -> Vec<Event>`
  - `CapturingEventSink::events_of_type<T>(&self, predicate: fn(&Event) -> bool) -> Vec<&Event>`
  - `impl EventSink for CapturingEventSink`
- **Dependencies:** TASK-7.2
- **Implementation steps:**
  1. Store `Arc<Mutex<Vec<Event>>>`
  2. `emit()`: lock, push clone of event
  3. `events()`: lock, clone vec
  4. `events_of_type()`: filter
- **Edge cases:**
  - Thread safety: `Mutex` ensures safe concurrent access
  - Large event count → vec grows unbounded → acceptable in tests
- **Definition of Done:** Captures all emitted events
- **Estimated effort:** 3 hours

---

### TASK-14.4: E2E integration test harness

- **§SPEC:** §5 (complete agent execution)
- **Labels:** `layer/testing`, `priority/critical`
- **Description:** Create the E2E test harness in `tests/e2e/`. Provides a function `run_test_agent(config: AgentConfig, mock_llm: MockLlm, mock_tools: Vec<MockTool>, task: &str) -> Result<(String, CapturingEventSink)>` that builds a minimal AgentLoop with mocks and runs it. Uses `tokio::time::pause()` + `tokio::time::advance()` for deterministic timing.
- **Files affected:**
  - `tests/e2e/common/mod.rs` (new)
  - `tests/e2e/common/harness.rs` (new)
  - `tests/e2e/Cargo.toml` (or use integration test in duga-core)
- **Types involved:** `AgentLoop`, all mock types
- **Functions to implement:**
  - `fn test_harness() -> ...` — builder for test AgentLoop
- **Dependencies:** TASK-14.1, TASK-14.2, TASK-14.3, TASK-10.1
- **Implementation steps:**
  1. Create test helper that builds AgentLoop with mocks
  2. Configure defaults: max_steps=10, max_tool_calls=20, max_runtime=60s, retry_on_error=0
  3. Provide convenience for adding mock tools and LLM responses
  4. Use `tokio::time::pause()` for time control
- **Definition of Done:** Harness builds and runs
- **Accepted criteria:** Can run a simple agent with 1-turn answer
- **Test plan:** This IS the test harness — tests use it
- **Estimated effort:** 4 hours

---

### TASK-14.5: E2E smoke test — fibonacci scenario

- **§SPEC:** §5, §11, §25 (E2E smoke test)
- **Labels:** `layer/testing`, `priority/critical`
- **Description:** Implement the fibonacci E2E smoke test: Agent receives *"Write a Rust function fibonacci(n: u64) -> u64 with unit tests. Ensure cargo test passes."* The test uses MockLlm with scripted responses that produce the 9-event sequence from PLAN.md §15: write src/fib.rs → read Cargo.toml → write Cargo.toml → shell cargo test → text answer. Verify the final answer and event sequence.
- **Files affected:**
  - `tests/e2e/fibonacci.rs` (new)
- **Types involved:** `MockLlm`, `MockTool`, `CapturingEventSink`
- **Dependencies:** TASK-14.4
- **Implementation steps:**
  1. Script MockLlm with 5 responses matching the fibonacci scenario
  2. Script MockTools (write, read, shell) with expected responses
  3. Run agent
  4. Assert final answer contains "test result: ok"
  5. Assert event sequence: LoopIteration → LlmRequest → LlmResponse → ToolCallStarted(write) → ToolCallFinished → ... → FinalResponse
  6. Assert events are emitted in correct order (GAP G3 satisfaction)
- **Edge cases:**
  - Tool responses must match what the MockLlm expects — consistency between mocks
  - Event count must match expected (9 events)
- **Definition of Done:** Fibonacci test passes
- **Acceptance criteria:**
  - Agent completes without errors
  - Final answer contains success message
  - Event order is correct
- **Test plan:** This IS the test
- **Estimated effort:** 6 hours

---

### TASK-14.6: Replay roundtrip test

- **§SPEC:** §25 (replay roundtrip verification)
- **Labels:** `layer/testing`, `priority/high`
- **Description:** Create a test that verifies the complete roundtrip: run an agent session with CapturingEventSink → write events to JSONL → read JSONL back → replay with MockLlm + MockToolDispatcher → verify the final answer matches. This validates the entire telemetry + replay pipeline.
- **Files affected:**
  - `tests/e2e/replay_roundtrip.rs` (new)
- **Types involved:** `CapturingEventSink`, `JsonlSink`, `JsonlReader`, `MockLlm`, `MockToolDispatcher`, `AgentLoop`
- **Dependencies:** TASK-14.5, TASK-13.3, TASK-13.4, TASK-7.4
- **Implementation steps:**
  1. Run fibonacci scenario with CapturingEventSink
  2. Write captured events to temp JSONL via JsonlSink
  3. Read JSONL back with JsonlReader
  4. Build MockLlm from LlmResponse events
  5. Build MockToolDispatcher from ToolCallFinished events
  6. Run AgentLoop with mocks
  7. Assert final answer matches original
- **Definition of Done:** Roundtrip passes
- **Accepted criteria:**
  - Replayed answer matches original answer exactly
  - All event types preserved in replay
  - Replay produces same number of tool calls
- **Test plan:** This IS the test
- **Estimated effort:** 5 hours
