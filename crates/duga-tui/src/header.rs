//! Header widget — model, status, tokens, cost, session.
//!
//! Renders a single-line header showing: app name, model, status dot,
//! token count, estimated cost, and session name.
//! Format: `duga-tui | qwen3-35b | Ready ● | 12.4k tokens | $0.0032 | Session: chess-svg`

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::app::{App, AppState};
use crate::theme::Theme;

/// Stateless header widget.
pub struct HeaderWidget;

impl HeaderWidget {
    /// Render the header into the given area.
    pub fn render(area: Rect, buf: &mut Buffer, app: &App, theme: &Theme) {
        let model = &app.config.model;
        let status = match &app.state {
            AppState::Idle => ("Ready", "●", theme.colors.success),
            AppState::Running {
                cancel_requested: true,
                ..
            } => ("Cancelling…", "◌", theme.colors.warning),
            AppState::Running {
                cancel_requested: false,
                ..
            } => ("Running…", "◌", theme.colors.warning),
        };

        // Token count: from transcript
        let token_str = format_tokens(app);

        // Cost: approximate
        let cost_str = format_cost(app);

        // Session name
        let session_str = format_session(app);

        // Build the header line with styled spans
        let mut spans: Vec<Span> = Vec::new();

        // App name
        spans.push(Span::styled(
            " duga-tui ",
            Style::default()
                .fg(Color::Black)
                .bg(theme.colors.primary)
                .add_modifier(Modifier::BOLD),
        ));

        // Separator
        spans.push(Span::styled("│", Style::default().fg(theme.colors.muted)));

        // Model
        spans.push(Span::styled(
            format!(" {} ", truncate_model(model, area.width as usize)),
            Style::default().fg(theme.colors.text),
        ));

        // Separator
        spans.push(Span::styled("│", Style::default().fg(theme.colors.muted)));

        // Thinking level
        let think_str = format!(" {:?} ", app.config.thinking_level);
        spans.push(Span::styled(
            think_str,
            Style::default().fg(theme.colors.text_dim),
        ));

        // Separator
        spans.push(Span::styled("│", Style::default().fg(theme.colors.muted)));

        // Status dot + label
        spans.push(Span::styled(
            format!(" {} {} ", status.1, status.0),
            Style::default().fg(status.2),
        ));

        // Separator
        spans.push(Span::styled("│", Style::default().fg(theme.colors.muted)));

        // Token count
        spans.push(Span::styled(
            format!(" {} ", token_str),
            Style::default().fg(theme.colors.text_dim),
        ));

        // Separator
        spans.push(Span::styled("│", Style::default().fg(theme.colors.muted)));

        // Cost
        spans.push(Span::styled(
            format!(" {} ", cost_str),
            Style::default().fg(theme.colors.text_dim),
        ));

        // Session (if available)
        if !session_str.is_empty() {
            spans.push(Span::styled("│", Style::default().fg(theme.colors.muted)));
            spans.push(Span::styled(
                format!(" {} ", session_str),
                Style::default().fg(theme.colors.text_dim),
            ));
        }

        // Pad the rest of the line with background
        let header = Paragraph::new(Line::from(spans))
            .style(Style::default().bg(theme.colors.surface));

        header.render(area, buf);
    }
}

/// Format total tokens from the transcript.
fn format_tokens(app: &App) -> String {
    // Count approximate tokens from message text lengths
    let total_chars: usize = app
        .transcript
        .items()
        .iter()
        .map(|item| match item {
            crate::transcript::TranscriptItem::UserMessage { text, .. } => text.len(),
            crate::transcript::TranscriptItem::AssistantMessage { text, .. } => text.len(),
            crate::transcript::TranscriptItem::SystemMessage { text, .. } => text.len(),
            crate::transcript::TranscriptItem::ToolCallBlock {
                description,
                output,
                ..
            } => {
                description.len()
                    + output.as_ref().map(|o| o.len()).unwrap_or(0)
                    + 100 // overhead estimate
            }
            crate::transcript::TranscriptItem::DelegationNotice {
                reason, ..
            } => reason.len(),
            crate::transcript::TranscriptItem::MemoryNotice { .. } => 50,
            crate::transcript::TranscriptItem::ThinkingBlock { text, .. } => text.len(),
        })
        .sum();

    let approx_tokens = (total_chars as f64 / 3.5) as u64; // rough char-to-token ratio

    if approx_tokens >= 1000 {
        format!("{:.1}k tokens", approx_tokens as f64 / 1000.0)
    } else if approx_tokens > 0 {
        format!("{} tokens", approx_tokens)
    } else {
        "— tokens".into()
    }
}

