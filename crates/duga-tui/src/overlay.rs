//! Overlay system for modal dialogs, help screens, and search.
//!
//! Uses a stack-based manager where overlays are pushed/popped.
//! The topmost overlay receives all keyboard events first.
//! Overlays render on top of the main UI.

use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

pub mod confirmation;
pub mod help;
pub mod search;

/// Action returned by an overlay's key handler.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OverlayAction {
    /// Key was consumed by the overlay.
    Consumed,
    /// Close this overlay.
    Close,
    /// Key was not handled (pass to next overlay or widget).
    Ignored,
}

/// Trait for overlay widgets (modals, dialogs, panels).
pub trait Overlay: Send {
    /// Render the overlay within the given area.
    fn render(&self, area: Rect, buf: &mut Buffer);

    /// Handle a keyboard event. Return the action to take.
    fn handle_key(&mut self, key: &KeyEvent) -> OverlayAction;

    /// Calculate the position and size of this overlay within the terminal.
    fn position(&self, terminal_width: u16, terminal_height: u16) -> Rect;
}

/// Manages a stack of overlays.
pub struct OverlayManager {
    stack: Vec<Box<dyn Overlay>>,
}

impl OverlayManager {
    pub fn new() -> Self {
        Self { stack: Vec::new() }
    }

    /// Push an overlay onto the stack (becomes the active overlay).
    pub fn push(&mut self, overlay: Box<dyn Overlay>) {
        self.stack.push(overlay);
    }

    /// Pop the topmost overlay.
    pub fn pop(&mut self) {
        self.stack.pop();
    }

    /// Whether any overlay is active.
    pub fn has_overlay(&self) -> bool {
        !self.stack.is_empty()
    }

    /// Render all overlays (topmost last, so it appears on top).
    pub fn render_all(&self, buf: &mut Buffer, terminal_area: Rect) {
        for overlay in &self.stack {
            let area = overlay.position(terminal_area.width, terminal_area.height);
            overlay.render(area, buf);
        }
    }

    /// Handle a key event — topmost overlay gets first crack.
    /// Returns true if the key was consumed.
    pub fn handle_key(&mut self, key: &KeyEvent) -> bool {
        if let Some(overlay) = self.stack.last_mut() {
            match overlay.handle_key(key) {
                OverlayAction::Consumed => true,
                OverlayAction::Close => {
                    self.stack.pop();
                    true
                }
                OverlayAction::Ignored => false,
            }
        } else {
            false
        }
    }
}

impl Default for OverlayManager {
    fn default() -> Self {
        Self::new()
    }
}
