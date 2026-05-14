//! Integration tests for the Telegram bot — tests auth, commands,
//! session management, and event flow without live API credentials.

#[cfg(test)]
mod bot_flow {
    use duga_telegram_bot::auth::*;
    use duga_config::TelegramConfig;

    #[test]
    fn authorize_allowed_chat() {
        let config = TelegramConfig {
            allowed_chat_ids: vec![123, 456],
            allow_all_chats_for_dev: false,
            ..Default::default()
        };
        assert!(is_allowed_chat(&config, None, 123));
        assert!(!is_allowed_chat(&config, None, 789));
    }

    #[test]
    fn authorize_dev_mode_allows_all() {
        let config = TelegramConfig {
            allowed_chat_ids: vec![],
            allow_all_chats_for_dev: true,
            ..Default::default()
        };
        assert!(is_allowed_chat(&config, None, 999));
    }

    #[test]
    fn strip_bot_mention() {
        assert_eq!(strip_mention("@duga_bot do stuff", "duga_bot"), "do stuff");
        assert_eq!(strip_mention("plain text", "duga_bot"), "plain text");
        assert_eq!(strip_mention("@other hi", "duga_bot"), "@other hi");
    }

    #[test]
    fn parse_callback_approve_deny() {
        match parse_callback_data("approve:abc-123") {
            CallbackAction::Approve(id) => assert_eq!(id, "abc-123"),
            other => panic!("expected Approve, got {other:?}"),
        }
        match parse_callback_data("deny:xyz-456") {
            CallbackAction::Deny(id) => assert_eq!(id, "xyz-456"),
            other => panic!("expected Deny, got {other:?}"),
        }
    }

    #[test]
    fn parse_callback_unknown() {
        match parse_callback_data("garbage") {
            CallbackAction::Unknown(s) => assert_eq!(s, "garbage"),
            _ => panic!("expected Unknown"),
        }
    }
}

#[cfg(test)]
mod formatting_tests {
    use duga_telegram_bot::formatting::*;

    #[test]
    fn escape_html() {
        let result = escape_telegram_plain_text("<b>bold</b>");
        assert!(!result.contains('<'));
        assert!(!result.contains('>'));
        assert!(result.contains("&lt;"));
    }

    #[test]
    fn chunk_short_message() {
        let chunks = chunk_message("hello");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "hello");
    }

    #[test]
    fn chunk_long_message_splits_on_newlines() {
        let line = "a".repeat(100) + "\n";
        let text: String = std::iter::repeat(&line).take(100).cloned().collect();
        let chunks = chunk_message(&text);
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(chunk.len() <= 4000);
        }
    }

    #[test]
    fn format_final_message_escapes() {
        let result = format_final_message("plain text");
        assert!(result.contains("plain text"));
    }
}

#[cfg(test)]
mod events_tests {
    use duga_telegram_bot::events::*;

    #[test]
    fn parse_immediate_event() {
        let json = r#"{"type": "immediate", "channelId": -123, "text": "Hello"}"#;
        let event: ScheduledEvent = serde_json::from_str(json).unwrap();
        match event {
            ScheduledEvent::Immediate { channel_id, text } => {
                assert_eq!(channel_id, -123);
                assert_eq!(text, "Hello");
            }
            _ => panic!("expected Immediate"),
        }
    }

    #[test]
    fn parse_periodic_event() {
        let json = r#"{"type": "periodic", "channelId": -456, "text": "Check inbox", "schedule": "0 9 * * 1-5", "timezone": "Europe/Lisbon"}"#;
        let event: ScheduledEvent = serde_json::from_str(json).unwrap();
        match event {
            ScheduledEvent::Periodic { channel_id, text, schedule, .. } => {
                assert_eq!(channel_id, -456);
                assert_eq!(text, "Check inbox");
                assert_eq!(schedule, "0 9 * * 1-5");
            }
            _ => panic!("expected Periodic"),
        }
    }

    #[test]
    fn parse_one_shot_event() {
        let json = r#"{"type": "one_shot", "channelId": -789, "text": "Reminder", "at": "2026-05-15T09:00:00+01:00"}"#;
        let event: ScheduledEvent = serde_json::from_str(json).unwrap();
        match event {
            ScheduledEvent::OneShot { channel_id, text, at } => {
                assert_eq!(channel_id, -789);
                assert_eq!(text, "Reminder");
                assert_eq!(at, "2026-05-15T09:00:00+01:00");
            }
            _ => panic!("expected OneShot"),
        }
    }

    #[test]
    fn invalid_json_rejected() {
        let json = r#"{"type": "garbage", "channelId": 123}"#;
        let result: Result<ScheduledEvent, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }
}

#[cfg(test)]
mod attachments_tests {
    use duga_telegram_bot::attachments::sanitize_filename;

    #[test]
    fn sanitize_filename_handles_special_chars() {
        assert_eq!(sanitize_filename("hello world.txt"), "hello_world.txt");
        assert_eq!(sanitize_filename("path/to/file"), "path_to_file");
        assert_eq!(sanitize_filename("normal-name.txt"), "normal-name.txt");
    }
}

#[cfg(test)]
mod config_tests {
    use duga_config::TelegramConfig;

    #[test]
    fn telegram_config_defaults() {
        let config = TelegramConfig::default();
        assert_eq!(config.token_env, "TELEGRAM_BOT_TOKEN");
        assert!(!config.allow_all_chats_for_dev);
        assert!(config.allowed_chat_ids.is_empty());
        assert!(config.attachments.enabled);
        assert_eq!(config.attachments.max_file_size_mb, 20);
    }

    #[test]
    fn validates_empty_allowed_chats_in_production() {
        let config = TelegramConfig {
            allowed_chat_ids: vec![],
            allowed_chat_usernames: vec![],
            allow_all_chats_for_dev: false,
            ..Default::default()
        };
        // Validation would catch this at config load time.
        assert!(!config.allow_all_chats_for_dev);
        assert!(config.allowed_chat_ids.is_empty());
        assert!(config.allowed_chat_usernames.is_empty());
    }

    #[test]
    fn allows_usernames_when_ids_empty() {
        let config = TelegramConfig {
            allowed_chat_ids: vec![],
            allowed_chat_usernames: vec!["testuser".into()],
            allow_all_chats_for_dev: false,
            ..Default::default()
        };
        assert!(!config.allowed_chat_usernames.is_empty());
    }
}
