//! Binary registry — resolved path allowlist for subprocess spawning.

use crate::error::BinaryError;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Maps allowed binary names to their resolved absolute paths.
#[derive(Clone, Debug, Default)]
pub struct BinaryRegistry {
    allowed: HashMap<String, PathBuf>,
}

impl BinaryRegistry {
    /// Build a registry by resolving each allowed binary name.
    ///
    /// # Errors
    /// Returns `BinaryError::NotFound` if any name cannot be resolved,
    /// or `BinaryError::WhichFailed` if `which` itself fails.
    ///
    /// Binary names containing `/` are rejected to prevent path-injection
    /// (e.g. `./malicious` or `../../bin/sh`).
    pub fn new(names: &[String]) -> Result<Self, BinaryError> {
        let mut allowed = HashMap::new();
        for name in names {
            // Reject names with path separators
            if name.contains('/') || (cfg!(windows) && name.contains('\\')) {
                return Err(BinaryError::NotFound(name.clone()));
            }
            if name.is_empty() {
                return Err(BinaryError::NotFound("<empty>".into()));
            }

            let path = which::which(name)
                .map_err(|e| BinaryError::WhichFailed(name.clone(), e.to_string()))?;
            let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            allowed.insert(name.clone(), canonical);
        }
        Ok(Self { allowed })
    }

    /// Resolve an allowed binary name to its absolute path.
    pub fn resolve(&self, name: &str) -> Result<&Path, BinaryError> {
        self.allowed
            .get(name)
            .map(|p| p.as_path())
            .ok_or_else(|| BinaryError::NotAllowed(name.to_string()))
    }

    /// Check if a binary name is in the allowlist.
    pub fn is_allowed(&self, name: &str) -> bool {
        self.allowed.contains_key(name)
    }

    /// Returns the number of registered binaries.
    pub fn len(&self) -> usize {
        self.allowed.len()
    }

    /// Returns `true` if no binaries are registered.
    pub fn is_empty(&self) -> bool {
        self.allowed.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_resolve_echo() {
        let reg = BinaryRegistry::new(&["echo".into()]).unwrap();
        let path = reg.resolve("echo").unwrap();
        assert!(path.is_absolute());
    }

    #[test]
    fn test_registry_not_allowed() {
        let reg = BinaryRegistry::new(&["echo".into()]).unwrap();
        let result = reg.resolve("nonexistent_binary_xyz");
        assert!(matches!(result, Err(BinaryError::NotAllowed(_))));
    }

    #[test]
    fn test_registry_not_found() {
        let result = BinaryRegistry::new(&["nonexistent_binary_xyz_12345".to_string()]);
        // May be NotFound or WhichFailed depending on `which` behavior
        assert!(result.is_err());
    }

    #[test]
    fn test_registry_rejects_path_separator() {
        let result = BinaryRegistry::new(&["./evil".into()]);
        assert!(matches!(result, Err(BinaryError::NotFound(_))));
    }

    #[test]
    fn test_registry_rejects_empty() {
        let result = BinaryRegistry::new(&["".into()]);
        assert!(matches!(result, Err(BinaryError::NotFound(_))));
    }

    #[test]
    fn test_is_allowed() {
        let reg = BinaryRegistry::new(&["echo".into(), "true".into()]).unwrap();
        assert!(reg.is_allowed("echo"));
        assert!(reg.is_allowed("true"));
        assert!(!reg.is_allowed("ls"));
    }

    #[test]
    fn test_registry_multiple() {
        let reg = BinaryRegistry::new(&["echo".into(), "true".into(), "false".into()]).unwrap();
        assert_eq!(reg.len(), 3);
        assert!(!reg.is_empty());
    }
}
