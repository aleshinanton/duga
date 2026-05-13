//! LLM response types.
//!
//! Defines the interface between the agent loop and the LLM providers.

use crate::message::AssistantMessage;
use serde::{Deserialize, Serialize};

/// Options for a single LLM call.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LlmCallOptions {
    pub streaming: bool,
}

impl Default for LlmCallOptions {
    fn default() -> Self {
        Self { streaming: false }
    }
}

/// Token usage statistics from a single LLM call.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TokenUsage {
    pub prompt: u32,
    pub completion: u32,
}

impl TokenUsage {
    pub fn total(self) -> u32 {
        self.prompt + self.completion
    }
}

/// Complete response from an LLM call.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LlmResponse {
    pub message: AssistantMessage,
    pub usage: TokenUsage,
}

/// A minimal summarization output stored in memory.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SummaryMessage {
    pub content: String,
    pub pinned_facts: Vec<String>,
}

impl SummaryMessage {
    pub fn new(content: String) -> Self {
        Self {
            content,
            pinned_facts: Vec::new(),
        }
    }

    pub fn with_pinned_facts(content: String, pinned_facts: Vec<String>) -> Self {
        Self {
            content,
            pinned_facts,
        }
    }
}

/// Monotonically increasing sequence number for event ordering.
#[derive(Clone, Copy, Debug, Deserialize, Hash, PartialEq, PartialOrd, Serialize)]
pub struct Seq(u64);

impl Seq {
    pub fn next(self) -> Seq {
        Seq(self.0 + 1)
    }

    pub fn value(self) -> u64 {
        self.0
    }
}

impl From<u64> for Seq {
    fn from(n: u64) -> Self {
        Seq(n)
    }
}

impl From<Seq> for u64 {
    fn from(s: Seq) -> Self {
        s.0
    }
}

impl std::fmt::Display for Seq {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "seq-{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_seq_next() {
        assert_eq!(Seq(0).next(), Seq(1));
        assert_eq!(Seq(42).next(), Seq(43));
    }

    #[test]
    fn test_seq_display() {
        assert_eq!(format!("{}", Seq(0)), "seq-0");
        assert_eq!(format!("{}", Seq(42)), "seq-42");
    }

    #[test]
    fn test_seq_ordering() {
        assert!(Seq(0) < Seq(1));
        assert!(Seq(100) > Seq(99));
    }

    #[test]
    fn test_seq_roundtrip() {
        let s = Seq(12345);
        let json = serde_json::to_string(&s).unwrap();
        let decoded: Seq = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, s);
    }

    #[test]
    fn test_summary_message_new() {
        let sm = SummaryMessage::new("test summary".into());
        assert_eq!(sm.content, "test summary");
        assert!(sm.pinned_facts.is_empty());
    }

    #[test]
    fn test_summary_message_with_pinned() {
        let sm = SummaryMessage::with_pinned_facts(
            "summary".into(),
            vec!["fact1".into(), "fact2".into()],
        );
        assert_eq!(sm.pinned_facts.len(), 2);
        assert_eq!(sm.pinned_facts[0], "fact1");
    }

    #[test]
    fn test_token_usage_total() {
        let usage = TokenUsage {
            prompt: 100,
            completion: 50,
        };
        assert_eq!(usage.total(), 150);
    }

    #[test]
    fn test_token_usage_roundtrip() {
        let usage = TokenUsage {
            prompt: 1234,
            completion: 5678,
        };
        let json = serde_json::to_string(&usage).unwrap();
        let decoded: TokenUsage = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, usage);
    }
}
