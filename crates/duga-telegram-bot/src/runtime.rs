//! Agent runtime integration for Telegram.
//!
//! Wires Telegram messages into the duga agent loop, attaching event bridges,
//! JSONL replay sinks, and Telegram-specific context.

use crate::log::BotLogger;
use crate::render::TelegramEventRenderer;
use crate::safety::TelegramConfirmationProvider;
use crate::session::SessionManager;
use anyhow::{Context, Result};
use duga_config::{Config, TelegramConfig};
use duga_events::{JsonlSink, RedactingSink};
use duga_runtime::events::FrontendEventBridge;
use duga_runtime::{
    build_agent, build_dispatcher, build_llm, resolve_provider, sandbox_environment_context,
    tool_guidance, ConfirmationMiddleware, ConfirmationPolicy,
};
use duga_sandbox::{CancellationToken, Workspace};
use std::sync::Arc;
use teloxide::prelude::*;

#[derive(Clone)]
pub struct TelegramRuntime {
    config: Config,
}

/// Request to run a task in a chat.
pub struct RunRequest {
    pub chat_id: i64,
    pub task: String,
}

impl TelegramRuntime {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    /// Run the agent loop for a chat message and return the final text.
    pub async fn run_task_for_chat(
        &self,
        chat_id: i64,
        task: String,
        bot: Bot,
        telegram_config: &TelegramConfig,
        _bot_logger: Arc<BotLogger>,
        session_manager: Arc<SessionManager>,
        cancellation: CancellationToken,
    ) -> Result<String> {
        let selection = resolve_provider(&self.config)?;
        let llm = build_llm(&selection.provider, &selection.model, &self.config)?;

        let workspace =
            Arc::new(Workspace::open(&self.config.workspace.root).context("opening workspace")?);

        let dispatcher = build_dispatcher(&self.config, workspace.clone())?;
        // When allow_all_binaries is enabled, bash confirmations are redundant —
        // the operator has already accepted the risk of arbitrary command execution.
        let require_confirmation: Vec<String> = if self.config.sandbox.allow_all_binaries {
            telegram_config
                .require_confirmation_for
                .iter()
                .filter(|t| t.as_str() != "bash")
                .cloned()
                .collect()
        } else {
            telegram_config.require_confirmation_for.clone()
        };
        if !require_confirmation.is_empty() {
            let policy = ConfirmationPolicy::new(
                require_confirmation,
                self.config.frontend.confirmation_timeout,
            );
            let provider = Arc::new(TelegramConfirmationProvider::new(
                bot.clone(),
                ChatId(chat_id),
                session_manager,
            ));
            dispatcher.set_confirmation(ConfirmationMiddleware::new(policy, provider));
        }

        // Set up event bridges.
        let (frontend_tx, frontend_bridge) = FrontendEventBridge::new(256);
        let frontend_sink = Arc::new(duga_runtime::events::FrontendEventSink::with_name(
            frontend_tx,
            "telegram",
        ));

        // JSONL replay sink for the chat.
        let chat_dir = telegram_config.data_dir.join(chat_id.to_string());
        tokio::fs::create_dir_all(&chat_dir).await?;
        let jsonl_path = chat_dir.join("session.jsonl");
        let jsonl_sink = Arc::new(JsonlSink::new(&jsonl_path)?);
        let replay_sink = Arc::new(RedactingSink::new(jsonl_sink));

        // Build frontend context for system prompt.
        let env_ctx = sandbox_environment_context(&self.config);
        let tool_guide = tool_guidance();
        let system_prompt = format!(
            "You are duga, a safe coding agent operating through Telegram.\n\
             Chat ID: {chat_id}\n\n\
             {env_ctx}\n\n\
             {tool_guide}\n\n\
             When facing a complex or multi-step problem, use the `think` tool first to \
             plan your approach before acting. This saves steps and produces better results.\n\
             Prefer `think` over running many small `bash` commands to explore the environment.\n\
             Be concise — Telegram messages have length limits."
        );

        let mut agent = build_agent(
            &self.config,
            llm,
            dispatcher,
            workspace,
            vec![frontend_sink, replay_sink],
            Some(system_prompt),
        )?;

        // Extract the inner receiver from the bridge for the renderer.
        let mut renderer_rx = frontend_bridge.into_inner();

        // Spawn the renderer task.
        let mut renderer = TelegramEventRenderer::new(bot.clone(), ChatId(chat_id));
        let renderer_handle = tokio::spawn(async move { renderer.run(&mut renderer_rx).await });

        let result = agent.run(task.clone(), cancellation).await;
        drop(agent);

        let _ = renderer_handle.await;

        match result {
            Ok(run_result) => {
                let text = run_result.message.text.unwrap_or_default();
                tracing::info!("chat {chat_id} run completed successfully");
                Ok(text)
            }
            Err(e) => {
                tracing::error!("chat {chat_id} run failed: {e}");
                Err(anyhow::anyhow!("Agent run failed: {e}"))
            }
        }
    }
}
