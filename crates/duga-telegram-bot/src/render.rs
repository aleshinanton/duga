//! Telegram event renderer with live message editing.
//!
//! Consumes frontend events from the non-blocking bridge and renders
//! progress messages in Telegram. Telegram API calls happen entirely
//! in this worker task — never in the agent loop.

use crate::formatting::{chunk_message, escape_telegram_plain_text, format_final_message};
use duga_runtime::events::FrontendEvent;
use teloxide::prelude::*;
use teloxide::types::ChatId;
use tokio::sync::mpsc;

/// Renders agent progress as live Telegram messages.
pub struct TelegramEventRenderer {
    bot: Bot,
    chat_id: ChatId,
    /// The ID of the currently-edited process message.
    process_message_id: Option<teloxide::types::MessageId>,
    /// Buffer for accumulating streaming token deltas.
    delta_buffer: String,
    /// Buffer for tool action labels.
    action_labels: Vec<String>,
    /// Whether the run has completed.
    finished: bool,
}

impl TelegramEventRenderer {
    pub fn new(bot: Bot, chat_id: ChatId) -> Self {
        Self {
            bot,
            chat_id,
            process_message_id: None,
            delta_buffer: String::new(),
            action_labels: Vec::new(),
            finished: false,
        }
    }

    /// Run the renderer event loop.
    pub async fn run(&mut self, rx: &mut mpsc::Receiver<FrontendEvent>) {
        while let Some(event) = rx.recv().await {
            if self.finished {
                break;
            }

            match event {
                FrontendEvent::RunStarted { task } => {
                    let _ = self.start_process_message(&task).await;
                }
                FrontendEvent::RunFinished { text } => {
                    self.finalize_process_message(text).await;
                    self.finished = true;
                    return;
                }
                FrontendEvent::ToolCallStarted {
                    tool_name,
                    tool_call_id: _,
                    attempt,
                } => {
                    let label = if attempt > 1 {
                        format!("🔧 {tool_name} (attempt {attempt})")
                    } else {
                        format!("🔧 {tool_name}")
                    };
                    self.action_labels.push(label);
                    let _ = self.edit_process_message().await;
                }
                FrontendEvent::ToolCallFinished {
                    tool_name,
                    success,
                    ..
                } => {
                    let label = if success {
                        format!("✅ {tool_name}")
                    } else {
                        format!("❌ {tool_name}")
                    };
                    self.action_labels.push(label);
                    let _ = self.edit_process_message().await;
                }
                FrontendEvent::LlmTokenDelta { delta, .. } => {
                    // Suppress thinking tokens (extended thinking produces these).
                    self.delta_buffer.push_str(&delta);
                    let _ = self.edit_process_message().await;
                }
                FrontendEvent::Error { message } => {
                    self.action_labels.push(format!("⚠️ Error: {message}"));
                    let _ = self.edit_process_message().await;
                }
                FrontendEvent::MemoryCompressed {
                    before_tokens,
                    after_tokens,
                } => {
                    self.action_labels
                        .push(format!("💾 Memory: {before_tokens} → {after_tokens} tokens"));
                    let _ = self.edit_process_message().await;
                }
            }
        }
    }

    /// Start a new process message (or edit existing).
    async fn start_process_message(&mut self, task: &str) -> Result<(), teloxide::RequestError> {
        let preview = if task.len() > 80 {
            format!("{}…", &task[..77])
        } else {
            task.to_string()
        };

        let msg = self
            .bot
            .send_message(self.chat_id, format!("🔄 Processing: {preview}"))
            .await?;

        self.process_message_id = Some(msg.id);
        Ok(())
    }

    /// Edit the process message with current state.
    async fn edit_process_message(&mut self) -> Result<(), teloxide::RequestError> {
        let msg_id = match self.process_message_id {
            Some(id) => id,
            None => return Ok(()),
        };

        let mut text = String::from("🔄 Processing…\n");

        // Show recent action labels (max 5).
        let recent: Vec<_> = self
            .action_labels
            .iter()
            .rev()
            .take(5)
            .rev()
            .cloned()
            .collect();

        for label in &recent {
            text.push_str(label);
            text.push('\n');
        }

        // Show streaming delta if present.
        if !self.delta_buffer.is_empty() {
            let escaped = escape_telegram_plain_text(&self.delta_buffer);
            let truncated: String = if escaped.len() > 200 {
                escaped.chars().take(197).collect::<String>() + "…"
            } else {
                escaped
            };
            text.push_str("\n```\n");
            text.push_str(&truncated);
            text.push_str("\n```");
        }

        let chunks = chunk_message(&text);
        // Send the first chunk as an edit.
        if let Some(first) = chunks.first() {
            self.bot
                .edit_message_text(self.chat_id, msg_id, first)
                .await?;
        }

        Ok(())
    }

    /// Finalize the process message and send the final answer.
    async fn finalize_process_message(&mut self, final_text: Option<String>) {
        // Delete or update the process message.
        if let Some(msg_id) = self.process_message_id {
            let _ = self
                .bot
                .delete_message(self.chat_id, msg_id)
                .await;
        }

        // Send the final answer.
        if let Some(text) = final_text {
            let formatted = format_final_message(&text);
            let chunks = chunk_message(&formatted);
            for chunk in chunks {
                let _ = self.bot.send_message(self.chat_id, chunk).await;
                // Small delay between chunks to avoid rate limiting.
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}