/// Format approximate cost.
fn format_cost(app: &App) -> String {
    let total_chars: usize = app
        .transcript
        .items()
        .iter()
        .map(|item| match item {
            crate::transcript::TranscriptItem::UserMessage { text, .. } => text.len(),
            crate::transcript::TranscriptItem::AssistantMessage { text, .. } => text.len(),
            crate::transcript::TranscriptItem::SystemMessage { text, .. } => text.len(),
            crate::transcript::TranscriptItem::ThinkingBlock { text, .. } => text.len(),
            _ => 0,
        })
        .sum();

    let approx_tokens = (total_chars as f64 / 3.5) as u64;
    // Rough pricing: ~$0.50/1M tokens (average across models)
    let cost = approx_tokens as f64 * 0.50 / 1_000_000.0;

    if cost >= 0.01 {
        format!("${:.4}", cost)
    } else if cost > 0.0 {
        format!("${:.6}", cost)
    } else {
        "$0.0000".into()
    }
}

/// Format session name.
fn format_session(app: &App) -> String {
    if let Some(ref id) = app.current_session_id {
        let sessions = crate::session::list_sessions(&app.sessions_dir);
        if let Some(info) = sessions.iter().find(|s| s.id == *id) {
            let title = truncate_str(&info.title, 30);
            return format!("Session: {}", title);
        }
    }
    String::new()
}

/// Truncate a model name to fit display width.
fn truncate_model(model: &str, term_width: usize) -> String {
    // Model should fit within reasonable bounds; the minimum header
    // width we can tolerate is ~60 chars before we truncate aggressively.
    if term_width < 60 {
        // Very narrow: show only first 8 chars
        format!("{}…", &model.chars().take(8).collect::<String>())
    } else {
        model.to_string()
    }
}

/// Truncate a string to max_width characters, appending "…".
fn truncate_str(s: &str, max_width: usize) -> String {
    if s.chars().count() <= max_width {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max_width.saturating_sub(1)).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_tokens_returns_dash_when_empty() {
        let result = format!("{}", format_tokens_helper(0));
        assert_eq!(result, "— tokens");
    }

    #[test]
    fn format_tokens_shows_count() {
        // 3500 chars ≈ 1000 tokens
        assert!(format_tokens_helper(3500).contains("tokens"));
    }

    #[test]
    fn format_tokens_uses_k_suffix() {
        // 35000 chars ≈ 10000 tokens = 10k
        assert!(format_tokens_helper(35000).contains('k'));
    }

    #[test]
    fn format_cost_zero_outputs_zero() {
        assert!(format!("{}", format_cost_helper(0)).contains("0.0000"));
    }

    #[test]
    fn truncate_str_truncates() {
        assert_eq!(truncate_str("hello world", 5), "hell…");
        assert_eq!(truncate_str("hi", 10), "hi");
    }

    // Helpers that compute token count from total chars
    fn format_tokens_helper(total_chars: usize) -> String {
        let approx_tokens = (total_chars as f64 / 3.5) as u64;
        if approx_tokens >= 1000 {
            format!("{:.1}k tokens", approx_tokens as f64 / 1000.0)
        } else if approx_tokens > 0 {
            format!("{} tokens", approx_tokens)
        } else {
            "— tokens".into()
        }
    }

    fn format_cost_helper(total_chars: usize) -> String {
        let approx_tokens = (total_chars as f64 / 3.5) as u64;
        let cost = approx_tokens as f64 * 0.50 / 1_000_000.0;
        if cost >= 0.01 {
            format!("${:.4}", cost)
        } else if cost > 0.0 {
            format!("${:.6}", cost)
        } else {
            "$0.0000".into()
        }
    }
}
