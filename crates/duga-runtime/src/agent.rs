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
    /// Optional MCP adapter (set when `plugins.mcp` is configured).
    pub mcp_adapter: Option<Arc<duga_mcp::McpAdapter>>,
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

    /// Initialize all tools that need lifecycle setup (e.g., MCP connections).
    /// Call once at session start, before running any agent loop.
    pub async fn init_tools(&self) {
        if let Some(ref adapter) = self.mcp_adapter {
            adapter.session_start().await;
        }
    }

    /// Shut down all tools that need lifecycle cleanup (e.g., MCP connections).
    /// Call once at session end.
    pub async fn shutdown_tools(&self) {
        if let Some(ref adapter) = self.mcp_adapter {
            adapter.session_shutdown().await;
        }
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
    mcp_adapter: Option<Arc<duga_mcp::McpAdapter>>,
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
        let today = {
            use std::time::SystemTime;
            let now = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            // Convert to date: days since epoch → year-month-day
            let days = (now / 86400) as i64;
            let (y, m, d) = days_to_ymd(days);
            format!("{y:04}-{m:02}-{d:02}")
        };
        let workspace_dir = config.workspace.root.display();
        format!(
            "You are duga, a safe coding agent.\n\n\
             Current date: {today}\n\
             Working directory: {workspace_dir}\n\n\
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
        mcp_adapter,
    })
}

/// Convert days since Unix epoch to (year, month, day).
fn days_to_ymd(days: i64) -> (i64, u32, u32) {
    // Civil calendar algorithm from Howard Hinnant
    let z = days + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
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

#[cfg(test)]
mod tests {
    use super::*;
    use duga_config::SandboxMode;
    fn make_config(mode: SandboxMode) -> duga_config::Config {
        let mode_str = match mode {
            SandboxMode::Host => "host",
            SandboxMode::Capability => "capability",
            SandboxMode::Docker => "docker",
        };
        let dir = tempfile::tempdir().unwrap();
        let yaml = format!(
            r#"
model: "dummy/test"
agent:
  limits:
    max_steps: 5
    max_tool_calls: 10
    max_runtime: 60s
    retry_on_error: 1
  features:
    streaming: false
  output:
    max_stdout_bytes: 4096
    max_stderr_bytes: 4096
    max_combined_bytes: 8192
  think:
    max_calls: 2
    max_tokens: 128
sandbox:
  mode: {mode_str}
  timeout: 30s
  allowed_binaries:
    - /bin/echo
workspace:
  root: {workspace_root}
environment:
  allowed:
    - HOME
memory:
  max_tokens: 4096
  compress_at_ratio: 0.8
  context_window_size: 50
  max_context_tokens: 12000
  summarizer: simple
plugins:
  dir: {plugin_dir}
  modules: []
"#,
            workspace_root = dir.path().display(),
            plugin_dir = dir.path().display(),
        );
        let path = dir.path().join("config.yaml");
        std::fs::write(&path, yaml).unwrap();
        duga_config::Config::load(&path).expect("test config must parse")
    }

    #[test]
    fn sandbox_context_docker_mode() {
        let config = make_config(SandboxMode::Docker);
        let ctx = sandbox_environment_context(&config);
        assert!(ctx.contains("sandboxed container"));
        assert!(ctx.contains("apk") || ctx.contains("apt"));
    }

    #[test]
    fn sandbox_context_host_mode() {
        let config = make_config(SandboxMode::Host);
        let ctx = sandbox_environment_context(&config);
        assert!(ctx.contains("direct system access"));
    }

    #[test]
    fn sandbox_context_capability_mode() {
        let config = make_config(SandboxMode::Capability);
        let ctx = sandbox_environment_context(&config);
        assert!(ctx.contains("direct system access"));
    }

    #[test]
    fn tool_guidance_includes_all_required_tools() {
        let guidance = tool_guidance();
        assert!(guidance.contains("think"), "should mention think");
        assert!(guidance.contains("shell"), "should mention shell");
        assert!(guidance.contains("read"), "should mention read");
        assert!(guidance.contains("edit"), "should mention edit");
        assert!(guidance.contains("write"), "should mention write");
        assert!(guidance.contains("search"), "should mention search");
    }

    #[test]
    fn tool_guidance_requires_non_empty_labels() {
        let guidance = tool_guidance();
        assert!(guidance.contains("label"), "should mention label requirement");
        assert!(guidance.contains("non-empty"), "should require non-empty");
    }

    #[test]
    fn tool_guidance_is_static_str() {
        let a = tool_guidance();
        let b = tool_guidance();
        assert_eq!(a, b);
        assert!(!a.is_empty());
    }

    #[test]
    fn days_to_ymd_known_dates() {
        // 2025-01-01 = day 20089
        assert_eq!(super::days_to_ymd(20089), (2025, 1, 1));
        // 1970-01-01 = day 0
        assert_eq!(super::days_to_ymd(0), (1970, 1, 1));
        // 2000-02-29 = day 11016 (leap day)
        assert_eq!(super::days_to_ymd(11016), (2000, 2, 29));
    }

    #[tokio::test]
    async fn runtime_summarizer_produces_role_labels() {
        let summarizer = RuntimeSummarizer;
        let messages = vec![
            duga_types::message::Message::user("hello"),
            duga_types::message::Message::assistant(Some("hi".into()), vec![], None),
        ];
        let summary = summarizer.summarize(&messages).await.unwrap();
        assert!(summary.content.contains("Compressed context"));
        assert!(summary.content.contains("user"));
        assert!(summary.content.contains("assistant"));
    }

    #[tokio::test]
    async fn runtime_summarizer_respects_limit_of_16() {
        let summarizer = RuntimeSummarizer;
        let mut messages = Vec::new();
        for i in 0..20 {
            messages.push(duga_types::message::Message::user(format!("msg {}", i)));
        }
        let summary = summarizer.summarize(&messages).await.unwrap();
        // Only first 16 should be included
        let role_count = summary.content.matches("user").count();
        assert_eq!(role_count, 16, "should summarize at most 16 messages");
    }
}
