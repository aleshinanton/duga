//! LLM provider boundary for duga.

pub mod dummy;

use duga_types::message::Message;

/// Minimal object-safe LLM boundary needed by the memory subsystem.
pub trait LlmClient: Send + Sync {
    fn count_tokens(&self, messages: &[Message]) -> usize;
}
