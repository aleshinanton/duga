mod cli;
mod signal;

use anyhow::{Context, Result};
use clap::Parser;
use cli::Cli;
use duga_config::Config;
use duga_core::loop_context::LoopContext;
use duga_core::loops::SimpleReActLoop;
use duga_core::Loop;
use duga_events::{JsonlSink, MultiSink, NullSink};
use duga_runtime::{
    build_agent, build_dispatcher,
    providers::{build_llm, resolve_provider},
};
use duga_sandbox::{CancellationToken, Workspace};
use std::sync::Arc;

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
    let dispatcher = build_dispatcher(&config, workspace.clone(), vec![])?;

    let event_sink = build_sinks(&cli, &config)?;
    let llm = build_llm(&selection.provider, &selection.model, &config)?;

    // Use the shared runtime builder.
    let mut runtime = build_agent(
        &config,
        llm.clone(),
        dispatcher.clone(),
        workspace.clone(),
        vec![event_sink],
        None,
    )?;

    let cancellation = CancellationToken::new();
    let signal_handle = signal::setup_signal_handler(cancellation.clone());

    // Build LoopContext and run via SimpleReActLoop.
    let loop_impl = SimpleReActLoop;
    let agent_config = config.agent.clone();
    let loop_config = &agent_config.loop_config;
    let mut ctx = LoopContext {
        config: &agent_config,
        memory: &mut runtime.memory,
        llm: &runtime.llm,
        tools: &runtime.dispatcher,
        workspace: &runtime.workspace,
        event_sink: &runtime.event_sink,
        summarizer: &runtime.summarizer,
        cancellation: &cancellation,
        registry: &runtime.registry,
        max_refinement_iterations: loop_config.max_refinement_iterations,
        max_delegation_depth: loop_config.max_delegation_depth,
        delegation_depth: 0,
        steer: None,
        steer_limits: None,    };

    let result = loop_impl.run(task, &mut ctx).await;
    signal_handle.abort();

    match result {
        Ok(result) => {
            if result.loop_id != "simple_react" {
                println!("⟳ via {} loop", result.loop_id);
            }
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
