//! ChatGPT subscription inference uses Codex Responses, NOT /v1/chat/completions.
use crate::{
    ChatFuture, LlmClient, LlmError, estimate_tokens, message_text,
    openai_oauth::OpenAiOAuthManager,
};
use duga_config::ThinkingLevel;
use duga_events::{Event, EventSink};
use duga_types::{
    llm::{LlmCallOptions, LlmResponse, TokenUsage},
    message::{AssistantMessage, ContentBlock, Message, Role},
    tool_call::ToolCall,
    tool_schema::ToolSchema,
};
use futures::StreamExt;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

const ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/responses";
const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

pub struct OpenAiCodexClient {
    model: String,
    auth: Arc<OpenAiOAuthManager>,
    thinking: ThinkingLevel,
    http: reqwest::Client,
    endpoint: String,
}
impl OpenAiCodexClient {
    /// The OAuth endpoint is fixed so BASE_URL cannot redirect bearer tokens to another service.
    pub fn new(
        model: impl Into<String>,
        auth: Arc<OpenAiOAuthManager>,
        thinking: ThinkingLevel,
    ) -> Self {
        Self {
            model: model.into(),
            auth,
            thinking,
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(15))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("valid HTTP client options"),
            endpoint: ENDPOINT.into(),
        }
    }
    fn request_body(&self, messages: &[Message], tools: &[ToolSchema]) -> Result<Value, LlmError> {
        let mut instructions = Vec::new();
        let mut input = Vec::new();
        for message in messages {
            match message.role {
                Role::System => instructions.push(message_text(message)),
                Role::User => input.push(json!({"role": "user", "content": [{"type": "input_text", "text": message_text(message)}]})),
                Role::Assistant => {
                    let text = message_text(message);
                    if !text.is_empty() {
                        input.push(json!({"role": "assistant", "content": [{"type": "output_text", "text": text}]}));
                    }
                    for block in &message.content {
                        if let ContentBlock::ToolCall(call) = block {
                            // Reconstruct the entire stateless conversation with stable local IDs.
                            // Both function_call and function_call_output use the same normalized ID.
                            input.push(json!({"type": "function_call", "call_id": format!("call_{}", call.id),
                                "name": call.tool, "arguments": call.raw_args.to_string()}));
                        }
                    }
                }
                Role::Tool => {
                    let id = message.name.as_deref().filter(|v| !v.is_empty())
                        .ok_or_else(|| LlmError::InvalidRequest("tool result has no call ID".into()))?;
                    input.push(json!({"type": "function_call_output", "call_id": format!("call_{id}"), "output": message_text(message)}));
                }
            }
        }
        let mut body = json!({
            "model": self.model, "store": false, "stream": true,
            "instructions": if instructions.is_empty() { "You are a helpful coding assistant.".into() } else { instructions.join("\n\n") },
            "input": input, "parallel_tool_calls": true,
        });
        if !tools.is_empty() {
            body["tools"] = Value::Array(
                tools
                    .iter()
                    .map(|tool| {
                        json!({
                            "type": "function", "name": tool.name, "description": tool.description,
                            "parameters": tool.args_schema, "strict": false,
                        })
                    })
                    .collect(),
            );
            body["tool_choice"] = json!("auto");
        }
        let effort = match self.thinking {
            ThinkingLevel::Max => Some("xhigh"), // Codex has no `max` API value.
            _ => self.thinking.to_api_param(),
        };
        if let Some(effort) = effort {
            body["reasoning"] = json!({"effort": effort, "summary": "auto"});
        }
        Ok(body)
    }
    async fn request(&self, body: &Value) -> Result<reqwest::Response, LlmError> {
        let mut credentials = self.auth.credentials().await?;
        for attempt in 0..2 {
            let response = self
                .http
                .post(&self.endpoint)
                .bearer_auth(&credentials.access_token)
                .header("ChatGPT-Account-Id", &credentials.account_id)
                .header("originator", "duga")
                .header("OpenAI-Beta", "responses=experimental")
                .header("Accept", "text/event-stream")
                .json(body)
                .send()
                .await
                .map_err(|e| LlmError::Transport(e.to_string()))?;
            let status = response.status();
            if status == reqwest::StatusCode::UNAUTHORIZED && attempt == 0 {
                credentials = self
                    .auth
                    .credentials_after_rejection(Some(&credentials.access_token))
                    .await?;
                continue;
            }
            if status.is_success() {
                return Ok(response);
            }
            return Err(match status.as_u16() {
                401 | 403 => LlmError::InvalidRequest(format!(
                    "OpenAI Codex access denied (HTTP {status}); run duga-tui --login openai and check your account's Codex access"
                )),
                429 => LlmError::RateLimited("OpenAI Codex usage or rate limit reached".into()),
                _ => LlmError::Provider(format!(
                    "OpenAI Codex request failed (HTTP {status}); check the selected model and account access"
                )),
            });
        }
        unreachable!("two attempts always return")
    }
}
impl LlmClient for OpenAiCodexClient {
    fn model(&self) -> &str {
        &self.model
    }
    fn count_tokens(&self, messages: &[Message]) -> usize {
        estimate_tokens(messages)
    }
    fn chat<'a>(
        &'a self,
        messages: &'a [Message],
        tools: &'a [ToolSchema],
        options: LlmCallOptions,
        sink: &'a dyn EventSink,
    ) -> ChatFuture<'a> {
        Box::pin(async move {
            let body = self.request_body(messages, tools)?;
            let response = self.request(&body).await?;
            parse_stream(response, &self.model, options.streaming, sink).await
        })
    }
}

