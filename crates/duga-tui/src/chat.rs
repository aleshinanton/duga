//! Chat panel — renders transcript items as ratatui Block-bordered cards.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget, Wrap};

use crate::theme::Theme;
use crate::transcript::{SystemLevel, Transcript, TranscriptItem};

pub struct ChatView;

impl ChatView {
    pub fn render(area: Rect, buf: &mut Buffer, transcript: &Transcript, theme: &Theme) {
        let items = transcript.items();
        let scroll = transcript.scroll();

        if items.is_empty() {
            let w = Paragraph::new(Text::from(vec![
                Line::from(Span::styled("Welcome to duga TUI! Type a task or question below and press Enter.", theme.text_dim_style())),
                Line::from(""),
                Line::from(Span::styled("F1: help  |  Ctrl+F: search  |  Ctrl+S: sessions  |  Ctrl+C: cancel  |  Ctrl+Q: quit", theme.text_dim_style())),
            ]))
            .block(Block::default().style(theme.surface_style()))
            .wrap(Wrap { trim: false });
            w.render(area, buf);
            return;
        }

        let (start, end) = transcript.estimate_visible_range(area.height);
        let avail_w = area.width.saturating_sub(2) as usize;

        // Build content lines
        let mut content: Vec<Line<'static>> = Vec::new();
        let mut idx = start;
        while idx < end {
            let item = &items[idx];
            let next_is_assist = idx + 1 < items.len()
                && matches!(item, TranscriptItem::ThinkingBlock { .. })
                && matches!(items[idx + 1], TranscriptItem::AssistantMessage { .. });

            if next_is_assist {
                if let (
                    TranscriptItem::ThinkingBlock { text: t, is_streaming: ts, is_expanded: te, scroll_offset: so, .. },
                    TranscriptItem::AssistantMessage { text: a, is_streaming: as_, .. },
                ) = (&items[idx], &items[idx + 1]) {
                    push_assistant_with_thinking(&mut content, a, *as_, t, *ts, *te, *so, avail_w, theme);
                    idx += 2;
                    continue;
                }
            }

            if idx > 0 && matches!(item, TranscriptItem::AssistantMessage { .. })
                && matches!(items[idx - 1], TranscriptItem::ThinkingBlock { .. }) {
                idx += 1;
                continue;
            }

            push_item(&mut content, item, avail_w, theme);
            idx += 1;
        }

        let n_lines = content.len() as u16;
        // Render into temp buffer to get scrollable height
        let tmp_h = n_lines.max(area.height);
        let mut tmp = Buffer::empty(Rect::new(0, 0, area.width, tmp_h));

        for (i, line) in content.iter().enumerate() {
            let y = i as u16;
            if y < tmp_h {
                line.clone().render(Rect::new(0, y, area.width, 1), &mut tmp);
            }
        }

        // Now render a Paragraph from this buffer, scrolled
        let visible = area.height as usize;
        let total = n_lines as usize;
        let max_off = total.saturating_sub(visible);
        let offset = if scroll.manual_scroll { max_off.saturating_sub(scroll.offset) } else { max_off };

        let clipped: Vec<Line> = content.into_iter().skip(offset).take(visible).collect();

        Paragraph::new(Text::from(clipped))
            .block(Block::default().style(theme.surface_style()))
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }
}

// ── push helpers ──────────────────────────────────────────────────────

