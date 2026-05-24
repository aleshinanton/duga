use duga_events::{Event, EventSink, StoredEvent};
use duga_llm::{estimate_tokens, ChatFuture, LlmClient, LlmError};
use duga_types::llm::{LlmCallOptions, LlmResponse, TokenUsage};
use duga_types::message::{AssistantMessage, Message};
use duga_types::tool_schema::ToolSchema;
use std::collections::VecDeque;
use std::sync::Mutex;

pub struct ReplayMockLlm {
    model: String,
    responses: Mutex<VecDeque<Result<LlmResponse, LlmError>>>,
}

impl ReplayMockLlm {
    pub fn from_events(model: impl Into<String>, events: &[StoredEvent]) -> Self {
        let responses = events
            .iter()
            .filter_map(|event| match &event.event {
                Event::LlmResponse {
                    text, tool_calls, ..
                } => Some(Ok(LlmResponse {
                    message: AssistantMessage {
                        text: text.clone(),
                        tool_calls: tool_calls.clone(),
                        reasoning_content: None,
                    },
                    usage: TokenUsage {
                        prompt: 0,
                        completion: 0,
                    },
                })),
                _ => None,
            })
            .collect();
        Self {
            model: model.into(),
            responses: Mutex::new(responses),
        }
    }
}

impl LlmClient for ReplayMockLlm {
    fn model(&self) -> &str {
        &self.model
    }

    fn chat<'a>(
        &'a self,
        _messages: &'a [Message],
        _tools: &'a [ToolSchema],
        _options: LlmCallOptions,
        _event_sink: &'a dyn EventSink,
    ) -> ChatFuture<'a> {
        Box::pin(async move {
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| LlmError::Provider("replay LLM exhausted".into()))?
        })
    }

    fn count_tokens(&self, messages: &[Message]) -> usize {
        estimate_tokens(messages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use duga_events::{Event, StoredEvent};
    use duga_types::message::Message;
    use duga_types::tool_call::ToolCall;

    fn make_llm_response_event(seq: u64, text: &str) -> StoredEvent {
        StoredEvent::new(
            seq,
            Event::LlmResponse {
                model: "test-model".into(),
                text: Some(text.into()),
                tool_calls: vec![],
            },
        )
    }

    #[test]
    fn model_returns_configured_name() {
        let llm = ReplayMockLlm::from_events("gpt-4", &[]);
        assert_eq!(llm.model(), "gpt-4");
    }

    #[test]
    fn from_empty_events_produces_empty_queue() {
        let llm = ReplayMockLlm::from_events("test", &[]);
        assert_eq!(llm.responses.lock().unwrap().len(), 0);
    }

    #[test]
    fn from_events_extracts_llm_responses() {
        let events = vec![
            make_llm_response_event(1, "first response"),
            make_llm_response_event(2, "second response"),
        ];
        let llm = ReplayMockLlm::from_events("test", &events);
        assert_eq!(llm.responses.lock().unwrap().len(), 2);
    }

    #[test]
    fn from_events_filters_non_llm_events() {
        let event = StoredEvent::new(
            1,
            Event::AgentStarted {
                task: "test".into(),
            },
        );
        let llm = ReplayMockLlm::from_events("test", &[event]);
        assert_eq!(llm.responses.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn chat_returns_queued_responses_in_order() {
        let events = vec![
            make_llm_response_event(1, "first"),
            make_llm_response_event(2, "second"),
        ];
        let llm = ReplayMockLlm::from_events("test", &events);
        let sink = duga_events::NullSink;

        let r1 = llm.chat(&[], &[], LlmCallOptions::default(), &sink).await.unwrap();
        assert_eq!(r1.message.text.as_deref(), Some("first"));

        let r2 = llm.chat(&[], &[], LlmCallOptions::default(), &sink).await.unwrap();
        assert_eq!(r2.message.text.as_deref(), Some("second"));
    }

    #[tokio::test]
    async fn chat_exhausted_returns_error() {
        let llm = ReplayMockLlm::from_events("test", &[]);
        let sink = duga_events::NullSink;

        let err = llm.chat(&[], &[], LlmCallOptions::default(), &sink).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("exhausted"), "got: {msg}");
    }

    #[test]
    fn count_tokens_uses_estimator() {
        let llm = ReplayMockLlm::from_events("test", &[]);
        let msgs = vec![Message::user("hello world")];
        let tokens = llm.count_tokens(&msgs);
        assert!(tokens > 0, "should estimate positive tokens");
    }
}
