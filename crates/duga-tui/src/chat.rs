//! Chat panel — renders transcript items as bordered message cards.
//!
//! Each user/assistant/system/error message is wrapped in a
//! ratatui `Block` with `Borders::ALL` and role-colored title.
//! Assistant messages include nested thinking blocks and markdown.
//! Uses `Transcript::estimate_visible_range()` for virtualization.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget, Wrap};

use crate::theme::Theme;
use crate::transcript::{SystemLevel, Transcript, TranscriptItem};

/// Stateless chat renderer.
pub struct ChatView;

impl ChatView {
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
            let (start, end) = transcript.estimate_visible_range(area.height);

            let mut idx = start;
            while idx < end {
                let item = &items[idx];

                let has_next_assistant = idx + 1 < items.len()
                    && matches!(item, TranscriptItem::ThinkingBlock { .. })
                    && matches!(items[idx + 1], TranscriptItem::AssistantMessage { .. });

                if has_next_assistant {
                    if let (
                        TranscriptItem::ThinkingBlock { text: think_text, is_streaming: think_streaming, is_expanded, scroll_offset, .. },
                        TranscriptItem::AssistantMessage { text: assist_text, is_streaming: assist_streaming, .. },
                    ) = (&items[idx], &items[idx + 1]) {
                        render_assistant_with_thinking(
                            assist_text, *assist_streaming,
                            think_text, *think_streaming, *is_expanded, *scroll_offset,
                            &mut lines, available_width, theme,
                        );
                        idx += 2;
                        continue;
                    }
                }

                if idx > 0
                    && matches!(item, TranscriptItem::AssistantMessage { .. })
                    && matches!(items[idx - 1], TranscriptItem::ThinkingBlock { .. })
                    && idx > start
                {
                    idx += 1;
                    continue;
                }

                render_item(item, &mut lines, available_width, theme);
                idx += 1;
            }
        }

        let total_lines = lines.len();
        let visible = area.height as usize;
        let max_offset = total_lines.saturating_sub(visible);

        let offset = if scroll.manual_scroll {
            max_offset.saturating_sub(scroll.offset)
        } else {
            max_offset
        };

        let clipped: Vec<Line> = lines.into_iter().skip(offset).take(visible).collect();

        let widget = Paragraph::new(Text::from(clipped))
            .block(Block::default().style(theme.surface_style()))
            .wrap(Wrap { trim: false });

        widget.render(area, buf);
    }
}

// ── Welcome ───────────────────────────────────────────────────────────

fn render_welcome(lines: &mut Vec<Line<'static>>, w: usize, theme: &Theme) {
    lines.push(Line::from(Span::styled(
        "Welcome to duga TUI! Type a task or question below and press Enter.",
        theme.text_dim_style(),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "F1: help  |  Ctrl+F: search  |  Ctrl+S: sessions  |  Ctrl+C: cancel  |  q: quit",
        theme.text_dim_style(),
    )));
    let _ = w;
}

// ── Item dispatch ─────────────────────────────────────────────────────

fn render_item(
    item: &TranscriptItem,
    lines: &mut Vec<Line<'static>>,
    w: usize,
    theme: &Theme,
) {
    match item {
        TranscriptItem::SystemMessage { text, level, .. } => {
            let (icon, title, style) = match level {
                SystemLevel::Info => ("ℹ", "Info", theme.system_style()),
                SystemLevel::Warn => ("⚠", "Warning", theme.warning_style()),
                SystemLevel::Error => ("✗", "Error", theme.error_style()),
            };
            render_card(&format!("{icon} {title}"), style, text, lines, w, theme, false, false);
        }
        TranscriptItem::UserMessage { text, .. } => {
            render_card("You", theme.user_style(), text, lines, w, theme, false, false);
        }
        TranscriptItem::AssistantMessage { text, is_streaming, .. } => {
            let title = if *is_streaming { "Assistant ⟳" } else { "Assistant" };
            render_card(title, theme.assistant_style(), text, lines, w, theme, *is_streaming, true);
        }
        TranscriptItem::ThinkingBlock { text, is_streaming, is_expanded, scroll_offset, .. } => {
            render_thinking_card(text, *is_streaming, *is_expanded, *scroll_offset, lines, w, theme);
        }
        TranscriptItem::ToolCallBlock { tool_name, description, is_running, is_success, .. } => {
            let icon = if *is_running { "⟳" } else if *is_success == Some(true) { "✓" } else if *is_success == Some(false) { "✗" } else { "?" };
            let style = if *is_running { theme.tool_running_style() } else if *is_success == Some(false) { theme.tool_error_style() } else { theme.tool_success_style() };
            render_card(&format!("{icon} Tool: {tool_name}"), style, description, lines, w, theme, *is_running, false);
        }
        TranscriptItem::DelegationNotice { from, to, reason, .. } => {
            render_card(&format!("🔄 {from} → {to}"), theme.delegation_style(), reason, lines, w, theme, false, false);
        }
        TranscriptItem::MemoryNotice { before_tokens, after_tokens, .. } => {
            render_card("💾 Memory", theme.memory_style(), &format!("{before_tokens} → {after_tokens} tokens"), lines, w, theme, false, false);
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("─".repeat(w.min(80)), theme.border_style())));
}

