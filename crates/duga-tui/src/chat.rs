//! Chat panel — renders transcript items as bordered message cards.
//!
//! Each user/assistant/system/error message is wrapped in a
//! `ratatui::widgets::Block` with a colored border and role title.
//! Assistant messages include nested thinking blocks and markdown rendering.
//! Uses `Transcript::estimate_visible_range()` for virtualization.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph, Widget, Wrap};

use crate::theme::Theme;
use crate::transcript::{SystemLevel, Transcript, TranscriptItem};

/// Stateless chat renderer.
pub struct ChatView;

impl ChatView {
    /// Render the chat panel into the given area.
    /// Uses item-based virtualization for performance.
    pub fn render(
        area: Rect,
        buf: &mut Buffer,
        transcript: &Transcript,
        theme: &Theme,
    ) {
        let items = transcript.items();
        let scroll = transcript.scroll();

        let mut lines: Vec<Line<'static>> = Vec::new();
        let available_width = area.width as usize;

        if items.is_empty() {
            render_welcome(&mut lines, available_width, theme);
        } else {
            // Virtualization: only render items likely to be visible
            let (start, end) = transcript.estimate_visible_range(area.height);
            let visible_items = &items[start..end];

            let mut idx = start;
            for item in visible_items {
                // Check if the next item is an AssistantMessage following a ThinkingBlock
                let has_next_assistant = idx + 1 < items.len()
                    && matches!(items[idx], TranscriptItem::ThinkingBlock { .. })
                    && matches!(items[idx + 1], TranscriptItem::AssistantMessage { .. });

                if has_next_assistant {
                    // Render thinking block + assistant as a combined card
                    if let (
                        TranscriptItem::ThinkingBlock { text: think_text, is_streaming: think_streaming, is_expanded, scroll_offset, .. },
                        TranscriptItem::AssistantMessage { text: assist_text, is_streaming: assist_streaming, .. },
                    ) = (&items[idx], &items[idx + 1]) {
                        render_assistant_with_thinking(
                            assist_text,
                            *assist_streaming,
                            think_text,
                            *think_streaming,
                            *is_expanded,
                            *scroll_offset,
                            &mut lines,
                            available_width,
                            theme,
                        );
                        idx += 2;
                        continue;
                    }
                }

                // If current item was already rendered (as part of a pair), skip
                if idx > start && idx < items.len() {
                    if matches!(items[idx - 1], TranscriptItem::ThinkingBlock { .. })
                        && matches!(items[idx], TranscriptItem::AssistantMessage { .. })
                        && idx > start // ensure the thinking block was rendered
                    {
                        // Check if the thinking block at idx-1 was included in the visible range
                        if start <= idx - 1 {
                            idx += 1;
                            continue;
                        }
                    }
                }

                render_item(item, &mut lines, available_width, theme);
                idx += 1;
            }
        }

        // Apply scroll offset
        let total_lines = lines.len();
        let visible_lines_count = area.height as usize;
        let max_offset = total_lines.saturating_sub(visible_lines_count);

        let offset = if scroll.manual_scroll {
            max_offset.saturating_sub(scroll.offset)
        } else {
            max_offset
        };

        let visible_lines: Vec<Line> = lines
            .into_iter()
            .skip(offset)
            .take(visible_lines_count)
            .collect();

        let widget = Paragraph::new(Text::from(visible_lines))
            .block(Block::default().style(theme.surface_style()))
            .wrap(Wrap { trim: false });

        widget.render(area, buf);
    }
}

/// Render the welcome/default screen.
fn render_welcome(lines: &mut Vec<Line<'static>>, _available_width: usize, theme: &Theme) {
    lines.push(Line::from(
        Span::styled(
            "Welcome to duga TUI! Type a task or question below and press Enter.",
            theme.text_dim_style(),
        ),
    ));
    lines.push(Line::from(""));
    lines.push(Line::from(
        Span::styled(
            "F1: help  |  Ctrl+F: search  |  Ctrl+S: sessions  |  Ctrl+C: cancel  |  q: quit",
            theme.text_dim_style(),
        ),
    ));
}

