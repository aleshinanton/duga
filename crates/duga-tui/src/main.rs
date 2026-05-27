//! duga-tui: Terminal UI frontend for the duga agent.
//!
//! Launches an interactive ratatui-based terminal interface that connects
//! to the shared duga runtime, provides live agent progress, tool-call
//! visibility, cancellation, replay browsing, and confirmation UX.

mod app;
mod terminal;

use anyhow::{Context, Result};
use clap::Parser;
use duga_config::Config;
use duga_runtime::providers::{build_llm, resolve_provider};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "duga-tui",
    about = "Interactive terminal UI for the duga coding agent",
    after_help = "Examples:\n  duga-tui --config duga.yaml\n  duga-tui --config duga.yaml --replay-dir ./replays"
)]
struct Cli {
    /// Path to the duga YAML config file.
    #[arg(long, default_value = "duga.yaml")]
    config: PathBuf,

    /// Directory for reading/writing session replay logs.
    #[arg(long, default_value = "./replays")]
    replay_dir: PathBuf,

    /// Enable verbose tracing output.
    #[arg(short, long)]
    verbose: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose)?;

    let config = Config::load(&cli.config)
        .with_context(|| format!("loading config {}", cli.config.display()))?;

    let _selection = resolve_provider(&config)?;

    tracing::info!(
        "duga-tui starting: model={} workspace={}",
        config.model,
        config.workspace.root.display()
    );

    // Enter the TUI event loop — this blocks until the user quits.
    terminal::run_tui(config, &cli.replay_dir).await
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
