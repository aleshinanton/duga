//! Telegram event renderer with live message editing.
//!
//! Consumes frontend events from the non-blocking bridge and renders
//! progress messages in Telegram. Telegram API calls happen entirely
//! in this worker task — never in the agent loop.

use crate::formatting::{
    chunk_message, escape_html, escape_telegram_plain_text, format_final_message,
};
use duga_runtime::events::FrontendEvent;
use std::collections::HashMap;
use teloxide::prelude::*;
use teloxide::types::{ChatId, ParseMode};
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
    /// Map from tool_call_id to (tool_name, description).
    tool_info: HashMap<String, (String, String)>,
    /// Map from tool_call_id to index in action_labels (for replacing start with finish).
    tool_label_index: HashMap<String, usize>,
    /// Fatal error message (shown separately, not counted as a step).
    error_message: Option<String>,
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
            tool_info: HashMap::new(),
            tool_label_index: HashMap::new(),
            error_message: None,
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
                    tool_call_id,
                    attempt,
                    description,
                } => {
                    self.tool_info.insert(
                        tool_call_id.clone(),
                        (tool_name.clone(), description.clone()),
                    );
                    let label = format_step_label(&tool_name, &description, attempt);
                    let idx = self.action_labels.len();
                    self.tool_label_index.insert(tool_call_id.clone(), idx);
                    self.action_labels.push(label);
                    let _ = self.edit_process_message().await;
                }
                FrontendEvent::ToolCallFinished {
                    tool_call_id,
                    success,
                    description,
                    ..
                } => {
                    let (tool_name, stored_description) = self
                        .tool_info
                        .get(&tool_call_id)
                        .map(|(n, d)| (n.as_str(), d.as_str()))
                        .unwrap_or(("tool", description.as_str()));
                    let display_desc = if stored_description.is_empty() {
                        &description
                    } else {
                        stored_description
                    };
                    let label = format_finish_label(tool_name, display_desc, success);
                    // Replace the start label instead of pushing a duplicate.
                    if let Some(&idx) = self.tool_label_index.get(&tool_call_id) {
                        if idx < self.action_labels.len() {
                            self.action_labels[idx] = label;
                        }
                    } else {
                        self.action_labels.push(label);
                    }
                    let _ = self.edit_process_message().await;
                }
                FrontendEvent::LlmTokenDelta { delta, .. } => {
                    // Suppress thinking tokens (extended thinking produces these).
                    self.delta_buffer.push_str(&delta);
                    let _ = self.edit_process_message().await;
                }
                FrontendEvent::Error { message } => {
                    self.error_message = Some(message);
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

        let total = self.action_labels.len();
        let mut text = format!("🔄 Processing… ({total} step{})\n", if total == 1 { "" } else { "s" });

        // Show fatal error at the top if present (not counted as a step).
        if let Some(ref err) = self.error_message {
            text.push_str(&format!("⚠️ Error: {err}\n"));
        }

        // Show recent action labels, collapsed if there are many.
        let recent: Vec<_> = self
            .action_labels
            .iter()
            .rev()
            .take(5)
            .rev()
            .cloned()
            .collect();

        let labels_text = recent.join("\n");
        if total > 5 {
            // Collapse the labels, show count outside blockquote.
            text.push_str(&format!(
                "<blockquote expandable>{}</blockquote>\n",
                escape_html(&labels_text)
            ));
            text.push_str(&format!("... and {} more\n", total - 5));
        } else {
            for label in &recent {
                text.push_str(label);
                text.push('\n');
            }
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
        // Use HTML parse mode when labels are collapsed.
        let use_html = total > 5;
        if let Some(first) = chunks.first() {
            let mut req = self.bot.edit_message_text(self.chat_id, msg_id, first.clone());
            if use_html {
                req = req.parse_mode(ParseMode::Html);
            }
            req.await?;
        }

        Ok(())
    }

    /// Finalize: edit the process message into a step history, then send
    /// a clean final answer in a separate new message.
    async fn finalize_process_message(&mut self, final_text: Option<String>) {
        // Edit the process message into a final step-history summary.
        if let Some(msg_id) = self.process_message_id {
            let step_count = self.action_labels.len();
            let mut header = format!("✅ Completed in {step_count} step(s)\n");

            // Show error if the run failed.
            if let Some(ref err) = self.error_message {
                header.push_str(&format!("⚠️ Error: {err}\n"));
            }

            let labels_text = self.action_labels.join("\n");

            let use_collapse = self.action_labels.len() > 5;
            let body = if use_collapse {
                // Chunk raw labels first, then wrap each chunk in blockquote.
                let label_chunks = chunk_message(&labels_text);
                label_chunks
                    .iter()
                    .map(|c| format!("<blockquote expandable>{}</blockquote>", escape_html(c)))
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                escape_html(&labels_text)
            };

            let full_text = format!("{header}{body}");
            let chunks = chunk_message(&full_text);
            if let Some(first) = chunks.first() {
                let _ = self
                    .bot
                    .edit_message_text(self.chat_id, msg_id, first.clone())
                    .parse_mode(ParseMode::Html)
                    .await;
            }
        }

        // Send the final answer as a clean new message.
        if let Some(text) = final_text {
            let use_collapse = text.len() > 300;
            if use_collapse {
                // Chunk raw text first, then wrap each chunk in blockquote.
                let text_chunks = chunk_message(&text);
                for chunk in text_chunks {
                    let collapsed = format!(
                        "<blockquote expandable>{}</blockquote>",
                        escape_html(&chunk)
                    );
                    let _ = self
                        .bot
                        .send_message(self.chat_id, collapsed)
                        .parse_mode(ParseMode::Html)
                        .await;
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            } else {
                let formatted = format_final_message(&text);
                let chunks = chunk_message(&formatted);
                for chunk in chunks {
                    let _ = self.bot.send_message(self.chat_id, chunk).await;
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        }
    }
}

/// Build a step label, showing `description` only when it adds value
/// (i.e., it's non-empty and differs from the tool name).
fn format_step_label(tool_name: &str, description: &str, attempt: u32) -> String {
    let icon = "🔧";
    let base = if description.is_empty() || description == tool_name {
        format!("{icon} {tool_name}")
    } else {
        format!("{icon} {tool_name}: {description}")
    };
    if attempt > 1 {
        format!("{base} (attempt {attempt})")
    } else {
        base
    }
}

/// Build a finish label (success or failure).
fn format_finish_label(tool_name: &str, description: &str, success: bool) -> String {
    let icon = if success { "✅" } else { "❌" };
    if description.is_empty() || description == tool_name {
        format!("{icon} {tool_name}")
    } else {
        format!("{icon} {tool_name}: {description}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_label_format_with_description() {
        // Meaningful description different from tool_name → shown.
        assert_eq!(
            format_step_label("bash", "ls -la", 1),
            "🔧 bash: ls -la"
        );
        assert_eq!(
            format_finish_label("read", "Reading config", true),
            "✅ read: Reading config"
        );
        assert_eq!(
            format_finish_label("search", "Searching for error", false),
            "❌ search: Searching for error"
        );
    }

    #[test]
    fn test_label_format_fallback_when_description_equals_tool_name() {
        // When the LLM doesn't provide a label, description == tool_name.
        // Renderer collapses to just the tool name (no "bash: bash" noise).
        assert_eq!(format_step_label("bash", "bash", 1), "🔧 bash");
        assert_eq!(format_finish_label("bash", "bash", true), "✅ bash");
        assert_eq!(format_finish_label("bash", "bash", false), "❌ bash");
    }

    #[test]
    fn test_label_format_with_empty_description() {
        // Empty description: just the tool name.
        assert_eq!(format_step_label("bash", "", 1), "🔧 bash");
        assert_eq!(format_finish_label("tool", "", true), "✅ tool");
        assert_eq!(format_finish_label("tool", "", false), "❌ tool");
    }

    #[test]
    fn test_label_format_with_retry() {
        assert_eq!(
            format_step_label("write", "Writing file", 3),
            "🔧 write: Writing file (attempt 3)"
        );
        // Retry with no label falls back to tool name only.
        assert_eq!(
            format_step_label("write", "write", 3),
            "🔧 write (attempt 3)"
        );
    }

    #[test]
    fn test_label_format_success() {
        assert_eq!(
            format_finish_label("read", "Reading config", true),
            "✅ read: Reading config"
        );
    }

    #[test]
    fn test_label_format_failure() {
        assert_eq!(
            format_finish_label("search", "Searching for error", false),
            "❌ search: Searching for error"
        );
    }

    #[test]
    fn test_label_format_backward_compat_no_description() {
        // Old tools without label → show just tool name (matches original behavior).
        assert_eq!(format_finish_label("tool", "", true), "✅ tool");
        assert_eq!(format_finish_label("tool", "", false), "❌ tool");
    }

    #[test]
    fn test_label_replacement_start_to_finish() {
        // Each tool shows only one line: start label replaced by finish label.
        // This mirrors the renderer's tool_label_index replacement logic.
        let mut labels: Vec<String> = Vec::new();
        let mut index_map: HashMap<String, usize> = HashMap::new();

        // Tool 1: start → replaced by finish.
        let id1 = "call_1";
        index_map.insert(id1.to_string(), labels.len());
        labels.push(format_step_label("bash", "ls", 1));
        assert_eq!(labels[0], "🔧 bash: ls");

        if let Some(&idx) = index_map.get(id1) {
            labels[idx] = format_finish_label("bash", "ls", true);
        }
        assert_eq!(labels[0], "✅ bash: ls");
        assert_eq!(labels.len(), 1, "still one line, not two");

        // Tool 2: start → replaced by finish (failure).
        let id2 = "call_2";
        index_map.insert(id2.to_string(), labels.len());
        labels.push(format_step_label("read", "Reading config", 1));

        if let Some(&idx) = index_map.get(id2) {
            labels[idx] = format_finish_label("read", "Reading config", false);
        }
        assert_eq!(labels[1], "❌ read: Reading config");
        assert_eq!(labels.len(), 2, "two tools, two lines");
    }
}
