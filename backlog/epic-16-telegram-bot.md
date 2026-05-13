# EPIC-16: Telegram Bot Frontend

**§SPEC:** §5, §24, §26, §32
**Labels:** `epic/telegram`
**Crates:** `duga-runtime` (new, shared with CLI/TUI), `duga-telegram-bot` (new)

## Goal

Add a Telegram bot frontend that reuses the shared duga runtime. The bot receives chat messages, runs the agent loop with the same config/tool/provider/plugin wiring as the CLI harness, streams progress through events when enabled, supports cancellation, and applies remote-execution safety controls.

Beyond the basic agent loop, the bot provides:
- **Persistent memory** (MEMORY.md files) — shared with TUI via `duga-runtime`
- **Skills** (SKILL.md files) — shared with TUI via `duga-runtime`
- **Scheduled events** (immediate, one-shot, periodic via JSON files in `events/`)
- **Live streaming** — edits a single Telegram message in place as the LLM generates
- **Attachment downloading** — photos, documents, audio, video into per-chat directories
- **Docker sandbox** — full shell access inside an isolated container (shared via `duga-sandbox`)
- **Real token counting** — per-provider tokenizers for accurate memory budgeting (shared via `duga-llm`)
- **Tool confirmation** — require user approval for risky tools over Telegram (shared hook in `duga-runtime`)

---

### TASK-16.1: Extract shared runtime composition from harness (+ persistent memory, skills, confirmation hooks)

- **§SPEC:** §5 (core loop), §32 (composition root)
- **Labels:** `layer/frontend`, `layer/cli`, `priority/critical`
- **Description:** Move reusable setup logic from `duga-harness` into a new `duga-runtime` crate. CLI, TUI, and Telegram frontends should call this crate to build providers, tools, plugin registry, sinks, memory, and `AgentLoop`. **This task also includes shared infrastructure needed by both Telegram and TUI frontends:** persistent MEMORY.md loading, SKILL.md skill loading, and tool confirmation hooks.
- **Files affected:**
  - `crates/duga-runtime/Cargo.toml` (new)
  - `crates/duga-runtime/src/lib.rs` (new)
  - `crates/duga-runtime/src/providers.rs` (new)
  - `crates/duga-runtime/src/tools.rs` (new — includes tool confirmation hook)
  - `crates/duga-runtime/src/agent.rs` (new)
  - `crates/duga-runtime/src/memory_context.rs` (new — MEMORY.md loading)
  - `crates/duga-runtime/src/skills.rs` (new — SKILL.md loading)
  - `crates/duga-harness/src/main.rs` (use shared runtime)
- **Types involved:** `RuntimeBuilder`, `RuntimeConfig`, `BuiltRuntime`, `AgentLoop`, `ProviderSelection`, `Skill`, `PersistentMemory`, `ToolConfirmationHook`, `ConfirmationPolicy`
- **Functions to implement:**
  - `resolve_provider(config: &Config) -> Result<ProviderSelection>`
  - `build_llm(selection: &ProviderSelection) -> Result<Arc<dyn LlmClient>>`
  - `build_dispatcher(config: &Config, workspace: Arc<Workspace>) -> Result<Arc<ToolDispatcher>>`
  - `build_agent(config: Config, extra_sink: Arc<dyn EventSink>) -> Result<AgentLoop>`
  - `load_persistent_memory(workspace: &Path, chat_dir: Option<&Path>) -> String` — reads MEMORY.md files
  - `load_skills(workspace: &Path, chat_dir: Option<&Path>) -> Vec<Skill>` — reads SKILL.md files
  - `format_skills_for_prompt(skills: &[Skill]) -> String` — system prompt injection
  - `register_confirmation_hook(dispatcher: &ToolDispatcher, policy: ConfirmationPolicy)` — shared tool confirmation infrastructure
- **Dependencies:** TASK-12.5, TASK-8.3, TASK-8.4, TASK-6.x (Memory)
- **Implementation steps:**
  1. Create `duga-runtime` crate with dependencies currently duplicated by `duga-harness`.
  2. Move provider/model routing into `providers.rs`.
  3. Move built-in tool and plugin registration into `tools.rs`.
  4. Move memory/summarizer/event-sink assembly into `agent.rs`.
  5. **Implement MEMORY.md loading:** read `workspace/MEMORY.md` (global) and optionally `workspace/<chat>/MEMORY.md` (per-chat). Inject into system prompt as "### Current Memory" section. Agent updates MEMORY.md via existing `write`/`edit` tools.
  6. **Implement SKILL.md loading:** walk `skills/` directories, parse YAML frontmatter (`name`, `description`), resolve `{baseDir}` placeholder to the skill's directory path, inject into system prompt. Channel skills override workspace skills on name collision.
  7. **Implement tool confirmation hook:** `ConfirmationPolicy` struct with `require_confirmation_for: Vec<String>` (tool names). Register a hook in `ToolDispatcher` that pauses tool dispatch and calls an async callback. Frontends provide the callback (Telegram sends inline button prompt, TUI shows modal dialog). Timeout-based denial after configurable period (default 60s).
  8. Update `duga-harness` to become a thin CLI wrapper.
