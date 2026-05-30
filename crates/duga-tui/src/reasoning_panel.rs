//! Reasoning panel — shows thinking content in sidebar with duration tracking.
//!
//! Maintains a history of thinking blocks for the current session.
//! On session reload, all blocks are populated from the transcript.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use std::time::Instant;

use crate::theme::Theme;

/// A single reasoning/thinking block.
#[derive(Clone, Debug)]
pub struct ReasoningBlock {
    pub text: String,
    pub is_streaming: bool,
    pub timestamp: Instant,
    /// Duration in seconds, set when streaming completes.
    pub duration_secs: Option<f64>,
    /// When the block started (for live duration).
    pub started_at: Option<Instant>,
}

/// Reasoning panel state.
#[derive(Clone, Debug, Default)]
pub struct ReasoningPanel {
    /// History of thinking blocks for this session.
    pub blocks: Vec<ReasoningBlock>,
}

impl ReasoningPanel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a new streaming thinking block (or append to current).
    pub fn add_block(&mut self, delta: &str) {
        if let Some(last) = self.blocks.last_mut() {
            if last.is_streaming {
                last.text.push_str(delta);
                return;
            }
        }
        // Start a new streaming block
        let now = Instant::now();
        self.blocks.push(ReasoningBlock {
            text: delta.to_string(),
            is_streaming: true,
            timestamp: now,
            duration_secs: None,
            started_at: Some(now),
        });
    }

    /// Mark the current streaming block as complete.
    pub fn finish_current_block(&mut self) {
        if let Some(last) = self.blocks.last_mut() {
            if last.is_streaming {
                last.is_streaming = false;
                last.duration_secs = last.started_at.map(|s| s.elapsed().as_secs_f64());
            }
        }
    }

    /// Populate blocks from a transcript (for session reload).
    pub fn load_from_transcript_items(&mut self, items: &[crate::transcript::TranscriptItem]) {
        self.blocks.clear();
        for item in items {
            if let crate::transcript::TranscriptItem::ThinkingBlock {
                text,
                is_streaming,
                timestamp,
                started_at,
                ..
            } = item
            {
                let duration = if !is_streaming {
                    started_at.map(|s| s.elapsed().as_secs_f64())
                } else {
                    None
                };
                self.blocks.push(ReasoningBlock {
                    text: text.clone(),
                    is_streaming: *is_streaming,
                    timestamp: *timestamp,
                    duration_secs: duration,
                    started_at: *started_at,
                });
            }
        }
    }

    /// Whether a block is currently streaming.
    pub fn is_streaming(&self) -> bool {
        self.blocks.last().map(|b| b.is_streaming).unwrap_or(false)
    }

    /// Reset state.
    pub fn reset(&mut self) {
        self.blocks.clear();
    }

    /// Render the reasoning panel in the given area.
    pub fn render(
        &self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
    ) {
        let streaming = self.is_streaming();
        let title = if streaming {
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

        let content = if self.blocks.is_empty() {
            vec![Line::from(Span::styled(
                "No reasoning data",
                theme.text_dim_style(),
            ))]
        } else {
            let mut lines = Vec::new();

            // Show block count if there's history
            if self.blocks.len() > 1 {
                lines.push(Line::from(Span::styled(
                    format!("{} reasoning blocks in this session", self.blocks.len()),
                    theme.text_dim_style(),
                )));
                lines.push(Line::from(""));
            }

            // Show the most recent block
            if let Some(block) = self.blocks.last() {
                let wc = block.text.split_whitespace().count();

                // Duration (frozen when completed, live when streaming)
                let duration_str = if let Some(frozen) = block.duration_secs {
                    format!("{:.1}s", frozen)
                } else if let Some(started) = block.started_at {
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
                let truncated = truncate_to_lines(&block.text, inner.width as usize, inner.height.saturating_sub(6) as usize);
                for line_text in truncated.lines() {
                    lines.push(Line::from(Span::styled(
                        line_text.to_string(),
                        theme.muted_italic_style(),
                    )));
                }

                if block.text.lines().count() > inner.height.saturating_sub(6) as usize {
                    lines.push(Line::from(Span::styled(
                        "…",
                        theme.muted_style(),
                    )));
                }
            }

            lines
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
    fn new_panel_has_empty_blocks() {
        let panel = ReasoningPanel::new();
        assert!(panel.blocks.is_empty());
        assert!(!panel.is_streaming());
    }

    #[test]
    fn add_block_creates_streaming_block() {
        let mut panel = ReasoningPanel::new();
        panel.add_block("hello");
        assert_eq!(panel.blocks.len(), 1);
        assert!(panel.blocks[0].is_streaming);
        assert!(panel.blocks[0].started_at.is_some());
        assert!(panel.is_streaming());
    }

    #[test]
    fn add_block_appends_to_streaming() {
        let mut panel = ReasoningPanel::new();
        panel.add_block("hello");
        panel.add_block(" world");
        assert_eq!(panel.blocks.len(), 1);
        assert_eq!(panel.blocks[0].text, "hello world");
    }

    #[test]
    fn finish_current_block_stops_streaming() {
        let mut panel = ReasoningPanel::new();
        panel.add_block("reasoning");
        assert!(panel.is_streaming());
        panel.finish_current_block();
        assert!(!panel.is_streaming());
        assert!(!panel.blocks[0].is_streaming);
        assert!(panel.blocks[0].duration_secs.is_some());
    }

    #[test]
    fn reset_clears_all_blocks() {
        let mut panel = ReasoningPanel::new();
        panel.add_block("test");
        panel.finish_current_block();
        panel.add_block("test2");
        assert_eq!(panel.blocks.len(), 2);
        panel.reset();
        assert!(panel.blocks.is_empty());
    }

    #[test]
    fn add_block_after_finish_creates_new_block() {
        let mut panel = ReasoningPanel::new();
        panel.add_block("first");
        panel.finish_current_block();
        panel.add_block("second");
        assert_eq!(panel.blocks.len(), 2);
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
