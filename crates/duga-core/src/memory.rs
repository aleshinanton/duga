//! Conversation memory with token budgeting and compression.

use crate::summarizer::Summarizer;
use duga_llm::LlmClient;
use duga_types::error::AgentError;
use duga_types::llm::SummaryMessage;
use duga_types::message::{AssistantMessage, ContentBlock, Message};
use duga_types::tool_result::ToolResult;
use std::collections::VecDeque;

#[derive(Debug)]
pub struct Memory {
    system_messages: Vec<Message>,
    summary: Option<SummaryMessage>,
    recent_messages: VecDeque<Message>,
    max_tokens: usize,
    compress_at_ratio: f64,
    pinned_task: Option<String>,
}

impl Memory {
    pub fn new(mut system_messages: Vec<Message>, max_tokens: usize, ratio: f64) -> Self {
        for message in &mut system_messages {
            message.pinned = true;
        }

        let compress_at_ratio = if ratio.is_nan() {
            0.8
        } else {
            ratio.clamp(0.1, 1.0)
        };

        Self {
            system_messages,
            summary: None,
            recent_messages: VecDeque::new(),
            max_tokens,
            compress_at_ratio,
            pinned_task: None,
        }
    }

    pub fn system_messages(&self) -> &[Message] {
        &self.system_messages
    }

    pub fn summary(&self) -> Option<&SummaryMessage> {
        self.summary.as_ref()
    }

    pub fn recent_messages(&self) -> &VecDeque<Message> {
        &self.recent_messages
    }

    pub fn compress_at_ratio(&self) -> f64 {
        self.compress_at_ratio
    }

    pub fn max_tokens(&self) -> usize {
        self.max_tokens
    }

    pub fn pinned_task(&self) -> Option<&str> {
        self.pinned_task.as_deref()
    }

    pub fn push_user(&mut self, task: String) {
        let should_pin = self.pinned_task.is_none();
        let mut message = Message::user(task.clone());
        if should_pin {
            message.pinned = true;
            self.pinned_task = Some(task);
        }
        self.recent_messages.push_back(message);
    }

    pub fn push_assistant(&mut self, msg: AssistantMessage) {
        self.recent_messages
            .push_back(Message::assistant(msg.text, msg.tool_calls));
    }

    pub fn push_tool_result(&mut self, result: ToolResult) {
        self.recent_messages
            .push_back(Message::tool(result.tool_call_id.as_uuid(), result.output));
    }

    /// Push an arbitrary message into memory (for restoring conversation history).
    pub fn push_msg(&mut self, msg: Message) {
        self.recent_messages.push_back(msg);
    }

    pub fn messages(&self) -> Vec<Message> {
        let mut messages = Vec::with_capacity(
            self.system_messages.len()
                + usize::from(self.summary.is_some())
                + self.recent_messages.len(),
        );
        messages.extend(self.system_messages.iter().cloned());
        if let Some(summary) = &self.summary {
            messages.push(Self::summary_to_message(summary));
        }
        messages.extend(self.recent_messages.iter().cloned());
        messages
    }

    pub fn token_count(&self, tokenizer: &dyn LlmClient) -> usize {
        tokenizer.count_tokens(&self.messages())
    }

    pub fn over_budget(&self, tokenizer: &dyn LlmClient) -> bool {
        if self.max_tokens == 0 {
            return true;
        }
        let threshold = (self.max_tokens as f64 * self.compress_at_ratio).floor() as usize;
        self.token_count(tokenizer) > threshold
    }

    pub async fn compress(
        &mut self,
        summarizer: &dyn Summarizer,
        tokenizer: &dyn LlmClient,
    ) -> Result<(), AgentError> {
        let pinned_facts = self.pinned_facts();
        let split = self.recent_messages.len() / 2;
        let oldest: Vec<_> = self.recent_messages.iter().take(split).cloned().collect();

        if !oldest.is_empty() || self.summary.is_some() {
            let input = self.compression_input(oldest, &pinned_facts);
            let mut summary = summarizer.summarize(&input).await?;
            for fact in pinned_facts {
                if !summary.pinned_facts.contains(&fact) {
                    summary.pinned_facts.push(fact);
                }
            }
            self.summary = Some(summary);
            self.recent_messages.drain(..split);
        }

        self.drop_until_budget(tokenizer)
    }

