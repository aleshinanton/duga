use super::*;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path},
};

fn jwt(account: &str) -> String {
    format!("header.{}.signature", URL_SAFE_NO_PAD.encode(serde_json::to_vec(&serde_json::json!({
        "https://api.openai.com/auth": {"chatgpt_account_id": account}, "exp": 4102444800_u64,
    })).unwrap()))
}
fn tokens() -> OpenAiTokens {
    OpenAiTokens {
        access_token: jwt("account-1"),
        refresh_token: "refresh-secret".into(),
        account_id: "account-1".into(),
        expires_at_ms: u64::MAX,
    }
}
#[test]
fn authorization_uses_independent_state_and_pkce() {
    let pkce = generate_pkce();
    let url = authorize_url(&pkce, "random-state", "http://127.0.0.1:1455/auth/callback");
    let parsed = reqwest::Url::parse(&url).unwrap();
    assert_eq!(parsed.host_str(), Some("auth.openai.com"));
    let params: HashMap<_, _> = parsed.query_pairs().collect();
    assert_eq!(params["state"], "random-state");
    assert_eq!(params["client_id"], CLIENT_ID);
    assert_eq!(params["code_challenge_method"], "S256");
    assert_eq!(params["code_challenge"], pkce.challenge);
    assert!(params["scope"].contains("offline_access"));
    assert!(!url.contains(&pkce.verifier));
}
#[test]
fn callback_requires_matching_state_exact_path_and_unambiguous_code() {
    assert_eq!(
        callback_code("/auth/callback?state=s&code=a%2Bb", "s").unwrap(),
        Some("a+b".into())
    );
    for target in [
        "/auth/callback?code=x",
        "/auth/callback?state=wrong&code=x",
        "/auth/callback?state=s&state=wrong&code=x",
        "/auth/callback?state=s&code=x&code=y",
        "/other?state=s&code=x",
        "//attacker/auth/callback?state=s&code=x",
        "https://attacker/auth/callback?state=s&code=x",
        "/auth/callback?state=s&code=",
    ] {
        assert_eq!(callback_code(target, "s").unwrap(), None, "{target}");
    }
    assert!(callback_code("/auth/callback?state=s&error=access_denied", "s").is_err());
    assert_eq!(
        callback_code("/auth/callback?state=wrong&error=access_denied", "s").unwrap(),
        None
    );
}
#[test]
fn credentials_are_atomic_private_and_redacted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/openai-auth.json");
    let mut credentials = tokens();
    save_tokens(&path, &credentials).unwrap();
    credentials.refresh_token = "rotated-refresh-secret".into();
    save_tokens(&path, &credentials).unwrap();
    let loaded = load_tokens(&path).unwrap().unwrap();
    assert_eq!(loaded.refresh_token, credentials.refresh_token);
    let debug = format!("{loaded:?}");
    assert!(!debug.contains("rotated-refresh-secret"));
    assert!(!debug.contains(&loaded.access_token));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
#[test]
fn token_response_supports_jwt_expiry_and_preserves_refresh_token() {
    let previous = tokens();
    let response = TokenResponse {
        access_token: jwt("account-1"),
        refresh_token: None,
        id_token: None,
        expires_in: None,
    };
    let next = response.into_tokens(Some(&previous)).unwrap();
    assert_eq!(next.expires_at_ms, 4102444800000);
    assert_eq!(next.refresh_token, previous.refresh_token);
}
#[tokio::test]
async fn exchange_is_form_encoded_and_never_exposes_error_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(header("content-type", "application/x-www-form-urlencoded"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": jwt("account-1"), "refresh_token": "new-refresh", "expires_in": 3600
        })))
        .expect(1)
        .mount(&server)
        .await;
    let credentials = post_token(
        &http_client(),
        &format!("{}/token", server.uri()),
        &[
            ("grant_type", "authorization_code"),
            ("code", "code+secret"),
            ("code_verifier", "verifier"),
            ("client_id", CLIENT_ID),
        ],
        None,
    )
    .await
    .unwrap();
    assert_eq!(credentials.refresh_token, "new-refresh");
    let requests = server.received_requests().await.unwrap();
    let encoded = std::str::from_utf8(&requests[0].body).unwrap();
    assert!(encoded.contains("code=code%2Bsecret"));
    assert!(encoded.contains("code_verifier=verifier"));
    Mock::given(path("/error"))
        .respond_with(ResponseTemplate::new(400).set_body_string("private-token-do-not-log"))
        .mount(&server)
        .await;
    let error = post_token(
        &http_client(),
        &format!("{}/error", server.uri()),
        &[],
        None,
    )
    .await
    .unwrap_err();
    assert!(!error.to_string().contains("private-token-do-not-log"));
}
#[tokio::test]
async fn concurrent_managers_refresh_once_and_persist_rotation() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let credentials_path = dir.path().join("auth.json");
    let mut expired = tokens();
    expired.expires_at_ms = 0;
    save_tokens(&credentials_path, &expired).unwrap();
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": jwt("account-2"), "refresh_token": "rotated", "expires_in": 3600,
        })))
        .expect(1)
        .mount(&server)
        .await;
    let first = OpenAiOAuthManager::load(credentials_path.clone())
        .unwrap()
        .with_token_url(format!("{}/token", server.uri()));
    let second = OpenAiOAuthManager::load(credentials_path.clone())
        .unwrap()
        .with_token_url(format!("{}/token", server.uri()));
    let (one, two) = tokio::join!(first.credentials(), second.credentials());
    assert_eq!(one.unwrap().refresh_token, "rotated");
    assert_eq!(two.unwrap().account_id, "account-2");
    assert_eq!(
        load_tokens(&credentials_path)
            .unwrap()
            .unwrap()
            .refresh_token,
        "rotated"
    );
}
#[tokio::test]
async fn refresh_persists_rotated_token_even_when_caller_is_cancelled() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let credentials_path = dir.path().join("auth.json");
    let mut expired = tokens();
    expired.expires_at_ms = 0;
    save_tokens(&credentials_path, &expired).unwrap();
    Mock::given(path("/token")).respond_with(ResponseTemplate::new(200)
        .set_delay(Duration::from_millis(100))
        .set_body_json(serde_json::json!({"access_token":jwt("account-1"), "refresh_token":"rotated", "expires_in":3600})))
        .expect(1).mount(&server).await;
    let manager = OpenAiOAuthManager::load(credentials_path.clone())
        .unwrap()
        .with_token_url(format!("{}/token", server.uri()));
    let caller = tokio::spawn(async move { manager.credentials().await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while server.received_requests().await.unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if load_tokens(&credentials_path)
                .unwrap()
                .unwrap()
                .refresh_token
                == "rotated"
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn callback_server_ignores_invalid_request_then_accepts_real_callback() {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let (code, mut stream) = wait_for_code(&listener, "expected-state").await.unwrap();
        respond(&mut stream, true).await;
        code
    });
    let client = reqwest::Client::new();
    let bad = client
        .get(format!(
            "http://{address}/auth/callback?state=wrong&code=evil"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);
    let good = client
        .get(format!(
            "http://{address}/auth/callback?state=expected-state&code=real"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(good.status(), 200);
    assert_eq!(handle.await.unwrap(), "real");
}
