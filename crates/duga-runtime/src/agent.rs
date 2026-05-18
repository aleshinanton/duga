//! Agent assembly: memory, summarizer, sinks, and loop construction.
//!
//! All frontends (CLI, Telegram, TUI) use `build_agent` to wire up the
//! complete agent runtime from config and frontend-specific sinks.

use anyhow::Result;
use duga_config::{Config, SandboxMode};
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

/// Build sandbox environment context for the system prompt.
///
/// Tells the LLM what execution environment it's running in so it can
/// choose appropriate commands and package managers.
pub fn sandbox_environment_context(config: &Config) -> String {
    match config.sandbox.mode {
        SandboxMode::Docker => {
            "You are running in a sandboxed container environment.\n\
             System tools may be minimal — install what you need using the \
             available package manager (try `apk`, `apt`, or `yum`).\n\
             Check available commands with `which` or `command -v` before \
             assuming they exist. File system changes persist across sessions."
                .to_string()
        }
        SandboxMode::Capability | SandboxMode::Host => {
            "You are running with direct system access. Standard Unix tools \
             should be available. Use `bash` for exploration and execution."
                .to_string()
        }
    }
}

/// Build tool guidance for the system prompt.
///
/// Explicitly describes available tools and when to use each, so the LLM
/// doesn't need to rely solely on JSON Schema descriptions.
pub fn tool_guidance() -> &'static str {
    concat!(
        "## Available Tools\n",
        "- `think` — Reason through complex problems before acting. \
         Use this FIRST for multi-step tasks, analysis, or planning. \
         Saves steps and produces better results.\n",
        "- `bash` — Run shell commands. Use for file operations, \
         package installation, git, and system exploration.\n",
        "- `read` — Read file contents. Supports offset/limit for large files.\n",
        "- `write` — Create or overwrite files atomically.\n",
        "- `search` — Search workspace files with regex patterns.\n",
    )
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
        let env = sandbox_environment_context(config);
        let tools = tool_guidance();
        format!(
            "You are duga, a safe coding agent.\n\n\
             {env}\n\n\
             {tools}\n\n\
             When facing a complex or multi-step problem, use the `think` tool first to \
             plan your approach before acting. This saves steps and produces better results.\n\
             Prefer `think` over running many small `bash` commands to explore the environment.\n\
             You have a limited think budget per conversation — use it strategically for the \
             hardest parts of the task."
        )
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
