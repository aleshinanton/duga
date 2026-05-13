use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config file not found: {0}")]
    FileNotFound(PathBuf),
    #[error("failed to parse config: {0}")]
    ParseError(String),
    #[error("config validation failed: {0}")]
    ValidationError(String),
}