- **Definition of Done:** `duga-harness` behavior is unchanged. Frontend crates can build an agent with persistent memory, skills, and confirmation hooks without copying code.
- **Acceptance criteria:**
  - CLI still runs with existing YAML config.
  - Provider routing tests move to `duga-runtime`.
  - MEMORY.md content appears in system prompt when file exists.
  - SKILL.md files parsed correctly; collision resolution works (channel skills override workspace).
  - Confirmation hook pauses tool dispatch; timeout denies safely.
  - No duplicated provider/tool/memory/skills wiring remains in `duga-harness`.
- **Test plan:** unit tests for provider resolution, tool registration, MEMORY.md loading, SKILL.md parsing, confirmation hook lifecycle; smoke test CLI.
- **Estimated effort:** 12 hours

---

### TASK-16.2: Telegram config struct + validation

- **§SPEC:** §32 (config loading), §20 (secrets), §26 (cancellation)
- **Labels:** `layer/telegram`, `layer/cli`, `priority/critical`
- **Description:** Extend `duga-config` with optional Telegram configuration. Token is read from an environment variable, not stored in YAML. Allowed chat IDs are mandatory for non-development mode. Includes configuration for events, skills, and attachments.
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
    events_dir: "./events"
    skills_dir: "./skills"
    data_dir: "./data"
    attachments:
      enabled: true
      max_file_size_mb: 20
      download_dir: "attachments"
  ```
  Note: sandbox configuration (Docker vs host vs capability) lives in the top-level `sandbox` section, shared with CLI/TUI (see TASK-16.2a).
- **Dependencies:** TASK-12.1, TASK-12.3
- **Implementation steps:**
  1. Add `telegram: Option<TelegramConfig>` to `Config`.
  2. Validate `token_env` is non-empty when Telegram is configured.
  3. Validate `allowed_chat_ids` is non-empty unless `allow_all_chats_for_dev` is true.
  4. Validate `events_dir` path (create on startup if needed).
  5. Validate `attachments.max_file_size_mb` is > 0 and ≤ 50 (Telegram's limit is 50MB).
  6. Warn if token env var name matches secret pattern.
- **Definition of Done:** Telegram config parses and validation catches unsafe defaults.
- **Acceptance criteria:**
  - Missing token env name → validation error.
  - Empty allowlist in production mode → validation error.
  - Invalid `max_file_size_mb` → validation error.
- **Test plan:** config unit tests with valid and invalid Telegram YAML.
- **Estimated effort:** 4 hours

---

### TASK-16.2a: Docker sandbox executor in duga-sandbox (shared)

- **§SPEC:** §15 (Sandbox), §17 (process execution)
- **Labels:** `layer/sandbox`, `priority/critical`
- **Description:** Add a Docker execution mode to `duga-sandbox` that runs commands inside a long-lived container via `docker exec`. Unlike the capability-bounded sandbox (binary allowlist, sanitized env), Docker mode gives the agent full shell access inside an isolated container. The workspace directory is bind-mounted at `/workspace`. Shared by Telegram, TUI, and CLI frontends. Tools run via `sh -c` (full shell access) rather than argv dispatch when in Docker mode.
- **Files affected:**
  - `crates/duga-sandbox/src/docker.rs` (new)
  - `crates/duga-sandbox/src/exec.rs` (modify — add Docker variant)
  - `crates/duga-sandbox/src/lib.rs` (add module)
  - `crates/duga-config/src/config.rs` (modify `SandboxConfig`)
- **Types involved:** `DockerExecutor`, `SandboxMode` enum (`Host`, `Capability`, `Docker`)
- **Functions to implement:**
  - `DockerExecutor::new(container: &str, workspace_mount: &str) -> Self`
  - `impl Executor for DockerExecutor` — runs `docker exec <container> sh -c <command>`
  - `validate_container(container: &str) -> Result<()>` — `docker inspect` check
  - `translate_to_container(path: &str) -> String` — host path → `/workspace/...`
  - `translate_to_host(path: &str) -> String` — `/workspace/...` → host path
- **Config extension:**
  ```yaml
  sandbox:
    mode: "docker"                  # "host" | "capability" | "docker"
    container: "duga-sandbox"       # required when mode=docker
    workspace_mount: "/workspace"   # mount point in container
    timeout: 120s
    # allowed_binaries + environment sections are ignored in docker mode
    # (full shell access inside container)
  ```
- **Dependencies:** TASK-2.4 (run_captured), TASK-2.5 (SanitizedEnv)
- **Implementation steps:**
  1. Add `SandboxMode` enum to `SandboxConfig` (default: `Capability` for backward compat).
  2. Implement `DockerExecutor` spawning `docker exec <container> sh -c <command>` via `tokio::process::Command`.
  3. Apply the same timeout, cancellation, and output limits as `run_captured()`.
  4. On startup: validate container exists and is running via `docker inspect`.
  5. Path translation: when tools write to `/workspace/...`, translate to host path for read/attach operations.
  6. When Docker mode is active, the `bash` tool uses `sh -c` (full shell), not argv dispatch.
  7. Inject `docker exec` context into the system prompt so the agent knows it's containerized (e.g., "Install tools with: apk add <package>").
- **Definition of Done:** Agent runs commands inside Docker container with full shell access. All three frontends (CLI, TUI, Telegram) can use Docker mode via the same config.
- **Acceptance criteria:**
  - `docker exec <container> sh -c "echo hello"` → returns "hello"
  - Agent can `apk add curl` inside container → persists across runs (bind mount)
  - Files written to `/workspace/...` are visible on host at workspace root
  - Output limits still enforced in Docker mode
  - CancellationToken stops `docker exec` processes
  - Container not running → clear startup error
  - Capability sandbox mode still works (backward compatible)
- **Test plan:** unit tests for path translation; integration test with real Docker container; test `apk add` persistence
- **Estimated effort:** 6 hours

---

### TASK-16.3: Create duga-telegram-bot crate with teloxide startup

- **§SPEC:** §32 (composition root)
- **Labels:** `layer/telegram`, `priority/critical`
- **Description:** Add the Telegram bot binary crate. It loads config, initializes tracing, reads the bot token from `telegram.token_env`, creates the Telegram client, and starts polling. Uses `teloxide` (or `grammy` if teloxide proves too heavy).
- **Files affected:**
  - `crates/duga-telegram-bot/Cargo.toml` (new)
  - `crates/duga-telegram-bot/src/main.rs` (new)
  - `crates/duga-telegram-bot/src/bot.rs` (new)
- **Types involved:** `teloxide::Bot` (or `grammy::Bot`), `Cli`, `TelegramConfig`
- **Functions to implement:**
  - `main() -> anyhow::Result<()>`
  - `run_bot(config: Config) -> anyhow::Result<()>`
- **Dependencies:** TASK-16.1, TASK-16.2, TASK-16.2a
- **Implementation steps:**
  1. Add `teloxide` (or `grammy`), `tokio`, `anyhow`, `tracing-subscriber`, `notify` (for events watching), `cron` (for periodic events).
  2. Parse `--config <PATH>` and `--verbose`.
  3. Load config and Telegram token from env var.
  4. Start long polling.
  5. Persist known chat/user metadata to `.telegram-state.json` so chat names and users survive restarts.
- **Definition of Done:** Bot starts and responds to messages from an allowed chat.
- **Acceptance criteria:**
  - Missing token env var → clear startup error.
  - DM message triggers agent run.
  - Group `@botname` mention triggers agent run.
  - Reply to bot message triggers agent run.
- **Test plan:** unit test command parsing; manual smoke with a test bot token.
- **Estimated effort:** 5 hours

---

### TASK-16.4: Telegram auth and update handling

- **§SPEC:** §20 (operator-controlled environment), §26 (remote cancellation)
- **Labels:** `layer/telegram`, `priority/critical`
- **Description:** Only process messages from configured chat IDs. Handle commands (`/start`, `/help`, `/stop`, `/status`, `/memory`, `/skills`, `/events`), plain text task messages, and inline button callbacks (from tool confirmation and choice buttons). The bot triggers on: any DM, `@botusername` mention in groups, and replies to its own messages.
- **Files affected:**
  - `crates/duga-telegram-bot/src/bot.rs`
  - `crates/duga-telegram-bot/src/auth.rs` (new)
- **Types involved:** `ChatId`, `Update`, `TelegramCommand`, `CallbackAction`
- **Functions to implement:**
  - `is_allowed_chat(config: &TelegramConfig, chat_id: ChatId) -> bool`
  - `handle_update(...)`
  - `parse_callback_data(data: &str) -> CallbackAction` — handles confirm/deny/choice callbacks
  - `strip_mention(text: &str, bot_username: &str) -> String` — removes `@botname` prefix from text
- **Dependencies:** TASK-16.3
- **Implementation steps:**
  1. Reject unauthorized chats without revealing config details (silent drop).
  2. Implement command parsing: `/start` (help summary), `/stop` (cancel active run), `/status` (show current run), `/memory` (show MEMORY.md content), `/skills` (list loaded skills), `/events` (list scheduled events).
  3. Detect `@botusername` mentions in groups, strip the mention from text before passing to agent.
  4. Detect replies to bot's own messages → trigger agent run with reply text.
  5. Route plain messages into task execution.
  6. Handle inline button callbacks: confirm/deny for tool confirmation, choice callbacks for general interaction.
- **Definition of Done:** Unauthorized chats are ignored or receive a generic denial; allowed chats can submit tasks via DM, mention, or reply.
- **Acceptance criteria:**
  - Unauthorized chat cannot run tools (silently dropped).
  - `/help` explains supported commands.
  - `/stop` cancels active run.
  - `/memory` shows current MEMORY.md content.
  - `/skills` lists loaded skills.
  - `/events` lists scheduled events.
  - `@botname do something` in group → agent runs with "do something".
  - Reply to bot message → agent runs with reply text.
- **Test plan:** unit tests for auth, command parsing, mention detection, and callback parsing.
- **Estimated effort:** 6 hours

---

### TASK-16.5: Per-chat session manager and cancellation

- **§SPEC:** §26 (Cancellation), §21-23 (memory)
- **Labels:** `layer/telegram`, `layer/loop`, `priority/critical`
- **Description:** Track active runs per chat. Each run has a `CancellationToken`, task handle, started timestamp. Per-chat message queuing with `ChannelQueue` to serialize agent runs (one at a time per chat). Reject a second run while one is active. Per-chat `context.jsonl` persistence for LLM message history across restarts (synced from `log.jsonl`).
- **Files affected:**
  - `crates/duga-telegram-bot/src/session.rs` (new)
  - `crates/duga-telegram-bot/src/queue.rs` (new)
- **Types involved:** `SessionManager`, `ChatSession`, `ActiveRun`, `ChannelQueue`
- **Functions to implement:**
  - `SessionManager::start_run(chat_id, task)`
  - `SessionManager::cancel(chat_id)`
  - `SessionManager::status(chat_id)`
  - `ChannelQueue::enqueue(chat_id, work)` — serializes per-chat processing
  - `sync_context_from_log(log_path) -> Vec<Message>` — rebuild context from log file
- **Dependencies:** TASK-16.4
- **Implementation steps:**
  1. Store sessions in `DashMap<ChatId, ChatSession>`.
  2. Each chat has a `ChannelQueue` that processes one run at a time.
  3. Reject a second run while one is active (return "run already in progress").
  4. Cancel current run on `/stop` → calls `CancellationToken::cancel()`.
  5. Clean up completed handles.
  6. Persist chat context to `<chat_id>/context.jsonl` after each run (for restart resilience).
  7. On restart: sync messages from `log.jsonl` into `context.jsonl` to rebuild context.
- **Definition of Done:** Users can start, inspect, and cancel a run per chat. Context survives bot restarts.
- **Acceptance criteria:**
  - `/stop` causes `AgentError::Cancelled`.
  - A second task while busy returns "run already in progress".
  - Bot restart → previous messages loaded from context.jsonl.
  - Multiple chats operate independently (parallel runs across different chats).
- **Test plan:** async unit tests with mock runtime.
- **Estimated effort:** 6 hours

---

### TASK-16.6: TelegramEventSink for progress reporting (with live message editing)

- **§SPEC:** §24 (events), §31 (observability)
- **Labels:** `layer/telegram`, `layer/events`, `priority/high`
- **Description:** Implement an `EventSink` that converts selected duga events into Telegram messages. Supports progress mode with live message editing (accumulates output in a single Telegram message via `editMessageText`). Tool traces are suppressed from chat display (telemetry only); only the current action label is shown (e.g., "_→ reading file_"). Final result sent as a separate message.
- **Files affected:**
  - `crates/duga-telegram-bot/src/sink.rs` (new)
  - `crates/duga-telegram-bot/src/formatting.rs` (new)
- **Types involved:** `TelegramEventSink`, `TelegramFormatter`, `ProcessMessage`
- **Functions to implement:**
  - `TelegramEventSink::new(bot, chat_id, options)`
  - `impl EventSink for TelegramEventSink`
  - `format_event(event: &Event) -> Option<String>`
  - `edit_process_message(chat_id, msg_id, text)` — accumulates and edits in place
  - `start_process_message(chat_id, text)` — sends initial "Thinking..." message
  - `finalize_process_message(chat_id)` — deletes process message, sends final result
- **Dependencies:** TASK-7.2, TASK-16.3
- **Implementation steps:**
  1. Map `ToolCallStarted` → show "_→ {label}_" in process message (no tool result details in chat).
  2. Map `ToolCallFinished` (success) → suppressed from chat (logged to console only).
  3. Map `ToolCallFinished` (error) → show "_Error: {truncated_error}_" in process message.
  4. Map `LlmTokenDelta` → accumulate text, call `editMessageText` with HTML parse_mode.
  5. Map `AgentFinished` → delete process message, send final result as a separate message.
  6. Escape Telegram HTML: convert `*bold*` → `<b>bold</b>`, `_italic_` → `<i>italic</i>`, `` `code` `` → `<code>code</code>`, `` ```block``` `` → `<pre>block</pre>`. Escape `&`, `<`, `>` in non-markup text before conversion.
  7. Chunk messages at 3800 chars (start new process message when full, include "(continued...)" in previous).
  8. Support `[SILENT]` marker: agent responds with only `[SILENT]` → delete process message, post nothing to chat.
  9. Support inline button syntax `---BUTTONS---/---ENDBUTTONS---` for confirmation and choice dialogs.
  10. Rate-limit edits to prevent Telegram flood limits (max 1 edit per 200ms).
- **Definition of Done:** Agent progress appears as a single live-edited message in Telegram.
- **Acceptance criteria:**
  - Final answer is always sent as a separate message.
  - Tool action labels ("_→ reading file_") update in the process message.
  - Tool results details are NOT shown in chat (only in log.jsonl).
  - Long outputs (>3800 chars) start new process messages.
  - HTML special characters properly escaped.
  - `[SILENT]` response → process message deleted, nothing posted to chat.
  - Inline buttons render correctly in Telegram.
- **Test plan:** formatter unit tests and sink tests with mock bot adapter.
- **Estimated effort:** 6 hours

---

### TASK-16.7: Run AgentLoop per Telegram message

- **§SPEC:** §5 (loop), §32 (composition root)
- **Labels:** `layer/telegram`, `layer/loop`, `priority/critical`
- **Description:** Wire Telegram messages into `duga-runtime::build_agent`. On each message, reload MEMORY.md + SKILL.md context (they may have changed since last run), build a fresh system prompt with chat metadata (known users, known chats), current memory, and loaded skills. Attach `TelegramEventSink` plus JSONL replay sink, then spawn the agent run.
- **Files affected:**
  - `crates/duga-telegram-bot/src/runtime.rs` (new)
  - `crates/duga-telegram-bot/src/session.rs`
- **Types involved:** `TelegramRuntime`, `RunRequest`, `RunResult`
- **Functions to implement:**
  - `run_task_for_chat(chat_id, task, config) -> JoinHandle<Result<...>>`
- **Dependencies:** TASK-16.1, TASK-16.5, TASK-16.6
- **Implementation steps:**
  1. Build agent with `duga-runtime` (which reloads MEMORY.md + skills each run).
  2. Attach `TelegramEventSink` and `JsonlSink` event sinks.
  3. Build system prompt including: chat metadata (channel list, user list from `.telegram-state.json`), current memory content, loaded skills summary, event creation instructions, Docker context (if applicable).
  4. Spawn `AgentLoop::run` with the user's message text (including attachment references if any).
  5. On completion: send final result via sink, persist context to `context.jsonl`.
  6. On error: send error summary to chat, log full error to console.
- **Definition of Done:** Plain text Telegram message runs the same agent loop as CLI, with full context (memory, skills, metadata).
- **Acceptance criteria:**
  - Bot answers a simple prompt using configured provider.
  - MEMORY.md content is visible to the agent (in system prompt).
  - Skills are listed in system prompt.
  - Chat metadata (users, channels) appears in system prompt.
  - Replay JSONL is written per chat/run.
- **Test plan:** integration test with mock Telegram adapter and mock LLM.
- **Estimated effort:** 6 hours

---

### TASK-16.8: Safety controls and tool confirmation (Telegram frontend)

- **§SPEC:** §13-16 (sandbox), §20 (environment), §26 (cancellation)
- **Labels:** `layer/telegram`, `layer/security`, `priority/critical`
- **Description:** Wire the shared tool confirmation hook from `duga-runtime` (TASK-16.1) to Telegram's UI. When a tool requiring confirmation is called (`bash`, `write` by default), the hook pauses dispatch and the Telegram sink sends an inline keyboard prompt: "Allow `bash: <label>`? [Approve] [Deny]". The user taps a button; the callback resolves the confirmation. Timeout after 60s denies automatically.
- **Files affected:**
  - `crates/duga-telegram-bot/src/safety.rs` (new — Telegram-specific confirmation UI)
- **Types involved:** `ToolConfirmationPolicy`, `PendingConfirmation` (from `duga-runtime`)
- **Functions to implement:**
  - `TelegramConfirmationUi::new(bot, chat_id)` — implements the confirmation callback for Telegram
  - `send_confirmation_prompt(tool_name, label, confirmation_id)` — sends inline keyboard
  - `handle_callback_confirm(confirmation_id)` / `handle_callback_deny(confirmation_id)`
- **Dependencies:** TASK-16.1 (confirmation hook in runtime), TASK-16.7
- **Implementation steps:**
  1. Register the Telegram confirmation callback with `duga-runtime`'s confirmation hook.
  2. On confirmation required: send inline keyboard message with tool name + label + [Approve]/[Deny] buttons.
  3. Store pending confirmation with a timeout timer (default 60s).
  4. On button callback: resolve confirm/deny, delete the prompt message.
  5. On timeout: deny automatically, edit prompt to show "Denied (timeout)".
  6. Denied tools return `ToolError::Denied("user denied confirmation")` to the agent loop.
- **Definition of Done:** Remote shell/write operations cannot run without same-chat confirmation when policy requires it.
- **Acceptance criteria:**
  - `bash` tool call → inline keyboard confirmation prompt appears.
  - Tap "Deny" → `ToolError::Denied` returned to agent.
  - Timeout (60s) → auto-denied, prompt updated to show timeout.
  - Tap "Approve" → tool executes normally.
  - Non-confirmation tools (read, search, think) execute without prompt.
- **Test plan:** mocked dispatcher tests for confirm/deny/timeout flows.
- **Estimated effort:** 5 hours

---

### TASK-16.9: Scheduled events (immediate, one-shot, periodic)

- **§SPEC:** new
- **Labels:** `layer/telegram`, `layer/events`, `priority/high`
- **Description:** Implement an event scheduling system based on JSON files in the `events/` directory. Three types: **immediate** (triggers as soon as detected), **one-shot** (triggers at a specific ISO 8601 timestamp), **periodic** (triggers on a cron schedule). The Telegram bot watches the directory via `inotify`/`kqueue` (`notify` crate) and spawns agent runs when events fire. Immediate and one-shot auto-delete after triggering; periodic events persist until their JSON file is deleted. Max 5 queued events per chat.
- **Files affected:**
  - `crates/duga-telegram-bot/src/events.rs` (new)
  - `crates/duga-telegram-bot/src/bot.rs` (modify — start/stop events watcher)
- **Types involved:** `EventsWatcher`, `ImmediateEvent`, `OneShotEvent`, `PeriodicEvent`
- **JSON formats:**
  ```json
  {"type": "immediate", "channelId": "-123456", "text": "New item arrived"}
  {"type": "one-shot", "channelId": "-123456", "text": "Reminder", "at": "2026-05-15T09:00:00+01:00"}
  {"type": "periodic", "channelId": "-123456", "text": "Check inbox", "schedule": "0 9 * * 1-5", "timezone": "Europe/Lisbon"}
  ```
- **Functions to implement:**
  - `EventsWatcher::new(events_dir, bot_handle) -> Self`
  - `EventsWatcher::start()` — scan existing files, start fs watcher
  - `EventsWatcher::stop()` — cancel all scheduled timers/crons
  - `handle_immediate(filename, event)` — check mtime > startup, enqueue run, delete file
  - `handle_one_shot(filename, event)` — schedule timer, enqueue run, delete file
  - `handle_periodic(filename, event)` — schedule cron, enqueue run (don't delete)
- **Dependencies:** TASK-16.7
- **Implementation steps:**
  1. Watch `events/` dir with `notify` crate (debounced, 100ms).
  2. On startup: scan existing files, schedule accordingly. Skip immediate events created before bot started (guard against stale files).
  3. On new file: parse JSON, validate `type`, `channelId`, `text`. Reject malformed files with warning.
  4. Immediate: check `mtime > bot.startup_ts`, enqueue agent run via session manager, delete file.
  5. One-shot: parse `at` to `DateTime<FixedOffset>`. If past → delete file. Else schedule `tokio::time::sleep`.
  6. Periodic: parse cron expression with `cron` crate. Schedule via `tokio::spawn` loop with `cron::Schedule`.
  7. Format event message: `[EVENT:<filename>:<type>:<schedule_info>] <text>`.
  8. Queue limit: max 5 events per chat. If full → log warning, discard event.
  9. Support `[SILENT]` agent response → nothing posted to chat (periodic checks with no actionable results).
- **Definition of Done:** Bot responds to scheduled events in the correct chat.
- **Acceptance criteria:**
  - Write immediate event JSON → agent runs within 1 second → file deleted.
  - One-shot event 5s in future → agent runs at correct time.
  - Periodic with `*/5 * * * *` → agent runs every 5 minutes.
  - Stale immediate (mtime before startup) → deleted without running.
  - Past one-shot → deleted without running.
  - Queue full → event discarded, warning logged.
  - `[SILENT]` response → process message deleted, nothing posted.
- **Test plan:** unit tests for JSON parsing and schedule logic; integration test with temp events dir.
- **Estimated effort:** 8 hours

---

### TASK-16.10: Streaming LLM responses to Telegram

- **§SPEC:** §5.2 (Streaming contract), §24 (LlmTokenDelta)
- **Labels:** `layer/telegram`, `layer/llm`, `priority/high`
- **Description:** Wire LLM token streaming through the Telegram bot for live progress updates. Depends on the shared SSE streaming support in `duga-llm` (TASK-8.6). The `TelegramEventSink` receives `LlmTokenDelta` events and accumulates them, calling `editMessageText` to update a single Telegram message in-place as the agent generates text. Thinking tokens (Claude extended thinking) are suppressed from chat display.
- **Files affected:**
  - `crates/duga-telegram-bot/src/sink.rs` (modify — handle LlmTokenDelta)
  - `crates/duga-telegram-bot/src/formatting.rs` (modify — HTML conversion)
- **Types involved:** `LlmTokenDelta` (from `duga-events`), `TelegramEventSink`
- **Functions to implement:**
  - `TelegramEventSink::on_token_delta(text: &str)` — buffer + edit message
  - `markdown_to_telegram_html(text: &str) -> String` — convert `*bold*` → `<b>`, etc.
- **Dependencies:** TASK-8.6 (SSE streaming in LLM clients), TASK-16.6
- **Implementation steps:**
  1. On first `LlmTokenDelta`: ensure process message exists (create with "_Thinking..._" if not).
  2. Accumulate deltas into a text buffer.
  3. Convert accumulated text: `*bold*` → `<b>bold</b>`, `_italic_` → `<i>italic</i>`, `` `code` `` → `<code>code</code>`, `` ```block``` `` → `<pre>block</pre>`. Convert HTML back to Markdown first (in case LLM outputs raw HTML).
  4. Escape `&`, `<`, `>` in non-markup text before conversion.
  5. Call `editMessageText(parse_mode="HTML")` with accumulated text.
  6. When accumulated text exceeds 3800 chars: start a new process message, include "*(continued...)*" in the previous.
  7. On `AgentFinished`: the final complete text replaces the last process message. Send final result as a separate "result" message.
  8. Thinking tokens (from `content.type == "thinking"`): suppressed from chat, logged to console only.
  9. Handle edit failures: if HTML parse fails, retry as plain text.
- **Definition of Done:** Agent responses appear character-by-character in Telegram.
- **Acceptance criteria:**
  - `streaming: true` → message updates live as LLM generates.
  - `streaming: false` → single message after completion (unchanged behavior).
  - Markdown formatting renders correctly in Telegram HTML.
  - Messages > 3800 chars → split into multiple process messages.
  - HTML parse errors → graceful fallback to plain text.
  - Thinking tokens NOT shown to user.
- **Test plan:** integration test with mock SSE stream + mock Telegram API.
- **Estimated effort:** 5 hours

---

### TASK-16.11: Attachment downloading

- **§SPEC:** new
- **Labels:** `layer/telegram`, `priority/high`
- **Description:** Download photos, documents, audio, voice, video, and stickers sent by users in Telegram chats. Save to `<workspace>/<chat_id>/attachments/` with sanitized filenames (`<msg_id>_<original_name>`). Image attachments (jpg, png, gif, webp) are base64-encoded and included in the LLM request as image content blocks (for vision-capable models). Non-image files are referenced in a `<chat_attachments>` block in the user message text.
- **Files affected:**
  - `crates/duga-telegram-bot/src/attachments.rs` (new)
  - `crates/duga-telegram-bot/src/bot.rs` (modify message handler)
  - `crates/duga-runtime/src/agent.rs` (modify — inject image content into LLM request)
- **Types involved:** `TelegramAttachment`, `DownloadedFile`, `ImageContentBlock`
- **Functions to implement:**
  - `download_attachment(bot: &Bot, file_id: &str, save_path: &Path) -> Result<Attachment>`
  - `collect_attachments(msg: &Message, chat_dir: &Path) -> Vec<Attachment>`
  - `build_image_content_block(file_path: &Path) -> Result<ImageContentBlock>` — detect MIME type, base64-encode
- **Dependencies:** TASK-16.3, TASK-16.7
- **Implementation steps:**
  1. In message handler: detect `photo`, `document`, `audio`, `voice`, `video`, `video_note`, `sticker` in the Telegram message.
  2. For photos: use the largest available size (last in array).
  3. Call `bot.get_file(file_id)` → get `file_path`.
  4. Download from `https://api.telegram.org/file/bot<token>/<file_path>` via `reqwest`.
  5. Save to `<workspace>/<chat_id>/attachments/<msg_id>_<sanitized_name>`.
  6. Detect MIME type: jpg/jpeg → `image/jpeg`, png → `image/png`, gif → `image/gif`, webp → `image/webp`.
  7. For images: read file, base64-encode, build `ImageContentBlock { type: "image", mime_type, data }`.
  8. For non-images: add path to `<chat_attachments>` block in user message.
  9. Pass image content blocks to `LlmClient::chat()` alongside text messages.
  10. Respect `max_file_size_mb` config — skip oversized files, log warning.
