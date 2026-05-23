# EPIC-28: File Attachment Sending (Bot → User)

**§SPEC:** §5 (Final Answer Formatting), §24 (Telegram Bot Rendering), §11 (Built-in Tools)
**Labels:** `epic/telegram`, `epic/tools`
**Crates:** `duga-telegram-bot`, `duga-runtime`, `duga-tools-builtin` (optional)

## Goal

Give the Telegram bot the ability to **send files as proper Telegram attachments** (documents, photos, audio) back to the user. Currently the bot can download incoming attachments (`attachments.rs`) but has no tool to send files out — when the user asks for an SVG, the agent can only dump raw SVG markup into a text message instead of sending a downloadable file.

## Motivation

- The user conversation from 2026-05-22 shows the problem clearly: the agent created `chessboard.svg` but could only reply with raw SVG text, even after the user explicitly asked "send me svg", "send as file", and "can you send me attachment?"
- Telegram natively supports sending files as documents, photos, audio, video, and animations — the bot should leverage this
- Other use cases: sending generated reports (PDF), sharing code files (`.rs`, `.py`, `.json`), sending screenshots or rendered charts, delivering audio summaries
- The fix requires no changes to `duga-types` or LLM providers — it's purely a Telegram transport concern

---

## Implementation Plan

### TASK-28.1: Create `SendFileTool` — a tool for sending files to the Telegram chat

