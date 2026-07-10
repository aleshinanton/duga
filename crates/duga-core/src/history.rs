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

    // Strip system messages — fresh system prompt is always provided.  Keep
    // compressed summaries because they are generated conversation context.
    let mut history: Vec<Message> = messages
        .into_iter()
        .filter(|m| !matches!(m.role, Role::System) || is_compressed_summary_message(m))
        .collect();

    let total_in_history = history.len();

    if context_window_size > 0 && history.len() > context_window_size {
        let start = history.len() - context_window_size;
        history = history.split_off(start);
    }

    if max_context_tokens > 0 {
        while crate::memory::estimate_tokens(&history) > max_context_tokens && history.len() > 1 {
            history.remove(0);
        }
    }

    let history = repair_reasoning_history(history);
    let history = normalize_tool_message_sequence(history);
    let history = strip_stale_reminders(history);
    let history = filter_empty_assistant_messages(history);

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

fn repair_reasoning_history(messages: Vec<Message>) -> Vec<Message> {
    if !messages
        .iter()
        .any(|message| message.role == Role::Assistant && assistant_has_reasoning(message))
    {
        return messages;
    }

    let mut out = Vec::with_capacity(messages.len());
    let mut skip_tool_results = false;
    for msg in messages {
        if skip_tool_results {
            if msg.role == Role::Tool {
                continue;
            }
            skip_tool_results = false;
        }

        if msg.role != Role::Assistant || assistant_has_reasoning(&msg) {
            out.push(msg);
            continue;
        }

        if assistant_has_tool_calls(&msg) {
            skip_tool_results = true;
            continue;
        }

        let text = message_text(&msg);
        if !text.is_empty() {
            let mut context = Message::system(format!("Historical assistant response:\n{text}"));
            context.pinned = msg.pinned;
            out.push(context);
        }
    }

    out
}

fn assistant_has_reasoning(message: &Message) -> bool {
    message
        .reasoning_content
        .as_deref()
        .is_some_and(|reasoning| !reasoning.is_empty())
}

fn assistant_has_tool_calls(message: &Message) -> bool {
    message
        .content
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolCall(_)))
}