// ── Card renderer ─────────────────────────────────────────────────────

fn render_card(
    title: &str,
    title_style: Style,
    content: &str,
    lines: &mut Vec<Line<'static>>,
    w: usize,
    theme: &Theme,
    is_streaming: bool,
    use_markdown: bool,
) {
    let inner_w = w.saturating_sub(4);

    let content_lines = if is_streaming && content.is_empty() {
        vec![Line::from(Span::styled("…", theme.muted_italic_style()))]
    } else if use_markdown && !content.is_empty() {
        let md = crate::markdown::render_markdown(content, inner_w);
        md.lines
    } else {
        crate::text::wrap_text(content, inner_w)
            .into_iter()
            .map(|s| Line::from(Span::styled(s, theme.text_style())))
            .collect()
    };

    let n_lines = content_lines.len();
    let n_lines = content_lines.len();
    let widget = Paragraph::new(Text::from(content_lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .title_style(title_style)
                .border_style(theme.border_style()),
        );

    // Render into a temporary buffer to extract lines
    let render_area = Rect::new(0, 0, w as u16, (n_lines + 2) as u16);
    let mut tmp_buf = Buffer::empty(render_area);
    widget.render(render_area, &mut tmp_buf);

    // Extract rendered lines from the buffer
    for row in 0..render_area.height {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let cells = &tmp_buf.content[(row as usize * render_area.width as usize)..];
        let mut i = 0usize;
        while i < render_area.width as usize && i < cells.len() {
            let cell = &cells[i];
            let ch = cell.symbol();
            let style = cell.style();
            // Merge adjacent same-style cells
            let mut run = String::new();
            let start = i;
            while i < render_area.width as usize && i < cells.len() && cells[i].style() == style {
                run.push_str(cells[i].symbol());
                i += 1;
            }
            if !run.is_empty() {
                spans.push(Span::styled(run, style));
            }
        }
        if !spans.is_empty() {
            lines.push(Line::from(spans));
        }
    }
}

// ── Thinking nested in assistant ──────────────────────────────────────

fn render_assistant_with_thinking(
    assist_text: &str, assist_streaming: bool,
    think_text: &str, think_streaming: bool,
    think_expanded: bool, think_scroll: usize,
    lines: &mut Vec<Line<'static>>, w: usize, theme: &Theme,
) {
    let inner_w = w.saturating_sub(6);
    let mut content: Vec<Line<'static>> = Vec::new();

    let prefix = if think_streaming { "⟳ " } else { "🧠" };
    let wc = think_text.split_whitespace().count();
    let think_lines_raw = crate::text::wrap_text(think_text, inner_w);
    let total = think_lines_raw.len();

    if think_expanded {
        content.push(Line::from(Span::styled(
            format!("{prefix} Reasoning ({} words):", wc),
            theme.muted_italic_style(),
        )));
        let start = if think_streaming { total.saturating_sub(8) } else { think_scroll.min(total.saturating_sub(1)) };
        for line in think_lines_raw.iter().skip(start).take(8) {
            content.push(Line::from(Span::styled(format!("  {line}"), theme.muted_italic_style())));
        }
        if total > 8 {
            let end = (start + 8).min(total);
            content.push(Line::from(Span::styled(
                format!("  ── {}-{end} of {total} ──", start + 1),
                theme.muted_style(),
            )));
        }
        content.push(Line::from(""));
        content.push(Line::from(Span::styled("─".repeat(inner_w.min(60)), theme.border_style())));
    } else if !think_streaming {
        content.push(Line::from(Span::styled(
            format!("{prefix} Reasoning: ({} words — Tab to expand)", wc),
            theme.text_dim_style(),
        )));
        content.push(Line::from(Span::styled("─".repeat(inner_w.min(60)), theme.border_style())));
    }

    // Assistant text
    if assist_streaming && assist_text.is_empty() {
        content.push(Line::from(Span::styled("…", theme.muted_italic_style())));
    } else if !assist_text.is_empty() {
        let md = crate::markdown::render_markdown(assist_text, inner_w);
        content.extend(md.lines);
    }

    let n_lines2 = content.len();
    let widget = Paragraph::new(Text::from(content))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(if assist_streaming { "Assistant ⟳" } else { "Assistant" })
                .title_style(theme.assistant_style())
                .border_style(theme.border_style()),
        );

    let h = (n_lines2 + 2) as u16;
    let render_area = Rect::new(0, 0, w as u16, h);
    let mut tmp_buf = Buffer::empty(render_area);
    widget.render(render_area, &mut tmp_buf);

    for row in 0..h {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let cells = &tmp_buf.content[(row as usize * w as usize)..];
        let mut i = 0;
        while i < w as usize && i < cells.len() {
            let style = cells[i].style();
            let mut run = String::new();
            while i < w as usize && i < cells.len() && cells[i].style() == style {
                run.push_str(cells[i].symbol());
                i += 1;
            }
            if !run.is_empty() {
                spans.push(Span::styled(run, style));
            }
        }
        if !spans.is_empty() {
            lines.push(Line::from(spans));
        }
    }
}

