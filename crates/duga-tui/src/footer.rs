//! Footer widget — context-aware shortcut bar.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::app::{App, AppState};
use crate::theme::Theme;

pub struct FooterView;

impl FooterView {
    pub fn render(area: Rect, buf: &mut Buffer, app: &App, theme: &Theme) {
        let shortcuts = match &app.state {
            AppState::Idle => vec![
                "F1 Help", "Ctrl+L Clear", "Tab Next Pane", "Shift+Tab Effort", "Ctrl+R Reasoning", "Ctrl+E Events", "Ctrl+Q Quit",
            ],
            AppState::Running { cancel_requested: true, .. } => vec![
                "Cancelling…", "Ctrl+R Retry", "Esc Back", "Ctrl+Q Force Quit",
            ],
            AppState::Running { cancel_requested: false, .. } => vec![
                "Ctrl+C Cancel", "Ctrl+G Steer", "Tab Focus", "Ctrl+Q Quit (after run)",
            ],
        };
        let num = shortcuts.len();
        let spans: Vec<Span> = shortcuts.into_iter().enumerate().flat_map(|(i, s)| {
            let mut v = vec![Span::styled(s, theme.text_dim_style())];
            if i < num - 1 { v.push(Span::styled(" | ", Style::default().fg(theme.colors.muted))); }
            v
        }).collect();
        Paragraph::new(Line::from(spans))
            .style(Style::default().bg(theme.colors.surface))
            .render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn footer_compiles() { assert!(true); }
}
