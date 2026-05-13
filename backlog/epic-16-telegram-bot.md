# EPIC-16: Telegram Bot Frontend

**SPEC:** §5, §24, §26, §32
**Labels:** `epic/telegram`
**Crates:** `duga-telegram-bot` (new)

## Goal

Add a Telegram bot frontend that reuses the shared frontend runtime from EPIC-18. Telegram owns transport-specific concerns only: chat authorization, command parsing, per-chat sessions, Telegram rendering, inline callbacks, scheduled Telegram events, attachments, and bot message logs.

Shared provider resolution, tool registration, MEMORY.md loading, SKILL.md loading, non-blocking event bridges, confirmation middleware, typed frontend config, and Docker sandbox execution are implemented in EPIC-18 and reused by CLI/TUI/Telegram.

---

### TASK-16.1: Telegram config struct + validation

- **SPEC:** §32 (config loading), §20 (secrets), §26 (cancellation)
- **Labels:** `layer/telegram`, `layer/cli`, `priority/critical`
- **Description:** Add Telegram-specific config under `telegram:`. Shared fields such as `provider`, `model`, `thinking_level`, `context_window`, `sandbox.mode`, and progress defaults come from EPIC-18 typed config.
- **Files affected:**
  - `crates/duga-config/src/config.rs`
- **Types involved:** `TelegramConfig`, `TelegramAttachmentConfig`
- **Proposed YAML:**
  ```yaml
  telegram:
    token_env: "TELEGRAM_BOT_TOKEN"
    allowed_chat_ids:
      - 123456789
    send_tool_events: true
    send_final_only: false
    require_confirmation_for:
      - bash
      - write
    events_dir: "./events"
    skills_dir: "./skills"
    data_dir: "./data"
    attachments:
      enabled: true
      max_file_size_mb: 20
      download_dir: "attachments"
  ```
- **Dependencies:** TASK-18.2, TASK-12.1, TASK-12.3
- **Implementation steps:**
  1. Add `telegram: Option<TelegramConfig>` to `Config`.
  2. Validate `token_env` is non-empty when Telegram is configured.
  3. Validate `allowed_chat_ids` is non-empty unless `allow_all_chats_for_dev` is true.
  4. Validate `events_dir` and `data_dir` are relative to the configured workspace or explicit absolute paths.
  5. Validate `attachments.max_file_size_mb` is > 0 and <= Telegram's current file limit.
  6. Do not warn just because `token_env` looks secret-like; instead ensure the resolved bot token is never forwarded to subprocess environment unless explicitly allowlisted.
- **Definition of Done:** Telegram config parses and validation catches unsafe defaults.
- **Acceptance criteria:**
  - Missing token env name -> validation error.
  - Empty allowlist in production mode -> validation error.
  - Invalid `max_file_size_mb` -> validation error.
- **Test plan:** config unit tests with valid and invalid Telegram YAML.
- **Estimated effort:** 3 hours

---

### TASK-16.2: Create duga-telegram-bot crate with teloxide startup

- **SPEC:** §32 (composition root)
- **Labels:** `layer/telegram`, `priority/critical`
- **Description:** Add the Telegram bot binary crate. It loads config, initializes tracing, reads the bot token from `telegram.token_env`, creates the Telegram client, and starts polling.
- **Files affected:**
  - `crates/duga-telegram-bot/Cargo.toml` (new)
  - `crates/duga-telegram-bot/src/main.rs` (new)
  - `crates/duga-telegram-bot/src/bot.rs` (new)
- **Types involved:** `teloxide::Bot`, `Cli`, `TelegramConfig`
- **Functions to implement:**
  - `main() -> anyhow::Result<()>`
  - `run_bot(config: Config) -> anyhow::Result<()>`
- **Dependencies:** TASK-18.1, TASK-18.2, TASK-16.1
- **Implementation steps:**
  1. Add `teloxide`, `tokio`, `anyhow`, `tracing-subscriber`, `notify`, and `cron`.
  2. Parse `--config <PATH>` and `--verbose`.
  3. Load config and Telegram token from env var.
  4. Start long polling.
  5. Persist known chat/user metadata to `.telegram-state.json`.
- **Definition of Done:** Bot starts and responds to messages from an allowed chat.
- **Acceptance criteria:**
  - Missing token env var -> clear startup error.
  - DM message triggers agent run.
  - Group `@botname` mention triggers agent run.
  - Reply to bot message triggers agent run.
- **Test plan:** unit test command parsing; manual smoke with a test bot token.
- **Estimated effort:** 5 hours

