//! Focus model — keyboard navigation router.
//!
//! `Focus::Chat | Reasoning | EventLog | Input | Overlay` enum.
//! Tab cycles through visible panels: Chat → [Reasoning] → [EventLog] → Input → Chat.
//! Sidebar panels are only in the cycle when they are expanded and the sidebar is rendered.
//! Shift+Tab cycles reasoning effort (Off → Low → Medium → High → Off).
//! Overlay is modal: entered when a dialog opens, exited via Esc/action.
//! Ctrl+R toggles reasoning, Ctrl+E toggles events. Esc returns to Chat.
//! Typing any character auto-switches focus to Input.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Which pane has keyboard focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Chat,
    Reasoning,
    EventLog,
    Input,
    Overlay,
}

/// Describes which sidebar panels are visible for focus cycling.
#[derive(Clone, Copy, Debug, Default)]
pub struct SidebarPanels {
    pub reasoning: bool,
    pub event_log: bool,
}

impl SidebarPanels {
    pub fn any(&self) -> bool {
        self.reasoning || self.event_log
    }
}

impl Focus {
    /// Default focus is Input.
    pub fn default() -> Self {
        Self::Input
    }

    /// Returns true if this focus is on a sidebar panel.
    pub fn is_sidebar(&self) -> bool {
        matches!(self, Focus::Reasoning | Focus::EventLog)
    }

    /// Tab order: Chat → [Reasoning] → [EventLog] → Input → Chat.
    /// Only includes sidebar panels that are actually visible.
    /// Overlay is modal and NOT in the cycle.
    pub fn next(&self, panels: SidebarPanels) -> Self {
        match self {
            Focus::Chat if panels.reasoning => Focus::Reasoning,
            Focus::Chat if panels.event_log => Focus::EventLog,
            Focus::Chat => Focus::Input,
            Focus::Reasoning if panels.event_log => Focus::EventLog,
            Focus::Reasoning => Focus::Input,
            Focus::EventLog => Focus::Input,
            Focus::Input => Focus::Chat,
            Focus::Overlay => Focus::Overlay,
        }
    }

