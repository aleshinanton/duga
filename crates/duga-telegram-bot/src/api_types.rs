//! Thin helpers for Telegram Rich Messages API (Bot API 10.1).
//!
//! Teloxide 0.17 predates the Rich Messages API.  We make direct HTTP calls
//! using the bot's public `api_url()` and `token()` — the same endpoint that
//! teloxide uses internally.

use serde::Serialize;
use teloxide::types::{Message, Recipient};
use teloxide::Bot;

// ── InputRichMessage ──────────────────────────────────────────────

/// Describes a rich message to be sent. Exactly one field must be set.
#[derive(Debug, Clone, Serialize)]
pub struct InputRichMessage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_rtl: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_entity_detection: Option<bool>,
}

impl InputRichMessage {
    pub fn markdown(text: impl Into<String>) -> Self {
        Self { markdown: Some(text.into()), html: None, is_rtl: None, skip_entity_detection: None }
    }
    pub fn html(text: impl Into<String>) -> Self {
        Self { html: Some(text.into()), markdown: None, is_rtl: None, skip_entity_detection: None }
    }
}

// ── API helpers ───────────────────────────────────────────────────

/// Send a rich message.  Returns the sent `Message` on success, or a string
/// describing the error (so callers can log and fall back).
pub async fn send_rich_message(
    bot: &Bot,
    chat_id: Recipient,
    rich_message: InputRichMessage,
) -> Result<Message, String> {
    let params = serde_json::json!({
        "chat_id": chat_id,
        "rich_message": rich_message,
    });
    call_api(bot, "sendRichMessage", &params).await
}

/// Stream a draft rich message.  Returns `true` on success.
///
/// The draft is ephemeral (30‑second preview); you must call
/// `sendRichMessage` with the final content to persist it.
pub async fn send_rich_message_draft(
    bot: &Bot,
    chat_id: i64,
    draft_id: u64,
    rich_message: InputRichMessage,
) -> Result<bool, String> {
    let params = serde_json::json!({
        "chat_id": chat_id,
        "draft_id": draft_id,
        "rich_message": rich_message,
    });
    call_api(bot, "sendRichMessageDraft", &params).await
}

/// Low-level JSON POST to `bot<token>/METHOD`.
async fn call_api<T: serde::de::DeserializeOwned>(
    bot: &Bot,
    method: &str,
    params: &serde_json::Value,
) -> Result<T, String> {
    let url = bot
        .api_url()
        .join(&format!("bot{}/{}", bot.token(), method))
        .map_err(|e| e.to_string())?;

    let client = reqwest::Client::new();
    let resp = client
        .post(url)
        .json(params)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;

    if body["ok"].as_bool() == Some(true) {
        serde_json::from_value(body["result"].clone())
            .map_err(|e| format!("deserializing result: {e}"))
    } else {
        let desc = body["description"].as_str().unwrap_or("unknown error");
        Err(desc.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_rich_message_markdown() {
        let m = InputRichMessage::markdown("**bold**");
        assert_eq!(m.markdown.as_deref(), Some("**bold**"));
        assert!(m.html.is_none());
    }

    #[test]
    fn input_rich_message_html() {
        let m = InputRichMessage::html("<b>x</b>");
        assert_eq!(m.html.as_deref(), Some("<b>x</b>"));
        assert!(m.markdown.is_none());
    }

    #[test]
    fn json_roundtrip() {
        let m = InputRichMessage::markdown("# Hi");
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["markdown"], "# Hi");
    }
}
