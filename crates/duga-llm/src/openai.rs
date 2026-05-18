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

#[derive(Debug)]
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
        let raw_args = parse_tool_args(&self.function.name, &self.function.arguments)?;
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

/// Parse tool arguments from the LLM's JSON string, with repair for common
/// LLM-generated JSON errors. Never crashes — returns empty object as last resort
/// (the tool's own schema validation will catch missing required fields).
fn parse_tool_args(_tool_name: &str, json_str: &str) -> Result<serde_json::Value, LlmError> {
    // Fast path: valid JSON.
    if let Ok(v) = serde_json::from_str(json_str) {
        return Ok(v);
    }

    // Try repair: fix unescaped control chars, truncated JSON, etc.
    let repaired = repair_json(json_str);
    if let Ok(v) = serde_json::from_str(&repaired) {
        return Ok(v);
    }

    // Last resort: return empty object so the run doesn't die.
    // The tool's schema validation will report missing required fields
    // to the LLM, which can then retry with corrected arguments.
    Ok(serde_json::json!({}))
}

/// Repair common LLM-generated JSON errors (modeled after pi-mom's repairJson).
///
/// Handles:
/// - Unescaped control characters inside strings (newlines, tabs, etc.)
/// - Invalid escape sequences (backslash before non-escape char)
/// - Truncated JSON (missing closing braces/brackets/quotes)
fn repair_json(json_str: &str) -> String {
    let mut out = String::with_capacity(json_str.len() + 16);
    let mut in_string = false;
    let chars: Vec<char> = json_str.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];

        if !in_string {
            out.push(ch);
            if ch == '"' {
                in_string = true;
            }
            i += 1;
            continue;
        }

        // Inside a string.
        if ch == '"' {
            out.push(ch);
            in_string = false;
            i += 1;
            continue;
        }

        if ch == '\\' {
            let next = chars.get(i + 1).copied();
            match next {
                // Valid JSON escapes: pass through.
                Some('"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't') => {
                    out.push('\\');
                    out.push(next.unwrap());
                    i += 2;
                    continue;
                }
                // Unicode escape \uXXXX.
                Some('u') => {
                    let hex: String = chars[i + 2..].iter().take(4).copied().collect();
                    if hex.len() == 4 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
                        out.push_str(&format!("\\u{}", hex));
                        i += 6;
                        continue;
                    }
                    // Invalid unicode escape — double the backslash.
                    out.push_str("\\\\");
                    i += 1;
                    continue;
                }
                // End of input after backslash — double it.
                None => {
                    out.push_str("\\\\");
                    i += 1;
                    continue;
                }
                // Invalid escape: double the backslash.
                _ => {
                    out.push_str("\\\\");
                    i += 1;
                    continue;
                }
            }
        }

        // Control character inside string — escape it.
        if ch.is_control() && ch != '\n' && ch != '\r' && ch != '\t' {
            // These are already valid in JSON strings.
            out.push(ch);
        } else if ch == '\n' {
            out.push_str("\\n");
        } else if ch == '\r' {
            out.push_str("\\r");
        } else if ch == '\t' {
            out.push_str("\\t");
        } else {
            out.push(ch);
        }
        i += 1;
    }

    // If still inside a string at EOF, close the quote.
    if in_string {
        out.push('"');
    }

    // Balance braces/brackets for truncated JSON.
    out = balance_json(&out);

    out
}

