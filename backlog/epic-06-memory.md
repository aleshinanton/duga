# EPIC-6: Memory System

**§SPEC:** §21–23  
**Labels:** `epic/memory`  
**Crate:** `duga-core`

## Goal
Implement the `Memory` struct with sliding window, token budget tracking, context compression via summarizer, and summary overflow rules.

---

### TASK-6.1: Memory struct + new constructor

- **§SPEC:** §21 (Memory System)
- **Labels:** `layer/memory`, `priority/critical`
- **Description:** Create the `duga-core` crate and implement `Memory` struct with fields: `system_messages: Vec<Message>`, `summary: Option<SummaryMessage>`, `recent_messages: VecDeque<Message>`, `max_tokens: usize`, `compress_at_ratio: f64`. Implement `Memory::new(system: Vec<Message>, max_tokens: usize, compress_at_ratio: f64) -> Self`. System messages are pinned at construction; never compressed.
- **Files affected:**
  - `crates/duga-core/Cargo.toml` (new)
  - `crates/duga-core/src/lib.rs` (new)
  - `crates/duga-core/src/memory.rs` (new)
- **Types involved:** `Memory`, `Message`, `SummaryMessage`
- **Functions to implement:**
  - `Memory::new(system_messages: Vec<Message>, max_tokens: usize, ratio: f64) -> Self`
  - `Memory::system_messages(&self) -> &[Message]`
- **Dependencies:** TASK-1.2 (Message), TASK-1.8 (SummaryMessage)
- **Implementation steps:**
  1. Create `duga-core` crate with deps: `duga-types`, `duga-llm`, `duga-tools`, `duga-events`, `duga-sandbox`, `tokio`, `tokio-util`, `tracing`
  2. Define `Memory` struct
  3. Implement `new()`: set system_messages, max_tokens, ratio (clamp to 0.1–1.0)
  4. Add test
- **Edge cases:**
  - `ratio: 0.0` → compress immediately (always over budget)
  - `ratio: 1.0` → compress at exact budget
  - `max_tokens: 0` → always over budget, but only `ContextOverflow` if even system doesn't fit
- **Definition of Done:** Struct compiles, `cargo test` passes
- **Acceptance criteria:**
  - `Memory::new(vec![msg], 8192, 0.8)` → fields set correctly
  - `ratio` outside 0.1–1.0 → clamped
- **Test plan:** unit: Test constructor with valid/invalid ratio
- **Estimated effort:** 3 hours

---

### TASK-6.2: Memory::push_user, push_assistant, push_tool_result

- **§SPEC:** §21 (Memory — message queues)
- **Labels:** `layer/memory`, `priority/critical`
- **Description:** Implement the three push methods: `push_user(task: String)` creates a user Message and pushes to `recent_messages`, `push_assistant(msg: AssistantMessage)` pushes an assistant Message (with optional text + tool calls), `push_tool_result(result: ToolResult)` pushes a tool Message (role=tool, content=result.output, tool_call_id=result.tool_call_id). The original user task is also added to `pinned_facts` for compression safety.
- **Files affected:**
  - `crates/duga-core/src/memory.rs` (append methods)
- **Types involved:** `Memory`, `Message`, `AssistantMessage`, `ToolResult`
- **Functions to implement:**
  - `Memory::push_user(&mut self, task: String)`
  - `Memory::push_assistant(&mut self, msg: AssistantMessage)`
  - `Memory::push_tool_result(&mut self, result: ToolResult)`
- **Dependencies:** TASK-6.1
- **Implementation steps:**
  1. `push_user`: `Message::user(task.clone())` + push to `recent_messages`; also store task as pinned fact
  2. `push_assistant`: convert `AssistantMessage` to `Message::assistant(text, tool_calls)` + push
  3. `push_tool_result`: `Message::tool(result.tool_call_id, result.output)` + push
  4. Add tests
- **Edge cases:**
  - Assistant message has text AND tool calls → both stored in Message content blocks
  - Tool result with empty output → still pushed (the LLM should know the tool ran)
  - Multiple users → only first task is pinned for compression (stored in `pinned_task: Option<String>`)
- **Definition of Done:** `cargo test` passes
- **Acceptance criteria:**
  - `push_user("task")` → recent_messages len=1, pinned_task="task"
  - `push_assistant(AssistantMessage{ text: Some("hi"), tool_calls: vec![] })` → recent_messages len=2
  - `push_tool_result(ToolResult{ output: "ok", .. })` → recent_messages len=3, role=Tool
- **Test plan:** unit: Test each push method, verify message roles and content
- **Estimated effort:** 3 hours

---

### TASK-6.3: Memory::messages ordering