fn push_item(out: &mut Vec<Line<'static>>, item: &TranscriptItem, w: usize, theme: &Theme) {
    match item {
        TranscriptItem::SystemMessage { text, level, .. } => {
            let (icon, title) = match level {
                SystemLevel::Info => ("i", "Info"),
                SystemLevel::Warn => ("!", "Warn"),
                SystemLevel::Error => ("X", "Error"),
            };
            push_card(out, &format!("{icon} {title}"), text, w, theme, false, false);
        }
        TranscriptItem::UserMessage { text, .. } => push_card(out, "You", text, w, theme, false, false),
        TranscriptItem::AssistantMessage { text, is_streaming, .. } => {
            let t = if *is_streaming { "Assistant ..." } else { "Assistant" };
            push_card(out, t, text, w, theme, *is_streaming, true);
        }
        TranscriptItem::ThinkingBlock { text, is_streaming, is_expanded, scroll_offset, .. } => {
            push_thinking(out, text, *is_streaming, *is_expanded, *scroll_offset, w, theme);
        }
        TranscriptItem::ToolCallBlock { tool_name, description, is_running, is_success, .. } => {
            let icon = if *is_running { "..." } else if *is_success == Some(true) { "OK" } else if *is_success == Some(false) { "FAIL" } else { "?" };
            push_card(out, &format!("{icon} {tool_name}"), description, w, theme, *is_running, false);
        }
        TranscriptItem::DelegationNotice { from, to, reason, .. } => {
            push_card(out, &format!(">> {from} -> {to}"), reason, w, theme, false, false);
        }
        TranscriptItem::MemoryNotice { before_tokens, after_tokens, .. } => {
            push_card(out, "Memory", &format!("{before_tokens} -> {after_tokens} tokens"), w, theme, false, false);
        }
    }
    out.push(Line::from(""));
}

fn push_card(out: &mut Vec<Line<'static>>, title: &str, body: &str, w: usize, theme: &Theme, streaming: bool, markdown: bool) {
    let style = Style::default().fg(theme.colors.text).bg(theme.colors.bg);
    let border_style = Style::default().fg(theme.colors.border).bg(theme.colors.bg);
    let _title_style = Style::default().fg(theme.colors.primary).bg(theme.colors.bg);

    let inner_w = w.saturating_sub(4);

    // Build the card content
    let card_body: Vec<Line<'static>> = if streaming && body.is_empty() {
        vec![Line::from(Span::styled("...", theme.muted_italic_style()))]
    } else if markdown && !body.is_empty() {
        let md = crate::markdown::render_markdown(body, inner_w);
        md.lines.into_iter().map(|l| {
            let s: String = l.spans.iter().map(|sp| sp.content.as_ref()).collect();
            Line::from(Span::styled(s, style))
        }).collect()
    } else {
        crate::text::wrap_text(body, inner_w)
            .into_iter()
            .map(|s| Line::from(Span::styled(s, style)))
            .collect()
    };

    // Render as ratatui Block widget into temp buffer, extract lines
    let h = (card_body.len() + 2) as u16;
    let card_w = w as u16;
    let mut tmp = Buffer::empty(Rect::new(0, 0, card_w, h));

    let widget = Paragraph::new(Text::from(card_body))
        .block(Block::default().borders(Borders::ALL).border_set(ratatui::symbols::border::ROUNDED).title(title).border_style(border_style))
        .style(style);
    widget.render(Rect::new(0, 0, card_w, h), &mut tmp);

    // Extract rendered lines
    for row in 0..h {
        let start = (row as usize) * (card_w as usize);
        let end = start + card_w as usize;
        let cells = &tmp.content[start..end.min(tmp.content.len())];

        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut i = 0;
        while i < cells.len() {
            let s = cells[i].style();
            let mut run = String::new();
            while i < cells.len() && cells[i].style() == s {
                run.push_str(cells[i].symbol());
                i += 1;
            }
            if !run.is_empty() {
                spans.push(Span::styled(run, s));
            }
        }
        if !spans.is_empty() {
            out.push(Line::from(spans));
        }
    }
}

