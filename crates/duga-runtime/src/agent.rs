//! Agent assembly: memory, summarizer, sinks, and loop construction.
//!
//! All frontends (CLI, Telegram, TUI) use `build_agent` to wire up the
//! complete agent runtime from config and frontend-specific sinks.

use anyhow::Result;
use duga_config::{Config, SandboxMode, SummarizerKind};
use duga_core::loops::SimpleReActLoop;
use duga_core::{LoopRegistry, Memory, Summarizer};
use duga_events::{EventSink, MultiSink};
use duga_llm::LlmClient;
use duga_sandbox::{CancellationToken, Workspace};
use duga_tools::ToolDispatcher;
use duga_types::message::Message;
use std::sync::Arc;

use crate::summarizer::SemanticSummarizer;

/// An assembled agent runtime ready to accept tasks.
pub struct BuiltRuntime {
    pub workspace: Arc<Workspace>,
    pub cancellation: CancellationToken,
    pub registry: Arc<LoopRegistry>,
    pub memory: Memory,
    pub summarizer: Arc<dyn Summarizer>,
    pub llm: Arc<dyn LlmClient>,
    pub dispatcher: Arc<ToolDispatcher>,
    pub event_sink: Arc<dyn EventSink>,
    pub config: Config,
}

impl BuiltRuntime {
    /// Read-only access to conversation memory.
    pub fn memory(&self) -> &Memory {
        &self.memory
    }

    /// Mutable access to conversation memory (for pre-run history restoration).
    pub fn memory_mut(&mut self) -> &mut Memory {
        &mut self.memory
    }
}

/// Build sandbox environment context for the system prompt.
///
/// Tells the LLM what execution environment it's running in so it can
/// choose appropriate commands and package managers.
pub fn sandbox_environment_context(config: &Config) -> String {
    match config.sandbox.mode {
        SandboxMode::Docker => "You are running in a sandboxed container environment.\n\
             System tools may be minimal — install what you need using the \
             available package manager (try `apk`, `apt`, or `yum`).\n\
             Check available commands with `which` or `command -v` before \
             assuming they exist. File system changes persist across sessions."
            .to_string(),
        SandboxMode::Capability | SandboxMode::Host => {
            "You are running with direct system access. Standard Unix tools \
             should be available. Use `shell` for exploration and execution."
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
        "- `shell` — Run shell commands. Use for file operations, \
         package installation, git, and system exploration.\n",
        "- `read` — Read file contents. Supports offset/limit for large files.\n",
        "- `edit` — Perform targeted text replacements in existing files. \
         Prefer this over `write` for small changes.\n",
        "- `write` — Create or overwrite files atomically.\n",
        "- `search` — Search workspace files with regex patterns.\n",
        "\n## Tool Call Labels\n",
        "Every tool call (`shell`, `edit`, `read`, `write`, `search`, `think`) \
         MUST include a non-empty `label` parameter. The label is a short, \
         human-readable description of what this step does. Never leave it \
         empty or omit it.\n",
        "\n## Lightweight Tool-Use Gate\n",
        "Before acting, analyze the request, decide which tools are actually \
         needed, skip unnecessary tool calls, and execute only the minimum set \
         of tools required.\n",
        "Use `think` for multi-step tasks; before HTTP requests to decide \
         whether an API call is worthwhile; before file searches to decide \
         exactly what to look for; before package installs to check whether \
         the dependency is already installed; and when intent is ambiguous, \
         clarify first.\n",
        "Avoid high-tax exploration such as running several commands just to \
         figure out what to do.\n",
    )
}

/// Build a complete agent runtime from config and external dependencies.
///
/// Frontends provide their own LLM client, dispatcher, workspace, and event sinks.
/// This function constructs the memory, summarizer, loop registry, and all wiring.
pub fn build_agent(
    config: &Config,
    llm: Arc<dyn LlmClient>,
    dispatcher: Arc<ToolDispatcher>,
    workspace: Arc<Workspace>,
    event_sinks: Vec<Arc<dyn EventSink>>,
    system_prompt: Option<String>,
) -> Result<BuiltRuntime> {
    // Build loop registry first — needed for system prompt generation.
    let mut registry = LoopRegistry::new();
    registry
        .register(Box::new(SimpleReActLoop))
        .expect("SimpleReActLoop must register successfully");
    duga_core::loops::register_default_loops(&mut registry);
    let registry = Arc::new(registry);

    let system_text = system_prompt.unwrap_or_else(|| {
        let env = sandbox_environment_context(config);
        let tools = tool_guidance();
        let strategies = registry.build_strategies_prompt(
            &config.agent.loop_config.enabled_loops,
        );
        format!(
            "You are duga, a safe coding agent.\n\n\
             {env}\n\n\
             {tools}\n\n\
             {strategies}\n\n\
             When facing a complex or multi-step problem, use the `think` tool first to \
             plan your approach before acting. This saves steps and produces better results.\n\
             Prefer `think` over running many small `shell` commands to explore the environment.\n\
             You have a limited think budget per conversation — use it strategically for the \
             hardest parts of the task."
        )
    });

    let memory = Memory::new(
        vec![Message::system(system_text)],
        config.memory.max_tokens,
        config.memory.compress_at_ratio,
        config.memory.context_window_size,
        config.memory.max_context_tokens,
    );

    // Dispatch summarizer based on config.
    let summarizer: Arc<dyn Summarizer> = match config.memory.summarizer {
        SummarizerKind::Simple => Arc::new(RuntimeSummarizer),
        SummarizerKind::Semantic => Arc::new(SemanticSummarizer::new(llm.clone())),
    };

    let event_sink: Arc<dyn EventSink> = Arc::new(MultiSink::new(event_sinks));

    Ok(BuiltRuntime {
        workspace,
        cancellation: CancellationToken::new(),
        registry,
        memory,
        summarizer,
        llm,
        dispatcher,
        event_sink,
        config: config.clone(),
    })
}

/// A simple summarizer that compresses context by retaining recent role labels.
///
/// Preserved for fallback; the default summarizer is now `SemanticSummarizer`.
struct RuntimeSummarizer;

impl Summarizer for RuntimeSummarizer {
    fn summarize<'a>(&'a self, messages: &'a [Message]) -> duga_core::SummaryFuture<'a> {
        Box::pin(async move {
            let mut content = String::from("Compressed context:");
            for message in messages.iter().take(16) {
                content.push_str("\n- ");
                content.push_str(&message.role.to_string());
            }
            Ok(duga_types::llm::SummaryMessage::new(content))
        })
    }
}