/// Render a single transcript item as a bordered message card.
fn render_item(
    item: &TranscriptItem,
    lines: &mut Vec<Line<'static>>,
    available_width: usize,
    theme: &Theme,
) {
    match item {
        TranscriptItem::SystemMessage { text, level, .. } => {
            render_system_card(text, level, lines, available_width, theme);
        }
        TranscriptItem::UserMessage { text, .. } => {
            render_card(
                "You",
                theme.user_style(),
                text,
                lines,
                available_width,
                theme,
                false,
                false, // not markdown
            );
        }
        TranscriptItem::AssistantMessage {
            text,
            is_streaming,
            ..
        } => {
            let title = if *is_streaming {
                "Assistant ⟳"
            } else {
                "Assistant"
            };
            render_card(
                title,
                theme.assistant_style(),
                text,
                lines,
                available_width,
                theme,
                *is_streaming,
                true, // use markdown rendering
            );
        }
        TranscriptItem::ThinkingBlock {
            text,
            is_streaming,
            is_expanded,
            scroll_offset,
            ..
        } => {
            render_thinking_card(
                text,
                *is_streaming,
                *is_expanded,
                *scroll_offset,
                lines,
                available_width,
                theme,
            );
        }
        TranscriptItem::ToolCallBlock {
            tool_name,
            description,
            is_running,
            is_success,
            ..
        } => {
            render_tool_card(
                tool_name,
                description,
                *is_running,
                *is_success,
                lines,
                available_width,
                theme,
            );
        }
        TranscriptItem::DelegationNotice {
            from,
            to,
            reason,
            ..
        } => {
            render_notice_card(
                &format!("🔄 {from} → {to}"),
                reason,
                theme.delegation_style(),
                lines,
                available_width,
                theme,
            );
        }
        TranscriptItem::MemoryNotice {
            before_tokens,
            after_tokens,
            ..
        } => {
            render_notice_card(
                "💾 Memory",
                &format!("{before_tokens} → {after_tokens} tokens"),
                theme.memory_style(),
                lines,
                available_width,
                theme,
            );
        }
    }

    // Add blank row between cards
    lines.push(Line::from(""));
    // Thin separator
    let sep_width = available_width.min(80);
    lines.push(Line::from(Span::styled(
        "─".repeat(sep_width),
        theme.border_style(),
    )));
}

/// Render a bordered message card.
fn render_card(
    title: &str,
    title_style: Style,
    content: &str,
    lines: &mut Vec<Line<'static>>,
    available_width: usize,
    theme: &Theme,
    is_streaming: bool,
    use_markdown: bool,
) {
    // Top border with title
    let border_line = format_card_top_border(title, title_style, available_width, theme);
    lines.push(Line::from(border_line));

    // Content: wrap at inner width (available_width - 4 for border + padding)
    let inner_width = available_width.saturating_sub(4);

    if is_streaming && content.is_empty() {
        lines.push(Line::from(Span::styled(
            " │  …".to_string(),
            theme.muted_italic_style(),
        )));
    } else if use_markdown && !content.is_empty() {
        // Render markdown with indentation
        let md = crate::markdown::render_markdown(content, inner_width);
        for md_line in md.lines {
            lines.push(Line::from(vec![
                Span::styled(" │ ", theme.border_style()),
                Span::raw(" "),
            ]));
            // Add the markdown spans
            let combined: Vec<Span> = std::iter::once(Span::styled(" │ ", theme.border_style()))
                .chain(md_line.spans.iter().cloned())
                .collect();
            lines.push(Line::from(combined));
        }
    } else {
        // Plain text rendering
        let wrapped = crate::text::wrap_text(content, inner_width);
        for line_text in wrapped {
            lines.push(Line::from(vec![
                Span::styled(" │ ", theme.border_style()),
                Span::styled(line_text, theme.text_style()),
            ]));
        }
    }

    // Bottom border
    let bottom = format_card_bottom(available_width, theme);
    lines.push(Line::from(bottom));
}

