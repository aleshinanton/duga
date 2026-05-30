//! Focus model — keyboard navigation router.
//!
//! `Focus::Chat | Sidebar | Input | Overlay` enum.
//! Tab/Shift+Tab cycles through Chat → Sidebar → Input (skipping Overlay).
//! Overlay is modal: entered when a dialog opens, exited via Esc/action.
//! Dedicated shortcuts (`i`, `r`, `e`, `Esc`) jump to specific panes.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Which pane has keyboard focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Chat,
    Sidebar,
    Input,
    Overlay,
}

impl Focus {
    /// Default focus is Chat.
    pub fn default() -> Self {
        Self::Chat
    }

    /// Tab order: Chat → Sidebar → Input → Chat.
    /// Overlay is modal and NOT in the cycle.
    pub fn next(&self, sidebar_visible: bool) -> Self {
        match self {
            Focus::Chat if sidebar_visible => Focus::Sidebar,
            Focus::Chat => Focus::Input,
            Focus::Sidebar => Focus::Input,
            Focus::Input => Focus::Chat,
            Focus::Overlay => Focus::Overlay, // Tab does NOT change out of overlay
        }
    }

    /// Shift+Tab order: Chat → Input → Sidebar → Chat.
    pub fn prev(&self, sidebar_visible: bool) -> Self {
        match self {
            Focus::Chat => Focus::Input,
            Focus::Input if sidebar_visible => Focus::Sidebar,
            Focus::Input => Focus::Chat,
            Focus::Sidebar => Focus::Chat,
            Focus::Overlay => Focus::Overlay, // Shift+Tab does NOT change out of overlay
        }
    }

    /// Enter overlay mode: saves current focus, sets Overlay.
    pub fn enter_overlay(current: Focus) -> (Focus, Focus) {
        (Focus::Overlay, current) // (new_focus, previous_focus)
    }

    /// Exit overlay mode: restores previous focus.
    pub fn exit_overlay(previous: Focus) -> Focus {
        previous
    }
}

/// Focus router — routes key events based on current focus.
pub struct FocusRouter;

impl FocusRouter {
    /// Route a key event based on current focus.
    /// Returns `true` if the key was consumed (should not be passed further).
    pub fn route(
        key: &KeyEvent,
        focus: &mut Focus,
        sidebar_visible: bool,
        has_overlay: bool,
        is_running: bool,
    ) -> FocusAction {
        // Overlay mode: only Esc closes overlay, everything else is ignored here
        // (overlay gets keys via the overlay manager, not the focus router).
        // But we handle Esc to exit overlay focus.
        if *focus == Focus::Overlay {
            // In overlay mode, Tab and shortcuts are blocked.
            // Only the overlay handler can close itself.
            return FocusAction::PassToOverlay;
        }

        // If any overlay is active (regardless of Focus enum), let overlay consume keys.
        if has_overlay {
            return FocusAction::PassToOverlay;
        }

        match key {
            // Tab: cycle to next focus
            KeyEvent {
                code: KeyCode::Tab,
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                *focus = focus.next(sidebar_visible);
                FocusAction::Consumed
            }

            // Shift+Tab: cycle to previous focus
            KeyEvent {
                code: KeyCode::BackTab,
                ..
            } => {
                *focus = focus.prev(sidebar_visible);
                FocusAction::Consumed
            }
            // Shift+Tab via Shift modifier on Tab key
            KeyEvent {
                code: KeyCode::Tab,
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::SHIFT) => {
                *focus = focus.prev(sidebar_visible);
                FocusAction::Consumed
            }

            // `i`: jump to Input (from any non-overlay focus)
            KeyEvent {
                code: KeyCode::Char('i'),
                modifiers: KeyModifiers::NONE,
                ..
            } if !is_running => {
                *focus = Focus::Input;
                FocusAction::Consumed
            }

            // `r`: jump to Sidebar + toggle reasoning
            KeyEvent {
                code: KeyCode::Char('r'),
                modifiers: KeyModifiers::NONE,
                ..
            } if sidebar_visible && !matches!(key.modifiers, KeyModifiers::CONTROL) => {
                *focus = Focus::Sidebar;
                FocusAction::ToggleReasoning
            }

            // `e`: jump to Sidebar + toggle events
            KeyEvent {
                code: KeyCode::Char('e'),
                modifiers: KeyModifiers::NONE,
                ..
            } if sidebar_visible && !matches!(key.modifiers, KeyModifiers::CONTROL) => {
                *focus = Focus::Sidebar;
                FocusAction::ToggleEvents
            }

            // Esc: return to Chat
            KeyEvent {
                code: KeyCode::Esc,
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                *focus = Focus::Chat;
                FocusAction::Consumed
            }

            _ => FocusAction::PassThrough,
        }
    }
}

