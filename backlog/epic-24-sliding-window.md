# EPIC-24: Sliding Window & Budget Enforcement

**Labels:** `epic/context-management`  
**Crates:** `duga-config`, `duga-core`, `duga-telegram-bot`  
**Depends on:** EPIC-6 (Memory System), EPIC-23 (Task Anchoring)

## Goal

Replace the current "load ALL previous messages" approach with a configurable, token-aware sliding window. Only the most recent N messages are loaded into context, with a hard token budget that aggressively drops oldest messages to fit.

## Motivation

Current behavior (`load_conversation_history` in `runtime.rs`):
1. Opens `session.jsonl`
2. Finds the LAST `LlmRequest` event
3. Extracts **ALL** messages from that request
4. Filters out only system messages
5. Feeds 518+ messages back into the agent

This loads irrelevant history from hours/days prior, saturating the context window with old topics. The LLM then drifts away from the current task.

A sliding window with a hard budget:
- Limits messages to the most recent N (default 50)
- Enforces a token cap (default 12,000)
- Drops old messages within the window if they exceed the budget

## Implementation Rules

1. **Be careful with changes.** `load_conversation_history()` is the sole entry point for restoring chat context. Any bug here results in amnesia or wrong answers. Test with both large (518 messages) and empty histories before merging.
2. **Keep it simple.** The sliding window is a single `truncate()` call on a `Vec`. The token estimate is a simple `len() * 256` heuristic — no HTTP round-trips, no precision requirements. Backward compatibility: `0` means "disable, load all".
3. **Implement step by step.** Start with config fields (TASK-24.1) — they're pure additions with no runtime impact. Then add window logic to `load_conversation_history()` (TASK-24.2) and test against real session data. Add the token estimator helper (TASK-24.3) as a standalone utility. Finally, add Memory-level enforcement (TASK-24.4) for CLI/TUI coverage.
4. **Validate and write tests.** Test with `context_window_size: 10` on 518 messages → 10 loaded. Test with `max_context_tokens: 0` → loads all (backward compatible). Test with `context_window_size: 0, max_context_tokens: 0` → no filtering (current behavior preserved). Add a unit test for `estimate_tokens()` with known inputs.
5. **Avoid changing core functionality.** The window is applied as a post-processing step on already-loaded messages. The `normalize_tool_message_sequence()` call still runs after window truncation. The `restore_history()` method signature does not change. Config defaults (`0`) preserve exact current behavior.

---

### TASK-24.1: Config fields for context window

- **Labels:** `layer/config`, `priority/critical`
- **Description:** Add two new fields to `MemoryConfig` (or a new `ContextConfig`):

```yaml
memory:
  max_tokens: 65536
  compress_at_ratio: 0.8
  context_window_size: 50        # NEW: max recent messages to load
  max_context_tokens: 12000      # NEW: hard token budget for loaded history
```

- `context_window_size` (usize): maximum number of recent messages to load from the session file. Default: `50`. Set to `0` to disable (load all, current behavior).
- `max_context_tokens` (usize): hard token budget for loaded history. If the window exceeds this, drop oldest messages until it fits. Default: `12000`. Set to `0` to disable.

- **Files affected:**
  - `crates/duga-config/src/config.rs` — add fields to `MemoryConfig`
  - `crates/duga-types/src/config.rs` — add `ContextConfig` or extend `MemoryConfig` (decide which)
  - `duga-deepseek.yaml` — add example values

- **Types involved:** `MemoryConfig`
- **Dependencies:** None
- **Implementation steps:**
  1. Add `context_window_size: usize` and `max_context_tokens: usize` to `MemoryConfig`
  2. Add `#[serde(default = "default_context_window_size")]` and `#[serde(default = "default_max_context_tokens")]`
  3. Default functions: `context_window_size` = 50, `max_context_tokens` = 12000
  4. Update `duga-deepseek.yaml` with comments showing the new fields
  5. Add validation: `context_window_size` must be >= 0, `max_context_tokens` must be >= 0
  6. Update all test YAML fixtures that construct `MemoryConfig`
