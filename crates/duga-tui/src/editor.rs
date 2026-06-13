//! Multi-line input editor for the TUI.
//!
//! Supports: multi-line input with word-wrap, cursor movement,
//! history navigation (Up/Down), paste, placeholder text.
//! Scroll offset is automatically adjusted during rendering so
//! the cursor line stays within the visible input area.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthChar;

/// Cached result of wrapping the full buffer for a specific display width.
#[derive(Clone, Debug)]
struct WrapCache {
    /// Display width this cache was computed for.
    text_width: u16,
    /// Buffer length at the time the cache was computed (dirtiness check).
    buffer_len: usize,
    /// All wrapped lines.
    lines: Vec<String>,
}

/// Multi-line input editor with history and cursor tracking.
#[derive(Clone, Debug)]
pub struct Editor {
    /// Full text buffer.
    buffer: String,
    /// Byte offset of the cursor within the buffer.
    cursor: usize,
    /// Vertical scroll offset (number of wrapped lines above visible area).
    scroll_offset: usize,
    /// History of submitted inputs.
    history: Vec<String>,
    /// Current position in history navigation (None = editing new text).
    history_index: Option<usize>,
    /// Placeholder text shown when buffer is empty.
    placeholder: String,
    /// Cache of wrapped lines. Invalidated whenever `buffer` changes.
    wrap_cache: Option<WrapCache>,
}

