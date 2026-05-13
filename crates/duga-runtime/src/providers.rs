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

/// Build an LLM client for the given provider and model.
pub fn build_llm(provider: &str, model_name: &str) -> Result<Arc<dyn LlmClient>> {
    match provider {
        "dummy" => Ok(Arc::new(DummyClient::with_response(
            format!("{provider}/{model_name}"),
            DummyClient::text_response(format!(
                "Dummy provider for model '{model_name}' is wired correctly."
            )),
        ))),
        "openai" => Ok(Arc::new(OpenAiClient::from_env(model_name)?)),
        "anthropic" => Ok(Arc::new(AnthropicClient::from_env(model_name)?)),
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
        WorkspaceConfig,
    };
    use duga_types::config::AgentConfig;

    fn test_config(model: &str) -> Config {
        Config {
            provider: None,
            model: model.into(),
            thinking_level: Default::default(),
            context_window: None,
            agent: AgentConfig::default(),
            sandbox: SandboxConfig {
                mode: Default::default(),
                container: None,
                workspace_mount: None,
                timeout: std::time::Duration::from_secs(1),
                allowed_binaries: vec![],
            },
            workspace: WorkspaceConfig { root: ".".into() },
            environment: EnvironmentConfig {
                allowed: Default::default(),
            },
            memory: MemoryConfig {
                max_tokens: 1024,
                compress_at_ratio: 0.8,
            },
            plugins: PluginConfig {
                dir: "./plugins".into(),
                modules: vec![],
            },
            frontend: FrontendConfig::default(),
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
        let err = match build_llm("unknown", "qwen") {
            Ok(_) => panic!("unknown provider should fail"),
            Err(error) => error.to_string(),
        };
        assert!(err.contains("unsupported LLM provider"));
    }

    #[test]
    fn build_provider_keeps_dummy_for_offline_smoke_tests() {
        let provider = build_llm("dummy", "test").unwrap();
        assert_eq!(provider.model(), "dummy/test");
    }
}
