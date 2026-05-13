//! OpenAI chat-completions provider.

use crate::{estimate_tokens, message_text, ChatFuture, LlmClient, LlmError};
use duga_events::EventSink;
use duga_types::llm::{LlmCallOptions, LlmResponse, TokenUsage};
use duga_types::message::{AssistantMessage, ContentBlock, Message, Role};
use duga_types::tool_call::ToolCall;
use duga_types::tool_schema::ToolSchema;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

pub struct OpenAiClient {
    model: String,
    api_key: String,
    base_url: String,
    http: reqwest::Client,
}

impl OpenAiClient {
    pub fn new(
        model: impl Into<String>,
        api_key: impl Into<String>,
        base_url: Option<String>,
    ) -> Self {
        Self {
            model: model.into(),
            api_key: api_key.into(),
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.into()),
            http: reqwest::Client::new(),
        }
    }

    pub fn from_env(model: impl Into<String>) -> Result<Self, LlmError> {
        let base_url = std::env::var("BASE_URL").ok();
        let api_key = match std::env::var("OPENAI_API_KEY") {
            Ok(api_key) => api_key,
            Err(_) if base_url.is_some() => String::new(),
            Err(_) => {
                return Err(LlmError::InvalidRequest(
                    "OPENAI_API_KEY is not set; set BASE_URL for OpenAI-compatible local endpoints"
                        .into(),
                ));
            }
        };
        Ok(Self::new(model, api_key, base_url))
    }

    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }
}

impl LlmClient for OpenAiClient {
    fn model(&self) -> &str {
        &self.model
    }

    fn chat<'a>(
        &'a self,
        messages: &'a [Message],
        tools: &'a [ToolSchema],
        _options: LlmCallOptions,
        _event_sink: &'a dyn EventSink,
    ) -> ChatFuture<'a> {
        Box::pin(async move {
            let request = OpenAiRequest::from_duga(&self.model, messages, tools)?;
            let request_builder = self.http.post(self.endpoint());
            let request_builder = if self.api_key.is_empty() {
                request_builder
            } else {
                request_builder.bearer_auth(&self.api_key)
            };
            let response = request_builder
                .json(&request)
                .send()
                .await
                .map_err(|e| LlmError::Transport(e.to_string()))?;
            let status = response.status();
            let body = response
                .text()
                .await
                .map_err(|e| LlmError::Transport(e.to_string()))?;
            if !status.is_success() {
                return Err(map_status(status, body));
            }
            let response: OpenAiResponse =
                serde_json::from_str(&body).map_err(|e| LlmError::Provider(e.to_string()))?;
            response.into_duga()
        })
    }

    fn count_tokens(&self, messages: &[Message]) -> usize {
        estimate_tokens(messages)
    }
}

fn map_status(status: StatusCode, body: String) -> LlmError {
    if status == StatusCode::TOO_MANY_REQUESTS {
        LlmError::RateLimited(body)
    } else {
        LlmError::Provider(format!("OpenAI HTTP {status}: {body}"))
    }
}

#[derive(Debug, Serialize)]
struct OpenAiRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OpenAiToolSpec>,
}

impl OpenAiRequest {
    fn from_duga(
        model: &str,
        messages: &[Message],
        tools: &[ToolSchema],
    ) -> Result<Self, LlmError> {
        Ok(Self {
            model: model.into(),
            messages: messages
                .iter()
                .map(openai_message)
                .collect::<Result<_, _>>()?,
            tools: tools.iter().map(OpenAiToolSpec::from_schema).collect(),
        })
    }
}

#[derive(Debug, Serialize)]
struct OpenAiMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAiToolCall>>,
}

fn openai_message(message: &Message) -> Result<OpenAiMessage, LlmError> {
    let text = message_text(message);
    let tool_calls = message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolCall(call) => Some(OpenAiToolCall::from_duga(call)),
            ContentBlock::Text { .. } => None,
        })
        .collect::<Vec<_>>();

    match message.role {
        Role::System | Role::User => Ok(OpenAiMessage {
            role: message.role.to_string(),
            content: Some(text),
            name: message.name.clone(),
            tool_call_id: None,
            tool_calls: None,
        }),
        Role::Assistant => Ok(OpenAiMessage {
            role: "assistant".into(),
            content: if text.is_empty() { None } else { Some(text) },
            name: message.name.clone(),
            tool_call_id: None,
            tool_calls: if tool_calls.is_empty() {
                None
            } else {
                Some(tool_calls)
            },
        }),
        Role::Tool => Ok(OpenAiMessage {
            role: "tool".into(),
            content: Some(text),
            name: None,
            tool_call_id: Some(message.name.clone().ok_or_else(|| {
                LlmError::InvalidRequest("tool message missing tool_call_id".into())
            })?),
            tool_calls: None,
        }),
    }
}

