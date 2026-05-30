//! Reasoning panel — shows thinking content in sidebar with duration tracking.
//!
//! Auto-expands during streaming, auto-collapses after completion.
//! Shows duration (time from first to last thinking delta) and word count.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use std::time::Instant;

use crate::theme::Theme;

/// Reasoning panel state.
#[derive(Clone, Debug, Default)]
pub struct ReasoningPanel {
    /// When reasoning started (for duration).
    pub started_at: Option<Instant>,
    /// Whether reasoning completed.
    pub completed: bool,
}

impl ReasoningPanel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark reasoning as started.
    pub fn start(&mut self) {
        self.started_at = Some(Instant::now());
        self.completed = false;
    }

    /// Mark reasoning as completed.
    pub fn finish(&mut self) {
        self.completed = true;
    }

    /// Reset state.
    pub fn reset(&mut self) {
        self.started_at = None;
        self.completed = false;
    }

    /// Render the reasoning panel in the given area.
    pub fn render(
        &self,
        area: Rect,
        buf: &mut Buffer,
        text: Option<&str>,
        is_streaming: bool,
        theme: &Theme,
    ) {
        let title = if is_streaming {
            "🧠 Reasoning (streaming…) "
        } else {
            "🧠 Reasoning "
        };

        let block = Block::default()
            .borders(Borders::ALL).border_set(ratatui::symbols::border::ROUNDED)
            .title(title)
            .border_style(Style::default().fg(theme.colors.border));

        let inner = block.inner(area);
        block.render(area, buf);

        let content = match text {
            Some(t) if t.is_empty() && is_streaming => {
                vec![Line::from(Span::styled(
                    "Waiting for reasoning…",
                    theme.muted_italic_style(),
                ))]
            }
            Some(t) => {
                let mut lines = Vec::new();
                let wc = t.split_whitespace().count();

                // Duration
                let duration_str = if let Some(started) = self.started_at {
                    let elapsed = started.elapsed().as_secs_f64();
                    format!("{:.1}s", elapsed)
                } else {
                    "—".into()
                };

                // Header with stats
                lines.push(Line::from(Span::styled(
                    format!("{} words  |  {} duration", wc, duration_str),
                    theme.text_dim_style(),
                )));
                lines.push(Line::from(""));

                // First few lines of text
                let truncated = truncate_to_lines(t, inner.width as usize, inner.height.saturating_sub(3) as usize);
                for line_text in truncated.lines() {
                    lines.push(Line::from(Span::styled(
                        line_text.to_string(),
                        theme.muted_italic_style(),
                    )));
                }

                if t.lines().count() > inner.height.saturating_sub(3) as usize {
                    lines.push(Line::from(Span::styled(
                        "…",
                        theme.muted_style(),
                    )));
                }

                lines
            }
            None => {
                vec![Line::from(Span::styled(
                    "No reasoning data",
                    theme.text_dim_style(),
                ))]
            }
        };

        let widget = Paragraph::new(content);
        widget.render(inner, buf);
    }
}

/// Truncate text to fit within given line count and width.
fn truncate_to_lines(text: &str, max_width: usize, max_lines: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut result = String::new();
    let mut count = 0;

    for line in lines.iter().take(max_lines) {
        let shortened = if line.len() > max_width {
            let end = line
                .char_indices()
                .take(max_width.saturating_sub(1))
                .last()
                .map(|(i, _)| i + 1)
                .unwrap_or(0);
            format!("{}…", &line[..end])
        } else {
            line.to_string()
        };
        result.push_str(&shortened);
        result.push('\n');
        count += 1;
    }

    if count < text.lines().count() {
        result.push_str("…");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_with_none_duration() {
        let panel = ReasoningPanel::new();
        assert!(panel.started_at.is_none());
    }

    #[test]
    fn start_sets_timestamp() {
        let mut panel = ReasoningPanel::new();
        panel.start();
        assert!(panel.started_at.is_some());
        assert!(!panel.completed);
    }

    #[test]
    fn finish_marks_completed() {
        let mut panel = ReasoningPanel::new();
        panel.start();
        panel.finish();
        assert!(panel.completed);
    }

    #[test]
    fn reset_clears_state() {
        let mut panel = ReasoningPanel::new();
        panel.start();
        panel.reset();
        assert!(panel.started_at.is_none());
        assert!(!panel.completed);
    }

    #[test]
    fn truncate_to_lines_fits() {
        let result = truncate_to_lines("short", 80, 5);
        assert_eq!(result.trim(), "short");
    }

    #[test]
    fn truncate_to_lines_truncates() {
        let long = "a\nb\nc\nd\ne\nf\ng\nh";
        let result = truncate_to_lines(long, 80, 3);
        let lines: Vec<&str> = result.lines().collect();
        assert!(lines.len() <= 4, "should have at most 4 lines (3 + …)");
    }
}