fn push_thinking(out: &mut Vec<Line<'static>>, text: &str, streaming: bool, expanded: bool, scroll: usize, w: usize, theme: &Theme) {
    let wc = text.split_whitespace().count();
    let title = if streaming { format!("Reasoning (streaming...)") } else { format!("Reasoning ({} words)", wc) };
    let inner_w = w.saturating_sub(4);
    let lines_raw = crate::text::wrap_text(text, inner_w);
    let total = lines_raw.len();

    let body: Vec<Line<'static>> = if expanded {
        let start = if streaming { total.saturating_sub(8) } else { scroll.min(total.saturating_sub(1)) };
        let mut v: Vec<Line<'static>> = lines_raw.iter().skip(start).take(8)
            .map(|s| Line::from(Span::styled(s.clone(), theme.muted_italic_style())))
            .collect();
        if total > 8 {
            v.push(Line::from(Span::styled(
                format!("-- {}-{} of {total} --", start+1, (start+8).min(total)),
                theme.muted_style(),
            )));
        }
        v
    } else if !streaming {
        vec![Line::from(Span::styled(format!("({wc} words — Ctrl+R to expand)"), theme.text_dim_style()))]
    } else {
        vec![]
    };

    let h = (body.len() + 2) as u16;
    let cw = w as u16;
    let mut tmp = Buffer::empty(Rect::new(0, 0, cw, h));
    Paragraph::new(Text::from(body))
        .block(Block::default().borders(Borders::ALL).border_set(ratatui::symbols::border::ROUNDED).title(title).border_style(Style::default().fg(theme.colors.border)))
        .render(Rect::new(0, 0, cw, h), &mut tmp);

    for row in 0..h {
        let cells = &tmp.content[(row as usize * cw as usize)..];
        let mut spans = vec![];
        let mut j = 0;
        while j < cw as usize && j < cells.len() {
            let s = cells[j].style();
            let mut run = String::new();
            while j < cw as usize && j < cells.len() && cells[j].style() == s { run.push_str(cells[j].symbol()); j += 1; }
            if !run.is_empty() { spans.push(Span::styled(run, s)); }
        }
        if !spans.is_empty() { out.push(Line::from(spans)); }
    }
}

