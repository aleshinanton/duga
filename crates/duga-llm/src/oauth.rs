//! Anthropic OAuth (PKCE) login, token storage, and refresh.
//!
//! Implements the same OAuth flow used by Claude Code:
//! 1. Generate a PKCE verifier/challenge pair.
//! 2. Direct the user to the Anthropic authorize URL in a browser.
//! 3. The user pastes back the `code#state` string.
//! 4. Exchange the code for access + refresh tokens.
//! 5. Persist tokens to disk and refresh them transparently before expiry.
//!
//! Tokens are stored in `~/.duga/auth.json` by default (override with the
//! `DUGA_AUTH_FILE` env var). Access tokens are sent as `Authorization:
//! Bearer` with the `anthropic-beta: oauth-2025-04-20` header by
//! [`crate::AnthropicClient`] when constructed via `with_oauth`.

use crate::LlmError;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Public OAuth client id used by Claude Code.
pub const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
/// Browser authorize endpoint.
pub const AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
/// Token exchange/refresh endpoint.
pub const TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";
/// Redirect that displays the code for manual copy/paste.
pub const REDIRECT_URI: &str = "https://console.anthropic.com/oauth/code/callback";
/// Scopes requested during login.
pub const SCOPES: &str = "org:create_api_key user:profile user:inference";
/// Beta header value required when authenticating with OAuth tokens.
pub const OAUTH_BETA_HEADER: &str = "oauth-2025-04-20";

/// Refresh tokens this many milliseconds before their actual expiry.
const EXPIRY_MARGIN_MS: u64 = 60_000;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ── PKCE ────────────────────────────────────────────────────────────────────

/// A PKCE verifier and its S256 challenge.
#[derive(Debug, Clone)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

/// Generate a fresh PKCE verifier/challenge pair.
pub fn generate_pkce() -> Pkce {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    Pkce {
        challenge: challenge_for(&verifier),
        verifier,
    }
}

fn challenge_for(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Build the browser authorize URL for a PKCE pair.
///
/// The verifier doubles as the OAuth `state` parameter, matching the flow
/// used by Claude Code.
pub fn authorize_url(pkce: &Pkce) -> String {
    let mut url = reqwest::Url::parse(AUTHORIZE_URL).expect("static URL is valid");
    url.query_pairs_mut()
        .append_pair("code", "true")
        .append_pair("client_id", CLIENT_ID)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", REDIRECT_URI)
        .append_pair("scope", SCOPES)
        .append_pair("code_challenge", &pkce.challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &pkce.verifier);
    url.to_string()
}

// ── Tokens ──────────────────────────────────────────────────────────────────

/// Persisted OAuth tokens.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct OAuthTokens {
    pub access_token: String,
    pub refresh_token: String,
    /// Unix epoch milliseconds at which `access_token` expires.
    pub expires_at_ms: u64,
}

impl OAuthTokens {
    /// True when the access token is expired or within the refresh margin.
    pub fn needs_refresh(&self) -> bool {
        now_ms() + EXPIRY_MARGIN_MS >= self.expires_at_ms
    }
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    /// Lifetime of the access token in seconds.
    expires_in: u64,
}

impl TokenResponse {
    fn into_tokens(self, previous_refresh: Option<&str>) -> Result<OAuthTokens, LlmError> {
        let refresh_token = self
            .refresh_token
            .or_else(|| previous_refresh.map(str::to_string))
            .ok_or_else(|| {
                LlmError::Provider("OAuth token response did not include a refresh token".into())
            })?;
        Ok(OAuthTokens {
            access_token: self.access_token,
            refresh_token,
            expires_at_ms: now_ms() + self.expires_in.saturating_mul(1000),
        })
    }
}

async fn post_token(
    token_url: &str,
    body: serde_json::Value,
    previous_refresh: Option<&str>,
) -> Result<OAuthTokens, LlmError> {
    let response = reqwest::Client::new()
        .post(token_url)
        .json(&body)
        .send()
        .await
        .map_err(|e| LlmError::Transport(e.to_string()))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|e| LlmError::Transport(e.to_string()))?;
    if !status.is_success() {
        return Err(LlmError::Provider(format!(
            "OAuth token endpoint returned HTTP {status}: {text}"
        )));
    }
    let parsed: TokenResponse = serde_json::from_str(&text)
        .map_err(|e| LlmError::Provider(format!("OAuth token response parse error: {e}")))?;
    parsed.into_tokens(previous_refresh)
}

/// Exchange a pasted `code#state` string for tokens.
pub async fn exchange_code(code_input: &str, verifier: &str) -> Result<OAuthTokens, LlmError> {
    exchange_code_at(TOKEN_URL, code_input, verifier).await
}

