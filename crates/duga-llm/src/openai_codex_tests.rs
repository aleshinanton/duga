use super::*;
use crate::openai_oauth::{OpenAiTokens, save_tokens};
use duga_events::{EventFuture, NullSink};
use std::sync::Mutex;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path},
};

#[derive(Default)]
struct RecordingSink(Mutex<Vec<Event>>);
impl EventSink for RecordingSink {
    fn emit<'a>(&'a self, event: Event) -> EventFuture<'a> {
        Box::pin(async move {
            self.0.lock().unwrap().push(event);
            Ok(())
        })
    }
}
fn manager(dir: &tempfile::TempDir) -> OpenAiOAuthManager {
    let path = dir.path().join("auth.json");
    save_tokens(
        &path,
        &OpenAiTokens {
            access_token: "access-secret".into(),
            refresh_token: "refresh-secret".into(),
            account_id: "account-1".into(),
            expires_at_ms: u64::MAX,
        },
    )
    .unwrap();
    OpenAiOAuthManager::load(path).unwrap()
}
fn response(output: Value) -> Value {
    json!({"status": "completed", "output": output, "usage": {"input_tokens": 11, "output_tokens": 7}})
}
fn sse(completed: Value) -> String {
    format!(
        "data: {}\n\ndata: {}\n\ndata: {}\n\n",
        json!({"type":"response.output_text.delta", "delta":"Привет"}),
        json!({"type":"response.reasoning_summary_text.delta", "delta":"Summary"}),
        json!({"type":"response.completed", "response": completed})
    )
}
#[test]
fn decoder_preserves_split_unicode_crlf_and_multiline_events() {
    let input =
        "event: response.output_text.delta\r\ndata: {\r\ndata: \"delta\":\"Привет 👋\"}\r\n\r\n";
    let mut decoder = SseDecoder::default();
    let mut frames = Vec::new();
    for byte in input.as_bytes() {
        frames.extend(decoder.push(&[*byte]).unwrap());
    }
    assert_eq!(frames.len(), 1);
    let value: Value = serde_json::from_slice(&frames[0]).unwrap();
    assert_eq!(value["delta"], "Привет 👋");
}
#[test]
fn decoder_rejects_oversized_frames() {
    let mut decoder = SseDecoder::default();
    assert!(decoder.push(&vec![b'x'; MAX_FRAME_BYTES + 1]).is_err());
}
#[test]
fn request_roundtrip_has_stable_tool_ids_and_no_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let mut client = OpenAiCodexClient::new(
        "gpt-5.1-codex",
        Arc::new(manager(&dir)),
        ThinkingLevel::High,
    );
    let call = ToolCall::new("read", json!({"path": "hello.txt"}));
    let messages = vec![
        Message::system("system instructions"),
        Message::user("hello"),
        Message::assistant(None, vec![call.clone()], None),
        Message::tool(call.id.as_uuid(), "result".into()),
    ];
    let tools = [ToolSchema::new(
        "read",
        "Read a file",
        json!({"type":"object"}),
    )];
    let body = client.request_body(&messages, &tools).unwrap();
    assert_eq!(body["store"], false);
    assert_eq!(body["stream"], true);
    assert_eq!(body["instructions"], "system instructions");
    assert_eq!(body["input"][1]["call_id"], body["input"][2]["call_id"]);
    assert_eq!(body["input"][2]["output"], "result");
    assert_eq!(body["tools"][0]["name"], "read");
    assert_eq!(body["reasoning"]["effort"], "high");
    assert!(!body.to_string().contains("access-secret"));
    client.thinking = ThinkingLevel::Max;
    assert_eq!(
        client.request_body(&messages, &tools).unwrap()["reasoning"]["effort"],
        "xhigh"
    );
}
#[test]
fn completed_response_parses_tools_reasoning_usage_and_rejects_invalid_arguments() {
    let parsed = completed_response(&response(json!([
        {"type":"reasoning", "summary":[{"type":"summary_text", "text":"Reasoning"}]},
        {"type":"message", "content":[{"type":"output_text", "text":"Hello"}]},
        {"type":"function_call", "call_id":"provider-id", "name":"read", "arguments":"{\"path\":\"a.txt\"}"}
    ]))).unwrap();
    assert_eq!(parsed.message.text.as_deref(), Some("Hello"));
    assert_eq!(
        parsed.message.reasoning_content.as_deref(),
        Some("Reasoning")
    );
    assert_eq!(parsed.message.tool_calls[0].raw_args["path"], "a.txt");
    assert_eq!(parsed.usage.prompt, 11);
    assert_eq!(parsed.usage.completion, 7);
    assert!(
        completed_response(&response(
            json!([{"type":"function_call", "name":"read", "arguments":"{"}])
        ))
        .is_err()
    );
    assert!(completed_response(&json!({"status":"incomplete", "output":[]})).is_err());
}
#[tokio::test]
async fn inference_uses_codex_endpoint_headers_and_respects_streaming_option() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let mut client =
        OpenAiCodexClient::new("gpt-5.1-codex", Arc::new(manager(&dir)), ThinkingLevel::Off);
    client.endpoint = format!("{}/codex/responses", server.uri());
    Mock::given(method("POST")).and(path("/codex/responses"))
        .and(header("authorization", "Bearer access-secret"))
        .and(header("chatgpt-account-id", "account-1"))
        .and(header("originator", "duga"))
        .respond_with(ResponseTemplate::new(200).insert_header("content-type", "text/event-stream")
            .set_body_string(sse(response(json!([{"type":"message", "content":[{"type":"output_text", "text":"Привет"}]}])))))
        .expect(2).mount(&server).await;
    for streaming in [true, false] {
        let sink = RecordingSink::default();
        let result = client
            .chat(
                &[Message::user("hello")],
                &[],
                LlmCallOptions { streaming },
                &sink,
            )
            .await
            .unwrap();
        assert_eq!(result.message.text.as_deref(), Some("Привет"));
        let events = sink.0.lock().unwrap();
        assert_eq!(events.len(), if streaming { 2 } else { 0 });
        assert!(!format!("{events:?}").contains("access-secret"));
    }
}
#[tokio::test]
async fn truncated_or_failed_stream_is_not_reported_as_success() {
    let server = MockServer::start().await;
    for (index, body) in [
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
        "data: {\"type\":\"response.failed\"}\n\n",
        "data: {\"type\":\"response.incomplete\"}\n\n",
    ]
    .iter()
    .enumerate()
    {
        let dir = tempfile::tempdir().unwrap();
        let mut client =
            OpenAiCodexClient::new("model", Arc::new(manager(&dir)), ThinkingLevel::Off);
        client.endpoint = format!("{}/test/{index}", server.uri());
        Mock::given(path(format!("/test/{index}")))
            .respond_with(ResponseTemplate::new(200).set_body_string(*body))
            .mount(&server)
            .await;
        assert!(
            client
                .chat(&[], &[], LlmCallOptions { streaming: false }, &NullSink)
                .await
                .is_err()
        );
    }
}
#[tokio::test]
async fn terminal_event_without_trailing_newline_is_accepted() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let mut client = OpenAiCodexClient::new("model", Arc::new(manager(&dir)), ThinkingLevel::Off);
    client.endpoint = format!("{}/responses", server.uri());
    Mock::given(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            "data: {}",
            json!({"type":"response.done", "response":response(json!([]))})
        )))
        .expect(1)
        .mount(&server)
        .await;
    client
        .chat(&[], &[], LlmCallOptions { streaming: false }, &NullSink)
        .await
        .unwrap();
}

