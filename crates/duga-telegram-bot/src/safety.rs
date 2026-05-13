//! Telegram confirmation UI for tool execution approval.
//!
//! Sends inline keyboard prompts for risky tool calls (bash, write)
//! and resolves approval/denial through callback buttons.

use duga_runtime::confirmation::ConfirmationRequest;
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
        InlineKeyboardButton::callback(
            "❌ Deny",
            format!("deny:{}", request.confirmation_id),
        ),
    ]]);

    bot.send_message(chat_id, format!("⚠️ Allow this action?\n\n{label}"))
        .reply_markup(keyboard)
        .await
}
