# EPIC-13: Observability

**§SPEC:** §24, §25, §31  
**Labels:** `epic/observability`  
**Crates:** `duga-core` (tracing), `duga-events` (replay), `duga-replay` (new)

## Goal
Structured tracing spans, JSONL replay format validation, and the replay binary that can re-execute recorded sessions.

---

### TASK-13.1: Tracing spans — AgentLoop + tool execution

- **§SPEC:** §31 (Observability)
- **Labels:** `layer/observability`, `priority/high`
- **Description:** Add `tracing` spans to the core loop. `AgentLoop::run()` gets a root span `agent_run`. Each loop iteration gets `loop_iteration { step }`. Each tool call gets `tool_call { tool, id }`. Spans carry fields: `step`, `tool_calls_count`, `tokens_used`, `duration_ms`. Use `#[tracing::instrument]` macro where possible.
- **Files affected:**
  - `crates/duga-core/src/loop.rs` (add tracing)
- **Types involved:** `tracing` spans
- **Functions to implement:** (instrumentation additions)
- **Dependencies:** TASK-10.1 (AgentLoop)
- **Implementation steps:**
  1. Add `#[tracing::instrument(skip(self), fields(task = %task))]` to `run()`
  2. Inside loop: `tracing::info!(step = self.steps, "Loop iteration")`
  3. Inside `handle_tool_call`: `tracing::info_span!("tool_call", tool = %call.tool, id = %call.id).in_scope(|| { ... })`
  4. On compression: `tracing::info!(before_tokens, after_tokens, "Memory compressed")`
  5. On termination: `tracing::info!(result = %final_answer, "Agent finished")`
- **Definition of Done:** Spans visible in tracing output
- **Acceptance criteria:** `RUST_LOG=info cargo run ...` shows span hierarchy
- **Test plan:** integration: Run harness, verify tracing output
- **Estimated effort:** 3 hours

---

### TASK-13.2: Structured log fields — latency + tokens + limits

- **§SPEC:** §31 (token usage telemetry, tool execution timing)
- **Labels:** `layer/observability`, `priority/high`
- **Description:** Add structured fields to tracing events: LLM latency (`llm_latency_ms`), token usage (`prompt_tokens`, `completion_tokens`), tool duration (`tool_duration_ms`), memory state (`memory_tokens`). These fields enable metrics extraction and dashboarding.
- **Files affected:**
  - `crates/duga-core/src/loop.rs` (add fields)
- **Types involved:** `tracing::info!` with fields
- **Dependencies:** TASK-13.1
- **Implementation steps:**
  1. After LLM response: `tracing::info!(llm_latency_ms = ..., prompt_tokens = response.usage.prompt, completion_tokens = response.usage.completion, "LLM response")`
  2. After tool: `tracing::info!(tool_duration_ms = result.duration_ms, tool_success = result.success, "Tool finished")`
  3. On over_budget: `tracing::info!(memory_tokens = count, max_tokens = self.memory.max_tokens, "Memory budget check")`
- **Definition of Done:** Structured fields logged
- **Acceptance criteria:** `RUST_LOG=info` output contains `prompt_tokens=... completion_tokens=...`
- **Test plan:** integration: Run harness, grep logs for fields
- **Estimated effort:** 2 hours

---

### TASK-13.3: Replay JSONL format validation + roundtrip

- **§SPEC:** §25 (Replay Format — JSONL roundtrip)
- **Labels:** `layer/observability`, `priority/high`
- **Description:** In the existing `JsonlSink` (TASK-7.4), add validation that each emitted JSONL line is valid JSON and contains required fields. Implement a `JsonlReader` in `duga-replay` that reads a JSONL file, validates the format, and returns `Vec<StoredEvent>`. Create roundtrip tests.
- **Files affected:**
  - `crates/duga-events/src/jsonl.rs` (add optional validation)
  - `crates/duga-replay/Cargo.toml` (new)
  - `crates/duga-replay/src/lib.rs` (new)
  - `crates/duga-replay/src/reader.rs` (new)
- **Types involved:** `StoredEvent`, `JsonlReader`
- **Functions to implement:**
  - `JsonlReader::read(path: &Path) -> Result<Vec<StoredEvent>, ReplayError>`
  - `ReplayError` enum: `FileNotFound`, `InvalidJson { line: usize, error: String }`, `MissingField { line: usize, field: String }`
