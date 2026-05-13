//! duga Telegram bot binary entry point.

use anyhow::{Context, Result};
use clap::Parser;
use duga_config::Config;
use duga_telegram_bot::bot;

/// duga Telegram bot — an LLM-powered coding agent over Telegram.
#[derive(Parser)]
#[command(name = "duga-telegram-bot", version, about)]
struct Cli {
    /// Path to YAML config file.
    #[arg(short, long, default_value = "duga.yaml")]
    config: std::path::PathBuf,

    /// Enable verbose logging.
    #[arg(short, long)]
    verbose: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize tracing.
    let filter = if cli.verbose {
        "duga_telegram_bot=debug,info"
    } else {
        "duga_telegram_bot=info,warn"
    };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .try_init()
        .map_err(|e| anyhow::anyhow!("initializing tracing: {e}"))?;

    // Load configuration.
    let config = Config::load(&cli.config)
        .with_context(|| format!("loading config {}", cli.config.display()))?;

    let telegram_config = config
        .telegram
        .as_ref()
        .context("telegram config section is required for duga-telegram-bot")?;

    // Resolve the bot token.
    let token = std::env::var(&telegram_config.token_env)
        .with_context(|| format!("reading token from env var '{}'", telegram_config.token_env))?;

    tracing::info!(
        "duga Telegram bot starting: allowed_chats={}, events_dir={}",
        telegram_config.allowed_chat_ids.len(),
        telegram_config.events_dir.display(),
    );

    bot::run(&config, &token).await
}
