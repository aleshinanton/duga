//! Telegram authorization and update parsing.
//!
//! Handles chat authorization, command extraction, mention stripping,
//! and inline callback parsing.

use duga_config::TelegramConfig;
use teloxide::types::{ChatId, Message};

/// Callback actions from inline buttons.
#[derive(Clone, Debug)]
pub enum CallbackAction {
    /// Approve a pending confirmation.
    Approve(String),
    /// Deny a pending confirmation.
    Deny(String),
    /// Unknown callback.
    Unknown(String),
}

/// Check whether a chat is authorized to interact with the bot.
pub fn is_allowed_chat(config: &TelegramConfig, chat_id: i64) -> bool {
    if config.allow_all_chats_for_dev {
        return true;
    }
    config.allowed_chat_ids.contains(&chat_id)
}

/// Resolve the effective chat ID from a message (works for both DM and groups).
pub fn resolve_chat_id(msg: &Message) -> Option<ChatId> {
    if msg.chat.is_private() {
        // In DM, the user's chat is the effective chat.
        Some(msg.chat.id)
    } else {
        // In groups, use the group chat ID.
        Some(msg.chat.id)
    }
}

/// Strip the bot's @username mention from a message text.
///
/// In groups, messages look like "@duga_bot do the thing".
/// This returns "do the thing".
pub fn strip_mention<'a>(text: &'a str, bot_username: &str) -> &'a str {
    let mention = format!("@{bot_username}");
    let text = text.trim();
    if let Some(rest) = text.strip_prefix(&mention) {
        rest.trim()
    } else {
        text
    }
}

/// Parse callback data from inline button callbacks.
///
/// Format: `action:payload` (e.g., `approve:abc123`, `deny:abc123`).
pub fn parse_callback_data(data: &str) -> CallbackAction {
    if let Some((action, payload)) = data.split_once(':') {
        match action {
            "approve" => CallbackAction::Approve(payload.to_string()),
            "deny" => CallbackAction::Deny(payload.to_string()),
            _ => CallbackAction::Unknown(data.to_string()),
        }
    } else {
        CallbackAction::Unknown(data.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_allowed_chat() {
        let config = TelegramConfig {
            allowed_chat_ids: vec![123, 456],
            allow_all_chats_for_dev: false,
            ..Default::default()
        };
        assert!(is_allowed_chat(&config, 123));
        assert!(is_allowed_chat(&config, 456));
        assert!(!is_allowed_chat(&config, 789));
    }

    #[test]
    fn test_is_allowed_chat_dev_mode() {
        let config = TelegramConfig {
            allowed_chat_ids: vec![],
            allow_all_chats_for_dev: true,
            ..Default::default()
        };
        assert!(is_allowed_chat(&config, 999));
    }

    #[test]
    fn test_strip_mention() {
        assert_eq!(strip_mention("@duga_bot do stuff", "duga_bot"), "do stuff");
        assert_eq!(strip_mention("just text", "duga_bot"), "just text");
        assert_eq!(strip_mention("@other_bot hi", "duga_bot"), "@other_bot hi");
    }

    #[test]
    fn test_parse_callback_approve() {
        match parse_callback_data("approve:abc-123") {
            CallbackAction::Approve(id) => assert_eq!(id, "abc-123"),
            other => panic!("expected Approve, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_callback_deny() {
        match parse_callback_data("deny:abc-123") {
            CallbackAction::Deny(id) => assert_eq!(id, "abc-123"),
            other => panic!("expected Deny, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_callback_unknown() {
        match parse_callback_data("garbage") {
            CallbackAction::Unknown(s) => assert_eq!(s, "garbage"),
            _ => panic!("expected Unknown"),
        }
    }
}
