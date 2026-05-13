# EPIC-16: Telegram Bot Frontend

**§SPEC:** §5, §24, §26, §32  
**Labels:** `epic/telegram`  
**Crates:** `duga-runtime` (new), `duga-telegram-bot` (new)

## Goal

Add a Telegram bot frontend that reuses the duga runtime instead of duplicating harness logic. The bot receives chat messages, runs the agent loop with the same config/tool/provider/plugin wiring as the CLI harness, streams progress through events when enabled, supports cancellation, and applies remote-execution safety controls.

---

### TASK-16.1: Extract shared runtime composition from harness

- **§SPEC:** §5 (core loop), §32 (composition root)
- **Labels:** `layer/frontend`, `layer/cli`, `priority/critical`
- **Description:** Move reusable setup logic from `duga-harness` into a new `duga-runtime` crate. CLI, TUI, and Telegram frontends should call this crate to build providers, tools, plugin registry, sinks, memory, and `AgentLoop`.
- **Files affected:**
  - `crates/duga-runtime/Cargo.toml` (new)
  - `crates/duga-runtime/src/lib.rs` (new)
  - `crates/duga-runtime/src/providers.rs` (new)
  - `crates/duga-runtime/src/tools.rs` (new)
  - `crates/duga-runtime/src/agent.rs` (new)
  - `crates/duga-harness/src/main.rs` (use shared runtime)
- **Types involved:** `RuntimeBuilder`, `RuntimeConfig`, `BuiltRuntime`, `AgentLoop`, `ProviderSelection`
- **Functions to implement:**
  - `resolve_provider(config: &Config) -> Result<ProviderSelection>`
  - `build_llm(selection: &ProviderSelection) -> Result<Arc<dyn LlmClient>>`
  - `build_dispatcher(config: &Config, workspace: Arc<Workspace>) -> Result<Arc<ToolDispatcher>>`
  - `build_agent(config: Config, extra_sink: Arc<dyn EventSink>) -> Result<AgentLoop>`
- **Dependencies:** TASK-12.5, TASK-8.3, TASK-8.4
- **Implementation steps:**
  1. Create `duga-runtime` crate with dependencies currently duplicated by `duga-harness`.
  2. Move provider/model routing into `providers.rs`.
  3. Move built-in tool and plugin registration into `tools.rs`.
  4. Move memory/summarizer/event-sink assembly into `agent.rs`.
  5. Update `duga-harness` to become a thin CLI wrapper.
- **Definition of Done:** `duga-harness` behavior is unchanged and frontend crates can build an agent without copying harness code.
- **Acceptance criteria:**
  - CLI still runs with existing YAML config.
  - Provider routing tests move to `duga-runtime`.
  - No duplicated provider/tool wiring remains in `duga-harness`.
- **Test plan:** unit tests for provider resolution and tool registration; smoke test CLI.
- **Estimated effort:** 6 hours

---

### TASK-16.2: Telegram config struct + validation

- **§SPEC:** §32 (config loading), §20 (secrets), §26 (cancellation)
- **Labels:** `layer/telegram`, `layer/cli`, `priority/critical`
- **Description:** Extend `duga-config` with optional Telegram configuration. Token is read from an environment variable, not stored in YAML. Allowed chat IDs are mandatory for non-development mode.
- **Files affected:**
  - `crates/duga-config/src/config.rs`
