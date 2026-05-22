//! Telegram bot startup and update routing.

use crate::auth::{is_allowed_chat, parse_callback_data, resolve_chat_id, strip_mention};
use crate::log::BotLogger;
use crate::runtime::{RunRequest, TelegramRuntime};
use crate::session::SessionManager;
use anyhow::Result;
use duga_config::Config;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{ChatId, Message};

/// Start the bot poll loop.
pub async fn run(config: &Config, token: &str) -> Result<()> {
    let bot = Bot::new(token);
    let bot_username = bot.get_me().await?.username().to_string();

    let telegram_config = config.telegram.as_ref().unwrap().clone();

    let session_manager = Arc::new(SessionManager::new());
    let bot_logger = Arc::new(BotLogger::new(telegram_config.data_dir.clone()));
    let runtime = Arc::new(TelegramRuntime::new(config.clone()));

    let schema = dptree::entry()
        .branch(
            Update::filter_message().endpoint({
                let session_manager = session_manager.clone();
                let telegram_config = telegram_config.clone();
                let bot_logger = bot_logger.clone();
                let runtime = runtime.clone();
                let bot_username = bot_username.clone();

                move |bot_moved: Bot, msg: Message| {
                    let bot = bot_moved.clone();
                    let session_manager = session_manager.clone();
                    let telegram_config = telegram_config.clone();
                    let bot_logger = bot_logger.clone();
                    let runtime = runtime.clone();
                    let bot_username = bot_username.clone();

                    async move {
                        handle_message(
                            bot,
                            msg,
                            session_manager,
                            telegram_config,
                            bot_logger,
                            runtime,
                            bot_username,
                        )
                        .await;
                        Ok::<_, anyhow::Error>(())
                    }
                }
            }),
        )
        .branch(
            Update::filter_callback_query().endpoint({
                let session_manager = session_manager.clone();
                let telegram_config = telegram_config.clone();
                let runtime = runtime.clone();

                move |bot_moved: Bot, cb: CallbackQuery| {
                    let bot = bot_moved.clone();
                    let session_manager = session_manager.clone();
                    let telegram_config = telegram_config.clone();
                    let runtime = runtime.clone();

                    async move {
                        handle_callback_query(bot, cb, session_manager, telegram_config, runtime)
                            .await;
                        Ok::<_, anyhow::Error>(())
                    }
                }
            }),
        );

    tracing::info!("bot @{bot_username} started, polling for updates…");
    Dispatcher::builder(bot, schema)
        .dependencies(dptree::deps![])
        .build()
        .dispatch()
        .await;

    Ok(())
}

async fn handle_message(
    bot: Bot,
    msg: Message,
    session_manager: Arc<SessionManager>,
    telegram_config: duga_config::TelegramConfig,
    bot_logger: Arc<BotLogger>,
    runtime: Arc<TelegramRuntime>,
    bot_username: String,
) {
    let chat_id = resolve_chat_id(&msg).unwrap_or(msg.chat.id);
    let chat_id_i64 = chat_id.0;

    let text = msg.text().map(|t| t.to_string());

    // Check authorization before commands or task handling.
    let sender = msg.from.as_ref();
    if !is_allowed_chat(&telegram_config, chat_id_i64, sender) {
        tracing::warn!("unauthorized chat {chat_id_i64}");
        return;
    }

    if let Some(ref t) = text {
        if t.starts_with('/') {
            handle_command(&bot, chat_id, t, &session_manager).await;
            return;
        }
        // Also recognise plain "stop"/"cancel" (case-insensitive) as /stop.
        if is_cancel_text(t) {
            handle_command(&bot, chat_id, "/stop", &session_manager).await;
            return;
        }
    }

    // Always download attachments when present, even if the message also has text.
    let attachment_summary = if telegram_config.attachments.enabled
        && (msg.photo().is_some()
            || msg.document().is_some()
            || msg.audio().is_some()
            || msg.voice().is_some()
            || msg.video().is_some())
    {
        let chat_dir = telegram_config
            .data_dir
            .join(chat_id_i64.to_string())
            .join("attachments");
        let _ = tokio::fs::create_dir_all(&chat_dir).await;
        match crate::attachments::download_attachments(&bot, &msg, &chat_dir, &telegram_config).await {
            Ok(files) if !files.is_empty() => {
                let summary = crate::attachments::summarize_attachments(&files);
                let _ = bot
                    .send_message(chat_id, format!("📎 Downloaded {} attachment(s)", files.len()))
                    .await;
                summary
            }
            _ => String::new(),
        }
    } else {
        String::new()
    };

    let task = match text {
        Some(ref t) => {
            let stripped = strip_mention(t, &bot_username);
            if attachment_summary.is_empty() {
                stripped.to_string()
            } else {
                format!("{}\n\n{}", attachment_summary, stripped)
            }
        }
        None => {
            if attachment_summary.is_empty() {
                // No text and no attachments.
                let _ = bot
                    .send_message(chat_id, "No supported attachments found in this message.")
                    .await;
                return;
            }
            attachment_summary
        }
    };

    // Log the incoming message.
    let _ = bot_logger
        .log_message(chat_id_i64, &msg, 0, true)
        .await;

    // Run the agent.
    let request = RunRequest {
        chat_id: chat_id_i64,
        task: task.to_string(),
    };

    let _ = session_manager
        .clone()
        .start_run(
            chat_id_i64,
            request,
            bot.clone(),
            telegram_config.clone(),
            (*runtime).clone(),
            bot_logger.clone(),
        )
        .await;
}

