# EPIC-7: Event System

**§SPEC:** §6b, §24–25  
**Labels:** `epic/events`  
**Crate:** `duga-events`

## Goal
Implement the `Event` enum, `EventSink` trait, `JsonlSink` with writer task, `MultiSink`, `NullSink`, `SeqAllocator`, and `Redactor` for secret pattern scrubbing.

---

### TASK-7.1: Event enum — all variants

- **§SPEC:** §24 (Event system — all variants)
- **Labels:** `layer/events`, `priority/critical`
- **Description:** Define the `Event` enum in `duga-events` with all 9 variants from SECT 24: `LoopIteration { step: usize }`, `LlmRequest { messages, tools, opts }`, `LlmTokenDelta { text }`, `LlmResponse { message, usage, latency_ms }`, `ToolCallStarted { id, tool, args }`, `ToolCallFinished { id, tool, success, output, duration_ms }`, `ToolCallFailed { id, tool, error }`, `MemoryCompressed { before_tokens, after_tokens }`, `FinalResponse`. All derive `Serialize`, `Deserialize`, `Clone`, `Debug`. Each variant carries a `seq` and `ts` — these are added by the sink layer, not part of the enum payload.
- **Files affected:**
  - `crates/duga-events/Cargo.toml` (new)
  - `crates/duga-events/src/lib.rs` (new)
  - `crates/duga-events/src/event.rs` (new)
- **Types involved:** `Event` enum, all referenced types from duga-types
- **Functions to implement:** None — pure data enum.
- **Dependencies:** TASK-1.2 (Message), TASK-1.3 (ToolCall, AssistantMessage), TASK-1.4 (ToolResult), TASK-1.7 (AgentConfig), TASK-1.8 (LlmResponse, TokenUsage)
- **Implementation steps:**
  1. Create `duga-events` crate
  2. Define `Event` enum in `event.rs`
  3. Derive `Serialize`, `Deserialize`, `Clone`, `Debug`, `PartialEq`
  4. Add `#[serde(tag = "event")]` for type-discriminated JSON serialization
  5. Add test for serialize/deserialize roundtrip of each variant
- **Edge cases:**
  - `ToolCallStarted.args` is `serde_json::Value` — may be any valid JSON
  - `LlmResponse.message` contains `AssistantMessage` with optional text and tool calls
- **Definition of Done:** All variants compile, roundtrip passes
- **Acceptance criteria:**
  - Each variant serializes with `"event"` field matching variant name
  - Deserialization reconstructs correct variant
- **Test plan:** unit: Test all 9 variants with sample data
- **Estimated effort:** 3 hours

---

### TASK-7.2: EventSink trait + NullSink

- **§SPEC:** §6b (EventSink Interface)
- **Labels:** `layer/events`, `priority/critical`
- **Description:** Define `trait EventSink: Send + Sync { async fn emit(&self, event: &Event); }`. Implement `NullSink` that discards all events (for tests and when no sink is configured). GAP G2: `emit` takes `&Event` (shared reference) — the event after seq/ts have been attached. The async signature allows sinks to do blocking I/O, but the unbounded channel design means emit should never block the loop.
- **Files affected:**
  - `crates/duga-events/src/sink.rs` (new)
  - `crates/duga-events/src/lib.rs` (add module)
- **Types involved:** `EventSink` (trait), `NullSink`, `Event`
- **Functions to implement:**
  - `trait EventSink`
  - `struct NullSink` + `impl EventSink for NullSink`
  - `struct StoredEvent { seq: u64, ts: String, event: Event }` — for replay/serialization
- **Dependencies:** TASK-7.1, TASK-1.8 (Seq)
- **Implementation steps:**
  1. Define `EventSink` trait with `async fn emit(&self, event: &Event)`
  2. Implement `NullSink` — `emit()` does nothing
  3. Define `StoredEvent` — the on-disk representation with seq + ts
  4. Add test
- **Edge cases:**
  - `&Event` must be thread-safe for concurrent emission → `Event: Send + Sync`
- **Definition of Done:** Trait compiles, NullSink works
- **Acceptance criteria:**
  - `NullSink.emit(&event).await` — no panic, no side effects
  - `StoredEvent` serializes with `seq`, `ts`, plus untagged event fields
- **Test plan:** unit: Test NullSink doesn't crash
- **Estimated effort:** 3 hours

