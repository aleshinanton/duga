# duga Telegram Bot

A Telegram bot frontend for the duga agent runtime. Supports text tasks, command handling, inline confirmation callbacks, scheduled events, attachment downloading, and per-chat sessions.

## Setup

### 1. Create a Telegram bot

1. Open Telegram and chat with [@BotFather](https://t.me/BotFather).
2. Send `/newbot` and follow the prompts.
3. Copy the bot token (e.g., `123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11`).

### 2. Configure duga

Create `duga.yaml`:

```yaml
provider: "openai"
model: "gpt-4o-mini"
agent:
  limits:
    max_steps: 50
    max_tool_calls: 100
    max_runtime: 10m
    retry_on_error: 2
  features:
    streaming: true
  output:
    max_stdout_bytes: 4096
    max_stderr_bytes: 4096
    max_combined_bytes: 8192
  think:
    max_calls: 8
    max_tokens: 4096
sandbox:
  mode: "capability"
  timeout: 30s
  allowed_binaries:
    - /usr/bin/cat
    - /usr/bin/ls
    - /usr/bin/grep
    - /usr/bin/find
    - /usr/bin/head
    - /usr/bin/tail
workspace:
  root: "~/duga-workspace"
environment:
  allowed:
    - HOME
memory:
  max_tokens: 4096
  compress_at_ratio: 0.8
plugins:
  dir: "./plugins"
  modules: []
frontend:
  progress_mode: "summary"
  confirmation_timeout: 60s
telegram:
  token_env: "TELEGRAM_BOT_TOKEN"
  allowed_chat_ids:
    - 123456789
  send_tool_events: true
  send_final_only: false
  require_confirmation_for:
    - shell
    - edit
    - write
  events_dir: "./events"
  skills_dir: "./skills"
  data_dir: "./data"
  attachments:
    enabled: true
    max_file_size_mb: 20
    download_dir: "attachments"
```

### 3. Set environment variables

```bash
export TELEGRAM_BOT_TOKEN="123456:ABC-DEF..."
export OPENAI_API_KEY="sk-..."
# Or for Anthropic:
export ANTHROPIC_API_KEY="sk-ant-..."
```

### 4. Run the bot

```bash
cargo run --release -p duga-telegram-bot -- --config duga.yaml
```

## Triggers

The bot responds to messages in three ways:

| Trigger | Context | Example |
|---------|---------|---------|
| **DM** | Direct message to the bot | `do a thing` |
| **Mention** | Group message mentioning `@botusername` | `@duga_bot do a thing` |
| **Reply** | Reply to a bot message | (reply) `try again` |

## Commands

| Command | Description |
|---------|-------------|
| `/start` | Start the bot |
| `/help` | Show available commands |
| `/stop` | Cancel the current task |
| `/status` | Show current task status |
| `/memory` | Show current memory state |
| `/skills` | List available skills |
| `/events` | Show recent events |

## Safety Model

### Chat Authorization

Only chats listed in `telegram.allowed_chat_ids` can interact with the bot. Set `telegram.allow_all_chats_for_dev: true` for development (not recommended in production).

### Tool Confirmation

Risky tools (shell, edit, write) can require inline confirmation via Telegram buttons. Configure with:

```yaml
telegram:
  require_confirmation_for:
    - shell
    - edit
    - write
```

When a confirmation is required, the bot sends an inline keyboard with [✅ Approve] and [❌ Deny] buttons. The timeout defaults to 60 seconds (configurable via `frontend.confirmation_timeout`). Timeouts are treated as denial.

## Scheduled Events

Drop JSON files into the `events/` directory to trigger agent runs:

### Immediate

```json
{"type": "immediate", "channelId": -123456789, "text": "New item arrived"}
```

### One-shot

```json
{"type": "one_shot", "channelId": -123456789, "text": "Reminder", "at": "2026-05-15T09:00:00+01:00"}
```

### Periodic (cron)

```json
{"type": "periodic", "channelId": -123456789, "text": "Check inbox", "schedule": "0 9 * * 1-5", "timezone": "Europe/Lisbon"}
```

Immediate and one-shot events are deleted after triggering. Periodic events persist until manually removed.

## Memory and Skills

### MEMORY.md

Create `MEMORY.md` in the workspace root or per-chat data directory. Content is injected into the system prompt as persistent memory.

### Skills

Create `skills/<name>/SKILL.md` files with optional YAML frontmatter:

```markdown
---
name: code-review
description: Review code for best practices
---

# Code Review Skill

Use this skill to review code changes...
```

Channel-level skills (`data/<chat_id>/skills/`) override workspace-level skills on name collision.

## Attachments

The bot downloads supported attachments (photos, documents, audio, voice, video, stickers) into per-chat directories. Downloaded files are referenced by path in the user message so the agent can access them.

Configuration:
```yaml
telegram:
  attachments:
    enabled: true
    max_file_size_mb: 20  # Telegram's max is 50 MB
    download_dir: "attachments"
```

Image-to-LLM support depends on a future multimodal content type in `duga-types`/`duga-llm`.

## Logging and Replay

- **log.jsonl** — All messages (incoming and outgoing) are logged per chat in `data/<chat_id>/log.jsonl`.
- **session.jsonl** — Full agent event stream per run, written for replay. Also serves as conversation context persistence — the bot loads previous messages from the last `LlmRequest` event on each new message, so it remembers the full conversation history across messages.
- **context.jsonl** — User messages synced for LLM context on restart.

## Process Message Rendering

During execution, the bot edits a single live "process message" with:
- **Step descriptions** — Each tool call shows a human-readable label provided by the LLM (e.g., `✅ shell: ls -la`). If the LLM doesn't provide a label, only the tool name is shown (`✅ shell`).
- **Single line per tool** — Start labels are replaced in-place by finish labels, so each tool call produces exactly one line (not a start/finish pair).
- **Collapsible blocks** — When step history exceeds 5 labels or the final answer exceeds 300 characters, content is wrapped in Telegram `<blockquote expandable>` tags with a "Show more" toggle.
- **Streaming delta** — LLM token output is shown in real-time as a code block, truncated to 200 characters.

## Architecture

```
┌─────────────────────────────┐
│    Telegram API             │
│  (teloxide long polling)    │
└──────────┬──────────────────┘
           │ Update
┌──────────▼──────────────────┐
│   bot.rs                    │
│   - Auth check              │
│   - Command routing         │
│   - Attachment handling     │
└──────────┬──────────────────┘
           │
┌──────────▼──────────────────┐
│   session.rs                │
│   - Per-chat DashMap        │
│   - One run per chat        │
│   - Cancellation            │
└──────────┬──────────────────┘
           │
┌──────────▼──────────────────┐
│   runtime.rs                │
│   - Load history from       │
│     session.jsonl           │
│   - Wire AgentLoop          │
│   - Event bridges           │
│   - JSONL sinks             │
│   - System prompt with      │
│     env context + tools     │
└──────────┬──────────────────┘
           │
┌──────────▼──────────────────┐
│   render.rs                 │
│   - Live process message    │
│   - Step descriptions       │
│   - Collapsible blocks      │
│   - Single line per tool    │
└──────────┬──────────────────┘
           │
┌──────────▼──────────────────┐
│   duga-runtime              │
│   - Provider resolution     │
│   - Tool registration       │
│   - Memory/skills loading   │
│   - Confirmation middleware  │
└─────────────────────────────┘
```

## Limitations

- **No multimodal image support yet** — Attachments are downloaded but not interpreted by the LLM.
- **Single run per chat** — A second task is rejected while one is active.
- **No streaming in group chats** — Live message editing is DM-only.
- **Think budget is limited** — The `think` tool has a per-run budget of 8 calls / 4096 tokens by default. Configure via `agent.think.max_calls` and `agent.think.max_tokens`.

## Troubleshooting

| Symptom | Likely cause |
|---------|-------------|
| Bot doesn't start | `TELEGRAM_BOT_TOKEN` not set or invalid |
| "telegram config section is required" | Config is missing the `telegram:` block |
| Messages ignored | Chat ID not in `allowed_chat_ids` |
| Bot responds slowly | Provider API latency; try streaming mode |
| Attachment fails | File exceeds `max_file_size_mb` limit |
