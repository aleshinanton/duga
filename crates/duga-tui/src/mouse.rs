//! Mouse click regions — reusable close buttons and hit-testing.
//!
//! Components register `ClickRegion`s during rendering, and
//! `App::handle_mouse_click` dispatches them on left-click.

use ratatui::layout::{Alignment, Rect};
use ratatui::text::Line;
use ratatui::widgets::Block;

/// Identifies what action a mouse click on a region should trigger.
#[derive(Clone, Copy, Debug)]
pub enum ClickTarget {
    /// Close the reasoning panel.
    CloseReasoning,
    /// Close the event log panel.
    CloseEvents,
    /// Dismiss the error/warning banner.
    DismissBanner,
}

/// A clickable screen region registered during rendering.
#[derive(Clone, Debug)]
pub struct ClickRegion {
    /// The terminal row (y) where the hit area sits.
    pub row: u16,
    /// Start column of the hit area (inclusive).
    pub col_start: u16,
    /// End column of the hit area (exclusive).
    pub col_end: u16,
    /// What happens when this region is clicked.
    pub target: ClickTarget,
}

/// Add a close "✕" title to a [`Block`], and register a click region
/// so mouse clicks on it are dispatched to the given target.
///
/// Returns the modified block and pushes a [`ClickRegion`] into `regions`.
pub fn with_close_title<'a>(
    block: Block<'a>,
    regions: &mut Vec<ClickRegion>,
    rect: Rect,
    target: ClickTarget,
) -> Block<'a> {
    // The ✕ is rendered right-aligned on the top border row.
    // We use the rightmost 3 columns as the hit area.
    regions.push(ClickRegion {
        row: rect.y,
        col_start: rect.right().saturating_sub(3),
        col_end: rect.right(),
        target,
    });

    block.title_top(Line::from("✕").alignment(Alignment::Right))
}