- **Edge cases:**
  - Multiple attachments in one message → download all.
  - Download fails → log warning, skip, continue with text-only prompt.
  - Image > 20MB (base64) → skip image, mention in attachments list only.
  - No `file_path` in `get_file` response → log warning, skip.
- **Definition of Done:** Attachments are downloaded and accessible to the agent.
- **Acceptance criteria:**
  - Photo sent to bot → saved in `attachments/` + base64 in LLM prompt.
  - Document sent → saved + path referenced in prompt.
  - Vision-capable model → sees the image content.
  - No attachment → prompt unchanged.
  - Oversized file → skipped with warning.
- **Test plan:** integration test with mock Telegram file API.
- **Estimated effort:** 5 hours

---

### TASK-16.12: Bot message logging (log.jsonl + context.jsonl)

- **§SPEC:** §25 (replay), §31 (observability)
- **Labels:** `layer/telegram`, `priority/high`
- **Description:** Log all messages (user, bot, edited) to per-chat `log.jsonl` in a human-greppable format. Maintain a separate `context.jsonl` for LLM message history (SessionManager-compatible format). On each agent run, sync new messages from `log.jsonl` into `context.jsonl` so the agent sees chat history even from when the bot was offline.
- **Files affected:**
  - `crates/duga-telegram-bot/src/log.rs` (new)
  - `crates/duga-telegram-bot/src/bot.rs` (modify — log on receive)
