//! Telegram live message formatting.
//!
//! Handles escaping, chunking, and final formatting of Telegram messages.
//! Partial streaming output is escaped as plain text; the final complete
//! message is converted from Markdown to Telegram-compatible HTML (legacy)
//! or passed as Rich Markdown (Bot API 10.1+).

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

/// Escape Telegram plain text to prevent accidental Markdown/HTML parsing.
pub fn escape_telegram_plain_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Escape HTML special characters for Telegram HTML parse mode.
pub fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Escape an HTML attribute value (quotes + ampersand).
fn escape_attr(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Wrap text in a collapsible blockquote if it exceeds the threshold.
/// Returns (html_text, needs_html_parse_mode).
pub fn maybe_collapse(text: &str, threshold: usize) -> (String, bool) {
    let escaped = escape_html(text);
    if text.len() > threshold {
        (format!("<blockquote expandable>{}</blockquote>", escaped), true)
    } else {
        (escaped, false)
    }
}

/// Convert Markdown text into Telegram-compatible HTML.
///
/// Parses Markdown with strikethrough + table extensions and maps supported
/// elements to Telegram HTML tags:
///   - **bold** / headings → `<b>` `</b>`
///   - *italic* → `<i>` `</i>`
///   - ~~strikethrough~~ → `<s>` `</s>`
///   - `inline code` → `<code>` `</code>`
///   - fenced code blocks → `<pre><code class="language-[lang]">` `</code></pre>`
///   - blockquotes → `<blockquote>` `</blockquote>`
///   - links → `<a href="url">` `</a>`
///   - unordered list items → `• ` prefix
///
/// All text (including inside `<code>` and `<pre>`) is HTML-escaped to
/// prevent Telegram 400 errors on generic types or logic symbols.
/// Unsupported elements (tables, horizontal rules) are stripped to plain
/// escaped text.  Raw `Event::Html` nodes from the parser are escaped to
/// mitigate indirect prompt injection.
pub fn markdown_to_telegram_html(input: &str) -> String {
    // First normalize raw inline HTML from untrusted tool output so that
    // safe tags like `<b>` render instead of showing as literal text.
    let input = sanitize_inline_html_to_markdown(input);
    let options = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES;
    let parser = Parser::new_ext(&input, options);

    let mut output = String::new();

    for event in parser {
        match event {
            Event::Start(tag) => push_start_tag(&mut output, tag),
            Event::End(tag) => push_end_tag(&mut output, tag),
            Event::Text(text) => {
                output.push_str(&escape_html(&text));
            }
            Event::Code(code) => {
                output.push_str("<code>");
                output.push_str(&escape_html(&code));
                output.push_str("</code>");
            }
            Event::Html(html) => {
                // Security: escape raw HTML nodes to prevent indirect
                // prompt injection from untrusted third-party data.
                output.push_str(&escape_html(&html));
            }
            Event::SoftBreak => output.push('\n'),
            Event::HardBreak => output.push('\n'),
            Event::Rule => {
                output.push_str("---\n");
            }
            Event::FootnoteReference(_) => {}
            Event::TaskListMarker(checked) => {
                if checked {
                    output.push_str("☑ ");
                } else {
                    output.push_str("☐ ");
                }
            }
            // InlineHtml: raw HTML embedded in Markdown (e.g., "<b>text</b>"
            // without Markdown delimiters).  Escape to prevent injection.
            Event::InlineHtml(html) => {
                output.push_str(&escape_html(&html));
            }
            // Math nodes: strip formulas (Telegram has no math rendering).
            Event::InlineMath(_) | Event::DisplayMath(_) => {}
        }
    }

    output.trim().to_string()
}

fn push_start_tag(output: &mut String, tag: Tag) {
    match tag {
        Tag::Paragraph | Tag::List(_) | Tag::Table(_)
        | Tag::TableHead | Tag::TableRow | Tag::TableCell => {}
        Tag::Heading { .. } => output.push_str("<b>"),
        Tag::BlockQuote(_) => output.push_str("<blockquote>"),
        Tag::CodeBlock(kind) => {
            let lang_attr = match kind {
                CodeBlockKind::Fenced(lang) if !lang.is_empty() => {
                    format!(" class=\"language-{}\"", escape_attr(&lang))
                }
                _ => String::new(),
            };
            output.push_str(&format!("<pre><code{}>", lang_attr));
        }
        Tag::Item => output.push_str("• "),
        Tag::Emphasis => output.push_str("<i>"),
        Tag::Strong => output.push_str("<b>"),
        Tag::Strikethrough => output.push_str("<s>"),
        Tag::Link { dest_url, .. } => {
            output.push_str(&format!("<a href=\"{}\">", escape_attr(&dest_url)));
        }
        Tag::Image { .. } => {
            // Images are not supported in Telegram HTML.
            output.push_str("[image]");
        }
        // Remaining unsupported elements — let text content pass through.
        _ => {}
    }
}

fn push_end_tag(output: &mut String, tag: TagEnd) {
    match tag {
        TagEnd::Paragraph | TagEnd::List(_) => output.push('\n'),
        TagEnd::Heading(_) => {
            output.push_str("</b>");
            output.push('\n');
        }
        TagEnd::BlockQuote(_) => output.push_str("</blockquote>"),
        TagEnd::CodeBlock => output.push_str("</code></pre>"),
        TagEnd::Item => output.push('\n'),
        TagEnd::Emphasis => output.push_str("</i>"),
        TagEnd::Strong => output.push_str("</b>"),
        TagEnd::Strikethrough => output.push_str("</s>"),
        TagEnd::Link => output.push_str("</a>"),
        TagEnd::Image
        | TagEnd::Table
        | TagEnd::TableHead
        | TagEnd::TableRow
        | TagEnd::TableCell => {}
        _ => {}
    }
}

/// Format a final message with Telegram HTML (legacy mode).
///
/// Delegates to `markdown_to_telegram_html` for full Markdown→HTML conversion
/// that Telegram's `ParseMode::Html` can render.
pub fn format_final_message(text: &str) -> String {
    let sanitized = sanitize_tool_call_syntax(text);
    markdown_to_telegram_html(&sanitized)
}

/// Format a final message for Rich Markdown (Bot API 10.1+).
///
/// Converts Markdown to Telegram's Rich Markdown format.  Rich Markdown is
/// GitHub-Flavored Markdown compatible with Telegram extensions:
///   - Tables with alignment (`|:---|:---:|`)
///   - Task lists (`- [ ]`, `- [x]`)
///   - Strikethrough (`~~text~~`), marked/highlight (`==text==`)
///   - Spoilers (`||text||`)
///   - Footnotes (`[^id]` / `[^id]: def`)
///   - Math blocks (`$$formula$$` / ```math)
///   - Details (`<details><summary>...</summary>...</details>`)
///   - Collages, slideshows, maps, pull quotes via HTML tags
///   - Custom emoji, date/time formatting
///
/// Most Markdown passes through unchanged because Rich Markdown is a superset.
/// We only:
///   1. Sanitize tool-call XML syntax (same as legacy HTML mode).
///   2. Collapse multiple blank lines.
///   3. Trim.
pub fn format_rich_markdown(text: &str) -> String {
    let sanitized = sanitize_tool_call_syntax(text);
    let sanitized = sanitize_inline_html_to_markdown(&sanitized);

    // Collapse 3+ consecutive newlines to at most 2 (preserve paragraph breaks,
    // but avoid excessive whitespace that wastes screen space on mobile).
    let mut result = String::with_capacity(sanitized.len());
    let mut newline_count = 0u32;
    for ch in sanitized.chars() {
        if ch == '\n' {
            newline_count += 1;
            if newline_count <= 2 {
                result.push(ch);
            }
        } else {
            newline_count = 0;
            result.push(ch);
        }
    }

    result.trim().to_string()
}

/// Build a Rich HTML message for streamed draft content.
///
/// Wraps text in a `<tg-thinking>` block for animated streaming,
/// optionally followed by the partial answer content.
pub fn build_draft_html(
    thinking_text: Option<&str>,
    partial_answer: Option<&str>,
) -> String {
    let mut html = String::new();

    if let Some(thinking) = thinking_text {
        if !thinking.is_empty() {
            html.push_str(&format!(
                "<tg-thinking>{}</tg-thinking>\n",
                escape_html(&truncate_str(thinking, 200))
            ));
        }
    }

    if let Some(answer) = partial_answer {
        if !answer.is_empty() {
            let rendered = markdown_to_telegram_html(answer);
            if !rendered.is_empty() {
                html.push_str(&rendered);
            }
        }
    }

    html
}

/// Build a step-progress body for the process message using Rich HTML.
///
/// Renders action labels as a collapsible `<blockquote expandable>` block
/// when there are many steps.  For shorter runs, labels are shown inline.
pub fn build_process_draft_html(
    action_labels: &[String],
    error_message: Option<&str>,
    streaming_delta: Option<&str>,
) -> String {
    let total = action_labels.len();
    let mut html = format!(
        "<b>🔄 Processing… ({total} step{})</b>\n",
        if total == 1 { "" } else { "s" }
    );

    if let Some(err) = error_message {
        html.push_str(&format!("⚠️ Error: {}  \n", escape_html(err)));
    }

    if total > 5 && !action_labels.is_empty() {
        // Collapse the step labels.
        let all_labels = action_labels.join("  \n");
        html.push_str(&format!(
            "<blockquote expandable>{}</blockquote>\n",
            escape_html(&all_labels)
        ));
    } else {
        // Show labels inline (last 5).
        let recent: Vec<_> = action_labels
            .iter()
            .rev()
            .take(5)
            .rev()
            .cloned()
            .collect();
        for label in &recent {
            html.push_str(&escape_html(label));
            html.push_str("  \n"); // double space for Telegram line break in HTML
        }
    }

    if let Some(delta) = streaming_delta {
        if !delta.is_empty() {
            html.push_str("\n<tg-thinking>");
            html.push_str(&escape_html(&truncate_str(delta, 120)));
            html.push_str("</tg-thinking>\n");
        }
    }

    html
}

/// Truncate a string to at most `max` chars, appending "…" if truncated.
fn truncate_str(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

/// Convert common inline HTML tags to Markdown and escape the rest.
///
/// Tool output and web pages often contain raw HTML like `<b>`, `<i>`, or
/// `<a href="...">`.  When the LLM includes that content in its answer, the
/// Telegram formatter would otherwise escape the tags literally, so the user
/// sees raw `<b>text</b>` instead of bold text.
///
/// This function converts safe formatting tags to Markdown syntax:
///   - `<b>`, `<strong>` → `**`
///   - `<i>`, `<em>` → `*`
///   - `<s>`, `<strike>`, `<del>` → `~~`
///   - `<code>` → `` ` ``
///   - `<a href="...">text</a>` → `[text](href)`
///   - `<br>` / `<br/>` → newline
///
/// Unknown or unsafe tags are escaped as HTML entities (`&lt;tag&gt;`) so
/// they render literally and cannot be interpreted as Telegram HTML/Markdown.
pub(crate) fn sanitize_inline_html_to_markdown(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut rest = text;
    let mut anchor_stack: Vec<String> = Vec::new();

    while let Some(pos) = rest.find('<') {
        result.push_str(&rest[..pos]);
        let after_lt = &rest[pos + 1..];

        let Some(end) = after_lt.find('>') else {
            // No matching '>' — escape the rest and stop.
            result.push_str(&escape_html(rest));
            break;
        };

        let tag_raw = &rest[pos + 1..pos + 1 + end];
        rest = &rest[pos + end + 2..];

        let tag_lower = tag_raw.to_lowercase();
        let is_self_closing = tag_lower.ends_with('/');
        let trimmed = tag_lower.trim().trim_end_matches('/').trim();

        match trimmed {
            "b" | "strong" => result.push_str("**"),
            "/b" | "/strong" => result.push_str("**"),
            "i" | "em" => result.push('*'),
            "/i" | "/em" => result.push('*'),
            "s" | "strike" | "del" => result.push_str("~~"),
            "/s" | "/strike" | "/del" => result.push_str("~~"),
            "code" => result.push('`'),
            "/code" => result.push('`'),
            "br" | "hr" => result.push('\n'),
            t if t == "a" || t.starts_with("a ") => {
                if let Some(href) = extract_href_value(tag_raw) {
                    anchor_stack.push(href);
                    result.push('[');
                } else {
                    result.push_str("&lt;a&gt;");
                }
            }
            "/a" => {
                if let Some(href) = anchor_stack.pop() {
                    result.push_str(&format!("]({href})"));
                } else {
                    result.push_str("&lt;/a&gt;");
                }
            }
            _ => {
                // Unknown/unsafe tag: escape the whole thing.
                result.push_str("&lt;");
                if is_self_closing {
                    result.push_str(tag_raw);
                    result.push_str("/&gt;");
                } else {
                    result.push_str(tag_raw);
                    result.push_str("&gt;");
                }
            }
        }
    }

    result.push_str(rest);

    // Close any unclosed anchor tags so the Markdown stays balanced.
    for href in anchor_stack {
        result.push_str(&format!("]({href})"));
    }

    result
}

/// Extract the value of an `href` attribute from an `<a ...>` tag.
fn extract_href_value(tag: &str) -> Option<String> {
    let lower = tag.to_lowercase();
    let idx = lower.find("href=")? + 5;
    let rest = &tag[idx..];
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let end = rest[1..].find(quote)? + 1;
    Some(rest[1..end].to_string())
}

/// Strip raw XML-like tool-call syntax that LLMs sometimes emit as prose.
///
/// Some LLMs (especially when primed with tool-call examples in context)
/// generate text containing literal `<invoke name="...">` or
/// `</tool_calls>` fragments.  These would be interpreted as Telegram HTML
/// tags and stripped, leaking partial artifacts to the user.
pub(crate) fn sanitize_tool_call_syntax(text: &str) -> String {
    // Strip </tool_calls> fragments (naked closing tag)
    let text = text.replace("</tool_calls>", "");

    // Strip <invoke name="..."> ... </invoke> blocks (with or without content)
    let text = strip_xml_tag(&text, "invoke", "[tool call removed]");

    // Strip <parameter ...> ... </parameter> blocks
    let text = strip_xml_tag(&text, "parameter", "");

    text
}

/// Strip balanced `<tag ...>...</tag>` blocks from text.
fn strip_xml_tag(text: &str, tag: &str, replacement: &str) -> String {
    let open_start = format!("<{tag} ");
    let open_simple = format!("<{tag}>");
    let close = format!("</{tag}>");

    let mut result = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        // Find opening tag
        let open_pos = rest.find(&open_start)
            .or_else(|| rest.find(&open_simple));
        match open_pos {
            Some(pos) => {
                result.push_str(&rest[..pos]);
                let after_open = &rest[pos..];
                // Find closing tag after the opening
                if let Some(close_pos) = after_open.find(&close) {
                    let skip_len = close_pos + close.len();
                    // When replacement is empty, avoid double spaces by trimming
                    // one space from the result or from the following text.
                    if replacement.is_empty() {
                        let result_ends_with_space = result.ends_with(' ');
                        let rest_starts_with_space = after_open[skip_len..].starts_with(' ');
                        if result_ends_with_space && rest_starts_with_space {
                            // Both sides have a space — skip the leading space in rest.
                            result.push_str(replacement);
                            rest = &after_open[skip_len + 1..];
                        } else {
                            result.push_str(replacement);
                            rest = &after_open[skip_len..];
                        }
                    } else {
                        result.push_str(replacement);
                        rest = &after_open[skip_len..];
                    }
                } else {
                    // No closing tag — treat as literal text
                    result.push_str(&rest[pos..pos + open_start.len()]);
                    rest = &rest[pos + open_start.len()..];
                }
            }
            None => {
                result.push_str(rest);
                break;
            }
        }
    }
    result
}

/// Chunk a message near Telegram's 4096 character limit.
/// NOTE: Callers that wrap chunks in HTML (e.g., `<blockquote expandable>`)
/// must chunk the raw text FIRST, then wrap each chunk individually.
pub fn chunk_message(text: &str) -> Vec<String> {
    const MAX_LEN: usize = 4000;

    if text.len() <= MAX_LEN {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        if remaining.len() <= MAX_LEN {
            chunks.push(remaining.to_string());
            break;
        }

        // Clamp MAX_LEN to a valid UTF-8 char boundary.
        let mut max = MAX_LEN.min(remaining.len());
        while max > 0 && !remaining.is_char_boundary(max) {
            max -= 1;
        }

        // Find a natural break point.
        let mut split_at = max;
        if let Some(pos) = remaining[..max].rfind('\n') {
            split_at = pos;
        } else if let Some(pos) = remaining[..max].rfind(' ') {
            split_at = pos;
        }

        chunks.push(remaining[..split_at].to_string());
        remaining = &remaining[split_at..];
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_html_chars() {
        let result = escape_telegram_plain_text("<script>alert('xss')</script>");
        assert!(!result.contains('<'));
        assert!(!result.contains('>'));
    }

    #[test]
    fn test_chunk_short_message() {
        let chunks = chunk_message("hello");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "hello");
    }

    #[test]
    fn test_chunk_long_message() {
        let line = "a".repeat(100) + "\n";
        let text: String = std::iter::repeat(&line).take(100).cloned().collect();
        let chunks = chunk_message(&text);
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(chunk.len() <= 4000);
        }
    }

    #[test]
    fn test_chunk_message_natural_breaks() {
        let text = "Line one\nLine two\nLine three";
        let text = text.repeat(1000);
        let chunks = chunk_message(&text);
        for chunk in &chunks {
            assert!(chunk.len() <= 4000);
        }
    }

    #[test]
    fn test_chunk_message_multibyte_utf8() {
        // Regression: slicing at byte 4000 used to panic on multi-byte chars.
        // Build a string where a 2-byte Cyrillic char straddles byte 4000.
        let mut text = String::with_capacity(4010);
        // Fill with single-byte chars up to byte 3999.
        for _ in 0..3999 {
            text.push('a');
        }
        // Add a 2-byte Cyrillic char that spans bytes 3999..4001.
        text.push('й'); // Cyrillic short i
        // Fill the rest.
        for _ in 0..100 {
            text.push('b');
        }
        let chunks = chunk_message(&text);
        assert!(chunks.len() >= 2);
        for chunk in &chunks {
            assert!(chunk.len() <= 4000);
            // Each chunk must be valid UTF-8.
            assert!(std::str::from_utf8(chunk.as_bytes()).is_ok());
        }
    }

    #[test]
    fn test_escape_html_special_chars() {
        assert_eq!(escape_html("&"), "&amp;");
        assert_eq!(escape_html("<"), "&lt;");
        assert_eq!(escape_html(">"), "&gt;");
        assert_eq!(escape_html("<b>bold</b>"), "&lt;b&gt;bold&lt;/b&gt;");
    }

    #[test]
    fn test_maybe_collapse_short_text() {
        let (result, needs_html) = maybe_collapse("short", 10);
        assert_eq!(result, "short");
        assert!(!needs_html);
    }

    #[test]
    fn test_maybe_collapse_long_text() {
        let (result, needs_html) = maybe_collapse("this is a very long message", 10);
        assert!(result.starts_with("<blockquote expandable>"));
        assert!(result.ends_with("</blockquote>"));
        assert!(needs_html);
    }

    #[test]
    fn test_maybe_collapse_escapes_html() {
        let (result, _needs_html) = maybe_collapse("<script>alert(1)</script>", 5);
        assert!(!result.contains('<') || result.starts_with("<blockquote"));
        assert!(result.contains("&lt;script&gt;"));
    }

    #[test]
    fn test_chunk_message_then_wrap_html() {
        // Demonstrate the correct approach: chunk raw text first, then wrap.
        let raw = "Line A\nLine B\nLine C".repeat(500);
        let raw_chunks = chunk_message(&raw);
        let html_chunks: Vec<String> = raw_chunks
            .iter()
            .map(|c| format!("<blockquote expandable>{}</blockquote>", escape_html(c)))
            .collect();
        for chunk in &html_chunks {
            // Each chunk should be a well-formed blockquote.
            assert!(chunk.starts_with("<blockquote expandable>"));
            assert!(chunk.ends_with("</blockquote>"));
            // Each should be under Telegram's limit.
            assert!(chunk.len() <= 4096);
        }
    }

    #[test]
    fn sanitize_removes_tool_calls_closing_tag() {
        let input = "Some text </tool_calls> more text";
        let result = super::sanitize_tool_call_syntax(input);
        assert_eq!(result, "Some text  more text");
    }

    #[test]
    fn sanitize_removes_invoke_block() {
        let input = "Before <invoke name=\"shell\">\n  <parameter name=\"cmd\">curl</parameter>\n</invoke> After";
        let result = super::sanitize_tool_call_syntax(input);
        assert_eq!(result, "Before [tool call removed] After");
    }

    #[test]
    fn sanitize_removes_parameter_blocks() {
        let input = "Text <parameter name=\"x\">value</parameter> end";
        let result = super::sanitize_tool_call_syntax(input);
        assert_eq!(result, "Text end");
    }

    #[test]
    fn sanitize_preserves_normal_text() {
        let input = "Normal text with < and > characters";
        let result = super::sanitize_tool_call_syntax(input);
        assert_eq!(result, input);
    }

    #[test]
    fn sanitize_inline_html_converts_bold() {
        let input = "Hello <b>world</b> and <strong>universe</strong>";
        let result = super::sanitize_inline_html_to_markdown(input);
        assert_eq!(result, "Hello **world** and **universe**");
    }

    #[test]
    fn sanitize_inline_html_converts_italic() {
        let input = "<i>italic</i> and <em>emphasis</em>";
        let result = super::sanitize_inline_html_to_markdown(input);
        assert_eq!(result, "*italic* and *emphasis*");
    }

    #[test]
    fn sanitize_inline_html_converts_strikethrough() {
        let input = "<s>strike</s>, <strike>strike2</strike>, <del>del</del>";
        let result = super::sanitize_inline_html_to_markdown(input);
        assert_eq!(result, "~~strike~~, ~~strike2~~, ~~del~~");
    }

    #[test]
    fn sanitize_inline_html_converts_code() {
        let input = "Use <code>cargo build</code> to compile";
        let result = super::sanitize_inline_html_to_markdown(input);
        assert_eq!(result, "Use `cargo build` to compile");
    }

    #[test]
    fn sanitize_inline_html_converts_link() {
        let input = "See <a href=\"https://example.com\">example</a> here";
        let result = super::sanitize_inline_html_to_markdown(input);
        assert_eq!(result, "See [example](https://example.com) here");
    }

    #[test]
    fn sanitize_inline_html_converts_self_closing_br() {
        let input = "Line one<br>Line two<br/>Line three";
        let result = super::sanitize_inline_html_to_markdown(input);
        assert_eq!(result, "Line one\nLine two\nLine three");
    }

    #[test]
    fn sanitize_inline_html_escapes_unknown_tags() {
        let input = "<script>alert(1)</script> and <div>block</div>";
        let result = super::sanitize_inline_html_to_markdown(input);
        assert_eq!(
            result,
            "&lt;script&gt;alert(1)&lt;/script&gt; and &lt;div&gt;block&lt;/div&gt;"
        );
    }

    #[test]
    fn sanitize_inline_html_preserves_plain_angle_brackets() {
        let input = "Use < or > for comparison";
        let result = super::sanitize_inline_html_to_markdown(input);
        assert_eq!(result, "Use &lt; or &gt; for comparison");
    }

    #[test]
    fn sanitize_inline_html_unclosed_anchor_is_balanced() {
        let input = "<a href=\"https://example.com\">unclosed";
        let result = super::sanitize_inline_html_to_markdown(input);
        assert_eq!(result, "[unclosed](https://example.com)");
    }

    #[test]
    fn sanitize_handles_nested_like_pattern() {
        // Real-world case: LLM text with invoke pattern
        let input = "Done. </tool_calls>\n<invoke name=\"shell\">\n<parameter name=\"command\" string=\"false\">[\"curl\",\"-s\"]</parameter>\n</invoke>";
        let result = super::sanitize_tool_call_syntax(input);
        assert!(!result.contains("</tool_calls>"), "closing tag should be removed");
        assert!(!result.contains("<invoke"), "invoke block should be removed");
        assert!(!result.contains("</invoke>"), "invoke closing should be removed");
        assert!(!result.contains("<parameter"), "parameter should be removed");
    }
}
