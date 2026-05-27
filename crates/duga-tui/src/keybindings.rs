//! Keybinding parsing and matching for the TUI.
//!
//! Converts human-readable key patterns (e.g. "ctrl-c", "shift-enter")
//! into crossterm `KeyEvent` matchers.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use duga_config::KeybindingsConfig;

/// A parsed key pattern that can match against crossterm KeyEvents.
#[derive(Clone, Debug)]
pub struct KeyPattern {
    code: KeyCode,
    modifiers: KeyModifiers,
}

impl KeyPattern {
    /// Parse a key pattern string like "ctrl-c", "f1", "enter", "page-up".
    pub fn parse(pattern: &str) -> Result<Self, String> {
        let pattern = pattern.trim().to_lowercase();

        // Try parsing as a whole key first (for keys with dashes like "page-up")
        if let Some(code) = Self::parse_key_code(&pattern) {
            return Ok(Self { code, modifiers: KeyModifiers::NONE });
        }

        // Parse modifiers first
        let mut modifiers = KeyModifiers::NONE;

        // Parse prefix modifiers separated by "-"
        let parts: Vec<&str> = pattern.split('-').collect();

        let key_part: &str;
        if parts.len() > 1 {
            for part in &parts[..parts.len() - 1] {
                match *part {
                    "ctrl" | "control" => modifiers |= KeyModifiers::CONTROL,
                    "alt" | "meta" => modifiers |= KeyModifiers::ALT,
                    "shift" => modifiers |= KeyModifiers::SHIFT,
                    other => return Err(format!("unknown modifier: {other}")),
                }
            }
            key_part = parts.last().unwrap();
        } else {
            key_part = &pattern;
        }

        let code = match Self::parse_key_code(key_part) {
            Some(c) => c,
            None => return Err(format!("unknown key: {key_part}")),
        };

        Ok(Self { code, modifiers })
    }

    /// Check if this pattern matches a crossterm KeyEvent.
    pub fn matches(&self, event: &KeyEvent) -> bool {
        self.code == event.code && self.modifiers == event.modifiers
    }

    /// Try to parse a key code from a string (without modifiers).
    fn parse_key_code(s: &str) -> Option<KeyCode> {
        match s {
            "enter" | "return" => Some(KeyCode::Enter),
            "escape" | "esc" => Some(KeyCode::Esc),
            "tab" => Some(KeyCode::Tab),
            "backspace" | "back" => Some(KeyCode::Backspace),
            "delete" | "del" => Some(KeyCode::Delete),
            "insert" | "ins" => Some(KeyCode::Insert),
            "home" => Some(KeyCode::Home),
            "end" => Some(KeyCode::End),
            "page-up" | "pageup" => Some(KeyCode::PageUp),
            "page-down" | "pagedown" => Some(KeyCode::PageDown),
            "up" => Some(KeyCode::Up),
            "down" => Some(KeyCode::Down),
            "left" => Some(KeyCode::Left),
            "right" => Some(KeyCode::Right),
            "space" => Some(KeyCode::Char(' ')),
            "f1" => Some(KeyCode::F(1)),
            "f2" => Some(KeyCode::F(2)),
            "f3" => Some(KeyCode::F(3)),
            "f4" => Some(KeyCode::F(4)),
            "f5" => Some(KeyCode::F(5)),
            "f6" => Some(KeyCode::F(6)),
            "f7" => Some(KeyCode::F(7)),
            "f8" => Some(KeyCode::F(8)),
            "f9" => Some(KeyCode::F(9)),
            "f10" => Some(KeyCode::F(10)),
            "f11" => Some(KeyCode::F(11)),
            "f12" => Some(KeyCode::F(12)),
            "null" => Some(KeyCode::Null),
            _ => {
                if s.len() == 1 {
                    Some(KeyCode::Char(s.chars().next().unwrap()))
                } else {
                    None
                }
            }
        }
    }
}

/// Runtime keybindings parsed from config.
#[derive(Clone, Debug)]
pub struct Keybindings {
    pub submit: KeyPattern,
    pub cancel: KeyPattern,
    pub quit: KeyPattern,
    pub help: KeyPattern,
    pub search: KeyPattern,
    pub scroll_up: KeyPattern,
    pub scroll_down: KeyPattern,
    pub toggle_tool: KeyPattern,
}

impl Keybindings {
    /// Build keybindings from config, falling back to defaults on parse errors.
    pub fn from_config(config: &KeybindingsConfig) -> Self {
        Self {
            submit: Self::parse_or_default(&config.submit, "enter"),
            cancel: Self::parse_or_default(&config.cancel, "ctrl-c"),
            quit: Self::parse_or_default(&config.quit, "q"),
            help: Self::parse_or_default(&config.help, "f1"),
            search: Self::parse_or_default(&config.search, "ctrl-f"),
            scroll_up: KeyPattern::parse("page-up").unwrap(),
            scroll_down: KeyPattern::parse("page-down").unwrap(),
            toggle_tool: KeyPattern::parse("tab").unwrap(),
        }
    }

    fn parse_or_default(pattern: &str, default: &str) -> KeyPattern {
        KeyPattern::parse(pattern).unwrap_or_else(|e| {
            tracing::warn!("invalid keybinding '{pattern}': {e}, using default '{default}'");
            KeyPattern::parse(default).expect("default keybinding must parse")
        })
    }
}

impl Default for Keybindings {
    fn default() -> Self {
        Self::from_config(&KeybindingsConfig::default())
    }
}

/// Check if a key event is a global keybinding.
/// Returns the action to take.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GlobalAction {
    Submit,
    Cancel,
    Quit,
    Help,
    Search,
    ScrollUp,
    ScrollDown,
    ToggleTool,
    None,
}

impl Keybindings {
    /// Match a key event against all global bindings.
    /// Returns the first matching action.
    pub fn match_key(&self, event: &KeyEvent) -> GlobalAction {
        if self.submit.matches(event) {
            GlobalAction::Submit
        } else if self.cancel.matches(event) {
            GlobalAction::Cancel
        } else if self.quit.matches(event) {
            GlobalAction::Quit
        } else if self.help.matches(event) {
            GlobalAction::Help
        } else if self.search.matches(event) {
            GlobalAction::Search
        } else if self.scroll_up.matches(event) {
            GlobalAction::ScrollUp
        } else if self.scroll_down.matches(event) {
            GlobalAction::ScrollDown
        } else if self.toggle_tool.matches(event) {
            GlobalAction::ToggleTool
        } else {
            GlobalAction::None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_enter() {
        let p = KeyPattern::parse("enter").unwrap();
        assert!(p.matches(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    }

    #[test]
    fn parse_ctrl_c() {
        let p = KeyPattern::parse("ctrl-c").unwrap();
        assert!(p.matches(&KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL
        )));
    }

    #[test]
    fn parse_f1() {
        let p = KeyPattern::parse("f1").unwrap();
        assert!(p.matches(&KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE)));
    }

    #[test]
    fn parse_shift_enter() {
        let p = KeyPattern::parse("shift-enter").unwrap();
        assert!(p.matches(&KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::SHIFT
        )));
    }

    #[test]
    fn parse_invalid_returns_error() {
        assert!(KeyPattern::parse("invalid-key").is_err());
    }

    #[test]
    fn defaults_parse() {
        let kb = Keybindings::default();
        assert!(kb.submit.matches(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
        assert!(kb.cancel.matches(&KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL
        )));
    }
}
