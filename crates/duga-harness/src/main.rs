mod cli;
mod signal;

use anyhow::{Context, Result};
use clap::Parser;
use cli::Cli;
use duga_config::Config;
use duga_core::{AgentLoop, Memory, Summarizer, SummaryFuture};
use duga_events::{JsonlSink, MultiSink, NullSink};
use duga_plugin_host::load_plugins;
use duga_runtime::providers::{build_llm, resolve_provider};
use duga_sandbox::{binary_registry::BinaryRegistry, CancellationToken, Workspace};
use duga_tools::{ErasedTool, ToolDispatcher};
use duga_types::llm::SummaryMessage;
use duga_types::message::Message;
use std::sync::Arc;

struct HarnessSummarizer;

impl Summarizer for HarnessSummarizer {
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

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose)?;

    let mut config = Config::load(&cli.config)
        .with_context(|| format!("loading config {}", cli.config.display()))?;
    if let Some(model) = &cli.model {
        config.model = model.clone();
    }
    if let Some(provider) = &cli.provider {
        config.provider = Some(provider.clone());
    }
    let selection = resolve_provider(&config)?;
    let task = cli
        .task_text()
        .context("task is required as a positional argument or --task")?
        .to_string();

    println!(
        "duga starting: provider={} model={} workspace={}",
        selection.provider,
        selection.model,
        config.workspace.root.display()
    );

    let workspace = Arc::new(Workspace::open(&config.workspace.root).context("opening workspace")?);
    let registry = Arc::new(
        BinaryRegistry::from_paths(&config.sandbox.allowed_binaries)
            .context("building binary registry")?,
    );
    let dispatcher = Arc::new(ToolDispatcher::new());
    duga_tools_builtin::register_builtin_tools(
        &dispatcher,
        registry,
        workspace.clone(),
        config.agent.output.clone(),
        config.sandbox.timeout,
        config.agent.think.clone(),
    )
    .context("registering built-in tools")?;

    let plugin_registry = load_plugins(
        &config.plugins.dir,
        &config.plugins.modules,
        &workspace,
        config.agent.output.clone(),
        config.sandbox.timeout,
    )
    .context("loading plugins")?;
    for plugin in plugin_registry.into_plugins() {
        dispatcher
            .register_erased(ErasedTool::erase(plugin))
            .context("registering plugin")?;
    }

    let event_sink = build_sinks(&cli, &config)?;
    let llm = build_llm(&selection.provider, &selection.model, &config)?;
    let memory = Memory::new(
        vec![Message::system("You are duga, a safe coding agent.")],
        config.memory.max_tokens,
        config.memory.compress_at_ratio,
    );
    let mut agent = AgentLoop::new(
        config.agent,
        memory,
        Arc::new(HarnessSummarizer),
        llm,
        dispatcher,
        (*workspace).clone(),
        event_sink,
    );

    let cancellation = CancellationToken::new();
    let signal_handle = signal::setup_signal_handler(cancellation.clone());
    let result = agent.run(task, cancellation).await;
    signal_handle.abort();

    match result {
        Ok(result) => {
            println!("{}", result.message.text.unwrap_or_default());
            Ok(())
        }
        Err(error) => Err(anyhow::anyhow!(error)),
    }
}

fn init_tracing(verbose: bool) -> Result<()> {
    let filter = if verbose { "info" } else { "warn" };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(filter)),
        )
        .try_init()
        .map_err(|e| anyhow::anyhow!("initializing tracing: {e}"))
}

fn build_sinks(cli: &Cli, _config: &Config) -> Result<Arc<dyn duga_events::EventSink>> {
    std::fs::create_dir_all(&cli.replay_dir)
        .with_context(|| format!("creating replay dir {}", cli.replay_dir.display()))?;
    let path = cli.replay_dir.join("session.jsonl");
    let jsonl = JsonlSink::new(path).context("opening replay event sink")?;
    let mut sinks = MultiSink::default();
    sinks.push(Arc::new(jsonl));
    sinks.push(Arc::new(NullSink));
    Ok(Arc::new(sinks))
}
