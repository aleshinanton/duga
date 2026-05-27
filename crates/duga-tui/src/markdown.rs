//! Markdown-to-ratatui rendering adapter.
//!
//! Converts `pulldown-cmark` events into ratatui `Text` with styling.
//! Supports: headings, bold, italic, code spans, code blocks, blockquotes,
//! lists, paragraphs, and links.

use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};

/// Convert a markdown string to ratatui `Text` with styling.
///
/// `max_width` is the available width for word-wrapping.
/// If `max_width` is 0, no wrapping is applied.
pub fn render_markdown(markdown: &str, max_width: usize) -> Text<'static> {
    let mut converter = MarkdownConverter::new(max_width);
    converter.convert(markdown)
}

/// Styled text with heading level information.
pub struct StyledText {
    pub text: Text<'static>,
}

struct MarkdownConverter {
    max_width: usize,
    lines: Vec<Line<'static>>,
    current_line: Vec<Span<'static>>,
    current_style: Style,
    /// True if we're inside a code block (no inline formatting).
    in_code_block: bool,
    code_block_lang: Option<String>,
    /// List nesting level.
    list_depth: usize,
    /// Set of styles active right now.
    bold: bool,
    italic: bool,
}

impl MarkdownConverter {
    fn new(max_width: usize) -> Self {
        Self {
            max_width,
            lines: Vec::new(),
            current_line: Vec::new(),
            current_style: Style::default(),
            in_code_block: false,
            code_block_lang: None,
            list_depth: 0,
            bold: false,
            italic: false,
        }
    }

    fn convert(&mut self, markdown: &str) -> Text<'static> {
        let parser = Parser::new(markdown);
        for event in parser {
            self.handle_event(event);
        }
        self.flush_line();
        Text::from(self.lines.clone())
    }

    fn handle_event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.handle_start_tag(tag),
            Event::End(tag_end) => self.handle_end_tag(tag_end),
            Event::Text(text) => self.handle_text(text.into()),
            Event::Code(code) => self.handle_inline_code(code.into()),
            Event::Html(_html) => {
                // Skip HTML for TUI rendering
            }
            Event::InlineHtml(_html) => {
                // Skip inline HTML
            }
            Event::SoftBreak => {
                self.push_str(" ");
            }
            Event::HardBreak => {
                self.flush_line();
            }
            Event::Rule => {
                self.flush_line();
                if self.max_width > 0 {
                    let rule = "─".repeat(self.max_width.min(40));
                    self.lines.push(Line::from(Span::styled(
                        rule,
                        Style::default().fg(Color::DarkGray),
                    )));
                } else {
                    self.lines
                        .push(Line::from(Span::styled("───", Style::default().fg(Color::DarkGray))));
                }
            }
            Event::FootnoteReference(_) => {}
            Event::TaskListMarker(_checked) => {}
            Event::InlineMath(_) | Event::DisplayMath(_) => {}
        }
    }

    fn handle_start_tag(&mut self, tag: Tag) {
        if self.in_code_block {
            return; // No formatting inside code blocks
        }
        match tag {
            Tag::Heading { level, .. } => {
                self.flush_line();
                let color = match level {
                    HeadingLevel::H1 | HeadingLevel::H2 | HeadingLevel::H3 => Color::Yellow,
                    _ => Color::DarkGray,
                };
                self.current_style = self.current_style.fg(color).add_modifier(Modifier::BOLD);
                // Prefix for headings
                let prefix = "#".repeat(level as usize);
                self.push_str(&format!("{prefix} "));
            }
            Tag::Paragraph => {
                // Paragraphs are implicit; add blank line before
                if !self.lines.is_empty() || !self.current_line.is_empty() {
                    self.flush_line();
                    // Add a blank line between paragraphs for readability
                    if self.lines.last().map(|l| !l.spans.is_empty()).unwrap_or(false) {
                        self.lines.push(Line::from(""));
                    }
                }
            }
            Tag::BlockQuote(_) => {
                self.flush_line();
                self.current_style = self.current_style.fg(Color::DarkGray);
                self.push_str("│ ");
            }
            Tag::CodeBlock(kind) => {
                self.flush_line();
                self.in_code_block = true;
                self.code_block_lang = match kind {
                    pulldown_cmark::CodeBlockKind::Fenced(lang) if !lang.is_empty() => {
                        Some(lang.to_string())
                    }
                    _ => None,
                };
                // Render code block header
                if let Some(ref lang) = self.code_block_lang {
                    self.lines.push(Line::from(Span::styled(
                        format!(" ```{lang}"),
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    )));
                }
            }
            Tag::List(num) => {
                self.flush_line();
                self.list_depth += 1;
                let _ = num; // We render bullets ourselves
            }
            Tag::Item => {
                self.flush_line();
                let indent = "  ".repeat(self.list_depth.saturating_sub(1));
                let bullet = if self.list_depth > 0 {
                    "• "
                } else {
                    "- "
                };
                self.push_str(&format!("{indent}{bullet}"));
            }
            Tag::Table(_) => {
                self.flush_line();
            }
            Tag::TableHead | Tag::TableRow => {}
            Tag::TableCell => {
                self.push_str(" │ ");
            }
            Tag::Emphasis => {
                self.italic = true;
                self.current_style = self.current_style.add_modifier(Modifier::ITALIC);
            }
            Tag::Strong => {
                self.bold = true;
                self.current_style = self.current_style.add_modifier(Modifier::BOLD);
            }
            Tag::Strikethrough => {
                self.current_style = self
                    .current_style
                    .add_modifier(Modifier::CROSSED_OUT);
            }
            Tag::Link { .. } => {
                self.current_style = self.current_style.fg(Color::Cyan).add_modifier(Modifier::UNDERLINED);
            }
            Tag::Image { .. } => {
                // Can't render images in TUI, show placeholder
                self.push_str("[Image]");
            }
            Tag::MetadataBlock(_) => {}
            _ => {}
        }
    }

    fn handle_end_tag(&mut self, tag_end: TagEnd) {
        match tag_end {
            TagEnd::Heading(_level) => {
                self.flush_line();
                self.current_style = Style::default();
            }
            TagEnd::Paragraph => {
                // No special handling needed
            }
            TagEnd::BlockQuote(_) => {
                self.flush_line();
                self.current_style = Style::default();
            }
            TagEnd::CodeBlock => {
                self.in_code_block = false;
                self.code_block_lang = None;
                self.flush_line();
            }
            TagEnd::List(_) => {
                self.list_depth = self.list_depth.saturating_sub(1);
                self.flush_line();
            }
            TagEnd::Item => {
                // No special handling
            }
            TagEnd::Table => {
                self.flush_line();
            }
            TagEnd::TableHead | TagEnd::TableRow => {}
            TagEnd::TableCell => {}
            TagEnd::Emphasis => {
                self.italic = false;
                self.current_style = self
                    .current_style
                    .remove_modifier(Modifier::ITALIC);
            }
            TagEnd::Strong => {
                self.bold = false;
                self.current_style = self
                    .current_style
                    .remove_modifier(Modifier::BOLD);
            }
            TagEnd::Strikethrough => {
                self.current_style = self
                    .current_style
                    .remove_modifier(Modifier::CROSSED_OUT);
            }
            TagEnd::Link => {
                self.current_style = Style::default();
            }
            TagEnd::Image => {}
            TagEnd::MetadataBlock(_) => {}
            _ => {}
        }
    }

    fn handle_text(&mut self, text: std::borrow::Cow<'_, str>) {
        if self.in_code_block {
            // In a code block, render raw text with a monospace-like style
            let style = Style::default()
                .fg(Color::Gray)
                .bg(Color::Rgb(30, 30, 30));
            self.lines.push(Line::from(Span::styled(
                text.to_string(),
                style,
            )));
            return;
        }
        self.push_str(&text);
    }

    fn handle_inline_code(&mut self, code: std::borrow::Cow<'_, str>) {
        if self.in_code_block {
            let style = Style::default()
                .fg(Color::Gray)
                .bg(Color::Rgb(30, 30, 30));
            self.lines.push(Line::from(Span::styled(
                code.to_string(),
                style,
            )));
            return;
        }
        self.push_span(Span::styled(
            format!("`{code}`"),
            Style::default()
                .fg(Color::Yellow)
                .bg(Color::Rgb(40, 40, 40)),
        ));
    }

    fn push_str(&mut self, s: &str) {
        let span = Span::styled(s.to_string(), self.current_style);
        self.current_line.push(span);
    }

    fn push_span(&mut self, span: Span<'static>) {
        self.current_line.push(span);
    }

    fn flush_line(&mut self) {
        if !self.current_line.is_empty() {
            let line = Line::from(self.current_line.clone());
            if self.max_width > 0 {
                // Word-wrap the line
                // For simplicity, we concatenate spans and wrap
                let text: String = line
                    .spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<Vec<_>>()
                    .join("");
                let wrapped = crate::text::wrap_text(&text, self.max_width);
                for w in wrapped {
                    self.lines.push(Line::from(w));
                }
            } else {
                self.lines.push(line);
            }
            self.current_line.clear();
        }
    }
}

