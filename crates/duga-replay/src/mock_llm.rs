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