/// Render an assistant message with a nested thinking block.
fn render_assistant_with_thinking(
    assist_text: &str,
    assist_streaming: bool,
    think_text: &str,
    think_streaming: bool,
    think_expanded: bool,
    think_scroll: usize,
    lines: &mut Vec<Line<'static>>,
    available_width: usize,
    theme: &Theme,
) {
    let title = if assist_streaming {
        "Assistant ⟳"
    } else {
        "Assistant"
    };

    // Top border of assistant card
    let border_line = format_card_top_border(title, theme.assistant_style(), available_width, theme);
    lines.push(Line::from(border_line));

    let inner_width = available_width.saturating_sub(4);
    let think_lines = crate::text::wrap_text(think_text, inner_width.saturating_sub(2));
    let total_think = think_lines.len();

    // Render thinking block inside assistant card
    let prefix = if think_streaming { "⟳ " } else { "🧠" };
    let wc = think_text.split_whitespace().count();

    if think_expanded {
        let duration = if let Some(dur) = compute_thinking_duration(think_text) {
            format!(" ({:.1}s)", dur)
        } else {
            String::new()
        };
        lines.push(Line::from(vec![
            Span::styled(" │ ", theme.border_style()),
            Span::styled(
                format!("  {prefix} Reasoning ({} words{duration}):", wc),
                theme.muted_italic_style(),
            ),
        ]));

        let start = if think_streaming {
            total_think.saturating_sub(8)
        } else {
            think_scroll.min(total_think.saturating_sub(1))
        };

        let display: Vec<&str> = think_lines
            .iter()
            .skip(start)
            .take(8)
            .map(|s| s.as_str())
            .collect();

        for w in &display {
            lines.push(Line::from(vec![
                Span::styled(" │ ", theme.border_style()),
                Span::styled(format!("    {w}"), theme.muted_italic_style()),
            ]));
        }

        if total_think > 8 {
            let end = (start + display.len()).min(total_think);
            lines.push(Line::from(vec![
                Span::styled(" │ ", theme.border_style()),
                Span::styled(
                    format!("    ── {}-{end} of {total_think} (Ctrl+O) ──", start + 1),
                    theme.muted_style(),
                ),
            ]));
        }
    } else if !think_streaming {
        lines.push(Line::from(vec![
            Span::styled(" │ ", theme.border_style()),
            Span::styled(
                format!("  {prefix} Reasoning: ({} words — Tab to expand)", wc),
                theme.text_dim_style(),
            ),
        ]));
    }

    // Separator between thinking and assistant response
    if think_expanded || (!think_streaming && think_text.len() > 0) {
        let sep = "─".repeat(inner_width.min(60));
        lines.push(Line::from(vec![
            Span::styled(" │ ", theme.border_style()),
            Span::styled(format!("  {sep}"), theme.border_style()),
        ]));
    }

    // Render assistant response with markdown
    if assist_streaming && assist_text.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(" │ ", theme.border_style()),
            Span::styled("  …", theme.muted_italic_style()),
        ]));
    } else if !assist_text.is_empty() {
        let md = crate::markdown::render_markdown(assist_text, inner_width.saturating_sub(1));
        for md_line in md.lines {
            let mut spans = vec![Span::styled(" │ ", theme.border_style())];
            spans.extend(md_line.spans.iter().cloned());
            lines.push(Line::from(spans));
        }
    }

    // Bottom border
    let bottom = format_card_bottom(available_width, theme);
    lines.push(Line::from(bottom));
}

/// Compute thinking duration in seconds from text statistics.
/// Returns None if can't compute.
fn compute_thinking_duration(_text: &str) -> Option<f64> {
    // Duration tracking needs timestamps from the transcript.
    // For now, return None — the sidebar reasoning panel will show
    // accurate duration from the ThinkingBlock's started_at.
    None
}

/// Format the top border of a card with title.
fn format_card_top_border(
    title: &str,
    title_style: Style,
    available_width: usize,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let mut spans: Vec<Span> = Vec::new();

    spans.push(Span::styled("┌─ ", theme.border_style()));
    spans.push(Span::styled(title.to_string(), title_style));
    spans.push(Span::styled(" ", theme.border_style()));

    let used = 3 + title.len() + 1;
    let remaining = available_width.saturating_sub(used);
    if remaining > 0 {
        spans.push(Span::styled(
            "─".repeat(remaining),
            theme.border_style(),
        ));
    }

    spans
}

/// Format the bottom border of a card.
fn format_card_bottom(available_width: usize, theme: &Theme) -> Vec<Span<'static>> {
    vec![Span::styled(
        format!("└{}", "─".repeat(available_width.saturating_sub(1))),
        theme.border_style(),
    )]
}

