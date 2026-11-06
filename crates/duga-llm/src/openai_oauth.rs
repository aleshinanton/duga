//! ChatGPT/Codex browser OAuth. Separate from API keys and Anthropic credentials.
//! Protocol reference: openai/codex, codex-rs/login/src/server.rs.
use crate::{
    LlmError,
    oauth::{Pkce, generate_pkce},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("valid HTTP client options")
}

/// Secrets are never included in Debug output, provider events or session logs.
#[derive(Clone, Serialize, Deserialize)]
pub struct OpenAiTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub account_id: String,
    pub expires_at_ms: u64,
}
impl std::fmt::Debug for OpenAiTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiTokens")
            .field("credentials", &"[REDACTED]")
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}
impl OpenAiTokens {
    fn validate(&self) -> Result<(), LlmError> {
        if self.access_token.is_empty()
            || self.refresh_token.is_empty()
            || self.account_id.is_empty()
        {
            return Err(LlmError::InvalidRequest(
                "incomplete OpenAI credentials; run duga-tui --login openai".into(),
            ));
        }
        Ok(())
    }
    fn needs_refresh(&self) -> bool {
        now_ms().saturating_add(60_000) >= self.expires_at_ms
    }
}

/// The JWT is decoded only for account routing/expiry, not used as proof of identity.
/// OpenAI validates the bearer token on each request.
fn jwt_payload(token: &str) -> Option<serde_json::Value> {
    let mut parts = token.split('.');
    parts.next()?;
    let payload = parts.next()?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    serde_json::from_slice(&bytes).ok()
}
fn account_id(token: &str) -> Option<String> {
    jwt_payload(token)?
        .get("https://api.openai.com/auth")?
        .get("chatgpt_account_id")?
        .as_str()
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}
impl TokenResponse {
    fn into_tokens(self, previous: Option<&OpenAiTokens>) -> Result<OpenAiTokens, LlmError> {
        let account_id = account_id(&self.access_token)
            .or_else(|| self.id_token.as_deref().and_then(account_id))
            .or_else(|| previous.map(|p| p.account_id.clone()))
            .ok_or_else(|| LlmError::Provider("OpenAI token has no ChatGPT account ID".into()))?;
        let expires_at_ms = self
            .expires_in
            .map(|s| now_ms().saturating_add(s.saturating_mul(1000)))
            .or_else(|| {
                jwt_payload(&self.access_token)?
                    .get("exp")?
                    .as_u64()
                    .map(|s| s.saturating_mul(1000))
            })
            .ok_or_else(|| LlmError::Provider("OpenAI token response has no expiry".into()))?;
        let tokens = OpenAiTokens {
            access_token: self.access_token,
            refresh_token: self
                .refresh_token
                .or_else(|| previous.map(|p| p.refresh_token.clone()))
                .ok_or_else(|| {
                    LlmError::Provider("OpenAI token response has no refresh token".into())
                })?,
            account_id,
            expires_at_ms,
        };
        tokens.validate()?;
        Ok(tokens)
    }
}
async fn post_token(
    client: &reqwest::Client,
    endpoint: &str,
    fields: &[(&str, &str)],
    previous: Option<&OpenAiTokens>,
) -> Result<OpenAiTokens, LlmError> {
    let response = client
        .post(endpoint)
        .form(fields)
        .send()
        .await
        .map_err(|e| LlmError::Transport(e.to_string()))?;
    if !response.status().is_success() {
        // Never expose the endpoint's raw response: it can contain credentials.
        return Err(LlmError::Provider(format!(
            "OpenAI OAuth token request failed (HTTP {}); run duga-tui --login openai if authorization expired",
            response.status()
        )));
    }
    let token: TokenResponse = response
        .json()
        .await
        .map_err(|_| LlmError::Provider("invalid OpenAI OAuth token response".into()))?;
    token.into_tokens(previous)
}

pub fn default_credentials_path() -> Result<PathBuf, LlmError> {
    if let Some(path) = std::env::var_os("DUGA_OPENAI_AUTH_FILE").filter(|p| !p.is_empty()) {
        return Ok(path.into());
    }
    let home = std::env::var_os("HOME")
        .filter(|p| !p.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|p| !p.is_empty()))
        .ok_or_else(|| {
            LlmError::InvalidRequest(
                "set HOME, USERPROFILE or DUGA_OPENAI_AUTH_FILE for OpenAI credential storage"
                    .into(),
            )
        })?;
    Ok(PathBuf::from(home).join(".duga/openai-auth.json"))
}
pub fn load_tokens(path: &Path) -> Result<Option<OpenAiTokens>, LlmError> {
    let raw = match std::fs::read(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(LlmError::Provider(
                "cannot read OpenAI credential file".into(),
            ));
        }
    };
    let tokens: OpenAiTokens = serde_json::from_slice(&raw).map_err(|_| {
        LlmError::Provider("invalid OpenAI credential file; run duga-tui --login openai".into())
    })?;
    tokens.validate()?;
    Ok(Some(tokens))
}
pub fn save_tokens(path: &Path, tokens: &OpenAiTokens) -> Result<(), LlmError> {
    use std::io::Write;
    tokens.validate()?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let store = || -> Result<(), Box<dyn std::error::Error>> {
        std::fs::create_dir_all(parent)?;
        // tempfile creates mode 0600 on Unix before writing any secret bytes.
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer_pretty(file.as_file_mut(), tokens)?;
        file.flush()?;
        file.as_file().sync_all()?;
        file.persist(path)?;
        Ok(())
    };
    store().map_err(|_| LlmError::Provider("cannot securely save OpenAI credentials".into()))
}