#[derive(Debug, Serialize)]
struct OpenAiToolCall {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    function: OpenAiFunctionCall,
}

impl OpenAiToolCall {
    fn from_duga(call: &ToolCall) -> Self {
        Self {
            id: call.id.to_string(),
            kind: "function".into(),
            function: OpenAiFunctionCall {
                name: call.tool.clone(),
                arguments: call.raw_args.to_string(),
            },
        }
    }
}

#[derive(Debug, Serialize)]
struct OpenAiFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Debug, Serialize)]
struct OpenAiToolSpec {
    #[serde(rename = "type")]
    kind: String,
    function: OpenAiFunctionSpec,
}

impl OpenAiToolSpec {
    fn from_schema(schema: &ToolSchema) -> Self {
        Self {
            kind: "function".into(),
            function: OpenAiFunctionSpec {
                name: schema.name.clone(),
                description: schema.description.clone(),
                parameters: schema.args_schema.clone(),
            },
        }
    }
}

#[derive(Debug, Serialize)]
struct OpenAiFunctionSpec {
    name: String,
    description: String,
    parameters: Value,
}

#[derive(Debug, Deserialize)]
struct OpenAiResponse {
    choices: Vec<OpenAiChoice>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

impl OpenAiResponse {
    fn into_duga(self) -> Result<LlmResponse, LlmError> {
        let choice = self
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| LlmError::Provider("OpenAI response had no choices".into()))?;
        let tool_calls = choice
            .message
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(OpenAiResponseToolCall::into_duga)
            .collect::<Result<Vec<_>, _>>()?;
        let usage = self.usage.unwrap_or_default();
        Ok(LlmResponse {
            message: AssistantMessage {
                text: choice.message.content,
                tool_calls,
            },
            usage: TokenUsage {
                prompt: usage.prompt_tokens,
                completion: usage.completion_tokens,
            },
        })
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: OpenAiResponseMessage,
}

#[derive(Debug, Deserialize)]
struct OpenAiResponseMessage {
    content: Option<String>,
    tool_calls: Option<Vec<OpenAiResponseToolCall>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiResponseToolCall {
    function: OpenAiResponseFunctionCall,
}

impl OpenAiResponseToolCall {
    fn into_duga(self) -> Result<ToolCall, LlmError> {
        let raw_args = serde_json::from_str(&self.function.arguments)
            .map_err(|e| LlmError::Provider(format!("invalid OpenAI tool arguments: {e}")))?;
        Ok(ToolCall::new(self.function.name, raw_args))
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiResponseFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Debug, Default, Deserialize)]
struct OpenAiUsage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use duga_types::message::Message;

    #[test]
    fn request_maps_tools_and_messages() {
        let request = OpenAiRequest::from_duga(
            "gpt-4.1",
            &[Message::system("sys"), Message::user("hello")],
            &[ToolSchema::new(
                "read",
                "read a file",
                serde_json::json!({"type": "object"}),
            )],
        )
        .unwrap();
        let json = serde_json::to_value(request).unwrap();

        assert_eq!(json["model"], "gpt-4.1");
        assert_eq!(json["messages"][0]["role"], "system");
        assert_eq!(json["tools"][0]["function"]["name"], "read");
    }

    #[test]
    fn response_maps_tool_calls() {
        let raw = serde_json::json!({
            "choices": [{
                "message": {
                    "content": "I'll read it",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "read", "arguments": "{\"path\":\"Cargo.toml\"}"}
                    }]
                }
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        });

        let response: OpenAiResponse = serde_json::from_value(raw).unwrap();
        let mapped = response.into_duga().unwrap();

        assert_eq!(mapped.message.text, Some("I'll read it".into()));
        assert_eq!(mapped.message.tool_calls[0].tool, "read");
        assert_eq!(mapped.message.tool_calls[0].raw_args["path"], "Cargo.toml");
        assert_eq!(mapped.usage.total(), 15);
    }
}