/// Render a system message card.
fn render_system_card(
    text: &str,
    level: &SystemLevel,
    lines: &mut Vec<Line<'static>>,
    available_width: usize,
    theme: &Theme,
) {
    let (icon, title, style) = match level {
        SystemLevel::Info => ("ℹ", "Info", theme.system_style()),
        SystemLevel::Warn => ("⚠", "Warning", theme.warning_style()),
        SystemLevel::Error => ("✗", "Error", theme.error_style()),
    };

    let title_str = format!("{icon} {title}");
    render_card(&title_str, style, text, lines, available_width, theme, false, false);
}

/// Render a thinking block card (standalone, not nested in assistant).
fn render_thinking_card(
    text: &str,
    is_streaming: bool,
    is_expanded: bool,
    scroll_offset: usize,
    lines: &mut Vec<Line<'static>>,
    available_width: usize,
    theme: &Theme,
) {
    let prefix = if is_streaming { "⟳ " } else { "🧠" };
    let wc = text.split_whitespace().count();
    let title = if is_streaming {
        format!("{prefix} Reasoning (streaming…)")
    } else {
        format!("{prefix} Reasoning ({} words)", wc)
    };

    let inner_width = available_width.saturating_sub(4);
    let think_lines = crate::text::wrap_text(text, inner_width);
    let total = think_lines.len();

    if is_expanded {
        let border_line = format_card_top_border(
            &title,
            theme.muted_italic_style(),
            available_width,
            theme,
        );
        lines.push(Line::from(border_line));

        let start = if is_streaming {
            total.saturating_sub(8)
        } else {
            scroll_offset.min(total.saturating_sub(1))
        };

        let display: Vec<&str> = think_lines
            .iter()
            .skip(start)
            .take(8)
            .map(|s| s.as_str())
            .collect();

        for w in &display {
            lines.push(Line::from(vec![
                Span::styled(" │  ", theme.border_style()),
                Span::styled((*w).to_string(), theme.muted_italic_style()),
            ]));
        }

        if total > 8 {
            let end = (start + display.len()).min(total);
            let page_start = start + 1;
            lines.push(Line::from(vec![
                Span::styled(" │  ", theme.border_style()),
                Span::styled(
                    format!("── {page_start}-{end} of {total} (Ctrl+O) ──"),
                    theme.muted_style(),
                ),
            ]));
        }

        let bottom = format_card_bottom(available_width, theme);
        lines.push(Line::from(bottom));
    } else if !is_streaming {
        let notice = format!(
            "{prefix} Reasoning: ({} words — Tab to expand)",
            text.split_whitespace().count()
        );
        lines.push(Line::from(Span::styled(notice, theme.text_dim_style())));
    }
}

/// Render a tool call block card.
fn render_tool_card(
    tool_name: &str,
    description: &str,
    is_running: bool,
    is_success: Option<bool>,
    lines: &mut Vec<Line<'static>>,
    available_width: usize,
    theme: &Theme,
) {
    let icon = if is_running {
        "⟳"
    } else {
        match is_success {
            Some(true) => "✓",
            Some(false) => "✗",
            None => "?",
        }
    };

    let tool_style = if is_running {
        theme.tool_running_style()
    } else {
        match is_success {
            Some(true) => theme.tool_success_style(),
            Some(false) => theme.tool_error_style(),
            None => theme.muted_style(),
        }
    };

    let title = format!("{icon} Tool: {tool_name}");

    render_card(
        &title,
        tool_style,
        description,
        lines,
        available_width,
        theme,
        is_running,
        false,
    );
}

