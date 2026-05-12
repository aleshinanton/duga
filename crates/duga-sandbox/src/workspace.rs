//! Workspace — capability-based filesystem isolation.
//!
//! `Workspace` wraps a `cap_std::fs::Dir` rooted at a user-provided path.
//! All file access goes through `resolve()` which validates that paths
//! never escape the workspace directory.

use crate::error::WorkspaceError;
use std::path::{Path, PathBuf};

/// A capability-bounded workspace directory.
#[derive(Debug)]
pub struct Workspace {
    root_dir: cap_std::fs::Dir,
    root_path: PathBuf,
}

impl Workspace {
    /// Open a workspace rooted at `root`.
    ///
    /// The path must exist and be a directory. The root is canonicalized
    /// so that symlinks are resolved.
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
            _ => WorkspaceError::Io(e),
        })?;

        let root_dir =
            cap_std::fs::Dir::open_ambient_dir(&root_path, cap_std::ambient_authority())
                .map_err(WorkspaceError::Io)?;

        Ok(Self { root_dir, root_path })
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
    ///
    /// Rejects:
    /// - Absolute paths
    /// - Paths containing `..` components
    /// - Windows drive-letter paths
    ///
    /// Returns the cleaned relative path on success.
    pub fn resolve(&self, relative_path: &Path) -> Result<PathBuf, WorkspaceError> {
        // Reject empty path — treat as workspace root
        if relative_path.as_os_str().is_empty() {
            return Ok(PathBuf::from("."));
        }

        // Reject absolute paths
        if relative_path.is_absolute() {
            return Err(WorkspaceError::PathEscapesWorkspace(relative_path.into()));
        }

        // Reject Windows drive-letter paths
        if cfg!(windows) {
            if let Some(prefix) = relative_path.components().next() {
                if matches!(prefix, std::path::Component::Prefix(_)) {
                    return Err(WorkspaceError::PathEscapesWorkspace(relative_path.into()));
                }
            }
        }

        // Normalize: remove ".", reject "..", collect remaining components
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
        assert!(matches!(result, Err(WorkspaceError::PathEscapesWorkspace(_))));
    }

    #[test]
    fn test_resolve_parent_rejected() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let result = ws.resolve(Path::new("foo/../bar"));
        assert!(matches!(result, Err(WorkspaceError::PathEscapesWorkspace(_))));
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
}