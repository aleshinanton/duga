//! Provider resolution and LLM client construction.
//!
//! Extracted from `duga-harness/src/main.rs` so CLI, Telegram, and TUI frontends
//! all use the same provider routing logic.

use anyhow::{Context, Result};
use duga_config::Config;
use duga_llm::dummy::DummyClient;
use duga_llm::{AnthropicClient, LlmClient, OpenAiClient};
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
    match provider {
        "dummy" => Ok(Arc::new(DummyClient::with_response(
            format!("{provider}/{model_name}"),
            DummyClient::text_response(format!(
                "Dummy provider for model '{model_name}' is wired correctly."
            )),
        ))),
        "openai" => {
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
                None => Err(anyhow::anyhow!(
                    "OPENAI_API_KEY is not set; set provider_api_key, provider_api_key_env, or provider_base_url for local endpoints"
                )),
            }
        }
        "anthropic" => {
            let base_url = resolve_base_url(config);
            let api_key = resolve_api_key(config, "ANTHROPIC_API_KEY")
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "ANTHROPIC_API_KEY is not set; set provider_api_key or provider_api_key_env"
                    )
                })?;
            let thinking = config.thinking_level.clone();
            Ok(Arc::new(AnthropicClient::with_thinking(
                model_name,
                api_key,
                base_url,
                thinking,
            )))
        }
        other => Err(anyhow::anyhow!(
            "unsupported LLM provider '{other}' (supported: dummy, openai, anthropic)"
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
                dir: "./plugins".into(),
                modules: vec![],
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