- **Types involved:** `LogEntry` (date, ts, user, userName, displayName, text, attachments, isBot)
- **Functions to implement:**
  - `log_message(chat_id, entry: LogEntry)` — append to `log.jsonl`
  - `sync_context_from_log(session_manager, log_path) -> usize` — sync user messages to context
- **Dependencies:** TASK-16.5
- **Implementation steps:**
  1. On each incoming message: serialize `LogEntry` as JSON, append to `<chat>/log.jsonl`.
  2. Deduplicate by message timestamp (skip if `<chat_id>:<ts>` already seen in last 60s).
  3. Log edited messages too (with `(edited)` prefix).
  4. On agent run: call `sync_context_from_log()` to backfill user messages into `context.jsonl`.
  5. Bot responses are logged after sending (with `isBot: true`).
- **Definition of Done:** All chat messages are persisted and available for agent context.
- **Acceptance criteria:**
  - User message → logged to `log.jsonl` with correct fields.
  - Bot response → logged with `isBot: true`.
  - Edited message → logged with `(edited)` prefix.
  - Agent run sees messages from before bot restart.
  - No duplicate entries within 60s window.
- **Test plan:** unit tests for log format, deduplication, context sync.
- **Estimated effort:** 3 hours

---

### TASK-16.13: Telegram bot integration tests and docs

