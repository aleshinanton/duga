# EPIC-25: Semantic Summarization

**Labels:** `epic/context-management`  
**Crates:** `duga-core`, `duga-runtime`  
**Depends on:** EPIC-6 (Memory System), EPIC-8 (LLM Layer), EPIC-23 (Task Anchoring)

## Goal

Replace the current `RuntimeSummarizer` (which produces useless role labels) with an LLM-driven summarizer that preserves topic identity, key findings, decisions, and unresolved questions. This prevents the LLM from losing track of what happened in compressed conversations.

## Motivation

Current summarizer (`duga-runtime/src/agent.rs`, lines 132–142):

```rust
impl Summarizer for RuntimeSummarizer {
    fn summarize(&self, messages: &[Message]) -> SummaryFuture {
        Box::pin(async move {
            let mut content = String::from("Compressed context:");
            for message in messages.iter().take(16) {
                content.push_str("\n- ");
                content.push_str(&message.role.to_string());  // ← JUST ROLE LABELS
            }
            Ok(SummaryMessage::new(content))
        })
    }
}
```

Output: `"Compressed context:\n- user\n- assistant\n- tool\n- assistant\n- user\n..."`

This carries **zero semantic information**. After compression, the LLM has:
- The current task anchor (once EPIC-23 is done)
- A meaningless list of role labels
- The most recent few messages

With no semantic summary of older context, the LLM's attention can drift to whatever dominated the recent unfiltered messages (e.g. Codex-CLI instead of Portuguese law).

An LLM-based summarizer produces:
```
Summary:
- User asked about Portuguese citizenship law (Lei 37/81)
- Key findings: Article 6 covers naturalization requirements, 5-year residency needed
- DRE consolidated version fetched but not fully parsed
- Unresolved: exact residency period requirements for different cases
```

This anchors the topic identity even after compression.

## Implementation Rules

1. **Be careful with changes.** The summarizer runs inside the agent loop's hot path (`compress_if_needed()`). A slow or failing summarizer blocks the entire agent. Always include a timeout (15s) and a fallback to the role-label summary on failure. The summarizer must never call the LLM with tools enabled — use a bare `chat()` call with `tools: []`.
2. **Keep it simple.** The `SemanticSummarizer` is a single struct with one method. No async trait objects beyond the existing `Summarizer` trait. The prompt template is a `const &str`. The existing summary is passed via the `compression_input()` method already built into `Memory` — reuse it, don't rewrite it.
3. **Implement step by step.** Build `SemanticSummarizer` as a standalone module (TASK-25.1). Test it with a `DummyClient` that records the prompt. Wire it into `build_agent()` (TASK-25.2) behind a config flag (TASK-25.4). Add caching (TASK-25.3) last, since it's an optimization, not a correctness requirement. At each step, verify with `cargo test` and a manual run against real session data.
4. **Validate and write tests.** Unit-test `format_messages_for_summarization()` with known message sequences. Integration-test the summarizer with `DummyClient` to verify prompt format and content. Test the fallback path: inject an LLM error, verify the role-label summary is returned instead. Test that the old `RuntimeSummarizer` still works when `summarizer: "simple"` is set.
5. **Avoid changing core functionality.** The `Summarizer` trait signature does not change. `Memory::compress()` is not modified. The existing `RuntimeSummarizer` is preserved (not deleted) and remains available via config. The semantic summarizer is a drop-in replacement implementing the same trait. No changes to `AgentLoop`, `Memory`, or any other core struct.

---

### TASK-25.1: LLM-based summarizer struct

- **Labels:** `layer/agent-loop`, `priority/critical`
- **Description:** Create a new `SemanticSummarizer` struct in `duga-runtime` that implements the `Summarizer` trait. Instead of building role labels, it sends the messages to the LLM with a summarization prompt.

```rust
pub struct SemanticSummarizer {
    llm: Arc<dyn LlmClient>,
}

impl Summarizer for SemanticSummarizer {
    fn summarize<'a>(&'a self, messages: &'a [Message]) -> SummaryFuture<'a> {
        // 1. Build a summarization prompt with the messages
        // 2. Call self.llm.chat() with no tools, low temperature
        // 3. Return the LLM's response as SummaryMessage
    }
}
```

**Summarization prompt template:**

```
Summarize the following conversation history in 3-5 bullet points.
Preserve: main topics discussed, key findings, decisions made, and unresolved questions.
Be concise. Output only the bullet points, no preamble.

{formatted_messages}
```

Where `{formatted_messages}` is each message concatenated with role prefix:
```
[user]: search for Portuguese citizenship law
[assistant]: I'll fetch the Lei da Nacionalidade...
[tool: shell]: Fetching from pgdlisboa.pt...
[assistant]: Found Article 6...
```

- **Files affected:**
  - `crates/duga-runtime/src/summarizer.rs` (new file)
  - `crates/duga-runtime/src/lib.rs` — re-export `SemanticSummarizer`

