//! Unified return type for all loop implementations.

use duga_types::message::AssistantMessage;
use serde::{Deserialize, Serialize};

/// The result returned by every `Loop::run()` invocation.
///
/// Supersedes the now-deprecated `AgentRunResult` by adding a `loop_id`
/// field that records which loop produced this result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoopResult {
    /// The terminal assistant message produced by this loop.
    pub message: AssistantMessage,
    /// Number of loop iterations (steps) consumed.
    pub steps: u32,
    /// Total tool calls made during this run.
    pub tool_calls: u32,
    /// Identifier of the loop that produced this result (e.g. "simple_react").
    pub loop_id: String,
}

impl LoopResult {
    /// Convenience: extract the text payload if any.
    pub fn text(&self) -> Option<&str> {
        self.message.text.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use duga_types::tool_call::ToolCall;

    #[test]
    fn loop_result_serde_roundtrip() {
        let msg = AssistantMessage {
            text: Some("done".into()),
            tool_calls: vec![ToolCall::new("read", serde_json::json!({"path": "x"}))],
            reasoning_content: None,
        };
        let result = LoopResult {
            message: msg,
            steps: 3,
            tool_calls: 5,
            loop_id: "problem_solving".into(),
        };

        let json = serde_json::to_string(&result).unwrap();
        let decoded: LoopResult = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, result);
        assert_eq!(decoded.text(), Some("done"));
    }
}
