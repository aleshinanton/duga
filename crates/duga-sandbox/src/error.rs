//! Error types for the duga-sandbox crate.

use std::path::PathBuf;
use thiserror::Error;

/// Errors from workspace operations.
#[derive(Clone, Debug, Error, PartialEq)]
pub enum WorkspaceError {
    /// The specified path does not exist.
    #[error("not found: {0:?}")]
    NotFound(PathBuf),
    /// The specified path is not a directory.
    #[error("not a directory: {0:?}")]
    NotADirectory(PathBuf),
    /// Permission denied for the specified path.
    #[error("permission denied: {0:?}")]
    PermissionDenied(PathBuf),
    /// An I/O error occurred.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    /// The resolved path escapes the workspace boundary.
    #[error("path escapes workspace: {0:?}")]
    PathEscapesWorkspace(PathBuf),
}

/// Errors from binary registry operations.
#[derive(Clone, Debug, Error, PartialEq)]
pub enum BinaryError {
    /// The binary was not found on the system PATH.
    #[error("binary not found: {0}")]
    NotFound(String),
    /// The binary name is not in the allowed list.
    #[error("binary not allowed: {0}")]
    NotAllowed(String),
    /// `which` failed for the binary.
    #[error("which failed for '{0}': {1}")]
    WhichFailed(String, String),
}

/// Errors from shell session operations.
#[derive(Clone, Debug, Error, PartialEq)]
pub enum ShellSessionError {
    /// `cd` was called with no argument.
    #[error("cd requires a path argument")]
    MissingCdArgument,
    /// `export` was called without `KEY=value` syntax.
    #[error("export requires KEY=value syntax, got: {0}")]
    InvalidExportSyntax(String),
    /// `unset` was called with no argument.
    #[error("unset requires a variable name argument")]
    MissingUnsetArgument,
    /// `cd` target could not be resolved within the workspace.
    #[error("invalid cwd: {0}")]
    InvalidCwd(String),
}