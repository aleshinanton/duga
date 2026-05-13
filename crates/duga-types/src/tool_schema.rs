//! Tool schema — the serialized JSON Schema for one tool.
//!
//! Used by the LLM to decide which tool to call and with what arguments.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Describes a tool's interface for LLM consumption.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub args_schema: Value,
}

impl ToolSchema {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        args_schema: Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            args_schema,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_tool_schema_roundtrip() {
        let schema = ToolSchema::new(
            "read",
            "read a file",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"]
            }),
        );
        let json = serde_json::to_string(&schema).unwrap();
        let decoded: ToolSchema = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.name, "read");
        assert_eq!(decoded.description, "read a file");
    }

    #[test]
    fn test_tool_schema_new() {
        let schema = ToolSchema::new("write", "write a file", serde_json::Value::Null);
        assert_eq!(schema.name, "write");
        assert_eq!(schema.description, "write a file");
    }
}