async fn handle_callback_query(
    bot: Bot,
    cb: CallbackQuery,
    session_manager: Arc<SessionManager>,
    telegram_config: duga_config::TelegramConfig,
    runtime: Arc<TelegramRuntime>,
) {
    let chat_id = cb.message.as_ref().map(|m| m.chat().id.0).unwrap_or(0);
    let sender = Some(&cb.from);
    if !is_allowed_chat(&telegram_config, chat_id, sender) {
        return;
    }

    if let Some(data) = &cb.data {
        let action = parse_callback_data(data);
        let _ = session_manager
            .handle_callback(chat_id, action, bot, &runtime)
            .await;
    }
}

async fn handle_command(
    bot: &Bot,
    chat_id: ChatId,
    text: &str,
    session_manager: &SessionManager,
) {
    let command = text
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_start_matches('/');

    let reply = match command {
        "start" => "👋 duga Telegram bot is ready.\n\nSend a message to start a task. Use /help for commands.".to_string(),
        "help" => "Commands:\n\
                   /start — Start the bot\n\
                   /help — Show this help\n\
                   /stop — Cancel the current task\n\
                   /status — Show current task status\n\
                   /memory — Show current memory\n\
                   /skills — List available skills\n\
                   /events — Show recent events"
            .to_string(),
        "stop" => {
            session_manager.cancel(chat_id.0);
            "🛑 Cancelling current task…".to_string()
        }
        "status" => {
            let status = session_manager.status(chat_id.0);
            format!("📊 {status}")
        }
        "memory" => {
            let memory = session_manager.memory_summary(chat_id.0);
            format!("🧠 {memory}")
        }
        "skills" => "📚 Skills loaded from workspace and channel directories.".to_string(),
        "events" => "📋 Events are written to the replay JSONL file.".to_string(),
        other => format!("Unknown command: /{other}\nUse /help for available commands."),
    };

    let _ = bot.send_message(chat_id, reply).await;
}

/// Returns `true` if the text is a plain cancel/stop request (case-insensitive).
/// This catches "stop" and "cancel" without requiring the `/` prefix.
pub fn is_cancel_text(text: &str) -> bool {
    let lowered = text.trim().to_lowercase();
    lowered == "stop" || lowered == "cancel"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_cancel_text_lowercase() {
        assert!(is_cancel_text("stop"));
        assert!(is_cancel_text("cancel"));
    }

    #[test]
    fn is_cancel_text_case_insensitive() {
        assert!(is_cancel_text("Stop"));
        assert!(is_cancel_text("STOP"));
        assert!(is_cancel_text("Cancel"));
        assert!(is_cancel_text("CANCEL"));
        assert!(is_cancel_text("StOp"));
        assert!(is_cancel_text("cAnCeL"));
    }

    #[test]
    fn is_cancel_text_with_whitespace() {
        assert!(is_cancel_text("  stop  "));
        assert!(is_cancel_text("\tstop\n"));
        assert!(is_cancel_text(" cancel "));
    }

    #[test]
    fn is_cancel_text_rejects_non_cancel() {
        assert!(!is_cancel_text("hello"));
        assert!(!is_cancel_text("/stop"));     // handled by / prefix check
        assert!(!is_cancel_text("stopping"));
        assert!(!is_cancel_text("stopp"));
        assert!(!is_cancel_text(""));
    }

    #[test]
    fn is_cancel_text_rejects_partial_matches() {
        assert!(!is_cancel_text("stopping"));
        assert!(!is_cancel_text("stop it"));
        assert!(!is_cancel_text("cancelling"));
        assert!(!is_cancel_text("please stop"));
    }
}
