# EPIC-23: Task Anchoring Prefix

**Labels:** `epic/context-management`  
**Crates:** `duga-core`, `duga-runtime`, `duga-telegram-bot`  
**Depends on:** EPIC-6 (Memory System), EPIC-10 (Core ReAct Loop)

## Goal

Prevent the LLM from answering a previous question when a new task arrives.  
Insert a persistent **task anchoring prefix** at the front of every LLM request and a **reminder suffix** on the user message so the model cannot drift to older topics in a saturated context window.

## Motivation

In production (deepseek-v4-flash, Telegram bot):
- The bot loads **518 prior messages** spanning Codex-CLI, Vinted skills, Mac search, eBay, etc.
- When the user asks a new question (e.g. Portuguese citizenship law), the LLM correctly starts working on it.
- After ~21 steps, when the context compresses via the trivial role-label summarizer, the LLM reverts to the dominant topic in the remaining context (Codex-CLI) and produces the wrong final answer.

A **task anchoring prefix** is a low-cost, high-impact fix that keeps the LLM focused on the current task regardless of context noise.

## Implementation Rules

1. **Be careful with changes.** The agent loop and memory system are core runtime paths — every modification must be surgically precise. Review each line of the diff before committing.
2. **Keep it simple.** Prefer minimal, localized edits. A single added `Option<Message>` field is better than a new struct. An `if task_anchor.is_some()` guard is better than a trait abstraction.
3. **Implement step by step.** Complete TASK-23.1 (anchor prefix) first. Validate it works end-to-end. Then add TASK-23.2 (reminder suffix). Then TASK-23.3 (Telegram integration). Never batch untested changes.
4. **Validate and write tests.** Every new code path must have a unit test. Critical paths (anchor not evicted, anchor not in compression input, anchor at position 0) need explicit assertions. Run `cargo test` after each task.
5. **Avoid changing core functionality.** The existing `Memory::compress()`, `Memory::drop_until_budget()`, and `Memory::messages()` logic must continue to work for non-anchored paths. All changes are additive — new fields, new guards, new prepend logic. Nothing is removed or restructured.

---

### TASK-23.1: Task anchoring prefix in AgentLoop

- **Labels:** `layer/agent-loop`, `priority/critical`
- **Description:** In `AgentLoop::run()`, before building the messages array for `LlmRequest`, prepend a `system`-role message containing the current task:

```
CURRENT TASK: {task}
All prior context is background only. Focus exclusively on the current task.
```

This message must be inserted at position 0 of the message list **every turn**, not just at the start of the run. It must be treated as a **pinned** system message so it survives compression and is never evicted.

- **Files affected:**
  - `crates/duga-core/src/agent_loop.rs` — insert prefix in the loop iteration
  - `crates/duga-core/src/memory.rs` — add `task_anchor` field, ensure it's never compressed/evicted
  - `crates/duga-types/src/message.rs` — may need `Message::task_anchor(text)` constructor

- **Types involved:** `AgentLoop`, `Memory`, `Message`
- **Functions to modify:**
  - `AgentLoop::run()` — build anchored message list each iteration
  - `Memory::new()` — accept optional task anchor
  - `Memory::messages()` — always prepend the task anchor to the output
  - `Memory::compress()` — never include the task anchor in summarization input
  - `Memory::drop_until_budget()` — never evict the task anchor
- **Dependencies:** TASK-6.1 (Memory struct), TASK-6.3 (messages ordering)
- **Implementation steps:**
  1. Add `task_anchor: Option<Message>` to `Memory` struct
  2. In `AgentLoop::run()`, after `self.memory.push_user(task)`, also set `self.memory.task_anchor = Some(Message::system(format!("CURRENT TASK: {task}\nAll prior context is background only. Focus exclusively on the current task.")))`
  3. In `Memory::messages()`, prepend the task anchor before system_messages
  4. In `Memory::compress()`, exclude the task anchor from the `oldest` slice passed to the summarizer
  5. In `Memory::drop_until_budget()`, skip the task anchor when looking for non-pinned messages to evict
  6. Pin the task anchor message (`message.pinned = true`)
- **Edge cases:**
  - Agent restarted mid-run (crash recovery): the task anchor is lost. On next run, the new task creates a fresh anchor. Acceptable.
  - Empty task: skip the anchor entirely (no-op).
  - Task contains newlines or special characters: use raw string, no escaping needed.
- **Definition of Done:** `cargo test` passes; manual test with telegram bot shows correct answer after topic switch.
- **Acceptance criteria:**
  - `messages()[0]` is always the task anchor (when task is set)
  - Task anchor never appears in compression input
  - Task anchor never gets evicted even when `hard_over_budget` fires
  - After compression, the task anchor is still at position 0
