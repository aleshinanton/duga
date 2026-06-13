//! Event log — structured event log with VecDeque, max 200 entries.
//!
//! Captures tool calls, LLM requests, memory compressions, errors,
//! delegations in a ring buffer. Rendered in the sidebar.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use std::collections::VecDeque;

use crate::mouse::{ClickRegion, ClickTarget, with_close_title};
use crate::theme::Theme;

/// Severity level for log entries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Success,
    Warning,
    Error,
    Debug,
}

/// A single event log entry.
#[derive(Clone, Debug)]
pub struct LogEntry {
    pub timestamp: std::time::Instant,
    pub level: LogLevel,
    pub icon: &'static str,
    pub message: String,
}

impl LogEntry {
    pub fn new(level: LogLevel, icon: &'static str, message: String) -> Self {
        Self {
            timestamp: std::time::Instant::now(),
            level,
            icon,
            message,
        }
    }

    /// Get the display style for this entry.
    pub fn style(&self, theme: &Theme) -> Style {
        match self.level {
            LogLevel::Info => theme.text_dim_style(),
            LogLevel::Success => theme.tool_success_style(),
            LogLevel::Warning => theme.warning_style(),
            LogLevel::Error => theme.error_style(),
            LogLevel::Debug => theme.muted_style(),
        }
    }
}

/// Event log with max capacity (FIFO eviction).
#[derive(Clone, Debug)]
pub struct EventLog {
    entries: VecDeque<LogEntry>,
    max_entries: usize,
}

impl EventLog {
    /// Create a new event log with the given max capacity.
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(max_entries),
            max_entries,
        }
    }

    /// Push a new entry. Evicts oldest if at capacity.
    pub fn push(&mut self, entry: LogEntry) {
        if self.entries.len() >= self.max_entries {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the log is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get all entries (most recent last).
    pub fn entries(&self) -> &VecDeque<LogEntry> {
        &self.entries
    }

    /// Clear all entries.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Render the event log into the given area.
    pub fn render(
        &self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        scroll_offset: usize,
        focused: bool,
        click_regions: &mut Vec<ClickRegion>,
    ) {
        let border_color = if focused {
            theme.colors.primary
        } else {
            theme.colors.border
        };
        let block = Block::default()
            .borders(Borders::ALL).border_set(ratatui::symbols::border::ROUNDED)
            .title(format!("📋 Event Log ({}) ", self.entries.len()))
            .border_style(Style::default().fg(border_color));
        let block = with_close_title(block, click_regions, area, ClickTarget::CloseEvents);

        let inner = block.inner(area);
        block.render(area, buf);

        if self.entries.is_empty() {
            let widget = Paragraph::new("No events yet")
                .style(theme.text_dim_style());
            widget.render(inner, buf);
        } else {
            let max_lines = inner.height as usize;
            let visible: Vec<Line> = self
                .entries
                .iter()
                .rev()
                .skip(scroll_offset)
                .take(max_lines)
                .rev()
                .map(|entry| {
                    Line::from(Span::styled(
                        format!("{} {}", entry.icon, entry.message),
                        entry.style(theme),
                    ))
                })
                .collect();

            let widget = Paragraph::new(visible);
            widget.render(inner, buf);
        }
    }
}

impl Default for EventLog {
    fn default() -> Self {
        Self::new(200)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_adds_entry() {
        let mut log = EventLog::new(5);
        log.push(LogEntry::new(LogLevel::Info, "●", "test".into()));
        assert_eq!(log.len(), 1);
    }

    #[test]
    fn max_capacity_enforced() {
        let mut log = EventLog::new(3);
        for i in 0..5 {
            log.push(LogEntry::new(LogLevel::Info, "●", format!("msg {i}")));
        }
        assert_eq!(log.len(), 3);
        // The first two should have been evicted
        assert_eq!(log.entries[0].message, "msg 2");
        assert_eq!(log.entries[2].message, "msg 4");
    }

    #[test]
    fn different_levels_have_different_icons() {
        let mut log = EventLog::new(10);
        log.push(LogEntry::new(LogLevel::Info, "●", "info".into()));
        log.push(LogEntry::new(LogLevel::Success, "✓", "success".into()));
        log.push(LogEntry::new(LogLevel::Warning, "⚠", "warn".into()));
        log.push(LogEntry::new(LogLevel::Error, "✗", "error".into()));
        assert_eq!(log.len(), 4);
        assert_eq!(log.entries[0].icon, "●");
        assert_eq!(log.entries[1].icon, "✓");
        assert_eq!(log.entries[2].icon, "⚠");
        assert_eq!(log.entries[3].icon, "✗");
    }

    #[test]
    fn clear_removes_all() {
        let mut log = EventLog::new(10);
        log.push(LogEntry::new(LogLevel::Info, "●", "test".into()));
        log.clear();
        assert!(log.is_empty());
    }
}