type TokenState = futures::lock::Mutex<OpenAiTokens>;
// Clients built for successive/concurrent runs share refresh-token rotation.
static STATES: OnceLock<Mutex<HashMap<PathBuf, Weak<TokenState>>>> = OnceLock::new();
pub struct OpenAiOAuthManager {
    path: PathBuf,
    tokens: Arc<TokenState>,
    http: reqwest::Client,
    token_url: String,
}
impl std::fmt::Debug for OpenAiOAuthManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiOAuthManager").finish_non_exhaustive()
    }
}
impl OpenAiOAuthManager {
    pub fn load(path: PathBuf) -> Result<Self, LlmError> {
        let tokens = load_tokens(&path)?.ok_or_else(|| {
            LlmError::InvalidRequest("no OpenAI credentials; run duga-tui --login openai".into())
        })?;
        let path = path
            .canonicalize()
            .map_err(|_| LlmError::Provider("cannot resolve OpenAI credential file".into()))?;
        let mut states = STATES
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap();
        states.retain(|_, state| state.strong_count() > 0);
        let state = states
            .get(&path)
            .and_then(Weak::upgrade)
            .unwrap_or_else(|| {
                let state = Arc::new(TokenState::new(tokens));
                states.insert(path.clone(), Arc::downgrade(&state));
                state
            });
        Ok(Self {
            path,
            tokens: state,
            http: http_client(),
            token_url: TOKEN_URL.into(),
        })
    }
    #[cfg(test)]
    pub(crate) fn with_token_url(mut self, url: String) -> Self {
        self.token_url = url;
        self
    }

    pub async fn credentials(&self) -> Result<OpenAiTokens, LlmError> {
        self.credentials_after_rejection(None).await
    }
    pub(crate) async fn credentials_after_rejection(
        &self,
        rejected: Option<&str>,
    ) -> Result<OpenAiTokens, LlmError> {
        {
            let tokens = self.tokens.lock().await;
            if !tokens.needs_refresh()
                && rejected.is_none_or(|value| value != tokens.access_token)
            {
                return Ok(tokens.clone());
            }
        }
        let tokens = Arc::clone(&self.tokens);
        let path = self.path.clone();
        let http = self.http.clone();
        let token_url = self.token_url.clone();
        let rejected = rejected.map(str::to_owned);
        // Once OpenAI has consumed a refresh token, finish persisting its replacement
        // even if the inference caller is cancelled by the run budget. Dropping a
        // JoinHandle detaches this bounded (HTTP timeout) refresh operation.
        tokio::spawn(async move {
            let mut tokens = tokens.lock().await;
            if tokens.needs_refresh()
                || rejected
                    .as_deref()
                    .is_some_and(|value| value == tokens.access_token)
            {
                // A different process may have refreshed the file since this manager was built.
                if let Some(stored) = load_tokens(&path)? {
                    if stored.refresh_token != tokens.refresh_token {
                        *tokens = stored;
                    }
                }
                if tokens.needs_refresh()
                    || rejected
                        .as_deref()
                        .is_some_and(|value| value == tokens.access_token)
                {
                    let updated = post_token(
                        &http,
                        &token_url,
                        &[
                            ("grant_type", "refresh_token"),
                            ("client_id", CLIENT_ID),
                            ("refresh_token", &tokens.refresh_token),
                        ],
                        Some(&tokens),
                    )
                    .await?;
                    // Keep the rotated token in memory even if persistence fails.
                    *tokens = updated;
                    save_tokens(&path, &tokens)?;
                }
            }
            Ok(tokens.clone())
        })
        .await
        .map_err(|_| LlmError::Provider("OpenAI credential refresh task failed".into()))?
    }
}

