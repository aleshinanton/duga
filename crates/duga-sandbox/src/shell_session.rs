//! Shell session state tracking.
//!
//! `ShellSession` tracks persistent shell state (cwd, env, id) across
//! multiple tool calls without keeping a running shell process.
//! Command classification (`classify`) and mutation (`apply`) are pure
//! operations on session state.

use crate::error::ShellSessionError;
use crate::workspace::Workspace;
use std::collections::HashMap;
use std::path::PathBuf;
use uuid::Uuid;

/// Represents a shell command classified by `ShellSession::classify`.
#[derive(Clone, Debug, PartialEq)]
pub enum SessionCommand {
    /// Change directory.
    Cd(PathBuf),
    /// Export an environment variable (`KEY=value`).
    Export(String, String),
    /// Unset an environment variable.
    Unset(String),
    /// Print working directory.
    Pwd,
    /// Spawn an external command.
    Spawn(Vec<String>),
}

/// Tracks state for a single interactive shell session.
#[derive(Debug)]
pub struct ShellSession {
    id: Uuid,
    cwd: PathBuf,
    env: HashMap<String, String>,
}

impl ShellSession {
    /// Create a new session with the given initial working directory.
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            id: Uuid::new_v4(),
            cwd,
            env: HashMap::new(),
        }
    }

    /// Unique session identifier.
    pub fn id(&self) -> Uuid {
        self.id
    }

    /// Current working directory (workspace-relative).
    pub fn cwd(&self) -> &PathBuf {
        &self.cwd
    }

    /// Current environment variables (mutable reference for tests).
    pub fn env_mut(&mut self) -> &mut HashMap<String, String> {
        &mut self.env
    }

    /// Current environment variables.
    pub fn env(&self) -> &HashMap<String, String> {
        &self.env
    }

    /// Classify a command-line argv into a `SessionCommand` variant.
    pub fn classify(command: &[String]) -> SessionCommand {
        match command.first().map(|s| s.as_str()) {
            Some("cd") => {
                let path = command.get(1).cloned().unwrap_or_default();
                SessionCommand::Cd(PathBuf::from(path))
            }
            Some("export") => {
                let rest = command.get(1).cloned().unwrap_or_default();
                if let Some(eq) = rest.find('=') {
                    let key = rest[..eq].to_string();
                    let value = rest[eq + 1..].to_string();
                    SessionCommand::Export(key, value)
                } else {
                    SessionCommand::Export(rest, String::new())
                }
            }
            Some("unset") => {
                let key = command.get(1).cloned().unwrap_or_default();
                SessionCommand::Unset(key)
            }
            Some("pwd") => SessionCommand::Pwd,
            _ => SessionCommand::Spawn(command.to_vec()),
        }
    }

    /// Apply a classified command, mutating session state where appropriate.
    ///
    /// - `Cd` → validates via workspace resolve, updates cwd
    /// - `Export` → updates env map
    /// - `Unset` → removes from env map
    /// - `Pwd` → returns cwd as `Some(String)`
    /// - `Spawn` → returns `None` (caller handles execution)
    pub fn apply(
        &mut self,
        cmd: SessionCommand,
        workspace: &Workspace,
    ) -> Result<Option<String>, ShellSessionError> {
        match cmd {
            SessionCommand::Cd(path) => {
                if path.as_os_str().is_empty() {
                    return Err(ShellSessionError::MissingCdArgument);
                }
                let resolved = workspace
                    .resolve(&path)
                    .map_err(|_| ShellSessionError::InvalidCwd(path.display().to_string()))?;
                self.cwd = resolved;
                Ok(None)
            }
            SessionCommand::Export(key, value) => {
                self.env.insert(key, value);
                Ok(None)
            }
            SessionCommand::Unset(key) => {
                if key.is_empty() {
                    return Err(ShellSessionError::MissingUnsetArgument);
                }
                self.env.remove(&key);
                Ok(None)
            }
            SessionCommand::Pwd => Ok(Some(self.cwd.display().to_string())),
            SessionCommand::Spawn(_) => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_ws() -> (tempfile::TempDir, Workspace) {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        (dir, ws)
    }

    #[test]
    fn test_session_new() {
        let sess = ShellSession::new(PathBuf::from("."));
        assert_eq!(sess.cwd().to_str(), Some("."));
        assert!(sess.env().is_empty());
    }

    #[test]
    fn test_classify_cd() {
        let cmd = ShellSession::classify(&["cd".into(), "src".into()]);
        assert_eq!(cmd, SessionCommand::Cd(PathBuf::from("src")));
    }

    #[test]
    fn test_classify_export() {
        let cmd = ShellSession::classify(&["export".into(), "FOO=bar".into()]);
        assert_eq!(cmd, SessionCommand::Export("FOO".into(), "bar".into()));
    }

    #[test]
    fn test_classify_export_no_value() {
        let cmd = ShellSession::classify(&["export".into(), "FOO".into()]);
        assert_eq!(cmd, SessionCommand::Export("FOO".into(), String::new()));
    }

    #[test]
    fn test_classify_unset() {
        let cmd = ShellSession::classify(&["unset".into(), "FOO".into()]);
        assert_eq!(cmd, SessionCommand::Unset("FOO".into()));
    }

    #[test]
    fn test_classify_pwd() {
        let cmd = ShellSession::classify(&["pwd".into()]);
        assert_eq!(cmd, SessionCommand::Pwd);
    }

    #[test]
    fn test_classify_spawn() {
        let cmd = ShellSession::classify(&["ls".into(), "-la".into()]);
        assert_eq!(
            cmd,
            SessionCommand::Spawn(vec!["ls".into(), "-la".into()])
        );
    }

    #[test]
    fn test_apply_cd() {
        let (_dir, ws) = make_ws();
        let mut sess = ShellSession::new(PathBuf::from("."));
        let result = sess.apply(SessionCommand::Cd(PathBuf::from(".")), &ws);
        assert!(result.is_ok());
        assert_eq!(sess.cwd().to_str(), Some("."));
    }

    #[test]
    fn test_apply_pwd() {
        let (_dir, _ws) = make_ws();
        let mut sess = ShellSession::new(PathBuf::from("src"));
        let result = sess.apply(SessionCommand::Pwd, &Workspace::open("/tmp").unwrap()).unwrap();
        assert_eq!(result, Some("src".into()));
    }

    #[test]
    fn test_apply_export() {
        let (_dir, ws) = make_ws();
        let mut sess = ShellSession::new(PathBuf::from("."));
        sess.apply(
            SessionCommand::Export("FOO".into(), "bar".into()),
            &ws,
        )
        .unwrap();
        assert_eq!(sess.env().get("FOO"), Some(&"bar".to_string()));
    }

    #[test]
    fn test_apply_unset() {
        let (_dir, ws) = make_ws();
        let mut sess = ShellSession::new(PathBuf::from("."));
        sess.env_mut().insert("FOO".into(), "bar".into());
        sess.apply(SessionCommand::Unset("FOO".into()), &ws)
            .unwrap();
        assert!(!sess.env().contains_key("FOO"));
    }

    #[test]
    fn test_apply_spawn_returns_none() {
        let (_dir, ws) = make_ws();
        let mut sess = ShellSession::new(PathBuf::from("."));
        let result = sess
            .apply(SessionCommand::Spawn(vec!["ls".into()]), &ws)
            .unwrap();
        assert!(result.is_none());
    }
}