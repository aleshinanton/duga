//! Message types — the universal communication format.
//!
//! Messages flow between LLM, memory, event system, and tools.
//! Every message has a `Role` and a list of `ContentBlock` items.

use crate::tool_call::ToolCall;
use serde::{Deserialize, Serialize};
use std::fmt;

/// The sender/receiver role of a message.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Role::System => write!(f, "system"),
            Role::User => write!(f, "user"),
            Role::Assistant => write!(f, "assistant"),
            Role::Tool => write!(f, "tool"),
        }
    }
}

/// A single piece of content within a message.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
    ToolCall(ToolCall),
}

/// A message in the agent conversation.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
    #[serde(default)]
    pub name: Option<String>,
    pub pinned: bool,
    /// Provider-specific reasoning/thinking content (e.g. DeepSeek reasoning_content).
    /// Must be passed back to the API when present in assistant messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}

impl Message {
    pub fn new(role: Role, content: Vec<ContentBlock>) -> Self {
        Self {
            role,
            content,
            name: None,
            pinned: false,
            reasoning_content: None,
        }
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self::new(Role::User, vec![ContentBlock::Text { text: text.into() }])
    }

    pub fn assistant(
        text: Option<String>,
        tool_calls: Vec<ToolCall>,
        reasoning_content: Option<String>,
    ) -> Self {
        let mut content = Vec::new();
        if let Some(t) = text {
            content.push(ContentBlock::Text { text: t });
        }
        for tc in tool_calls {
            content.push(ContentBlock::ToolCall(tc));
        }
        Self {
            role: Role::Assistant,
            content,
            name: None,
            pinned: false,
            reasoning_content,
        }
    }

    pub fn system(text: impl Into<String>) -> Self {
        Self::new(Role::System, vec![ContentBlock::Text { text: text.into() }])
    }

    pub fn tool(tool_call_id: uuid::Uuid, output: String) -> Self {
        Self::new(Role::Tool, vec![ContentBlock::Text { text: output }])
            .with_name(tool_call_id.to_string())
    }

    fn with_name(mut self, name: String) -> Self {
        self.name = Some(name);
        self
    }
}

/// The content of an assistant turn: optional text plus tool calls.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AssistantMessage {
    pub text: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    /// Provider-specific reasoning/thinking content (e.g. DeepSeek reasoning_content).
    /// Must be passed back to the API when present in assistant messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}

impl AssistantMessage {
    pub fn is_termination(&self) -> bool {
        self.tool_calls.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_message_roundtrip() {
        let msg = Message::user("hello world");
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.role, Role::User);
        assert_eq!(decoded.content.len(), 1);
    }

    #[test]
    fn test_assistant_message_roundtrip() {
        let msg = Message::assistant(None, vec![], None);
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.role, Role::Assistant);
    }

    #[test]
    fn test_assistant_with_text_roundtrip() {
        let msg = Message::assistant(Some("hello".into()), vec![], None);
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.role, Role::Assistant);
        assert!(matches!(
            decoded.content[0],
            ContentBlock::Text { ref text } if text == "hello"
        ));
    }

    #[test]
    fn test_system_message_roundtrip() {
        let msg = Message::system("you are helpful");
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.role, Role::System);
    }

    #[test]
    fn test_role_display() {
        assert_eq!(format!("{}", Role::System), "system");
        assert_eq!(format!("{}", Role::User), "user");
        assert_eq!(format!("{}", Role::Assistant), "assistant");
        assert_eq!(format!("{}", Role::Tool), "tool");
    }

    #[test]
    fn test_tool_message() {
        let id = uuid::Uuid::new_v4();
        let msg = Message::tool(id, "result".into());
        assert_eq!(msg.role, Role::Tool);
        assert_eq!(msg.name, Some(id.to_string()));
    }

    #[test]
    fn test_assistant_with_tool_calls() {
        let tc = ToolCall::new("search", serde_json::json!({"query": "test"}));
        let msg = Message::assistant(Some("thinking".into()), vec![tc.clone()], None);
        assert_eq!(msg.content.len(), 2);
        assert!(matches!(
            msg.content[1],
            ContentBlock::ToolCall(ref t) if t.id == tc.id
        ));
    }
}