---

### TASK-16.3: Telegram auth and update handling

- **SPEC:** §20 (operator-controlled environment), §26 (remote cancellation)
- **Labels:** `layer/telegram`, `priority/critical`
- **Description:** Only process messages from configured chat IDs. Handle commands (`/start`, `/help`, `/stop`, `/status`, `/memory`, `/skills`, `/events`), plain text task messages, and inline button callbacks.
- **Files affected:**
  - `crates/duga-telegram-bot/src/bot.rs`
  - `crates/duga-telegram-bot/src/auth.rs` (new)
- **Types involved:** `ChatId`, `Update`, `TelegramCommand`, `CallbackAction`
- **Functions to implement:**
  - `is_allowed_chat(config: &TelegramConfig, chat_id: ChatId) -> bool`
  - `handle_update(...)`
  - `parse_callback_data(data: &str) -> CallbackAction`
  - `strip_mention(text: &str, bot_username: &str) -> String`
- **Dependencies:** TASK-16.2
- **Implementation steps:**
  1. Reject unauthorized chats without revealing config details.
  2. Implement command parsing.
  3. Detect `@botusername` mentions in groups and strip the mention.
  4. Detect replies to bot messages.
  5. Route plain messages into task execution.
  6. Handle inline button callbacks for confirmation and choices.
- **Definition of Done:** Allowed chats can submit tasks via DM, mention, or reply; unauthorized chats cannot run tools.
- **Acceptance criteria:**
  - Unauthorized chat cannot run tools.
  - `/help` explains supported commands.
  - `/stop` cancels active run.
  - `/memory`, `/skills`, and `/events` show current state.
- **Test plan:** unit tests for auth, command parsing, mention detection, and callback parsing.
- **Estimated effort:** 6 hours

---

### TASK-16.4: Per-chat session manager and cancellation

- **SPEC:** §26 (Cancellation), §21-23 (memory)
- **Labels:** `layer/telegram`, `layer/loop`, `priority/critical`
- **Description:** Track active runs per chat and serialize work with one active run per chat. Multiple chats may run in parallel.
- **Files affected:**
  - `crates/duga-telegram-bot/src/session.rs` (new)
  - `crates/duga-telegram-bot/src/queue.rs` (new)
- **Types involved:** `SessionManager`, `ChatSession`, `ActiveRun`, `ChannelQueue`
- **Functions to implement:**
  - `SessionManager::start_run(chat_id, task)`
  - `SessionManager::cancel(chat_id)`
  - `SessionManager::status(chat_id)`
  - `ChannelQueue::enqueue(chat_id, work)`
  - `sync_context_from_log(log_path) -> Vec<Message>`
- **Dependencies:** TASK-16.3
- **Implementation steps:**
  1. Store sessions in `DashMap<ChatId, ChatSession>`.
  2. Process one run at a time per chat.
  3. Reject a second run while one is active.
  4. Cancel current run on `/stop`.
  5. Persist chat context to `<chat_id>/context.jsonl` after each run.
  6. On restart, sync messages from `log.jsonl` into `context.jsonl`.
- **Definition of Done:** Users can start, inspect, and cancel one run per chat; context survives restarts.
- **Acceptance criteria:**
  - `/stop` causes `AgentError::Cancelled`.
  - A second task while busy returns "run already in progress".
  - Bot restart loads previous context.
  - Multiple chats operate independently.
- **Test plan:** async unit tests with mock runtime.
- **Estimated effort:** 6 hours

---

### TASK-16.5: Telegram event renderer with live message editing

- **SPEC:** §24 (events), §31 (observability)
- **Labels:** `layer/telegram`, `layer/events`, `priority/high`
- **Description:** Consume UI events from the EPIC-18 non-blocking event bridge and render Telegram messages. The renderer must never be called directly from `AgentLoop::emit`; Telegram API calls happen in a worker task.
- **Files affected:**
  - `crates/duga-telegram-bot/src/render.rs` (new)
  - `crates/duga-telegram-bot/src/formatting.rs` (new)
- **Types involved:** `TelegramEventRenderer`, `TelegramFormatter`, `ProcessMessage`
- **Functions to implement:**
  - `TelegramEventRenderer::run(rx)`
  - `edit_process_message(chat_id, msg_id, text)`
  - `start_process_message(chat_id, text)`
  - `finalize_process_message(chat_id)`
