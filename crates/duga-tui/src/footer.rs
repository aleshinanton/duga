//! Footer widget — context-aware shortcut bar.
//!
//! Renders a single-line footer showing keyboard shortcuts.
//! Shortcuts vary based on current focus and app state.
//! Format: `F1 Help | Ctrl+L Clear | Ctrl+R Retry | Tab Focus | q Quit`

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::app::{App, AppState};
use crate::theme::Theme;

/// Stateless footer widget.
pub struct FooterView;

impl FooterView {
    /// Render the footer into the given area.
    pub fn render(area: Rect, buf: &mut Buffer, app: &App, theme: &Theme) {
        let shortcuts = get_shortcuts(app);
        let num_shortcuts = shortcuts.len();

        let spans: Vec<Span> = shortcuts
            .into_iter()
            .enumerate()
            .flat_map(|(i, s)| {
                let mut v = vec![Span::styled(s, theme.text_dim_style())];
                if i < num_shortcuts - 1 {
                    v.push(Span::styled(" | ", Style::default().fg(theme.colors.muted)));
                }
                v
            })
            .collect();

        let footer = Paragraph::new(Line::from(spans))
            .style(Style::default().bg(theme.colors.surface));

        footer.render(area, buf);
    }
}

/// Select shortcuts based on app state and focus.
fn get_shortcuts(app: &App) -> Vec<&'static str> {
    match &app.state {
        AppState::Idle => {
            vec![
                "F1 Help",
                "Ctrl+L Clear",
                "Tab Focus",
                "Ctrl+S Sessions",
                "q Quit",
            ]
        }
        AppState::Running {
            cancel_requested,
            ..
        } => {
            if *cancel_requested {
                vec![
                    "Cancelling…",
                    "Ctrl+R Retry",
                    "Esc Back",
                    "q Force Quit",
                ]
            } else {
                vec![
                    "Ctrl+C Cancel",
                    "Ctrl+G Steer",
                    "Tab Focus",
                    "q Quit (after run)",
                ]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn footer_idle_shows_correct_shortcuts() {
        let shortcuts = get_shortcuts_for_state(&AppState::Idle);
        let all: String = shortcuts.join(" ");
        assert!(all.contains("F1 Help"));
        assert!(all.contains("q Quit"));
    }

    #[test]
    fn footer_running_shows_cancel() {
        let shortcuts = get_shortcuts_for_state(&AppState::Running {
            run_id: 0,
            cancel_requested: false,
            loop_name: "simple_react".into(),
        });
        let all: String = shortcuts.join(" ");
        assert!(all.contains("Ctrl+C Cancel"));
    }

    #[test]
    fn footer_cancelling_shows_retry() {
        let shortcuts = get_shortcuts_for_state(&AppState::Running {
            run_id: 0,
            cancel_requested: true,
            loop_name: "simple_react".into(),
        });
        let all: String = shortcuts.join(" ");
        assert!(all.contains("Ctrl+R Retry"));
    }

    // Helper
    fn get_shortcuts_for_state(state: &AppState) -> Vec<&'static str> {
        match state {
            AppState::Idle => vec![
                "F1 Help",
                "Ctrl+L Clear",
                "Tab Focus",
                "Ctrl+S Sessions",
                "q Quit",
            ],
            AppState::Running {
                cancel_requested,
                ..
            } => {
                if *cancel_requested {
                    vec![
                        "Cancelling…",
                        "Ctrl+R Retry",
                        "Esc Back",
                        "q Force Quit",
                    ]
                } else {
                    vec![
                        "Ctrl+C Cancel",
                        "Ctrl+G Steer",
                        "Tab Focus",
                        "q Quit (after run)",
                    ]
                }
            }
        }
    }
}
