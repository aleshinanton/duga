//! Tool call representation.
//!
//! A `ToolCall` identifies a specific tool invocation with a unique ID,
//! the tool name, and raw JSON arguments.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Unique identifier for a tool call instance.
#[derive(Clone, Debug, Deserialize, Hash, PartialEq, Serialize)]
pub struct CallId(Uuid);

impl CallId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl std::fmt::Display for CallId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Default for CallId {
    fn default() -> Self {
        Self::new()
    }
}

/// Represents a single tool call made by the assistant.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ToolCall {
    pub id: CallId,
    pub tool: String,
    pub raw_args: serde_json::Value,
}

impl ToolCall {
    pub fn new(tool: impl Into<String>, raw_args: serde_json::Value) -> Self {
        Self {
            id: CallId::new(),
            tool: tool.into(),
            raw_args,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_tool_call_new_generates_id() {
        let tc = ToolCall::new("shell", json!({"cmd": "ls"}));
        assert_ne!(tc.id.as_uuid().to_string().len(), 0);
        assert_eq!(tc.tool, "shell");
    }

    #[test]
    fn test_tool_call_roundtrip() {
        let tc = ToolCall::new("search", json!({"query": "test"}));
        let json = serde_json::to_string(&tc).unwrap();
        let decoded: ToolCall = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.id, tc.id);
        assert_eq!(decoded.tool, "search");
    }

    #[test]
    fn test_call_id_display() {
        let id = CallId::new();
        let s = format!("{}", id);
        assert_eq!(s.len(), 36);
    }
}