- **Dependencies:** TASK-18.4, TASK-16.2
- **Implementation steps:**
  1. Map tool start events to small action labels, not raw tool output.
  2. Suppress successful tool result details from chat; keep them in JSONL logs.
  3. Show truncated errors in the process message.
  4. Chunk messages near Telegram limits.
  5. Rate-limit edits to avoid Telegram flood limits.
  6. If Telegram send/edit fails, log the error and continue the agent run.
- **Definition of Done:** Agent progress appears as live Telegram updates without blocking or failing the runtime.
- **Acceptance criteria:**
  - Final answer is always sent when Telegram API succeeds.
  - Tool action labels update in the process message.
  - Tool result details are not shown in chat.
  - Long outputs are chunked.
  - Renderer failures are surfaced in logs but do not fail `AgentLoop`.
- **Test plan:** formatter tests and renderer tests with mock Telegram API.
- **Estimated effort:** 6 hours

---

### TASK-16.6: Run AgentLoop per Telegram message

- **SPEC:** §5 (loop), §32 (composition root)
- **Labels:** `layer/telegram`, `layer/loop`, `priority/critical`
- **Description:** Wire Telegram messages into `duga-runtime::build_agent`. On each message, reload shared MEMORY.md/SKILL.md context through EPIC-18, attach the non-blocking Telegram event bridge plus JSONL replay sink, then spawn the agent run.
- **Files affected:**
  - `crates/duga-telegram-bot/src/runtime.rs` (new)
  - `crates/duga-telegram-bot/src/session.rs`
- **Types involved:** `TelegramRuntime`, `RunRequest`, `RunResult`
- **Functions to implement:**
  - `run_task_for_chat(chat_id, task, config) -> JoinHandle<Result<...>>`
- **Dependencies:** TASK-18.1, TASK-18.3, TASK-18.4, TASK-16.4, TASK-16.5
- **Implementation steps:**
  1. Build agent with `duga-runtime`.
  2. Attach Telegram event bridge and JSONL event sinks.
  3. Build frontend context with chat metadata, memory summary, loaded skills, event instructions, and sandbox context.
  4. Spawn `AgentLoop::run` with the user's message text.
  5. On completion, persist context to `context.jsonl`.
  6. On error, send a concise error summary to chat and log full details.
- **Definition of Done:** Plain text Telegram messages run the same agent loop as CLI with Telegram-specific context.
- **Acceptance criteria:**
  - Bot answers a simple prompt using configured provider.
  - MEMORY.md and SKILL.md content reach the system prompt through runtime.
  - Chat metadata appears in the frontend context.
  - Replay JSONL is written per chat/run.
- **Test plan:** integration test with mock Telegram adapter and mock LLM.
- **Estimated effort:** 6 hours

---

### TASK-16.7: Telegram confirmation UI

- **SPEC:** §13-16 (sandbox), §20 (environment), §26 (cancellation)
- **Labels:** `layer/telegram`, `layer/security`, `priority/critical`
- **Description:** Implement Telegram's UI side of the EPIC-18 confirmation middleware. Risky tool calls pause in shared middleware; Telegram resolves the request via inline keyboard callbacks.
- **Files affected:**
  - `crates/duga-telegram-bot/src/safety.rs` (new)
  - `crates/duga-telegram-bot/src/bot.rs`
- **Types involved:** `TelegramConfirmationUi`, `ConfirmationRequest`, `ConfirmationDecision`
- **Functions to implement:**
  - `TelegramConfirmationUi::new(bot, chat_id)`
  - `send_confirmation_prompt(tool_name, label, confirmation_id)`
  - `handle_callback_confirm(confirmation_id)`
  - `handle_callback_deny(confirmation_id)`
- **Dependencies:** TASK-18.5, TASK-16.6
- **Implementation steps:**
  1. Register Telegram's confirmation provider with `duga-runtime`.
  2. Send inline keyboard prompt with [Approve] and [Deny].
  3. Store pending confirmation with timeout.
  4. Resolve confirm/deny from button callbacks.
  5. Timeout as denied and edit the prompt.
- **Definition of Done:** Remote shell/write operations cannot run without same-chat confirmation when policy requires it.
- **Acceptance criteria:**
  - `bash` tool call shows inline keyboard prompt.
  - Deny returns `ToolError::Denied` to the agent.
  - Timeout denies safely.
  - Approve executes the tool normally.
- **Test plan:** mocked middleware tests for approve/deny/timeout flows.
- **Estimated effort:** 5 hours

---

### TASK-16.8: Scheduled events