impl Editor {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            cursor: 0,
            scroll_offset: 0,
            history: Vec::new(),
            history_index: None,
            placeholder: "Type a task or question…".into(),
            wrap_cache: None,
        }
    }

    /// Set placeholder text.
    pub fn with_placeholder(mut self, text: impl Into<String>) -> Self {
        self.placeholder = text.into();
        self
    }

    /// Returns the full text buffer.
    pub fn text(&self) -> &str {
        &self.buffer
    }

    /// Returns the cursor byte position.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Returns the current scroll offset.
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Returns the placeholder text.
    pub fn placeholder(&self) -> &str {
        &self.placeholder
    }

    /// Take the text (clears the editor) and add to history.
    pub fn take_text(&mut self) -> String {
        let text = std::mem::take(&mut self.buffer);
        if !text.trim().is_empty() {
            self.history.push(text.clone());
        }
        self.cursor = 0;
        self.scroll_offset = 0;
        self.history_index = None;
        self.invalidate_cache();
        text
    }

    /// Insert text at cursor position (for paste, character input).
    pub fn insert_text(&mut self, text: &str) {
        self.buffer.insert_str(self.cursor, text);
        self.cursor += text.len();
        self.history_index = None;
        self.invalidate_cache();
    }

    /// Insert text at cursor position, enforcing a maximum line limit.
    /// If the insert would cause the total logical lines (separated by \\n)
    /// to exceed `max_lines`, the text is truncated at the last line boundary
    /// that keeps the total within the limit.  Returns the number of bytes
    /// actually inserted.
    pub fn insert_text_safe_lines(&mut self, text: &str, max_lines: usize) -> usize {
        let current_lines = if self.buffer.is_empty() { 0 } else { self.buffer.lines().count() };
        if current_lines >= max_lines {
            return 0;
        }
        let available = max_lines - current_lines;

        // Split the new text into lines and count them
        let text_lines: Vec<&str> = text.split('\n').collect();
        if text_lines.len() <= available {
            // Entire paste fits
            let len = text.len();
            if len > 0 {
                self.insert_text(text);
            }
            return len;
        }

        // Only take the first `available` lines.
        // Find the byte offset where the `available`-th line ends (the newline before it).
        let mut byte_pos = 0usize;
        let mut lines_found = 0usize;
        for (i, _) in text.char_indices() {
            if text[i..].starts_with('\n') {
                lines_found += 1;
                if lines_found == available {
                    byte_pos = i;
                    break;
                }
            }
        }
        // If we matched exactly `available` newlines, `byte_pos` points to the last
        // allowed newline.  Take up to that byte (inclusive of newline).
        // If we never found enough newlines, take all text (but we already handled
        // the "fits" case above, so this shouldn't happen).
        if byte_pos > 0 {
            let to_insert = &text[..=byte_pos]; // include the newline
            let len = to_insert.len();
            if len > 0 {
                self.insert_text(to_insert);
            }
            len
        } else {
            // No room for even one line? Should not happen because available > 0.
            0
        }
    }

    /// Insert a character at cursor position.
    pub fn insert_char(&mut self, ch: char) {
        self.buffer.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
        self.history_index = None;
        self.invalidate_cache();
    }

    /// Insert a newline at cursor position.
    pub fn insert_newline(&mut self) {
        self.insert_char('\n');
    }

    // ── Cache management ──────────────────────────────────────────────

    /// Invalidate the wrap cache so it is recomputed on the next access.
    fn invalidate_cache(&mut self) {
        self.wrap_cache = None;
    }

    /// Ensure the wrap cache is up-to-date for the given `text_width`.
    /// Returns a reference to the cached wrapped lines.
    fn ensure_cache(&mut self, text_width: u16) -> &[String] {
        let fresh = match &self.wrap_cache {
            Some(c) => c.text_width != text_width || c.buffer_len != self.buffer.len(),
            None => true,
        };
        if fresh {
            let lines = wrap_lines(&self.buffer, text_width);
            self.wrap_cache = Some(WrapCache {
                text_width,
                buffer_len: self.buffer.len(),
                lines,
            });
        }
        &self.wrap_cache.as_ref().unwrap().lines
    }

    // ── Key handling ──────────────────────────────────────────────────

    /// Handle a key event. Returns true if the key was consumed.
    pub fn handle_key(&mut self, key: &KeyEvent) -> EditorAction {
        match key {
            // Enter: submit (or newline with Shift).
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::SHIFT,
                ..
            } => {
                self.insert_newline();
                EditorAction::Consumed
            }
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                if !self.buffer.trim().is_empty() {
                    return EditorAction::Submit;
                }
                EditorAction::Consumed
            }
            // Backspace
            KeyEvent {
                code: KeyCode::Backspace,
                ..
            } => {
                self.delete_before_cursor();
                EditorAction::Consumed
            }
            // Delete
            KeyEvent {
                code: KeyCode::Delete,
                ..
            } => {
                self.delete_after_cursor();
                EditorAction::Consumed
            }
            // Cursor movement
            KeyEvent {
                code: KeyCode::Left,
                modifiers: KeyModifiers::CONTROL,
                ..
            } => {
                self.move_word_left();
                EditorAction::Consumed
            }
            KeyEvent {
                code: KeyCode::Right,
                modifiers: KeyModifiers::CONTROL,
                ..
            } => {
                self.move_word_right();
                EditorAction::Consumed
            }
            KeyEvent {
                code: KeyCode::Left,
                ..
            } => {
                self.move_left();
                EditorAction::Consumed
            }
            KeyEvent {
                code: KeyCode::Right,
                ..
            } => {
                self.move_right();
                EditorAction::Consumed
            }
            KeyEvent {
                code: KeyCode::Home,
                ..
            } => {
                self.move_home();
                EditorAction::Consumed
            }
            KeyEvent {
                code: KeyCode::End,
                ..
            } => {
                self.move_end();
                EditorAction::Consumed
            }
            // History navigation
            KeyEvent {
                code: KeyCode::Up,
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                self.navigate_history_back();
                EditorAction::Consumed
            }
            KeyEvent {
                code: KeyCode::Down,
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                self.navigate_history_forward();
                EditorAction::Consumed
            }
            // Tab: accept autocomplete (placeholder)
            KeyEvent {
                code: KeyCode::Tab,
                ..
            } => {
                // Autocomplete placeholder — for now, do nothing
                EditorAction::Consumed
            }
            // Character input
            KeyEvent {
                code: KeyCode::Char(ch),
                modifiers: KeyModifiers::NONE | KeyModifiers::SHIFT,
                ..
            } => {
                self.insert_char(*ch);
                EditorAction::Consumed
            }
            KeyEvent {
                code: KeyCode::Char(ch),
                modifiers: _,
                ..
            } => {
                // Ctrl+char and Alt+char are handled as keybindings,
                // not as text input. Ignore here.
                let _ = ch;
                EditorAction::Ignored
            }
            // Everything else is ignored.
            _ => EditorAction::Ignored,
        }
    }

    // ── Cursor movement helpers ────────────────────────────────────────

    fn move_left(&mut self) {
        if self.cursor > 0 {
            // Move one grapheme cluster left
            let mut idx = self.cursor;
            while idx > 0 {
                idx -= 1;
                if self.buffer.is_char_boundary(idx) {
                    self.cursor = idx;
                    break;
                }
            }
        }
    }

    fn move_right(&mut self) {
        if self.cursor < self.buffer.len() {
            // Move one code point right
            let mut chars = self.buffer[self.cursor..].chars();
            if let Some(ch) = chars.next() {
                self.cursor += ch.len_utf8();
            }
        }
    }

    fn move_word_left(&mut self) {
        // Skip whitespace, then skip word chars
        let mut idx = self.cursor;
        // Skip trailing whitespace
        while idx > 0 && self.char_before(idx).is_some_and(|c| c.is_whitespace()) {
            idx = self.prev_char_boundary(idx);
        }
        // Skip word chars
        while idx > 0 && self.char_before(idx).is_some_and(|c| !c.is_whitespace()) {
            idx = self.prev_char_boundary(idx);
        }
        self.cursor = idx;
    }

    fn move_word_right(&mut self) {
        let mut idx = self.cursor;
        // Skip word chars
        while idx < self.buffer.len()
            && self.char_at(idx).is_some_and(|c| !c.is_whitespace())
        {
            idx += self.char_at(idx).map(|c| c.len_utf8()).unwrap_or(0);
        }
        // Skip whitespace
        while idx < self.buffer.len()
            && self.char_at(idx).is_some_and(|c| c.is_whitespace())
        {
            idx += self.char_at(idx).map(|c| c.len_utf8()).unwrap_or(0);
        }
        self.cursor = idx;
    }

    fn move_home(&mut self) {
        // Move to start of current line
        let line_start = self.buffer[..self.cursor]
            .rfind('\n')
            .map(|p| p + 1)
            .unwrap_or(0);
        self.cursor = line_start;
    }

    fn move_end(&mut self) {
        // Move to end of current line
        let line_end = self.buffer[self.cursor..]
            .find('\n')
            .map(|p| self.cursor + p)
            .unwrap_or(self.buffer.len());
        self.cursor = line_end;
    }

    fn delete_before_cursor(&mut self) {
        if self.cursor > 0 {
            let prev = self.prev_char_boundary(self.cursor);
            self.buffer.replace_range(prev..self.cursor, "");
            self.cursor = prev;
            self.invalidate_cache();
        }
    }

    fn delete_after_cursor(&mut self) {
        if self.cursor < self.buffer.len() {
            let next = self.next_char_boundary(self.cursor);
            self.buffer.replace_range(self.cursor..next, "");
            self.invalidate_cache();
        }
    }

    fn navigate_history_back(&mut self) {
        if self.history.is_empty() {
            return;
        }
        match self.history_index {
            None => {
                // Start navigating from the latest entry
                self.history_index = Some(self.history.len() - 1);
            }
            Some(0) => {
                // Already at oldest; nothing changes
            }
            Some(idx) => {
                self.history_index = Some(idx - 1);
            }
        }
        if let Some(idx) = self.history_index {
            self.buffer = self.history[idx].clone();
            self.cursor = self.buffer.len();
            self.invalidate_cache();
        }
    }

    fn navigate_history_forward(&mut self) {
        match self.history_index {
            None => {
                // Not navigating
            }
            Some(idx) if idx + 1 < self.history.len() => {
                self.history_index = Some(idx + 1);
                self.buffer = self.history[idx + 1].clone();
                self.cursor = self.buffer.len();
                self.invalidate_cache();
            }
            Some(_) => {
                // Reached the end of history; restore fresh buffer
                self.history_index = None;
                self.buffer.clear();
                self.cursor = 0;
                self.invalidate_cache();
            }
        }
    }

    // ── Helpers ────────────────────────────────────────────────────────

    fn char_at(&self, idx: usize) -> Option<char> {
        self.buffer[idx..].chars().next()
    }

    fn char_before(&self, idx: usize) -> Option<char> {
        if idx == 0 {
            return None;
        }
        let mut pos = idx;
        while pos > 0 {
            pos -= 1;
            if self.buffer.is_char_boundary(pos) {
                return self.buffer[pos..].chars().next();
            }
        }
        None
    }

    fn prev_char_boundary(&self, idx: usize) -> usize {
        let mut pos = idx;
        while pos > 0 {
            pos -= 1;
            if self.buffer.is_char_boundary(pos) {
                return pos;
            }
        }
        0
    }

    fn next_char_boundary(&self, idx: usize) -> usize {
        let mut pos = idx;
        while pos < self.buffer.len() {
            pos += 1;
            if self.buffer.is_char_boundary(pos) {
                return pos;
            }
        }
        self.buffer.len()
    }

    // ── Scroll-aware rendering helpers ─────────────────────────────────

    /// Compute the wrapped-line index (0-based) that the cursor falls on
    /// for the given display width per line.
    pub fn cursor_wrapped_line(&self, text_width: u16) -> usize {
        let before = &self.buffer[..self.cursor];
        wrap_position(before, text_width).0
    }

    /// Compute the cursor's (wrapped_line, column) position for the given
    /// display width.  Both values are 0-based.
    pub fn cursor_position(&self, text_width: u16) -> (usize, usize) {
        let before = &self.buffer[..self.cursor];
        wrap_position(before, text_width)
    }

    /// Total number of wrapped lines for the full buffer at `text_width`.
    /// Uses the wrap cache for O(1) after the first call in a frame.
    pub fn wrapped_line_count(&mut self, text_width: u16) -> usize {
        self.ensure_cache(text_width).len()
    }

    /// Adjust `scroll_offset` so the cursor line is visible within
    /// `visible_rows`.  Call this once per frame before extracting
    /// visible text.
    pub fn scroll_to_cursor(&mut self, text_width: u16, visible_rows: usize) {
        if visible_rows == 0 {
            return;
        }
        let cursor_line = self.cursor_wrapped_line(text_width);
        if cursor_line < self.scroll_offset {
            self.scroll_offset = cursor_line;
        } else if cursor_line >= self.scroll_offset + visible_rows {
            self.scroll_offset = cursor_line - visible_rows + 1;
        }
        // Clamp: don't scroll past total line count.
        let total = self.wrapped_line_count(text_width);
        let max_offset = total.saturating_sub(visible_rows);
        if self.scroll_offset > max_offset {
            self.scroll_offset = max_offset;
        }
    }

    /// Return the text that should be visible given the current scroll
    /// offset.  The returned string is pre-wrapped at `text_width` so
    /// each embedded newline corresponds to one display row.
    pub fn visible_text(&mut self, text_width: u16, visible_rows: usize) -> String {
        let skip = self.scroll_offset;
        let lines = self.ensure_cache(text_width);
        if skip >= lines.len() {
            return String::new();
        }
        let end = (skip + visible_rows).min(lines.len());
        lines[skip..end].join("\n")
    }
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

