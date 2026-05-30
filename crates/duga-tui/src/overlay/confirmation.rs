//! Confirmation dialog overlay for risky tool execution.
//!
//! Renders a centered modal with title, prompt, and selectable options.
//! Input is captured exclusively until the dialog is dismissed.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::{Block, Borders},
    prelude::Widget,
};
use super::{Overlay, OverlayAction};

/// Result of a confirmation dialog.
#[derive(Clone, Debug)]
pub enum ConfirmationResult {
    /// User confirmed (index of selected option).
    Confirmed(usize),
    /// User cancelled.
    Cancelled,
    /// User dismissed (Escape pressed).
    Dismissed,
}

/// A confirmation dialog with a title, prompt, and options.
pub struct ConfirmationDialog {
    title: String,
    prompt: String,
    options: Vec<String>,
    selected: usize,
    result: Option<ConfirmationResult>,
}

impl ConfirmationDialog {
    pub fn new(
        title: impl Into<String>,
        prompt: impl Into<String>,
        options: Vec<String>,
    ) -> Self {
        Self {
            title: title.into(),
            prompt: prompt.into(),
            options,
            selected: 0,
            result: None,
        }
    }

    /// Take the result (if any).
    pub fn take_result(&mut self) -> Option<ConfirmationResult> {
        self.result.take()
    }

    /// Standard yes/no dialog.
    pub fn yes_no(title: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self::new(
            title,
            prompt,
            vec!["Yes".into(), "No".into()],
        )
    }

    /// Yes/no/cancel dialog.
    pub fn yes_no_cancel(title: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self::new(
            title,
            prompt,
            vec!["Yes".into(), "No".into(), "Cancel".into()],
        )
    }
}

impl Overlay for ConfirmationDialog {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        // Dim background
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(' ').set_bg(Color::Rgb(10, 10, 10));
                }
            }
        }

        // Calculate dialog size (at most 60% of terminal)
        let dialog_w = ((area.width as f32) * 0.6) as u16;
        let prompt_lines = wrap_lines(&self.prompt, dialog_w.saturating_sub(4) as usize);
        let dialog_h = (prompt_lines.len() as u16) + self.options.len() as u16 + 6;
        let dialog_x = area.x + (area.width.saturating_sub(dialog_w)) / 2;
        let dialog_y = area.y + (area.height.saturating_sub(dialog_h)) / 2;
        let dialog_area = Rect::new(dialog_x, dialog_y, dialog_w, dialog_h);

        // Render dialog box
        let block = Block::default()
            .borders(Borders::ALL).border_set(ratatui::symbols::border::ROUNDED)
            .title(format!(" {} ", self.title))
            .border_style(Style::default().fg(Color::Yellow))
            .style(Style::default().bg(Color::Rgb(30, 30, 30)));
        let inner = block.inner(dialog_area);
        block.render(dialog_area, buf);

        // Render prompt
        let mut y = inner.y;
        for line in &prompt_lines {
            if y < inner.bottom() {
                let truncated = if line.len() > inner.width as usize {
                    &line[..inner.width as usize]
                } else {
                    line
                };
                ratatui::text::Line::from(truncated.to_string())
                    .style(Style::default().fg(Color::White))
                    .render(Rect::new(inner.x, y, inner.width, 1), buf);
            }
            y += 1;
        }
        y += 1; // Blank line between prompt and options

        // Render options
        for (idx, option) in self.options.iter().enumerate() {
            if y < inner.bottom() {
                let is_selected = idx == self.selected;
                let prefix = if is_selected { "▸ " } else { "  " };
                let text = format!("{prefix}{option}");
                let style = if is_selected {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Gray).bg(Color::Rgb(30, 30, 30))
                };
                let truncated = if text.len() > inner.width as usize {
                    &text[..inner.width as usize]
                } else {
                    &text
                };
                ratatui::text::Line::from(truncated.to_string())
                    .style(style)
                    .render(Rect::new(inner.x, y, inner.width, 1), buf);
            }
            y += 1;
        }

        // Hint at bottom
        let hint_y = inner.bottom().saturating_sub(1);
        if hint_y < inner.bottom() && hint_y >= inner.y {
            let hint = "↑/↓ select  |  Enter confirm  |  Esc dismiss";
            ratatui::text::Line::from(hint)
                .style(Style::default().fg(Color::DarkGray))
                .render(
                    Rect::new(inner.x, hint_y, inner.width, 1),
                    buf,
                );
        }
    }

    fn handle_key(&mut self, key: &KeyEvent) -> OverlayAction {
        match key {
            KeyEvent {
                code: KeyCode::Esc, ..
            } => {
                self.result = Some(ConfirmationResult::Dismissed);
                OverlayAction::Close
            }
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                self.result = Some(ConfirmationResult::Confirmed(self.selected));
                OverlayAction::Close
            }
            KeyEvent {
                code: KeyCode::Up, ..
            } => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
                OverlayAction::Consumed
            }
            KeyEvent {
                code: KeyCode::Down, ..
            } => {
                if self.selected + 1 < self.options.len() {
                    self.selected += 1;
                }
                OverlayAction::Consumed
            }
            // Quick keys: y/n
            KeyEvent {
                code: KeyCode::Char('y'),
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                if self.options.iter().any(|o| o.to_lowercase() == "yes") {
                    self.selected = self
                        .options
                        .iter()
                        .position(|o| o.to_lowercase() == "yes")
                        .unwrap_or(0);
                    self.result = Some(ConfirmationResult::Confirmed(self.selected));
                    return OverlayAction::Close;
                }
                OverlayAction::Consumed
            }
            KeyEvent {
                code: KeyCode::Char('n'),
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                if self.options.iter().any(|o| o.to_lowercase() == "no") {
                    self.result = Some(ConfirmationResult::Cancelled);
                    return OverlayAction::Close;
                }
                OverlayAction::Consumed
            }
            _ => OverlayAction::Ignored,
        }
    }

    fn position(&self, _terminal_width: u16, _terminal_height: u16) -> Rect {
        Rect::new(0, 0, _terminal_width, _terminal_height)
    }
}

/// Wrap text into lines for a given width.
fn wrap_lines(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![];
    }
    let mut lines = Vec::new();
    for raw_line in text.split('\n') {
        if raw_line.is_empty() {
            lines.push(String::new());
            continue;
        }
        if raw_line.len() <= max_width {
            lines.push(raw_line.to_string());
            continue;
        }
        let words: Vec<&str> = raw_line.split(' ').collect();
        let mut current = String::new();
        for word in words {
            if current.is_empty() {
                current.push_str(word);
            } else if current.len() + 1 + word.len() <= max_width {
                current.push(' ');
                current.push_str(word);
            } else {
                lines.push(current.clone());
                current = word.to_string();
            }
        }
        if !current.is_empty() {
            lines.push(current);
        }
    }
    lines
}