fn authorize_url(pkce: &Pkce, state: &str, redirect: &str) -> String {
    let mut url = reqwest::Url::parse(AUTHORIZE_URL).unwrap();
    url.query_pairs_mut().extend_pairs([
        ("response_type", "code"),
        ("client_id", CLIENT_ID),
        ("redirect_uri", redirect),
        ("scope", "openid profile email offline_access"),
        ("code_challenge", &pkce.challenge),
        ("code_challenge_method", "S256"),
        ("state", state),
        ("id_token_add_organizations", "true"),
        ("codex_cli_simplified_flow", "true"),
        ("originator", "duga"),
    ]);
    url.into()
}
fn callback_code(target: &str, expected_state: &str) -> Result<Option<String>, LlmError> {
    // Only origin-form request targets on the fixed callback path are accepted.
    if !target.starts_with('/') || target.starts_with("//") {
        return Ok(None);
    }
    let url = reqwest::Url::parse(&format!("http://localhost{target}"))
        .map_err(|_| LlmError::InvalidRequest("invalid OAuth callback".into()))?;
    if url.path() != "/auth/callback" {
        return Ok(None);
    }
    let pairs: Vec<_> = url.query_pairs().collect();
    if pairs.iter().filter(|(key, _)| key == "state").count() != 1
        || !pairs
            .iter()
            .any(|(key, value)| key == "state" && value == expected_state)
    {
        return Ok(None);
    }
    if pairs.iter().any(|(key, _)| key == "error") {
        return Err(LlmError::InvalidRequest(
            "OpenAI authorization was denied or cancelled".into(),
        ));
    }
    let codes: Vec<_> = pairs.iter().filter(|(key, _)| key == "code").collect();
    if codes.len() != 1 || codes[0].1.is_empty() {
        return Ok(None);
    }
    Ok(Some(codes[0].1.to_string()))
}
async fn respond(stream: &mut TcpStream, success: bool) {
    let (status, body) = if success {
        (
            "200 OK",
            "OpenAI login complete. You can close this tab and return to duga.",
        )
    } else {
        (
            "400 Bad Request",
            "Login callback was not accepted. Return to duga and try again.",
        )
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let _ = tokio::time::timeout(
        Duration::from_secs(2),
        stream.write_all(response.as_bytes()),
    )
    .await;
}
async fn wait_for_code(
    listener: &TcpListener,
    state: &str,
) -> Result<(String, TcpStream), LlmError> {
    loop {
        let (mut stream, _) = listener
            .accept()
            .await
            .map_err(|_| LlmError::Transport("OAuth callback listener failed".into()))?;
        let request = tokio::time::timeout(Duration::from_secs(3), async {
            let mut bytes = Vec::new();
            loop {
                let mut chunk = [0; 1024];
                let n = stream.read(&mut chunk).await.ok()?;
                if n == 0 || bytes.len() + n > 8192 {
                    return None;
                }
                bytes.extend_from_slice(&chunk[..n]);
                if bytes.windows(4).any(|s| s == b"\r\n\r\n") {
                    break;
                }
            }
            let text = std::str::from_utf8(&bytes).ok()?;
            let mut parts = text.lines().next()?.split_whitespace();
            if parts.next()? != "GET" {
                return None;
            }
            Some(parts.next()?.to_owned())
        })
        .await
        .ok()
        .flatten();
        if let Some(target) = request {
            match callback_code(&target, state) {
                Ok(Some(code)) => return Ok((code, stream)),
                Err(error) => {
                    respond(&mut stream, false).await;
                    return Err(error);
                }
                _ => {}
            }
        }
        respond(&mut stream, false).await;
    }
}

/// Run before entering terminal raw/alternate-screen mode. No password is handled by duga.
pub async fn login_interactive(path: &Path) -> Result<(), LlmError> {
    let mut listener = None;
    for port in [1455, 1457] {
        if let Ok(bound) = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await {
            listener = Some(bound);
            break;
        }
    }
    let listener = listener.ok_or_else(|| {
        LlmError::Transport(
            "OpenAI login needs localhost port 1455 or 1457; stop other login attempts and retry"
                .into(),
        )
    })?;
    let redirect = format!(
        "http://127.0.0.1:{}/auth/callback",
        listener
            .local_addr()
            .map_err(|e| LlmError::Transport(e.to_string()))?
            .port()
    );
    let pkce = generate_pkce();
    let mut random = [0; 32];
    rand::rngs::OsRng.fill_bytes(&mut random);
    let state = URL_SAFE_NO_PAD.encode(random);
    let url = authorize_url(&pkce, &state, &redirect);
    println!(
        "Sign in to OpenAI / ChatGPT in your browser:\n\n{url}\n\nWaiting up to 5 minutes. Press Ctrl+C to cancel."
    );
    if webbrowser::open(&url).is_err() {
        println!("Could not open the browser automatically. Open the URL above manually.");
    }
    let flow = async {
        let (code, mut stream) = wait_for_code(&listener, &state).await?;
        let result = post_token(
            &http_client(),
            TOKEN_URL,
            &[
                ("grant_type", "authorization_code"),
                ("client_id", CLIENT_ID),
                ("code", &code),
                ("code_verifier", &pkce.verifier),
                ("redirect_uri", &redirect),
            ],
            None,
        )
        .await
        .and_then(|tokens| save_tokens(path, &tokens));
        respond(&mut stream, result.is_ok()).await;
        result
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => return Err(LlmError::Cancelled),
        result = tokio::time::timeout(LOGIN_TIMEOUT, flow) => result.map_err(|_| LlmError::Timeout)??,
    }
    println!(
        "OpenAI login complete. Credentials saved to {}",
        path.display()
    );
    Ok(())
}

#[cfg(test)]
#[path = "openai_oauth_tests.rs"]
mod tests;