- **§SPEC:** §25 (replay), §31 (observability), §32 (config)
- **Labels:** `layer/telegram`, `layer/testing`, `priority/high`
- **Description:** Add tests and documentation for the full Telegram bot. Tests use mock Telegram API, mock LLM, and mock event files. No live Telegram API required.
- **Files affected:**
  - `crates/duga-telegram-bot/tests/bot_flow.rs` (new)
  - `crates/duga-telegram-bot/tests/events_integration.rs` (new)
  - `crates/duga-telegram-bot/tests/attachments.rs` (new)
  - `README.md` (Telegram bot setup section)
  - `docs/telegram-bot.md` (new)
- **Types involved:** mock Telegram adapter, `CapturingEventSink`, `MockLlm`, `MockTool`, test event JSON fixtures
- **Functions to implement:** test helpers for simulated updates, outbound messages, file downloads
- **Dependencies:** TASK-16.1 through TASK-16.12
- **Implementation steps:**
  1. Abstract Telegram send/receive behind a trait for tests (`TelegramApi`).
  2. Simulate: DM message, group mention, reply to bot, unauthorized chat, `/stop` command.
  3. Simulate tool confirmation flow: approve, deny, timeout.
  4. Simulate scheduled events: immediate, one-shot, periodic.
  5. Simulate attachment download and prompt injection.
  6. Verify `log.jsonl` and `context.jsonl` output.
  7. Document setup: token env var, Docker sandbox creation, allowed chat IDs, safety confirmations, events, skills, memory.
