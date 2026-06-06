//! YAML configuration loading and validation.

pub mod config;
pub mod error;
pub mod mcp_config;

pub use config::{
    Config, EnvironmentConfig, FrontendConfig, KeybindingsConfig, MemoryConfig, PluginConfig,
    PluginModuleConfig, ProgressMode, ProviderKind, SandboxConfig, SandboxMode, SummarizerKind,
    TelegramAttachmentConfig, TelegramConfig, TelegramSendFileConfig, ThemeConfig, ThinkingLevel,
    ToolEventFormat, TuiConfig, WasiCapabilities, WorkspaceConfig,
};
pub use error::ConfigError;
pub use mcp_config::{
    expand_env_vars, server_identity_hash, McpDirectToolsMode, McpLifecycleMode,
    McpPluginConfig, McpServerDefinition, McpSettings,
};
