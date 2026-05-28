//! Help screen overlay showing keybindings and commands.

use crate::keybindings::Keybindings;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    prelude::{StatefulWidget, Widget},
    style::{Color, Style},
    widgets::{Block, Borders, Scrollbar, ScrollbarOrientation, ScrollbarState},
};
use super::{Overlay, OverlayAction};

/// A help section with title and keybinding items.
struct HelpSection {
    title: &'static str,
    items: Vec<HelpItem>,
}

struct HelpItem {
    key: String,
    description: &'static str,
}

pub struct HelpOverlay {
    sections: Vec<HelpSection>,
    scroll_offset: u16,
}

impl HelpOverlay {
    pub fn new(keybindings: &Keybindings) -> Self {
        let cancel_str = keybinding_to_string_cancel(keybindings);
        let quit_str = keybinding_to_string_quit(keybindings);
        let submit_str = keybinding_to_string(keybindings);

        let sections = vec![
            HelpSection {
                title: "Session",
                items: vec![
                    HelpItem { key: "Enter".into(), description: "Submit prompt / Send steering (while running)" },
                    HelpItem { key: cancel_str, description: "Cancel current run" },
                    HelpItem { key: "Ctrl+L".into(), description: "Clear transcript" },
                    HelpItem { key: quit_str, description: "Quit (when idle)" },
                ],
            },
            HelpSection {
                title: "Navigation",
                items: vec![
                    HelpItem { key: "Up / Down".into(), description: "Navigate history / scroll" },
                    HelpItem { key: "PageUp / PageDown".into(), description: "Scroll transcript" },
                    HelpItem { key: "Home / End".into(), description: "Top / bottom of transcript" },
                ],
            },
            HelpSection {
                title: "Overlays",
                items: vec![
                    HelpItem { key: "F1".into(), description: "Toggle help" },
                    HelpItem { key: "Ctrl+F".into(), description: "Search transcript" },
                    HelpItem { key: "Ctrl+S".into(), description: "Browse & resume sessions" },
                    HelpItem { key: "Escape".into(), description: "Close overlay / dismiss dialog" },
                ],
            },
            HelpSection {
                title: "Collapse / Expand",
                items: vec![
                    HelpItem { key: "Tab".into(), description: "Collapse or expand all tool outputs" },
                ],
            },
            HelpSection {
                title: "Editor",
                items: vec![
                    HelpItem { key: "Shift+Enter".into(), description: "New line" },
                    HelpItem { key: "Ctrl+Left / Ctrl+Right".into(), description: "Word jump" },
                    HelpItem { key: "Backspace / Delete".into(), description: "Character deletion" },
                ],
            },
            HelpSection {
                title: "Other",
                items: vec![
                    HelpItem { key: submit_str, description: "Submit prompt (from editor)" },
                    HelpItem { key: "Ctrl+C".into(), description: "Cancel run" },
                ],
            },
        ];

        Self {
            sections,
            scroll_offset: 0,
        }
    }
}

impl Overlay for HelpOverlay {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        // Dim background
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(' ').set_bg(Color::Rgb(20, 20, 20));
                }
            }
        }

        // Calculate dialog area (centered, 70% of terminal)
        let dialog_w = ((area.width as f32) * 0.7) as u16;
        let dialog_h = ((area.height as f32) * 0.8) as u16;
        let dialog_x = area.x + (area.width.saturating_sub(dialog_w)) / 2;
        let dialog_y = area.y + (area.height.saturating_sub(dialog_h)) / 2;
        let dialog_area = Rect::new(dialog_x, dialog_y, dialog_w, dialog_h);

        // Render dialog box
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Help — Keybindings ")
            .border_style(Style::default().fg(Color::Cyan))
            .style(Style::default().bg(Color::Rgb(30, 30, 30)));
        let inner = block.inner(dialog_area);
        block.render(dialog_area, buf);

        // Render content
        let mut all_lines: Vec<String> = Vec::new();
        for section in &self.sections {
            all_lines.push(format!("── {} ──", section.title));
            for item in &section.items {
                all_lines.push(format!("  {:20}  {}", item.key, item.description));
            }
            all_lines.push(String::new());
        }
        all_lines.push("Press Escape or F1 to close.".into());

        let available_lines = inner.height as usize;
        let total_lines = all_lines.len();
        let max_offset = total_lines.saturating_sub(available_lines);

        for (i, line) in all_lines
            .iter()
            .skip(self.scroll_offset as usize)
            .take(available_lines)
            .enumerate()
        {
            let y = inner.y + i as u16;
            if y < inner.bottom() {
                let truncated = if line.len() > inner.width as usize {
                    &line[..inner.width as usize]
                } else {
                    line
                };
                ratatui::text::Line::from(truncated.to_string()).render(
                    Rect::new(inner.x, y, inner.width, 1),
                    buf,
                );
            }
        }

        // Render scrollbar
        if total_lines > available_lines {
            let mut scrollbar_state =
                ScrollbarState::new(max_offset).position(self.scroll_offset as usize);
            let scroll_area = Rect::new(
                inner.x + inner.width.saturating_sub(1),
                inner.y,
                1,
                inner.height,
            );
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .render(scroll_area, buf, &mut scrollbar_state);
        }
    }

    fn handle_key(&mut self, key: &KeyEvent) -> OverlayAction {
        match key {
            KeyEvent {
                code: KeyCode::Esc, ..
            }
            | KeyEvent {
                code: KeyCode::F(1), ..
            } => OverlayAction::Close,
            KeyEvent {
                code: KeyCode::Up, ..
            }
            | KeyEvent {
                code: KeyCode::Char('k'),
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
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
                self.scroll_offset = self.scroll_offset.saturating_add(1);
                OverlayAction::Consumed
            }
            KeyEvent {
                code: KeyCode::PageUp, ..
            } => {
                self.scroll_offset = self.scroll_offset.saturating_sub(10);
                OverlayAction::Consumed
            }
            KeyEvent {
                code: KeyCode::PageDown, ..
            } => {
                self.scroll_offset = self.scroll_offset.saturating_add(10);
                OverlayAction::Consumed
            }
            _ => OverlayAction::Ignored,
        }
    }

    fn position(&self, _terminal_width: u16, _terminal_height: u16) -> Rect {
        Rect::new(0, 0, _terminal_width, _terminal_height)
    }
}

fn keybinding_to_string(kb: &Keybindings) -> String {
    if kb.submit.matches(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)) {
        "Enter".into()
    } else {
        "custom-submit".into()
    }
}

fn keybinding_to_string_cancel(kb: &Keybindings) -> String {
    if kb.cancel.matches(&KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)) {
        "Ctrl+C".into()
    } else {
        "custom-cancel".into()
    }
}

fn keybinding_to_string_quit(kb: &Keybindings) -> String {
    if kb.quit.matches(&KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)) {
        "q".into()
    } else {
        "custom-quit".into()
    }
}