async fn exchange_code_at(
    token_url: &str,
    code_input: &str,
    verifier: &str,
) -> Result<OAuthTokens, LlmError> {
    let trimmed = code_input.trim();
    let (code, state) = match trimmed.split_once('#') {
        Some((code, state)) => (code, state),
        None => (trimmed, verifier),
    };
    if code.is_empty() {
        return Err(LlmError::InvalidRequest("authorization code is empty".into()));
    }
    let body = serde_json::json!({
        "grant_type": "authorization_code",
        "code": code,
        "state": state,
        "client_id": CLIENT_ID,
        "redirect_uri": REDIRECT_URI,
        "code_verifier": verifier,
    });
    post_token(token_url, body, None).await
}

/// Refresh an access token using a refresh token.
pub async fn refresh_tokens(refresh_token: &str) -> Result<OAuthTokens, LlmError> {
    refresh_tokens_at(TOKEN_URL, refresh_token).await
}

async fn refresh_tokens_at(token_url: &str, refresh_token: &str) -> Result<OAuthTokens, LlmError> {
    let body = serde_json::json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "client_id": CLIENT_ID,
    });
    post_token(token_url, body, Some(refresh_token)).await
}

// ── Storage ─────────────────────────────────────────────────────────────────

/// Default credentials path: `$DUGA_AUTH_FILE` or `~/.duga/auth.json`.
pub fn default_credentials_path() -> PathBuf {
    if let Ok(path) = std::env::var("DUGA_AUTH_FILE") {
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    Path::new(&home).join(".duga").join("auth.json")
}

/// Load tokens from disk. `Ok(None)` when the file does not exist.
pub fn load_tokens(path: &Path) -> Result<Option<OAuthTokens>, LlmError> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(LlmError::Provider(format!(
                "reading OAuth credentials {}: {e}",
                path.display()
            )));
        }
    };
    let tokens = serde_json::from_str(&raw).map_err(|e| {
        LlmError::Provider(format!(
            "parsing OAuth credentials {}: {e}",
            path.display()
        ))
    })?;
    Ok(Some(tokens))
}

/// Save tokens to disk with owner-only permissions.
pub fn save_tokens(path: &Path, tokens: &OAuthTokens) -> Result<(), LlmError> {
    let map_err = |e: std::io::Error| {
        LlmError::Provider(format!("writing OAuth credentials {}: {e}", path.display()))
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(map_err)?;
    }
    let raw = serde_json::to_string_pretty(tokens)
        .map_err(|e| LlmError::Provider(format!("serializing OAuth credentials: {e}")))?;
    std::fs::write(path, raw).map_err(map_err)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(map_err)?;
    }
    Ok(())
}

// ── Manager ─────────────────────────────────────────────────────────────────

/// Holds OAuth tokens and refreshes/persists them transparently.
#[derive(Debug)]
pub struct OAuthManager {
    path: PathBuf,
    tokens: futures::lock::Mutex<OAuthTokens>,
}

impl OAuthManager {
    /// Load a manager from stored credentials. Errors when no credentials
    /// exist — run the login flow first.
    pub fn load(path: PathBuf) -> Result<Self, LlmError> {
        let tokens = load_tokens(&path)?.ok_or_else(|| {
            LlmError::InvalidRequest(format!(
                "no Anthropic OAuth credentials at {} — run the login flow first (--login anthropic)",
                path.display()
            ))
        })?;
        Ok(Self {
            path,
            tokens: futures::lock::Mutex::new(tokens),
        })
    }

    /// Build a manager from in-memory tokens (used right after login/tests).
    pub fn from_tokens(path: PathBuf, tokens: OAuthTokens) -> Self {
        Self {
            path,
            tokens: futures::lock::Mutex::new(tokens),
        }
    }

    /// Return a valid access token, refreshing and persisting when needed.
    pub async fn access_token(&self) -> Result<String, LlmError> {
        let mut guard = self.tokens.lock().await;
        if guard.needs_refresh() {
            let refreshed = refresh_tokens(&guard.refresh_token).await?;
            save_tokens(&self.path, &refreshed)?;
            *guard = refreshed;
        }
        Ok(guard.access_token.clone())
    }
}

// ── Interactive login ───────────────────────────────────────────────────────

