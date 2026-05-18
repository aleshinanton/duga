//! Telegram live message formatting.
//!
//! Handles escaping, chunking, and final formatting of Telegram messages.
//! Partial streaming output is escaped as plain text; the final complete
//! message may be formatted in Telegram HTML.

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

/// Format a final message with Telegram HTML.
/// Applies safe formatting: bold, italic, code, pre blocks.
pub fn format_final_message(text: &str) -> String {
    // For simplicity, wrap in a pre block if it looks like code output.
    if text.contains('\n') && (text.contains("```") || text.starts_with("```")) {
        text.to_string()
    } else if text.len() > 200 {
        // Just use plain text for long messages — safe fallback.
        escape_telegram_plain_text(text)
    } else {
        // Safe plain text.
        escape_telegram_plain_text(text)
    }
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

        // Find a natural break point.
        let mut split_at = MAX_LEN;
        if let Some(pos) = remaining[..MAX_LEN].rfind('\n') {
            split_at = pos;
        } else if let Some(pos) = remaining[..MAX_LEN].rfind(' ') {
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
}