fn message_text(message: &Message) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            ContentBlock::ToolCall(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_compressed_summary_message(message: &Message) -> bool {
    if message.role != Role::System {
        return false;
    }

    let text = message_text(message);
    text.starts_with("Summary:\n") || text.contains("\n\nSummary:\n")
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

/// Remove assistant messages that have no text and no tool calls.
///
/// OpenAI-compatible APIs (DeepSeek, etc.) reject these with HTTP 400
/// ("Invalid assistant message: content or tool_calls must be set").
/// These empty messages can appear in history after a run where the
/// LLM returned reasoning content but no final answer text.
fn filter_empty_assistant_messages(messages: Vec<Message>) -> Vec<Message> {
    messages
        .into_iter()
        .filter(|m| {
            if m.role != Role::Assistant {
                return true;
            }
            // Keep the message if it has text content or tool calls.
            m.content.iter().any(|block| match block {
                ContentBlock::Text { text } => !text.is_empty(),
                ContentBlock::ToolCall(_) => true,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use duga_events::{Event, StoredEvent};
    use duga_types::tool_call::ToolCall;
    use duga_types::tool_schema::ToolSchema;

    fn write_llm_request(messages: Vec<Message>) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::new().unwrap();
        let stored = StoredEvent::new(
            1,
            Event::LlmRequest {
                model: "deepseek-reasoner".into(),
                messages,
                tools: Vec::<ToolSchema>::new(),
            },
        );
        std::fs::write(
            file.path(),
            format!("{}\n", serde_json::to_string(&stored).unwrap()),
        )
        .unwrap();
        file
    }

    #[test]
    fn normalizes_orphan_tool_messages() {
        let tool_call = ToolCall::new("read", serde_json::json!({"path": "file.txt"}));
        let tool_id = tool_call.id.clone();
        let messages = vec![
            Message::tool(tool_id.as_uuid(), "orphan".into()),
            Message::assistant(None, vec![tool_call], None),
            Message::tool(tool_id.as_uuid(), "kept".into()),
        ];

        let normalized = normalize_tool_message_sequence(messages);

        assert_eq!(normalized.len(), 2);
        assert_eq!(normalized[0].role, Role::Assistant);
        assert_eq!(normalized[1].role, Role::Tool);
    }

    #[test]
    fn preserves_non_reasoning_assistant_history() {
        let tool_call = ToolCall::new("read", serde_json::json!({"path": "file.txt"}));
        let tool_id = tool_call.id.clone();
        let messages = vec![
            Message::user("read file.txt"),
            Message::assistant(None, vec![tool_call], None),
            Message::tool(tool_id.as_uuid(), "contents".into()),
            Message::assistant(Some("done".into()), vec![], None),
        ];

        let repaired = repair_reasoning_history(messages);

        assert_eq!(repaired.len(), 4);
        assert_eq!(repaired[1].role, Role::Assistant);
        assert_eq!(repaired[2].role, Role::Tool);
        assert_eq!(repaired[3].role, Role::Assistant);
    }

    #[test]
    fn repairs_mixed_reasoning_history_without_orphan_tools() {
        let stale_tool_call = ToolCall::new("read", serde_json::json!({"path": "old.txt"}));
        let stale_tool_id = stale_tool_call.id.clone();
        let messages = vec![
            Message::user("old task"),
            Message::assistant(
                Some("kept response".into()),
                vec![],
                Some("kept reasoning".into()),
            ),
            Message::assistant(Some("synthetic assistant".into()), vec![], None),
            Message::assistant(Some("stale tool text".into()), vec![stale_tool_call], None),
            Message::tool(stale_tool_id.as_uuid(), "stale output".into()),
            Message::user("next task"),
        ];

        let repaired = normalize_tool_message_sequence(repair_reasoning_history(messages));

        assert_eq!(
            repaired
                .iter()
                .map(|message| message.role.clone())
                .collect::<Vec<_>>(),
            vec![Role::User, Role::Assistant, Role::System, Role::User,]
        );
        assert_eq!(
            repaired[1].reasoning_content.as_deref(),
            Some("kept reasoning")
        );
        assert!(message_text(&repaired[2]).contains("synthetic assistant"));
        assert!(repaired.iter().all(|message| message.role != Role::Tool));
    }

    #[test]
    fn load_history_keeps_compressed_summary_but_strips_system_prompt() {
        let file = write_llm_request(vec![
            Message::system("fresh system prompt from old run"),
            Message::system("Key facts:\n- pinned\n\nSummary:\nold context"),
            Message::user("next task"),
        ]);

        let history = load_conversation_history(file.path(), 0, 0).unwrap();

        assert_eq!(history.len(), 2);
        assert_eq!(history[0].role, Role::System);
        assert!(message_text(&history[0]).contains("Summary:\nold context"));
        assert_eq!(history[1].role, Role::User);
    }

    #[test]
    fn filters_empty_assistant_messages_from_history() {
        #[allow(deprecated)]
        let tool_call = ToolCall::new("read", serde_json::json!({"path": "file.txt"}));
        let tool_id = tool_call.id.clone();
        let messages = vec![
            Message::user("hello"),
            // Empty assistant: no text, no tool calls — should be removed.
            Message::assistant(None, vec![], None),
            Message::assistant(Some("valid".into()), vec![], None),
            // Assistant with tool calls is valid.
            Message::assistant(None, vec![tool_call], None),
            Message::tool(tool_id.as_uuid(), "output".into()),
            // Another empty assistant — should be removed.
            Message::assistant(Some("".into()), vec![], None),
            Message::user("next"),
        ];

        let filtered = filter_empty_assistant_messages(messages);

        assert_eq!(filtered.len(), 5);
        assert_eq!(filtered[0].role, Role::User);
        assert_eq!(filtered[1].role, Role::Assistant);
        assert_eq!(filtered[1].content[0], ContentBlock::Text { text: "valid".into() });
        assert_eq!(filtered[2].role, Role::Assistant); // has tool call
        assert_eq!(filtered[3].role, Role::Tool);
        assert_eq!(filtered[4].role, Role::User);
    }

    #[test]
    fn filter_empty_assistant_preserves_all_non_assistant() {
        let tool_id = duga_types::tool_call::CallId::new();
        let messages = vec![
            Message::user("hi"),
            Message::assistant(None, vec![], None), // empty, removed
            Message::system("system note"),
            Message::tool(tool_id.as_uuid(), "tool output".into()),
        ];

        let filtered = filter_empty_assistant_messages(messages);

        assert_eq!(filtered.len(), 3);
        assert_eq!(filtered[0].role, Role::User);
        assert_eq!(filtered[1].role, Role::System);
        assert_eq!(filtered[2].role, Role::Tool);
    }

    #[test]
    fn load_history_filters_empty_assistant() {
        // Regression: after a run with empty termination (text=null),
        // the saved history must not include empty assistant messages
        // because OpenAI-compatible APIs reject them with HTTP 400.
        #[allow(deprecated)]
        let tool_call = ToolCall::new("bash", serde_json::json!({"cmd": "ls"}));
        let tool_id = tool_call.id.clone();
        let file = write_llm_request(vec![
            Message::user("list files"),
            Message::assistant(None, vec![tool_call], None),
            Message::tool(tool_id.as_uuid(), "file.txt".into()),
            // The empty assistant that causes HTTP 400.
            Message::assistant(None, vec![], None),
        ]);

        let history = load_conversation_history(file.path(), 0, 0).unwrap();

        // Should have 3 messages: user, assistant (with tool call), tool result.
        // The empty assistant must be filtered out.
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].role, Role::User);
        assert_eq!(history[1].role, Role::Assistant);
        assert!(history[1].content.iter().any(|b| matches!(b, ContentBlock::ToolCall(_))));
        assert_eq!(history[2].role, Role::Tool);
    }
}
