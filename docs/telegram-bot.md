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
  context_window_size: 50
  max_context_tokens: 12000
  summarizer: semantic
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

### Data Directory Access

The `read`, `write`, and `edit` tools can access files in the bot's data
directory (`telegram.data_dir`) in addition to the workspace. When a path
doesn't exist in the workspace, the tools fall back to checking the data
directory.

**Resolution order** (for all three tools):
1. Workspace (`~/duga-workspace`) — always checked first
2. Data directory (`./data`) — fallback for relative paths only

**Write behavior**: new root-level files go to workspace by default.
Files in subdirectories follow whichever location has the matching parent
directory (e.g., `logs/out.txt` goes to `data/logs/out.txt` if `data/logs/` exists
but the workspace has no `logs/` dir).

**Security**: files in the data directory are subject to the same
symlink and path-traversal checks as workspace files. Tool confirmation
(if enabled for `edit`/`write`) still applies regardless of which directory
the file is in.

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

### Mixed Text + Attachment Handling

Messages that contain both text AND attachments (e.g., a photo with a caption) are no longer treated as text-only. The bot now:
1. Downloads all supported attachments from the message
2. Generates a textual summary like:
   ```
   [Attachments:
   - photo_718.jpg (photo, 1.2 MB)
   - report.pdf (document, 234 KB)]
   ```
3. Prepends the summary to the user's text before sending it to the agent

This means the agent can always `read` downloaded file paths to access attachment content, regardless of whether the message also has text.

## Sending Files (Bot → User)

The agent can send workspace files as proper Telegram attachments using the `send_file` tool. This is useful for delivering generated files (SVGs, PDFs, code files, screenshots, etc.) instead of dumping raw content into text messages.

### File Type Detection

The tool automatically selects the correct Telegram send method based on file extension:

| Extension | Send Method | Notes |
|-----------|-------------|-------|
| `.jpg`, `.jpeg`, `.png`, `.webp` | `sendDocument` (default) or `sendPhoto` (with `as_photo: true`) | Images are sent as documents by default for reliability |
| `.mp3`, `.flac`, `.m4a`, `.wav` | `sendAudio` | |
| `.mp4`, `.mov`, `.webm`, `.avi`, `.mkv` | `sendVideo` | |
| `.gif` | `sendAnimation` | |
| `.ogg` | `sendVoice` | |
| `.svg`, `.pdf`, `.zip`, `.json`, `.txt`, `.rs`, `.py`, `.csv`, `.html`, others | `sendDocument` | Catch-all for non-media files |

### Tool Arguments

| Argument | Type | Required | Description |
|----------|------|----------|-------------|
| `path` | string | Yes | Workspace path to the file to send |
| `caption` | string | No | Text caption (up to 1024 chars) |
| `as_photo` | boolean | No | Send image files as photo instead of document |
| `label` | string | No | Human-readable step description (shown in UI) |

### Configuration

Configure `send_file` behavior under `telegram.send_file`:

```yaml
telegram:
  send_file:
    enabled: true
    max_file_size_mb: 50      # Default 50 MB (Telegram Bot API limit)
    allowed_extensions:
      - .svg
      - .png
      - .jpg
      - .pdf
      - .txt
      - .json
      - .rs
      - .py
      - .html
      - .md
      - .csv
      - .zip
      - .mp3
      - .mp4
```

- `enabled`: Whether the tool is available (default: `true`)
- `max_file_size_mb`: Maximum file size (default: 50 MB for standard Bot API, up to 2000 MB with a Local Bot API Server)
- `allowed_extensions`: If non-empty, restricts which file types the bot can send. Empty means all types are allowed.

### Security

- **Path traversal protection**: `send_file` validates that the requested path is within the workspace using the same `Workspace::resolve()` mechanism used by `read`/`write`/`edit` tools
- **File size limits**: Enforced at the tool level to prevent large uploads from consuming bandwidth
- **Extension allowlist**: Optional restrict-on-send policy via `allowed_extensions`
- **No shell execution**: The tool only reads files and sends them via the Telegram API

### Usage Example

The agent can use `send_file` like this:

```json
{
  "tool": "send_file",
  "raw_args": {
    "path": "chessboard.svg",
    "caption": "Here's your chessboard!",
    "as_photo": false
  }
}
```

The bot will send the file as a proper downloadable attachment with the caption below it.

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
│   - Pass data_dir as        │
│     aux_roots to tools      │
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
- **Standard Bot API 50 MB file limit** — The `send_file` tool enforces a 50 MB limit by default. If you run a [Local Bot API Server](https://core.telegram.org/bots/api#using-a-local-bot-api-server), you can increase `max_file_size_mb` up to 2000 MB.
- **No streaming file uploads** — The file is fully read into memory before sending. Very large files may consume significant memory.

## Troubleshooting

| Symptom | Likely cause |
|---------|-------------|
| Bot doesn't start | `TELEGRAM_BOT_TOKEN` not set or invalid |
| "telegram config section is required" | Config is missing the `telegram:` block |
| Messages ignored | Chat ID not in `allowed_chat_ids` |
| Bot responds slowly | Provider API latency; try streaming mode |
| Attachment fails | File exceeds `max_file_size_mb` limit |