/// Add missing closing braces/brackets for truncated JSON.
fn balance_json(json_str: &str) -> String {
    let mut depth_brace: i32 = 0;
    let mut depth_bracket: i32 = 0;
    let mut in_string = false;
    let mut escape = false;

    for ch in json_str.chars() {
        if escape {
            escape = false;
            continue;
        }
        if ch == '\\' && in_string {
            escape = true;
            continue;
        }
        if ch == '"' && !escape {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        match ch {
            '{' => depth_brace += 1,
            '}' => depth_brace -= 1,
            '[' => depth_bracket += 1,
            ']' => depth_bracket -= 1,
            _ => {}
        }
    }

    let mut out = json_str.to_string();
    for _ in 0..depth_bracket.max(0) {
        out.push(']');
    }
    for _ in 0..depth_brace.max(0) {
        out.push('}');
    }
    out
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

    // ── JSON repair tests ──

    #[test]
    fn parse_tool_args_valid_json() {
        let v = parse_tool_args("bash", r#"{"cmd": "ls"}"#).unwrap();
        assert_eq!(v["cmd"], "ls");
    }

    #[test]
    fn parse_tool_args_empty_string() {
        // Empty string → repaired to valid empty object.
        let v = parse_tool_args("bash", "").unwrap();
        assert!(v.is_object());
        assert!(v.as_object().unwrap().is_empty());
    }

    #[test]
    fn parse_tool_args_truncated_json() {
        // Truncated: missing closing brace.
        let v = parse_tool_args("bash", r#"{"command": ["ls""#).unwrap();
        // Should be repaired by adding closing brace.
        assert!(v.is_object());
        assert_eq!(v["command"][0], "ls");
    }

    #[test]
    fn parse_tool_args_truncated_nested() {
        // Truncated: missing closing brace and bracket.
        let v = parse_tool_args("bash", r#"{"command": ["ls", "-la""#).unwrap();
        assert!(v.is_object());
        assert_eq!(v["command"][1], "-la");
    }

    #[test]
    fn parse_tool_args_garbage_json() {
        // Completely invalid JSON → fallback to empty object.
        let v = parse_tool_args("bash", "not json at all!!!").unwrap();
        assert!(v.is_object());
        assert!(v.as_object().unwrap().is_empty());
    }

    #[test]
    fn parse_tool_args_never_crashes() {
        // All kinds of bad input should return Ok, not Err.
        for input in &["", "{", "[}", "xxx", "{\"a\": "] {
            let result = parse_tool_args("test", input);
            assert!(result.is_ok(), "should not crash on: {input:?}");
        }
    }

    #[test]
    fn repair_json_unescaped_newlines_in_string() {
        // LLM sometimes puts literal newlines inside JSON strings.
        let bad = "{\"thought\": \"line1\nline2\"}";
        let repaired = repair_json(bad);
        let v: serde_json::Value = serde_json::from_str(&repaired).unwrap();
        assert_eq!(v["thought"], "line1\nline2");
    }

    #[test]
    fn repair_json_invalid_escape_sequence() {
        // LLM sometimes uses backslash before non-escape chars like \x, \p, etc.
        // \U is not a valid JSON escape — it should be doubled to \\U.
        let bad = r#"{"path": "C:\Users\stuff"}"#;
        let repaired = repair_json(bad);
        let v: serde_json::Value = serde_json::from_str(&repaired).unwrap();
        // \U was doubled to \\U, \s was doubled to \\s.
        assert_eq!(v["path"], "C:\\Users\\stuff");
    }

    #[test]
    fn balance_json_truncated_object() {
        assert_eq!(balance_json(r#"{"a": 1"#), r#"{"a": 1}"#);
        assert_eq!(balance_json(r#"{"a": {"b": 2}"#), r#"{"a": {"b": 2}}"#);
    }

    #[test]
    fn balance_json_truncated_array() {
        assert_eq!(balance_json(r#"["a", "b""#), r#"["a", "b"]"#);
        assert_eq!(balance_json(r#"{"cmd": ["ls""#), r#"{"cmd": ["ls"]}"#);
    }

    #[test]
    fn balance_json_complete_is_unchanged() {
        let valid = r#"{"a": 1, "b": [2, 3]}"#;
        assert_eq!(balance_json(valid), valid);
    }

    #[test]
    fn response_maps_tool_calls_with_bad_json() {
        // Tool call arguments have unescaped newlines (common LLM mistake).
        let raw = serde_json::json!({
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "think", "arguments": "{\"thought\": \"line one\nline two\"}"}
                    }]
                }
            }]
        });

        let response: OpenAiResponse = serde_json::from_value(raw).unwrap();
        let mapped = response.into_duga().unwrap();
        // Should have repaired the newlines and parsed successfully.
        assert_eq!(mapped.message.tool_calls[0].tool, "think");
        assert_eq!(mapped.message.tool_calls[0].raw_args["thought"], "line one\nline two");
    }

    #[test]
    fn response_maps_tool_calls_truncated_json() {
        // Truncated tool arguments → repaired, not fatal.
        let raw = serde_json::json!({
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "bash", "arguments": "{\"command\": [\"ls\""}
                    }]
                }
            }]
        });

        let response: OpenAiResponse = serde_json::from_value(raw).unwrap();
        let mapped = response.into_duga().unwrap();
        // Should have added missing ]} and parsed.
        assert_eq!(mapped.message.tool_calls[0].tool, "bash");
        assert_eq!(mapped.message.tool_calls[0].raw_args["command"][0], "ls");
    }

    #[test]
    fn response_maps_tool_calls_garbage_json() {
        // Completely invalid tool arguments → empty object, not fatal.
        let raw = serde_json::json!({
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "bash", "arguments": "I am not JSON"}
                    }]
                }
            }]
        });

        let response: OpenAiResponse = serde_json::from_value(raw).unwrap();
        let mapped = response.into_duga().unwrap();
        // Should fall back to empty object — run not killed.
        assert_eq!(mapped.message.tool_calls[0].tool, "bash");
        assert!(mapped.message.tool_calls[0].raw_args.as_object().unwrap().is_empty());
    }
}