/// Result of key routing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FocusAction {
    /// Key was consumed by focus router (don't pass further).
    Consumed,
    /// Key should be passed to the active overlay for handling.
    PassToOverlay,
    /// Key should be passed through to the focused widget.
    PassThrough,
    /// Toggle reasoning panel (sidebar focus + toggle).
    ToggleReasoning,
    /// Toggle events panel (sidebar focus + toggle).
    ToggleEvents,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_next_with_sidebar() {
        assert_eq!(Focus::Chat.next(true), Focus::Sidebar);
        assert_eq!(Focus::Sidebar.next(true), Focus::Input);
        assert_eq!(Focus::Input.next(true), Focus::Chat);
    }

    #[test]
    fn focus_next_without_sidebar() {
        assert_eq!(Focus::Chat.next(false), Focus::Input);
        assert_eq!(Focus::Input.next(false), Focus::Chat);
    }

    #[test]
    fn focus_prev_with_sidebar() {
        assert_eq!(Focus::Chat.prev(true), Focus::Input);
        assert_eq!(Focus::Input.prev(true), Focus::Sidebar);
        assert_eq!(Focus::Sidebar.prev(true), Focus::Chat);
    }

    #[test]
    fn focus_prev_without_sidebar() {
        assert_eq!(Focus::Chat.prev(false), Focus::Input);
        assert_eq!(Focus::Input.prev(false), Focus::Chat);
    }

    #[test]
    fn overlay_not_in_tab_cycle() {
        // Tab from Overlay stays Overlay
        assert_eq!(Focus::Overlay.next(true), Focus::Overlay);
        assert_eq!(Focus::Overlay.prev(true), Focus::Overlay);
    }

    #[test]
    fn enter_exit_overlay_roundtrip() {
        let (new, prev) = Focus::enter_overlay(Focus::Chat);
        assert_eq!(new, Focus::Overlay);
        assert_eq!(prev, Focus::Chat);
        assert_eq!(Focus::exit_overlay(prev), Focus::Chat);
    }

    #[test]
    fn enter_exit_overlay_from_input() {
        let (new, prev) = Focus::enter_overlay(Focus::Input);
        assert_eq!(new, Focus::Overlay);
        assert_eq!(Focus::exit_overlay(prev), Focus::Input);
    }

    #[test]
    fn tab_key_cycles() {
        let mut focus = Focus::Chat;
        let key = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        let action = FocusRouter::route(&key, &mut focus, true, false, false);
        assert_eq!(action, FocusAction::Consumed);
        assert_eq!(focus, Focus::Sidebar);
    }

    #[test]
    fn tab_blocked_during_overlay() {
        let mut focus = Focus::Overlay;
        let key = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        let action = FocusRouter::route(&key, &mut focus, true, false, false);
        assert_eq!(action, FocusAction::PassToOverlay);
        assert_eq!(focus, Focus::Overlay);
    }

    #[test]
    fn tab_blocked_when_has_overlay() {
        let mut focus = Focus::Chat;
        let key = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        let action = FocusRouter::route(&key, &mut focus, true, true, false);
        assert_eq!(action, FocusAction::PassToOverlay);
        assert_eq!(focus, Focus::Chat); // unchanged
    }

    #[test]
    fn i_key_jumps_to_input() {
        let mut focus = Focus::Chat;
        let key = KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE);
        let action = FocusRouter::route(&key, &mut focus, true, false, false);
        assert_eq!(action, FocusAction::Consumed);
        assert_eq!(focus, Focus::Input);
    }

    #[test]
    fn i_key_blocked_when_running() {
        let mut focus = Focus::Chat;
        let key = KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE);
        let action = FocusRouter::route(&key, &mut focus, true, false, true);
        assert_eq!(action, FocusAction::PassThrough);
        assert_eq!(focus, Focus::Chat); // unchanged
    }

    #[test]
    fn i_key_blocked_during_overlay() {
        let mut focus = Focus::Chat;
        let key = KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE);
        let action = FocusRouter::route(&key, &mut focus, true, true, false);
        assert_eq!(action, FocusAction::PassToOverlay);
        assert_eq!(focus, Focus::Chat); // unchanged
    }

    #[test]
    fn r_key_toggles_reasoning() {
        let mut focus = Focus::Chat;
        let key = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE);
        let action = FocusRouter::route(&key, &mut focus, true, false, false);
        assert_eq!(action, FocusAction::ToggleReasoning);
        assert_eq!(focus, Focus::Sidebar);
    }

    #[test]
    fn r_key_noop_when_sidebar_hidden() {
        let mut focus = Focus::Chat;
        let key = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE);
        let action = FocusRouter::route(&key, &mut focus, false, false, false);
        assert_eq!(action, FocusAction::PassThrough);
        assert_eq!(focus, Focus::Chat); // unchanged
    }

    #[test]
    fn esc_returns_to_chat() {
        let mut focus = Focus::Input;
        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        let action = FocusRouter::route(&key, &mut focus, true, false, false);
        assert_eq!(action, FocusAction::Consumed);
        assert_eq!(focus, Focus::Chat);
    }

    #[test]
    fn ctrl_r_not_consumed_as_r() {
        let mut focus = Focus::Chat;
        let key = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL);
        let action = FocusRouter::route(&key, &mut focus, true, false, false);
        assert_eq!(action, FocusAction::PassThrough);
        assert_eq!(focus, Focus::Chat); // unchanged
    }
}
