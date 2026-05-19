use duga_core::{Memory, Summarizer, SummaryFuture};
use duga_llm::LlmClient;
use duga_types::error::AgentError;
use duga_types::llm::SummaryMessage;
use duga_types::message::{AssistantMessage, Message};
use std::sync::Mutex;

struct MockTokenizer;

impl LlmClient for MockTokenizer {
    fn count_tokens(&self, messages: &[Message]) -> usize {
        messages.len() * 10
    }
}

struct MockSummarizer {
    seen: Mutex<Vec<Vec<Message>>>,
}

impl MockSummarizer {
    fn new() -> Self {
        Self {
            seen: Mutex::new(Vec::new()),
        }
    }
}

impl Summarizer for MockSummarizer {
    fn summarize<'a>(&'a self, messages: &'a [Message]) -> SummaryFuture<'a> {
        Box::pin(async move {
            self.seen.lock().unwrap().push(messages.to_vec());
            Ok(SummaryMessage::with_pinned_facts(
                format!("summary of {} messages", messages.len()),
                Vec::new(),
            ))
        })
    }
}

#[tokio::test]
async fn full_memory_lifecycle_compresses_and_keeps_recent_context() {
    let mut memory = Memory::new(vec![Message::system("system")], 100, 0.8);
    for i in 0..8 {
        memory.push_user(format!("message {i}"));
    }
    assert!(memory.over_budget(&MockTokenizer));

    memory
        .compress(&MockSummarizer::new(), &MockTokenizer)
        .await
        .unwrap();

    assert!(memory.summary().is_some());
    assert_eq!(memory.recent_messages().len(), 4);
    assert_eq!(
        memory.summary().unwrap().pinned_facts,
        vec!["message 0".to_string()]
    );
}

#[tokio::test]
async fn second_compression_receives_previous_summary_as_context() {
    let mut memory = Memory::new(vec![Message::system("system")], 200, 0.8);
    for i in 0..6 {
        memory.push_user(format!("message {i}"));
    }
    let summarizer = MockSummarizer::new();
    memory.compress(&summarizer, &MockTokenizer).await.unwrap();

    for i in 6..10 {
        memory.push_assistant(AssistantMessage {
            text: Some(format!("assistant {i}")),
            tool_calls: vec![],
            reasoning_content: None,
        });
    }
    memory.compress(&summarizer, &MockTokenizer).await.unwrap();

    let seen = summarizer.seen.lock().unwrap();
    let second_input = &seen[1];
    assert!(second_input.iter().any(|message| {
        message
            .content
            .iter()
            .any(|block| format!("{:?}", block).contains("summary of"))
    }));
}

#[tokio::test]
async fn overflow_reports_error_when_pinned_context_cannot_fit() {
    let mut memory = Memory::new(vec![Message::system("system")], 10, 1.0);
    memory.push_user("important pinned task".into());

    let result = memory
        .compress(&MockSummarizer::new(), &MockTokenizer)
        .await;

    assert!(matches!(result, Err(AgentError::ContextOverflow)));
}
