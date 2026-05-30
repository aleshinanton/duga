//! Shared conversation history loading for session persistence.
//!
//! Used by both the Telegram bot and the TUI to restore prior conversation
//! context from a JSONL session file.

use duga_events::{Event, StoredEvent};
use duga_types::message::{ContentBlock, Message, Role};
use std::path::Path;

/// Load the last conversation from a JSONL session file.
///
/// Reads all `LlmRequest` events, takes the last one (the most recent
/// full conversation snapshot), strips system messages, applies the
/// sliding window and token budget, and normalises orphaned tool messages.
///
/// Returns `None` if the file doesn't exist or contains no usable history.
pub fn load_conversation_history(
    path: &Path,
    context_window_size: usize,
    max_context_tokens: usize,
) -> Option<Vec<Message>> {
    use std::io::{BufRead, BufReader};

    let file = std::fs::File::open(path).ok()?;
    let reader = BufReader::new(file);

    let mut last_messages: Option<Vec<Message>> = None;
    for line in reader.lines() {
        let line = line.ok()?;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Ok(event) = serde_json::from_str::<StoredEvent>(trimmed) {
            if let Event::LlmRequest { messages, .. } = event.event {
                last_messages = Some(messages);
            }
        }
    }

    let messages = last_messages?;
    if messages.is_empty() {
        return None;
    }

    // Strip system messages — fresh system prompt is always provided.
    let mut history: Vec<Message> = messages
        .into_iter()
        .filter(|m| !matches!(m.role, Role::System))
        .collect();

    let total_in_history = history.len();

    if context_window_size > 0 && history.len() > context_window_size {
        let start = history.len() - context_window_size;
        history = history.split_off(start);
    }

    if max_context_tokens > 0 {
        while crate::memory::estimate_tokens(&history) > max_context_tokens
            && history.len() > 1
        {
            history.remove(0);
        }
    }

    let history = normalize_tool_message_sequence(history);
    let history = strip_stale_reminders(history);

    if history.is_empty() {
        None
    } else {
        tracing::info!(
            loaded = history.len(),
            total = total_in_history,
            "Loaded {} of {} messages from session history",
            history.len(),
            total_in_history
        );
        Some(history)
    }
}

/// Remove orphaned tool messages that lack a preceding assistant with tool_calls.
///
/// Can happen after a compression split in a previous run. Without this,
/// the LLM provider returns HTTP 400 ("tool_result without tool_use").
pub fn normalize_tool_message_sequence(messages: Vec<Message>) -> Vec<Message> {
    let mut out = Vec::with_capacity(messages.len());
    let mut last_assistant_had_tool_calls = false;
    for msg in messages {
        if msg.role == Role::Tool {
            if last_assistant_had_tool_calls {
                out.push(msg);
            }
            // else: orphaned — drop silently.
            // Do NOT reset the flag; multiple results can follow one assistant.
        } else {
            let has_tool_calls = msg.role == Role::Assistant
                && msg
                    .content
                    .iter()
                    .any(|block| matches!(block, ContentBlock::ToolCall(_)));
            last_assistant_had_tool_calls = has_tool_calls;
            out.push(msg);
        }
    }
    out
}

/// Strip stale "Reminder: Focus exclusively on the current task: …" suffixes
/// from historical user messages.  These were injected by a prior run and
/// conflict with the new run's task anchor.
fn strip_stale_reminders(messages: Vec<Message>) -> Vec<Message> {
    messages
        .into_iter()
        .map(|mut msg| {
            if msg.role == Role::User {
                msg.content = msg
                    .content
                    .into_iter()
                    .map(|block| match block {
                        ContentBlock::Text { text } => {
                            let cleaned = strip_reminder_suffix(&text);
                            ContentBlock::Text { text: cleaned }
                        }
                        other => other,
                    })
                    .collect();
            }
            msg
        })
        .collect()
}

/// Remove the "Reminder: Focus exclusively on the current task: …" suffix
/// that simple_react appends to every user message.
fn strip_reminder_suffix(text: &str) -> String {
    let marker = "\n\nReminder: Focus exclusively on the current task:";
    if let Some(pos) = text.rfind(marker) {
        text[..pos].to_string()
    } else {
        text.to_string()
    }
}