- **Edge cases:**
  - `context_window_size: 0` → disable window, load all (backward compatible)
  - `max_context_tokens: 0` → disable budget, load all
  - `context_window_size: 0` and `max_context_tokens: 0` → current behavior preserved
  - Both set: window size is the primary limiter; budget is applied to the window
- **Definition of Done:** Config parses with new fields; existing tests pass; new defaults are sensible.
- **Acceptance criteria:**
  - `MemoryConfig::default().context_window_size == 50`
  - `MemoryConfig::default().max_context_tokens == 12000`
  - YAML with explicit values parses correctly
  - YAML without new fields falls back to defaults
- **Test plan:**
  - unit: deserialize `MemoryConfig` with and without new fields
  - unit: validate rejects negative values
  - integration: config file round-trip
- **Estimated effort:** 2 hours

---

### TASK-24.2: Sliding window in load_conversation_history

- **Labels:** `layer/telegram`, `priority/critical`
- **Description:** Modify `load_conversation_history()` in `runtime.rs` to apply the sliding window and token budget from `MemoryConfig`.

Current logic:
```rust
// Loads ALL messages from the last LlmRequest
let messages = last_messages?;  // could be 518 messages
```

New logic:
```rust
// 1. Load ALL messages from the last LlmRequest
// 2. Apply sliding window: keep only the last N messages
// 3. Apply token budget: count tokens, drop from front until under budget
// 4. Normalize (existing orphan tool cleanup)
```

Token counting: use a fixed-per-message estimate (e.g. 256 tokens per message) rather than calling the LLM tokenizer (which requires an HTTP round-trip to DeepSeek). This is a heuristic; precision is not required.

- **Files affected:**
  - `crates/duga-telegram-bot/src/runtime.rs` — modify `load_conversation_history()`

- **Types involved:** `Message`, `MemoryConfig`
- **Functions to modify:**
  - `load_conversation_history(path, config: &MemoryConfig) -> Option<Vec<Message>>`
- **Dependencies:** TASK-24.1
- **Implementation steps:**
  1. Add `config: &MemoryConfig` parameter to `load_conversation_history()`
  2. Update caller in `run_task_for_chat()` to pass `&self.config.memory`
  3. After loading messages from `LlmRequest`, if `config.context_window_size > 0`:
     - Keep only the last `context_window_size` messages (truncate from the front)
  4. After window truncation, if `config.max_context_tokens > 0`:
     - Estimate tokens: `messages.len() * 256`
     - While estimated tokens > `max_context_tokens`, remove from the front of the window
     - Minimum 1 message retained (never remove the last one)
  5. Run the existing `normalize_tool_message_sequence()` on the result
  6. Log: `"Loaded {} messages after window ({} total in history)"`
- **Edge cases:**
  - Window size larger than available messages → keep all (no-op)
  - Token budget drops window to 1 message → acceptable, the user task is appended separately
  - History is empty → return `None` as before
  - History contains only system messages → window may produce empty result → return `None`
- **Definition of Done:** Telegram bot loads at most N messages from history.
- **Acceptance criteria:**
  - With `context_window_size: 10` and 518 messages in history, only 10 are loaded
  - With `max_context_tokens: 2000` (est. 8 messages), at most 8 are loaded
  - Logging indicates pre/post-window message counts
- **Test plan:**
  - unit: test with small window (5) on mocked history
  - unit: test with tight budget on larger history
  - unit: test with zero window (load all) → backward compatible
  - integration: telegram bot run, verify log shows correct message count
- **Estimated effort:** 3 hours

---

### TASK-24.3: Token estimation helper for budget enforcement

- **Labels:** `layer/memory`, `priority/medium`
- **Description:** Add a lightweight `estimate_tokens(messages: &[Message]) -> usize` function to `duga-core/src/memory.rs`. This provides a fast, offline estimate (no LLM API call needed) for the sliding window budget enforcement.

**Heuristic:** `messages.len() * 256` — each message averages ~256 tokens (roughly 200 words). This is conservative enough for budget enforcement without the overhead of calling the tokenizer.