/// SSE is parsed as bytes until full UTF-8 lines arrive, including CRLF/multiline frames.
#[derive(Default)]
struct SseDecoder {
    bytes: Vec<u8>,
    data: Vec<u8>,
}
impl SseDecoder {
    fn push(&mut self, chunk: &[u8]) -> Result<Vec<Vec<u8>>, LlmError> {
        self.bytes.extend_from_slice(chunk);
        let mut frames = Vec::new();
        let mut offset = 0;
        while let Some(position) = self.bytes[offset..].iter().position(|b| *b == b'\n') {
            let end = offset + position;
            let line = self.bytes[offset..end]
                .strip_suffix(b"\r")
                .unwrap_or(&self.bytes[offset..end]);
            if line.is_empty() {
                if !self.data.is_empty() {
                    frames.push(std::mem::take(&mut self.data));
                }
            } else if let Some(data) = line.strip_prefix(b"data:") {
                let data = data.strip_prefix(b" ").unwrap_or(data);
                if !self.data.is_empty() {
                    self.data.push(b'\n');
                }
                self.data.extend_from_slice(data);
                if self.data.len() > MAX_FRAME_BYTES {
                    return Err(LlmError::Provider(
                        "OpenAI SSE frame exceeds size limit".into(),
                    ));
                }
            }
            offset = end + 1;
        }
        self.bytes.drain(..offset);
        if self.bytes.len() > MAX_FRAME_BYTES {
            return Err(LlmError::Provider(
                "OpenAI SSE line exceeds size limit".into(),
            ));
        }
        Ok(frames)
    }
    fn finish(&mut self) -> Result<Vec<Vec<u8>>, LlmError> {
        // Servers occasionally omit the final empty line.
        self.push(b"\n\n")
    }
}
async fn handle_frame(
    frame: &[u8],
    model: &str,
    streaming: bool,
    sink: &dyn EventSink,
) -> Result<Option<LlmResponse>, LlmError> {
    if frame == b"[DONE]" {
        return Ok(None);
    }
    let value: Value = serde_json::from_slice(frame)
        .map_err(|_| LlmError::Provider("invalid OpenAI Responses SSE event".into()))?;
    match value["type"].as_str().unwrap_or("") {
        "response.output_text.delta"
        | "response.reasoning_summary_text.delta"
        | "response.reasoning_text.delta"
            if streaming =>
        {
            if let Some(delta) = value["delta"].as_str() {
                let event = if value["type"] == "response.output_text.delta" {
                    Event::LlmTokenDelta {
                        model: model.into(),
                        delta: delta.into(),
                    }
                } else {
                    Event::LlmThinkingDelta {
                        model: model.into(),
                        delta: delta.into(),
                    }
                };
                sink.emit(event)
                    .await
                    .map_err(|e| LlmError::Provider(e.to_string()))?;
            }
        }
        "response.completed" | "response.done" => {
            return completed_response(&value["response"]).map(Some);
        }
        "response.failed" | "response.incomplete" | "error" => {
            return Err(LlmError::Provider(
                "OpenAI Codex response failed or was incomplete".into(),
            ));
        }
        _ => {}
    }
    Ok(None)
}
async fn parse_stream(
    response: reqwest::Response,
    model: &str,
    streaming: bool,
    sink: &dyn EventSink,
) -> Result<LlmResponse, LlmError> {
    let mut stream = response.bytes_stream();
    let mut decoder = SseDecoder::default();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| LlmError::Transport(e.to_string()))?;
        for frame in decoder.push(&chunk)? {
            if let Some(result) = handle_frame(&frame, model, streaming, sink).await? {
                return Ok(result);
            }
        }
    }
    for frame in decoder.finish()? {
        if let Some(result) = handle_frame(&frame, model, streaming, sink).await? {
            return Ok(result);
        }
    }
    Err(LlmError::Provider(
        "OpenAI Codex stream ended before response.completed".into(),
    ))
}
fn completed_response(response: &Value) -> Result<LlmResponse, LlmError> {
    if response["status"].as_str() != Some("completed") {
        return Err(LlmError::Provider(
            "OpenAI Codex response is not complete".into(),
        ));
    }
    let output = response["output"]
        .as_array()
        .ok_or_else(|| LlmError::Provider("OpenAI Codex response has no output array".into()))?;
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut tool_calls = Vec::new();
    for item in output {
        match item["type"].as_str() {
            Some("message") => {
                if let Some(content) = item["content"].as_array() {
                    for block in content {
                        if let Some(value) =
                            block["text"].as_str().or_else(|| block["refusal"].as_str())
                        {
                            text.push_str(value);
                        }
                    }
                }
            }
            Some("function_call") => {
                let name = item["name"]
                    .as_str()
                    .filter(|n| !n.is_empty())
                    .ok_or_else(|| LlmError::Provider("OpenAI function call has no name".into()))?;
                let raw = item["arguments"].as_str().ok_or_else(|| {
                    LlmError::Provider("OpenAI function call has no arguments".into())
                })?;
                let args: Value = serde_json::from_str(raw).map_err(|_| {
                    LlmError::Provider("OpenAI function call has invalid JSON arguments".into())
                })?;
                if !args.is_object() {
                    return Err(LlmError::Provider(
                        "OpenAI function arguments must be an object".into(),
                    ));
                }
                tool_calls.push(ToolCall::new(name, args));
            }
            Some("reasoning") => {
                if let Some(summary) = item["summary"].as_array() {
                    for block in summary {
                        if let Some(value) = block["text"].as_str() {
                            reasoning.push_str(value);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(LlmResponse {
        message: AssistantMessage {
            text: (!text.is_empty()).then_some(text),
            tool_calls,
            reasoning_content: (!reasoning.is_empty()).then_some(reasoning),
        },
        usage: TokenUsage {
            prompt: response["usage"]["input_tokens"]
                .as_u64()
                .unwrap_or(0)
                .min(u32::MAX as u64) as u32,
            completion: response["usage"]["output_tokens"]
                .as_u64()
                .unwrap_or(0)
                .min(u32::MAX as u64) as u32,
        },
    })
}

#[cfg(test)]
#[path = "openai_codex_tests.rs"]
mod tests;