- **Types involved:** `SemanticSummarizer`, `Summarizer`, `LlmClient`, `SummaryMessage`
- **Functions to implement:**
  - `SemanticSummarizer::new(llm: Arc<dyn LlmClient>) -> Self`
  - `impl Summarizer for SemanticSummarizer`
  - `fn format_messages_for_summarization(messages: &[Message]) -> String`
- **Dependencies:** EPIC-8 (LLM Layer — `LlmClient::chat()`)
- **Implementation steps:**
  1. Create `summarizer.rs` in `duga-runtime/src/`
  2. Implement `format_messages_for_summarization()`:
     - Iterate messages, extract text content
     - Prefix each with role: `[user]:`, `[assistant]:`, `[tool: {name}]:`
     - Truncate very long messages (>500 chars) with `...(truncated)`
     - Skip messages with no text content
  3. Implement `SemanticSummarizer::summarize()`:
     - Build system prompt: "You are a conversation summarizer. Output only bullet points."
     - Build user message: the summarization prompt template with formatted messages
     - Call `self.llm.chat()` with temperature=0.3, no tools, no streaming
     - Extract text from response
     - Return `SummaryMessage::new(text)`
  4. Add timeout: if the LLM call takes >15s, fall back to a trivial summary (list of user messages)
- **Edge cases:**
  - LLM returns empty response → fall back to role-label summary
  - LLM returns very long summary (>2000 chars) → truncate
  - LLM errors (rate limit, timeout) → fall back to trivial summary
  - No messages to summarize → return empty `SummaryMessage`
- **Definition of Done:** `SemanticSummarizer` compiles; integration test produces meaningful summaries.
- **Acceptance criteria:**
  - Summary contains actual topics from the conversation (not role labels)
  - Summary is 3-5 bullet points
  - Summary includes key findings and decisions
  - Fallback works on LLM error
- **Test plan:**
  - unit: `format_messages_for_summarization()` produces correct formatting
  - unit: fallback on empty list returns empty summary
  - integration: run with `DummyClient`, verify prompt format sent to LLM
  - manual: run with real LLM, inspect summary content
- **Estimated effort:** 4 hours

---

### TASK-25.2: Wire SemanticSummarizer into build_agent

- **Labels:** `layer/agent-loop`, `priority/critical`
- **Description:** Replace the old `RuntimeSummarizer` with `SemanticSummarizer` in `build_agent()`. The `SemanticSummarizer` needs an `Arc<dyn LlmClient>`, which is already available at the call site.

- **Files affected:**
  - `crates/duga-runtime/src/agent.rs` — swap summarizer

- **Types involved:** `build_agent`, `SemanticSummarizer`
- **Functions to modify:**
  - `build_agent()` — replace `Arc::new(RuntimeSummarizer)` with `Arc::new(SemanticSummarizer::new(llm.clone()))`
- **Dependencies:** TASK-25.1
- **Implementation steps:**
  1. Remove `RuntimeSummarizer` struct (or gate behind a feature flag)
  2. Import `SemanticSummarizer`
  3. Create it passing `llm.clone()`
  4. Pass to `AgentLoop::new()` as before
- **Edge cases:**
  - The `SemanticSummarizer` circularly calls the same LLM that's being summarized. This is intentional and safe because summarization is a separate chat call with no tools and a different system prompt.
- **Definition of Done:** `cargo test` passes; agent uses semantic summarizer.
- **Acceptance criteria:**
  - `Memory::compress()` calls `SemanticSummarizer::summarize()` instead of `RuntimeSummarizer::summarize()`
  - Compression produces meaningful text, not role labels
- **Test plan:**
  - unit: verify `build_agent()` creates `SemanticSummarizer`
  - integration: agent test with compression, inspect summary event
- **Estimated effort:** 1 hour

---

### TASK-25.3: Summary caching (avoid re-summarizing every turn)

- **Labels:** `layer/memory`, `priority/medium`
- **Description:** The `SemanticSummarizer` is called every time the memory exceeds the token budget. If the agent compresses early in a run (e.g. step 5 of 21), subsequent compressions should reuse the existing summary plus append newly-compressed messages, rather than re-summarizing from scratch.

**Implementation:**
- The `Memory` struct already stores `summary: Option<SummaryMessage>`.
- When `compress()` is called again, it includes the existing summary in the prompt as prior context.
- The summarizer prompt becomes:

```
Previous summary:
{existing_summary}

New messages to incorporate:
{formatted_messages}

Update the summary to include the new messages. Keep 3-5 bullet points.
Preserve: main topics, findings, decisions, unresolved questions.
Be concise. Output only the bullet points, no preamble.
```

- **Files affected:**
  - `crates/duga-core/src/memory.rs` — pass existing summary to summarizer
  - `crates/duga-runtime/src/summarizer.rs` — handle existing summary in prompt

- **Types involved:** `Memory`, `SemanticSummarizer`
- **Functions to modify:**
  - `Memory::compress()` — already passes `self.summary` via `compression_input()`
  - `SemanticSummarizer::summarize()` — detect existing summary in input messages
