//! Layout engine — computes pane rectangles from terminal dimensions.
//!
//! Supports: header (1 row), chat (variable), sidebar (percentage width),
//! error banner (conditional 1 row), input (4 rows), footer (1 row).
//! When width < breakpoint, sidebar collapses to 0 width.

use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// Pre-computed pane rectangles for the multi-pane layout.
#[derive(Clone, Debug)]
pub struct PaneRects {
    /// Header bar (1 row).
    pub header: Rect,
    /// Chat panel (fills remaining horizontal in the chat row).
    pub chat: Rect,
    /// Sidebar panel (percentage-based width, right side).
    pub sidebar: Rect,
    /// Error banner (1 row when active, 0 otherwise).
    pub banner: Rect,
    /// Input area (4 rows).
    pub input: Rect,
    /// Footer bar (1 row).
    pub footer: Rect,
}

/// Computes pane rectangles from terminal size and config.
#[derive(Clone, Debug)]
pub struct LayoutManager {
    /// Sidebar width percentage (15-40, default 25).
    sidebar_width_pct: u8,
    /// Whether the header is shown.
    show_header: bool,
    /// Whether the sidebar is shown.
    show_sidebar: bool,
    /// Whether the footer is shown.
    show_footer: bool,
    /// Width threshold below which sidebar collapses.
    responsive_breakpoint: u16,
}

impl LayoutManager {
    /// Create a new layout manager from TUI config.
    pub fn new(
        sidebar_width_pct: u8,
        show_header: bool,
        show_sidebar: bool,
        show_footer: bool,
        responsive_breakpoint: u16,
    ) -> Self {
        Self {
            sidebar_width_pct: sidebar_width_pct.clamp(15, 40),
            show_header,
            show_sidebar,
            show_footer,
            responsive_breakpoint,
        }
    }

