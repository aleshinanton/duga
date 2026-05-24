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

#[cfg(test)]
mod tests {
    use super::*;

    // ── WorkspaceError ──────────────────────────────────────────────────

    #[test]
    fn workspace_error_display_not_found() {
        let err = WorkspaceError::NotFound(PathBuf::from("/tmp/missing"));
        let msg = err.to_string();
        assert!(msg.contains("not found"));
        assert!(msg.contains("missing"));
    }

    #[test]
    fn workspace_error_display_path_escapes() {
        let err = WorkspaceError::PathEscapesWorkspace(PathBuf::from("../etc"));
        let msg = err.to_string();
        assert!(msg.contains("escapes"));
        assert!(msg.contains("../etc"));
    }

    #[test]
    fn workspace_error_display_permission_denied() {
        let err = WorkspaceError::PermissionDenied(PathBuf::from("/root/secret"));
        assert!(err.to_string().contains("permission denied"));
    }

    #[test]
    fn workspace_error_display_not_a_directory() {
        let err = WorkspaceError::NotADirectory(PathBuf::from("/tmp/file.txt"));
        assert!(err.to_string().contains("not a directory"));
    }

    #[test]
    fn workspace_error_display_io() {
        let err = WorkspaceError::Io("broken pipe".into());
        assert!(err.to_string().contains("i/o error"));
        assert!(err.to_string().contains("broken pipe"));
    }

    #[test]
    fn workspace_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let ws_err: WorkspaceError = io_err.into();
        assert!(matches!(ws_err, WorkspaceError::Io(_)));
        assert!(ws_err.to_string().contains("file missing"));
    }

    #[test]
    fn workspace_error_equality() {
        let a = WorkspaceError::NotFound(PathBuf::from("/x"));
        let b = WorkspaceError::NotFound(PathBuf::from("/x"));
        assert_eq!(a, b);
    }

    // ── BinaryError ─────────────────────────────────────────────────────

    #[test]
    fn binary_error_display_not_found() {
        let err = BinaryError::NotFound("python".into());
        assert!(err.to_string().contains("not found"));
        assert!(err.to_string().contains("python"));
    }

    #[test]
    fn binary_error_display_not_allowed() {
        let err = BinaryError::NotAllowed("rm".into());
        assert!(err.to_string().contains("not allowed"));
        assert!(err.to_string().contains("rm"));
    }

    #[test]
    fn binary_error_display_which_failed() {
        let err = BinaryError::WhichFailed("java".into(), "no java in PATH".into());
        let msg = err.to_string();
        assert!(msg.contains("which failed"));
        assert!(msg.contains("java"));
        assert!(msg.contains("no java in PATH"));
    }

    #[test]
    fn binary_error_equality() {
        let a = BinaryError::NotFound("cmd".into());
        let b = BinaryError::NotFound("cmd".into());
        assert_eq!(a, b);
        assert_ne!(a, BinaryError::NotAllowed("cmd".into()));
    }

    // ── ShellSessionError ───────────────────────────────────────────────

    #[test]
    fn shell_session_error_missing_cd_arg() {
        let err = ShellSessionError::MissingCdArgument;
        assert!(err.to_string().contains("cd requires"));
    }

    #[test]
    fn shell_session_error_invalid_export_syntax() {
        let err = ShellSessionError::InvalidExportSyntax("FOO".into());
        assert!(err.to_string().contains("export requires"));
        assert!(err.to_string().contains("FOO"));
    }

    #[test]
    fn shell_session_error_protected_env() {
        let err = ShellSessionError::ProtectedEnv("PATH".into());
        assert!(err.to_string().contains("protected"));
        assert!(err.to_string().contains("PATH"));
    }

    #[test]
    fn errors_are_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<WorkspaceError>();
        assert_send_sync::<BinaryError>();
        assert_send_sync::<ShellSessionError>();
    }

    #[test]
    fn errors_are_cloneable() {
        let err = WorkspaceError::NotFound(PathBuf::from("/x"));
        let cloned = err.clone();
        assert_eq!(err, cloned);

        let err = BinaryError::NotAllowed("cmd".into());
        let cloned = err.clone();
        assert_eq!(err, cloned);
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
    #[error("protected environment variable cannot be set: {0}")]
    ProtectedEnv(String),
}