- **Labels:** `layer/telegram`, `priority/high`
- **Description:** Implement a new tool `send_file` that reads a file from the workspace and sends it as a Telegram attachment to the current chat. The tool should:
  - Accept a `path` parameter (the workspace file path to send)
  - Accept an optional `caption` parameter (text to accompany the file, up to 1024 chars)
  - Accept an optional `as_photo` boolean (if true, attempt to send as a photo instead of document for image formats)
  - Auto-detect the file MIME type by extension:
    - `.jpg`, `.jpeg`, `.png`, `.webp` → send as photo (`send_photo`) if `as_photo` is true or if caption is short, else as document via `send_document`
    - `.mp3`, `.flac`, `.m4a`, `.wav` → send as audio (`send_audio`)
    - `.mp4`, `.mov`, `.webm`, `.avi`, `.mkv` → send as video (`send_video`)
    - `.gif` → send as animation (`send_animation`)
    - `.ogg` → send as voice if file is small (< 1MB), else as document
    - Everything else (`.svg`, `.pdf`, `.zip`, `.json`, `.txt`, `.rs`, `.py`, `.csv`, `.html`, etc.) → send as document (`send_document`)
  - Read the file from the workspace filesystem
  - Stream the file content to Telegram via `teloxide::Bot::send_document()` (or the appropriate method)
  - Return a success message with the file_id and file name on success, or an error description on failure
  - Enforce a configurable max send file size (default 50 MB, matching Telegram's limit)
- **Files affected:**
  - `crates/duga-telegram-bot/src/send_file.rs` (new)
  - `crates/duga-telegram-bot/src/lib.rs` (export new module)
- **Types involved:** `SendFileArgs` (with JsonSchema derive), `SendFileTool`
- **Functions to implement:**
  - `SendFileTool::new(bot: Bot, chat_id: ChatId) -> Self`
  - `impl Tool for SendFileTool`
  - `fn mime_type_from_extension(path: &Path) -> &str`
  - `fn send_method_for_file(path: &Path, as_photo: bool) -> SendMethod`
- **Dependencies:** TASK-28.3
- **Implementation steps:**
  1. Create `SendFileArgs` struct with `path` (String), optional `caption` (String), optional `as_photo` (bool)
  2. Implement the `Tool` trait with schema validation
  3. In `execute`, read the file, determine the best send method, call the appropriate `teloxide` API
  4. Handle errors: file not found, file too large, Telegram API errors, rate limiting
  5. Return a human-readable result message (e.g. "Sent `chessboard.svg` as document (9.5 KB)")
  6. Add tests using a mock bot
- **Definition of Done:** The agent can invoke `send_file` and the file appears as a proper Telegram attachment in the chat.
- **Acceptance criteria:**
  - Agent calls `send_file` with a path → file arrives as a downloadable document/photo/audio in chat
  - SVG files are sent as documents (Telegram doesn't render SVGs inline via sendPhoto)
  - Image files (jpg/png) can be sent as photos when `as_photo: true`
  - Non-existent file path returns a clear error message
  - File exceeding max size returns a clear error message
  - Caption text appears below the file when provided
- **Test plan:** Unit tests with mock Telegram API; manual smoke test with SVG creation
- **Estimated effort:** 6 hours

### TASK-28.2: Add `send_file` config section with size limits and allowed paths

- **Labels:** `layer/config`, `priority/high`
- **Description:** Add an optional `send_file` configuration section under `telegram:` to control file sending behavior:
  ```yaml
  telegram:
    send_file:
      enabled: true
      max_file_size_mb: 50
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
  - `enabled`: whether the `send_file` tool is available to the agent (default: true)
  - `max_file_size_mb`: maximum file size for outgoing attachments (default: 50, Telegram's limit; up to 2000 with local Bot API server)
  - `allowed_extensions`: if non-empty, restricts which file types the bot can send (empty = allow all)
- **Files affected:**
  - `crates/duga-config/src/config.rs`
- **Types involved:** `TelegramSendFileConfig`
- **Dependencies:** TASK-28.1
- **Implementation steps:**
  1. Add `TelegramSendFileConfig` struct with `enabled`, `max_file_size_mb`, `allowed_extensions` fields
  2. Add `send_file` field to `TelegramConfig` with default
  3. Add validation: `max_file_size_mb` must be > 0 and <= 2000
  4. Wire the config into the `SendFileTool` constructor
- **Definition of Done:** Bot startup reads `telegram.send_file` config; invalid values produce validation errors.
- **Acceptance criteria:**
  - Default config allows all file types up to 50 MB
  - `max_file_size_mb: 0` produces a validation error
  - `max_file_size_mb: 5000` produces a validation error
  - `allowed_extensions: [".svg"]` restricts the tool to only SVG files
- **Test plan:** Unit tests for config parsing and validation
- **Estimated effort:** 2 hours

### TASK-28.3: Register `SendFileTool` in the Telegram runtime

- **Labels:** `layer/telegram`, `priority/high`
- **Description:** Wire the `SendFileTool` into the Telegram bot's tool dispatcher so the LLM agent can use it. The tool must be registered after `build_dispatcher` but only for Telegram sessions (not CLI/TUI).
- **Files affected:**
  - `crates/duga-telegram-bot/src/runtime.rs`
  - `crates/duga-telegram-bot/src/session.rs` (pass `bot` and `chat_id` through)
- **Types involved:** `SendFileTool`, `ErasedExecutor`, `ToolDispatcher`
- **Functions to modify:**
  - `TelegramRuntime::run_task_for_chat()` — register the `SendFileTool` in the dispatcher
- **Dependencies:** TASK-28.1, TASK-28.2
- **Implementation steps:**
  1. In `run_task_for_chat`, after `build_dispatcher`, create a `SendFileTool` with the bot handle and chat_id
  2. Register it as an erased tool on the dispatcher via `dispatcher.register_erased(ErasedTool::erase(send_file_tool))`
  3. Pass the `TelegramSendFileConfig` to the tool constructor for size/extension enforcement
- **Implementation notes:**
  - The dispatcher currently wraps the tool registry in an `Arc<ToolDispatcher>`. Since we need to register a new tool after the Arc is created, modify `build_dispatcher` to accept a `Vec<Box<dyn ErasedTool>>` of extra tools, or alternatively have `run_task_for_chat` build a separate dispatcher that includes Telegram-specific tools.
  - *Preferred approach:* Add a `register_telegram_tools` function that takes the `Arc<ToolDispatcher>` and registers Telegram-specific tools after the shared ones.
- **Definition of Done:** The `send_file` tool appears in the agent's tool list and can be invoked in Telegram sessions.
- **Acceptance criteria:**
  - Agent's system prompt includes `send_file` in the available tools list
  - Tool is NOT available in CLI or TUI mode (only Telegram)
  - Tool call with valid args sends the file to the correct chat
- **Test plan:** Integration test with mock bot; verify tool schema includes `send_file`
- **Estimated effort:** 3 hours

### TASK-28.4: Update system prompt to inform the agent about `send_file`

- **Labels:** `layer/telegram`, `layer/llm`, `priority/medium`
- **Description:** Add a note in the Telegram system prompt (in `runtime.rs`) telling the agent it has a `send_file` tool available and how to use it effectively. This helps the agent naturally choose to send files when the user requests them.
- **Files affected:**
  - `crates/duga-telegram-bot/src/runtime.rs`
- **Dependencies:** TASK-28.3
- **Implementation steps:**
  1. Add a line to the system prompt: "You can use `send_file` to send files (documents, images, audio, video) to the user as proper Telegram attachments."
  2. Optionally add guidance: "When the user asks for a file, use `send_file` instead of dumping file content into a text message."
  3. Keep the prompt concise — the tool's own schema description will provide the details
- **Definition of Done:** Agent's system prompt informs it about the `send_file` capability.
- **Acceptance criteria:**
  - System prompt contains a `send_file` tool reference
  - No change to non-Telegram runtimes
- **Test plan:** Inspect system prompt text in test
- **Estimated effort:** 1 hour

### TASK-28.5: Handle mixed text+attachment incoming messages

- **Labels:** `layer/telegram`, `priority/medium`
- **Description:** Fix the current binary split in `handle_message` (`bot.rs`) where a message with both text AND a photo is treated as text-only, discarding the attachment. Instead, download attachments for ALL messages and prepend a summary of downloaded files to the user's text prompt.
- **Files affected:**
  - `crates/duga-telegram-bot/src/bot.rs`
  - `crates/duga-telegram-bot/src/attachments.rs`
- **Functions to modify:**
  - `handle_message()` — always download attachments, even when text is present
  - New function: `summarize_attachments(files: &[DownloadedFile]) -> String`
- **Dependencies:** TASK-28.1 (conceptual dependency — follows the same file-attachment pipeline)
- **Implementation steps:**
  1. In `handle_message`, remove the `if text.is_some() { … } else { handle_attachment_message() }` split
  2. Always call `download_attachments()` for messages that have supported attachment types
  3. Generate a textual attachment summary like:
     ```
     [Attachments:
     - photo_718.jpg (photo, 1.2 MB)
     - report.pdf (document, 234 KB)]
     ```
  4. Prepend this summary to the user's text so the agent sees the file paths
  5. If there's no text and no attachments, fall back to the current "no supported attachments" message
- **Implementation notes:**
  - This is a smaller change than full multimodal `ContentBlock` support but immediately useful
  - The agent can use `read` on the file paths to access attachment content
  - Future EPIC-29 can add native `ContentBlock::Image` for vision model support
- **Definition of Done:** A message with text + photo results in both text reaching the agent AND the photo being downloaded and referenced.
- **Acceptance criteria:**
  - Text-only message → unchanged behavior
  - Photo-only message → photo downloaded, path appears in prompt
  - Text + photo → both text prompt and photo path appear in prompt, agent can `read` the photo path
  - Text + document → document downloaded, path in prompt
- **Test plan:** Unit tests with mock bot and mock messages
- **Estimated effort:** 3 hours

### TASK-28.6: Tests for file attachment sending

- **Labels:** `layer/testing`, `priority/high`
- **Description:** Add comprehensive tests for the new file attachment sending functionality:
  - Unit tests for `SendFileTool` with a mock bot
  - Unit tests for MIME type detection
  - Unit tests for config validation (max_file_size, allowed_extensions)
  - Integration test: agent creates a file then uses `send_file` to deliver it
  - Regression test: existing attachment downloading still works
  - Test that `send_file` is NOT available in CLI/TUI modes
- **Files affected:**
  - `crates/duga-telegram-bot/tests/bot_flow.rs`
  - `crates/duga-telegram-bot/tests/send_file.rs` (new)
- **Dependencies:** TASK-28.1 through TASK-28.5
- **Implementation steps:**
  1. Create a mock Telegram bot that captures `send_document`/`send_photo` calls
  2. Test that valid files are sent with correct MIME type
  3. Test that non-existent files return errors
  4. Test that oversized files are rejected
  5. Test `allowed_extensions` filtering
  6. Test mixed text+attachment message handling
- **Definition of Done:** All attachment sending features have test coverage.
- **Acceptance criteria:**
  - `cargo test -p duga-telegram-bot` passes all new tests
  - No regressions in existing tests
- **Test plan:** Run `cargo test -p duga-telegram-bot`
- **Estimated effort:** 4 hours

### TASK-28.7: Update Telegram bot documentation

- **Labels:** `layer/docs`, `priority/medium`
- **Description:** Update `docs/telegram-bot.md` and `backlog/epic-16-telegram-bot.md` to document the new `send_file` capability:
  - How the agent can send files via the `send_file` tool
  - Configuration options (`telegram.send_file.enabled`, `max_file_size_mb`, `allowed_extensions`)
  - Supported file types and how they're mapped to Telegram attachment types
  - Limitations (standard Bot API 50 MB limit vs Local Bot API Server 2000 MB limit)
  - How incoming attachments are now injected into prompts even when text is present (TASK-28.5)
- **Files affected:**
  - `docs/telegram-bot.md`
  - `docs/architecture.md` (if "Tool List" section exists)
- **Dependencies:** TASK-28.5
- **Definition of Done:** Documentation covers sending and receiving attachments.
- **Acceptance criteria:**
  - Clear explanation of the `send_file` tool, its arguments, and constraints
  - Example YAML config with commentary
  - Note about the 50 MB vs 2000 MB limit
- **Test plan:** Review rendered Markdown
- **Estimated effort:** 2 hours

---

## Summary Table

| Task | Name | Est. Hours |
|------|------|------------|
| TASK-28.1 | Create `SendFileTool` | 6 |
| TASK-28.2 | Add `send_file` config section | 2 |
| TASK-28.3 | Register `SendFileTool` in Telegram runtime | 3 |
| TASK-28.4 | Update system prompt for `send_file` | 1 |
| TASK-28.5 | Handle mixed text+attachment incoming messages | 3 |
| TASK-28.6 | Tests for file attachment sending | 4 |
| TASK-28.7 | Update documentation | 2 |
| **Total** | | **21 hours** |

---

## Files Affected

| File | Change |
|---|---|
| `crates/duga-telegram-bot/src/send_file.rs` | New — `SendFileTool` implementation |
| `crates/duga-telegram-bot/src/lib.rs` | Export `send_file` module |
| `crates/duga-telegram-bot/src/runtime.rs` | Register `SendFileTool` in dispatcher; update system prompt |
| `crates/duga-telegram-bot/src/bot.rs` | Fix mixed text+attachment message handling |
| `crates/duga-telegram-bot/src/attachments.rs` | Add `summarize_attachments()` helper |
| `crates/duga-config/src/config.rs` | Add `TelegramSendFileConfig` |
| `crates/duga-telegram-bot/tests/send_file.rs` | New — test file |
| `crates/duga-telegram-bot/tests/bot_flow.rs` | Update — add attachment tests |
| `docs/telegram-bot.md` | Add `send_file` documentation |
| `backlog/00-overview.md` | Add EPIC-28 entry to summary table |

## Security

- **File path traversal**: `SendFileTool` must validate that the requested path is within the workspace (reuse `Workspace::resolve`)
- **File size limits**: Enforced at the tool level to prevent large file uploads from consuming bandwidth
- **Extension allowlist**: Optional restrict-on-send policy via `allowed_extensions` — complements the existing download-side controls
- **No shell execution**: The tool only reads files and sends them via Telegram API — no command execution

## Future Work (EPIC-29)

- Native multimodal `ContentBlock::Image` in `duga-types` for vision model support
- Base64 image encoding from downloaded attachments → LLM vision APIs
- OpenAI/Anthropic client serialization of image content blocks
- Session history persistence for image blocks