/// Render a notice card (delegation, memory).
fn render_notice_card(
    title: &str,
    content: &str,
    title_style: Style,
    lines: &mut Vec<Line<'static>>,
    available_width: usize,
    theme: &Theme,
) {
    render_card(title, title_style, content, lines, available_width, theme, false, false);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn make_theme() -> Theme {
        Theme::dark()
    }

    fn make_transcript(items: Vec<TranscriptItem>) -> Transcript {
        let mut t = Transcript::new();
        for item in items {
            t.push(item);
        }
        t
    }

    #[test]
    fn user_card_has_correct_border() {
        let theme = make_theme();
        let mut lines: Vec<Line<'static>> = Vec::new();
        render_item(
            &TranscriptItem::UserMessage {
                text: "hello".into(),
                timestamp: Instant::now(),
            },
            &mut lines,
            80,
            &theme,
        );
        let first = &lines[0];
        let text: String = first.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("┌─"));
        assert!(text.contains("You"));
    }

    #[test]
    fn assistant_card_has_correct_border() {
        let theme = make_theme();
        let mut lines: Vec<Line<'static>> = Vec::new();
        render_item(
            &TranscriptItem::AssistantMessage {
                text: "hi".into(),
                timestamp: Instant::now(),
                is_streaming: false,
            },
            &mut lines,
            80,
            &theme,
        );
        let first = &lines[0];
        let text: String = first.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("Assistant"));
    }

    #[test]
    fn error_card_has_red_content() {
        let theme = make_theme();
        let mut lines: Vec<Line<'static>> = Vec::new();
        render_item(
            &TranscriptItem::SystemMessage {
                text: "fail".into(),
                level: SystemLevel::Error,
                timestamp: Instant::now(),
            },
            &mut lines,
            80,
            &theme,
        );
        let first = &lines[0];
        let text: String = first.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("Error"));
    }

    #[test]
    fn streaming_card_shows_spinner() {
        let theme = make_theme();
        let mut lines: Vec<Line<'static>> = Vec::new();
        render_item(
            &TranscriptItem::AssistantMessage {
                text: String::new(),
                timestamp: Instant::now(),
                is_streaming: true,
            },
            &mut lines,
            80,
            &theme,
        );
        let title = &lines[0];
        let text: String = title.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("⟳"));
    }

    #[test]
    fn item_separator_between_cards() {
        let theme = make_theme();
        let mut lines: Vec<Line<'static>> = Vec::new();
        render_item(
            &TranscriptItem::UserMessage {
                text: "a".into(),
                timestamp: Instant::now(),
            },
            &mut lines,
            80,
            &theme,
        );
        render_item(
            &TranscriptItem::AssistantMessage {
                text: "b".into(),
                timestamp: Instant::now(),
                is_streaming: false,
            },
            &mut lines,
            80,
            &theme,
        );
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(all_text.contains("──"));
    }

    #[test]
    fn tool_card_shows_name() {
        let theme = make_theme();
        let mut lines: Vec<Line<'static>> = Vec::new();
        render_item(
            &TranscriptItem::ToolCallBlock {
                tool_call_id: "t1".into(),
                tool_name: "shell".into(),
                description: "ls -la".into(),
                raw_args: None,
                is_running: false,
                is_success: Some(true),
                is_expanded: false,
                timestamp: Instant::now(),
                output: None,
            },
            &mut lines,
            80,
            &theme,
        );
        let first = &lines[0];
        let text: String = first.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("shell"));
        assert!(text.contains("Tool"));
    }

    #[test]
    fn empty_transcript_shows_welcome() {
        let theme = make_theme();
        let mut lines: Vec<Line<'static>> = Vec::new();
        render_welcome(&mut lines, 80, &theme);
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(text.contains("Welcome"));
    }

    #[test]
    fn thinking_nested_in_assistant_combined() {
        let theme = make_theme();
        let mut lines: Vec<Line<'static>> = Vec::new();
        render_assistant_with_thinking(
            "Here is the answer",
            false,
            "Let me think...",
            false,
            true,
            0,
            &mut lines,
            80,
            &theme,
        );
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(all_text.contains("Assistant"));
        assert!(all_text.contains("Reasoning"));
        assert!(all_text.contains("Let me think"));
        assert!(all_text.contains("Here is the answer"));
    }

    #[test]
    fn virtualization_visible_range() {
        let mut t = Transcript::new();
        for _ in 0..50 {
            t.push(TranscriptItem::SystemMessage {
                text: "msg".into(),
                level: SystemLevel::Info,
                timestamp: Instant::now(),
            });
        }
        let (start, end) = t.estimate_visible_range(20);
        // Should return a subset of items
        assert!(end <= 50);
        assert!(start <= end);
        assert!(end - start > 0, "visible range should contain items");
    }
}
