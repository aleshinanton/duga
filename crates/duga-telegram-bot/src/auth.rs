//! Telegram authorization and update parsing.
//!
//! Handles chat authorization, command extraction, mention stripping,
//! and inline callback parsing.

use duga_config::TelegramConfig;
use teloxide::types::{ChatId, Message, User, UserId};
/// Callback actions from inline buttons.
#[derive(Clone, Debug)]
pub enum CallbackAction {
    Approve(String),
    Deny(String),
    Unknown(String),
}

/// Check whether a chat is authorized.
/// For DMs, checks sender's @username against allowed_chat_usernames.
/// For groups, checks chat ID against allowed_chat_ids.
pub fn is_allowed_chat(
    config: &TelegramConfig,
    chat_id: i64,
    sender: Option<&User>,
) -> bool {
    if config.allow_all_chats_for_dev {
        return true;
    }
    if config.allowed_chat_ids.contains(&chat_id) {
        return true;
    }
    // Check sender's username.
    if let Some(user) = sender {
        if let Some(ref username) = user.username {
            let lowered = username.to_lowercase();
            if config.allowed_chat_usernames.iter().any(|u| u.to_lowercase() == lowered) {
                return true;
            }
        }
    }
    false
}

/// Resolve the effective chat ID from a message.
pub fn resolve_chat_id(msg: &Message) -> Option<ChatId> {
    Some(msg.chat.id)
}

/// Strip the bot's @username mention from message text.
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
/// Format: `action:payload` (e.g., `approve:abc123`).
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

    fn make_user(username: Option<&str>) -> User {
        User {
            id: UserId(123),
            is_bot: false,
            first_name: "Test".into(),
            last_name: None,
            username: username.map(|s| s.into()),
            language_code: None,
            is_premium: false,
            added_to_attachment_menu: false,
        }
    }

    #[test]
    fn test_is_allowed_chat() {
        let config = TelegramConfig {
            allowed_chat_ids: vec![123, 456],
            allow_all_chats_for_dev: false,
            ..Default::default()
        };
        assert!(is_allowed_chat(&config, 123, None));
        assert!(is_allowed_chat(&config, 456, None));
        assert!(!is_allowed_chat(&config, 789, None));
    }

    #[test]
    fn test_is_allowed_chat_dev_mode() {
        let config = TelegramConfig {
            allowed_chat_ids: vec![],
            allow_all_chats_for_dev: true,
            ..Default::default()
        };
        assert!(is_allowed_chat(&config, 999, None));
    }

    #[test]
    fn test_is_allowed_by_username() {
        let config = TelegramConfig {
            allowed_chat_ids: vec![],
            allowed_chat_usernames: vec!["testuser".into()],
            allow_all_chats_for_dev: false,
            ..Default::default()
        };
        let user = make_user(Some("TestUser"));
        assert!(is_allowed_chat(&config, 999, Some(&user)));
        assert!(!is_allowed_chat(&config, 999, Some(&make_user(Some("other")))));
        assert!(!is_allowed_chat(&config, 999, None));
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