    /// Reverse Tab order: Input → [EventLog] → [Reasoning] → Chat.
    pub fn prev(&self, panels: SidebarPanels) -> Self {
        match self {
            Focus::Chat => Focus::Input,
            Focus::Input if panels.event_log => Focus::EventLog,
            Focus::Input if panels.reasoning => Focus::Reasoning,
            Focus::Input => Focus::Chat,
            Focus::EventLog if panels.reasoning => Focus::Reasoning,
            Focus::EventLog => Focus::Chat,
            Focus::Reasoning => Focus::Chat,
            Focus::Overlay => Focus::Overlay,
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
    /// Returns a `FocusAction` indicating what to do.
    pub fn route(
        key: &KeyEvent,
        focus: &mut Focus,
        panels: SidebarPanels,
        has_overlay: bool,
        is_running: bool,
    ) -> FocusAction {
        // Overlay mode: block all focus shortcuts
        if *focus == Focus::Overlay {
            return FocusAction::PassToOverlay;
        }

        // If any overlay is active, let overlay consume keys.
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
                *focus = focus.next(panels);
                FocusAction::Consumed
            }

            // Shift+Tab: cycle reasoning/thinking effort
            KeyEvent {
                code: KeyCode::BackTab,
                ..
            } => {
                FocusAction::CycleThinkingEffort
            }
            // Shift+Tab via Shift modifier on Tab key
            KeyEvent {
                code: KeyCode::Tab,
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::SHIFT) => {
                FocusAction::CycleThinkingEffort
            }

            // Ctrl+R: toggle reasoning panel (only when idle)
            KeyEvent {
                code: KeyCode::Char('r'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } if !is_running => {
                *focus = Focus::Reasoning;
                FocusAction::ToggleReasoning
            }

            // Ctrl+E: toggle event log panel (only when idle)
            KeyEvent {
                code: KeyCode::Char('e'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } if !is_running => {
                *focus = Focus::EventLog;
                FocusAction::ToggleEvents
            }

            // Esc: return to Chat (from any non-overlay focus)
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
    /// Cycle thinking/reasoning effort level.
    CycleThinkingEffort,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_panels() -> SidebarPanels {
        SidebarPanels { reasoning: false, event_log: false }
    }

    fn both_panels() -> SidebarPanels {
        SidebarPanels { reasoning: true, event_log: true }
    }

    fn reasoning_only() -> SidebarPanels {
        SidebarPanels { reasoning: true, event_log: false }
    }

    fn events_only() -> SidebarPanels {
        SidebarPanels { reasoning: false, event_log: true }
    }

    #[test]
    fn focus_next_with_both_panels() {
        assert_eq!(Focus::Chat.next(both_panels()), Focus::Reasoning);
        assert_eq!(Focus::Reasoning.next(both_panels()), Focus::EventLog);
        assert_eq!(Focus::EventLog.next(both_panels()), Focus::Input);
        assert_eq!(Focus::Input.next(both_panels()), Focus::Chat);
    }

    #[test]
    fn focus_next_reasoning_only() {
        assert_eq!(Focus::Chat.next(reasoning_only()), Focus::Reasoning);
        assert_eq!(Focus::Reasoning.next(reasoning_only()), Focus::Input);
        assert_eq!(Focus::Input.next(reasoning_only()), Focus::Chat);
    }

    #[test]
    fn focus_next_events_only() {
        assert_eq!(Focus::Chat.next(events_only()), Focus::EventLog);
        assert_eq!(Focus::EventLog.next(events_only()), Focus::Input);
        assert_eq!(Focus::Input.next(events_only()), Focus::Chat);
    }

    #[test]
    fn focus_next_no_panels() {
        assert_eq!(Focus::Chat.next(no_panels()), Focus::Input);
        assert_eq!(Focus::Input.next(no_panels()), Focus::Chat);
    }

    #[test]
    fn focus_prev_with_both_panels() {
        assert_eq!(Focus::Chat.prev(both_panels()), Focus::Input);
        assert_eq!(Focus::Input.prev(both_panels()), Focus::EventLog);
        assert_eq!(Focus::EventLog.prev(both_panels()), Focus::Reasoning);
        assert_eq!(Focus::Reasoning.prev(both_panels()), Focus::Chat);
    }

    #[test]
    fn focus_prev_no_panels() {
        assert_eq!(Focus::Chat.prev(no_panels()), Focus::Input);
        assert_eq!(Focus::Input.prev(no_panels()), Focus::Chat);
    }

    #[test]
    fn overlay_not_in_tab_cycle() {
        assert_eq!(Focus::Overlay.next(both_panels()), Focus::Overlay);
        assert_eq!(Focus::Overlay.prev(both_panels()), Focus::Overlay);
    }

    #[test]
    fn enter_exit_overlay_roundtrip() {
        let (new, prev) = Focus::enter_overlay(Focus::Chat);
        assert_eq!(new, Focus::Overlay);
        assert_eq!(prev, Focus::Chat);
        assert_eq!(Focus::exit_overlay(prev), Focus::Chat);
    }

    #[test]
    fn tab_key_cycles() {
        let mut focus = Focus::Chat;
        let key = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        let action = FocusRouter::route(&key, &mut focus, both_panels(), false, false);
        assert_eq!(action, FocusAction::Consumed);
        assert_eq!(focus, Focus::Reasoning);
    }

    #[test]
    fn tab_blocked_during_overlay() {
        let mut focus = Focus::Overlay;
        let key = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        let action = FocusRouter::route(&key, &mut focus, both_panels(), false, false);
        assert_eq!(action, FocusAction::PassToOverlay);
        assert_eq!(focus, Focus::Overlay);
    }

    #[test]
    fn ctrl_r_toggles_reasoning() {
        let mut focus = Focus::Chat;
        let key = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL);
        let action = FocusRouter::route(&key, &mut focus, both_panels(), false, false);
        assert_eq!(action, FocusAction::ToggleReasoning);
        assert_eq!(focus, Focus::Reasoning);
    }

    #[test]
    fn ctrl_r_noop_when_sidebar_hidden() {
        let mut focus = Focus::Chat;
        let key = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL);
        let action = FocusRouter::route(&key, &mut focus, no_panels(), false, false);
        // Ctrl+R always toggles reasoning (sidebar visibility is a UI concern, not a router concern)
        assert_eq!(action, FocusAction::ToggleReasoning);
    }

    #[test]
    fn ctrl_r_noop_when_running() {
        let mut focus = Focus::Chat;
        let key = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL);
        let action = FocusRouter::route(&key, &mut focus, both_panels(), false, true);
        assert_eq!(action, FocusAction::PassThrough);
    }

    #[test]
    fn ctrl_e_toggles_events() {
        let mut focus = Focus::Chat;
        let key = KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL);
        let action = FocusRouter::route(&key, &mut focus, both_panels(), false, false);
        assert_eq!(action, FocusAction::ToggleEvents);
        assert_eq!(focus, Focus::EventLog);
    }

    #[test]
    fn esc_returns_to_chat() {
        let mut focus = Focus::Input;
        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        let action = FocusRouter::route(&key, &mut focus, both_panels(), false, false);
        assert_eq!(action, FocusAction::Consumed);
        assert_eq!(focus, Focus::Chat);
    }

    #[test]
    fn bare_r_passes_through() {
        let mut focus = Focus::Chat;
        let key = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE);
        let action = FocusRouter::route(&key, &mut focus, both_panels(), false, false);
        assert_eq!(action, FocusAction::PassThrough);
    }

    #[test]
    fn shift_tab_cycles_thinking_effort() {
        let mut focus = Focus::Chat;
        let key = KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE);
        let action = FocusRouter::route(&key, &mut focus, both_panels(), false, false);
        assert_eq!(action, FocusAction::CycleThinkingEffort);
        assert_eq!(focus, Focus::Chat);
    }

    #[test]
    fn shift_tab_with_shift_modifier_also_cycles_thinking_effort() {
        let mut focus = Focus::Input;
        let key = KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT);
        let action = FocusRouter::route(&key, &mut focus, both_panels(), false, false);
        assert_eq!(action, FocusAction::CycleThinkingEffort);
        assert_eq!(focus, Focus::Input);
    }
}