- **Definition of Done:** Telegram bot behavior is covered without live API credentials.
- **Acceptance criteria:**
  - All tests pass offline (`cargo test -p duga-telegram-bot`).
  - README contains bot setup, Docker sandbox, and configuration guide.
  - docs/telegram-bot.md explains: triggers (DM, mention, reply), commands, events system, safety model, memory, skills, limitations.
- **Test plan:** `cargo test -p duga-telegram-bot`.
- **Estimated effort:** 6 hours

---

## Summary Table

| Task | Name | Est. Hours |
|------|------|------------|
| TASK-16.1 | Extract shared runtime — providers, tools, agent, **+MEMORY.md, +SKILL.md, +confirmation hooks** | 12 |
| TASK-16.2 | Telegram config struct + validation | 4 |
| TASK-16.2a | Docker sandbox executor in `duga-sandbox` (shared) | 6 |
| TASK-16.3 | `duga-telegram-bot` crate + teloxide/grammy startup | 5 |
| TASK-16.4 | Telegram auth + update handling (DM, mention, reply, commands, callbacks) | 6 |
| TASK-16.5 | Per-chat session manager + ChannelQueue + context persistence | 6 |
| TASK-16.6 | TelegramEventSink with live message editing | 6 |
| TASK-16.7 | Run AgentLoop per message (with memory, skills, metadata in prompt) | 6 |
| TASK-16.8 | Safety controls + tool confirmation (Telegram inline keyboard UI) | 5 |
| TASK-16.9 | Scheduled events (immediate, one-shot, periodic) | 8 |
| TASK-16.10 | Streaming LLM → Telegram live editing | 5 |
| TASK-16.11 | Attachment downloading (photos, docs, audio, video) | 5 |
| TASK-16.12 | Bot message logging (log.jsonl + context.jsonl) | 3 |
| TASK-16.13 | Integration tests and docs | 6 |
| **Total** | | **83 hours** |