fn push_assistant_with_thinking(
    out: &mut Vec<Line<'static>>,
    assist: &str, as_: bool,
    think: &str, ts: bool, te: bool, so: usize,
    w: usize, theme: &Theme,
) {
    let title = if as_ { "Assistant ..." } else { "Assistant" };
    let inner_w = w.saturating_sub(4);
    let think_lines = crate::text::wrap_text(think, inner_w);
    let total = think_lines.len();
    let wc = think.split_whitespace().count();

    let mut body: Vec<Line<'static>> = Vec::new();

    if te {
        // Expanded: show header + lines
        body.push(Line::from(Span::styled(format!("Reasoning ({} words):", wc), theme.muted_italic_style())));
        let start = if ts { total.saturating_sub(8) } else { so.min(total.saturating_sub(1)) };
        for line in think_lines.iter().skip(start).take(8) {
            body.push(Line::from(Span::styled(format!("  {line}"), theme.muted_italic_style())));
        }
        if total > 8 {
            body.push(Line::from(Span::styled(
                format!("  -- {}-{} of {total} --", start+1, (start+8).min(total)),
                theme.muted_style(),
            )));
        }
    } else if !ts {
        // Collapsed: single line with word count + expand hint
        body.push(Line::from(Span::styled(
            format!("Reasoning ({} words) — Ctrl+R to expand", wc),
            theme.text_dim_style(),
        )));
    } else {
        // Streaming but collapsed (shouldn't normally happen)
        body.push(Line::from(Span::styled("Reasoning…", theme.muted_italic_style())));
    }

    // separator
    body.push(Line::from(Span::styled("─".repeat(inner_w.min(60)), Style::default().fg(theme.colors.border))));

    if as_ && assist.is_empty() {
        body.push(Line::from(Span::styled("...", theme.muted_italic_style())));
    } else if !assist.is_empty() {
        let md = crate::markdown::render_markdown(assist, inner_w);
        for l in md.lines {
            let s: String = l.spans.iter().map(|sp| sp.content.as_ref()).collect();
            body.push(Line::from(Span::styled(s, theme.text_style())));
        }
    }

    let h = (body.len() + 2) as u16;
    let cw = w as u16;
    let mut tmp = Buffer::empty(Rect::new(0, 0, cw, h));
    Paragraph::new(Text::from(body))
        .block(Block::default().borders(Borders::ALL).border_set(ratatui::symbols::border::ROUNDED).title(title).border_style(Style::default().fg(theme.colors.border)))
        .render(Rect::new(0, 0, cw, h), &mut tmp);

    for row in 0..h {
        let cells = &tmp.content[(row as usize * cw as usize)..];
        let mut spans = vec![];
        let mut j = 0;
        while j < cw as usize && j < cells.len() {
            let s = cells[j].style();
            let mut run = String::new();
            while j < cw as usize && j < cells.len() && cells[j].style() == s { run.push_str(cells[j].symbol()); j += 1; }
            if !run.is_empty() { spans.push(Span::styled(run, s)); }
        }
        if !spans.is_empty() { out.push(Line::from(spans)); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    fn t() -> Theme { Theme::dark() }

    #[test]
    fn user_card() {
        let theme = t(); let mut v = vec![];
        push_item(&mut v, &TranscriptItem::UserMessage { text: "hi".into(), timestamp: Instant::now() }, 40, &theme);
        let all: String = v.iter().flat_map(|l| l.spans.iter().map(|s| s.content.as_ref())).collect();
        assert!(all.contains("You")); assert!(all.contains("hi"));
    }
    #[test]
    fn assistant_md() {
        let theme = t(); let mut v = vec![];
        push_item(&mut v, &TranscriptItem::AssistantMessage { text: "**b**".into(), timestamp: Instant::now(), is_streaming: false }, 40, &theme);
        let all: String = v.iter().flat_map(|l| l.spans.iter().map(|s| s.content.as_ref())).collect();
        assert!(all.contains("Assistant")); assert!(all.contains("b"));
    }
    #[test]
    fn error() {
        let theme = t(); let mut v = vec![];
        push_item(&mut v, &TranscriptItem::SystemMessage { text: "fail".into(), level: SystemLevel::Error, timestamp: Instant::now() }, 40, &theme);
        let all: String = v.iter().flat_map(|l| l.spans.iter().map(|s| s.content.as_ref())).collect();
        assert!(all.contains("Error"));
    }
    #[test]
    fn streaming() {
        let theme = t(); let mut v = vec![];
        push_item(&mut v, &TranscriptItem::AssistantMessage { text: String::new(), timestamp: Instant::now(), is_streaming: true }, 40, &theme);
        let all: String = v.iter().flat_map(|l| l.spans.iter().map(|s| s.content.as_ref())).collect();
        assert!(all.contains("..."));
    }
    #[test]
    fn think_in_assist() {
        let theme = t(); let mut v = vec![];
        push_assistant_with_thinking(&mut v, "ans", false, "thk", false, true, 0, 40, &theme);
        let all: String = v.iter().flat_map(|l| l.spans.iter().map(|s| s.content.as_ref())).collect();
        assert!(all.contains("Assistant")); assert!(all.contains("Reasoning")); assert!(all.contains("thk")); assert!(all.contains("ans"));
    }
    #[test]
    fn tool() {
        let theme = t(); let mut v = vec![];
        push_item(&mut v, &TranscriptItem::ToolCallBlock { tool_call_id: "t".into(), tool_name: "sh".into(), description: "ls".into(), raw_args: None, is_running: false, is_success: Some(true), is_expanded: false, timestamp: Instant::now(), output: None }, 40, &theme);
        let all: String = v.iter().flat_map(|l| l.spans.iter().map(|s| s.content.as_ref())).collect();
        assert!(all.contains("sh")); assert!(all.contains("OK"));
    }
    #[test]
    fn range() {
        let mut tr = Transcript::new();
        for _ in 0..50 { tr.push(TranscriptItem::SystemMessage { text: "m".into(), level: SystemLevel::Info, timestamp: Instant::now() }); }
        let (s, e) = tr.estimate_visible_range(20);
        assert!(e <= 50 && s <= e && e - s > 0);
    }
}
