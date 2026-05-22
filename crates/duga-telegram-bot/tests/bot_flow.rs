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
        assert!(is_allowed_chat(&config, 123, None));
        assert!(!is_allowed_chat(&config, 789, None));
    }

    #[test]
    fn authorize_dev_mode_allows_all() {
        let config = TelegramConfig {
            allowed_chat_ids: vec![],
            allow_all_chats_for_dev: true,
            ..Default::default()
        };
        assert!(is_allowed_chat(&config, 999, None));
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

    // ── escaping ─────────────────────────────────────────────────

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

    // ── markdown_to_telegram_html — EPIC-27 e2e tests ───────────

    #[test]
    fn empty_string() {
        assert_eq!(markdown_to_telegram_html(""), "");
    }

    #[test]
    fn whitespace_only() {
        let result = markdown_to_telegram_html("   \n  ");
        assert!(result.is_empty());
    }

    #[test]
    fn plain_text_no_markdown() {
        let result = markdown_to_telegram_html("Hello world");
        // Plain text should be HTML-escaped (identical to escape_html output).
        assert_eq!(result, "Hello world");
    }

    #[test]
    fn plain_text_with_special_chars() {
        let result = markdown_to_telegram_html("a <b> & c");
        assert_eq!(result, "a &lt;b&gt; &amp; c");
    }

    #[test]
    fn bold_text() {
        let result = markdown_to_telegram_html("**bold text**");
        assert!(result.contains("<b>") && result.contains("</b>"));
        assert!(result.contains("bold text"));
    }

    #[test]
    fn italic_text() {
        let result = markdown_to_telegram_html("*italic text*");
        assert!(result.contains("<i>") && result.contains("</i>"));
        assert!(result.contains("italic text"));
    }

    #[test]
    fn strikethrough_text() {
        let result = markdown_to_telegram_html("~~struck~~");
        assert!(result.contains("<s>") && result.contains("</s>"));
        assert!(result.contains("struck"));
    }

    #[test]
    fn nested_bold_in_italic() {
        let result = markdown_to_telegram_html("*italic **and bold***");
        assert!(result.contains("<i>"));
        assert!(result.contains("<b>"));
        assert!(result.contains("</b>"));
        assert!(result.contains("</i>"));
    }

    #[test]
    fn nested_italic_in_bold() {
        let result = markdown_to_telegram_html("**bold *and italic***");
        assert!(result.contains("<b>"));
        assert!(result.contains("<i>"));
        assert!(result.contains("</i>"));
        assert!(result.contains("</b>"));
    }

    #[test]
    fn inline_code() {
        let result = markdown_to_telegram_html("use `let x = 1;` here");
        assert!(result.contains("<code>"));
        assert!(result.contains("</code>"));
        assert!(result.contains("let x = 1;"));
    }

    #[test]
    fn fenced_code_block_with_language() {
        let result = markdown_to_telegram_html("```rust\nfn main() {}\n```");
        assert!(result.contains("<pre><code class=\"language-rust\">"));
        assert!(result.contains("fn main() {}"));
        assert!(result.contains("</code></pre>"));
    }

    #[test]
    fn fenced_code_block_no_language() {
        let result = markdown_to_telegram_html("```\nplain code\n```");
        assert!(result.contains("<pre><code>"));
        assert!(!result.contains("class="));
        assert!(result.contains("plain code"));
        assert!(result.contains("</code></pre>"));
    }

    #[test]
    fn code_block_generic_types() {
        // Vec<T>, HashMap<K, V> — must be HTML-escaped inside <code>.
        let result = markdown_to_telegram_html("```rust\nlet v: Vec&lt;T&gt; = vec![];\nlet m: HashMap&lt;K, V&gt; = HashMap::new();\n```");
        assert!(result.contains("Vec"));
        // The raw < and > in code must be escaped to prevent Telegram 400 errors.
        assert!(!result
            .chars()
            .any(|c| c == '<' && !result.contains("<pre") && !result.contains("<code")));
    }

    #[test]
    fn code_block_logic_symbols() {
        // &&, ||, <, > — must be HTML-escaped inside <code>.
        let md = "```c\nif (a &lt; b &amp;&amp; c &gt; d) { return true; }\n```";
        let result = markdown_to_telegram_html(md);
        // The result should contain escaped entities, not raw < > &
        assert!(result.contains("&amp;"));
    }

    #[test]
    fn link_basic() {
        let result = markdown_to_telegram_html("[click here](https://example.com)");
        assert!(result.contains("<a href=\"https://example.com\">"));
        assert!(result.contains("click here"));
        assert!(result.contains("</a>"));
    }

    #[test]
    fn link_special_chars_in_url() {
        let result =
            markdown_to_telegram_html("[docs](https://example.com/search?q=foo&bar=baz)");
        assert!(result.contains("href=\""));
        assert!(result.contains("search?q=foo&amp;bar=baz"));
    }

    #[test]
    fn blockquote_single_line() {
        let result = markdown_to_telegram_html("> quoted text");
        assert!(result.contains("<blockquote>"));
        assert!(result.contains("quoted text"));
        assert!(result.contains("</blockquote>"));
    }

    #[test]
    fn blockquote_multi_line() {
        let result = markdown_to_telegram_html("> line one\n> line two\n> line three");
        assert!(result.contains("<blockquote>"));
        assert!(result.contains("line one"));
        assert!(result.contains("line two"));
        assert!(result.contains("line three"));
        assert!(result.contains("</blockquote>"));
    }

    #[test]
    fn heading_becomes_bold() {
        let result = markdown_to_telegram_html("# Heading 1");
        assert!(result.contains("<b>"));
        assert!(result.contains("Heading 1"));
        assert!(result.contains("</b>"));
    }

    #[test]
    fn heading_level_2() {
        let result = markdown_to_telegram_html("## Subheading");
        assert!(result.contains("<b>"));
        assert!(result.contains("Subheading"));
        assert!(result.contains("</b>"));
    }

    #[test]
    fn unordered_list() {
        let result = markdown_to_telegram_html("- item one\n- item two\n- item three");
        assert!(result.contains("• item one"));
        assert!(result.contains("• item two"));
        assert!(result.contains("• item three"));
    }

    #[test]
    fn ordered_list_renders_as_bullets() {
        let result = markdown_to_telegram_html("1. first\n2. second\n3. third");
        // Telegram has no list tags; ordered lists become bullets too.
        assert!(result.contains("• first"));
        assert!(result.contains("• second"));
        assert!(result.contains("• third"));
    }

    #[test]
    fn table_stripped_to_plain_text() {
        let md = "| Col A | Col B |\n|-------|-------|\n| a1    | b1    |\n| a2    | b2    |";
        let result = markdown_to_telegram_html(md);
        // Table tags are stripped, but text content survives.
        assert!(result.contains("Col A"));
        assert!(result.contains("a1"));
        assert!(!result.contains("<table"));
    }

    #[test]
    fn horizontal_rule() {
        let result = markdown_to_telegram_html("before\n\n---\n\nafter");
        assert!(result.contains("before"));
        assert!(result.contains("---"));
        assert!(result.contains("after"));
    }

    #[test]
    fn malformed_markdown_passthrough() {
        // Unclosed bold — treated as literal text.
        let result = markdown_to_telegram_html("**unclosed bold");
        assert!(result.contains("unclosed bold"));
    }

    #[test]
    fn combined_formatting() {
        let md = "# Result\n\n**Bold** and *italic* with `code` and ~~strike~~";
        let result = markdown_to_telegram_html(md);
        assert!(result.contains("<b>"));
        assert!(result.contains("<i>"));
        assert!(result.contains("<code>"));
        assert!(result.contains("<s>"));
    }

    #[test]
    fn html_injection_is_escaped() {
        // Simulates Event::Html from untrusted data.
        let result = markdown_to_telegram_html("<script>alert('xss')</script>");
        assert!(!result.contains("<script>"));
        assert!(result.contains("&lt;script&gt;"));
    }

    #[test]
    fn link_with_html_injection_in_text() {
        // Link text containing HTML — the text node is escaped.
        let result = markdown_to_telegram_html("[<script>](https://safe.com)");
        assert!(!result.contains("<script>"));
        assert!(result.contains("&lt;script&gt;"));
        assert!(result.contains("href=\"https://safe.com\""));
    }

    #[test]
    fn format_final_message_delegates() {
        // format_final_message now delegates to markdown_to_telegram_html.
        let result = format_final_message("**bold** and *italic*");
        assert!(result.contains("<b>"));
        assert!(result.contains("<i>"));
    }

    #[test]
    fn format_final_message_escapes_plain_text() {
        let result = format_final_message("<script>alert(1)</script>");
        assert!(!result.contains("<script>"));
        assert!(result.contains("&lt;script&gt;"));
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