Optionally, count characters and divide by 4 (rule of thumb: 1 token ≈ 4 chars for English text, ~2 chars for code).

- **Files affected:**
  - `crates/duga-core/src/memory.rs` — add `estimate_tokens()` helper

- **Types involved:** `Message`
- **Functions to implement:**
  - `pub fn estimate_tokens(messages: &[Message]) -> usize`
- **Dependencies:** TASK-24.1
- **Implementation steps:**
  1. Implement character-based estimate: sum length of all text content blocks, divide by 4
  2. Fall back to `messages.len() * 256` for messages without text blocks (e.g. image-only)
  3. Add unit tests with known message sizes
- **Edge cases:**
  - Empty message list → 0
  - Tool call messages (no text content) → count as 64 tokens each (arbitrary but sensible)
  - Very long messages: character estimate is more accurate; cap at some reasonable per-message max
- **Definition of Done:** `cargo test` passes; estimate is within 2x of actual token count.
- **Acceptance criteria:**
  - `estimate_tokens(&[]) == 0`
  - `estimate_tokens(&[Message::user("hello")])` ≈ 2 (5 chars / 4 ≈ 1.25, rounded)
  - `estimate_tokens(&vec![Message::user(""); 100])` ≈ 100 * 64 = 6400 (empty message fallback)
- **Test plan:**
  - unit: character-based estimate for known text
  - unit: empty message list
  - unit: mixed text + tool messages
- **Estimated effort:** 1.5 hours

---

### TASK-24.4: Memory-level window enforcement

- **Labels:** `layer/memory`, `priority/medium`
- **Description:** In addition to the Telegram-level window (TASK-24.2), enforce the sliding window at the `Memory` level so the CLI/TUI frontends also benefit. When `restore_history()` is called, apply the same window logic.

- **Files affected:**
  - `crates/duga-core/src/memory.rs` — modify `push_msg` or add `restore_with_window()`

- **Types involved:** `Memory`
- **Functions to modify:**
  - `Memory::restore_history()` or add `Memory::enforce_window(&mut self, window_size: usize, max_tokens: usize)`
- **Dependencies:** TASK-24.3
- **Implementation steps:**
  1. In `Memory`, track the window config: `context_window_size` and `max_context_tokens`
  2. After `restore_history()` pushes all messages, call `enforce_window()`:
     - If window_size > 0, keep only the last `window_size` messages in `recent_messages`
     - If max_tokens > 0, use `estimate_tokens()` to drop from front until under budget
     - Never remove the pinned task message
  3. Log before/after message counts
- **Edge cases:**
  - Window enforcement must not remove the pinned task message
  - After enforcement, if only the pinned task remains, that's acceptable
  - Window enforcement runs once per `restore_history()`, not every turn
- **Definition of Done:** `Memory` respects window config from `MemoryConfig`.
- **Acceptance criteria:**
  - 200 messages loaded, `window_size: 20` → 20 messages in memory
  - Pinned task survives window enforcement
- **Test plan:**
  - unit: push 100 messages, enforce window 20, verify 20 remain
  - unit: verify pinned task in remaining messages
- **Estimated effort:** 2 hours

---

## Epic Summary

| Task | Name | Est. Hours |
|------|------|------------|
| TASK-24.1 | Config fields for context window | 2 |
| TASK-24.2 | Sliding window in load_conversation_history | 3 |
| TASK-24.3 | Token estimation helper | 1.5 |
| TASK-24.4 | Memory-level window enforcement | 2 |
| **Total** | | **8.5 hours** |

## Files Summary

| File | Change |
|------|--------|
| `crates/duga-config/src/config.rs` | Add `context_window_size`, `max_context_tokens` to `MemoryConfig` |
| `crates/duga-types/src/config.rs` | (Optional) `ContextConfig` struct |
| `crates/duga-core/src/memory.rs` | `estimate_tokens()`, `enforce_window()` |
| `crates/duga-telegram-bot/src/runtime.rs` | Window + budget in `load_conversation_history()` |
| `duga-deepseek.yaml` | Example config entries |
