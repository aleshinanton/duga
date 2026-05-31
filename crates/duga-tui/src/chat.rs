//! Chat panel — renders transcript items as ratatui Block-bordered cards.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Widget, Wrap};

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

        let avail_w = area.width.saturating_sub(2) as usize;

        // Build content lines from ALL items — we clip by line offset below.
        let mut content: Vec<Line<'static>> = Vec::new();
        for item in items {
            push_item(&mut content, item, avail_w, theme);
        }

        // Line-based scrolling: skip the top `offset` lines to show the
        // visible portion.  offset = 0 means "show the bottom of the content"
        // (auto-follow), while larger offsets scroll up into older lines.
        let visible = area.height as usize;
        let total = content.len();
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
        TranscriptItem::ToolCallBlock { tool_name, description, is_running, is_success, is_expanded, raw_args, output, .. } => {
            let icon = if *is_running { "..." } else if *is_success == Some(true) { "OK" } else if *is_success == Some(false) { "FAIL" } else { "?" };
            let body = if *is_expanded {
                let mut b = String::new();
                b.push_str(description);
                if let Some(args) = raw_args {
                    b.push_str("\n\nArgs:\n");
                    b.push_str(args);
                }
                if let Some(out) = output {
                    b.push_str("\n\nOutput:\n");
                    b.push_str(out);
                }
                b
            } else {
                description.clone()
            };
            push_card(out, &format!("{icon} {tool_name}"), &body, w, theme, *is_running, false);
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

    let inner_w = w.saturating_sub(6);

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
        .block(Block::default().borders(Borders::ALL).border_set(ratatui::symbols::border::ROUNDED).padding(Padding::horizontal(1)).title(title).border_style(border_style))
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
    let inner_w = w.saturating_sub(6);
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
        .block(Block::default().borders(Borders::ALL).border_set(ratatui::symbols::border::ROUNDED).padding(Padding::horizontal(1)).title(title).border_style(Style::default().fg(theme.colors.border)))
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
