//! Deterministic test doubles for agent loop and replay tests.

use duga_events::{Event, EventFuture, EventSink};
use duga_llm::{ChatFuture, LlmClient, LlmError};
use duga_tools::context::ToolContext;
use duga_tools::result::ToolCallResult;
use duga_tools::Tool;
use duga_types::error::ToolError;
use duga_types::llm::{LlmCallOptions, LlmResponse};
use duga_types::message::Message;
use duga_types::tool_call::CallId;
use duga_types::tool_result::{ToolResult, ToolResultBuilder};
use duga_types::tool_schema::ToolSchema;
use serde_json::Value;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

type ToolResponse = Result<ToolResult, ToolError>;
type ExpectedToolResponse = (Value, ToolResponse);

#[derive(Clone)]
pub struct MockTool {
    name: String,
    description: String,
    responses: Arc<Mutex<VecDeque<ExpectedToolResponse>>>,
    always: Arc<Mutex<Option<ToolResponse>>>,
    retryable: bool,
}

impl MockTool {
    pub fn new(name: &str, description: &str) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            responses: Arc::new(Mutex::new(VecDeque::new())),
            always: Arc::new(Mutex::new(None)),
            retryable: false,
        }
    }

    pub fn with_response(
        self,
        expected_args: Value,
        response: Result<ToolResult, ToolError>,
    ) -> Self {
        self.responses
            .lock()
            .unwrap()
            .push_back((expected_args, response));
        self
    }

    pub fn always(self, response: Result<ToolResult, ToolError>) -> Self {
        *self.always.lock().unwrap() = Some(response);
        self
    }

    pub fn with_retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }

    pub fn success(output: impl Into<String>) -> ToolResult {
        ToolResultBuilder::new()
            .tool_call_id(CallId::new())
            .success(true)
            .output(output)
            .build()
            .expect("tool_call_id is set")
    }
}

impl std::fmt::Debug for MockTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockTool")
            .field("name", &self.name)
            .field("retryable", &self.retryable)
            .finish()
    }
}

impl Tool for MockTool {
    type Args = Value;

    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn retryable(&self) -> bool {
        self.retryable
    }

    fn json_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": true
        })
    }

    async fn execute(&self, _ctx: ToolContext<'_>, args: Self::Args) -> ToolCallResult {
        if let Some(response) = self.always.lock().unwrap().clone() {
            return response;
        }

        let Some((expected, response)) = self.responses.lock().unwrap().pop_front() else {
            return Err(ToolError::Io(format!(
                "mock tool '{}' queue exhausted",
                self.name
            )));
        };
        if expected != args {
            return Err(ToolError::InvalidArgs(format!(
                "mock tool '{}' expected args {}, got {}",
                self.name, expected, args
            )));
        }
        response
    }
}

pub struct MockLlm {
    model: String,
    responses: Arc<Mutex<VecDeque<Result<LlmResponse, LlmError>>>>,
    received: Arc<Mutex<Vec<Vec<Message>>>>,
    token_multiplier: usize,
}

impl MockLlm {
    pub fn new(responses: Vec<Result<LlmResponse, LlmError>>) -> Self {
        Self {
            model: "mock".into(),
            responses: Arc::new(Mutex::new(responses.into())),
            received: Arc::new(Mutex::new(Vec::new())),
            token_multiplier: 10,
        }
    }

    pub fn with_token_multiplier(mut self, multiplier: usize) -> Self {
        self.token_multiplier = multiplier;
        self
    }

    pub fn received_messages(&self) -> Vec<Vec<Message>> {
        self.received.lock().unwrap().clone()
    }
}

impl LlmClient for MockLlm {
    fn model(&self) -> &str {
        &self.model
    }

    fn chat<'a>(
        &'a self,
        messages: &'a [Message],
        _tools: &'a [ToolSchema],
        options: LlmCallOptions,
        event_sink: &'a dyn EventSink,
    ) -> ChatFuture<'a> {
        Box::pin(async move {
            self.received.lock().unwrap().push(messages.to_vec());
            let response = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| LlmError::Provider("mock exhausted".into()))??;
            if options.streaming {
                if let Some(reasoning) = &response.message.reasoning_content {
                    for ch in reasoning.chars() {
                        event_sink
                            .emit(Event::LlmThinkingDelta {
                                model: self.model.clone(),
                                delta: ch.to_string(),
                            })
                            .await
                            .map_err(|e| LlmError::Provider(e.to_string()))?;
                    }
                    event_sink
                        .emit(Event::LlmThinkingDelta {
                            model: self.model.clone(),
                            delta: "\n".into(),
                        })
                        .await
                        .map_err(|e| LlmError::Provider(e.to_string()))?;
                }
                if let Some(text) = &response.message.text {
                    for word in text.split_whitespace() {
                        event_sink
                            .emit(Event::LlmTokenDelta {
                                model: self.model.clone(),
                                delta: word.into(),
                            })
                            .await
                            .map_err(|e| LlmError::Provider(e.to_string()))?;
                    }
                }
            }
            Ok(response)
        })
    }

    fn count_tokens(&self, messages: &[Message]) -> usize {
        messages.len() * self.token_multiplier
    }
}

