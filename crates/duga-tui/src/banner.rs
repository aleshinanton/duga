//! Error banners — dismissible full-width notifications.
//!
//! Displays a full-width, dismissible banner above the input area for
//! errors, warnings, and cancellations. Auto-dismisses after a
//! configurable timeout (default 5 seconds) or on user dismissal key.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use std::time::Instant;

use crate::theme::Theme;

/// Severity level for banners.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BannerLevel {
    Info,
    Warn,
    Error,
    Cancel,
}

/// A dismissible banner.
#[derive(Clone, Debug)]
pub struct ErrorBanner {
    /// Active banner content.
    active: Option<(BannerLevel, String, Instant)>,
    /// Auto-dismiss timeout in seconds.
    auto_dismiss_secs: u64,
}

impl ErrorBanner {
    /// Create a new banner manager.
    pub fn new(auto_dismiss_secs: u64) -> Self {
        Self {
            active: None,
            auto_dismiss_secs,
        }
    }

    /// Show a banner. Replaces any existing banner.
    pub fn show(&mut self, level: BannerLevel, message: String) {
        self.active = Some((level, message, Instant::now()));
    }

    /// Dismiss the active banner.
    pub fn dismiss(&mut self) {
        self.active = None;
    }

    /// Whether a banner is currently active.
    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// Tick the auto-dismiss timer. Returns true if still active.
    pub fn tick(&mut self) -> bool {
        if let Some((_, _, since)) = &self.active {
            if since.elapsed().as_secs() >= self.auto_dismiss_secs {
                self.active = None;
                return false;
            }
        }
        self.active.is_some()
    }

    /// Render the banner into the given area.
    pub fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        if area.height == 0 {
            return;
        }

        if let Some((level, message, since)) = &self.active {
            let remaining = self
                .auto_dismiss_secs
                .saturating_sub(since.elapsed().as_secs());

            let (bg_color, fg_color, prefix) = match level {
                BannerLevel::Error => (theme.colors.error, Color::White, "✗ Error"),
                BannerLevel::Warn => (theme.colors.warning, Color::Black, "⚠ Warning"),
                BannerLevel::Cancel => (theme.colors.warning, Color::Black, "◌ Cancelled"),
                BannerLevel::Info => (theme.colors.primary, Color::White, "ℹ Info"),
            };

            let full_text = if remaining > 0 {
                format!(
                    " {}: {}  (dismissing in {}s — Enter to dismiss) ",
                    prefix, message, remaining
                )
            } else {
                format!(" {}: {}  (Enter to dismiss) ", prefix, message)
            };

            let widget = Paragraph::new(Line::from(Span::styled(
                full_text,
                Style::default().fg(fg_color).bg(bg_color),
            )))
            .style(Style::default().bg(bg_color));

            widget.render(area, buf);
        }
    }
}

impl Default for ErrorBanner {
    fn default() -> Self {
        Self::new(5)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn show_sets_active() {
        let mut banner = ErrorBanner::new(5);
        assert!(!banner.is_active());
        banner.show(BannerLevel::Error, "test error".into());
        assert!(banner.is_active());
    }

    #[test]
    fn dismiss_clears_banner() {
        let mut banner = ErrorBanner::new(5);
        banner.show(BannerLevel::Error, "test".into());
        banner.dismiss();
        assert!(!banner.is_active());
    }

    #[test]
    fn show_replaces_previous() {
        let mut banner = ErrorBanner::new(5);
        banner.show(BannerLevel::Error, "first".into());
        banner.show(BannerLevel::Warn, "second".into());
        // Active — no stacking
        assert!(banner.is_active());
    }

    #[test]
    fn tick_auto_dismisses() {
        let mut banner = ErrorBanner::new(0); // 0 second timeout
        banner.show(BannerLevel::Error, "test".into());
        thread::sleep(Duration::from_millis(10));
        assert!(!banner.tick()); // Should dismiss immediately
        assert!(!banner.is_active());
    }

    #[test]
    fn tick_long_timeout_stays_active() {
        let mut banner = ErrorBanner::new(3600); // 1 hour
        banner.show(BannerLevel::Error, "test".into());
        assert!(banner.tick());
    }
}
