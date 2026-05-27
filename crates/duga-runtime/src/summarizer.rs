//! LLM-driven semantic summarizer for context compression.
//!
//! Replaces the trivial role-label summarizer with an LLM-powered summarizer
//! that preserves topic identity, key findings, decisions, and unresolved
//! questions from compressed conversation segments.

use duga_core::{Summarizer, SummaryFuture};
use duga_llm::LlmClient;
use duga_types::llm::{LlmCallOptions, SummaryMessage};
use duga_types::message::Message;
use std::sync::Arc;
use std::time::Duration;

const SUMMARIZATION_TIMEOUT: Duration = Duration::from_secs(15);

/// Summarization prompt template for initial (from-scratch) compression.
const SUMMARIZE_PROMPT: &str = "\
Summarize the following conversation history in 3-5 bullet points.
Preserve: main topics discussed, key findings, decisions made, and unresolved questions.
Be concise. Output only the bullet points, no preamble.

";

/// Summarization prompt template for update (incremental) compression
/// where an existing summary is available.
const UPDATE_SUMMARIZE_PROMPT: &str = "\
Previous summary:
{existing_summary}

New messages to incorporate:

Update the summary to include the new messages. Keep 3-5 bullet points.
Preserve: main topics, findings, decisions, unresolved questions.
Be concise. Output only the bullet points, no preamble.

";

/// An LLM-driven summarizer that produces meaningful semantic summaries
/// instead of useless role-label lists.
pub struct SemanticSummarizer {
    llm: Arc<dyn LlmClient>,
}

impl SemanticSummarizer {
    pub fn new(llm: Arc<dyn LlmClient>) -> Self {
        Self { llm }
    }
}

impl Summarizer for SemanticSummarizer {
    fn summarize<'a>(&'a self, messages: &'a [Message]) -> SummaryFuture<'a> {
        Box::pin(async move {
            if messages.is_empty() {
                return Ok(SummaryMessage::new(String::new()));
            }

            // Detect whether an existing summary is present in the input.
            let existing_summary = find_existing_summary(messages);

            let prompt = if let Some(ref existing) = existing_summary {
                UPDATE_SUMMARIZE_PROMPT.replace("{existing_summary}", existing)
            } else {
                SUMMARIZE_PROMPT.to_string()
            };

            let formatted = format_messages_for_summarization(messages);
            let user_message = format!("{prompt}{formatted}");

            let llm_messages = vec![
                Message::system(
                    "You are a conversation summarizer. Output only bullet points.",
                ),
                Message::user(user_message),
            ];

            let call = self.llm.chat(
                &llm_messages,
                &[], // no tools
                LlmCallOptions { streaming: false },
                &NullEventSink,
            );

            match tokio::time::timeout(SUMMARIZATION_TIMEOUT, call).await {
                Ok(Ok(response)) => {
                    let text = response
                        .message
                        .text
                        .unwrap_or_default()
                        .trim()
                        .to_string();
                    if text.is_empty() || text.len() > 2000 {
                        // Fall back to role-label on empty or overly long response.
                        Ok(fallback_summary(messages))
                    } else {
                        Ok(SummaryMessage::new(text))
                    }
                }
                Ok(Err(_)) | Err(_) => {
                    // LLM error or timeout — fall back to trivial summary.
                    Ok(fallback_summary(messages))
                }
            }
        })
    }
}

/// Build a single string from a message slice for the summarization prompt.
///
/// Each message is prefixed with its role.
fn format_messages_for_summarization(messages: &[Message]) -> String {
    let mut out = String::new();
    for msg in messages {
        let role = format_role(msg);
        let text = extract_message_text(msg);
        if text.is_empty() {
            continue;
        }
        out.push_str(&format!("[{}]: {}\n", role, text));
    }
    out
}

