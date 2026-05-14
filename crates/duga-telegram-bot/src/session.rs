//! Per-chat session manager and cancellation.

use crate::auth::CallbackAction;
use crate::log::BotLogger;
use crate::runtime::{RunRequest, TelegramRuntime};
use crate::safety::PendingConfirmation;
use anyhow::Result;
use dashmap::DashMap;
use duga_config::TelegramConfig;
use duga_sandbox::CancellationToken;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::ChatId;

#[derive(Default)]
pub struct ChatSessionState {
    pub active: bool,
    pub pending_confirmation: Option<PendingConfirmation>,
    pub cancellation: Option<CancellationToken>,
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
        let cancellation = CancellationToken::new();
        let already_active = {
            let mut session = self.sessions.entry(chat_id).or_default();
            if session.active {
                true
            } else {
                session.active = true;
                session.pending_confirmation = None;
                session.cancellation = Some(cancellation.clone());
                session.cancelled = false;
                false
            }
        };

        if already_active {
            bot.send_message(
                ChatId(chat_id),
                "⚠️ A task is already running. Use /stop to cancel it.",
            )
            .await?;
            return Ok(());
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
                    manager.clone(),
                    cancellation,
                )
                .await;

            match result {
                Ok(_final_text) => {
                    // Final answer is already sent by the TelegramEventRenderer
                    // via the RunFinished event (see render.rs::finalize_process_message).
                }
                Err(e) => {
                    let _ = bot_clone
                        .send_message(ChatId(chat_id), format!("❌ Error: {e}"))
                        .await;
                }
            }

            if let Some(mut session) = manager.sessions.get_mut(&chat_id) {
                session.active = false;
                session.pending_confirmation = None;
                session.cancellation = None;
            }
        });

        Ok(())
    }

    /// Cancel the active run for the given chat.
    pub fn cancel(&self, chat_id: i64) {
        if let Some(mut session) = self.sessions.get_mut(&chat_id) {
            session.cancelled = true;
            if let Some(cancellation) = &session.cancellation {
                cancellation.cancel();
            }
            if let Some(pending) = session.pending_confirmation.take() {
                let _ = pending.resolver.send(false);
            }
        }
    }

    pub fn set_pending_confirmation(&self, chat_id: i64, pending: PendingConfirmation) -> bool {
        let Some(mut session) = self.sessions.get_mut(&chat_id) else {
            return false;
        };
        if !session.active || session.pending_confirmation.is_some() {
            return false;
        }
        session.pending_confirmation = Some(pending);
        true
    }

    pub fn clear_pending_confirmation(&self, chat_id: i64, confirmation_id: &str) -> bool {
        let Some(mut session) = self.sessions.get_mut(&chat_id) else {
            return false;
        };
        let matches = session
            .pending_confirmation
            .as_ref()
            .is_some_and(|pending| pending.confirmation_id == confirmation_id);
        if matches {
            session.pending_confirmation = None;
        }
        matches
    }

    fn resolve_pending_confirmation(
        &self,
        chat_id: i64,
        confirmation_id: &str,
        approved: bool,
    ) -> bool {
        let Some(mut session) = self.sessions.get_mut(&chat_id) else {
            return false;
        };
        let matches = session
            .pending_confirmation
            .as_ref()
            .is_some_and(|pending| pending.confirmation_id == confirmation_id);
        if !matches {
            return false;
        }
        if let Some(pending) = session.pending_confirmation.take() {
            let _ = pending.resolver.send(approved);
        }
        true
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
                self.resolve_pending_confirmation(chat_id, &id, true);
            }
            CallbackAction::Deny(id) => {
                self.resolve_pending_confirmation(chat_id, &id, false);
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
