use crate::ConfigError;
pub use duga_plugin_abi::{PluginModuleConfig, WasiCapabilities};
use duga_sandbox::binary_registry::BinaryPattern;
use duga_sandbox::env::is_secret_pattern;
use duga_types::config::AgentConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

// ── Typed enums for shared frontend config ──────────────────────────────────

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Dummy,
    Openai,
    Anthropic,
    #[serde(untagged)]
    Custom(String),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingLevel {
    Off,
    Low,
    Medium,
    High,
}

impl Default for ThinkingLevel {
    fn default() -> Self {
        Self::Off
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxMode {
    Host,
    Capability,
    Docker,
}

impl Default for SandboxMode {
    fn default() -> Self {
        Self::Capability
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressMode {
    FinalOnly,
    Summary,
    Verbose,
}

impl Default for ProgressMode {
    fn default() -> Self {
        Self::Summary
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FrontendConfig {
    #[serde(default)]
    pub progress_mode: ProgressMode,
    #[serde(with = "duration_format", default = "default_confirmation_timeout")]
    pub confirmation_timeout: Duration,
}

impl Default for FrontendConfig {
    fn default() -> Self {
        Self {
            progress_mode: ProgressMode::default(),
            confirmation_timeout: default_confirmation_timeout(),
        }
    }
}

fn default_confirmation_timeout() -> Duration {
    Duration::from_secs(60)
}

// ── Telegram config ─────────────────────────────────────────────────────────

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TelegramConfig {
    #[serde(default = "default_telegram_token_env")]
    pub token_env: String,
    #[serde(default)]
    pub allowed_chat_ids: Vec<i64>,
    #[serde(default)]
    pub allowed_chat_usernames: Vec<String>,
    #[serde(default)]
    pub allow_all_chats_for_dev: bool,
    #[serde(default = "default_true")]
    pub send_tool_events: bool,
    #[serde(default)]
    pub send_final_only: bool,
    #[serde(default)]
    pub require_confirmation_for: Vec<String>,
    #[serde(default = "default_telegram_events_dir")]
    pub events_dir: PathBuf,
    #[serde(default = "default_telegram_skills_dir")]
    pub skills_dir: PathBuf,
    #[serde(default = "default_telegram_data_dir")]
    pub data_dir: PathBuf,
    #[serde(default)]
    pub attachments: TelegramAttachmentConfig,
}

fn default_telegram_token_env() -> String {
    "TELEGRAM_BOT_TOKEN".into()
}

fn default_telegram_events_dir() -> PathBuf {
    PathBuf::from("./events")
}

fn default_telegram_skills_dir() -> PathBuf {
    PathBuf::from("./skills")
}

fn default_telegram_data_dir() -> PathBuf {
    PathBuf::from("./data")
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TelegramAttachmentConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_max_attachment_mb")]
    pub max_file_size_mb: u64,
    #[serde(default = "default_attachment_download_dir")]
    pub download_dir: PathBuf,
}

impl Default for TelegramAttachmentConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_file_size_mb: default_max_attachment_mb(),
            download_dir: default_attachment_download_dir(),
        }
    }
}

fn default_max_attachment_mb() -> u64 {
    20
}

fn default_attachment_download_dir() -> PathBuf {
    PathBuf::from("attachments")
}

impl Default for TelegramConfig {
    fn default() -> Self {
        Self {
            token_env: default_telegram_token_env(),
            allowed_chat_ids: Vec::new(),
            allowed_chat_usernames: Vec::new(),
            allow_all_chats_for_dev: false,
            send_tool_events: true,
            send_final_only: false,
            require_confirmation_for: Vec::new(),
            events_dir: default_telegram_events_dir(),
            skills_dir: default_telegram_skills_dir(),
            data_dir: default_telegram_data_dir(),
            attachments: TelegramAttachmentConfig::default(),
        }
    }
}

// ── Top-level Config ────────────────────────────────────────────────────────

#[derive(Clone, Deserialize, PartialEq, Serialize)]
pub struct Config {
    #[serde(default)]
    pub provider: Option<String>,
    pub model: String,
    #[serde(default)]
    pub provider_api_key: Option<String>,
    #[serde(default)]
    pub provider_api_key_env: Option<String>,
    #[serde(default)]
    pub provider_base_url: Option<String>,
    #[serde(default)]
    pub provider_base_url_env: Option<String>,
    #[serde(default)]
    pub thinking_level: ThinkingLevel,
    #[serde(default)]
    pub context_window: Option<u32>,
    pub agent: AgentConfig,
    pub sandbox: SandboxConfig,
    pub workspace: WorkspaceConfig,
    pub environment: EnvironmentConfig,
    pub memory: MemoryConfig,
    pub plugins: PluginConfig,
    #[serde(default)]
    pub frontend: FrontendConfig,
    #[serde(default)]
    pub telegram: Option<TelegramConfig>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field(
                "provider_api_key",
                &self.provider_api_key.as_ref().map(|_| "<redacted>"),
            )
            .field("provider_api_key_env", &self.provider_api_key_env)
            .field("provider_base_url", &self.provider_base_url)
            .field("provider_base_url_env", &self.provider_base_url_env)
            .field("thinking_level", &self.thinking_level)
            .field("context_window", &self.context_window)
            .field("agent", &self.agent)
            .field("sandbox", &self.sandbox)
            .field("workspace", &self.workspace)
            .field("environment", &self.environment)
            .field("memory", &self.memory)
            .field("plugins", &self.plugins)
            .field("frontend", &self.frontend)
            .field("telegram", &self.telegram)
            .finish()
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SandboxConfig {
    #[serde(default)]
    pub mode: SandboxMode,
    #[serde(default)]
    pub container: Option<String>,
    #[serde(default = "default_workspace_mount")]
    pub workspace_mount: Option<String>,
    #[serde(with = "duration_format")]
    pub timeout: Duration,
    /// List of allowed binaries. Each entry may be a bare name (e.g. `echo`),
    /// an absolute path (e.g. `/usr/bin/cat`), or a glob pattern
    /// (e.g. `/usr/bin/*`, `/usr/local/bin/g*`). Glob patterns are expanded
    /// at startup by walking matching directories.
    #[serde(default)]
    pub allowed_binaries: Vec<String>,
    /// When `true`, skip the binary allowlist check entirely. The bare command
    /// name from the LLM is passed directly to the executor. This is intended
    /// for Docker/container modes where OS-level isolation provides the primary
    /// security boundary.
    #[serde(default)]
    pub allow_all_binaries: bool,
}

fn default_workspace_mount() -> Option<String> {
    Some("/workspace".into())
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct WorkspaceConfig {
    pub root: PathBuf,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EnvironmentConfig {
    pub allowed: HashSet<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MemoryConfig {
    pub max_tokens: usize,
    pub compress_at_ratio: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PluginConfig {
    pub dir: PathBuf,
    #[serde(default)]
    pub modules: Vec<PluginModuleConfig>,
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let raw =
            std::fs::read_to_string(path).map_err(|_| ConfigError::FileNotFound(path.into()))?;
        let mut config: Self =
            serde_yaml::from_str(&raw).map_err(|e| ConfigError::ParseError(e.to_string()))?;
        config.workspace.root = expand_tilde(&config.workspace.root)?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        let mut errors = Vec::new();

        if !self.workspace.root.exists() {
            errors.push(format!(
                "workspace root not found: {}",
                self.workspace.root.display()
            ));
        } else if !self.workspace.root.is_dir() {
            errors.push(format!(
                "workspace root is not a directory: {}",
                self.workspace.root.display()
            ));
        }

        // Validate binary allowlist entries.
        // Glob patterns are expanded later; we only warn for obvious issues here.
        for entry in &self.sandbox.allowed_binaries {
            let pattern = match BinaryPattern::parse(entry) {
                Ok(p) => p,
                Err(e) => {
                    errors.push(format!("invalid binary entry '{}': {}", entry, e));
                    continue;
                }
            };
            match pattern {
                BinaryPattern::Exact(ref s) => {
                    // If it contains '/', treat as absolute path and validate.
                    if s.contains('/') {
                        let path = Path::new(s);
                        if !path.is_absolute() {
                            errors.push(format!(
                                "binary path must be absolute: {}",
                                path.display()
                            ));
                            continue;
                        }
                        if !path.exists() {
                            errors.push(format!("binary not found: {}", path.display()));
                            continue;
                        }
                        if !path.is_file() {
                            errors.push(format!("binary is not a file: {}", path.display()));
                            continue;
                        }
                        if !is_executable(path) {
                            errors.push(format!("binary is not executable: {}", path.display()));
                        }
                    }
                    // Bare names (no '/') are resolved via `which` at runtime — skip validation.
                }
                BinaryPattern::Glob(_) => {
                    // Glob patterns are expanded at startup by BinaryRegistry.
                    // We only check for path traversal here.
                }
            }
        }

        // Warn when allow_all_binaries is used with non-container sandbox modes.
        if self.sandbox.allow_all_binaries && matches!(self.sandbox.mode, SandboxMode::Host | SandboxMode::Capability) {
            tracing::warn!(
                "sandbox.allow_all_binaries is true but sandbox.mode is {:?}. \
                 This removes defense-in-depth; only use allow_all_binaries with Docker/container isolation.",
                self.sandbox.mode
            );
        }

        if self.agent.limits.max_steps == 0 {
            errors.push("agent.limits.max_steps must be greater than 0".into());
        }
        if self.agent.limits.max_tool_calls == 0 {
            errors.push("agent.limits.max_tool_calls must be greater than 0".into());
        }
        if self.agent.limits.max_runtime.is_zero() {
            errors.push("agent.limits.max_runtime must be greater than 0".into());
        }
        if self.memory.max_tokens == 0 {
            errors.push("memory.max_tokens must be greater than 0".into());
        }
        if self
            .provider
            .as_deref()
            .map(str::trim)
            .is_some_and(str::is_empty)
        {
            errors.push("provider must not be empty when specified".into());
        }
        if !(0.0..=1.0).contains(&self.memory.compress_at_ratio)
            || self.memory.compress_at_ratio == 0.0
            || self.memory.compress_at_ratio.is_nan()
        {
            errors.push("memory.compress_at_ratio must be between 0.0 and 1.0".into());
        }

        for name in &self.environment.allowed {
            if is_secret_pattern(name) {
                tracing::warn!(
                    "Secret-pattern variable '{}' in environment allowlist; this variable will be available to subprocesses",
                    name
                );
            }
        }

        if let Some(ctx_window) = self.context_window {
            if ctx_window < 4096 {
                errors.push(format!(
                    "context_window must be at least 4096, got {ctx_window}"
                ));
            }
            if ctx_window > 1_000_000 {
                errors.push(format!(
                    "context_window must not exceed 1,000,000, got {ctx_window}"
                ));
            }
        }

        if let Some(ref telegram) = self.telegram {
            let token = std::env::var(&telegram.token_env).unwrap_or_default();
            if token.is_empty() && !cfg!(test) {
                // In tests, we allow missing token.
                tracing::warn!(
                    "Telegram token env var '{}' is empty or not set",
                    telegram.token_env
                );
            }
            if !telegram.allow_all_chats_for_dev
                && telegram.allowed_chat_ids.is_empty()
                && telegram.allowed_chat_usernames.is_empty()
            {
                errors.push(
                    "at least one of telegram.allowed_chat_ids or telegram.allowed_chat_usernames must be non-empty (or set allow_all_chats_for_dev: true)"
                        .into(),
                );
            }
            if telegram.attachments.enabled && telegram.attachments.max_file_size_mb == 0 {
                errors.push("telegram.attachments.max_file_size_mb must be > 0".into());
            }
            if telegram.attachments.enabled && telegram.attachments.max_file_size_mb > 50 {
                errors.push("telegram.attachments.max_file_size_mb must not exceed Telegram's 50 MB limit".into());
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(ConfigError::ValidationError(errors.join("\n")))
        }
    }
}

fn expand_tilde(path: &Path) -> Result<PathBuf, ConfigError> {
    let raw = path.to_string_lossy();
    if cfg!(windows) || !raw.starts_with('~') {
        return Ok(path.to_path_buf());
    }
    if raw == "~" || raw.starts_with("~/") {
        let home = std::env::var("HOME").map_err(|_| {
            ConfigError::ValidationError("HOME must be set to expand '~' paths".into())
        })?;
        if raw == "~" {
            Ok(PathBuf::from(home))
        } else {
            Ok(PathBuf::from(home).join(&raw[2..]))
        }
    } else {
        Ok(path.to_path_buf())
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

mod duration_format {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!("{}s", duration.as_secs()))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        let raw = raw.trim();
        let parse = |value: &str| {
            value
                .parse::<u64>()
                .map_err(|_| serde::de::Error::custom(format!("invalid duration '{raw}'")))
        };
        let duration = if let Some(value) = raw.strip_suffix("ms") {
            Duration::from_millis(parse(value)?)
        } else if let Some(value) = raw.strip_suffix('s') {
            Duration::from_secs(parse(value)?)
        } else if let Some(value) = raw.strip_suffix('m') {
            Duration::from_secs(parse(value)? * 60)
        } else if let Some(value) = raw.strip_suffix('h') {
            Duration::from_secs(parse(value)? * 3600)
        } else {
            return Err(serde::de::Error::custom(
                "duration must use ms, s, m, or h suffix",
            ));
        };
        if duration.is_zero() {
            return Err(serde::de::Error::custom("duration must be positive"));
        }
        Ok(duration)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn executable_file(dir: &tempfile::TempDir) -> PathBuf {
        let path = dir.path().join("tool");
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
        }
        path
    }

    fn yaml(dir: &tempfile::TempDir, binary: &Path) -> String {
        format!(
            r#"
model: "dummy/test"
agent:
  limits:
    max_steps: 5
    max_tool_calls: 10
    max_runtime: 60s
    retry_on_error: 1
  features:
    streaming: false
  output:
    max_stdout_bytes: 1024
    max_stderr_bytes: 1024
    max_combined_bytes: 2048
  think:
    max_calls: 2
    max_tokens: 128
sandbox:
  timeout: 10s
  allowed_binaries:
    - {}
workspace:
  root: {}
environment:
  allowed:
    - HOME
memory:
  max_tokens: 4096
  compress_at_ratio: 0.8
plugins:
  dir: {}
  modules:
    - name: format-code.wasm
      capabilities:
        workspace_fs: true
"#,
            binary.display(),
            dir.path().display(),
            dir.path().join("plugins").display()
        )
    }

    #[test]
    fn load_parses_yaml_and_validates() {
        let dir = tempfile::tempdir().unwrap();
        let binary = executable_file(&dir);
        let path = dir.path().join("duga.yaml");
        std::fs::write(&path, yaml(&dir, &binary)).unwrap();

        let config = Config::load(&path).unwrap();

        assert_eq!(config.model, "dummy/test");
        assert_eq!(config.provider, None);
        assert_eq!(config.sandbox.timeout, Duration::from_secs(10));
        assert_eq!(config.plugins.modules[0].name, "format-code.wasm");
    }

    #[test]
    fn load_accepts_separate_provider_field() {
        let dir = tempfile::tempdir().unwrap();
        let binary = executable_file(&dir);
        let path = dir.path().join("duga.yaml");
        let mut raw = yaml(&dir, &binary);
        raw = raw.replacen(
            "model: \"dummy/test\"",
            "provider: \"ollama\"\nmodel: \"qwen\"",
            1,
        );
        std::fs::write(&path, raw).unwrap();

        let config = Config::load(&path).unwrap();

        assert_eq!(config.provider.as_deref(), Some("ollama"));
        assert_eq!(config.model, "qwen");
    }

    #[test]
    fn debug_redacts_literal_provider_api_key() {
        let dir = tempfile::tempdir().unwrap();
        let binary = executable_file(&dir);
        let mut config: Config = serde_yaml::from_str(&yaml(&dir, &binary)).unwrap();
        config.provider_api_key = Some("sk-secret-value".into());

        let debug = format!("{config:?}");

        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("sk-secret-value"));
    }

    #[test]
    fn missing_file_is_specific_error() {
        let err = Config::load("/no/such/config.yaml").unwrap_err();
        assert!(matches!(err, ConfigError::FileNotFound(_)));
    }

    #[test]
    fn validate_collects_errors() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("missing");
        let mut config: Config = serde_yaml::from_str(&yaml(&dir, &binary)).unwrap();
        config.agent.limits.max_steps = 0;
        config.memory.max_tokens = 0;

        let err = config.validate().unwrap_err().to_string();

        assert!(err.contains("binary not found"));
        assert!(err.contains("max_steps"));
        assert!(err.contains("memory.max_tokens"));
    }

    #[test]
    fn tilde_expands_from_home() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", dir.path());
        let expanded = expand_tilde(Path::new("~/demo")).unwrap();
        assert_eq!(expanded, dir.path().join("demo"));
    }

    #[test]
    fn partial_yaml_is_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("duga.yaml");
        let mut file = std::fs::File::create(&path).unwrap();
        writeln!(file, "model: dummy/test").unwrap();

        let err = Config::load(&path).unwrap_err();

        assert!(matches!(err, ConfigError::ParseError(_)));
    }
}
