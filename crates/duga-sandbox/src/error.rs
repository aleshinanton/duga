//! Error types for the duga-sandbox crate.

use std::path::PathBuf;
use thiserror::Error;

/// Errors from workspace operations.
#[derive(Clone, Debug, Error, PartialEq)]
pub enum WorkspaceError {
    #[error("not found: {0:?}")]
    NotFound(PathBuf),
    #[error("not a directory: {0:?}")]
    NotADirectory(PathBuf),
    #[error("permission denied: {0:?}")]
    PermissionDenied(PathBuf),
    #[error("i/o error: {0}")]
    Io(String),
    #[error("path escapes workspace: {0:?}")]
    PathEscapesWorkspace(PathBuf),
}

impl From<std::io::Error> for WorkspaceError {
    fn from(e: std::io::Error) -> Self {
        WorkspaceError::Io(e.to_string())
    }
}

#[derive(Clone, Debug, Error, PartialEq)]
pub enum BinaryError {
    #[error("binary not found: {0}")]
    NotFound(String),
    #[error("binary not allowed: {0}")]
    NotAllowed(String),
    #[error("which failed for '{0}': {1}")]
    WhichFailed(String, String),
}

#[derive(Clone, Debug, Error, PartialEq)]
pub enum ShellSessionError {
    #[error("cd requires a path argument")]
    MissingCdArgument,
    #[error("export requires KEY=value syntax, got: {0}")]
    InvalidExportSyntax(String),
    #[error("unset requires a variable name argument")]
    MissingUnsetArgument,
    #[error("invalid cwd: {0}")]
    InvalidCwd(String),
}
