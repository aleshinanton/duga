//! Summary compression types.
//!
//! Used during context window overflow to compress older messages
//! while preserving pinned facts verbatim.

use crate::llm::Seq;
use serde::{Deserialize, Serialize};

/// Instruction to the summarizer about which messages to compress.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CompressionRequest {
    /// The messages eligible for compression (oldest first).
    pub messages: Vec<SummarizableMessage>,
    /// Facts that must survive verbatim in the output.
    pub pinned_facts: Vec<String>,
    /// Maximum number of tokens for the summary output.
    pub max_summary_tokens: u32,
}

/// A message that may be included in a compression request.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SummarizableMessage {
    pub seq: Seq,
    pub role: String,
    pub content: String,
}

/// Result of a compression pass.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CompressionResult {
    pub summary: String,
    pub pinned_facts: Vec<String>,
    pub tokens_consumed: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compression_request_roundtrip() {
        let req = CompressionRequest {
            messages: vec![SummarizableMessage {
                seq: Seq::from(0),
                role: "user".into(),
                content: "hello".into(),
            }],
            pinned_facts: vec!["fact1".into()],
            max_summary_tokens: 512,
        };
        let json = serde_json::to_string(&req).unwrap();
        let decoded: CompressionRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, req);
    }

    #[test]
    fn test_compression_result_roundtrip() {
        let result = CompressionResult {
            summary: "user asked for something".into(),
            pinned_facts: vec!["important".into()],
            tokens_consumed: 50,
        };
        let json = serde_json::to_string(&result).unwrap();
        let decoded: CompressionResult = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, result);
    }
}
