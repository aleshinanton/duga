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

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingLevel {
    Off,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl ThinkingLevel {
    /// Returns the API parameter value for OpenAI / DeepSeek `reasoning_effort`.
    /// Returns `None` when thinking is disabled (the field should be omitted).
    pub fn to_api_param(&self) -> Option<&'static str> {
        match self {
            ThinkingLevel::Off => None,
            ThinkingLevel::Low => Some("low"),
            ThinkingLevel::Medium => Some("medium"),
            ThinkingLevel::High => Some("high"),
            ThinkingLevel::Xhigh => Some("xhigh"),
            ThinkingLevel::Max => Some("max"),
        }
    }

    /// Returns the Anthropic thinking budget in tokens.
    /// Returns `None` when thinking is disabled.
    pub fn anthropic_budget_tokens(&self) -> Option<u32> {
        match self {
            ThinkingLevel::Off => None,
            ThinkingLevel::Low => Some(1024),
            ThinkingLevel::Medium => Some(4096),
            ThinkingLevel::High => Some(16384),
            ThinkingLevel::Xhigh => Some(32768),
            ThinkingLevel::Max => Some(65536),
        }
    }
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

// ── TUI config ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TuiConfig {
    #[serde(default = "default_true")]
    pub ime_support: bool,
    #[serde(default = "default_true")]
    pub protocol_detection: bool,
    #[serde(default)]
    pub tool_event_format: ToolEventFormat,
    #[serde(default)]
    pub theme: ThemeConfig,
    #[serde(default)]
    pub keybindings: KeybindingsConfig,
    /// Tool names that require user confirmation before execution
    /// (e.g. ["shell", "write", "edit"]).
    #[serde(default)]
    pub require_confirmation_for: Vec<String>,
    /// Default agent loop to use (e.g. "simple_react", "problem_solving",
    /// "search", "decomposition", "verification").
    /// Defaults to "simple_react" if not set.
    #[serde(default = "default_loop_id")]
    pub default_loop: String,
    /// Whether to display LLM thinking/reasoning content in the TUI.
    /// When false, thinking events are silently dropped from the display
    /// (but still logged in session JSONL). Defaults to true.
    #[serde(default = "default_true")]
    pub show_thinking: bool,
    // ── EPIC-33: TUI Redesign config fields ────────────────────────
    /// Whether to show the persistent sidebar.
    #[serde(default = "default_true")]
    pub show_sidebar: bool,
    /// Sidebar width as percentage of terminal width (15-40).
    #[serde(default = "default_sidebar_pct")]
    pub sidebar_width_pct: u8,
    /// Whether to show the footer shortcut bar.
    #[serde(default = "default_true")]
    pub show_footer: bool,
    /// Whether to show the header bar.
    #[serde(default = "default_true")]
    pub show_header: bool,
    /// Width threshold below which sidebar collapses (0 = always show).
    #[serde(default = "default_responsive_breakpoint")]
    pub responsive_breakpoint: u16,
    /// Maximum characters in the input area.
    #[serde(default = "default_input_max_chars")]
    pub input_max_chars: usize,
    /// Auto-dismiss timeout for error banners in seconds.
    #[serde(default = "default_banner_auto_dismiss")]
    pub banner_auto_dismiss_secs: u64,
    /// Maximum number of entries in the event log.
    #[serde(default = "default_event_log_max")]
    pub event_log_max_entries: usize,
    /// Whether to use the new multi-pane layout (EPIC-33). Set to false
    /// to fall back to the original single-pane layout.
    #[serde(default = "default_true")]
    pub use_new_layout: bool,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            ime_support: true,
            protocol_detection: true,
            tool_event_format: ToolEventFormat::default(),
            theme: ThemeConfig::default(),
            keybindings: KeybindingsConfig::default(),
            require_confirmation_for: Vec::new(),
            default_loop: default_loop_id(),
            show_thinking: true,
            show_sidebar: true,
            sidebar_width_pct: default_sidebar_pct(),
            show_footer: true,
            show_header: true,
            responsive_breakpoint: default_responsive_breakpoint(),
            input_max_chars: default_input_max_chars(),
            banner_auto_dismiss_secs: default_banner_auto_dismiss(),
            event_log_max_entries: default_event_log_max(),
            use_new_layout: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolEventFormat {
    /// Always show expanded tool call blocks.
    Full,
    /// Tool calls are collapsed by default; expand to see details.
    Collapsed,
    /// Only show tool names after completion (no running state visible).
    FinalOnly,
}

impl Default for ToolEventFormat {
    fn default() -> Self {
        Self::Collapsed
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ThemeConfig {
    #[serde(default = "default_theme_name")]
    pub name: String,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            name: default_theme_name(),
        }
    }
}

fn default_theme_name() -> String {
    "default".into()
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct KeybindingsConfig {
    #[serde(default = "default_submit_key")]
    pub submit: String,
    #[serde(default = "default_cancel_key")]
    pub cancel: String,
    #[serde(default = "default_quit_key")]
    pub quit: String,
    #[serde(default = "default_help_key")]
    pub help: String,
    #[serde(default = "default_search_key")]
    pub search: String,
    #[serde(default = "default_steer_key")]
    pub steer: String,
}

impl Default for KeybindingsConfig {
    fn default() -> Self {
        Self {
            submit: default_submit_key(),
            cancel: default_cancel_key(),
            quit: default_quit_key(),
            help: default_help_key(),
            search: default_search_key(),
            steer: default_steer_key(),
        }
    }
}

fn default_submit_key() -> String {
    "enter".into()
}
fn default_cancel_key() -> String {
    "ctrl-c".into()
}
fn default_quit_key() -> String {
    "ctrl-q".into()
}
fn default_help_key() -> String {
    "f1".into()
}
fn default_search_key() -> String {
    "ctrl-f".into()
}
fn default_steer_key() -> String {
    "ctrl-g".into()
}
fn default_loop_id() -> String {
    "simple_react".into()
}

fn default_sidebar_pct() -> u8 {
    25
}

fn default_responsive_breakpoint() -> u16 {
    120
}

fn default_input_max_chars() -> usize {
    500
}

fn default_banner_auto_dismiss() -> u64 {
    5
}

fn default_event_log_max() -> usize {
    200
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
    #[serde(default = "default_telegram_data_dir")]
    pub data_dir: PathBuf,
    #[serde(default)]
    pub attachments: TelegramAttachmentConfig,
    #[serde(default)]
    pub send_file: TelegramSendFileConfig,
    /// Default agent loop (e.g. "simple_react", "problem_solving").
    /// Defaults to "simple_react".
    #[serde(default = "default_loop_id")]
    pub default_loop: String,
}

fn default_telegram_token_env() -> String {
    "TELEGRAM_BOT_TOKEN".into()
}

fn default_telegram_events_dir() -> PathBuf {
    PathBuf::from("./events")
}

fn default_telegram_data_dir() -> PathBuf {
    PathBuf::from("./data")
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TelegramSendFileConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_max_send_file_mb")]
    pub max_file_size_mb: u64,
    #[serde(default)]
    pub allowed_extensions: Vec<String>,
}

impl Default for TelegramSendFileConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_file_size_mb: default_max_send_file_mb(),
            allowed_extensions: Vec::new(),
        }
    }
}

