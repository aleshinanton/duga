//! Session picker overlay — browse, resume, and delete past sessions.

use crate::session::SessionInfo;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    prelude::Widget,
    style::{Color, Modifier, Style},
    widgets::{Block, Borders},
};
use super::{Overlay, OverlayAction};
use tokio::sync::mpsc::UnboundedSender;

pub struct SessionPickerOverlay {
    sessions: Vec<SessionInfo>,
    selected: usize,
    scroll_offset: usize,
    result_tx: UnboundedSender<String>,
    /// Channel to request deletion of the selected session.
    delete_tx: Option<UnboundedSender<String>>,
    /// Whether a delete has been requested (waiting for confirmation).
    delete_requested: bool,
}

impl SessionPickerOverlay {
    pub fn new(
        sessions: Vec<SessionInfo>,
        result_tx: UnboundedSender<String>,
        delete_tx: Option<UnboundedSender<String>>,
    ) -> Self {
        Self {
            sessions,
            selected: 0,
            scroll_offset: 0,
            result_tx,
            delete_tx,
            delete_requested: false,
        }
    }

    /// Update the session list (e.g. after a deletion).
    pub fn set_sessions(&mut self, sessions: Vec<SessionInfo>) {
        self.sessions = sessions;
        if self.selected >= self.sessions.len() {
            self.selected = self.sessions.len().saturating_sub(1);
        }
        if self.scroll_offset > self.selected {
            self.scroll_offset = self.selected;
        }
    }

    fn select_current(&self) -> OverlayAction {
        if let Some(session) = self.sessions.get(self.selected) {
            let _ = self.result_tx.send(session.id.clone());
        }
        OverlayAction::Close
    }

    fn request_delete(&mut self) -> OverlayAction {
        if let Some(ref tx) = self.delete_tx {
            if let Some(session) = self.sessions.get(self.selected) {
                let _ = tx.send(session.id.clone());
                self.delete_requested = true;
            }
        }
        OverlayAction::Consumed
    }
}

impl Overlay for SessionPickerOverlay {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        // Dim background
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(' ').set_bg(Color::Rgb(20, 20, 20));
                }
            }
        }

        // Dialog: 80 % wide, 60 % tall, centered
        let dialog_w = ((area.width as f32) * 0.8) as u16;
        let dialog_h = ((area.height as f32) * 0.6) as u16;
        let dialog_x = area.x + (area.width.saturating_sub(dialog_w)) / 2;
        let dialog_y = area.y + (area.height.saturating_sub(dialog_h)) / 2;
        let dialog_area = Rect::new(dialog_x, dialog_y, dialog_w, dialog_h);

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Sessions — ↑↓ navigate · Enter resume · Del delete · Esc cancel ")
            .border_style(Style::default().fg(Color::Cyan))
            .style(Style::default().bg(Color::Rgb(30, 30, 30)));
        let inner = block.inner(dialog_area);
        block.render(dialog_area, buf);

        if self.sessions.is_empty() {
            ratatui::text::Line::from(
                ratatui::text::Span::styled(
                    "No sessions yet.",
                    Style::default().fg(Color::DarkGray),
                ),
            )
            .render(Rect::new(inner.x, inner.y, inner.width, 1), buf);
            return;
        }

        let visible = inner.height as usize;
        let title_w = (inner.width.saturating_sub(16)) as usize; // reserve right column for time

        for (row, session) in self
            .sessions
            .iter()
            .enumerate()
            .skip(self.scroll_offset)
            .take(visible)
        {
            let y = inner.y + (row - self.scroll_offset) as u16;
            let is_selected = row == self.selected;

            let bg = if is_selected {
                Color::Rgb(50, 80, 120)
            } else {
                Color::Rgb(30, 30, 30)
            };
            let fg = if is_selected {
                Color::White
            } else {
                Color::Gray
            };

            // Fill row background
            for x in inner.x..inner.x + inner.width {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(' ').set_bg(bg);
                }
            }

            // Title (truncated)
            let title: String = session.title.chars().take(title_w).collect();
            let padded = format!(" {:<width$}", title, width = title_w);

            let title_style = if is_selected {
                Style::default()
                    .fg(fg)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(fg).bg(bg)
            };

            ratatui::text::Line::from(ratatui::text::Span::styled(padded, title_style))
                .render(Rect::new(inner.x, y, inner.width.saturating_sub(14), 1), buf);

            // Time (right-aligned)
            let time = session.relative_time();
            let time_str = format!("{:>13}", time);
            let time_style = Style::default().fg(Color::DarkGray).bg(bg);
            let time_x = inner.x + inner.width.saturating_sub(14);
            ratatui::text::Line::from(ratatui::text::Span::styled(time_str, time_style))
                .render(Rect::new(time_x, y, 14, 1), buf);
        }
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
            } => self.select_current(),
            KeyEvent {
                code: KeyCode::Delete, ..
            }
            | KeyEvent {
                code: KeyCode::Char('d'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => self.request_delete(),
            KeyEvent {
                code: KeyCode::Up, ..
            }
            | KeyEvent {
                code: KeyCode::Char('k'),
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                self.selected = self.selected.saturating_sub(1);
                if self.selected < self.scroll_offset {
                    self.scroll_offset = self.selected;
                }
                OverlayAction::Consumed
            }
            KeyEvent {
                code: KeyCode::Down, ..
            }
            | KeyEvent {
                code: KeyCode::Char('j'),
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                if !self.sessions.is_empty() {
                    self.selected = (self.selected + 1).min(self.sessions.len() - 1);
                }
                OverlayAction::Consumed
            }
            KeyEvent {
                code: KeyCode::PageUp, ..
            } => {
                self.selected = self.selected.saturating_sub(10);
                if self.selected < self.scroll_offset {
                    self.scroll_offset = self.selected;
                }
                OverlayAction::Consumed
            }
            KeyEvent {
                code: KeyCode::PageDown,
                ..
            } => {
                if !self.sessions.is_empty() {
                    self.selected = (self.selected + 10).min(self.sessions.len() - 1);
                }
                OverlayAction::Consumed
            }
            _ => OverlayAction::Ignored,
        }
    }

    fn position(&self, terminal_width: u16, terminal_height: u16) -> Rect {
        Rect::new(0, 0, terminal_width, terminal_height)
    }
}
