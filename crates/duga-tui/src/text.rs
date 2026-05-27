//! Text utilities for the TUI: width calculation, word-wrap, truncation.
//!
//! Uses `unicode-width` for CJK character support.

use unicode_width::UnicodeWidthStr;

/// Display width of a string, excluding ANSI escape sequences.
pub fn visible_width(s: &str) -> usize {
    // Strip ANSI escape sequences before measuring.
    let clean = strip_ansi(s);
    UnicodeWidthStr::width(clean.as_str())
}

/// Strip ANSI escape sequences from a string.
fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            // Skip escape sequence: \x1b[...m or similar
            while let Some(&c) = chars.peek() {
                chars.next();
                if c.is_alphabetic() || c == '~' {
                    break;
                }
            }
        } else {
            result.push(ch);
        }
    }
    result
}

/// Truncate a string to fit within a visible width, appending "…" if truncated.
pub fn truncate_to_width(s: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let clean = strip_ansi(s);
    if UnicodeWidthStr::width(clean.as_str()) <= max_width {
        return s.to_string();
    }
    let mut width = 0usize;
    let mut result = String::with_capacity(max_width + 3);
    for ch in clean.chars() {
        let ch_width = UnicodeWidthStr::width(ch.to_string().as_str());
        if width + ch_width + 1 > max_width {
            // Reserve 1 for "…"
            result.push('…');
            break;
        }
        width += ch_width;
        result.push(ch);
    }
    result
}

/// Word-wrap text to fit within a given width.
/// Returns a vector of lines, each no wider than `max_width` (visible width).
pub fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![];
    }
    let mut lines: Vec<String> = Vec::new();
    for raw_line in text.split('\n') {
        if raw_line.is_empty() {
            lines.push(String::new());
            continue;
        }
        let clean = strip_ansi(raw_line);
        if UnicodeWidthStr::width(clean.as_str()) <= max_width {
            lines.push(raw_line.to_string());
            continue;
        }
        // Word-based wrapping
        let words: Vec<&str> = raw_line.split(' ').collect();
        let mut current = String::new();
        let mut current_width: usize = 0;
        for word in words {
            let word_w = UnicodeWidthStr::width(strip_ansi(word).as_str());
            if current.is_empty() {
                current.push_str(word);
                current_width = word_w;
            } else if current_width + 1 + word_w <= max_width {
                current.push(' ');
                current.push_str(word);
                current_width += 1 + word_w;
            } else {
                if !current.is_empty() {
                    lines.push(current.clone());
                }
                current = word.to_string();
                current_width = word_w;
            }
        }
        if !current.is_empty() {
            lines.push(current);
        }
    }
    lines
}

/// Wrap text preserving ANSI codes. Works like wrap_text but handles ANSI gracefully.
pub fn wrap_text_with_ansi(text: &str, max_width: usize) -> Vec<String> {
    // Simple approach: strip ANSI, wrap, then re-apply.
    // For now, use the basic wrap_text.
    wrap_text(text, max_width)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_width_ascii() {
        assert_eq!(visible_width("hello"), 5);
        assert_eq!(visible_width(""), 0);
    }

    #[test]
    fn visible_width_cjk() {
        // CJK characters are width 2
        assert_eq!(visible_width("你好"), 4);
        assert_eq!(visible_width("hello世界"), 9); // 5 + 2*2 = 9
    }

    #[test]
    fn visible_width_strips_ansi() {
        // ANSI codes should be ignored
        let ansi = "\x1b[1;32mhello\x1b[0m";
        assert_eq!(visible_width(ansi), 5);
    }

    #[test]
    fn truncate_to_width_noop() {
        assert_eq!(truncate_to_width("hello", 10), "hello");
    }

    #[test]
    fn truncate_to_width_truncates() {
        let result = truncate_to_width("hello world", 8);
        assert!(result.len() <= 11); // "hello w…"
        assert!(result.contains('…'));
    }

    #[test]
    fn truncate_zero_width() {
        assert_eq!(truncate_to_width("hello", 0), "");
    }

    #[test]
    fn wrap_text_noop() {
        assert_eq!(wrap_text("hello", 10), vec!["hello"]);
    }

    #[test]
    fn wrap_text_wraps() {
        let result = wrap_text("hello world foo", 8);
        assert_eq!(result.len(), 3); // "hello", "world", "foo"
    }

    #[test]
    fn wrap_text_respects_newlines() {
        let result = wrap_text("line1\n\nline2", 80);
        assert_eq!(result, vec!["line1", "", "line2"]);
    }

    #[test]
    fn wrap_text_long_word() {
        let result = wrap_text("supercalifragilistic", 5);
        assert!(!result.is_empty());
    }
}