fn default_max_send_file_mb() -> u64 {
    50
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
            data_dir: default_telegram_data_dir(),
            attachments: TelegramAttachmentConfig::default(),
            send_file: TelegramSendFileConfig::default(),
            default_loop: default_loop_id(),
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
    pub tui: Option<TuiConfig>,
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
            .field("tui", &self.tui)
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
#[serde(rename_all = "snake_case")]
pub enum SummarizerKind {
    /// Role-label summarizer (trivial, fast, no LLM call).
    Simple,
    /// LLM-driven semantic summarizer (preserves topic identity).
    Semantic,
}

impl Default for SummarizerKind {
    fn default() -> Self {
        Self::Semantic
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MemoryConfig {
    pub max_tokens: usize,
    pub compress_at_ratio: f64,
    /// Maximum number of recent messages to load from session history.
    /// Set to 0 to disable (load all, backward-compatible behavior).
    #[serde(default = "default_context_window_size")]
    pub context_window_size: usize,
    /// Hard token budget for loaded history.
    /// If the window exceeds this, drop oldest messages until it fits.
    /// Set to 0 to disable.
    #[serde(default = "default_max_context_tokens")]
    pub max_context_tokens: usize,
    /// Which summarizer to use for context compression.
    /// "semantic" (default) uses an LLM call; "simple" uses role labels.
    #[serde(default)]
    pub summarizer: SummarizerKind,
}

fn default_context_window_size() -> usize {
    50
}

fn default_max_context_tokens() -> usize {
    12000
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

        // Validate binary allowlist entries (skipped when allow_all_binaries is set).
        // Glob patterns are expanded later; we only warn for obvious issues here.
        if !self.sandbox.allow_all_binaries {
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
                                errors
                                    .push(format!("binary is not executable: {}", path.display()));
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
        }

        // Warn when allow_all_binaries is used with non-container sandbox modes.
        if self.sandbox.allow_all_binaries
            && matches!(
                self.sandbox.mode,
                SandboxMode::Host | SandboxMode::Capability
            )
        {
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
        if self.memory.context_window_size > 10_000 {
            errors.push(format!(
                "memory.context_window_size must not exceed 10,000, got {}",
                self.memory.context_window_size
            ));
        }
        if self.memory.max_context_tokens > 1_000_000 {
            errors.push(format!(
                "memory.max_context_tokens must not exceed 1,000,000, got {}",
                self.memory.max_context_tokens
            ));
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
                errors.push(
                    "telegram.attachments.max_file_size_mb must not exceed Telegram's 50 MB limit"
                        .into(),
                );
            }
            if telegram.send_file.enabled && telegram.send_file.max_file_size_mb == 0 {
                errors.push("telegram.send_file.max_file_size_mb must be > 0".into());
            }
            if telegram.send_file.enabled && telegram.send_file.max_file_size_mb > 2000 {
                errors.push(
                    "telegram.send_file.max_file_size_mb must not exceed 2000 MB (Bot API limit)"
                        .into(),
                );
            }
        }

        // Loop config validation.
        if self.agent.loop_config.max_refinement_iterations == 0 {
            errors.push("agent.loop.max_refinement_iterations must be at least 1".into());
        }
        if self.agent.loop_config.max_delegation_depth > 10 {
            tracing::warn!(
                "agent.loop.max_delegation_depth is {}. Values above 10 are suspicious — \
                 the default is 2. Did you mean to set it that high?",
                self.agent.loop_config.max_delegation_depth
            );
        }
        // `simple_react` is always the entry point, not a delegation target.
        // Warn if it appears in `enabled_loops` (harmless but confusing).
        if self
            .agent
            .loop_config
            .enabled_loops
            .contains(&"simple_react".to_string())
        {
            tracing::warn!(
                "'simple_react' listed in agent.loop.enabled_loops — it is always \
                 the entry point and cannot be delegated to. Removing it from \
                 effective enabled list."
            );
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
  context_window_size: 50
  max_context_tokens: 12000
  summarizer: semantic
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
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("HOME", dir.path()) };
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

    // ── TelegramSendFileConfig tests ────────────────────────────────────

    #[test]
    fn send_file_config_defaults() {
        let config = TelegramSendFileConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_file_size_mb, 50);
        assert!(config.allowed_extensions.is_empty());
    }

    #[test]
    fn send_file_config_validate_zero_max_size() {
        let dir = tempfile::tempdir().unwrap();
        let binary = executable_file(&dir);
        let mut c: Config = serde_yaml::from_str(&yaml(&dir, &binary)).unwrap();
        c.telegram = Some(TelegramConfig {
            send_file: TelegramSendFileConfig {
                enabled: true,
                max_file_size_mb: 0,
                allowed_extensions: vec![],
            },
            allowed_chat_ids: vec![1],
            ..Default::default()
        });
        let err = c.validate().unwrap_err().to_string();
        assert!(
            err.contains("send_file.max_file_size_mb must be > 0"),
            "Expected max_file_size_mb validation error, got: {err}"
        );
        let _ = dir;
    }

    #[test]
    fn send_file_config_validate_exceeds_max() {
        let dir = tempfile::tempdir().unwrap();
        let binary = executable_file(&dir);
        let mut c: Config = serde_yaml::from_str(&yaml(&dir, &binary)).unwrap();
        c.telegram = Some(TelegramConfig {
            send_file: TelegramSendFileConfig {
                enabled: true,
                max_file_size_mb: 5000,
                allowed_extensions: vec![],
            },
            allowed_chat_ids: vec![1],
            ..Default::default()
        });
        let err = c.validate().unwrap_err().to_string();
        assert!(
            err.contains("send_file.max_file_size_mb must not exceed 2000 MB"),
            "Expected max_file_size_mb validation error, got: {err}"
        );
        let _ = dir;
    }

    #[test]
    fn send_file_config_disabled_skips_validation() {
        let dir = tempfile::tempdir().unwrap();
        let binary = executable_file(&dir);
        let mut c: Config = serde_yaml::from_str(&yaml(&dir, &binary)).unwrap();
        c.telegram = Some(TelegramConfig {
            send_file: TelegramSendFileConfig {
                enabled: false,
                max_file_size_mb: 0,
                allowed_extensions: vec![],
            },
            allowed_chat_ids: vec![1],
            ..Default::default()
        });
        assert!(
            c.validate().is_ok(),
            "Disabled send_file should not validate size"
        );
        let _ = dir;
    }

    #[test]
    fn send_file_config_allowed_extensions() {
        let config = TelegramSendFileConfig {
            enabled: true,
            max_file_size_mb: 50,
            allowed_extensions: vec![".svg".into(), ".png".into()],
        };
        assert_eq!(config.allowed_extensions.len(), 2);
        assert!(config.allowed_extensions.contains(&".svg".to_string()));
        assert!(config.allowed_extensions.contains(&".png".to_string()));
    }

    #[test]
    fn send_file_yaml_parse() {
        let yaml = r#"
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
    - /usr/bin/echo
workspace:
  root: /
environment:
  allowed:
    - HOME
memory:
  max_tokens: 4096
  compress_at_ratio: 0.8
  context_window_size: 50
  max_context_tokens: 12000
  summarizer: semantic
plugins:
  dir: /tmp
  modules: []
telegram:
  allowed_chat_ids:
    - 123
  send_file:
    enabled: true
    max_file_size_mb: 100
    allowed_extensions:
      - .svg
      - .png
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        let tg = config.telegram.unwrap();
        assert!(tg.send_file.enabled);
        assert_eq!(tg.send_file.max_file_size_mb, 100);
        assert_eq!(tg.send_file.allowed_extensions, vec![".svg", ".png"]);
    }
}