---

### TASK-7.3: SeqAllocator + seq/ts attachment

- **§SPEC:** §24 (seq + ts on events), §25 (JSONL format)
- **Labels:** `layer/events`, `priority/critical`
- **Description:** Implement `SeqAllocator` wrapping `AtomicU64` that returns monotonically increasing sequence numbers. Implement a function `prepare_event(seq: u64, event: Event) -> StoredEvent` that attaches sequence number and ISO-8601 timestamp. The seq is assigned at emit time by the sink, not by the loop.
- **Files affected:**
  - `crates/duga-events/src/seq.rs` (new)
  - `crates/duga-events/src/lib.rs` (add module)
- **Types involved:** `SeqAllocator`, `Seq`, `StoredEvent`, `Event`
- **Functions to implement:**
  - `SeqAllocator::new() -> Self`
  - `SeqAllocator::next(&self) -> Seq`
  - `fn prepare_event(seq: Seq, event: &Event) -> StoredEvent`
- **Dependencies:** TASK-7.1, TASK-7.2, TASK-1.8 (Seq)
- **Implementation steps:**
  1. Define `SeqAllocator(AtomicU64)` — starts at 1
  2. `next()`: `AtomicU64::fetch_add(1, Ordering::Relaxed)` → wrap in Seq
  3. `prepare_event()`: capture `Utc::now().to_rfc3339()` → build StoredEvent
  4. Add tests
- **Edge cases:**
  - Seq overflow: `u64::MAX` → wraps to 0 (practically impossible)
  - Timestamp precision: RFC 3339 with milliseconds
  - Atomic ordering: Relaxed is sufficient (only one allocator, no shared state)
- **Definition of Done:** Monotonic seqs, timestamps attached
- **Acceptance criteria:**
  - `SeqAllocator::next()` returns 1, 2, 3...
  - `prepare_event` produces StoredEvent with seq + ISO-8601 ts
- **Test plan:** unit: Test seq monotonicity, test prepare_event format
- **Estimated effort:** 3 hours

---

### TASK-7.4: JsonlSink with writer task

- **§SPEC:** §25 (Replay Format — JSONL)
- **Labels:** `layer/events`, `priority/critical`
- **Description:** Implement `JsonlSink` that spawns a background writer task. Uses `mpsc::UnboundedSender<StoredEvent>` for non-blocking event submission. The writer task receives `StoredEvent` values, serializes each to one JSON line, and appends to the file. Implement `JsonlSink::new(path: PathBuf) -> Self` which opens the file and spawns the task. Implement `EventSink for JsonlSink` — `emit()` sends to the channel.
- **Files affected:**
  - `crates/duga-events/src/jsonl.rs` (new)
  - `crates/duga-events/src/lib.rs` (add module)
- **Types involved:** `JsonlSink`, `EventSink`, `StoredEvent`, `SeqAllocator`
- **Functions to implement:**
  - `JsonlSink::new(path: impl Into<PathBuf>) -> io::Result<Self>`
  - `impl EventSink for JsonlSink`
  - Interior: writer task spawned via `tokio::spawn`
