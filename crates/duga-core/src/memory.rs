//! Conversation memory with token budgeting and compression.

use crate::summarizer::Summarizer;
use duga_llm::LlmClient;
use duga_types::error::AgentError;
use duga_types::llm::SummaryMessage;
use duga_types::message::{AssistantMessage, ContentBlock, Message, Role};
use duga_types::tool_result::ToolResult;
use std::collections::VecDeque;

#[derive(Debug)]
pub struct Memory {
    system_messages: Vec<Message>,
    /// Task anchoring prefix pinned at position 0 of every request.
    /// Never compressed, never evicted.
    task_anchor: Option<Message>,
    summary: Option<SummaryMessage>,
    recent_messages: VecDeque<Message>,
    max_tokens: usize,
    compress_at_ratio: f64,
    pinned_task: Option<String>,
    /// Sliding window config for restored history (0 = disabled).
    context_window_size: usize,
    max_context_tokens: usize,
}

impl Memory {
    pub fn new(
        mut system_messages: Vec<Message>,
        max_tokens: usize,
        ratio: f64,
        context_window_size: usize,
        max_context_tokens: usize,
    ) -> Self {
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
            task_anchor: None,
            summary: None,
            recent_messages: VecDeque::new(),
            max_tokens,
            compress_at_ratio,
            pinned_task: None,
            context_window_size,
            max_context_tokens,
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

    /// Set the task anchoring prefix that is pinned at position 0 of every request.
    ///
    /// The anchor is never included in compression input nor evicted by the
    /// token budget.  Passing `None` clears any existing anchor.
    pub fn set_task_anchor(&mut self, task: Option<String>) {
        self.task_anchor = task.map(|t| {
            let mut msg = Message::system(t);
            msg.pinned = true;
            msg
        });
    }

    /// Restore conversation history into memory (for persistent chat context).
    ///
    /// All messages are stored as evictable regardless of their previous
    /// `pinned` status — only the *current* run's messages should be
    /// protected from eviction.  This prevents pinned messages from
    /// accumulating across runs and blocking the sliding window.
    ///
    /// The sliding window and token budget from the memory config are applied
    /// after loading to prevent irrelevant history from saturating context.
    pub fn restore_history(&mut self, messages: Vec<Message>) {
        let before = self.recent_messages.len();
        for mut msg in messages {
            // Drop cross-run pin accumulation: only the current run's
            // push_user decides what stays pinned.  Without this, every
            // run's first user message would remain pinned forever and
            // enforce_window can't evict them.
            msg.pinned = false;
            self.push_msg(msg);
        }
        let after = self.recent_messages.len();
        self.enforce_window();
        tracing::info!(
            before,
            after,
            remaining = self.recent_messages.len(),
            "History restored and window enforced"
        );
    }

    /// Enforce the sliding window and token budget on restored history.
    ///
    /// Called after `restore_history` to limit how many messages from
    /// previous sessions are kept in the active context.  Pinned messages
    /// from the *current* run (first user task, system prompts) are never
    /// removed.  Restored history messages are unpinned by `restore_history`
    /// and are eligible for eviction.
    pub fn enforce_window(&mut self) {
        let window_size = self.context_window_size;
        let max_tokens = self.max_context_tokens;

        if window_size > 0 && self.recent_messages.len() > window_size {
            let excess = self.recent_messages.len() - window_size;
            let mut dropped = 0;
            while dropped < excess && self.recent_messages.len() > 1 {
                // Find the first non-pinned message to drop.
                if let Some(idx) = self
                    .recent_messages
                    .iter()
                    .position(|m| !m.pinned)
                {
                    self.recent_messages.remove(idx);
                    dropped += 1;
                } else {
                    break; // only pinned messages remain
                }
            }
        }

        if max_tokens > 0 {
            while estimate_tokens(&Vec::from_iter(self.recent_messages.iter().cloned()))
                > max_tokens
                && self.recent_messages.len() > 1
            {
                if let Some(idx) = self
                    .recent_messages
                    .iter()
                    .position(|m| !m.pinned)
                {
                    self.recent_messages.remove(idx);
                } else {
                    break;
                }
            }
        }
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
            .push_back(Message::assistant(msg.text, msg.tool_calls, msg.reasoning_content));
    }

    pub fn push_tool_result(&mut self, result: ToolResult) {
        self.recent_messages
            .push_back(Message::tool(result.tool_call_id.as_uuid(), result.output));
    }

    /// Push an arbitrary message into memory (for restoring conversation history).
    pub fn push_msg(&mut self, msg: Message) {
        self.recent_messages.push_back(msg);
    }

    /// Take a checkpoint — returns the current count of recent messages.
    ///
    /// Use with [`restore`] to isolate context between independent loop
    /// iterations (e.g., verification answer attempts, decomposition subtasks,
    /// search cycles).  Only `recent_messages` is affected; system messages,
    /// task anchor, and summary are immutable from the loop's perspective.
    ///
    /// [`restore`]: Self::restore
    pub fn checkpoint(&self) -> usize {
        self.recent_messages.len()
    }

    /// Restore memory to a previous checkpoint by truncating recent messages.
    ///
    /// Only affects `recent_messages`; pinned/task-anchor/system/summary
    /// messages are never touched.
    pub fn restore(&mut self, checkpoint: usize) {
        self.recent_messages.truncate(checkpoint);
    }

    pub fn messages(&self) -> Vec<Message> {
        let mut messages = Vec::with_capacity(
            usize::from(self.task_anchor.is_some())
                + self.system_messages.len()
                + usize::from(self.summary.is_some())
                + self.recent_messages.len(),
        );
        // Task anchor at position 0 — never evicted.
        if let Some(anchor) = &self.task_anchor {
            messages.push(anchor.clone());
        }
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
        let split = safe_split_index(&self.recent_messages, self.recent_messages.len() / 2);
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
        // The task anchor lives in its own field and is never passed here
        // (it is not part of recent_messages).  Therefore no explicit
        // filtering is needed — the anchor is excluded by construction.
        let mut input =
            Vec::with_capacity(oldest.len() + usize::from(self.summary.is_some()) + 1);
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
                // If we're removing an assistant with tool_calls, also remove
                // its immediately following tool-result messages to prevent
                // orphaned tool messages in the LLM request.
                let mut remove_count = 1;
                if self.recent_messages[index].role == Role::Assistant
                    && self.recent_messages[index]
                        .content
                        .iter()
                        .any(|block| matches!(block, ContentBlock::ToolCall(_)))
                {
                    for next in self.recent_messages.range(index + 1..) {
                        if next.role == Role::Tool {
                            remove_count += 1;
                        } else {
                            break;
                        }
                    }
                }
                self.recent_messages.drain(index..index + remove_count);
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
        Message::system(content)
    }
}

/// Find a split index that doesn't orphan tool messages from their
/// preceding assistant tool_calls message.
fn safe_split_index(messages: &VecDeque<Message>, ideal: usize) -> usize {
    let mut split = ideal.min(messages.len());
    // Walk backward until we're not pointing at a tool message.
    while split > 0 && messages[split].role == Role::Tool {
        split -= 1;
    }
    split
}

fn format_pinned_facts(facts: &[String]) -> String {
    let mut content = String::from("Key facts:");
    for fact in facts {
        content.push_str("\n- ");
        content.push_str(fact);
    }
    content
}

/// Estimate tokens for a slice of messages using a character-based heuristic.
///
/// Roughly 1 token ≈ 4 characters for English text.  Falls back to 64 tokens
/// per message for messages without text content blocks (e.g. tool calls).
/// No LLM round-trip required — suitable for offline budget enforcement.
pub fn estimate_tokens(messages: &[Message]) -> usize {
    messages
        .iter()
        .map(|m| {
            let char_count: usize = m
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.len()),
                    _ => None,
                })
                .sum();
            if char_count > 0 {
                // +3 for rounding up after integer division
                (char_count + 3) / 4
            } else {
                64 // sensible default for tool-call / empty messages
            }
        })
        .sum()
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
    use duga_types::tool_call::{CallId, ToolCall};
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
        let memory = Memory::new(vec![Message::system("sys")], 100, 2.0, 0, 0);
        assert!(memory.system_messages()[0].pinned);
        assert_eq!(memory.compress_at_ratio(), 1.0);

        let memory = Memory::new(vec![], 100, 0.0, 0, 0);
        assert_eq!(memory.compress_at_ratio(), 0.1);
    }

    #[test]
    fn push_methods_store_expected_messages() {
        let mut memory = Memory::new(vec![], 100, 0.8, 0, 0);
        memory.push_user("task".into());
        memory.push_assistant(AssistantMessage {
            text: Some("hi".into()),
            tool_calls: vec![],
            reasoning_content: None,
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
        let mut memory = Memory::new(vec![Message::system("sys")], 100, 0.8, 0, 0);
        memory.summary = Some(SummaryMessage::with_pinned_facts(
            "old".into(),
            vec!["fact".into()],
        ));
        memory.push_user("task".into());

        let messages = memory.messages();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role.to_string(), "system");
        assert_eq!(messages[1].role, Role::System);
        assert!(message_text(&messages[1]).contains("Key facts:\n- fact\n\nSummary:\nold"));
        assert_eq!(messages[2].role.to_string(), "user");
    }

    #[test]
    fn budget_uses_tokenizer_and_ratio() {
        let memory = Memory::new(vec![], 200, 0.8, 0, 0);
        assert!(!memory.over_budget(&FixedTokenizer(100)));
        assert!(!memory.over_budget(&FixedTokenizer(160)));
        assert!(memory.over_budget(&FixedTokenizer(170)));

        let memory = Memory::new(vec![], 0, 0.8, 0, 0);
        assert!(memory.over_budget(&FixedTokenizer(0)));
    }

    #[tokio::test]
    async fn compression_summarizes_oldest_half_and_preserves_pinned_task() {
        let mut memory = Memory::new(vec![Message::system("sys")], 100, 0.8, 0, 0);
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
        let mut memory = Memory::new(vec![], 100, 0.8, 0, 0);
        memory.summary = Some(SummaryMessage::new("previous".into()));
        memory.push_user("task".into());
        memory.push_assistant(AssistantMessage {
            text: Some("new".into()),
            tool_calls: vec![],
            reasoning_content: None,
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
        let mut memory = Memory::new(vec![Message::system("sys")], 100, 0.8, 0, 0);
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
        let mut memory = Memory::new(vec![Message::system("sys")], 2, 1.0, 0, 0);
        memory.push_user("task".into());
        memory.push_assistant(AssistantMessage {
            text: Some("drop me first".into()),
            tool_calls: vec![],
            reasoning_content: None,
        });
        memory.push_assistant(AssistantMessage {
            text: Some("drop me second".into()),
            tool_calls: vec![],
            reasoning_content: None,
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
        let mut memory = Memory::new(vec![Message::system("sys")], 1, 1.0, 0, 0);
        memory.push_user("task".into());
        let summarizer = RecordingSummarizer::new();

        let result = memory
            .compress(&summarizer, &LinearTokenizer { per_message: 1 })
            .await;

        assert!(matches!(result, Err(AgentError::ContextOverflow)));
    }

    #[tokio::test]
    async fn compression_does_not_orphan_tool_messages() {
        // Regression: splitting at len/2 could separate an assistant
        // tool_calls message from its tool results, violating the
        // OpenAI requirement that tool messages follow tool_calls.
        let mut memory = Memory::new(vec![Message::system("sys")], 1000, 0.8, 0, 0);
        memory.push_user("task".into());
        memory.push_assistant(AssistantMessage {
            text: None,
            tool_calls: vec![ToolCall::new("bash", serde_json::json!({"command": ["ls"]}))],
            reasoning_content: None,
        });
        memory.push_tool_result(
            ToolResultBuilder::new()
                .tool_call_id(CallId::new())
                .success(true)
                .output("file.txt")
                .build()
                .unwrap(),
        );
        memory.push_user("next task".into());
        memory.push_assistant(AssistantMessage {
            text: Some("done".into()),
            tool_calls: vec![],
            reasoning_content: None,
        });

        let summarizer = RecordingSummarizer::new();
        memory
            .compress(&summarizer, &LinearTokenizer { per_message: 1 })
            .await
            .unwrap();

        // After compression, the first recent message must not be a
        // tool message (which would be orphaned).
        let recent = memory.recent_messages();
        assert!(
            recent[0].role != Role::Tool,
            "first recent message must not be an orphaned tool role"
        );
    }

    // ── Task anchoring (EPIC-23) tests ──────────────────────────────────

    #[test]
    fn task_anchor_at_position_zero() {
        let mut memory = Memory::new(vec![Message::system("sys")], 100, 0.8, 0, 0);
        memory.set_task_anchor(Some("test task".into()));
        memory.push_user("hello".into());

        let messages = memory.messages();
        assert!(messages[0].role == Role::System);
        let text = message_text(&messages[0]);
        assert_eq!(text, "test task", "anchor should be the raw task text");
        // System messages come after the anchor.
        assert_eq!(messages[1].role.to_string(), "system");
    }

    #[test]
    fn task_anchor_excluded_from_messages_when_not_set() {
        let mut memory = Memory::new(vec![Message::system("sys")], 100, 0.8, 0, 0);
        memory.push_user("task".into());

        let messages = memory.messages();
        assert_eq!(messages[0].role.to_string(), "system");
        // The anchor is not there — first system message is the one we passed.
        let text = message_text(&messages[0]);
        assert_eq!(text, "sys");
    }

    #[test]
    fn task_anchor_not_in_recent_messages() {
        let mut memory = Memory::new(vec![], 100, 0.8, 0, 0);
        memory.set_task_anchor(Some("task".into()));
        memory.push_user("hello".into());

        // The anchor lives in its own field, not in recent_messages.
        assert_eq!(memory.recent_messages().len(), 1);
        assert_eq!(memory.recent_messages()[0].role, Role::User);
    }

    #[tokio::test]
    async fn compression_input_excludes_task_anchor() {
        let mut memory = Memory::new(vec![Message::system("sys")], 100, 0.8, 0, 0);
        memory.set_task_anchor(Some("my task".into()));
        for i in 0..10 {
            memory.push_user(format!("message {i}"));
        }
        let summarizer = RecordingSummarizer::new();

        memory
            .compress(&summarizer, &LinearTokenizer { per_message: 1 })
            .await
            .unwrap();

        let calls = summarizer.calls.lock().unwrap();
        // The summarizer input should NOT contain the task anchor text.
        for msg in &calls[0] {
            let text = message_text(msg);
            assert!(
                text != "my task",
                "task anchor should not be in compression input, got: {text}"
            );
        }
    }

    #[tokio::test]
    async fn task_anchor_survives_drop_until_budget() {
        // Set a tight budget so drop_until_budget fires but still
        // accommodates the anchor + system + summary + pinned user.
        // Non-pinned assistant messages get dropped first.
        let mut memory = Memory::new(vec![Message::system("sys")], 5, 1.0, 0, 0);
        memory.set_task_anchor(Some("critical task".into()));
        memory.push_user("user1".into()); // pinned (first user)
        memory.push_assistant(AssistantMessage {
            text: Some("reply1".into()),
            tool_calls: vec![],
            reasoning_content: None,
        }); // NOT pinned, can be evicted

        let summarizer = RecordingSummarizer::new();
        memory
            .compress(&summarizer, &LinearTokenizer { per_message: 1 })
            .await
            .unwrap();

        // The anchor must still be present in messages().
        let messages = memory.messages();
        let anchor_text = message_text(&messages[0]);
        assert!(
            anchor_text == "critical task",
            "anchor should survive budget enforcement"
        );
    }

    #[test]
    fn set_task_anchor_none_clears_anchor() {
        let mut memory = Memory::new(vec![], 100, 0.8, 0, 0);
        memory.set_task_anchor(Some("task".into()));
        assert!(memory.task_anchor.is_some());

        memory.set_task_anchor(None);
        assert!(memory.task_anchor.is_none());

        let messages = memory.messages();
        assert!(messages.is_empty());
    }

    #[tokio::test]
    async fn drop_until_budget_removes_tool_results_with_assistant() {
        // When an assistant with tool_calls is evicted by the budget,
        // its tool results must also be evicted to avoid orphaned tool
        // messages.
        // max_tokens=5: fits system(1) + summary(1) + pinned_user(1) safely,
        // but the assistant+tool pair is evicted together.
        let mut memory = Memory::new(vec![Message::system("sys")], 5, 1.0, 0, 0);
        // First, build up a non-compressible base.
        memory.push_user("task".into());
        memory.push_assistant(AssistantMessage {
            text: None,
            tool_calls: vec![ToolCall::new("bash", serde_json::json!({"command": ["ls"]}))],
            reasoning_content: None,
        });
        memory.push_tool_result(
            ToolResultBuilder::new()
                .tool_call_id(CallId::new())
                .success(true)
                .output("file.txt")
                .build()
                .unwrap(),
        );
        memory.push_user("task2".into());
        memory.push_assistant(AssistantMessage {
            text: Some("ok".into()),
            tool_calls: vec![],
            reasoning_content: None,
        });

        let summarizer = RecordingSummarizer::new();
        memory
            .compress(&summarizer, &LinearTokenizer { per_message: 1 })
            .await
            .unwrap();

        // No message in recent should be an orphaned tool role.
        for msg in memory.recent_messages().iter() {
            assert_ne!(
                msg.role,
                Role::Tool,
                "tool messages must not be orphaned"
            );
        }
    }

    #[test]
    fn checkpoint_restore_truncates_recent_messages() {
        let mut memory = Memory::new(vec![Message::system("sys")], 1000, 0.8, 0, 0);
        memory.push_user("msg1".into());
        memory.push_user("msg2".into());

        let cp = memory.checkpoint();
        assert_eq!(cp, 2, "checkpoint should return message count");

        memory.push_user("msg3".into());
        memory.push_user("msg4".into());
        assert_eq!(memory.recent_messages().len(), 4);

        memory.restore(cp);
        assert_eq!(memory.recent_messages().len(), 2, "should truncate to cp");

        // Messages after restore should be msg1, msg2
        let msgs = memory.messages();
        // First is task anchor (if any), then system, then recent
        let recent: Vec<_> = msgs.iter().filter(|m| m.role == Role::User).collect();
        assert_eq!(recent.len(), 2);
    }

    #[test]
    fn checkpoint_restore_preserves_system_and_pinned() {
        let mut memory = Memory::new(
            vec![Message::system("sys")],
            1000, 0.8, 0, 0,
        );
        memory.set_task_anchor(Some("task".into()));

        let cp = memory.checkpoint();
        memory.push_user("temp".into());
        memory.restore(cp);

        let msgs = memory.messages();
        // Should still have task anchor + system
        assert!(msgs.iter().any(|m| m.role == Role::System),
            "system messages must survive checkpoint/restore");
        let anchor_text: String = msgs.iter()
            .filter_map(|m| m.content.iter().find_map(|c| match c {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            }))
            .collect::<Vec<_>>()
            .join("");
        assert!(anchor_text.contains("task"),
            "task anchor must survive checkpoint/restore");
    }

    #[test]
    fn restore_beyond_length_no_op() {
        let mut memory = Memory::new(vec![], 1000, 0.8, 0, 0);
        memory.push_user("msg1".into());
        // Restore to a point beyond current length — no panic
        memory.restore(5);
        assert_eq!(memory.recent_messages().len(), 1);
    }

    // ── restore_history pin-clearing tests ────────────────────────────

    #[test]
    fn restore_history_clears_pinned_on_all_messages() {
        let mut memory = Memory::new(vec![], 1000, 0.8, 50, 24000);

        // Simulate restoring history from a previous run where
        // several messages were pinned (as happens when each run's
        // first push_user sets pinned=true).
        let mut pinned_user = Message::user("old task 1");
        pinned_user.pinned = true;
        let mut pinned_user2 = Message::user("old task 2");
        pinned_user2.pinned = true;
        let assistant = Message::assistant(Some("old reply".into()), vec![], None);
        // assistant is not pinned by default — verify it stays that way

        memory.restore_history(vec![
            pinned_user,
            assistant.clone(),
            pinned_user2,
        ]);

        // All three messages should be present.
        assert_eq!(memory.recent_messages().len(), 3);

        // None of the restored messages should be pinned.
        for msg in memory.recent_messages().iter() {
            assert!(
                !msg.pinned,
                "restored messages must not be pinned; role={}, text={:?}",
                msg.role,
                msg.content.first().and_then(|c| match c {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
            );
        }
    }

    #[test]
    fn current_run_push_user_still_pins_after_restore() {
        // The current run's first push_user must still produce a pinned
        // message even after history has been restored.
        let mut memory = Memory::new(vec![], 1000, 0.8, 50, 24000);

        // Restore some history first.
        let mut old = Message::user("old task");
        old.pinned = true;
        memory.restore_history(vec![old]);

        assert!(!memory.recent_messages()[0].pinned,
            "restored message should be unpinned");

        // Now push the current run's task — this should be pinned.
        memory.push_user("current task".into());

        let recent = memory.recent_messages();
        assert_eq!(recent.len(), 2);
        assert!(!recent[0].pinned, "restored message still unpinned");
        assert!(recent[1].pinned, "current-run push_user must pin");
        assert_eq!(memory.pinned_task(), Some("current task"));
    }

    #[test]
    fn enforce_window_evicts_restored_messages_when_over_limit() {
        // With a tight window and restored (now-unpinned) messages,
        // enforce_window must be able to evict the excess.  Before
        // the fix, restored messages carried their old pinned=true
        // and enforce_window would break out early.
        let mut memory = Memory::new(vec![], 1000, 0.8, 3, 0);

        // Restore 5 messages — all would have been pinned in the
        // previous run's capture.
        let restored: Vec<Message> = (0..5)
            .map(|i| {
                let mut m = Message::user(format!("old msg {i}"));
                m.pinned = true; // as they would come from previous LlmRequest
                m
            })
            .collect();
        memory.restore_history(restored);

        // Window size is 3, so 2 should have been evicted.
        assert_eq!(
            memory.recent_messages().len(),
            3,
            "window of 3 must evict 2 excess messages"
        );
    }

    #[test]
    fn multi_run_does_not_accumulate_pinned_user_messages() {
        // Simulate 3 consecutive runs where each run's first message
        // was pinned.  After restoring history from the captured
        // LlmRequest, only the *last* run's push_user should remain
        // pinned.
        let mut memory = Memory::new(vec![], 1000, 0.8, 50, 24000);

        // Run 1: first user message gets pinned.
        memory.push_user("run 1 task".into());
        memory.push_assistant(AssistantMessage {
            text: Some("run 1 reply".into()),
            tool_calls: vec![],
            reasoning_content: None,
        });
        assert!(memory.recent_messages()[0].pinned);

        // Capture what would be in the LlmRequest.
        let run1_snapshot: Vec<Message> = memory
            .recent_messages()
            .iter()
            .cloned()
            .collect();

        // Run 2: fresh memory, restore run 1, add run 2.
        let mut memory2 = Memory::new(vec![], 1000, 0.8, 50, 24000);
        memory2.restore_history(run1_snapshot);
        memory2.push_user("run 2 task".into());
        memory2.push_assistant(AssistantMessage {
            text: Some("run 2 reply".into()),
            tool_calls: vec![],
            reasoning_content: None,
        });

        // Only the last push_user (run 2) should be pinned.
        let pinned_count = memory2
            .recent_messages()
            .iter()
            .filter(|m| m.pinned)
            .count();
        assert_eq!(
            pinned_count, 1,
            "only the current run's task should be pinned after restore, got {}",
            pinned_count
        );
        let last_user = memory2
            .recent_messages()
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .unwrap();
        assert!(
            last_user.pinned,
            "current run's user message must be pinned"
        );
    }
}
