//! Summarizer boundary used by context compression.

use duga_types::error::AgentError;
use duga_types::llm::SummaryMessage;
use duga_types::message::Message;
use std::future::Future;
use std::pin::Pin;

pub type SummaryFuture<'a> =
    Pin<Box<dyn Future<Output = Result<SummaryMessage, AgentError>> + Send + 'a>>;

pub trait Summarizer: Send + Sync {
    fn summarize<'a>(&'a self, messages: &'a [Message]) -> SummaryFuture<'a>;
}