#[derive(Clone, Default)]
pub struct CapturingEventSink {
    events: Arc<Mutex<Vec<Event>>>,
}

impl CapturingEventSink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn events(&self) -> Vec<Event> {
        self.events.lock().unwrap().clone()
    }

    pub fn events_of_type(&self, predicate: impl Fn(&Event) -> bool) -> Vec<Event> {
        self.events()
            .into_iter()
            .filter(|event| predicate(event))
            .collect()
    }
}

impl EventSink for CapturingEventSink {
    fn name(&self) -> &str {
        "capturing"
    }

    fn emit<'a>(&'a self, event: Event) -> EventFuture<'a> {
        Box::pin(async move {
            self.events.lock().unwrap().push(event);
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use duga_events::NullSink;
    use duga_sandbox::{CancellationToken, Workspace};

    #[tokio::test]
    async fn mock_tool_returns_scripted_response() {
        let tool = MockTool::new("mock", "mock")
            .with_response(serde_json::json!({"x": 1}), Ok(MockTool::success("ok")));
        let dir = tempfile::tempdir().unwrap();
        let workspace = Workspace::open(dir.path()).unwrap();

        let result = tool
            .execute(
                ToolContext::new(&workspace, CancellationToken::new(), &NullSink),
                serde_json::json!({"x": 1}),
            )
            .await
            .unwrap();

        assert_eq!(result.output, "ok");
    }

    #[tokio::test]
    async fn mock_llm_records_messages() {
        let response = LlmResponse {
            message: duga_types::message::AssistantMessage {
                text: Some("done".into()),
                tool_calls: vec![],
                reasoning_content: None,
            },
            usage: duga_types::llm::TokenUsage {
                prompt: 0,
                completion: 0,
            },
        };
        let llm = MockLlm::new(vec![Ok(response)]);
        let sink = CapturingEventSink::new();

        let result = llm
            .chat(
                &[Message::user("hello")],
                &[],
                LlmCallOptions { streaming: true },
                &sink,
            )
            .await
            .unwrap();

        assert_eq!(result.message.text, Some("done".into()));
        assert_eq!(llm.received_messages().len(), 1);
        assert_eq!(
            sink.events(),
            vec![Event::LlmTokenDelta {
                model: "mock".into(),
                delta: "done".into()
            }]
        );
    }

    #[tokio::test]
    async fn mock_llm_streaming_with_thinking_emits_both_delta_types() {
        let response = LlmResponse {
            message: duga_types::message::AssistantMessage {
                text: Some("the answer is 42".into()),
                tool_calls: vec![],
                reasoning_content: Some("Let me think step by step".into()),
            },
            usage: duga_types::llm::TokenUsage { prompt: 0, completion: 0 },
        };
        let llm = MockLlm::new(vec![Ok(response)]);
        let sink = CapturingEventSink::new();
        llm.chat(&[Message::user("q")], &[], LlmCallOptions { streaming: true }, &sink).await.unwrap();
        let events = sink.events();
        assert!(matches!(events[0], Event::LlmThinkingDelta { .. }));
        assert!(matches!(events.last().unwrap(), Event::LlmTokenDelta { .. }));
        // "Let me think step by step" → 25 chars + \n separator = 26 thinking deltas
        // "the answer is 42" → 4 words = 4 text deltas; total: 30
        assert_eq!(events.len(), 30);
    }

    #[tokio::test]
    async fn mock_llm_non_streaming_emits_zero_deltas() {
        let response = LlmResponse {
            message: duga_types::message::AssistantMessage {
                text: Some("done".into()),
                tool_calls: vec![],
                reasoning_content: Some("thinking".into()),
            },
            usage: duga_types::llm::TokenUsage { prompt: 0, completion: 0 },
        };
        let llm = MockLlm::new(vec![Ok(response)]);
        let sink = CapturingEventSink::new();
        llm.chat(&[Message::user("hello")], &[], LlmCallOptions { streaming: false }, &sink).await.unwrap();
        assert!(sink.events().is_empty());
    }
}
