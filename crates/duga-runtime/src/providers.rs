//! Provider resolution and LLM client construction.
//!
//! Extracted from `duga-harness/src/main.rs` so CLI, Telegram, and TUI frontends
//! all use the same provider routing logic.

use anyhow::{Context, Result};
use duga_config::Config;
use duga_llm::dummy::DummyClient;
use duga_llm::{
    AnthropicClient, LlmClient, OAuthManager, OpenAiClient, OpenAiCodexClient, OpenAiOAuthManager,
};
use std::sync::Arc;

/// Resolved provider and model after processing config overrides.
#[derive(Debug, PartialEq)]
pub struct ProviderSelection {
    pub provider: String,
    pub model: String,
}

/// Resolve provider and model from config, handling both `provider/model` notation
/// and the separate `provider` + `model` fields.
pub fn resolve_provider(config: &Config) -> Result<ProviderSelection> {
    let model = config.model.trim();
    let provider = config.provider.as_deref().map(str::trim);
    if let Some(provider) = provider {
        if provider.is_empty() {
            return Err(anyhow::anyhow!("provider must not be empty"));
        }
        let model = match model.split_once('/') {
            Some((embedded_provider, embedded_model)) if embedded_provider == provider => {
                embedded_model
            }
            Some((embedded_provider, _)) => {
                return Err(anyhow::anyhow!(
                    "provider '{}' conflicts with model provider '{}'",
                    provider,
                    embedded_provider
                ));
            }
            None => model,
        };
        return Ok(ProviderSelection {
            provider: provider.into(),
            model: model.into(),
        });
    }

    let (provider, model) = model
        .split_once('/')
        .context("model must use provider/model format or config must set provider")?;
    Ok(ProviderSelection {
        provider: provider.into(),
        model: model.into(),
    })
}

/// Resolve API key from config with cascading priority:
/// 1. `provider_api_key` literal in config
/// 2. `provider_api_key_env` env var
/// 3. Provider's default env var (`default_env`)
fn resolve_api_key(config: &Config, default_env: &str) -> Option<String> {
    // 1. Literal value in config
    if let Some(ref key) = config.provider_api_key {
        return Some(key.clone());
    }

    // 2. Custom env var name
    if let Some(ref env_name) = config.provider_api_key_env {
        return std::env::var(env_name).ok();
    }

    // 3. Provider's default env var
    std::env::var(default_env).ok()
}

/// Resolve base URL from config with cascading priority:
/// 1. `provider_base_url` literal in config
/// 2. `provider_base_url_env` env var
/// 3. `BASE_URL` env var (shared default)
fn resolve_base_url(config: &Config) -> Option<String> {
    // 1. Literal value in config
    if let Some(ref url) = config.provider_base_url {
        return Some(url.clone());
    }

    // 2. Custom env var name
    if let Some(ref env_name) = config.provider_base_url_env {
        return std::env::var(env_name).ok();
    }

    // 3. Shared default env var
    std::env::var("BASE_URL").ok()
}

