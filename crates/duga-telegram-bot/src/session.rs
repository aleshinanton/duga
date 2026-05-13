//! Per-chat session manager and cancellation.

use crate::auth::CallbackAction;
use crate::log::BotLogger;
use crate::runtime::{RunRequest, TelegramRuntime};
use crate::safety::PendingConfirmation;
use anyhow::Result;
use dashmap::DashMap;
use duga_config::TelegramConfig;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::ChatId;

#[derive(Default)]
pub struct ChatSessionState {
    pub active: bool,
    pub pending_confirmation: Option<PendingConfirmation>,
    pub cancelled: bool,
}

/// Manages per-chat sessions with one active run per chat.
pub struct SessionManager {
    sessions: DashMap<i64, ChatSessionState>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: DashMap::new(),
        }
    }

    /// Start an agent run for the given chat. Rejects if a run is already active.
    pub async fn start_run(
        self: Arc<Self>,
        chat_id: i64,
        request: RunRequest,
        bot: Bot,
        telegram_config: TelegramConfig,
        runtime: TelegramRuntime,
        bot_logger: Arc<BotLogger>,
    ) -> Result<()> {
        {
            let mut session = self.sessions.entry(chat_id).or_default();
            if session.active {
                bot.send_message(
                    ChatId(chat_id),
                    "⚠️ A task is already running. Use /stop to cancel it.",
                )
                .await?;
                return Ok(());
            }
            session.active = true;
            session.cancelled = false;
        }

        let manager = self.clone();
        let bot_clone = bot.clone();
        let logger_clone = bot_logger.clone();
        let config_clone = telegram_config.clone();

        tokio::spawn(async move {
            let result = runtime
                .run_task_for_chat(
                    chat_id,
                    request.task.clone(),
                    bot_clone.clone(),
                    &config_clone,
                    logger_clone.clone(),
                )
                .await;

            match result {
                Ok(final_text) => {
                    if !final_text.is_empty() {
                        let _ = bot_clone.send_message(ChatId(chat_id), &final_text).await;
                    }
                }
                Err(e) => {
                    let _ = bot_clone
                        .send_message(ChatId(chat_id), format!("❌ Error: {e}"))
                        .await;
                }
            }

            if let Some(mut session) = manager.sessions.get_mut(&chat_id) {
                session.active = false;
            }
        });

        Ok(())
    }

    /// Cancel the active run for the given chat.
    pub fn cancel(&self, chat_id: i64) {
        if let Some(mut session) = self.sessions.get_mut(&chat_id) {
            session.cancelled = true;
        }
    }

    /// Get the current status of a chat's session.
    pub fn status(&self, chat_id: i64) -> String {
        match self.sessions.get(&chat_id) {
            Some(session) if session.active => "Active run in progress.".to_string(),
            _ => "No active run.".to_string(),
        }
    }

    /// Get a memory summary for the chat.
    pub fn memory_summary(&self, chat_id: i64) -> String {
        match self.sessions.get(&chat_id) {
            Some(_) => "Memory is active for this chat.".to_string(),
            None => "No memory for this chat yet.".to_string(),
        }
    }

    /// Handle an inline callback action (approve/deny).
    pub async fn handle_callback(
        &self,
        chat_id: i64,
        action: CallbackAction,
        bot: Bot,
        _runtime: &TelegramRuntime,
    ) -> Result<()> {
        match action {
            CallbackAction::Approve(id) => {
                let pending = {
                    let mut session = self.sessions.get_mut(&chat_id);
                    session.as_mut().and_then(|s| s.pending_confirmation.take())
                };
                if let Some(pending) = pending {
                    if pending.confirmation_id == id {
                        let _ = pending.resolver.send(true);
                    }
                }
            }
            CallbackAction::Deny(id) => {
                let pending = {
                    let mut session = self.sessions.get_mut(&chat_id);
                    session.as_mut().and_then(|s| s.pending_confirmation.take())
                };
                if let Some(pending) = pending {
                    if pending.confirmation_id == id {
                        let _ = pending.resolver.send(false);
                    }
                }
            }
            CallbackAction::Unknown(data) => {
                tracing::warn!("unknown callback data: {data}");
                bot.send_message(ChatId(chat_id), "Unknown callback action.")
                    .await?;
            }
        }

        Ok(())
    }
}