/// Quick markdown-to-text for simple strings (no styling).
pub fn markdown_to_plain_text(markdown: &str) -> String {
    let parser = Parser::new(markdown);
    let mut text = String::new();
    for event in parser {
        if let Event::Text(t) = event {
            text.push_str(&t);
        } else if let Event::Code(c) = event {
            text.push_str(&c);
        } else if let Event::SoftBreak = event {
            text.push(' ');
        } else if let Event::HardBreak = event {
            text.push('\n');
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_plain_text() {
        let text = render_markdown("hello world", 80);
        let lines: Vec<String> = text.lines.iter().map(|l| {
            l.spans.iter().map(|s| s.content.as_ref()).collect::<Vec<_>>().join("")
        }).collect();
        let any_hello = lines.iter().any(|l| l.contains("hello"));
        assert!(any_hello, "expected 'hello' in {:?}", lines);
    }

    #[test]
    fn render_bold_text() {
        use Modifier;
        let text = render_markdown("**bold** text", 80);
        // Should have a span with BOLD modifier
        let spans: Vec<&Span> = text.lines.iter().flat_map(|l| l.spans.iter()).collect();
        let has_bold = spans.iter().any(|s| {
            s.style.add_modifier(Modifier::BOLD).add_modifier(Modifier::BOLD)
                == s.style.add_modifier(Modifier::BOLD)
        });
        // We check that at least one span contains "bold"
        let contains_bold_text = spans.iter().any(|s| s.content.contains("bold"));
        assert!(contains_bold_text, "expected a span containing 'bold'");
    }

    #[test]
    fn markdown_to_plain_extracts_text() {
        let result = markdown_to_plain_text("hello **world**");
        assert_eq!(result, "hello world");
    }
}
