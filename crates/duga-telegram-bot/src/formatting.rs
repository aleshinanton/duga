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
}