- **Types involved:** `TelegramConfig`
- **Functions to implement:** config struct + validation
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
  ```
- **Dependencies:** TASK-12.1, TASK-12.3
- **Implementation steps:**
  1. Add `telegram: Option<TelegramConfig>` to `Config`.
  2. Validate `token_env` is non-empty when Telegram is configured.
  3. Validate `allowed_chat_ids` is non-empty unless `allow_all_chats_for_dev` is true.
  4. Warn if token env var name matches secret pattern.
- **Definition of Done:** Telegram config parses and validation catches unsafe defaults.
- **Acceptance criteria:**
  - Missing token env name -> validation error.
  - Empty allowlist in production mode -> validation error.
- **Test plan:** config unit tests with valid and invalid Telegram YAML.
- **Estimated effort:** 3 hours

---

### TASK-16.3: Create duga-telegram-bot crate with teloxide startup

- **§SPEC:** §32 (composition root)
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
- **Dependencies:** TASK-16.1, TASK-16.2
- **Implementation steps:**
  1. Add `teloxide`, `tokio`, `anyhow`, and `tracing-subscriber`.
  2. Parse `--config <PATH>` and `--verbose`.
  3. Load config and Telegram token.
  4. Start long polling.
- **Definition of Done:** Bot starts and responds to `/start` from an allowed chat.
- **Acceptance criteria:**
  - Missing token env var -> clear startup error.
  - `/start` returns a short help message.
- **Test plan:** unit test command parsing; manual smoke with a test bot token.
- **Estimated effort:** 4 hours

---

### TASK-16.4: Telegram auth and update handling

- **§SPEC:** §20 (operator-controlled environment), §26 (remote cancellation)
- **Labels:** `layer/telegram`, `priority/critical`
- **Description:** Only process messages from configured chat IDs. Handle `/start`, `/help`, `/cancel`, `/status`, and plain text task messages.
- **Files affected:**
  - `crates/duga-telegram-bot/src/bot.rs`
  - `crates/duga-telegram-bot/src/auth.rs` (new)
- **Types involved:** `ChatId`, `Update`, `TelegramCommand`
- **Functions to implement:**
  - `is_allowed_chat(config: &TelegramConfig, chat_id: ChatId) -> bool`
  - `handle_update(...)`
- **Dependencies:** TASK-16.3
- **Implementation steps:**
  1. Reject unauthorized chats without revealing config details.
  2. Implement command parsing.
  3. Route plain messages into task execution.
  4. Add `/cancel` and `/status` hooks for active sessions.
- **Definition of Done:** Unauthorized chats are ignored or receive a generic denial; allowed chats can submit tasks.
- **Acceptance criteria:**
  - Unauthorized chat cannot run tools.
  - `/help` explains supported commands.
- **Test plan:** unit tests for auth and command parsing.
- **Estimated effort:** 4 hours

---

### TASK-16.5: Per-chat session manager and cancellation

- **§SPEC:** §26 (Cancellation), §21-23 (memory)
- **Labels:** `layer/telegram`, `layer/loop`, `priority/critical`
- **Description:** Track active runs per chat. Each run has a `CancellationToken`, task handle, started timestamp, and optional per-chat memory/session policy.
- **Files affected:**
  - `crates/duga-telegram-bot/src/session.rs` (new)
- **Types involved:** `SessionManager`, `ChatSession`, `ActiveRun`
- **Functions to implement:**
  - `SessionManager::start_run(chat_id, task)`
  - `SessionManager::cancel(chat_id)`
  - `SessionManager::status(chat_id)`
- **Dependencies:** TASK-16.4
- **Implementation steps:**
  1. Store sessions in `DashMap` or `tokio::sync::Mutex<HashMap<...>>`.
  2. Reject or queue a second run while one is active in the same chat.
  3. Cancel current run on `/cancel`.
  4. Clean up completed handles.
- **Definition of Done:** Users can start, inspect, and cancel a run per chat.
- **Acceptance criteria:**
  - `/cancel` causes `AgentError::Cancelled`.
  - A second task while busy returns "run already in progress".
- **Test plan:** async unit tests with mock runtime.
- **Estimated effort:** 5 hours

---

### TASK-16.6: TelegramEventSink for progress reporting

- **§SPEC:** §24 (events), §31 (observability)
- **Labels:** `layer/telegram`, `layer/events`, `priority/high`
- **Description:** Implement an `EventSink` that converts selected duga events into Telegram messages. It supports final-only mode and progress mode.
- **Files affected:**
  - `crates/duga-telegram-bot/src/sink.rs` (new)
  - `crates/duga-telegram-bot/src/formatting.rs` (new)
- **Types involved:** `TelegramEventSink`, `TelegramFormatter`
- **Functions to implement:**
  - `TelegramEventSink::new(bot, chat_id, options)`
  - `impl EventSink for TelegramEventSink`
  - `format_event(event: &Event) -> Option<String>`
- **Dependencies:** TASK-7.2, TASK-16.3
- **Implementation steps:**
  1. Map `ToolCallStarted`, `ToolCallFinished`, `Error`, and `AgentFinished`.
  2. Escape Telegram Markdown/HTML safely.
  3. Chunk messages over Telegram length limits.
  4. Rate-limit progress messages to avoid flooding.
- **Definition of Done:** Agent progress can be sent to Telegram without blocking the core loop.
- **Acceptance criteria:**
  - Final answer is always sent.
  - Tool events are sent only when enabled.
  - Long outputs are chunked or summarized.
- **Test plan:** formatter unit tests and sink tests with mock bot adapter.
- **Estimated effort:** 5 hours

---

### TASK-16.7: Run AgentLoop per Telegram message

- **§SPEC:** §5 (loop), §32 (composition root)
- **Labels:** `layer/telegram`, `layer/loop`, `priority/critical`
- **Description:** Wire Telegram messages into `duga-runtime::build_agent`. Add a per-run `TelegramEventSink` plus JSONL sink, then spawn the agent run.
- **Files affected:**
  - `crates/duga-telegram-bot/src/runtime.rs` (new)
  - `crates/duga-telegram-bot/src/session.rs`
- **Types involved:** `TelegramRuntime`, `RunRequest`, `RunResult`
- **Functions to implement:**
  - `run_task_for_chat(chat_id, task, config) -> JoinHandle<Result<...>>`
- **Dependencies:** TASK-16.1, TASK-16.5, TASK-16.6
- **Implementation steps:**
  1. Build agent with shared runtime.
  2. Attach Telegram and JSONL event sinks.
  3. Spawn `AgentLoop::run`.
  4. Send final result or error to chat.
- **Definition of Done:** Plain text Telegram message runs the same agent loop as CLI.
- **Acceptance criteria:**
  - Bot answers a simple prompt using configured provider.
  - Replay JSONL is written per chat/run.
- **Test plan:** integration test with mock Telegram adapter and mock LLM.
- **Estimated effort:** 5 hours

---

### TASK-16.8: Safety controls and tool confirmation

- **§SPEC:** §13-16 (sandbox), §20 (environment), §26 (cancellation)
- **Labels:** `layer/telegram`, `layer/security`, `priority/critical`
- **Description:** Add remote safety controls before enabling powerful tools over Telegram. For configured tools (`bash`, `write` by default), require explicit confirmation from the same chat before dispatch.
- **Files affected:**
  - `crates/duga-telegram-bot/src/safety.rs` (new)
  - `crates/duga-runtime/src/tools.rs` (confirmation hook support)
- **Types involved:** `ToolConfirmationPolicy`, `PendingConfirmation`
- **Functions to implement:**
  - `requires_confirmation(tool_name: &str) -> bool`
  - `request_confirmation(chat_id, tool_call)`
  - `confirm_or_deny(chat_id, confirmation_id)`
- **Dependencies:** TASK-16.7
- **Implementation steps:**
  1. Define config for confirmation-required tools.
  2. Pause or fail tool calls pending confirmation.
  3. Add `/confirm <id>` and `/deny <id>` commands.
  4. Timeout pending confirmations.
- **Definition of Done:** Remote shell/write operations cannot run without same-chat confirmation when policy requires it.
- **Acceptance criteria:**
  - `bash` tool call creates a confirmation prompt.
  - `/deny` returns a tool error to the agent.
  - Timeout denies safely.
- **Test plan:** mocked dispatcher tests for confirm/deny flows.
- **Estimated effort:** 6 hours

---

### TASK-16.9: Telegram bot integration tests and docs

- **§SPEC:** §25 (replay), §31 (observability), §32 (config)
- **Labels:** `layer/telegram`, `layer/testing`, `priority/high`
- **Description:** Add tests and documentation for running the Telegram bot. Tests should not require the real Telegram API.
- **Files affected:**
  - `crates/duga-telegram-bot/tests/bot_flow.rs` (new)
  - `README.md` (Telegram bot setup section)
  - `docs/telegram-bot.md` (new)
- **Types involved:** mock Telegram adapter, `CapturingEventSink`, `MockLlm`, `MockTool`
- **Functions to implement:** test helpers for simulated updates and outbound messages
- **Dependencies:** TASK-16.1-TASK-16.8
- **Implementation steps:**
  1. Abstract Telegram send/receive behind a small trait for tests.
  2. Simulate allowed and unauthorized chats.
  3. Simulate `/cancel`, `/status`, and one successful task.
  4. Verify replay file creation.
  5. Document setup: token env var, allowed chat IDs, safety confirmations.
- **Definition of Done:** Telegram bot behavior is covered without live API credentials.
- **Acceptance criteria:**
  - Tests pass offline.
  - README contains minimal bot setup.
  - docs page explains safety model and limitations.
- **Test plan:** `cargo test -p duga-telegram-bot`.
- **Estimated effort:** 4 hours