- **Dependencies:** TASK-7.2, TASK-7.3
- **Implementation steps:**
  1. `JsonlSink` holds `sender: UnboundedSender<StoredEvent>`, `seq: Arc<SeqAllocator>`
  2. `new()`: open file for append, create channel, spawn writer task
  3. Writer task loop: `while let Some(event) = rx.recv().await` → `serde_json::to_string(&event)` → `writeln!(file, "{}", json)`
  4. `emit()`: `self.seq.next()` → `prepare_event()` → `self.sender.send(stored)`
  5. On file write error → `tracing::error!` and continue (don't crash the agent)
  6. Add test
- **Edge cases:**
  - File can't be opened → return `io::Error`
  - Channel full (unbounded → never full, but memory grows — bounded by max_events)
  - Writer task panics → new events silently lost (acceptable for telemetry)
  - File rotation: not in v1 — single file per run
  - Emit called after writer task died → `send` returns Err, log warning
- **Definition of Done:** JSONL written correctly, `cargo test` passes
- **Acceptance criteria:**
  - `JsonlSink::new(path)` creates file
  - Emit 3 events → file contains 3 JSON lines
  - Each line is valid JSON with seq, ts, event fields
- **Test plan:** unit: Create temp file, emit events, read back and verify JSONL format
- **Estimated effort:** 5 hours

---

### TASK-7.5: MultiSink fan-out

- **§SPEC:** §6b (multiple sinks via MultiSink)
- **Labels:** `layer/events`, `priority/critical`
- **Description:** Implement `MultiSink` that holds `Vec<Arc<dyn EventSink>>` and fans out `emit()` calls to all child sinks. If one child sink fails (panics), the others continue. `emit()` calls all sinks sequentially (fan-out, not fan-in).
- **Files affected:**
  - `crates/duga-events/src/multi.rs` (new)
  - `crates/duga-events/src/lib.rs` (add module)
- **Types involved:** `MultiSink`, `EventSink`
- **Functions to implement:**
  - `MultiSink::new(sinks: Vec<Arc<dyn EventSink>>) -> Self`
  - `MultiSink::add(&mut self, sink: Arc<dyn EventSink>)`
  - `impl EventSink for MultiSink`
- **Dependencies:** TASK-7.2
- **Implementation steps:**
  1. `emit()`: iterate sinks, call `sink.emit(event).await`
  2. Handle panics: wrap each call in `std::panic::catch_unwind` — but async makes this tricky. Simpler: if one sink's emit returns an error (unlikely since emit returns ()), log and continue
  3. Test with NullSink + MockSink
- **Edge cases:**
  - Empty MultiSink → emit is no-op
  - One sink blocks → all others delayed (sequential execution)
  - Arc clone: MultiSink can be cloned for sharing — but the inner vec is shared
- **Definition of Done:** Fan-out works
- **Acceptance criteria:**
  - Emit to MultiSink with 2 child sinks → both receive event
  - One child panics → other still receives event
- **Test plan:** unit: MockSink that records events; test fan-out
- **Estimated effort:** 3 hours

---

### TASK-7.6: Redactor — secret pattern matching

- **§SPEC:** §24.1 (Redaction)
- **Labels:** `layer/events`, `priority/critical`
- **Description:** Implement `Redactor` that holds a list of regex patterns and replaces matching string values with `"<REDACTED>"`. Patterns include the built-in secret patterns from SECT 20 (`*_TOKEN`, `*_KEY`, `*_SECRET`, `*_PASSWORD`) plus any operator-configured regexes from config. Redaction walks `serde_json::Value` trees (ToolCallStarted.args, ToolCallFinished.output) and replaces matching string values in-place.
- **Files affected:**
  - `crates/duga-events/src/redactor.rs` (new)
  - `crates/duga-events/src/lib.rs` (add module)
- **Types involved:** `Redactor`, `serde_json::Value`
- **Functions to implement:**
  - `Redactor::new(extra_patterns: Vec<Regex>) -> Self`
  - `Redactor::redact_value(&self, value: &mut Value)` — recursively walk and replace
  - `Redactor::redact_str(&self, s: &str) -> Cow<str>` — check if string matches any pattern
- **Dependencies:** TASK-7.1 (Event), TASK-2.5 (is_secret_pattern)
- **Implementation steps:**
  1. Build-in patterns: compile regexes for `(?i).*_(token|key|secret|password)$` (case-insensitive pattern matching variable names, but redact VALUES not names)
  2. Actually, SECT 24.1 says "replaces values matching the secret patterns" — walks event payloads and replaces VALUES (the content of variables), not the variable names themselves. However, the spec also says "environment variable values surfaced in tool args/output" — so redact based on context:
     - Redact is best-effort. For MVP: redact any string value that looks like a potential secret (long random strings, base64 patterns, known API key prefixes like `sk-`, `ghp_`)
  3. Implement `redact_value()`: recursively walk `Value`, for strings check patterns, for objects/arrays recurse
  4. Add tests
- **Edge cases:**
  - Nested JSON → recurse into all levels
  - Very large strings → regex matching is O(n)
  - False positives: short strings like "token" or "key" in filenames → this is why best-effort is noted
  - GAP G14: redact before serialization to JSONL, so `seq` is stable
- **Definition of Done:** Redaction works
- **Acceptance criteria:**
  - String `"sk-proj-abc123"` → `"<REDACTED>"`
  - String `"ghp_abcdef123456"` → `"<REDACTED>"`
  - String `"hello world"` → unchanged
  - Nested object values redacted
- **Test plan:** unit: Test known secret patterns, test non-secret strings, test nested JSON, test empty JSON
- **Estimated effort:** 5 hours

---

### TASK-7.7: Redactor integration with JsonlSink

- **§SPEC:** §24.1 (Redaction applied before emission)
- **Labels:** `layer/events`, `priority/critical`
- **Description:** Integrate `Redactor` into the event emission pipeline. Before `JsonlSink` serializes a `StoredEvent`, run redaction on relevant fields: `ToolCallStarted.args`, `ToolCallFinished.output`, and `LlmRequest.messages` text content. GAP G14: redact after seq/ts attachment but before serialization — seq remains stable across redacted and non-redacted runs.
- **Files affected:**
  - `crates/duga-events/src/jsonl.rs` (integrate redactor)
  - `crates/duga-events/src/redacting_sink.rs` (new — decorator)
- **Types involved:** `RedactingSink`, `JsonlSink`, `Redactor`, `Event`
- **Functions to implement:**
  - `RedactingSink::new(inner: Arc<dyn EventSink>, redactor: Redactor) -> Self`
  - `impl EventSink for RedactingSink`
- **Dependencies:** TASK-7.4, TASK-7.6
- **Implementation steps:**
  1. Create `RedactingSink` wrapper
  2. `emit()`: clone the event → redact mutable fields → forward to inner sink
  3. Redaction targets:
      - `Event::ToolCallStarted { args, .. }` → redact args
      - `Event::ToolCallFinished { output, .. }` → redact output
      - `Event::LlmRequest { messages, .. }` → redact message text strings
  4. Add integration test
- **Edge cases:**
  - Event has no redactable fields → pass through unchanged
  - Redaction panics → catch and log, don't block emission
- **Definition of Done:** Redaction pipeline works end-to-end
- **Acceptance criteria:**
  - Event with secret in args → emitted with `<REDACTED>` in JSONL
  - Same event without secrets → emitted unchanged
  - Seq numbers are identical with/without redacted content
- **Test plan:** integration: Emit events with secret-like values through RedactingSink → JsonlSink, verify JSONL output
- **Estimated effort:** 4 hours

---

### TASK-7.8: Event emission from ToolDispatcher + AgentLoop integration points

- **§SPEC:** §5.1 (handle_tool_call events), §5 (loop events)
- **Labels:** `layer/events`, `priority/critical`
- **Description:** Wire event emission into the actual execution path. In `ToolDispatcher::dispatch()` (or in `handle_tool_call` in EPIC-10), emit `Event::ToolCallStarted` before execution and `Event::ToolCallFinished`/`Event::ToolCallFailed` after. In the core loop, emit `Event::LoopIteration`, `Event::LlmRequest`, `Event::LlmResponse`, `Event::MemoryCompressed`, `Event::FinalResponse`. This task focuses on the event types and ensuring they carry correct data — actual emission points are in EPIC-10.
- **Files affected:**
  - `crates/duga-tools/src/dispatcher.rs` (add event emission)
  - `crates/duga-core/src/loop.rs` (event emission points)
- **Types involved:** `Event` variants, `EventSink`, `ToolDispatcher`
- **Functions to implement:** (integration wiring)
- **Dependencies:** TASK-7.1, TASK-3.4, TASK-10.1
- **Implementation steps:**
  1. Modify `ToolDispatcher::dispatch()` to accept `&dyn EventSink` through ToolContext or as parameter
  2. Emit `ToolCallStarted` with id, tool name, and redacted args
  3. After execution: on Ok → emit `ToolCallFinished`, on Err → emit `ToolCallFailed`
  4. In AgentLoop (EPIC-10): emit loop events
  5. Ensure events carry correct data per SECT 24
- **Edge cases:**
  - Event emission failure should not abort tool execution — `emit()` returns (), can't fail
  - ToolCallStarted emitted before actual dispatch → if dispatch panics, event is already emitted (acceptable for telemetry)
- **Definition of Done:** Events emitted at correct points
- **Acceptance criteria:**
  - Tool call produces ToolCallStarted + ToolCallFinished events in order
  - Failed tool call produces ToolCallStarted + ToolCallFailed
  - Loop iteration produces LoopIteration event
- **Test plan:** integration: Mock AgentLoop run, capture events, verify order and content
- **Estimated effort:** 4 hours