    fn compression_input(&self, oldest: Vec<Message>, pinned_facts: &[String]) -> Vec<Message> {
        let mut input = Vec::with_capacity(oldest.len() + usize::from(self.summary.is_some()) + 1);
        if !pinned_facts.is_empty() {
            input.push(Message::system(format_pinned_facts(pinned_facts)));
        }
        if let Some(summary) = &self.summary {
            input.push(Self::summary_to_message(summary));
        }
        input.extend(oldest);
        input
    }

    fn drop_until_budget(&mut self, tokenizer: &dyn LlmClient) -> Result<(), AgentError> {
        while self.hard_over_budget(tokenizer) {
            if let Some(index) = self
                .recent_messages
                .iter()
                .position(|message| !message.pinned)
            {
                self.recent_messages.remove(index);
            } else {
                return Err(AgentError::ContextOverflow);
            }
        }
        Ok(())
    }

    fn hard_over_budget(&self, tokenizer: &dyn LlmClient) -> bool {
        self.token_count(tokenizer) > self.max_tokens
    }

    fn pinned_facts(&self) -> Vec<String> {
        let mut facts = Vec::new();
        if let Some(task) = &self.pinned_task {
            facts.push(task.clone());
        }

        for message in self.recent_messages.iter().filter(|message| message.pinned) {
            let text = message_text(message);
            if !text.is_empty() && !facts.contains(&text) {
                facts.push(text);
            }
        }

        facts
    }

    fn summary_to_message(summary: &SummaryMessage) -> Message {
        let mut content = String::new();
        if !summary.pinned_facts.is_empty() {
            content.push_str(&format_pinned_facts(&summary.pinned_facts));
            content.push_str("\n\n");
        }
        content.push_str("Summary:\n");
        content.push_str(&summary.content);
        Message::assistant(Some(content), vec![])
    }
}

