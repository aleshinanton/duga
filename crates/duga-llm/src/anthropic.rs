//! Anthropic Messages API provider.

use crate::{estimate_tokens, message_text, ChatFuture, LlmClient, LlmError};
use duga_events::EventSink;
use duga_types::llm::{LlmCallOptions, LlmResponse, TokenUsage};
use duga_types::message::{AssistantMessage, ContentBlock, Message, Role};
use duga_types::tool_call::ToolCall;
use duga_types::tool_schema::ToolSchema;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const ANTHROPIC_VERSION: &str = "2023-06-01";

pub struct AnthropicClient {
    model: String,
    api_key: String,
    base_url: String,
    max_tokens: u32,
    http: reqwest::Client,
}

impl AnthropicClient {
    pub fn new(
        model: impl Into<String>,
        api_key: impl Into<String>,
        base_url: Option<String>,
    ) -> Self {
        Self {
            model: model.into(),
            api_key: api_key.into(),
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.into()),
            max_tokens: 4096,
            http: reqwest::Client::new(),
        }
    }

    pub fn from_env(model: impl Into<String>) -> Result<Self, LlmError> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| LlmError::InvalidRequest("ANTHROPIC_API_KEY is not set".into()))?;
        let base_url = std::env::var("BASE_URL").ok();
        Ok(Self::new(model, api_key, base_url))
    }

    fn endpoint(&self) -> String {
        format!("{}/v1/messages", self.base_url.trim_end_matches('/'))
    }
}

impl LlmClient for AnthropicClient {
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
            let request =
                AnthropicRequest::from_duga(&self.model, self.max_tokens, messages, tools);
            let response = self
                .http
                .post(self.endpoint())
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", ANTHROPIC_VERSION)
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
            let response: AnthropicResponse =
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
        LlmError::Provider(format!("Anthropic HTTP {status}: {body}"))
    }
}

#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<AnthropicToolSpec>,
}

impl AnthropicRequest {
    fn from_duga(model: &str, max_tokens: u32, messages: &[Message], tools: &[ToolSchema]) -> Self {
        let system = messages
            .iter()
            .filter(|message| matches!(message.role, Role::System))
            .map(message_text)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        Self {
            model: model.into(),
            max_tokens,
            system: if system.is_empty() {
                None
            } else {
                Some(system)
            },
            messages: messages
                .iter()
                .filter(|message| !matches!(message.role, Role::System))
                .map(anthropic_message)
                .collect(),
            tools: tools.iter().map(AnthropicToolSpec::from_schema).collect(),
        }
    }
}

#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: String,
    content: Vec<AnthropicContentBlock>,
}

fn anthropic_message(message: &Message) -> AnthropicMessage {
    match message.role {
        Role::Assistant => {
            let mut content = Vec::new();
            let text = message_text(message);
            if !text.is_empty() {
                content.push(AnthropicContentBlock::Text { text });
            }
            for block in &message.content {
                if let ContentBlock::ToolCall(call) = block {
                    content.push(AnthropicContentBlock::ToolUse {
                        id: call.id.to_string(),
                        name: call.tool.clone(),
                        input: call.raw_args.clone(),
                    });
                }
            }
            AnthropicMessage {
                role: "assistant".into(),
                content,
            }
        }
        Role::Tool => AnthropicMessage {
            role: "user".into(),
            content: vec![AnthropicContentBlock::ToolResult {
                tool_use_id: message.name.clone().unwrap_or_default(),
                content: message_text(message),
            }],
        },
        Role::System | Role::User => AnthropicMessage {
            role: "user".into(),
            content: vec![AnthropicContentBlock::Text {
                text: message_text(message),
            }],
        },
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

#[derive(Debug, Serialize)]
struct AnthropicToolSpec {
    name: String,
    description: String,
    input_schema: Value,
}

impl AnthropicToolSpec {
    fn from_schema(schema: &ToolSchema) -> Self {
        Self {
            name: schema.name.clone(),
            description: schema.description.clone(),
            input_schema: schema.args_schema.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicResponseBlock>,
    #[serde(default)]
    usage: AnthropicUsage,
}

impl AnthropicResponse {
    fn into_duga(self) -> Result<LlmResponse, LlmError> {
        let mut text = Vec::new();
        let mut tool_calls = Vec::new();
        for block in self.content {
            match block {
                AnthropicResponseBlock::Text { text: block_text } => text.push(block_text),
                AnthropicResponseBlock::ToolUse { name, input, .. } => {
                    tool_calls.push(ToolCall::new(name, input));
                }
                AnthropicResponseBlock::Other => {}
            }
        }
        Ok(LlmResponse {
            message: AssistantMessage {
                text: if text.is_empty() {
                    None
                } else {
                    Some(text.join("\n"))
                },
                tool_calls,
            },
            usage: TokenUsage {
                prompt: self.usage.input_tokens,
                completion: self.usage.output_tokens,
            },
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicResponseBlock {
    Text {
        text: String,
    },
    ToolUse {
        name: String,
        input: Value,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Default, Deserialize)]
struct AnthropicUsage {
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    output_tokens: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use duga_types::message::Message;

    #[test]
    fn request_maps_system_tools_and_messages() {
        let request = AnthropicRequest::from_duga(
            "claude-sonnet-4-5",
            4096,
            &[Message::system("sys"), Message::user("hello")],
            &[ToolSchema::new(
                "read",
                "read a file",
                serde_json::json!({"type": "object"}),
            )],
        );
        let json = serde_json::to_value(request).unwrap();

        assert_eq!(json["model"], "claude-sonnet-4-5");
        assert_eq!(json["system"], "sys");
        assert_eq!(json["messages"][0]["role"], "user");
        assert_eq!(json["tools"][0]["name"], "read");
    }

    #[test]
    fn response_maps_tool_use_blocks() {
        let raw = serde_json::json!({
            "content": [
                {"type": "text", "text": "I'll read it"},
                {"type": "tool_use", "id": "toolu_1", "name": "read", "input": {"path": "Cargo.toml"}}
            ],
            "usage": {"input_tokens": 10, "output_tokens": 5}
        });

        let response: AnthropicResponse = serde_json::from_value(raw).unwrap();
        let mapped = response.into_duga().unwrap();

        assert_eq!(mapped.message.text, Some("I'll read it".into()));
        assert_eq!(mapped.message.tool_calls[0].tool, "read");
        assert_eq!(mapped.message.tool_calls[0].raw_args["path"], "Cargo.toml");
        assert_eq!(mapped.usage.total(), 15);
    }
}
