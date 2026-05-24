//! Telegram confirmation UI for tool execution approval.
//!
//! Sends inline keyboard prompts for risky tool calls (shell, write)
//! and resolves approval/denial through callback buttons.

use crate::session::SessionManager;
use duga_runtime::confirmation::{ConfirmationDecision, ConfirmationProvider, ConfirmationRequest};
use std::sync::Arc;
use std::time::Duration;
use teloxide::prelude::*;
use teloxide::types::{ChatId, InlineKeyboardButton, InlineKeyboardMarkup};
use tokio::sync::oneshot;

/// A pending confirmation awaiting user response.
pub struct PendingConfirmation {
    pub confirmation_id: String,
    pub resolver: oneshot::Sender<bool>,
}

/// Send a confirmation prompt with inline [Approve] and [Deny] buttons.
pub async fn send_confirmation_prompt(
    bot: &Bot,
    chat_id: ChatId,
    request: &ConfirmationRequest,
) -> Result<Message, teloxide::RequestError> {
    let label = if let Some(ref args) = request.arguments {
        format!("{}\n```\n{}\n```", request.label, args)
    } else {
        request.label.clone()
    };

    let keyboard = InlineKeyboardMarkup::new(vec![vec![
        InlineKeyboardButton::callback(
            "✅ Approve",
            format!("approve:{}", request.confirmation_id),
        ),
        InlineKeyboardButton::callback("❌ Deny", format!("deny:{}", request.confirmation_id)),
    ]]);

    bot.send_message(chat_id, format!("⚠️ Allow this action?\n\n{label}"))
        .reply_markup(keyboard)
        .await
}

pub struct TelegramConfirmationProvider {
    bot: Bot,
    chat_id: ChatId,
    session_manager: Arc<SessionManager>,
}

impl TelegramConfirmationProvider {
    pub fn new(bot: Bot, chat_id: ChatId, session_manager: Arc<SessionManager>) -> Self {
        Self {
            bot,
            chat_id,
            session_manager,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use duga_runtime::confirmation::ConfirmationRequest;

    #[test]
    fn confirmation_request_serialisation_roundtrip() {
        let req = ConfirmationRequest {
            confirmation_id: "abc-123".into(),
            tool_name: "shell".into(),
            label: "Run dangerous command".into(),
            arguments: Some("rm -rf /".into()),
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: ConfirmationRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.confirmation_id, "abc-123");
        assert_eq!(parsed.tool_name, "shell");
        assert_eq!(parsed.label, "Run dangerous command");
        assert_eq!(parsed.arguments, Some("rm -rf /".into()));
    }

    #[test]
    fn confirmation_request_without_arguments() {
        let req = ConfirmationRequest {
            confirmation_id: "xyz".into(),
            tool_name: "edit".into(),
            label: "Edit file".into(),
            arguments: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: ConfirmationRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.arguments, None);
    }
}

#[async_trait::async_trait]
impl ConfirmationProvider for TelegramConfirmationProvider {
    async fn confirm(
        &self,
        request: &ConfirmationRequest,
        timeout: Duration,
    ) -> ConfirmationDecision {
        let (tx, rx) = oneshot::channel();
        let pending = PendingConfirmation {
            confirmation_id: request.confirmation_id.clone(),
            resolver: tx,
        };

        if !self
            .session_manager
            .set_pending_confirmation(self.chat_id.0, pending)
        {
            tracing::warn!(
                chat_id = self.chat_id.0,
                confirmation_id = %request.confirmation_id,
                "unable to register pending confirmation"
            );
            return ConfirmationDecision::Denied;
        }

        if let Err(error) = send_confirmation_prompt(&self.bot, self.chat_id, request).await {
            self.session_manager
                .clear_pending_confirmation(self.chat_id.0, &request.confirmation_id);
            tracing::warn!(
                chat_id = self.chat_id.0,
                confirmation_id = %request.confirmation_id,
                error = %error,
                "unable to send confirmation prompt"
            );
            return ConfirmationDecision::Denied;
        }

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(true)) => ConfirmationDecision::Approved,
            Ok(Ok(false)) => ConfirmationDecision::Denied,
            Ok(Err(_)) => ConfirmationDecision::Denied,
            Err(_) => {
                self.session_manager
                    .clear_pending_confirmation(self.chat_id.0, &request.confirmation_id);
                ConfirmationDecision::Timeout
            }
        }
    }
}
