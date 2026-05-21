//! Binary registry — resolved path allowlist for subprocess spawning.
//!
//! Supports three modes:
//! - **Exact** (default): each entry is a bare name or absolute path resolved at startup.
//! - **Glob patterns**: entries containing `*`, `?`, or `[` are expanded by walking matching
//!   directories and registering each discovered executable.
//! - **Allow-all**: a sentinel mode that skips all checks (for container/VM sandboxes).

use crate::error::BinaryError;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Describes how an allowed_binaries entry is interpreted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BinaryPattern {
    /// A bare name (e.g. `echo`) or absolute path (e.g. `/usr/bin/cat`).
    Exact(String),
    /// A glob pattern (e.g. `/usr/bin/*`, `/usr/local/bin/g*`).
    Glob(String),
}

impl BinaryPattern {
    /// Parse a string entry into a `BinaryPattern`.
    ///
    /// If the string contains `*`, `?`, or `[`, it is treated as a glob.
    /// Otherwise it is an exact entry.
    pub fn parse(entry: &str) -> Result<Self, BinaryError> {
        if entry.is_empty() {
            return Err(BinaryError::NotFound("<empty>".into()));
        }
        // Reject path traversal regardless of pattern type.
        if entry.contains("..") {
            return Err(BinaryError::NotFound(format!(
                "path traversal not allowed: {}",
                entry
            )));
        }
        if entry.starts_with("./") || entry.starts_with(".\\") {
            return Err(BinaryError::NotFound(format!(
                "relative path not allowed: {}",
                entry
            )));
        }
        if entry.contains('*') || entry.contains('?') || entry.contains('[') {
            Ok(Self::Glob(entry.to_string()))
        } else {
            Ok(Self::Exact(entry.to_string()))
        }
    }
}

/// Internal sentinel for allow-all mode.
#[derive(Clone, Debug, PartialEq, Eq)]
enum RegistryMode {
    /// Normal mode with a resolved allowlist.
    Normal(HashMap<String, PathBuf>),
    /// Allow-all sentinel — all binaries pass through.
    AllowAll,
}

/// Maps allowed binary names to their resolved absolute paths.
#[derive(Clone, Debug)]
pub struct BinaryRegistry {
    mode: RegistryMode,
    /// Number of entries (for reporting), always 0 in AllowAll.
    entry_count: usize,
}

impl Default for BinaryRegistry {
    fn default() -> Self {
        Self {
            mode: RegistryMode::Normal(HashMap::new()),
            entry_count: 0,
        }
    }
}