- **SPEC:** new
- **Labels:** `layer/telegram`, `layer/events`, `priority/high`
- **Description:** Implement Telegram event scheduling from JSON files in `events/`: immediate, one-shot, and periodic. Events enqueue agent runs through the per-chat session manager.
- **Files affected:**
  - `crates/duga-telegram-bot/src/events.rs` (new)
  - `crates/duga-telegram-bot/src/bot.rs`
- **Types involved:** `EventsWatcher`, `ImmediateEvent`, `OneShotEvent`, `PeriodicEvent`
- **JSON formats:**
  ```json
  {"type": "immediate", "channelId": "-123456", "text": "New item arrived"}
  {"type": "one-shot", "channelId": "-123456", "text": "Reminder", "at": "2026-05-15T09:00:00+01:00"}
  {"type": "periodic", "channelId": "-123456", "text": "Check inbox", "schedule": "0 9 * * 1-5", "timezone": "Europe/Lisbon"}
  ```
- **Dependencies:** TASK-16.6
- **Implementation steps:**
  1. Watch `events/` with `notify`.
  2. On startup, scan existing files and schedule non-stale events.
  3. Validate `type`, `channelId`, and `text`.
  4. Delete immediate and one-shot files after triggering.
  5. Keep periodic files until deleted.
  6. Enforce a max queued event count per chat.
- **Definition of Done:** Bot responds to scheduled events in the correct chat.
- **Acceptance criteria:**
  - Immediate event runs quickly and file is deleted.
  - One-shot event runs at the requested time.
  - Periodic event runs on schedule.
  - Stale or malformed events are handled explicitly.
- **Test plan:** unit tests for JSON parsing and scheduling; integration test with temp events dir.
- **Estimated effort:** 8 hours

---

### TASK-16.9: Streaming LLM responses to Telegram

- **SPEC:** §5.2 (Streaming contract), §24 (LlmTokenDelta)
- **Labels:** `layer/telegram`, `layer/llm`, `priority/high`
- **Description:** Render `LlmTokenDelta` events as live Telegram updates. Streaming output is escaped plain text while incomplete; optional formatting may be applied only to the final complete response.
- **Files affected:**
  - `crates/duga-telegram-bot/src/render.rs`
  - `crates/duga-telegram-bot/src/formatting.rs`
- **Types involved:** `LlmTokenDelta`, `TelegramEventRenderer`
- **Functions to implement:**
  - `TelegramEventRenderer::on_token_delta(text: &str)`
  - `escape_telegram_plain_text(text: &str) -> String`
  - `format_final_message(text: &str) -> FormattedTelegramMessage`
- **Dependencies:** TASK-8.6, TASK-16.5
- **Implementation steps:**
  1. On first delta, ensure process message exists.
  2. Accumulate deltas into a text buffer.
  3. Escape partial output as plain text; do not parse partial Markdown into Telegram HTML.
  4. Rate-limit edits and chunk near Telegram limits.
  5. On final response, optionally render safe Markdown/HTML after validating the complete message.
  6. Suppress provider thinking tokens from chat display.
  7. If formatted final output fails, fall back to escaped plain text.
- **Definition of Done:** Streaming is live and robust against malformed partial markup.
- **Acceptance criteria:**
  - `streaming: true` updates live as text arrives.
  - `streaming: false` sends a single final message.
  - Partial Markdown never breaks Telegram HTML parsing.
  - Messages near length limits are split safely.
  - Thinking tokens are not shown to the user.
- **Test plan:** integration test with mock SSE stream + mock Telegram API.
- **Estimated effort:** 5 hours

---

### TASK-16.10: Attachment downloading

- **SPEC:** new
- **Labels:** `layer/telegram`, `priority/high`
- **Description:** Download photos, documents, audio, voice, video, and stickers into per-chat directories. Image-to-LLM support depends on a future shared multimodal content type in `duga-types`/`duga-llm`; until then, attachments are referenced by path in the user message.
- **Files affected:**
  - `crates/duga-telegram-bot/src/attachments.rs` (new)
  - `crates/duga-telegram-bot/src/bot.rs`
- **Types involved:** `TelegramAttachment`, `DownloadedFile`
- **Functions to implement:**
  - `download_attachment(bot: &Bot, file_id: &str, save_path: &Path) -> Result<Attachment>`
  - `collect_attachments(msg: &Message, chat_dir: &Path) -> Vec<Attachment>`
- **Dependencies:** TASK-16.2, TASK-16.6
- **Implementation steps:**
  1. Detect supported Telegram attachment kinds.
  2. Download files through Telegram file API.
  3. Save to `<workspace>/<chat_id>/attachments/<msg_id>_<sanitized_name>`.
  4. Add saved paths to a `<chat_attachments>` block in the user message.
  5. Enforce configured file size limits.
