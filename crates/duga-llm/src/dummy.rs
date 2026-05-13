//! Dummy LLM client for tests and offline development.

use crate::{estimate_tokens, ChatFuture, LlmClient, LlmError};
use duga_events::EventSink;
use duga_types::llm::{LlmCallOptions, LlmResponse, TokenUsage};
use duga_types::message::{AssistantMessage, Message};
use duga_types::tool_schema::ToolSchema;
use std::collections::VecDeque;
use std::sync::Mutex;

pub struct DummyClient {
    model: String,
    responses: Mutex<VecDeque<LlmResponse>>,
}

impl DummyClient {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            responses: Mutex::new(VecDeque::new()),
        }
    }

    pub fn with_response(model: impl Into<String>, response: LlmResponse) -> Self {
        let mut responses = VecDeque::new();
        responses.push_back(response);
        Self {
            model: model.into(),
            responses: Mutex::new(responses),
        }
    }

    pub fn push_response(&self, response: LlmResponse) {
        self.responses.lock().unwrap().push_back(response);
    }

    pub fn text_response(text: impl Into<String>) -> LlmResponse {
        LlmResponse {
            message: AssistantMessage {
                text: Some(text.into()),
                tool_calls: Vec::new(),
            },
            usage: TokenUsage {
                prompt: 0,
                completion: 0,
            },
        }
    }
}

impl Default for DummyClient {
    fn default() -> Self {
        Self::new("dummy")
    }
}

impl LlmClient for DummyClient {
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
                .ok_or_else(|| LlmError::Provider("dummy client has no queued response".into()))
        })
    }

    fn count_tokens(&self, messages: &[Message]) -> usize {
        estimate_tokens(messages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use duga_events::NullSink;
    use duga_types::message::Message;

    #[test]
    fn counts_tokens_with_fallback_estimator() {
        let client = DummyClient::default();
        assert_eq!(client.count_tokens(&[Message::user("hello world")]), 6);
    }

    #[tokio::test]
    async fn returns_queued_responses_in_order() {
        let client = DummyClient::default();
        client.push_response(DummyClient::text_response("one"));
        client.push_response(DummyClient::text_response("two"));

        let first = client
            .chat(&[], &[], LlmCallOptions::default(), &NullSink)
            .await
            .unwrap();
        let second = client
            .chat(&[], &[], LlmCallOptions::default(), &NullSink)
            .await
            .unwrap();

        assert_eq!(first.message.text, Some("one".into()));
        assert_eq!(second.message.text, Some("two".into()));
    }
}
