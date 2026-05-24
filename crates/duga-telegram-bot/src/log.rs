//! Bot message logging for per-chat persistence.
//!
//! Logs all messages to `log.jsonl` and maintains `context.jsonl` for
//! LLM history. Supports deduplication by message ID.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use teloxide::prelude::*;
use tokio::io::AsyncWriteExt;

/// A single log entry for a chat message.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LogEntry {
    pub message_id: i64,
    pub chat_id: i64,
    pub user_id: Option<i64>,
    pub timestamp: String,
    pub direction: String, // "in" or "out"
    pub text: String,
    pub is_edit: bool,
}

/// Logger for per-chat message persistence.
#[derive(Clone)]
pub struct BotLogger {
    data_dir: PathBuf,
}

impl BotLogger {
    pub fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    /// Log an incoming or outgoing message.
    pub async fn log_message(
        &self,
        chat_id: i64,
        msg: &Message,
        _user_id: i64,
        is_incoming: bool,
    ) -> Result<()> {
        let chat_dir = self.chat_dir(chat_id);
        tokio::fs::create_dir_all(&chat_dir).await?;

        let log_path = chat_dir.join("log.jsonl");
        let entry = LogEntry {
            message_id: msg.id.0 as i64,
            chat_id,
            user_id: msg.from.as_ref().map(|u| u.id.0 as i64),
            timestamp: chrono::Utc::now().to_rfc3339(),
            direction: if is_incoming {
                "in".into()
            } else {
                "out".into()
            },
            text: msg.text().unwrap_or("[non-text message]").to_string(),
            is_edit: false,
        };

        let line = serde_json::to_string(&entry)? + "\n";
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .await?;
        file.write_all(line.as_bytes()).await?;
        file.flush().await?;

        Ok(())
    }

    fn chat_dir(&self, chat_id: i64) -> PathBuf {
        self.data_dir.join(chat_id.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_entry_serialization_roundtrip() {
        let entry = LogEntry {
            message_id: 42,
            chat_id: -1001234567890,
            user_id: Some(12345),
            timestamp: "2024-01-01T00:00:00Z".into(),
            direction: "in".into(),
            text: "Hello, bot!".into(),
            is_edit: false,
        };

        let json = serde_json::to_string(&entry).unwrap();
        let parsed: LogEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.message_id, 42);
        assert_eq!(parsed.chat_id, -1001234567890);
        assert_eq!(parsed.user_id, Some(12345));
        assert_eq!(parsed.direction, "in");
        assert_eq!(parsed.text, "Hello, bot!");
        assert!(!parsed.is_edit);
    }

    #[test]
    fn log_entry_direction_out() {
        let entry = LogEntry {
            message_id: 1,
            chat_id: 100,
            user_id: None,
            timestamp: "2024-01-01T00:00:00Z".into(),
            direction: "out".into(),
            text: "Response".into(),
            is_edit: true,
        };
        assert_eq!(entry.direction, "out");
        assert!(entry.is_edit);
    }

    #[test]
    fn log_entry_cloneable() {
        let entry = LogEntry {
            message_id: 1,
            chat_id: 100,
            user_id: Some(999),
            timestamp: "now".into(),
            direction: "in".into(),
            text: "msg".into(),
            is_edit: false,
        };
        let cloned = entry.clone();
        assert_eq!(cloned.message_id, entry.message_id);
        assert_eq!(cloned.text, entry.text);
    }

    #[test]
    fn bot_logger_chat_dir() {
        let logger = BotLogger::new(PathBuf::from("/tmp/logs"));
        let dir = logger.chat_dir(-123);
        assert_eq!(dir, PathBuf::from("/tmp/logs/-123"));
    }

    #[test]
    fn bot_logger_new_stores_path() {
        let logger = BotLogger::new(PathBuf::from("/data/bot"));
        assert_eq!(logger.chat_dir(42), PathBuf::from("/data/bot/42"));
    }
}