- **Definition of Done:** Attachments are downloaded and accessible to the agent as files.
- **Acceptance criteria:**
  - Photo/document sent to bot is saved with a sanitized name.
  - Prompt includes attachment paths.
  - Oversized files are skipped with a visible warning.
- **Test plan:** integration test with mock Telegram file API.
- **Estimated effort:** 4 hours

---

### TASK-16.11: Bot message logging

- **SPEC:** §25 (replay), §31 (observability)
- **Labels:** `layer/telegram`, `priority/high`
- **Description:** Log all messages to per-chat `log.jsonl` in a human-greppable format and maintain `context.jsonl` for LLM history.
- **Files affected:**
  - `crates/duga-telegram-bot/src/log.rs` (new)
  - `crates/duga-telegram-bot/src/bot.rs`
- **Types involved:** `LogEntry`
- **Functions to implement:**
  - `log_message(chat_id, entry: LogEntry)`
  - `sync_context_from_log(session_manager, log_path) -> usize`
- **Dependencies:** TASK-16.4
- **Implementation steps:**
  1. Append incoming messages to `<chat>/log.jsonl`.
  2. Deduplicate by message ID, not timestamp.
  3. Log edited messages as edits.
  4. Sync user messages into `context.jsonl` before agent runs.
  5. Log bot responses after sending.
- **Definition of Done:** Chat messages are persisted and available for agent context.
- **Acceptance criteria:**
  - User and bot messages are logged with stable IDs.
  - Edited messages are logged.
  - Agent run sees messages from before restart.
  - Duplicate delivery does not duplicate context.
- **Test plan:** unit tests for log format, deduplication, and context sync.
- **Estimated effort:** 3 hours

---

### TASK-16.12: Telegram bot integration tests and docs

- **SPEC:** §25 (replay), §31 (observability), §32 (config)
- **Labels:** `layer/telegram`, `layer/testing`, `priority/high`
- **Description:** Add tests and documentation for the full Telegram bot. Tests use mock Telegram API, mock LLM, and mock event files.
- **Files affected:**
  - `crates/duga-telegram-bot/tests/bot_flow.rs` (new)
  - `crates/duga-telegram-bot/tests/events_integration.rs` (new)
  - `crates/duga-telegram-bot/tests/attachments.rs` (new)
  - `README.md` (Telegram bot setup section)
  - `docs/telegram-bot.md` (new)
- **Types involved:** mock Telegram adapter, `CapturingEventSink`, `MockLlm`, `MockTool`
- **Dependencies:** TASK-16.1 through TASK-16.11, TASK-18.1 through TASK-18.6
- **Implementation steps:**
  1. Abstract Telegram send/receive behind `TelegramApi`.
  2. Simulate DM, group mention, reply, unauthorized chat, and `/stop`.
  3. Simulate confirmation approve, deny, timeout.
  4. Simulate scheduled events and attachments.
  5. Verify `log.jsonl` and `context.jsonl` output.
  6. Document token env var, allowed chats, safety confirmations, events, skills, memory, and Docker mode.
- **Definition of Done:** Telegram behavior is covered without live API credentials.
- **Acceptance criteria:**
  - Tests pass offline.
  - README contains bot setup and configuration guide.
  - `docs/telegram-bot.md` explains triggers, commands, events, safety model, memory, skills, and limitations.
- **Test plan:** `cargo test -p duga-telegram-bot`.
- **Estimated effort:** 6 hours

---

## Summary Table

| Task | Name | Est. Hours |
|------|------|------------|
| TASK-16.1 | Telegram config struct + validation | 3 |
| TASK-16.2 | `duga-telegram-bot` crate + startup | 5 |
| TASK-16.3 | Telegram auth + update handling | 6 |
| TASK-16.4 | Per-chat session manager + context persistence | 6 |
| TASK-16.5 | Telegram event renderer with non-blocking live editing | 6 |
| TASK-16.6 | Run AgentLoop per Telegram message | 6 |
| TASK-16.7 | Telegram confirmation UI | 5 |
| TASK-16.8 | Scheduled events | 8 |
| TASK-16.9 | Streaming LLM responses to Telegram | 5 |
| TASK-16.10 | Attachment downloading | 4 |
| TASK-16.11 | Bot message logging | 3 |
| TASK-16.12 | Integration tests and docs | 6 |
| **Total** | | **63 hours** |