fn format_pinned_facts(facts: &[String]) -> String {
    let mut content = String::from("Key facts:");
    for fact in facts {
        content.push_str("\n- ");
        content.push_str(fact);
    }
    content
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

#[cfg(test)]
mod tests {
    use super::*;
    use duga_types::tool_call::CallId;
    use duga_types::tool_result::ToolResultBuilder;
    use std::sync::Mutex;

    struct FixedTokenizer(usize);

    impl LlmClient for FixedTokenizer {
        fn count_tokens(&self, _messages: &[Message]) -> usize {
            self.0
        }
    }

    struct LinearTokenizer {
        per_message: usize,
    }

    impl LlmClient for LinearTokenizer {
        fn count_tokens(&self, messages: &[Message]) -> usize {
            messages.len() * self.per_message
        }
    }

    struct RecordingSummarizer {
        calls: Mutex<Vec<Vec<Message>>>,
    }

    struct FailingSummarizer;

    impl RecordingSummarizer {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
            }
        }

        fn call_count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    impl Summarizer for RecordingSummarizer {
        fn summarize<'a>(&'a self, messages: &'a [Message]) -> crate::SummaryFuture<'a> {
            Box::pin(async move {
                self.calls.lock().unwrap().push(messages.to_vec());
                Ok(SummaryMessage::with_pinned_facts(
                    format!("summary of {} messages", messages.len()),
                    Vec::new(),
                ))
            })
        }
    }

    impl Summarizer for FailingSummarizer {
        fn summarize<'a>(&'a self, _messages: &'a [Message]) -> crate::SummaryFuture<'a> {
            Box::pin(async { Err(AgentError::SummarizerFailed("boom".into())) })
        }
    }

    #[test]
    fn constructor_pins_system_and_clamps_ratio() {
        let memory = Memory::new(vec![Message::system("sys")], 100, 2.0);
        assert!(memory.system_messages()[0].pinned);
        assert_eq!(memory.compress_at_ratio(), 1.0);

        let memory = Memory::new(vec![], 100, 0.0);
        assert_eq!(memory.compress_at_ratio(), 0.1);
    }

    #[test]
    fn push_methods_store_expected_messages() {
        let mut memory = Memory::new(vec![], 100, 0.8);
        memory.push_user("task".into());
        memory.push_assistant(AssistantMessage {
            text: Some("hi".into()),
            tool_calls: vec![],
        });
        let result = ToolResultBuilder::new()
            .tool_call_id(CallId::new())
            .success(true)
            .output("ok")
            .build()
            .unwrap();
        memory.push_tool_result(result);

        assert_eq!(memory.recent_messages().len(), 3);
        assert_eq!(memory.pinned_task(), Some("task"));
        assert!(memory.recent_messages()[0].pinned);
        assert_eq!(memory.recent_messages()[2].role.to_string(), "tool");
    }

    #[test]
    fn messages_are_ordered_system_summary_recent() {
        let mut memory = Memory::new(vec![Message::system("sys")], 100, 0.8);
        memory.summary = Some(SummaryMessage::with_pinned_facts(
            "old".into(),
            vec!["fact".into()],
        ));
        memory.push_user("task".into());

        let messages = memory.messages();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role.to_string(), "system");
        assert!(message_text(&messages[1]).contains("Key facts:\n- fact\n\nSummary:\nold"));
        assert_eq!(messages[2].role.to_string(), "user");
    }

    #[test]
    fn budget_uses_tokenizer_and_ratio() {
        let memory = Memory::new(vec![], 200, 0.8);
        assert!(!memory.over_budget(&FixedTokenizer(100)));
        assert!(!memory.over_budget(&FixedTokenizer(160)));
        assert!(memory.over_budget(&FixedTokenizer(170)));

        let memory = Memory::new(vec![], 0, 0.8);
        assert!(memory.over_budget(&FixedTokenizer(0)));
    }

    #[tokio::test]
    async fn compression_summarizes_oldest_half_and_preserves_pinned_task() {
        let mut memory = Memory::new(vec![Message::system("sys")], 100, 0.8);
        for i in 0..10 {
            memory.push_user(format!("message {i}"));
        }
        let summarizer = RecordingSummarizer::new();

        memory
            .compress(&summarizer, &LinearTokenizer { per_message: 1 })
            .await
            .unwrap();

        assert_eq!(summarizer.call_count(), 1);
        assert_eq!(memory.recent_messages().len(), 5);
        assert_eq!(
            memory.summary().unwrap().pinned_facts,
            vec!["message 0".to_string()]
        );
    }

    #[tokio::test]
    async fn compression_includes_existing_summary_as_context() {
        let mut memory = Memory::new(vec![], 100, 0.8);
        memory.summary = Some(SummaryMessage::new("previous".into()));
        memory.push_user("task".into());
        memory.push_assistant(AssistantMessage {
            text: Some("new".into()),
            tool_calls: vec![],
        });
        let summarizer = RecordingSummarizer::new();

        memory
            .compress(&summarizer, &LinearTokenizer { per_message: 1 })
            .await
            .unwrap();

        let calls = summarizer.calls.lock().unwrap();
        let input_text = message_text(&calls[0][1]);
        assert!(input_text.contains("Summary:\nprevious"));
    }

    #[tokio::test]
    async fn compression_failure_preserves_recent_messages() {
        let mut memory = Memory::new(vec![Message::system("sys")], 100, 0.8);
        for i in 0..4 {
            memory.push_user(format!("message {i}"));
        }
        let before = memory.recent_messages().clone();

        let result = memory
            .compress(&FailingSummarizer, &LinearTokenizer { per_message: 1 })
            .await;

        assert!(matches!(result, Err(AgentError::SummarizerFailed(_))));
        assert_eq!(memory.recent_messages(), &before);
        assert!(memory.summary().is_none());
    }

    #[tokio::test]
    async fn overflow_drops_oldest_non_pinned_recent() {
        let mut memory = Memory::new(vec![Message::system("sys")], 2, 1.0);
        memory.push_user("task".into());
        memory.push_assistant(AssistantMessage {
            text: Some("drop me first".into()),
            tool_calls: vec![],
        });
        memory.push_assistant(AssistantMessage {
            text: Some("drop me second".into()),
            tool_calls: vec![],
        });
        let summarizer = RecordingSummarizer::new();

        memory
            .compress(&summarizer, &LinearTokenizer { per_message: 1 })
            .await
            .unwrap();

        assert!(memory
            .recent_messages()
            .iter()
            .all(|message| message.pinned));
    }

    #[tokio::test]
    async fn overflow_returns_context_overflow_when_only_pinned_remains() {
        let mut memory = Memory::new(vec![Message::system("sys")], 1, 1.0);
        memory.push_user("task".into());
        let summarizer = RecordingSummarizer::new();

        let result = memory
            .compress(&summarizer, &LinearTokenizer { per_message: 1 })
            .await;

        assert!(matches!(result, Err(AgentError::ContextOverflow)));
    }
}