- **Dependencies:** TASK-25.1
- **Implementation steps:**
  1. The `Memory::compress()` already builds `compression_input()` which includes the existing summary. Verify this works correctly.
  2. In `SemanticSummarizer::summarize()`, detect if any input message is a summary (check for "Summary:" prefix or role=system with summary content).
  3. If existing summary found, use the "Previous summary + new messages" prompt template.
  4. If no existing summary, use the original "Summarize from scratch" template.
  5. Ensure the LLM's response replaces (not duplicates) the existing summary.
- **Edge cases:**
  - First compression: no existing summary → use from-scratch template
  - Second compression: existing summary present → use update template
  - Summary grows too large (>1000 chars) → truncate the existing summary portion
- **Definition of Done:** Second compression reuses first compression's summary.
- **Acceptance criteria:**
  - Compression 1 produces summary S1
  - Compression 2 produces summary S2 that references S1's content
  - S2 is not just a repetition of S1
- **Test plan:**
  - unit: mock LLM that records prompt, verify second call includes first summary
  - integration: run agent through 2 compressions, compare summaries
- **Estimated effort:** 2 hours

---

### TASK-25.4: Config flag for summarizer type

- **Labels:** `layer/config`, `priority/low`
- **Description:** Add a config flag to choose between the semantic summarizer and the old role-label summarizer. This allows fallback if the semantic summarizer has issues in production.

```yaml
memory:
  max_tokens: 65536
  compress_at_ratio: 0.8
  summarizer: "semantic"  # "semantic" or "simple" (role-label)
```

- **Files affected:**
  - `crates/duga-config/src/config.rs` — add `summarizer` field
  - `crates/duga-runtime/src/agent.rs` — dispatch on config
  - `duga-deepseek.yaml` — example

- **Types involved:** `MemoryConfig`
- **Dependencies:** TASK-25.2
- **Implementation steps:**
  1. Add `SummarizerKind` enum: `Simple`, `Semantic`
  2. Add `#[serde(default)] summarizer: SummarizerKind` to `MemoryConfig`
  3. Default to `Semantic`
  4. In `build_agent()`, create the appropriate summarizer based on config
  5. Update YAML fixtures
- **Edge cases:**
  - Unknown summarizer value → validation error
  - Missing field → default to Semantic
- **Definition of Done:** Config switchable summarizer.
- **Acceptance criteria:**
  - `summarizer: "simple"` → uses `RuntimeSummarizer`
  - `summarizer: "semantic"` → uses `SemanticSummarizer`
  - Missing field → defaults to `semantic`
- **Test plan:**
  - unit: deserialize config with both values
  - unit: verify correct summarizer created
- **Estimated effort:** 1.5 hours

---

## Epic Summary

| Task | Name | Est. Hours |
|------|------|------------|
| TASK-25.1 | LLM-based summarizer struct | 4 |
| TASK-25.2 | Wire SemanticSummarizer into build_agent | 1 |
| TASK-25.3 | Summary caching | 2 |
| TASK-25.4 | Config flag for summarizer type | 1.5 |
| **Total** | | **8.5 hours** |

## Files Summary

| File | Change |
|------|--------|
| `crates/duga-runtime/src/summarizer.rs` | **NEW** — `SemanticSummarizer` implementation |
| `crates/duga-runtime/src/lib.rs` | Re-export summarizer |
| `crates/duga-runtime/src/agent.rs` | Swap `RuntimeSummarizer` → `SemanticSummarizer`; dispatch on config |
| `crates/duga-config/src/config.rs` | Add `SummarizerKind`, `summarizer` field on `MemoryConfig` |
| `duga-deepseek.yaml` | Example config |

---

## Cross-Epic Integration

The three epics (23, 24, 25) compose into the final context assembly order:

```
┌─────────────────────────────────────────────┐
│  [Task Anchoring Prefix]       (system)     │  ← EPIC-23
│  "CURRENT TASK: {task}..."                  │
├─────────────────────────────────────────────┤
│  [Semantic Summary of old msgs] (system)    │  ← EPIC-25
│  "Summary: - User asked about..."           │     (if compressed)
├─────────────────────────────────────────────┤
│  [Last N messages]    (user/assistant/tool) │  ← EPIC-24
│  (sliding window, max N messages)           │
├─────────────────────────────────────────────┤
│  [Current user message + suffix]  (user)    │  ← EPIC-23
│  "...\nReminder: Focus on current task"     │
└─────────────────────────────────────────────┘
```

**Interaction notes:**
- The task anchor (EPIC-23) is pinned and never included in summarization input (EPIC-25) or sliding window eviction (EPIC-24).
- The semantic summary (EPIC-25) replaces the role-label summary. It is treated as a system message and sits between the task anchor and the sliding window messages.
- The sliding window (EPIC-24) restricts how many recent messages are loaded. Messages outside the window are candidates for semantic summarization (EPIC-25) rather than being silently dropped.
- Compaction (EPIC-25) runs when the total context exceeds the token budget. It summarizes oldest messages into the summary block, then the sliding window (EPIC-24) is applied to the remaining recent messages.

**Combined estimated effort:** 23 hours
