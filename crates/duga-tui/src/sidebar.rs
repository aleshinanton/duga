//! Sidebar container — split pane with reasoning panel (top) and event log (bottom).
//!
//! Each panel can be collapsed independently. When both collapsed, shows
//! empty state. Uses `EventLog` and `ReasoningPanel` for content.

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::widgets::{Paragraph, Widget};

use crate::event_log::EventLog;
use crate::reasoning_panel::ReasoningPanel;
use crate::theme::Theme;

/// Sidebar state: which panels are expanded.
#[derive(Clone, Debug)]
pub struct SidebarState {
    pub reasoning_expanded: bool,
    pub events_expanded: bool,
}

impl Default for SidebarState {
    fn default() -> Self {
        Self {
            reasoning_expanded: false,
            events_expanded: false,
        }
    }
}

impl SidebarState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn toggle_reasoning(&mut self) {
        self.reasoning_expanded = !self.reasoning_expanded;
    }

    pub fn toggle_events(&mut self) {
        self.events_expanded = !self.events_expanded;
    }

    /// Whether any panel is expanded.
    pub fn has_content(&self) -> bool {
        self.reasoning_expanded || self.events_expanded
    }
}

/// Sidebar view — renders reasoning panel + event log inside the sidebar area.
pub struct SidebarView;

impl SidebarView {
    /// Render the sidebar into the given area.
    /// When width is 0, do nothing (responsive collapse).
    pub fn render(
        area: Rect,
        buf: &mut Buffer,
        state: &SidebarState,
        reasoning: &ReasoningPanel,
        event_log: &EventLog,
        scroll_offset: usize,
        theme: &Theme,
    ) {
        if area.width == 0 {
            return;
        }

        let show_reasoning = state.reasoning_expanded;
        let show_events = state.events_expanded;

        // Compute sub-panes
        let (reasoning_rect, events_rect) =
            compute_panels(area, show_reasoning, show_events);

        // Render reasoning panel
        if show_reasoning {
            reasoning.render(
                reasoning_rect,
                buf,
                theme,
            );
        }

        // Render event log panel
        if show_events {
            event_log.render(events_rect, buf, theme, scroll_offset);
        }

        // If both collapsed, show empty state
        if !show_reasoning && !show_events {
            render_empty_sidebar(area, buf, theme);
        }
    }
}

/// Compute sub-panel rectangles.
fn compute_panels(
    area: Rect,
    show_reasoning: bool,
    show_events: bool,
) -> (Rect, Rect) {
    match (show_reasoning, show_events) {
        (true, true) => {
            let split = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(area);
            (split[0], split[1])
        }
        (true, false) => (area, Rect::default()),
        (false, true) => (Rect::default(), area),
        (false, false) => (Rect::default(), Rect::default()),
    }
}

/// Render empty sidebar message.
fn render_empty_sidebar(area: Rect, buf: &mut Buffer, theme: &Theme) {
    let text = "No sidebar content\n\nCtrl+R for reasoning\nCtrl+E for events";
    let widget = Paragraph::new(text)
        .style(theme.text_dim_style());
    widget.render(area, buf);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_state_defaults() {
        let state = SidebarState::new();
        assert!(!state.reasoning_expanded);
        assert!(state.events_expanded);
        assert!(state.has_content());
    }

    #[test]
    fn toggle_reasoning() {
        let mut state = SidebarState::new();
        state.toggle_reasoning();
        assert!(state.reasoning_expanded);
        state.toggle_reasoning();
        assert!(!state.reasoning_expanded);
    }

    #[test]
    fn toggle_events() {
        let mut state = SidebarState::new();
        state.toggle_events();
        assert!(!state.events_expanded);
        assert!(!state.has_content());
    }

    #[test]
    fn both_collapsed_no_content() {
        let mut state = SidebarState::new();
        state.toggle_events();
        assert!(!state.has_content());
    }

    #[test]
    fn compute_panels_both_expanded() {
        let area = Rect::new(0, 0, 40, 20);
        let (reasoning, events) = compute_panels(area, true, true);
        assert_eq!(reasoning.height, 10);
        assert_eq!(events.height, 10);
        assert_eq!(reasoning.width, 40);
        assert_eq!(events.width, 40);
    }

    #[test]
    fn compute_panels_one_collapsed() {
        let area = Rect::new(0, 0, 40, 20);
        let (reasoning, events) = compute_panels(area, true, false);
        assert_eq!(reasoning.height, 20);
        assert_eq!(events.height, 0);
    }
}
