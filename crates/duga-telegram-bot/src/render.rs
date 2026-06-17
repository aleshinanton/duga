//! Telegram event renderer with live message editing.
//!
//! Consumes frontend events from the non-blocking bridge and renders
//! progress messages in Telegram. Telegram API calls happen entirely
//! in this worker task — never in the agent loop.
//!
//! Uses `sendRichMessage` (Bot API 10.1) for the final answer to enable
//! Rich Markdown formatting (tables, task lists, spoilers, etc.).

use crate::api_types::{send_rich_message, InputRichMessage};
use crate::formatting::{
    build_process_draft_html, chunk_message, escape_html, format_final_message,
    format_rich_markdown,
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
    /// Buffer for accumulating thinking/reasoning deltas (e.g. DeepSeek reasoning_content).
    thinking_buffer: String,
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
            thinking_buffer: String::new(),
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
                    tracing::info!(
                        "renderer received RunFinished, text_len={}",
                        text.as_ref().map(|t| t.len()).unwrap_or(0)
                    );
                    self.finalize_process_message(text).await;
                    self.finished = true;
                    return;
                }
                FrontendEvent::LoopDelegated { to, reason, .. } => {
                    let label = format!("🔀 delegated to *{}*: {}", to, reason);
                    self.action_labels.push(label);
                    let _ = self.edit_process_message().await;
                }
                FrontendEvent::ToolCallStarted {
                    tool_name,
                    tool_call_id,
                    attempt,
                    description,
                    ..
                } => {
                    // Skip think tool labels — thinking content goes to the final message.
                    if tool_name == "think" {
                        continue;
                    }
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
                    // Skip think tool labels — thinking content goes to the final message.
                    if tool_name == "think" {
                        continue;
                    }
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
                    self.delta_buffer.push_str(&delta);
                }
                FrontendEvent::LlmThinkingDelta { delta, .. } => {
                    self.thinking_buffer.push_str(&delta);
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

    /// Start a new process message.
    async fn start_process_message(&mut self, task: &str) -> Result<(), teloxide::RequestError> {
        let preview = if task.chars().count() > 80 {
            format!("{}…", task.chars().take(77).collect::<String>())
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

        let text = build_process_draft_html(
            &self.action_labels,
            self.error_message.as_deref(),
            None,
        );
        let chunks = chunk_message(&text);
        let use_html = self.action_labels.len() > 5;
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
    /// the final answer as a new rich message.
    async fn finalize_process_message(&mut self, final_text: Option<String>) {
        tracing::info!(
            "finalize_process_message called, final_text={}, thinking_len={}",
            final_text.as_ref().map(|t| t.len()).unwrap_or(0),
            self.thinking_buffer.len()
        );

        // ── 1. Edit the process message into a final step-history summary ──
        if let Some(msg_id) = self.process_message_id {
            let step_count = self.action_labels.len();
            let mut header = format!("✅ Completed in {step_count} step(s)\n");

            if let Some(ref err) = self.error_message {
                header.push_str(&format!("⚠️ Error: {err}\n"));
            }

            let labels_text = self.action_labels.join("\n");

            let use_collapse = self.action_labels.len() > 5;
            let body = if use_collapse {
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

        // ── 2. Build and send the final answer ─────────────────────────
        let final_text = build_final_text(final_text, &mut self.thinking_buffer);

        if let Some(text) = final_text {
            let use_collapse = text.len() > 300;
            if use_collapse {
                tracing::info!(
                    "sending final rich message (collapsed, {} chars, {} chunks)",
                    text.len(),
                    chunk_message(&text).len()
                );
                // For collapsed long messages, chunk raw text first, then
                // wrap each chunk in a blockquote.  We send each chunk as a
                // separate message (no inline expandable in Rich Markdown).
                let text_chunks = chunk_message(&text);
                for (i, chunk) in text_chunks.iter().enumerate() {
                    let rich_md = format_rich_markdown(chunk);
                    let collapsed = format!(
                        "<blockquote expandable>{}</blockquote>",
                        escape_html(&rich_md)
                    );

                    // Try sendRichMessage first (Bot API 10.1+).
                    // Fall back to send_message with Html parse mode.
                    let result = send_rich_message(
                        &self.bot,
                        teloxide::types::Recipient::Id(self.chat_id),
                        InputRichMessage::html(&collapsed),
                    )
                    .await;

                    if let Err(e) = result {
                        tracing::warn!(
                            "sendRichMessage failed for chunk {i}, falling back: {e}"
                        );
                        // Fallback: send as regular HTML message.
                        let _ = self
                            .bot
                            .send_message(self.chat_id, &collapsed)
                            .parse_mode(ParseMode::Html)
                            .await;
                    } else {
                        tracing::info!("final rich message chunk {i} sent");
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            } else {
                // Short message — send as a single rich message.
                let rich_md = format_rich_markdown(&text);
                tracing::info!(
                    "sending final rich message ({} chars)",
                    rich_md.len()
                );

                let result = send_rich_message(
                    &self.bot,
                    teloxide::types::Recipient::Id(self.chat_id),
                    InputRichMessage::markdown(&rich_md),
                )
                .await;

                match result {
                    Ok(_msg) => {
                        tracing::info!("final rich message sent");
                    }
                    Err(e) => {
                        tracing::warn!(
                            "sendRichMessage failed, falling back to legacy send_message: {e}"
                        );
                        // Fallback: send as regular HTML message.
                        let formatted = format_final_message(&text);
                        let chunks = chunk_message(&formatted);
                        for (i, chunk) in chunks.iter().enumerate() {
                            match self
                                .bot
                                .send_message(self.chat_id, chunk)
                                .parse_mode(ParseMode::Html)
                                .await
                            {
                                Ok(_) => tracing::info!("fallback chunk {i} sent"),
                                Err(e2) => {
                                    tracing::error!("fallback chunk {i} failed: {e2}");
                                    let _ = self.bot.send_message(self.chat_id, chunk).await;
                                }
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        }
                    }
                }
            }
        }
    }
}

/// Build the final answer text, prepending thinking when available.
fn build_final_text(
    final_text: Option<String>,
    thinking_buffer: &mut String,
) -> Option<String> {
    match (final_text, thinking_buffer.is_empty()) {
        (Some(text), false) => {
            let thinking = std::mem::take(thinking_buffer);
            Some(format!("💭 *Thinking*\n{thinking}\n\n{text}"))
        }
        (Some(text), true) => Some(text),
        (None, false) => {
            let thinking = std::mem::take(thinking_buffer);
            Some(format!("💭 *Thinking*\n{thinking}"))
        }
        (None, true) => None,
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
    use crate::formatting::build_process_draft_html;

    #[test]
    fn test_label_format_with_description() {
        // Meaningful description different from tool_name → shown.
        assert_eq!(
            format_step_label("shell", "ls -la", 1),
            "🔧 shell: ls -la"
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
        // Renderer collapses to just the tool name (no "shell: shell" noise).
        assert_eq!(format_step_label("shell", "shell", 1), "🔧 shell");
        assert_eq!(format_finish_label("shell", "shell", true), "✅ shell");
        assert_eq!(format_finish_label("shell", "shell", false), "❌ shell");
    }

    #[test]
    fn test_label_format_with_empty_description() {
        // Empty description: just the tool name.
        assert_eq!(format_step_label("shell", "", 1), "🔧 shell");
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
        labels.push(format_step_label("shell", "ls", 1));
        assert_eq!(labels[0], "🔧 shell: ls");

        if let Some(&idx) = index_map.get(id1) {
            labels[idx] = format_finish_label("shell", "ls", true);
        }
        assert_eq!(labels[0], "✅ shell: ls");
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

    // ── e2e: process message must not contain thinking or streaming delta ──

    #[test]
    fn process_text_excludes_thinking_and_delta() {
        let labels = vec!["✅ shell: ls".into(), "✅ read: config".into()];
        let text = build_process_draft_html(&labels, None, None);

        // Header should be present.
        assert!(text.contains("🔄 Processing… (2 steps)"));
        // Labels should be present.
        assert!(text.contains("✅ shell: ls"));
        assert!(text.contains("✅ read: config"));
        // Must NOT contain thinking content.
        assert!(!text.contains("💭"));
        assert!(!text.contains("Thinking"));
        assert!(!text.contains("reasoning"));
    }

    #[test]
    fn process_text_with_error() {
        let labels: Vec<String> = vec![];
        let text = build_process_draft_html(&labels, Some("LLM timeout"), None);
        assert!(text.contains("⚠️ Error: LLM timeout"));
        assert!(text.contains("🔄 Processing… (0 steps)"));
    }

    #[test]
    fn process_text_collapses_labels_over_five() {
        let labels: Vec<String> = (0..6).map(|i| format!("✅ tool_{i}")).collect();
        let text = build_process_draft_html(&labels, None, None);
        // Many labels should be collapsed.
        assert!(text.contains("<blockquote expandable>"));
        assert!(text.contains("✅ tool_0"));
        assert!(text.contains("✅ tool_5"));
        // No individual labels outside blockquote.
        let outside: String = text.split("<blockquote").next().unwrap().to_string();
        assert!(!outside.contains("✅ tool_"));
    }

    #[test]
    fn process_text_no_labels_under_six_not_collapsed() {
        let labels: Vec<String> = vec!["✅ shell".into(), "✅ read".into()];
        let text = build_process_draft_html(&labels, None, None);
        assert!(!text.contains("<blockquote"));
    }

    // ── e2e: final answer includes thinking, process message does not ──

    #[test]
    fn final_text_includes_thinking_when_present() {
        let mut thinking = "Let me analyze this step by step…".to_string();
        let result = build_final_text(Some("The answer is 42.".into()), &mut thinking);
        let text = result.unwrap();
        assert!(text.contains("💭 *Thinking*"));
        assert!(text.contains("Let me analyze this step by step…"));
        assert!(text.contains("The answer is 42."));
        // thinking_buffer consumed.
        assert!(thinking.is_empty());
    }

    #[test]
    fn final_text_thinking_only_when_answer_missing() {
        let mut thinking = "Reasoning without answer".to_string();
        let result = build_final_text(None, &mut thinking);
        let text = result.unwrap();
        assert!(text.contains("💭 *Thinking*"));
        assert!(text.contains("Reasoning without answer"));
        assert!(!text.contains("\n\n")); // no answer appended
        assert!(thinking.is_empty());
    }

    #[test]
    fn final_text_answer_only_when_no_thinking() {
        let mut thinking = String::new();
        let result = build_final_text(Some("Plain answer".into()), &mut thinking);
        assert_eq!(result.unwrap(), "Plain answer");
    }

    #[test]
    fn final_text_none_when_both_missing() {
        let mut thinking = String::new();
        let result = build_final_text(None, &mut thinking);
        assert!(result.is_none());
    }

    #[test]
    fn final_text_thinking_does_not_leak_into_process_text() {
        // The process text is built from action_labels only.
        // Even if thinking exists in the renderer state, build_process_draft_html
        // takes only labels and error_message — thinking cannot leak.
        let labels = vec!["✅ shell: ls".into()];
        let text = build_process_draft_html(&labels, None, None);
        assert!(!text.contains("💭"));
        assert!(!text.contains("Thinking"));
        assert!(!text.contains("think"));
    }

    // ── e2e: renderer event loop simulates full flow ──

    /// Simulates the full event sequence from agent start to finish
    /// and verifies that thinking/delta content is not in the process message.
    #[tokio::test]
    async fn e2e_thinking_only_in_final_not_process() {
        use duga_runtime::events::FrontendEventBridge;

        let bot = teloxide::Bot::new("dummy");
        let chat_id = teloxide::types::ChatId(0);
        let mut renderer = TelegramEventRenderer::new(bot, chat_id);

        let (tx, bridge) = FrontendEventBridge::new(32);
        let mut rx = bridge.into_inner();

        // Spawn renderer.
        let handle = tokio::spawn(async move { renderer.run(&mut rx).await });

        // Simulate RunStarted.
        tx.send(FrontendEvent::RunStarted {
            task: "test task".into(),
        })
        .await
        .unwrap();

        // Simulate LlmThinkingDelta (must NOT appear in process message).
        tx.send(FrontendEvent::LlmThinkingDelta {
            model: "test".into(),
            delta: "hidden reasoning".into(),
        })
        .await
        .unwrap();

        // Simulate think tool call (must be skipped).
        tx.send(FrontendEvent::ToolCallStarted {
            tool_name: "think".into(),
            tool_call_id: "think_1".into(),
            attempt: 1,
            description: "Planning".into(),
            raw_args: None,
        })
        .await
        .unwrap();

        tx.send(FrontendEvent::ToolCallFinished {
            tool_name: "think".into(),
            tool_call_id: "think_1".into(),
            success: true,
            attempt: 1,
            description: "Planning".into(),
            output: None,
        })
        .await
        .unwrap();

        // Simulate regular tool call.
        tx.send(FrontendEvent::ToolCallStarted {
            tool_name: "shell".into(),
            tool_call_id: "shell_1".into(),
            attempt: 1,
            description: "ls".into(),
            raw_args: None,
        })
        .await
        .unwrap();

        tx.send(FrontendEvent::ToolCallFinished {
            tool_name: "shell".into(),
            tool_call_id: "shell_1".into(),
            success: true,
            attempt: 1,
            description: "ls".into(),
            output: None,
        })
        .await
        .unwrap();

        // Simulate LlmTokenDelta (streaming text — must NOT appear).
        tx.send(FrontendEvent::LlmTokenDelta {
            model: "test".into(),
            delta: "streaming answer".into(),
        })
        .await
        .unwrap();

        // Simulate RunFinished with answer.
        tx.send(FrontendEvent::RunFinished {
            text: Some("final answer".into()),
        })
        .await
        .unwrap();

        // Drop sender to close channel.
        drop(tx);
        handle.await.unwrap();

        // Test passes if the renderer ran without panicking.
        // The actual message content is verified by the unit tests above.
    }
}