- **§SPEC:** §21 (Memory — messages() returns correct order)
- **Labels:** `layer/memory`, `priority/critical`
- **Description:** Implement `Memory::messages(&self) -> Vec<Message>` that returns the full message list in order: system_messages → summary (if any) → recent_messages. This is what's sent to the LLM. The summary is converted to a single `Message::assistant` with the summary content. Pinned facts are prefixed to the summary output.
- **Files affected:**
  - `crates/duga-core/src/memory.rs` (append method)
- **Types involved:** `Memory`, `Message`, `SummaryMessage`
- **Functions to implement:**
  - `Memory::messages(&self) -> Vec<Message>`
  - `fn summary_to_message(summary: &SummaryMessage) -> Message`
- **Dependencies:** TASK-6.2
- **Implementation steps:**
  1. Build result vec
  2. Extend with `self.system_messages.clone()`
  3. If `self.summary` is Some: build summary message with pinned facts prefix + summary content
  4. Extend with `self.recent_messages` (clone)
  5. Return
  6. Add test
- **Edge cases:**
  - No summary → skip summary message
  - No recent_messages → return just system (initial state)
  - Pinned facts in summary: format as `"Key facts:\n- fact1\n- fact2\n\nSummary:\n{content}"`
- **Definition of Done:** Order is correct, `cargo test` passes
- **Acceptance criteria:**
  - Empty memory → messages() = system_messages
  - With summary + 2 recent → order: system, summary, recent[0], recent[1]
- **Test plan:** unit: Test all three ordering scenarios
- **Estimated effort:** 2 hours

---

### TASK-6.4: Memory::over_budget token counting

- **§SPEC:** §21 (over_budget), §6a (count_tokens)
- **Labels:** `layer/memory`, `priority/critical`
- **Description:** Implement `Memory::over_budget(&self, tokenizer: &dyn LlmClient) -> bool` that calls `tokenizer.count_tokens(&self.messages())` and compares against `max_tokens * compress_at_ratio`. Returns true when the budget is exceeded (triggering compression). GAP G13: `Memory` holds `Arc<dyn Tokenizer>` (a separated trait) rather than `Arc<dyn LlmClient>`. For now, accept `&dyn LlmClient` as a parameter. A `Tokenizer` trait extraction will be done as a subtask if needed.
- **Files affected:**
  - `crates/duga-core/src/memory.rs` (append method)
- **Types involved:** `Memory`, `LlmClient`
- **Functions to implement:**
  - `Memory::over_budget(&self, tokenizer: &dyn LlmClient) -> bool`
  - `Memory::token_count(&self, tokenizer: &dyn LlmClient) -> usize`
- **Dependencies:** TASK-6.3, TASK-8.1 (LlmClient trait with count_tokens)
- **Implementation steps:**
  1. Call `tokenizer.count_tokens(&messages)`
  2. Compare: `count > (self.max_tokens as f64 * self.compress_at_ratio) as usize`
  3. Return boolean
  4. `token_count()` returns raw count for event emission
  5. Add test with mock tokenizer
- **Edge cases:**
  - `count_tokens` returns 0 (empty messages) → not over budget (if max_tokens > 0)
  - `max_tokens = 0` → always over budget
  - GAP G6: if `count_tokens` is async → this method becomes async too. For MVP, assume sync.
- **Definition of Done:** `cargo test` passes
- **Acceptance criteria:**
  - 100 tokens, max_tokens=200, ratio=0.8 → over_budget=false (100 < 160)
  - 170 tokens, max_tokens=200, ratio=0.8 → over_budget=true (170 > 160)
- **Test plan:** unit: Mock tokenizer returning known counts; test boundary cases
- **Estimated effort:** 3 hours

---

### TASK-6.5: Context compression — oldest half summarization

- **§SPEC:** §22 (Context Compression — summarization rules)
- **Labels:** `layer/memory`, `priority/critical`
- **Description:** Implement `Memory::compress(&mut self, summarizer: &dyn Summarizer) -> Result<(), AgentError>`. The compression algorithm: (1) split `recent_messages` into oldest half and newest half, (2) feed oldest half + existing summary (as context only) to the summarizer, (3) replace self.summary with the new summary, (4) replace recent_messages with the newest half. Summaries NEVER summarize other summaries — the old summary is included as input plaintext, not recursively summarized.
- **Files affected:**
  - `crates/duga-core/src/memory.rs` (append compress method)
  - `crates/duga-core/src/summarizer.rs` (new — Summarizer trait)
- **Types involved:** `Memory`, `Summarizer`, `SummaryMessage`, `AgentError`
- **Functions to implement:**
  - `Memory::compress(&mut self, summarizer: &dyn Summarizer) -> Result<(), AgentError>`
  - `trait Summarizer: Send + Sync { async fn summarize(&self, messages: &[Message]) -> Result<SummaryMessage>; }`
