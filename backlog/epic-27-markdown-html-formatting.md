# EPIC-27: Markdown → Telegram HTML Conversion for Final Messages

**§SPEC:** §5 (Final Answer Formatting), §24 (Telegram Bot Rendering)
**Labels:** `epic/telegram`, `epic/formatting`
**Crates:** `duga-telegram-bot`

## Goal

When the LLM produces a final answer with Markdown formatting (bold, italic, code blocks, links, lists, etc.), the raw Markdown is currently sent as plain text to Telegram — making the output look messy and unformatted. Convert Markdown into Telegram-compatible HTML so formatting is rendered properly.

## Motivation

- The LLM often emits Markdown in final answers (headings, bold/italic, inline code, fenced code blocks with language hints, lists, links, blockquotes)
- Telegram's `sendMessage` supports a limited set of HTML tags (`<b>`, `<i>`, `<code>`, `<pre>`, `<a>`, `<blockquote>`, `<s>`)
- Currently `format_final_message` only does plain-text HTML escaping — no Markdown parsing
- The "short message" path (≤300 chars) doesn't set `ParseMode::Html`, so Telegram treats everything as plain text
- Code blocks with generic types (`Foo<T>`) or logic symbols (`&&`, `||`, `<`, `>`) cause Telegram 400 errors unless HTML entities are escaped inside code/pre tags
- Untrusted third-party data (ingested via tools like web search) could inject malicious HTML via `Event::Html` — indirect prompt injection vector

---

## Implementation Plan

### TASK-27.1: Add `pulldown-cmark` dependency

- **Labels:** `layer/deps`, `priority/critical`
- **Description:** Add `pulldown-cmark = { workspace = true }` to `crates/duga-telegram-bot/Cargo.toml` (already declared in workspace root).
- **Files affected:**
  - `crates/duga-telegram-bot/Cargo.toml`

### TASK-27.2: Implement `markdown_to_telegram_html` in `formatting.rs`

- **Labels:** `layer/telegram`, `priority/critical`
- **Description:** Refactor `format_final_message` to delegate to a new `markdown_to_telegram_html` function instead of plain `escape_html`. This function:
  - Parses Markdown with `pulldown-cmark` (strikethrough + table extensions enabled)
  - Maps supported elements to Telegram HTML tags:
    - **bold** / headings → `<b>` / `</b>`
    - *italic* → `<i>` / `</i>`
    - ~~strikethrough~~ → `<s>` / `</s>`
    - inline code → `<code>` / `</code>`
    - fenced code blocks → `<pre><code class="language-[lang]">` / `</code></pre>` (captures language identifier for native Telegram syntax highlighting)
    - blockquotes → `<blockquote>` / `</blockquote>`
    - links → `<a href="url">` / `</a>`
    - list items → Unicode bullet (•) — Telegram has no list tags
  - Strips unsupported elements (tables, horizontal rules, nested lists) to plain text
  - **Global HTML escaping**: Escapes `<`, `>`, `&` across all text nodes *including inside inline code and preformatted code blocks* to prevent Telegram 400 errors on code containing generic types or logic symbols
  - **Safe HTML Event handling**: Escapes raw `Event::Html` blocks instead of passing them through unescaped (mitigates indirect prompt injection)
  - Trims trailing whitespace
- **Files affected:**
  - `crates/duga-telegram-bot/src/formatting.rs`

### TASK-27.3: Fix `render.rs` short-message path to use `ParseMode::Html`

- **Labels:** `layer/telegram`, `priority/critical`
- **Description:** The "short message" path (≤300 chars) in `finalize_process_message` currently sends formatted HTML without `ParseMode::Html`, causing Telegram to treat it as plain text. Fix it to set `ParseMode::Html`.
- **Constraint:** The `edit_process_message` streaming-update path intentionally uses plain text and must **not** use `ParseMode::Html` — add an inline comment explaining this.
- **Files affected:**
  - `crates/duga-telegram-bot/src/render.rs`

### TASK-27.4: Security — mitigate indirect prompt injection via `Event::Html`

- **Labels:** `layer/telegram`, `priority/medium`, `security`
- **Description:** The LLM may ingest untrusted third-party data via tools (web searches, API calls, etc.). Raw `Event::Html` elements from the Markdown parser must be safely escaped rather than passed through unescaped. This prevents malicious external payloads from injecting unauthorized HTML elements (e.g., fake system links or phishing anchors) into the final user output.
- **Covered by:** TASK-27.2 (global HTML escaping + Event::Html sanitization)
- **Files affected:**
  - `crates/duga-telegram-bot/src/formatting.rs`

### TASK-27.5: Ensure all existing tests pass

- **Labels:** `layer/testing`, `priority/critical`
- **Description:** Verify that all existing tests still pass after the refactoring. The new `markdown_to_telegram_html` function should also be tested with edge cases:
  - Empty string / whitespace-only input
  - Plain text (no Markdown) — should match `escape_html` output
  - Nested formatting (bold inside italic, etc.)
  - Code blocks with generic types (`Vec<T>`, `HashMap<K, V>`)
  - Code blocks with `&&`, `||`, `<`, `>`
  - Links with special chars in URL
  - Blockquotes with multi-line content
  - Tables (unsupported — should strip to plain text)
  - Malformed Markdown

---

## Files Affected

| File | Change |
|---|---|
| `crates/duga-telegram-bot/Cargo.toml` | Add `pulldown-cmark` dep |
| `crates/duga-telegram-bot/src/formatting.rs` | New `markdown_to_telegram_html`; refactor `format_final_message` |
| `crates/duga-telegram-bot/src/render.rs` | Fix short-message `ParseMode::Html`; add comment on streaming path |

## Security

- **Indirect Prompt Injection**: Raw `Event::Html` nodes from the Markdown parser are escaped, preventing malicious third-party data from injecting arbitrary HTML into the Telegram output
- **Code Block Safety**: All HTML entities are escaped inside `<code>` and `<pre>` tags, preventing Telegram API 400 errors on code containing `<`, `>`, `&`
