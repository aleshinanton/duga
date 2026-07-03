//! LLM provider boundary for duga.

pub mod anthropic;
pub mod dummy;
pub mod oauth;
pub mod openai;
pub mod registry;

use duga_events::EventSink;
use duga_types::llm::{LlmCallOptions, LlmResponse};
use duga_types::message::{ContentBlock, Message};
use duga_types::tool_schema::ToolSchema;
use std::future::Future;
use std::pin::Pin;

pub use anthropic::AnthropicClient;
pub use oauth::OAuthManager;
pub use openai::OpenAiClient;
pub use registry::{ProviderRegistry, ProviderRegistryError};

pub type ChatFuture<'a> = Pin<Box<dyn Future<Output = Result<LlmResponse, LlmError>> + Send + 'a>>;

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum LlmError {
    #[error("llm request was cancelled")]
    Cancelled,
    #[error("llm request timed out")]
    Timeout,
    #[error("llm rate limited: {0}")]
    RateLimited(String),
    #[error("invalid llm request: {0}")]
    InvalidRequest(String),
    #[error("llm transport error: {0}")]
    Transport(String),
    #[error("llm provider error: {0}")]
    Provider(String),
}

/// Object-safe LLM boundary used by memory and the agent loop.
pub trait LlmClient: Send + Sync {
    fn model(&self) -> &str {
        "unknown"
    }

    fn chat<'a>(
        &'a self,
        _messages: &'a [Message],
        _tools: &'a [ToolSchema],
        _options: LlmCallOptions,
        _event_sink: &'a dyn EventSink,
    ) -> ChatFuture<'a> {
        Box::pin(async { Err(LlmError::Provider("chat is not implemented".into())) })
    }

    fn count_tokens(&self, messages: &[Message]) -> usize;
}

pub fn estimate_tokens(messages: &[Message]) -> usize {
    messages
        .iter()
        .map(|message| {
            4 + message
                .content
                .iter()
                .map(|block| match block {
                    ContentBlock::Text { text } => text.split_whitespace().count(),
                    ContentBlock::ToolCall(call) => {
                        1 + call.tool.split_whitespace().count()
                            + call.raw_args.to_string().split_whitespace().count()
                    }
                })
                .sum::<usize>()
        })
        .sum()
}

pub(crate) fn message_text(message: &Message) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            ContentBlock::ToolCall(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