/// Run the interactive PKCE login flow on stdin/stdout and persist tokens.
pub async fn login_interactive(path: &Path) -> Result<(), LlmError> {
    let pkce = generate_pkce();
    let url = authorize_url(&pkce);
    println!("Open this URL in your browser and authorize duga:\n");
    println!("  {url}\n");
    println!("After authorizing, copy the code shown (format: code#state) and paste it here.");
    print!("code> ");
    std::io::stdout()
        .flush()
        .map_err(|e| LlmError::Provider(e.to_string()))?;
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| LlmError::Provider(format!("reading code from stdin: {e}")))?;
    let tokens = exchange_code(&line, &pkce.verifier).await?;
    save_tokens(path, &tokens)?;
    println!(
        "✓ Anthropic OAuth login complete. Credentials saved to {}",
        path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_is_s256_of_verifier() {
        let pkce = generate_pkce();
        assert_eq!(pkce.challenge, challenge_for(&pkce.verifier));
        // RFC 7636: verifier must be 43-128 chars.
        assert!(pkce.verifier.len() >= 43);
        // URL-safe base64, no padding.
        assert!(!pkce.challenge.contains('='));
        assert!(!pkce.challenge.contains('+'));
    }

    #[test]
    fn pkce_verifiers_are_unique() {
        assert_ne!(generate_pkce().verifier, generate_pkce().verifier);
    }

    #[test]
    fn authorize_url_contains_pkce_and_scopes() {
        let pkce = generate_pkce();
        let url = authorize_url(&pkce);
        assert!(url.starts_with(AUTHORIZE_URL));
        assert!(url.contains(&format!("code_challenge={}", pkce.challenge)));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e"));
        assert!(url.contains("user%3Ainference"));
    }

    #[test]
    fn token_store_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("auth.json");
        let tokens = OAuthTokens {
            access_token: "at".into(),
            refresh_token: "rt".into(),
            expires_at_ms: 12345,
        };
        save_tokens(&path, &tokens).unwrap();
        assert_eq!(load_tokens(&path).unwrap(), Some(tokens));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn load_tokens_missing_file_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_tokens(&dir.path().join("missing.json")).unwrap(), None);
    }

    #[test]
    fn needs_refresh_respects_margin() {
        let fresh = OAuthTokens {
            access_token: "at".into(),
            refresh_token: "rt".into(),
            expires_at_ms: now_ms() + 3_600_000,
        };
        assert!(!fresh.needs_refresh());
        let stale = OAuthTokens {
            expires_at_ms: now_ms() + 1_000,
            ..fresh.clone()
        };
        assert!(stale.needs_refresh());
    }

    #[test]
    fn token_response_keeps_previous_refresh_token() {
        let response = TokenResponse {
            access_token: "new-at".into(),
            refresh_token: None,
            expires_in: 3600,
        };
        let tokens = response.into_tokens(Some("old-rt")).unwrap();
        assert_eq!(tokens.refresh_token, "old-rt");
        assert!(tokens.expires_at_ms > now_ms());
    }

    #[test]
    fn token_response_without_any_refresh_token_errors() {
        let response = TokenResponse {
            access_token: "new-at".into(),
            refresh_token: None,
            expires_in: 3600,
        };
        assert!(response.into_tokens(None).is_err());
    }

    #[tokio::test]
    async fn exchange_code_rejects_empty_code() {
        let err = exchange_code("  ", "verifier").await.unwrap_err();
        assert!(matches!(err, LlmError::InvalidRequest(_)));
    }

    #[tokio::test]
    async fn exchange_and_refresh_hit_token_endpoint() {
        use wiremock::matchers::{body_partial_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/oauth/token"))
            .and(body_partial_json(serde_json::json!({
                "grant_type": "authorization_code",
                "code": "the-code",
                "state": "the-state",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "at-1",
                "refresh_token": "rt-1",
                "expires_in": 3600,
            })))
            .expect(1)
            .mount(&server)
            .await;

        let token_url = format!("{}/v1/oauth/token", server.uri());
        let tokens = exchange_code_at(&token_url, "the-code#the-state", "verifier")
            .await
            .unwrap();
        assert_eq!(tokens.access_token, "at-1");
        assert_eq!(tokens.refresh_token, "rt-1");

        server.reset().await;
        Mock::given(method("POST"))
            .and(path("/v1/oauth/token"))
            .and(body_partial_json(serde_json::json!({
                "grant_type": "refresh_token",
                "refresh_token": "rt-1",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "at-2",
                "expires_in": 3600,
            })))
            .expect(1)
            .mount(&server)
            .await;

        let refreshed = refresh_tokens_at(&token_url, "rt-1").await.unwrap();
        assert_eq!(refreshed.access_token, "at-2");
        // Refresh token preserved when the endpoint omits it.
        assert_eq!(refreshed.refresh_token, "rt-1");
    }
}
