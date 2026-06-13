//! Help screen overlay showing keybindings and commands.

use crate::keybindings::Keybindings;
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    widgets::{Block, Borders, Scrollbar, ScrollbarOrientation, ScrollbarState, Widget},
};
use ratatui::widgets::StatefulWidget;
use super::{Overlay, OverlayAction};

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
    colors: HelpColors,
}

#[derive(Clone)]
struct HelpColors {
    bg: ratatui::style::Color,
    dialog_bg: ratatui::style::Color,
    border: ratatui::style::Color,
}

impl HelpOverlay {
    pub fn new(keybindings: &Keybindings, theme: &Theme) -> Self {
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
                    HelpItem { key: "Tab / Shift+Tab".into(), description: "Cycle focus (Chat / Sidebar / Input)" },
                    HelpItem { key: "j / k / ↑ / ↓".into(), description: "Scroll (Chat & Sidebar focus)" },
                    HelpItem { key: "PgUp / PgDn".into(), description: "Scroll page (Chat focus)" },
                    HelpItem { key: "g / G".into(), description: "Jump bottom / top (Chat focus)" },
                    HelpItem { key: "Esc".into(), description: "Return focus to Chat" },
                ],
            },
            HelpSection {
                title: "Focus & Sidebar",
                items: vec![
                    HelpItem { key: "Tab / Shift+Tab".into(), description: "Cycle focus (Chat / Sidebar / Input)" },
                    HelpItem { key: "Ctrl+R".into(), description: "Toggle reasoning panel" },
                    HelpItem { key: "Ctrl+E".into(), description: "Toggle event log panel" },
                    HelpItem { key: "Esc".into(), description: "Return focus to Chat" },
                ],
            },
            HelpSection {
                title: "Editor",
                items: vec![
                    HelpItem { key: "Shift+Enter".into(), description: "New line" },
                    HelpItem { key: "Ctrl+Left / Ctrl+Right".into(), description: "Word jump" },
                    HelpItem { key: "Up / Down".into(), description: "History navigation" },
                ],
            },
            HelpSection {
                title: "Other",
                items: vec![
                    HelpItem { key: submit_str, description: "Submit prompt (from editor)" },
                    HelpItem { key: "Tab".into(), description: "Toggle tool / thinking expand" },
                    HelpItem { key: "Ctrl+C".into(), description: "Cancel run" },
                ],
            },
        ];

        Self {
            sections,
            scroll_offset: 0,
            colors: HelpColors {
                bg: theme.colors.bg,
                dialog_bg: theme.colors.surface,
                border: theme.colors.primary,
            },
        }
    }
}

impl Overlay for HelpOverlay {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        // Dim background
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(' ').set_bg(self.colors.bg);
                }
            }
        }

        let dialog_w = ((area.width as f32) * 0.7) as u16;
        let dialog_h = ((area.height as f32) * 0.85) as u16;
        let dialog_x = area.x + (area.width.saturating_sub(dialog_w)) / 2;
        let dialog_y = area.y + (area.height.saturating_sub(dialog_h)) / 2;
        let dialog_area = Rect::new(dialog_x, dialog_y, dialog_w, dialog_h);

        let block = Block::default()
            .borders(Borders::ALL).border_set(ratatui::symbols::border::ROUNDED)
            .title(" Help — Keybindings ")
            .border_style(Style::default().fg(self.colors.border))
            .style(Style::default().bg(self.colors.dialog_bg));
        let inner = block.inner(dialog_area);
        block.render(dialog_area, buf);

        let mut all_lines: Vec<String> = Vec::new();
        for section in &self.sections {
            all_lines.push(format!("-- {} --", section.title));
            for item in &section.items {
                all_lines.push(format!("  {:22}  {}", item.key, item.description));
            }
            all_lines.push(String::new());
        }
        all_lines.push("Press Escape or F1 to close.".into());

        let available = inner.height as usize;
        let total = all_lines.len();
        let max_off = total.saturating_sub(available);

        for (i, line) in all_lines.iter().skip(self.scroll_offset as usize).take(available).enumerate() {
            let y = inner.y + i as u16;
            if y < inner.bottom() {
                let truncated = if line.len() > inner.width as usize { &line[..inner.width as usize] } else { line };
                ratatui::text::Line::from(truncated.to_string()).render(
                    Rect::new(inner.x, y, inner.width, 1), buf,
                );
            }
        }

        if total > available {
            let mut sb = ScrollbarState::new(max_off).position(self.scroll_offset as usize);
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None).end_symbol(None)
                .render(
                    Rect::new(inner.x + inner.width.saturating_sub(1), inner.y, 1, inner.height),
                    buf, &mut sb,
                );
        }
    }

    fn handle_key(&mut self, key: &KeyEvent) -> OverlayAction {
        match key {
            KeyEvent { code: KeyCode::Esc, .. }
            | KeyEvent { code: KeyCode::F(1), .. } => OverlayAction::Close,
            KeyEvent { code: KeyCode::Up, .. }
            | KeyEvent { code: KeyCode::Char('k'), modifiers: KeyModifiers::NONE, .. } => {
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
                OverlayAction::Consumed
            }
            KeyEvent { code: KeyCode::Down, .. }
            | KeyEvent { code: KeyCode::Char('j'), modifiers: KeyModifiers::NONE, .. } => {
                self.scroll_offset = self.scroll_offset.saturating_add(1);
                OverlayAction::Consumed
            }
            KeyEvent { code: KeyCode::PageUp, .. } => {
                self.scroll_offset = self.scroll_offset.saturating_sub(10);
                OverlayAction::Consumed
            }
            KeyEvent { code: KeyCode::PageDown, .. } => {
                self.scroll_offset = self.scroll_offset.saturating_add(10);
                OverlayAction::Consumed
            }
            _ => OverlayAction::Ignored,
        }
    }

    fn position(&self, w: u16, h: u16) -> Rect {
        Rect::new(0, 0, w, h)
    }
}

fn keybinding_to_string(kb: &Keybindings) -> String {
    if kb.submit.matches(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)) { "Enter".into() } else { "custom-submit".into() }
}
fn keybinding_to_string_cancel(kb: &Keybindings) -> String {
    if kb.cancel.matches(&KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)) { "Ctrl+C".into() } else { "custom-cancel".into() }
}
fn keybinding_to_string_quit(kb: &Keybindings) -> String {
    if kb.quit.matches(&KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)) { "q".into() } else { "custom-quit".into() }
}
