//! Agent assembly: memory, summarizer, sinks, and loop construction.
//!
//! All frontends (CLI, Telegram, TUI) use `build_agent` to wire up the
//! complete agent runtime from config and frontend-specific sinks.

use anyhow::Result;
use duga_config::Config;
use duga_core::{AgentLoop, Memory, Summarizer, SummaryFuture};
use duga_events::{EventSink, MultiSink};
use duga_llm::LlmClient;
use duga_sandbox::{CancellationToken, Workspace};
use duga_tools::ToolDispatcher;
use duga_types::llm::SummaryMessage;
use duga_types::message::Message;
use std::sync::Arc;

/// An assembled agent runtime ready to accept tasks.
pub struct BuiltRuntime {
    pub agent: AgentLoop,
    pub workspace: Arc<Workspace>,
    pub cancellation: CancellationToken,
}

/// Build a complete agent runtime from config and external dependencies.
///
/// Frontends provide their own LLM client, dispatcher, workspace, and event sinks.
/// This function constructs the memory, summarizer, and agent loop around them.
pub fn build_agent(
    config: &Config,
    llm: Arc<dyn LlmClient>,
    dispatcher: Arc<ToolDispatcher>,
    workspace: Arc<Workspace>,
    event_sinks: Vec<Arc<dyn EventSink>>,
    system_prompt: Option<String>,
) -> Result<AgentLoop> {
    let system_text = system_prompt.unwrap_or_else(|| {
        "You are duga, a safe coding agent. Use tools to accomplish the user's task."
            .to_string()
    });

    let memory = Memory::new(
        vec![Message::system(system_text)],
        config.memory.max_tokens,
        config.memory.compress_at_ratio,
    );

    let summarizer = Arc::new(RuntimeSummarizer);

    let event_sink = Arc::new(MultiSink::new(event_sinks));

    Ok(AgentLoop::new(
        config.agent.clone(),
        memory,
        summarizer,
        llm,
        dispatcher,
        (*workspace).clone(),
        event_sink,
    ))
}

/// A simple summarizer that compresses context by retaining recent role labels.
struct RuntimeSummarizer;

impl Summarizer for RuntimeSummarizer {
    fn summarize<'a>(&'a self, messages: &'a [Message]) -> SummaryFuture<'a> {
        Box::pin(async move {
            let mut content = String::from("Compressed context:");
            for message in messages.iter().take(16) {
                content.push_str("\n- ");
                content.push_str(&message.role.to_string());
            }
            Ok(SummaryMessage::new(content))
        })
    }
}