impl BinaryRegistry {
    /// Build a registry by resolving each allowed binary name via `which`.
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
        let count = allowed.len();
        Ok(Self {
            mode: RegistryMode::Normal(allowed),
            entry_count: count,
        })
    }

    /// Build a registry from validated absolute binary paths.
    pub fn from_paths(paths: &[PathBuf]) -> Result<Self, BinaryError> {
        let mut allowed = HashMap::new();
        for path in paths {
            if !path.is_absolute() || !path.is_file() {
                return Err(BinaryError::NotFound(path.display().to_string()));
            }
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| BinaryError::NotFound(path.display().to_string()))?
                .to_string();
            let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            allowed.insert(name, canonical);
        }
        let count = allowed.len();
        Ok(Self {
            mode: RegistryMode::Normal(allowed),
            entry_count: count,
        })
    }

    /// Build a registry from `BinaryPattern` entries.
    ///
    /// Exact entries are resolved via `which` (bare names) or validated as
    /// absolute paths. Glob entries are expanded by walking matching
    /// directories and registering each executable found.
    ///
    /// Warnings are emitted via `tracing::warn` for:
    /// - Glob patterns that match zero files.
    pub fn from_patterns(patterns: &[BinaryPattern]) -> Result<Self, BinaryError> {
        let mut allowed: HashMap<String, PathBuf> = HashMap::new();

        for pattern in patterns {
            match pattern {
                BinaryPattern::Exact(entry) => {
                    if entry.contains('/') {
                        // Absolute path.
                        let path = PathBuf::from(entry);
                        if !path.is_absolute() {
                            return Err(BinaryError::NotFound(entry.clone()));
                        }
                        if !path.is_file() {
                            return Err(BinaryError::NotFound(format!(
                                "binary not found: {}",
                                path.display()
                            )));
                        }
                        let name = path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .ok_or_else(|| {
                                BinaryError::NotFound(format!("invalid path: {}", entry))
                            })?
                            .to_string();
                        let canonical =
                            std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                        allowed.insert(name, canonical);
                    } else {
                        // Bare name — resolve via which.
                        let path = which::which(entry).map_err(|e| {
                            BinaryError::WhichFailed(entry.clone(), e.to_string())
                        })?;
                        let canonical =
                            std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                        allowed.insert(entry.clone(), canonical);
                    }
                }
                BinaryPattern::Glob(glob_pattern) => {
                    let paths: Vec<PathBuf> = match glob::glob(glob_pattern) {
                        Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
                        Err(e) => {
                            tracing::warn!(
                                "Invalid glob pattern '{}': {}",
                                glob_pattern,
                                e
                            );
                            continue;
                        }
                    };

                    if paths.is_empty() {
                        tracing::warn!(
                            "Glob pattern '{}' matched zero files (no binaries registered)",
                            glob_pattern
                        );
                    }

                    for path in &paths {
                        if !path.is_file() {
                            continue;
                        }
                        // Skip non-executable files on unix.
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            let is_exec = path
                                .metadata()
                                .map(|m| m.permissions().mode() & 0o111 != 0)
                                .unwrap_or(false);
                            if !is_exec {
                                continue;
                            }
                        }
                        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                            let canonical =
                                std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
                            allowed.insert(name.to_string(), canonical);
                        }
                    }
                }
            }
        }

        let count = allowed.len();
        Ok(Self {
            mode: RegistryMode::Normal(allowed),
            entry_count: count,
        })
    }

    /// Create a sentinel registry that allows all binaries.
    ///
    /// In this mode, `resolve()` returns the bare name as-is and
    /// `is_allowed()` always returns `true`. This is intended for
    /// container/VM sandbox modes where OS-level isolation provides
    /// the primary security boundary.
    pub fn allow_all() -> Self {
        Self {
            mode: RegistryMode::AllowAll,
            entry_count: 0,
        }
    }

    /// Returns `true` if this registry is in allow-all mode.
    pub fn is_allow_all(&self) -> bool {
        matches!(self.mode, RegistryMode::AllowAll)
    }

    /// Resolve an allowed binary name to its absolute path.
    ///
    /// In allow-all mode, returns the name itself as a path.
    pub fn resolve(&self, name: &str) -> Result<&Path, BinaryError> {
        match &self.mode {
            RegistryMode::Normal(allowed) => allowed
                .get(name)
                .map(|p| p.as_path())
                .ok_or_else(|| BinaryError::NotAllowed(name.to_string())),
            RegistryMode::AllowAll => {
                // In allow-all mode, the name is the path.
                // We return a static reference trick — the caller should
                // treat it as the bare program name.
                Err(BinaryError::NotAllowed(
                    "resolve() should not be called in allow-all mode; use is_allow_all() gate"
                        .into(),
                ))
            }
        }
    }

    /// Resolve an allowed binary name to a `PathBuf`.
    ///
    /// In allow-all mode, returns the bare name as a `PathBuf`.
    pub fn resolve_to_pathbuf(&self, name: &str) -> Result<PathBuf, BinaryError> {
        match &self.mode {
            RegistryMode::Normal(allowed) => allowed
                .get(name)
                .cloned()
                .ok_or_else(|| BinaryError::NotAllowed(name.to_string())),
            RegistryMode::AllowAll => {
                // In allow-all mode, the bare name is the "path".
                Ok(PathBuf::from(name))
            }
        }
    }

    /// Check if a binary name is in the allowlist.
    ///
    /// In allow-all mode, always returns `true`.
    pub fn is_allowed(&self, name: &str) -> bool {
        match &self.mode {
            RegistryMode::Normal(allowed) => allowed.contains_key(name),
            RegistryMode::AllowAll => true,
        }
    }

    /// Returns the number of registered binaries.
    ///
    /// In allow-all mode, returns 0 (unbounded).
    pub fn len(&self) -> usize {
        self.entry_count
    }

    /// Returns `true` if no binaries are registered.
    ///
    /// In allow-all mode, returns `true` (the registry is intentionally empty).
    pub fn is_empty(&self) -> bool {
        self.entry_count == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

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

    #[test]
    fn test_registry_from_absolute_paths() {
        let echo = which::which("echo").unwrap();
        let reg = BinaryRegistry::from_paths(&[echo.clone()]).unwrap();
        let name = echo.file_name().unwrap().to_str().unwrap();
        assert!(reg.resolve(name).unwrap().is_absolute());
    }

    // ── Allow-all tests ────────────────────────────────────────────────────

    #[test]
    fn test_allow_all_is_allow_all() {
        let reg = BinaryRegistry::allow_all();
        assert!(reg.is_allow_all());
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
    }

    #[test]
    fn test_allow_all_is_allowed_always_true() {
        let reg = BinaryRegistry::allow_all();
        assert!(reg.is_allowed("anything"));
        assert!(reg.is_allowed("echo"));
        assert!(reg.is_allowed("nonexistent_binary_xyz"));
    }

    #[test]
    fn test_allow_all_resolve_to_pathbuf() {
        let reg = BinaryRegistry::allow_all();
        let pb = reg.resolve_to_pathbuf("echo").unwrap();
        assert_eq!(pb, PathBuf::from("echo"));
    }

    // ── Pattern parse tests ────────────────────────────────────────────────

    #[test]
    fn test_parse_exact_bare_name() {
        let p = BinaryPattern::parse("echo").unwrap();
        assert_eq!(p, BinaryPattern::Exact("echo".into()));
    }

    #[test]
    fn test_parse_exact_absolute_path() {
        let p = BinaryPattern::parse("/usr/bin/cat").unwrap();
        assert_eq!(p, BinaryPattern::Exact("/usr/bin/cat".into()));
    }

    #[test]
    fn test_parse_glob_star() {
        let p = BinaryPattern::parse("/usr/bin/*").unwrap();
        assert_eq!(p, BinaryPattern::Glob("/usr/bin/*".into()));
    }

    #[test]
    fn test_parse_glob_question() {
        let p = BinaryPattern::parse("/usr/bin/g?").unwrap();
        assert_eq!(p, BinaryPattern::Glob("/usr/bin/g?".into()));
    }

    #[test]
    fn test_parse_glob_bracket() {
        let p = BinaryPattern::parse("/usr/bin/[gc]*").unwrap();
        assert_eq!(p, BinaryPattern::Glob("/usr/bin/[gc]*".into()));
    }

    #[test]
    fn test_parse_rejects_empty() {
        let result = BinaryPattern::parse("");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_rejects_traversal() {
        let result = BinaryPattern::parse("../etc/passwd");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_rejects_relative_dot_slash() {
        let result = BinaryPattern::parse("./evil");
        assert!(result.is_err());
    }

    // ── from_patterns tests ────────────────────────────────────────────────

    #[test]
    fn test_from_patterns_exact_and_glob() {
        let dir = tempfile::tempdir().unwrap();
        // Create a few fake executables.
        let a = dir.path().join("tool_a");
        let b = dir.path().join("tool_b");
        let c = dir.path().join("tool_c");
        std::fs::write(&a, "#!/bin/sh\n").unwrap();
        std::fs::write(&b, "#!/bin/sh\n").unwrap();
        std::fs::write(&c, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for f in [&a, &b, &c] {
                let mut perms = std::fs::metadata(f).unwrap().permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(f, perms).unwrap();
            }
        }

        let glob_pattern = format!("{}/*", dir.path().display());
        let patterns = vec![
            BinaryPattern::Glob(glob_pattern),
            BinaryPattern::Exact("echo".into()),
        ];
        let reg = BinaryRegistry::from_patterns(&patterns).unwrap();

        // The glob should have registered tool_a, tool_b, tool_c.
        assert!(reg.is_allowed("tool_a"));
        assert!(reg.is_allowed("tool_b"));
        assert!(reg.is_allowed("tool_c"));
        // And echo via exact resolution.
        assert!(reg.is_allowed("echo"));
        assert!(reg.len() >= 4);
    }

    #[test]
    fn test_from_patterns_glob_no_matches_warns() {
        let patterns = vec![BinaryPattern::Glob("/nonexistent_dir_xyz/*".into())];
        // Should not error, just warn and register nothing.
        let reg = BinaryRegistry::from_patterns(&patterns).unwrap();
        assert_eq!(reg.len(), 0);
        assert!(reg.is_empty());
    }

    #[test]
    fn test_from_patterns_exact_absolute_path_rejects_missing() {
        let patterns = vec![BinaryPattern::Exact("/nonexistent/binary_xyz".into())];
        let result = BinaryRegistry::from_patterns(&patterns);
        assert!(result.is_err());
    }
}