#[tokio::test]
async fn redirects_are_rejected_instead_of_forwarding_bearer_credentials() {
    let server = MockServer::start().await;
    let other = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let mut client = OpenAiCodexClient::new("model", Arc::new(manager(&dir)), ThinkingLevel::Off);
    client.endpoint = format!("{}/responses", server.uri());
    Mock::given(path("/responses"))
        .respond_with(
            ResponseTemplate::new(307).insert_header("location", format!("{}/stolen", other.uri())),
        )
        .expect(1)
        .mount(&server)
        .await;
    let error = client
        .chat(&[], &[], LlmCallOptions { streaming: false }, &NullSink)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("307"));
    assert!(other.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn unauthorized_request_refreshes_once_and_retries_with_new_token() {
    use base64::Engine;
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let auth = manager(&dir).with_token_url(format!("{}/token", server.uri()));
    let mut client = OpenAiCodexClient::new("model", Arc::new(auth), ThinkingLevel::Off);
    client.endpoint = format!("{}/responses", server.uri());
    let access = format!(
        "header.{}.signature",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(
                &json!({"https://api.openai.com/auth":{"chatgpt_account_id":"account-2"}})
            )
            .unwrap()
        )
    );
    Mock::given(path("/responses"))
        .and(header("authorization", "Bearer access-secret"))
        .respond_with(ResponseTemplate::new(401))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": access, "refresh_token": "rotated", "expires_in":3600
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/responses"))
        .and(header("authorization", format!("Bearer {access}")))
        .and(header("chatgpt-account-id", "account-2"))
        .respond_with(ResponseTemplate::new(200).set_body_string(sse(response(json!([])))))
        .expect(1)
        .mount(&server)
        .await;
    client
        .chat(&[], &[], LlmCallOptions { streaming: false }, &NullSink)
        .await
        .unwrap();
}
