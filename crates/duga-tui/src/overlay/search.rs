//! Search overlay for searching within the transcript.

use crate::editor::Editor;
use crate::transcript::Transcript;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    prelude::Widget,
    style::{Color, Style},
    widgets::{Block, Borders, Paragraph},
};
use super::{Overlay, OverlayAction};

pub struct SearchOverlay {
    pub input: Editor,
    pub matches: Vec<usize>,
    pub selected_match: Option<usize>,
    pub matched_text: Vec<String>,
}

impl SearchOverlay {
    pub fn new() -> Self {
        Self {
            input: Editor::new().with_placeholder("Search transcript…"),
            matches: Vec::new(),
            selected_match: None,
            matched_text: Vec::new(),
        }
    }

    /// Perform search on transcript items.
    pub fn search(&mut self, transcript: &Transcript) {
        let query = self.input.text().to_lowercase();
        self.matches.clear();
        self.matched_text.clear();

        if query.is_empty() {
            self.selected_match = None;
            return;
        }

        for (idx, item) in transcript.items().iter().enumerate() {
            let text = transcript_item_to_string(item);
            if text.to_lowercase().contains(&query) {
                self.matches.push(idx);
                let snippet = extract_snippet(&text, &query, 60);
                self.matched_text.push(snippet);
            }
        }

        if !self.matches.is_empty() {
            self.selected_match = Some(0);
        } else {
            self.selected_match = None;
        }
    }

    /// Move to next match.
    pub fn next_match(&mut self) {
        if self.matches.is_empty() {
            return;
        }
        if let Some(current) = self.selected_match {
            let next = (current + 1) % self.matches.len();
            self.selected_match = Some(next);
        }
    }

    /// Move to previous match.
    pub fn prev_match(&mut self) {
        if self.matches.is_empty() {
            return;
        }
        if let Some(current) = self.selected_match {
            let prev = if current == 0 {
                self.matches.len() - 1
            } else {
                current - 1
            };
            self.selected_match = Some(prev);
        }
    }
}

impl Overlay for SearchOverlay {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        // Dim background
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(' ').set_bg(Color::Rgb(15, 15, 15));
                }
            }
        }

        // Search bar at top
        let bar_h = 3u16;
        let bar_y = area.y;
        let bar_area = Rect::new(area.x, bar_y, area.width, bar_h);

        let input_text = if self.input.text().is_empty() {
            format!("🔍 {}", self.input.placeholder())
        } else {
            format!("🔍 {}", self.input.text())
        };

        let input_style = Style::default().fg(Color::White).bg(Color::Rgb(30, 30, 30));
        Paragraph::new(input_text)
            .style(input_style)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan))
                    .title(format!(
                        " Search — {} of {} matches ",
                        self.selected_match.map(|s| s + 1).unwrap_or(0),
                        self.matches.len()
                    )),
            )
            .render(bar_area, buf);

        // Results area below
        let results_y = bar_y + bar_h + 1;
        let results_h = area.height.saturating_sub(bar_h + 1);
        if results_h == 0 {
            return;
        }
        let results_area = Rect::new(area.x, results_y, area.width, results_h);

        let mut lines: Vec<String> = Vec::new();
        for (i, (item_idx, snippet)) in self
            .matches
            .iter()
            .zip(self.matched_text.iter())
            .enumerate()
        {
            let prefix = if Some(i) == self.selected_match {
                "> "
            } else {
                "  "
            };
            lines.push(format!("{prefix}[{item_idx}] {snippet}"));
        }
        if lines.is_empty() && !self.input.text().is_empty() {
            lines.push("No matches found.".into());
        }

        for (i, line) in lines.iter().take(results_h as usize).enumerate() {
            let y = results_area.y + i as u16;
            if y < results_area.bottom() {
                let truncated = if line.len() > results_area.width as usize {
                    &line[..results_area.width as usize]
                } else {
                    line
                };
                ratatui::text::Line::from(truncated.to_string()).render(
                    Rect::new(results_area.x, y, results_area.width, 1),
                    buf,
                );
            }
        }

        // Hint at bottom
        let hint_y = area.bottom().saturating_sub(1);
        let hint = "Enter/Shift+Enter: next/prev match  |  Escape: close";
        ratatui::text::Line::from(hint)
            .style(Style::default().fg(Color::DarkGray))
            .render(Rect::new(area.x, hint_y, area.width, 1), buf);
    }

    fn handle_key(&mut self, key: &KeyEvent) -> OverlayAction {
        match key {
            KeyEvent {
                code: KeyCode::Esc, ..
            } => OverlayAction::Close,
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                self.next_match();
                OverlayAction::Consumed
            }
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::SHIFT,
                ..
            } => {
                self.prev_match();
                OverlayAction::Consumed
            }
            KeyEvent {
                code: KeyCode::Char(ch),
                modifiers: KeyModifiers::NONE | KeyModifiers::SHIFT,
                ..
            } => {
                self.input.insert_char(*ch);
                OverlayAction::Consumed
            }
            KeyEvent {
                code: KeyCode::Backspace,
                ..
            } => {
                let _ = self.input.handle_key(key);
                OverlayAction::Consumed
            }
            _ => OverlayAction::Ignored,
        }
    }

    fn position(&self, _terminal_width: u16, _terminal_height: u16) -> Rect {
        Rect::new(0, 0, _terminal_width, _terminal_height)
    }
}

fn transcript_item_to_string(item: &crate::transcript::TranscriptItem) -> String {
    match item {
        crate::transcript::TranscriptItem::SystemMessage { text, .. } => text.clone(),
        crate::transcript::TranscriptItem::UserMessage { text, .. } => text.clone(),
        crate::transcript::TranscriptItem::AssistantMessage { text, .. } => text.clone(),
        crate::transcript::TranscriptItem::ToolCallBlock {
            tool_name,
            description,
            ..
        } => format!("{tool_name}: {description}"),
        crate::transcript::TranscriptItem::DelegationNotice {
            from, to, reason, ..
        } => format!("{from} → {to}: {reason}"),
        crate::transcript::TranscriptItem::MemoryNotice {
            before_tokens,
            after_tokens,
            ..
        } => format!("memory: {before_tokens} → {after_tokens} tokens"),
    }
}

fn extract_snippet(text: &str, query: &str, max_len: usize) -> String {
    let lower = text.to_lowercase();
    if let Some(pos) = lower.find(query) {
        let start = pos.saturating_sub(max_len / 2);
        let end = (pos + query.len() + max_len / 2).min(text.len());
        let mut snippet = String::new();
        if start > 0 {
            snippet.push_str("…");
        }
        snippet.push_str(&text[start..end]);
        if end < text.len() {
            snippet.push_str("…");
        }
        snippet
    } else {
        let end = text.len().min(max_len);
        text[..end].to_string()
    }
}