// ── Thinking card (standalone) ────────────────────────────────────────

fn render_thinking_card(
    text: &str, is_streaming: bool, is_expanded: bool, scroll_offset: usize,
    lines: &mut Vec<Line<'static>>, w: usize, theme: &Theme,
) {
    let prefix = if is_streaming { "⟳ " } else { "🧠" };
    let wc = text.split_whitespace().count();
    let title = if is_streaming {
        format!("{prefix} Reasoning (streaming…)")
    } else {
        format!("{prefix} Reasoning ({} words)", wc)
    };

    let inner_w = w.saturating_sub(4);
    let think_lines = crate::text::wrap_text(text, inner_w);
    let total = think_lines.len();

    let mut content: Vec<Line<'static>> = Vec::new();

    if is_expanded {
        let start = if is_streaming { total.saturating_sub(8) } else { scroll_offset.min(total.saturating_sub(1)) };
        for line in think_lines.iter().skip(start).take(8) {
            content.push(Line::from(Span::styled(line.clone(), theme.muted_italic_style())));
        }
        if total > 8 {
            let end = (start + 8).min(total);
            content.push(Line::from(Span::styled(
                format!("── {}-{end} of {total} (Ctrl+O) ──", start + 1),
                theme.muted_style(),
            )));
        }
    } else if !is_streaming {
        content.push(Line::from(Span::styled(
            format!("({} words — Tab to expand)", wc),
            theme.text_dim_style(),
        )));
    }

    let n_lines3 = content.len();
    let widget = Paragraph::new(Text::from(content))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .title_style(theme.muted_italic_style())
                .border_style(theme.border_style()),
        );

    let h = (n_lines3 + 2) as u16;
    let render_area = Rect::new(0, 0, w as u16, h);
    let mut tmp_buf = Buffer::empty(render_area);
    widget.render(render_area, &mut tmp_buf);

    for row in 0..h {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let cells = &tmp_buf.content[(row as usize * w as usize)..];
        let mut i = 0;
        while i < w as usize && i < cells.len() {
            let style = cells[i].style();
            let mut run = String::new();
            while i < w as usize && i < cells.len() && cells[i].style() == style {
                run.push_str(cells[i].symbol());
                i += 1;
            }
            if !run.is_empty() {
                spans.push(Span::styled(run, style));
            }
        }
        if !spans.is_empty() {
            lines.push(Line::from(spans));
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn make_theme() -> Theme { Theme::dark() }

    #[test]
    fn user_card_has_correct_border() {
        let theme = make_theme();
        let mut lines: Vec<Line<'static>> = Vec::new();
        render_item(
            &TranscriptItem::UserMessage { text: "hello".into(), timestamp: Instant::now() },
            &mut lines, 80, &theme,
        );
        let all: String = lines.iter().flat_map(|l| l.spans.iter().map(|s| s.content.as_ref())).collect();
        assert!(all.contains("You"));
        assert!(all.contains("hello"));
    }

    #[test]
    fn assistant_card_has_markdown() {
        let theme = make_theme();
        let mut lines: Vec<Line<'static>> = Vec::new();
        render_item(
            &TranscriptItem::AssistantMessage { text: "**bold**".into(), timestamp: Instant::now(), is_streaming: false },
            &mut lines, 80, &theme,
        );
        let all: String = lines.iter().flat_map(|l| l.spans.iter().map(|s| s.content.as_ref())).collect();
        assert!(all.contains("Assistant"));
        assert!(all.contains("bold"));
    }

    #[test]
    fn error_card_shows() {
        let theme = make_theme();
        let mut lines: Vec<Line<'static>> = Vec::new();
        render_item(
            &TranscriptItem::SystemMessage { text: "fail".into(), level: SystemLevel::Error, timestamp: Instant::now() },
            &mut lines, 80, &theme,
        );
        let all: String = lines.iter().flat_map(|l| l.spans.iter().map(|s| s.content.as_ref())).collect();
        assert!(all.contains("Error"));
    }

    #[test]
    fn streaming_shows_spinner() {
        let theme = make_theme();
        let mut lines: Vec<Line<'static>> = Vec::new();
        render_item(
            &TranscriptItem::AssistantMessage { text: String::new(), timestamp: Instant::now(), is_streaming: true },
            &mut lines, 80, &theme,
        );
        let all: String = lines.iter().flat_map(|l| l.spans.iter().map(|s| s.content.as_ref())).collect();
        assert!(all.contains("⟳") || all.contains("…"));
    }

    #[test]
    fn thinking_nested_in_assistant() {
        let theme = make_theme();
        let mut lines: Vec<Line<'static>> = Vec::new();
        render_assistant_with_thinking(
            "answer", false, "thinking", false, true, 0,
            &mut lines, 80, &theme,
        );
        let all: String = lines.iter().flat_map(|l| l.spans.iter().map(|s| s.content.as_ref())).collect();
        assert!(all.contains("Assistant"));
        assert!(all.contains("Reasoning"));
        assert!(all.contains("thinking"));
    }

    #[test]
    fn separator_between_cards() {
        let theme = make_theme();
        let mut lines: Vec<Line<'static>> = Vec::new();
        render_item(&TranscriptItem::UserMessage { text: "a".into(), timestamp: Instant::now() }, &mut lines, 80, &theme);
        render_item(&TranscriptItem::AssistantMessage { text: "b".into(), timestamp: Instant::now(), is_streaming: false }, &mut lines, 80, &theme);
        let all: String = lines.iter().flat_map(|l| l.spans.iter().map(|s| s.content.as_ref())).collect();
        // Should have separator ── between cards
        assert!(all.matches('─').count() > 2);
    }

    #[test]
    fn tool_card_shows_name() {
        let theme = make_theme();
        let mut lines: Vec<Line<'static>> = Vec::new();
        render_item(
            &TranscriptItem::ToolCallBlock {
                tool_call_id: "t1".into(), tool_name: "shell".into(), description: "ls".into(),
                raw_args: None, is_running: false, is_success: Some(true), is_expanded: false,
                timestamp: Instant::now(), output: None,
            },
            &mut lines, 80, &theme,
        );
        let all: String = lines.iter().flat_map(|l| l.spans.iter().map(|s| s.content.as_ref())).collect();
        assert!(all.contains("shell"));
        assert!(all.contains("Tool"));
    }

    #[test]
    fn welcome_message_shows() {
        let theme = make_theme();
        let mut lines: Vec<Line<'static>> = Vec::new();
        render_welcome(&mut lines, 80, &theme);
        let all: String = lines.iter().flat_map(|l| l.spans.iter().map(|s| s.content.as_ref())).collect();
        assert!(all.contains("Welcome"));
    }

    #[test]
    fn virtualization_visible_range() {
        let mut t = Transcript::new();
        for _ in 0..50 {
            t.push(TranscriptItem::SystemMessage { text: "msg".into(), level: SystemLevel::Info, timestamp: Instant::now() });
        }
        let (start, end) = t.estimate_visible_range(20);
        assert!(end <= 50);
        assert!(start <= end);
        assert!(end - start > 0);
    }
}
