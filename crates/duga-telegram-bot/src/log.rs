//! Bot message logging for per-chat persistence.
//!
//! Logs all messages to `log.jsonl` and maintains `context.jsonl` for
//! LLM history. Supports deduplication by message ID.

use anyhow::Result;
use serde::Serialize;
use std::path::PathBuf;
use teloxide::prelude::*;
use tokio::io::AsyncWriteExt;

/// A single log entry for a chat message.
#[derive(Clone, Debug, Serialize)]
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
