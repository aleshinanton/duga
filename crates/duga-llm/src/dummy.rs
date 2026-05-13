//! Dummy LLM client for testing. Will be replaced in EPIC-8.

use crate::LlmClient;
use duga_types::message::{ContentBlock, Message};

pub struct DummyClient;

impl LlmClient for DummyClient {
    fn count_tokens(&self, messages: &[Message]) -> usize {
        messages
            .iter()
            .flat_map(|message| &message.content)
            .map(|block| match block {
                ContentBlock::Text { text } => text.split_whitespace().count(),
                ContentBlock::ToolCall(_) => 1,
            })
            .sum()
    }
}
