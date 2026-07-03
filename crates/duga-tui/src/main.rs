//! duga-tui: Terminal UI frontend for the duga agent.
//!
//! Launches an interactive ratatui-based terminal interface that connects
//! to the shared duga runtime, provides live agent progress, tool-call
//! visibility, cancellation, search, and confirmation UX.

use anyhow::{Context, Result};
use clap::Parser;
use duga_config::Config;
use std::path::PathBuf;

// All modules are declared in lib.rs

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

    /// Run an interactive OAuth login for a provider (e.g. "anthropic") and exit.
    #[arg(long, value_name = "PROVIDER")]
    login: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if let Some(provider) = &cli.login {
        anyhow::ensure!(
            provider == "anthropic",
            "OAuth login is not supported for provider '{provider}' (supported: anthropic)"
        );
        let path = duga_llm::oauth::default_credentials_path();
        return duga_llm::oauth::login_interactive(&path)
            .await
            .map_err(|e| anyhow::anyhow!("anthropic OAuth login failed: {e}"));
    }

    init_tracing(cli.verbose, &cli.replay_dir)?;

    let config = Config::load(&cli.config)
        .with_context(|| format!("loading config {}", cli.config.display()))?;

    tracing::info!(
        "duga-tui starting: model={} workspace={}",
        config.model,
        config.workspace.root.display()
    );

    // Enter the TUI event loop — this blocks until the user quits.
    duga_tui::terminal::run_tui(config, &cli.replay_dir).await
}

/// Initialize tracing. The TUI owns stdout (ratatui draws into it) and
/// stderr can leak into the alternate-screen viewport on some terminals,
/// so we route logs to a file under the replay dir instead of either of
/// those streams. The path can be overridden with DUGA_TUI_LOG.
fn init_tracing(verbose: bool, replay_dir: &std::path::Path) -> Result<()> {
    let filter = if verbose { "info" } else { "warn" };

    let log_path = match std::env::var("DUGA_TUI_LOG").ok() {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => replay_dir.join("duga-tui.log"),
    };

    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("opening TUI log file {}", log_path.display()))?;
    let writer = std::sync::Mutex::new(file);

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(filter)),
        )
        .with_ansi(false)
        .with_writer(writer)
        .try_init()
        .map_err(|e| anyhow::anyhow!("initializing tracing: {e}"))
}
