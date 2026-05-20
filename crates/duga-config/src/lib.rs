//! YAML configuration loading and validation.

pub mod config;
pub mod error;

pub use config::{
    Config, EnvironmentConfig, FrontendConfig, MemoryConfig, PluginConfig, PluginModuleConfig,
    ProgressMode, ProviderKind, SandboxConfig, SandboxMode, SummarizerKind,
    TelegramAttachmentConfig, TelegramConfig, ThinkingLevel, WasiCapabilities, WorkspaceConfig,
};
pub use error::ConfigError;