### Shared infrastructure (implemented in other epics, used by EPIC-16)

These tasks are in other epics but are **blocking dependencies** for the Telegram bot features above:

| Task | Epic | Description | Used by EPIC-16 |
|------|------|-------------|-----------------|
| TASK-8.6 | EPIC-8 | SSE streaming in LLM clients (`LlmTokenDelta`) | TASK-16.10 (Streaming) |
| TASK-8.7 | EPIC-8 | Real token counting (tiktoken-rs, per-provider) | Memory budget accuracy |
| TASK-6.x | EPIC-6 | Memory system (token budget + compression) | TASK-16.1 (MEMORY.md integration) |

### Features shared with EPIC-17 (TUI)

These features are implemented in `duga-runtime` or `duga-sandbox` and used by both frontends:

| Feature | Location | Used by Telegram | Used by TUI |
|---------|----------|-----------------|-------------|
| Provider resolution + LLM building | `duga-runtime` | TASK-16.7 | TASK-17.6 |
| Tool registration (builtins + WASM plugins) | `duga-runtime` | TASK-16.7 | TASK-17.6 |
| MEMORY.md persistent context | `duga-runtime` | TASK-16.7 | TASK-17.6 |
| SKILL.md loading + prompt injection | `duga-runtime` | TASK-16.7 | TASK-17.6 |
| Tool confirmation hooks | `duga-runtime` | TASK-16.8 | TASK-17.7 |
| Docker sandbox executor | `duga-sandbox` | TASK-16.2a | TASK-17.2 (via config) |
| SSE streaming (`LlmTokenDelta`) | `duga-llm` | TASK-16.10 | TASK-17.5 |
| Real token counting | `duga-llm` | Memory budget | Memory budget |