/// Build an LLM client for the given provider and model,
/// resolving credentials from config fields first, then env vars.
pub fn build_llm(provider: &str, model_name: &str, config: &Config) -> Result<Arc<dyn LlmClient>> {
    if let Some(auth) = config.provider_auth.as_deref() {
        anyhow::ensure!(
            matches!(auth, "api_key" | "oauth"),
            "unsupported provider_auth '{auth}' (expected api_key or oauth)"
        );
    }
    match provider {
        "dummy" => Ok(Arc::new(DummyClient::with_response(
            format!("{provider}/{model_name}"),
            DummyClient::text_response(format!(
                "Dummy provider for model '{model_name}' is wired correctly."
            )),
        ))),
        "openai" | "openai-codex" => {
            if provider == "openai-codex" || config.provider_auth.as_deref() == Some("oauth") {
                anyhow::ensure!(
                    config.provider_base_url.is_none() && config.provider_base_url_env.is_none(),
                    "OpenAI OAuth uses the ChatGPT Codex endpoint; remove provider_base_url/provider_base_url_env (use API-key auth for compatible endpoints)"
                );
                let manager =
                    OpenAiOAuthManager::load(duga_llm::openai_oauth::default_credentials_path()?)?;
                return Ok(Arc::new(OpenAiCodexClient::new(
                    model_name,
                    Arc::new(manager),
                    config.thinking_level,
                )));
            }
            let base_url = resolve_base_url(config);
            let api_key = resolve_api_key(config, "OPENAI_API_KEY");
            let thinking = config.thinking_level;
            match api_key {
                Some(key) => Ok(Arc::new(OpenAiClient::with_thinking(
                    model_name, key, base_url, thinking,
                ))),
                None if base_url.is_some() => {
                    // Local endpoint — empty key is fine
                    Ok(Arc::new(OpenAiClient::with_thinking(
                        model_name, "", base_url, thinking,
                    )))
                }
                None => {
                    if config.provider_auth.as_deref() != Some("api_key") {
                        let path = duga_llm::openai_oauth::default_credentials_path()?;
                        if path.is_file() {
                            let manager = OpenAiOAuthManager::load(path)?;
                            return Ok(Arc::new(OpenAiCodexClient::new(
                                model_name,
                                Arc::new(manager),
                                thinking,
                            )));
                        }
                    }
                    Err(anyhow::anyhow!(
                        "no OpenAI credentials: set OPENAI_API_KEY, use provider_base_url for local endpoints, or run duga-tui --login openai and set provider_auth: oauth"
                    ))
                }
            }
        }
        "anthropic" => {
            let base_url = resolve_base_url(config);
            let thinking = config.thinking_level.clone();
            let credentials_path = duga_llm::oauth::default_credentials_path();

            // Explicit `provider_auth: "oauth"` always uses stored OAuth
            // credentials. Otherwise prefer an API key, but fall back to
            // stored OAuth credentials when no key is configured.
            let oauth_requested = config.provider_auth.as_deref() == Some("oauth");
            if oauth_requested {
                let manager = OAuthManager::load(credentials_path)?;
                return Ok(Arc::new(AnthropicClient::with_oauth(
                    model_name,
                    Arc::new(manager),
                    base_url,
                    thinking,
                )));
            }

            match resolve_api_key(config, "ANTHROPIC_API_KEY") {
                Some(api_key) => Ok(Arc::new(AnthropicClient::with_thinking(
                    model_name, api_key, base_url, thinking,
                ))),
                None if credentials_path.is_file() => {
                    let manager = OAuthManager::load(credentials_path)?;
                    Ok(Arc::new(AnthropicClient::with_oauth(
                        model_name,
                        Arc::new(manager),
                        base_url,
                        thinking,
                    )))
                }
                None => Err(anyhow::anyhow!(
                    "no Anthropic credentials: set ANTHROPIC_API_KEY (or provider_api_key / provider_api_key_env), or log in with OAuth (--login anthropic) and set provider_auth: \"oauth\""
                )),
            }
        }
        other => Err(anyhow::anyhow!(
            "unsupported LLM provider '{other}' (supported: dummy, openai, openai-codex, anthropic)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use duga_config::{
        Config, EnvironmentConfig, FrontendConfig, MemoryConfig, PluginConfig, SandboxConfig,
        SummarizerKind, WorkspaceConfig,
    };
    use duga_types::config::AgentConfig;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    struct EnvGuard {
        _lock: MutexGuard<'static, ()>,
        saved: Vec<(&'static str, Option<String>)>,
    }

    impl EnvGuard {
        fn new(keys: &[&'static str]) -> Self {
            let lock = ENV_LOCK
                .get_or_init(|| Mutex::new(()))
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let saved = keys
                .iter()
                .map(|&key| (key, std::env::var(key).ok()))
                .collect();
            Self { _lock: lock, saved }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in &self.saved {
                match value {
                    // TODO: Audit that the environment access only happens in single-threaded code.
                    Some(value) => unsafe { std::env::set_var(key, value) },
                    // TODO: Audit that the environment access only happens in single-threaded code.
                    None => unsafe { std::env::remove_var(key) },
                }
            }
        }
    }

    fn test_config(model: &str) -> Config {
        Config {
            provider: None,
            model: model.into(),
            provider_api_key: None,
            provider_api_key_env: None,
            provider_base_url: None,
            provider_base_url_env: None,
            provider_auth: None,
            thinking_level: Default::default(),
            context_window: None,
            agent: AgentConfig::default(),
            sandbox: SandboxConfig {
                mode: Default::default(),
                container: None,
                workspace_mount: None,
                timeout: std::time::Duration::from_secs(1),
                allowed_binaries: vec![],
                allow_all_binaries: false,
            },
            workspace: WorkspaceConfig { root: ".".into() },
            environment: EnvironmentConfig {
                allowed: Default::default(),
            },
            memory: MemoryConfig {
                max_tokens: 1024,
                compress_at_ratio: 0.8,
                context_window_size: 0,
                max_context_tokens: 0,
                summarizer: SummarizerKind::Simple,
            },
            plugins: PluginConfig {
                wasm_dir: "./plugins".into(),
                wasm_modules: vec![],
                mcp: None,
            },
            frontend: FrontendConfig::default(),
            tui: None,
            telegram: None,
        }
    }

    #[test]
    fn resolve_provider_supports_legacy_provider_model() {
        let mut config = test_config("dummy/test");
        config.provider = None;
        assert_eq!(
            resolve_provider(&config).unwrap(),
            ProviderSelection {
                provider: "dummy".into(),
                model: "test".into()
            }
        );
    }

    #[test]
    fn resolve_provider_supports_separate_provider_field() {
        let mut config = test_config("test");
        config.provider = Some("dummy".into());
        assert_eq!(
            resolve_provider(&config).unwrap(),
            ProviderSelection {
                provider: "dummy".into(),
                model: "test".into()
            }
        );
    }

    #[test]
    fn resolve_provider_rejects_conflicting_provider_sources() {
        let mut config = test_config("openai/gpt");
        config.provider = Some("anthropic".into());
        let err = resolve_provider(&config).unwrap_err().to_string();
        assert!(err.contains("conflicts"));
    }

    #[test]
    fn build_provider_rejects_unknown_provider() {
        let config = test_config("unknown/qwen");
        let err = match build_llm("unknown", "qwen", &config) {
            Ok(_) => panic!("unknown provider should fail"),
            Err(error) => error.to_string(),
        };
        assert!(err.contains("unsupported LLM provider"));
    }

    #[test]
    fn build_provider_keeps_dummy_for_offline_smoke_tests() {
        let config = test_config("dummy/test");
        let provider = build_llm("dummy", "test", &config).unwrap();
        assert_eq!(provider.model(), "dummy/test");
    }

    #[test]
    fn openai_oauth_uses_credentials_and_ignores_ambient_api_endpoint() {
        let _env = EnvGuard::new(&["DUGA_OPENAI_AUTH_FILE", "OPENAI_API_KEY", "BASE_URL"]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("openai.json");
        duga_llm::openai_oauth::save_tokens(
            &path,
            &duga_llm::openai_oauth::OpenAiTokens {
                access_token: "access".into(),
                refresh_token: "refresh".into(),
                account_id: "account".into(),
                expires_at_ms: u64::MAX,
            },
        )
        .unwrap();
        unsafe {
            std::env::set_var("DUGA_OPENAI_AUTH_FILE", &path);
            std::env::set_var("OPENAI_API_KEY", "unrelated-api-key");
            std::env::set_var("BASE_URL", "http://localhost:11434/v1");
        }
        let mut config = test_config("openai/test-model");
        config.provider_auth = Some("oauth".into());
        assert_eq!(
            build_llm("openai", "test-model", &config).unwrap().model(),
            "test-model"
        );
        assert_eq!(
            build_llm("openai-codex", "test-model", &config)
                .unwrap()
                .model(),
            "test-model"
        );
    }

    #[test]
    fn openai_oauth_does_not_fall_back_to_api_key_or_custom_endpoint() {
        let _env = EnvGuard::new(&["DUGA_OPENAI_AUTH_FILE"]);
        let dir = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("DUGA_OPENAI_AUTH_FILE", dir.path().join("missing.json"));
        }
        let mut config = test_config("openai/test-model");
        config.provider_auth = Some("oauth".into());
        config.provider_api_key = Some("api-key".into());
        let error = build_llm("openai", "test-model", &config)
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("--login openai"));
        config.provider_base_url = Some("http://localhost/attacker".into());
        let error = build_llm("openai", "test-model", &config)
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("remove provider_base_url"));
        config.provider_auth = Some("api_key".into());
        assert!(build_llm("openai", "test-model", &config).is_ok());
    }

    fn write_oauth_credentials(dir: &tempfile::TempDir) -> std::path::PathBuf {
        let path = dir.path().join("auth.json");
        let tokens = duga_llm::oauth::OAuthTokens {
            access_token: "at".into(),
            refresh_token: "rt".into(),
            expires_at_ms: u64::MAX,
        };
        duga_llm::oauth::save_tokens(&path, &tokens).unwrap();
        path
    }

    #[test]
    fn build_anthropic_oauth_uses_stored_credentials() {
        let _env = EnvGuard::new(&["DUGA_AUTH_FILE", "ANTHROPIC_API_KEY"]);
        let dir = tempfile::tempdir().unwrap();
        let path = write_oauth_credentials(&dir);
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("DUGA_AUTH_FILE", &path) };
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::remove_var("ANTHROPIC_API_KEY") };

        let mut config = test_config("claude-sonnet-4-5");
        config.provider = Some("anthropic".into());
        config.provider_auth = Some("oauth".into());

        let client = build_llm("anthropic", "claude-sonnet-4-5", &config).unwrap();
        assert_eq!(client.model(), "claude-sonnet-4-5");
    }

    #[test]
    fn build_anthropic_oauth_errors_without_credentials() {
        let _env = EnvGuard::new(&["DUGA_AUTH_FILE", "ANTHROPIC_API_KEY"]);
        let dir = tempfile::tempdir().unwrap();
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("DUGA_AUTH_FILE", dir.path().join("missing.json")) };

        let mut config = test_config("claude-sonnet-4-5");
        config.provider_auth = Some("oauth".into());

        let err = match build_llm("anthropic", "claude-sonnet-4-5", &config) {
            Ok(_) => panic!("oauth without credentials should fail"),
            Err(error) => error.to_string(),
        };
        assert!(err.contains("login"), "unexpected error: {err}");
    }

    #[test]
    fn build_anthropic_falls_back_to_oauth_when_no_api_key() {
        let _env = EnvGuard::new(&["DUGA_AUTH_FILE", "ANTHROPIC_API_KEY"]);
        let dir = tempfile::tempdir().unwrap();
        let path = write_oauth_credentials(&dir);
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("DUGA_AUTH_FILE", &path) };
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::remove_var("ANTHROPIC_API_KEY") };

        let config = test_config("claude-sonnet-4-5");
        let client = build_llm("anthropic", "claude-sonnet-4-5", &config).unwrap();
        assert_eq!(client.model(), "claude-sonnet-4-5");
    }

    #[test]
    fn build_anthropic_prefers_api_key_over_oauth_by_default() {
        let _env = EnvGuard::new(&["DUGA_AUTH_FILE", "ANTHROPIC_API_KEY"]);
        let dir = tempfile::tempdir().unwrap();
        let path = write_oauth_credentials(&dir);
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("DUGA_AUTH_FILE", &path) };
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("ANTHROPIC_API_KEY", "sk-ant-key") };

        let config = test_config("claude-sonnet-4-5");
        let client = build_llm("anthropic", "claude-sonnet-4-5", &config).unwrap();
        assert_eq!(client.model(), "claude-sonnet-4-5");
    }

    #[test]
    fn resolve_api_key_uses_literal_first() {
        let _env = EnvGuard::new(&["CUSTOM_KEY", "OPENAI_API_KEY"]);
        let mut config = test_config("test");
        config.provider_api_key = Some("sk-literal".into());
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("CUSTOM_KEY", "sk-from-env") };
        config.provider_api_key_env = Some("CUSTOM_KEY".into());
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::remove_var("OPENAI_API_KEY") };

        let result = resolve_api_key(&config, "OPENAI_API_KEY");
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::remove_var("CUSTOM_KEY") };

        assert_eq!(result, Some("sk-literal".into()));
    }

    #[test]
    fn resolve_api_key_falls_back_to_env_var_name() {
        let _env = EnvGuard::new(&["CUSTOM_KEY", "OPENAI_API_KEY"]);
        let mut config = test_config("test");
        config.provider_api_key = None;
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("CUSTOM_KEY", "sk-from-env") };
        config.provider_api_key_env = Some("CUSTOM_KEY".into());
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::remove_var("OPENAI_API_KEY") };

        let result = resolve_api_key(&config, "OPENAI_API_KEY");
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::remove_var("CUSTOM_KEY") };

        assert_eq!(result, Some("sk-from-env".into()));
    }

    #[test]
    fn resolve_api_key_falls_back_to_default_env() {
        let _env = EnvGuard::new(&["OPENAI_API_KEY"]);
        let config = test_config("test");
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("OPENAI_API_KEY", "sk-default") };
        let result = resolve_api_key(&config, "OPENAI_API_KEY");
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::remove_var("OPENAI_API_KEY") };
        assert_eq!(result, Some("sk-default".into()));
    }

    #[test]
    fn resolve_base_url_uses_literal_first() {
        let _env = EnvGuard::new(&["CUSTOM_URL", "BASE_URL"]);
        let mut config = test_config("test");
        config.provider_base_url = Some("http://literal:8080/v1".into());
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("CUSTOM_URL", "http://from-env:8080/v1") };
        config.provider_base_url_env = Some("CUSTOM_URL".into());

        let result = resolve_base_url(&config);
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::remove_var("CUSTOM_URL") };

        assert_eq!(result, Some("http://literal:8080/v1".into()));
    }

    #[test]
    fn resolve_base_url_falls_back_to_env_var_name() {
        let _env = EnvGuard::new(&["CUSTOM_URL", "BASE_URL"]);
        let mut config = test_config("test");
        config.provider_base_url = None;
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("CUSTOM_URL", "http://from-env:8080/v1") };
        config.provider_base_url_env = Some("CUSTOM_URL".into());

        let result = resolve_base_url(&config);
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::remove_var("CUSTOM_URL") };

        assert_eq!(result, Some("http://from-env:8080/v1".into()));
    }

    #[test]
    fn resolve_base_url_falls_back_to_default_env() {
        let _env = EnvGuard::new(&["BASE_URL"]);
        let config = test_config("test");
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("BASE_URL", "http://default:8080/v1") };
        let result = resolve_base_url(&config);
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::remove_var("BASE_URL") };
        assert_eq!(result, Some("http://default:8080/v1".into()));
    }

    #[test]
    fn resolve_base_url_none_when_nothing_set() {
        let _env = EnvGuard::new(&["BASE_URL"]);
        let config = test_config("test");
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::remove_var("BASE_URL") };
        let result = resolve_base_url(&config);
        assert_eq!(result, None);
    }
}