- **Dependencies:** TASK-6.4, TASK-1.8 (SummaryMessage)
- **Implementation steps:**
  1. Define `Summarizer` trait in `crates/duga-core/src/summarizer.rs`
  2. In `compress()`:
      - Split `recent_messages`: `let split = recent.len() / 2; let oldest = recent.drain(..split).collect::<Vec<_>>();` — newest remains
      - Build input for summarizer: oldest messages + (existing summary as a single context message)
      - Call `summarizer.summarize(&input).await`
      - Replace `self.summary` with result
      - The summary `SummaryMessage` must include the pinned task in `pinned_facts`
  3. Add test with `MockSummarizer`
- **Edge cases:**
  - `recent_messages` has 1 message → oldest half is 0 messages, newest is 1 → nothing to compress. `compress()` should be called only when `over_budget()`.
  - First compression (no existing summary) → skip "existing summary as input" step
  - Summarizer returns empty summary → replace self.summary with None? → No, always store (empty SummaryMessage)
  - Pinned facts must be passed to summarizer and come back verbatim
- **Definition of Done:** Compression works, `cargo test` passes
- **Acceptance criteria:**
  - 10 recent messages, compress → 5 newest retained, summary exists
  - 10 recent + existing summary → old summary included in summarizer input, NOT re-summarized
  - Pinned task appears in summary.pinned_facts
- **Test plan:** unit: MockSummarizer that returns known content; test split size, test pinned facts, test old summary inclusion
- **Estimated effort:** 6 hours

---

### TASK-6.6: Summary overflow rules + pinned fact protection

- **§SPEC:** §22.1 (Summary overflow)
- **Labels:** `layer/memory`, `priority/critical`
- **Description:** Implement the overflow safety rules from SECT 22.1 in `compress()`: after compression, if system + summary + retained-recent still exceeds `max_tokens`, (1) drop the oldest non-pinned messages from retained-recent one at a time until budget is met, (2) if budget still can't be met with only system + summary + pinned messages, return `AgentError::ContextOverflow`. NEVER silently truncate pinned content.
- **Files affected:**
  - `crates/duga-core/src/memory.rs` (append to compress)
- **Types involved:** `Memory`, `AgentError::ContextOverflow`
- **Functions to implement:**
  - `fn drop_until_budget(&mut self, tokenizer: &dyn LlmClient) -> Result<(), AgentError>` (called after compress)
- **Dependencies:** TASK-6.5
- **Implementation steps:**
  1. After `compress()`, call `drop_until_budget()`
  2. While `over_budget(tokenizer)`:
      - If `recent_messages` has non-pinned messages → remove oldest non-pinned
      - If only pinned messages remain → return `Err(AgentError::ContextOverflow)`
  3. Add test with mock tokenizer
- **Edge cases:**
  - All recent_messages are pinned → return ContextOverflow immediately
  - Dropping one message brings token count from 1001 to 999 (below 1000 limit) → stop
  - Pinned messages include the user task AND any messages with `pinned: true`
- **Definition of Done:** Overflow rules enforced, `cargo test` passes
- **Acceptance criteria:**
  - Over budget after compress → non-pinned messages dropped
  - Only pinned remain, still over budget → `Err(AgentError::ContextOverflow)`
  - Pinned messages never dropped
- **Test plan:** unit: Test with tokenizer returning high counts; test pinned preservation; test ContextOverflow trigger
- **Estimated effort:** 5 hours

---

### TASK-6.7: Memory integration tests with MockSummarizer

- **§SPEC:** §21–23 (full memory subsystem)
- **Labels:** `layer/memory`, `priority/high`
- **Description:** Create integration tests exercising the full memory lifecycle: push messages → over_budget triggers compress → summary created → more messages → compress again (existing summary as context) → overflow drops messages → ContextOverflow when pinned exceeds budget. Use `MockSummarizer` and `MockTokenizer` for deterministic testing.
- **Files affected:**
  - `crates/duga-core/tests/memory_integration.rs` (new)
- **Types involved:** `Memory`, `MockSummarizer`, `MockTokenizer`, `Message`
- **Dependencies:** TASK-6.6
- **Implementation steps:**
  1. Create `MockTokenizer` that returns `messages.len() * 10` as token count (simple linear model)
  2. Create `MockSummarizer` that returns "summary of {n} messages"
  3. Test full lifecycle
  4. Test double compression (summary-as-input scenario)
  5. Test overflow with pinned messages
- **Acceptance criteria:** Full lifecycle test passes
- **Test plan:** integration tests as described
- **Estimated effort:** 4 hours