/// Find an existing summary message in the input slice.
///
/// Returns the summary text if one of the messages looks like a previous
/// summary (starts with "Summary:" or "Key facts:").
fn find_existing_summary(messages: &[Message]) -> Option<String> {
    for msg in messages {
        let text = extract_message_text(msg);
        if text.contains("Summary:") || text.starts_with("Key facts:") {
            // Extract just the summary portion after "Summary:".
            if let Some(pos) = text.find("Summary:") {
                let summary = text[pos..].to_string();
                if !summary.trim().is_empty() {
                    return Some(summary);
                }
            }
            return Some(text);
        }
    }
    None
}

/// Extract all text content from a message.
fn extract_message_text(msg: &Message) -> String {
    msg.content
        .iter()
        .filter_map(|block| match block {
            duga_types::message::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Format the role prefix for a message in summarization input.
fn format_role(msg: &Message) -> String {
    match msg.role {
        duga_types::message::Role::System => "system".into(),
        duga_types::message::Role::User => "user".into(),
        duga_types::message::Role::Assistant => "assistant".into(),
        duga_types::message::Role::Tool => {
            if let Some(ref name) = msg.name {
                format!("tool: {}", name)
            } else {
                "tool".into()
            }
        }
    }
}

/// Fallback summary when the LLM summarizer fails or times out.
///
/// Produces a simple role-label list (same as the old RuntimeSummarizer)
/// so the agent can still make forward progress.
fn fallback_summary(messages: &[Message]) -> SummaryMessage {
    let mut content = String::from("Compressed context:");
    for message in messages.iter().take(16) {
        content.push_str("\n- ");
        content.push_str(&message.role.to_string());
    }
    SummaryMessage::new(content)
}

// ── Event sink stub for non-streaming LLM calls ─────────────────────────

struct NullEventSink;

impl duga_events::EventSink for NullEventSink {
    fn name(&self) -> &str {
        "null-summarizer"
    }

    fn emit<'a>(&'a self, _event: duga_events::Event) -> duga_events::EventFuture<'a> {
        Box::pin(async { Ok(()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use duga_types::message::{ContentBlock, Message, Role};

    fn text_msg(role: Role, text: &str) -> Message {
        Message {
            role,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            name: None,
            pinned: false,
            reasoning_content: None,
        }
    }

    #[test]
    fn format_messages_basic() {
        let messages = vec![
            text_msg(Role::User, "search for Portuguese citizenship law"),
            text_msg(
                Role::Assistant,
                "I'll fetch the Lei da Nacionalidade...",
            ),
        ];
        let result = format_messages_for_summarization(&messages);
        assert!(result.contains("[user]: search for Portuguese citizenship law"));
        assert!(result.contains("[assistant]: I'll fetch the Lei da Nacionalidade..."));
    }

    #[test]
    fn format_messages_preserves_multibyte_utf8() {
        // Regression: slicing at byte boundary used to panic on Cyrillic chars.
        let text = "Я хочу чтобы ты сделал глубокое исследование как здесь устроен TUI и проанализировал как этот функционал возможно реализовать в duga";
        let messages = vec![text_msg(Role::User, text)];
        let result = format_messages_for_summarization(&messages);
        assert!(result.contains(text), "multibyte text must survive intact");
    }

    #[test]
    fn format_messages_preserves_long_messages() {
        // Long messages are no longer truncated — pass through verbatim.
        let long = "A".repeat(600);
        let messages = vec![text_msg(Role::User, &long)];
        let result = format_messages_for_summarization(&messages);
        assert!(result.contains(&long), "long messages must not be truncated");
        assert!(!result.contains("truncated"));
    }

    #[test]
    fn format_messages_with_multiple_content_blocks() {
        let msg = Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Text {
                    text: "part one".into(),
                },
                ContentBlock::Text {
                    text: "part two".into(),
                },
            ],
            name: None,
            pinned: false,
            reasoning_content: None,
        };
        let result = format_messages_for_summarization(&[msg]);
        // Text blocks joined with newline.
        assert!(result.contains("part one\npart two"));
    }

    #[test]
    fn format_messages_skips_empty() {
        let messages = vec![Message {
            role: Role::Tool,
            content: vec![],
            name: None,
            pinned: false,
            reasoning_content: None,
        }];
        let result = format_messages_for_summarization(&messages);
        assert!(result.is_empty());
    }

    #[test]
    fn find_existing_summary_detects_summary_prefix() {
        let messages = vec![text_msg(Role::Assistant, "Summary:\n- topic A\n- finding B")];
        let result = find_existing_summary(&messages);
        assert!(result.is_some());
        assert!(result.unwrap().contains("topic A"));
    }

    #[test]
    fn find_existing_summary_detects_key_facts_prefix() {
        let messages = vec![text_msg(Role::User, "Key facts:\n- fact one\n- fact two")];
        let result = find_existing_summary(&messages);
        assert!(result.is_some());
        let summary = result.unwrap();
        assert!(summary.contains("fact one"));
    }

    #[test]
    fn find_existing_summary_extracts_after_summary_marker() {
        // "Summary:" mid-text — extract from that point onward.
        let messages = vec![text_msg(
            Role::Assistant,
            "blah blah Summary:\n- key point\n- another",
        )];
        let result = find_existing_summary(&messages);
        assert!(result.is_some());
        let summary = result.unwrap();
        assert!(summary.starts_with("Summary:"));
        assert!(summary.contains("key point"));
        assert!(!summary.contains("blah blah"));
    }

    #[test]
    fn find_existing_summary_returns_none_when_absent() {
        let messages = vec![text_msg(Role::User, "hello")];
        assert!(find_existing_summary(&messages).is_none());
    }

    #[test]
    fn extract_message_text_basic() {
        let msg = text_msg(Role::User, "hello world");
        assert_eq!(extract_message_text(&msg), "hello world");
    }

    #[test]
    fn extract_message_text_skips_non_text_blocks() {
        let msg = Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Text {
                    text: "using tool".into(),
                },
                ContentBlock::ToolCall(duga_types::tool_call::ToolCall::new(
                    "bash",
                    serde_json::json!({}),
                )),
            ],
            name: None,
            pinned: false,
            reasoning_content: None,
        };
        assert_eq!(extract_message_text(&msg), "using tool");
    }

    #[test]
    fn extract_message_text_empty_on_no_text() {
        let msg = Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolCall(duga_types::tool_call::ToolCall::new(
                "bash",
                serde_json::json!({}),
            ))],
            name: None,
            pinned: false,
            reasoning_content: None,
        };
        assert!(extract_message_text(&msg).is_empty());
    }

    #[test]
    fn format_role_all_variants() {
        assert_eq!(format_role(&text_msg(Role::System, "x")), "system");
        assert_eq!(format_role(&text_msg(Role::User, "x")), "user");
        assert_eq!(
            format_role(&text_msg(Role::Assistant, "x")),
            "assistant"
        );
    }

    #[test]
    fn format_role_tool_anonymous() {
        let msg = Message {
            role: Role::Tool,
            content: vec![],
            name: None,
            pinned: false,
            reasoning_content: None,
        };
        assert_eq!(format_role(&msg), "tool");
    }

    #[test]
    fn format_role_includes_tool_name() {
        let msg = Message {
            role: Role::Tool,
            content: vec![ContentBlock::Text {
                text: "output".into(),
            }],
            name: Some("bash".into()),
            pinned: false,
            reasoning_content: None,
        };
        assert_eq!(format_role(&msg), "tool: bash");
    }

    #[test]
    fn fallback_summary_produces_role_labels() {
        let messages = vec![
            text_msg(Role::User, "task"),
            text_msg(Role::Assistant, "reply"),
        ];
        let summary = fallback_summary(&messages);
        assert!(summary.content.contains("Compressed context:"));
        assert!(summary.content.contains("user"));
        assert!(summary.content.contains("assistant"));
    }

    #[test]
    fn fallback_summary_caps_at_16_messages() {
        let messages: Vec<_> = (0..20)
            .map(|i| text_msg(Role::User, &format!("msg {i}")))
            .collect();
        let summary = fallback_summary(&messages);
        // Should only contain 16 lines (one per message, capped)
        let line_count = summary.content.lines().count();
        assert_eq!(line_count, 17); // "Compressed context:" + 16 messages
    }
}