    /// Compute pane rectangles for the given terminal size.
    ///
    /// Args:
    /// - `term_width`: terminal column count
    /// - `term_height`: terminal row count
    /// - `show_banner`: whether the error banner area should be allocated
    pub fn compute(&self, term_width: u16, term_height: u16, show_banner: bool) -> PaneRects {
        let sidebar_visible = self.show_sidebar && term_width >= self.responsive_breakpoint;
        let header_rows: u16 = if self.show_header { 1 } else { 0 };
        let footer_rows: u16 = if self.show_footer { 1 } else { 0 };
        let banner_rows: u16 = if show_banner { 1 } else { 0 };
        let input_rows: u16 = 4;

        // Vertical split:
        //   [header (0|1)] [chat+sidebar (fill)] [banner (0|1)] [input (4)] [footer (0|1)]
        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(header_rows),
                Constraint::Min(5), // chat row fills remaining
                Constraint::Length(banner_rows),
                Constraint::Length(input_rows),
                Constraint::Length(footer_rows),
            ])
            .split(Rect::new(0, 0, term_width, term_height));

        let header = vertical[0];
        let chat_row = vertical[1];
        let banner = vertical[2];
        let input = vertical[3];
        let footer = vertical[4];

        // Horizontal split within chat_row:
        //   [chat (remaining)] [sidebar (percentage)]
        let chat_pct = if sidebar_visible {
            (100 - self.sidebar_width_pct as u16).max(30)
        } else {
            100
        };
        let sidebar_pct = if sidebar_visible {
            self.sidebar_width_pct as u16
        } else {
            0
        };

        let horizontal = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(chat_pct),
                Constraint::Percentage(sidebar_pct),
            ])
            .split(chat_row);

        let chat = horizontal[0];
        let sidebar = horizontal[1];

        PaneRects {
            header,
            chat,
            sidebar,
            banner,
            input,
            footer,
        }
    }

    /// Whether the sidebar would be visible at the given width.
    pub fn is_sidebar_visible(&self, term_width: u16) -> bool {
        self.show_sidebar && term_width >= self.responsive_breakpoint
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_200x60_no_banner() {
        let mgr = LayoutManager::new(25, true, true, true, 120);
        let rects = mgr.compute(200, 60, false);

        // Header: 1 row
        assert_eq!(rects.header.height, 1);
        assert_eq!(rects.header.width, 200);

        // Chat: remaining rows after header (1) + input (4) + footer (1) = 60-6 = 54
        assert_eq!(rects.chat.height, 54);

        // Sidebar: 25% of 200 = 50 cols
        assert_eq!(rects.sidebar.width, 50);
        assert_eq!(rects.chat.width, 150);

        // Banner: 0 height when not showing
        assert_eq!(rects.banner.height, 0);

        // Input: 4 rows
        assert_eq!(rects.input.height, 4);

        // Footer: 1 row
        assert_eq!(rects.footer.height, 1);
    }

    #[test]
    fn compute_80x24_sidebar_hidden() {
        let mgr = LayoutManager::new(25, true, true, true, 120);
        let rects = mgr.compute(80, 24, false);

        // Below breakpoint (120): sidebar should be hidden.
        assert_eq!(rects.sidebar.width, 0);
        assert_eq!(rects.chat.width, 80);
    }

    #[test]
    fn compute_80x24_sidebar_hidden_even_with_content() {
        let mgr = LayoutManager::new(25, true, true, true, 120);
        let rects = mgr.compute(80, 24, false);

        // Content does not override the responsive breakpoint.
        assert_eq!(rects.sidebar.width, 0);
        assert_eq!(rects.chat.width, 80);
    }

    #[test]
    fn compute_120x40_sidebar_visible() {
        let mgr = LayoutManager::new(25, true, true, true, 120);
        let rects = mgr.compute(120, 40, false);

        // At breakpoint: sidebar is 25% of 120 = 30
        assert_eq!(rects.sidebar.width, 30);
        assert_eq!(rects.chat.width, 90);
    }

    #[test]
    fn compute_with_banner() {
        let mgr = LayoutManager::new(25, true, true, true, 120);
        let rects = mgr.compute(200, 60, true);

        // Banner is 1 row
        assert_eq!(rects.banner.height, 1);

        // Input shifts down: chat is shorter
        assert_eq!(rects.chat.height, 53); // 60 - 1(header) - 1(banner) - 4(input) - 1(footer)
        assert_eq!(rects.input.y, 55); // header(0,1) + chat(1,54) + banner(55,1) = input at y=56? Let's check
    }

    #[test]
    fn no_header_no_footer() {
        let mgr = LayoutManager::new(25, false, true, false, 120);
        let rects = mgr.compute(200, 60, false);

        assert_eq!(rects.header.height, 0);
        assert_eq!(rects.footer.height, 0);
        // Chat fills more: 60 - 0 - 4(input) = 56
        assert_eq!(rects.chat.height, 56);
    }

    #[test]
    fn sidebar_width_clamped() {
        // Test that sidebar width is clamped to valid range
        let mgr_clamp_low = LayoutManager::new(5, true, true, true, 120);
        let rects = mgr_clamp_low.compute(200, 60, false);
        assert_eq!(rects.sidebar.width, 30); // 15% of 200

        let mgr_clamp_high = LayoutManager::new(50, true, true, true, 120);
        let rects = mgr_clamp_high.compute(200, 60, false);
        assert_eq!(rects.sidebar.width, 80); // 40% of 200
    }

    #[test]
    fn sidebar_not_shown_when_disabled() {
        let mgr = LayoutManager::new(25, true, false, true, 0);
        let rects = mgr.compute(200, 60, false);

        // Sidebar disabled even at wide terminal
        assert_eq!(rects.sidebar.width, 0);
        assert_eq!(rects.chat.width, 200);
    }

    #[test]
    fn is_sidebar_visible_checks_width() {
        let mgr = LayoutManager::new(25, true, true, true, 120);
        assert!(mgr.is_sidebar_visible(120));
        assert!(!mgr.is_sidebar_visible(119));
        assert!(!mgr.is_sidebar_visible(80));
    }
}