/// Action result from editor key handling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorAction {
    /// Key was consumed by the editor.
    Consumed,
    /// Key was not handled by the editor (pass to other handlers).
    Ignored,
    /// User pressed Enter to submit the input.
    Submit,
}

// ── Wrapping helpers (free functions) ──────────────────────────────────────

/// Walk `text` and return the (line_index, column) after the last character.
fn wrap_position(text: &str, width: u16) -> (usize, usize) {
    if width == 0 || text.is_empty() {
        if text.lines().count() == 0 {
            return (0, 0);
        }
        let last = text.lines().last().unwrap_or("");
        return (text.lines().count().saturating_sub(1), last.len());
    }
    let mut line = 0usize;
    let mut col = 0usize;
    for ch in text.chars() {
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            let w = ch.width().unwrap_or(0).max(1);
            if col + w > width as usize {
                line += 1;
                col = w;
            } else {
                col += w;
            }
        }
    }
    (line, col)
}

/// Wrap `text` at `width` columns using incremental width tracking – O(n) per call.
fn wrap_lines(text: &str, width: u16) -> Vec<String> {
    let w = width as usize;
    if w == 0 || text.is_empty() {
        return text.lines().map(|s| s.to_string()).collect();
    }
    let mut lines: Vec<String> = vec![String::new()];
    let mut cur_w: usize = 0;
    for ch in text.chars() {
        if ch == '\n' {
            lines.push(String::new());
            cur_w = 0;
        } else {
            let cw = ch.width().unwrap_or(0).max(1);
            if cur_w + cw > w {
                lines.push(String::new());
                cur_w = 0;
            }
            lines.last_mut().unwrap().push(ch);
            cur_w += cw;
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_editor_is_empty() {
        let editor = Editor::new();
        assert_eq!(editor.text(), "");
        assert_eq!(editor.cursor(), 0);
    }

    #[test]
    fn insert_char_and_text() {
        let mut editor = Editor::new();
        editor.insert_char('h');
        editor.insert_char('i');
        assert_eq!(editor.text(), "hi");
        assert_eq!(editor.cursor(), 2);
    }

    #[test]
    fn enter_submits() {
        let mut editor = Editor::new();
        editor.insert_text("hello");
        let action = editor.handle_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(action, EditorAction::Submit);
        // Editor keeps text until caller calls take_text()
        assert_eq!(editor.text(), "hello");
    }

    #[test]
    fn shift_enter_newline() {
        let mut editor = Editor::new();
        editor.insert_text("hello");
        let action = editor.handle_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
        assert_eq!(action, EditorAction::Consumed);
        assert!(editor.text().contains('\n'));
    }

    #[test]
    fn empty_enter_does_not_submit() {
        let mut editor = Editor::new();
        let action = editor.handle_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(action, EditorAction::Consumed);
    }

    #[test]
    fn backspace_works() {
        let mut editor = Editor::new();
        editor.insert_text("abc");
        editor.handle_key(&KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(editor.text(), "ab");
    }

    #[test]
    fn history_navigation() {
        let mut editor = Editor::new();
        editor.insert_text("first");
        editor.take_text(); // Adds to history
        editor.insert_text("second");
        editor.take_text();

        // Navigate back
        editor.handle_key(&KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(editor.text(), "second");

        editor.handle_key(&KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(editor.text(), "first");
    }

    #[test]
    fn take_text_adds_to_history_and_clears() {
        let mut editor = Editor::new();
        editor.insert_text("hello world");
        let text = editor.take_text();
        assert_eq!(text, "hello world");
        assert_eq!(editor.text(), "");
        assert_eq!(editor.cursor(), 0);
    }

    #[test]
    fn empty_take_does_not_add_to_history() {
        let mut editor = Editor::new();
        editor.insert_text("   ");
        let text = editor.take_text();
        assert_eq!(text, "   "); // Still returned, but not added to history
    }

    // ── insert_text_safe_lines tests ──────────────────────────────────

    #[test]
    fn insert_text_safe_lines_within_limit() {
        let mut editor = Editor::new();
        let n = editor.insert_text_safe_lines("hello\nworld", 10);
        assert_eq!(n, 11); // "hello\nworld" = 11 bytes
        assert_eq!(editor.text(), "hello\nworld");
    }

    #[test]
    fn insert_text_safe_lines_exceeds_limit() {
        let mut editor = Editor::new();
        editor.insert_text("line1\nline2"); // 2 lines
        // Paste 5 lines when only 3 fit (limit = 5, current = 2, available = 3)
        let n = editor.insert_text_safe_lines("a\nb\nc\nd\ne", 5);
        // Only "a\nb\nc\n" should be inserted (3 lines: a, b, c with trailing newline)
        assert_eq!(editor.text(), "line1\nline2a\nb\nc\n");
        assert_eq!(n, 6); // "a\nb\nc\n" = 6 bytes
    }

    #[test]
    fn insert_text_safe_lines_no_space() {
        let mut editor = Editor::new();
        editor.insert_text("a\nb\nc\nd\ne"); // 5 lines
        let n = editor.insert_text_safe_lines("extra", 5); // no room
        assert_eq!(n, 0);
        assert_eq!(editor.text(), "a\nb\nc\nd\ne");
    }

    #[test]
    fn insert_text_safe_lines_exact_fit() {
        let mut editor = Editor::new();
        editor.insert_text("line1\nline2"); // 2 lines
        let n = editor.insert_text_safe_lines("c\nd\ne", 5); // exactly 3 fit
        assert_eq!(n, 5); // "c\nd\ne" = 5 bytes
        assert_eq!(editor.text(), "line1\nline2c\nd\ne");
    }

    #[test]
    fn insert_text_safe_lines_single_line_no_newline() {
        let mut editor = Editor::new();
        editor.insert_text("line1\nline2\nline3"); // 3 lines
        // Paste single line without trailing newline when only 2 slots left (limit=5)
        let n = editor.insert_text_safe_lines("extra", 5);
        assert_eq!(n, 5);
        assert_eq!(editor.text(), "line1\nline2\nline3extra");
    }

    #[test]
    fn insert_text_safe_lines_empty_buffer() {
        let mut editor = Editor::new();
        // Empty buffer counts as 0 lines
        let n = editor.insert_text_safe_lines("a\nb\nc", 2);
        // Only "a\nb\n" fits (2 lines)
        assert_eq!(n, 4); // "a\nb\n" = 4 bytes
        assert_eq!(editor.text(), "a\nb\n");
    }

    // ── Cache tests ───────────────────────────────────────────────────

    #[test]
    fn cache_invalidated_on_insert() {
        let mut editor = Editor::new();
        editor.insert_text("hello");
        {
            let lines1 = editor.ensure_cache(80);
            assert_eq!(lines1, &["hello".to_string()]);
        }

        // Buffer changed — cache should be recomputed
        editor.insert_text(" world");
        {
            let lines2 = editor.ensure_cache(80);
            assert_eq!(lines2, &["hello world".to_string()]);
        }
    }

    #[test]
    fn cache_invalidated_on_delete() {
        let mut editor = Editor::new();
        editor.insert_text("hello");
        editor.ensure_cache(80);

        editor.handle_key(&KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        let lines = editor.ensure_cache(80);
        assert_eq!(lines, &["hell".to_string()]);
    }

    #[test]
    fn cache_invalidated_on_take_text() {
        let mut editor = Editor::new();
        editor.insert_text("hello world");
        editor.ensure_cache(80);

        editor.take_text();
        let lines = editor.ensure_cache(80);
        // Empty buffer wraps to no lines (empty vec).
        assert_eq!(lines, &[] as &[String]);
    }

    #[test]
    fn cache_reused_when_buffer_unchanged() {
        let mut editor = Editor::new();
        editor.insert_text("hello");
        // First access populates cache; second immediately reuses it
        let n1 = editor.wrapped_line_count(80);
        let n2 = editor.wrapped_line_count(80);
        assert_eq!(n1, n2);
        assert_eq!(n1, 1);
    }

    #[test]
    fn cache_invalidated_on_width_change() {
        let mut editor = Editor::new();
        editor.insert_text("hello world");
        editor.ensure_cache(80);
        // Different width → recompute. At width 5, "hello world" wraps to 3 lines:
        // "hello", " worl", "d"
        let lines2 = editor.ensure_cache(5);
        assert_eq!(lines2.len(), 3);
        assert_eq!(lines2[0], "hello");
        assert_eq!(lines2[1], " worl");
        assert_eq!(lines2[2], "d");
    }

    #[test]
    fn visible_text_uses_cache() {
        let mut editor = Editor::new();
        editor.insert_text("line1\nline2\nline3\nline4");
        let visible = editor.visible_text(80, 2);
        assert_eq!(visible, "line1\nline2");
        // Second call should use cache (same buffer, same width)
        let visible2 = editor.visible_text(80, 1);
        assert_eq!(visible2, "line1");
    }

    #[test]
    fn wrapped_line_count_uses_cache() {
        let mut editor = Editor::new();
        editor.insert_text("line1\nline2\nline3");
        let n1 = editor.wrapped_line_count(80);
        assert_eq!(n1, 3);
        // No change — cache hit
        let n2 = editor.wrapped_line_count(80);
        assert_eq!(n2, 3);
    }

    // ── O(n) wrap_lines tests ─────────────────────────────────────────

    #[test]
    fn wrap_lines_linear_perf() {
        // With the O(n²) bug fixed, wrapping a long line should be fast.
        let text = "x".repeat(5000);
        let lines = wrap_lines(&text, 80);
        // 5000 / 80 = 62.5 → 63 lines
        assert_eq!(lines.len(), 63);
    }

    #[test]
    fn wrap_lines_matches_wrap_position() {
        // These two functions should agree on wrapping boundaries.
        let text = "abcdefghijklmnop";
        // width 4: abc\d, efgh, ijkl, mnop
        let lines = wrap_lines(text, 4);
        let (cursor_line, _) = wrap_position(text, 4);
        // cursor at end → last line index = total_lines - 1
        assert_eq!(cursor_line, lines.len() - 1);
    }

    // ── Scroll / wrapping tests ───────────────────────────────────────

    #[test]
    fn cursor_wrapped_line_single_line() {
        let mut editor = Editor::new();
        editor.insert_text("hello");
        assert_eq!(editor.cursor_wrapped_line(80), 0);
    }

    #[test]
    fn cursor_wrapped_line_with_wrap() {
        let mut editor = Editor::new();
        // "abcdefghij" wraps at width 3 → lines: abc, def, ghi, j
        editor.insert_text("abcdefghij");
        editor.move_home(); // cursor at 0
        assert_eq!(editor.cursor_wrapped_line(3), 0);
        // At byte 4 the character 'd' has wrapped to line 1.
        for _ in 0..4 {
            editor.move_right();
        }
        assert_eq!(editor.cursor_wrapped_line(3), 1); // 'd' starts line 1
    }

    #[test]
    fn scroll_to_cursor_basic() {
        let mut editor = Editor::new();
        // Fill with many lines
        for _ in 0..10 {
            editor.insert_text("line\n");
        }
        assert_eq!(editor.scroll_offset(), 0);
        // Cursor at end (line 10), visible_rows = 2
        editor.scroll_to_cursor(80, 2);
        assert_eq!(editor.scroll_offset(), 9); // cursor at line 10, visible [9,10]
    }

    #[test]
    fn scroll_to_cursor_does_not_overscroll() {
        let mut editor = Editor::new();
        editor.insert_text("short");
        assert_eq!(editor.scroll_offset(), 0);
        editor.scroll_to_cursor(80, 4);
        assert_eq!(editor.scroll_offset(), 0); // fits in view
    }

    #[test]
    fn visible_text_extracts_correct_slice() {
        let mut editor = Editor::new();
        editor.insert_text("line1\nline2\nline3\nline4");
        editor.scroll_offset = 1; // skip first line
        let visible = editor.visible_text(80, 2);
        assert_eq!(visible, "line2\nline3");
    }

    #[test]
    fn visible_text_wrapping() {
        let mut editor = Editor::new();
        editor.insert_text("abcdefghij"); // 10 chars
        // At width 3, wraps to 4 lines: abc, def, ghi, j
        let visible = editor.visible_text(3, 2);
        assert_eq!(visible, "abc\ndef");
    }

    #[test]
    fn visible_text_with_scroll_and_wrap() {
        let mut editor = Editor::new();
        editor.insert_text("abcdefghijklmno"); // 15 chars
        // At width 3, wraps to 5 lines: abc, def, ghi, jkl, mno
        editor.scroll_offset = 1; // skip "abc"
        let visible = editor.visible_text(3, 2);
        assert_eq!(visible, "def\nghi");
    }
}
