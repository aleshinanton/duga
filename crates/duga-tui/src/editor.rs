//! Multi-line input editor for the TUI.
//!
//! Supports: multi-line input with word-wrap, cursor movement,
//! history navigation (Up/Down), paste, placeholder text.
//! Scroll offset is automatically adjusted during rendering so
//! the cursor line stays within the visible input area.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthChar;

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
        text
    }

    /// Insert text at cursor position (for paste, character input).
    pub fn insert_text(&mut self, text: &str) {
        self.buffer.insert_str(self.cursor, text);
        self.cursor += text.len();
        self.history_index = None;
    }

    /// Insert a character at cursor position.
    pub fn insert_char(&mut self, ch: char) {
        self.buffer.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
        self.history_index = None;
    }

    /// Insert a newline at cursor position.
    pub fn insert_newline(&mut self) {
        self.insert_char('\n');
    }

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
        }
    }

    fn delete_after_cursor(&mut self) {
        if self.cursor < self.buffer.len() {
            let next = self.next_char_boundary(self.cursor);
            self.buffer.replace_range(self.cursor..next, "");
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
            }
            Some(_) => {
                // Reached the end of history; restore fresh buffer
                self.history_index = None;
                self.buffer.clear();
                self.cursor = 0;
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
    pub fn wrapped_line_count(&self, text_width: u16) -> usize {
        line_count(&self.buffer, text_width)
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
    pub fn visible_text(&self, text_width: u16, visible_rows: usize) -> String {
        extract_lines(&self.buffer, text_width, self.scroll_offset, visible_rows)
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

/// Count the number of wrapped lines for `text` at the given display width.
fn line_count(text: &str, width: u16) -> usize {
    if width == 0 || text.is_empty() {
        return 1; // one (possibly empty) line
    }
    let mut lines = 1usize;
    let mut col = 0usize;
    for ch in text.chars() {
        if ch == '\n' {
            lines += 1;
            col = 0;
        } else {
            let w = ch.width().unwrap_or(0).max(1);
            if col + w > width as usize {
                lines += 1;
                col = w;
            } else {
                col += w;
            }
        }
    }
    lines
}

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

/// Wrap `text` into lines according to display width, then return only
/// `count` lines starting at `skip`.
fn extract_lines(text: &str, width: u16, skip: usize, count: usize) -> String {
    let all = wrap_lines(text, width);
    if skip >= all.len() {
        return String::new();
    }
    let end = (skip + count).min(all.len());
    all[skip..end].join("\n")
}

/// Wrap `text` at `width` columns, returning a vector of display lines.
fn wrap_lines(text: &str, width: u16) -> Vec<String> {
    let w = width as usize;
    if w == 0 || text.is_empty() {
        return text.lines().map(|s| s.to_string()).collect();
    }
    let mut lines: Vec<String> = vec![String::new()];
    for ch in text.chars() {
        if ch == '\n' {
            lines.push(String::new());
        } else {
            let cw = ch.width().unwrap_or(0).max(1);
            let current = lines.last_mut().unwrap();
            // Measure current line display width.
            let cur_w: usize = current.chars().map(|c| c.width().unwrap_or(0).max(1)).sum();
            if cur_w + cw > w {
                lines.push(String::new());
            }
            lines.last_mut().unwrap().push(ch);
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