- **Dependencies:** TASK-7.4, TASK-7.2
- **Implementation steps:**
  1. Create `duga-replay` crate
  2. Implement `JsonlReader::read()`: read file line-by-line, parse each as `StoredEvent`, collect
  3. Validate each line has `seq`, `ts`, `event` fields
  4. Handle empty lines (skip), comment lines (skip `#`), truncated lines (error)
  5. Create roundtrip test: emit events → read back → verify
- **Edge cases:**
  - Empty file → empty vec (no error)
  - Trailing newline → skip last empty line
  - Mid-file corruption → ReplayError with line number
- **Definition of Done:** JSONL read/write roundtrip works
- **Acceptance criteria:**
  - Write 5 events → read back → 5 StoredEvents with matching seq + event data
  - Corrupted JSON → ReplayError with line number
- **Test plan:** unit: Roundtrip test with various events
- **Estimated effort:** 4 hours

---

### TASK-13.4: Replay binary — duga-replay

- **§SPEC:** §25 (Replay — replay binary), GAP G4
- **Labels:** `layer/observability`, `priority/high`
- **Description:** Implement the `duga-replay` binary that reads a JSONL replay file and re-executes the recorded session. GAP G4: ship a `duga-replay` binary. Modes: (1) `--substitute-llm` — uses MockLlm replaying stored responses, but executes tools for real; (2) `--mock-all` — replays everything, no real tool execution; (3) `--live-llm` — uses a real LLM but replays tool responses from the log.
- **Files affected:**
  - `crates/duga-replay/src/main.rs` (new)
  - `crates/duga-replay/src/mock_llm.rs` (new)
  - `crates/duga-replay/src/mock_tools.rs` (new)
- **Types involved:** `MockLlm`, `MockToolDispatcher`, `AgentLoop`
- **Functions to implement:**
  - `MockLlm::new(events: Vec<LlmResponse>) -> Self`
  - `MockToolDispatcher::from_events(events: &[StoredEvent]) -> Self`
  - Replay binary CLI
- **Dependencies:** TASK-13.3, TASK-10.1, TASK-14.1, TASK-14.2
- **Implementation steps:**
  1. Create `duga-replay` binary
  2. `MockLlm`: pops `LlmResponse` from queue on each `chat()` call
  3. `MockToolDispatcher`: looks up `ToolCallFinished.output` by call ID
  4. CLI: `duga-replay --input session.jsonl [--substitute-llm | --mock-all | --live-llm]`
  5. Run AgentLoop with mocks
  6. Print result
- **Edge cases:**
  - Replay file has fewer responses than tool calls → MockLlm returns error
  - Tool call ID not found → MockToolDispatcher returns error
  - Mixed modes: substitute-llm uses mocks for LLM, real tools
- **Definition of Done:** Replay binary works
- **Acceptance criteria:**
  - `duga-replay --input test.jsonl --mock-all` replays session
  - Replay output matches stored FinalResponse
- **Test plan:** integration: Record a session, replay it, verify output matches
- **Estimated effort:** 6 hours

---

### TASK-13.5: Golden file replay fixtures

- **§SPEC:** §25 (replay fixtures for testing)
- **Labels:** `layer/observability`, `priority/normal`
- **Description:** Create golden file replay fixtures in `tests/golden/`. One fixture: the fibonacci E2E scenario from the plan. The fixture is a JSONL file with the expected event sequence. Tests verify that replaying the fixture produces the expected final answer.
- **Files affected:**
  - `tests/golden/fibonacci.jsonl` (new)
  - `tests/golden/README.md` (new — fixture format docs)
- **Types involved:** `StoredEvent`
- **Dependencies:** TASK-13.4
- **Implementation steps:**
  1. Create fibonacci.jsonl with the 9-event sequence from PLAN.md §15
  2. Create test: read fixture, replay with --mock-all, verify answer
  3. Document fixture format in README
- **Definition of Done:** Golden file test passes
- **Acceptance criteria:** Replay fibonacci fixture → answer contains "test result: ok"
- **Test plan:** unit: Replay golden fixture
- **Estimated effort:** 3 hours
