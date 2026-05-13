//! Workspace — capability-based filesystem isolation.
//!
//! `Workspace` wraps a `cap_std::fs::Dir` rooted at a user-provided path.
//! All file access goes through `resolve()` which validates that paths
//! never escape the workspace directory.

use crate::error::WorkspaceError;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A capability-bounded workspace directory.
#[derive(Clone, Debug)]
pub struct Workspace {
    root_dir: Arc<cap_std::fs::Dir>,
    root_path: PathBuf,
}

impl Workspace {
    /// Open a workspace rooted at `root`.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, WorkspaceError> {
        let root = root.as_ref();

        if !root.exists() {
            return Err(WorkspaceError::NotFound(root.to_path_buf()));
        }
        if !root.is_dir() {
            return Err(WorkspaceError::NotADirectory(root.to_path_buf()));
        }

        let root_path = root.canonicalize().map_err(|e| match e.kind() {
            std::io::ErrorKind::PermissionDenied => WorkspaceError::PermissionDenied(root.into()),
            _ => WorkspaceError::Io(e.to_string()),
        })?;

        let root_dir = cap_std::fs::Dir::open_ambient_dir(&root_path, cap_std::ambient_authority())
            .map_err(|e| WorkspaceError::Io(e.to_string()))?;

        Ok(Self {
            root_dir: Arc::new(root_dir),
            root_path,
        })
    }

    /// Returns a reference to the capability-bounded directory.
    pub fn root_dir(&self) -> &cap_std::fs::Dir {
        &self.root_dir
    }

    /// Returns the canonicalized root path of this workspace.
    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    /// Resolve a workspace-relative path and validate it stays within bounds.
    pub fn resolve(&self, relative_path: &Path) -> Result<PathBuf, WorkspaceError> {
        if relative_path.as_os_str().is_empty() {
            return Ok(PathBuf::from("."));
        }

        if relative_path.is_absolute() {
            return Err(WorkspaceError::PathEscapesWorkspace(relative_path.into()));
        }

        if cfg!(windows) {
            if let Some(prefix) = relative_path.components().next() {
                if matches!(prefix, std::path::Component::Prefix(_)) {
                    return Err(WorkspaceError::PathEscapesWorkspace(relative_path.into()));
                }
            }
        }

        let mut components = Vec::new();
        for component in relative_path.components() {
            match component {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    return Err(WorkspaceError::PathEscapesWorkspace(relative_path.into()));
                }
                std::path::Component::Normal(os) => {
                    components.push(os);
                }
                std::path::Component::Prefix(_) => {
                    return Err(WorkspaceError::PathEscapesWorkspace(relative_path.into()));
                }
                std::path::Component::RootDir => {}
            }
        }

        let resolved: PathBuf = components.iter().collect();
        if resolved.as_os_str().is_empty() {
            Ok(PathBuf::from("."))
        } else {
            Ok(resolved)
        }
    }

    // ── TASK-5.4: Directory and path utility methods ─────────────────────

    /// Create a directory and all of its parent components.
    pub fn create_dir_all(&self, path: &Path) -> Result<(), WorkspaceError> {
        let resolved = self.resolve(path)?;
        self.root_dir.create_dir_all(&resolved).map_err(Into::into)
    }

    /// Remove a file.
    pub fn remove_file(&self, path: &Path) -> Result<(), WorkspaceError> {
        let resolved = self.resolve(path)?;
        self.root_dir.remove_file(&resolved).map_err(Into::into)
    }

    /// Check whether a path exists within the workspace.
    pub fn exists(&self, path: &Path) -> bool {
        match self.resolve(path) {
            Ok(resolved) => self.root_dir.metadata(&resolved).is_ok(),
            Err(_) => false,
        }
    }

    /// Check whether a path is a regular file.
    pub fn is_file(&self, path: &Path) -> bool {
        match self.resolve(path) {
            Ok(resolved) => self
                .root_dir
                .metadata(&resolved)
                .map(|m| m.is_file())
                .unwrap_or(false),
            Err(_) => false,
        }
    }

    /// Check whether a path is a directory.
    pub fn is_dir(&self, path: &Path) -> bool {
        match self.resolve(path) {
            Ok(resolved) => self
                .root_dir
                .metadata(&resolved)
                .map(|m| m.is_dir())
                .unwrap_or(false),
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_workspace_open_success() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        assert_eq!(ws.root_path(), dir.path().canonicalize().unwrap());
    }

    #[test]
    fn test_workspace_open_not_found() {
        let result = Workspace::open("/nonexistent/path/xyz");
        assert!(matches!(result, Err(WorkspaceError::NotFound(_))));
    }

    #[test]
    fn test_workspace_open_not_a_directory() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("myfile.txt");
        std::fs::write(&file_path, "test").unwrap();
        let result = Workspace::open(&file_path);
        assert!(matches!(result, Err(WorkspaceError::NotADirectory(_))));
    }

    #[test]
    fn test_resolve_relative_path() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let resolved = ws.resolve(Path::new("src/main.rs")).unwrap();
        assert_eq!(resolved, PathBuf::from("src/main.rs"));
    }

    #[test]
    fn test_resolve_dot() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let resolved = ws.resolve(Path::new(".")).unwrap();
        assert_eq!(resolved, PathBuf::from("."));
    }

    #[test]
    fn test_resolve_empty() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let resolved = ws.resolve(Path::new("")).unwrap();
        assert_eq!(resolved, PathBuf::from("."));
    }

    #[test]
    fn test_resolve_absolute_rejected() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let result = ws.resolve(Path::new("/etc/passwd"));
        assert!(matches!(
            result,
            Err(WorkspaceError::PathEscapesWorkspace(_))
        ));
    }

    #[test]
    fn test_resolve_parent_rejected() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let result = ws.resolve(Path::new("foo/../bar"));
        assert!(matches!(
            result,
            Err(WorkspaceError::PathEscapesWorkspace(_))
        ));
    }

    #[test]
    fn test_resolve_dot_prefix() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let resolved = ws.resolve(Path::new("./src/main.rs")).unwrap();
        assert_eq!(resolved, PathBuf::from("src/main.rs"));
    }

    #[test]
    fn test_resolve_double_slash_normalized() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let resolved = ws.resolve(Path::new("foo//bar")).unwrap();
        assert_eq!(resolved, PathBuf::from("foo/bar"));
    }

    // ── TASK-5.4 tests ──────────────────────────────────────────────────

    #[test]
    fn test_create_dir_all() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        ws.create_dir_all(Path::new("a/b/c")).unwrap();
        assert!(ws.exists(Path::new("a/b/c")));
        assert!(ws.is_dir(Path::new("a/b/c")));
    }

    #[test]
    fn test_create_dir_all_idempotent() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        ws.create_dir_all(Path::new(".")).unwrap(); // no-op
        assert!(ws.exists(Path::new(".")));
    }

    #[test]
    fn test_write_and_remove_file() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        ws.create_dir_all(Path::new("sub")).unwrap();
        // Use std::fs to create the file via the resolved workspace path
        let resolved = ws.resolve(Path::new("sub/test.txt")).unwrap();
        let full_path = ws.root_path().join(&resolved);
        std::fs::write(&full_path, b"hello").unwrap();
        assert!(ws.exists(Path::new("sub/test.txt")));
        assert!(ws.is_file(Path::new("sub/test.txt")));
        ws.remove_file(Path::new("sub/test.txt")).unwrap();
        assert!(!ws.exists(Path::new("sub/test.txt")));
    }

    #[test]
    fn test_exists_false_for_missing() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        assert!(!ws.exists(Path::new("nonexistent")));
    }

    #[test]
    fn test_is_file_false_for_dir() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        ws.create_dir_all(Path::new("mydir")).unwrap();
        assert!(!ws.is_file(Path::new("mydir")));
        assert!(ws.is_dir(Path::new("mydir")));
    }

    #[test]
    fn test_is_dir_false_for_file() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let f = ws.root_dir().create("test.txt").unwrap();
        drop(f);
        assert!(!ws.is_dir(Path::new("test.txt")));
        assert!(ws.is_file(Path::new("test.txt")));
    }

    #[test]
    fn test_remove_file_escaping_path_rejected() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let result = ws.remove_file(Path::new("../etc/passwd"));
        assert!(matches!(
            result,
            Err(crate::error::WorkspaceError::PathEscapesWorkspace(_))
        ));
    }

    #[test]
    fn test_create_dir_all_escaping_path_rejected() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let result = ws.create_dir_all(Path::new("../../evil"));
        assert!(matches!(
            result,
            Err(crate::error::WorkspaceError::PathEscapesWorkspace(_))
        ));
    }
}