- **Test plan:**
  - unit: verify `messages()` output order with anchor set
  - unit: verify anchor is excluded from compression input (`RecordingSummarizer` check)
  - unit: verify anchor survives `drop_until_budget` when budget is tiny
  - integration: run two different tasks back-to-back, verify the second answer matches the second task, not the first
- **Estimated effort:** 4 hours

---

### TASK-23.2: Reminder suffix on user message

- **Labels:** `layer/agent-loop`, `priority/high`
- **Description:** Append a reminder suffix to the user's task message before pushing it to memory:

```
\n\nReminder: Focus exclusively on the current task: {task}
```

This is a belt-and-suspenders measure: even if the prefix is somehow missed (e.g. by a provider that ignores system messages), the user message itself carries the anchoring instruction.

- **Files affected:**
  - `crates/duga-core/src/agent_loop.rs` — append suffix to task before `push_user`
  - `crates/duga-telegram-bot/src/runtime.rs` — optionally apply at Telegram layer

- **Types involved:** `AgentLoop`
- **Functions to modify:**
  - `AgentLoop::run()` — modify `task` before `push_user`
- **Dependencies:** TASK-23.1
- **Implementation steps:**
  1. In `AgentLoop::run()`, create `anchored_task = format!("{task}\n\nReminder: Focus exclusively on the current task: {task}")`
  2. Pass `anchored_task` to `self.memory.push_user()` instead of the raw task
  3. The task anchor (TASK-23.1) still uses the original `task` string for its content
- **Edge cases:**
  - Very long task (>1000 chars): the suffix is short enough that it won't meaningfully affect token budget
  - Task already contains "Reminder": no dedup needed, repetition is harmless
- **Definition of Done:** The user message stored in memory ends with the reminder suffix.
- **Acceptance criteria:**
  - `recent_messages[0].content` contains the reminder suffix when the task is the first user message
  - The reminder references the exact same task as the anchor prefix
- **Test plan:**
  - unit: inspect `memory.recent_messages()[0]` after `run()`, verify suffix presence
  - unit: verify suffix uses correct task text (not truncated or modified)
- **Estimated effort:** 1 hour

---

### TASK-23.3: Task anchor in Telegram runtime

- **Labels:** `layer/telegram`, `priority/medium`
- **Description:** The Telegram runtime constructs its own system prompt in `run_task_for_chat()`. Ensure the task anchoring prefix is **also** included in the Telegram system prompt when `restore_history()` is used. Since the Telegram runtime already provides a custom system prompt, we integrate the anchor there rather than duplicating it.

- **Files affected:**
  - `crates/duga-telegram-bot/src/runtime.rs` — add task anchor to system prompt

- **Types involved:** `TelegramRuntime`
- **Functions to modify:**
  - `TelegramRuntime::run_task_for_chat()` — prepend task anchor to the system prompt
- **Dependencies:** TASK-23.1
- **Implementation steps:**
  1. Before building the system prompt, create an anchor string: `format!("CURRENT TASK: {task}\nAll prior context is background only. Focus exclusively on the current task.\n\n")`
  2. Prepend this to the existing system prompt
  3. The anchor appears first in the combined prompt, before environment context, tool guidance, etc.
- **Edge cases:**
  - CLI / TUI frontends: these use `build_agent()` directly which goes through `AgentLoop::run()`, so they get the anchor from TASK-23.1 automatically.
- **Definition of Done:** The Telegram system prompt starts with the task anchor.
- **Acceptance criteria:**
  - `system_prompt.starts_with("CURRENT TASK:")` after construction
  - Anchor text matches the actual task, not a stale previous task
- **Test plan:**
  - unit: verify system prompt contains task anchor
  - manual: telegram bot run with topic switch, verify correct answer
- **Estimated effort:** 1 hour

---

## Epic Summary

| Task | Name | Est. Hours |
|------|------|------------|
| TASK-23.1 | Task anchoring prefix in AgentLoop | 4 |
| TASK-23.2 | Reminder suffix on user message | 1 |
| TASK-23.3 | Task anchor in Telegram runtime | 1 |
| **Total** | | **6 hours** |

## Files Summary

| File | Change |
|------|--------|
| `crates/duga-core/src/memory.rs` | Add `task_anchor` field; protect from compression/eviction |
| `crates/duga-core/src/agent_loop.rs` | Set anchor at run start; append suffix to user message |
| `crates/duga-telegram-bot/src/runtime.rs` | Prepend anchor to system prompt |
| `crates/duga-core/tests/memory_integration.rs` | Tests for anchor persistence |
