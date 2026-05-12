//! Sanitized environment builder.
//!
//! `SanitizedEnv` builds a filtered subprocess environment from an allowlist.
//! The host `PATH` is replaced with a fixed safe default.
//! Secret-pattern variables (e.g. `*_TOKEN`, `*_SECRET`) that slip through
//! the allowlist are flagged via warnings.

use std::collections::HashMap;
use std::env;

/// Patterns that indicate a variable likely contains a secret.
const SECRET_PATTERNS: &[&str] = &["_TOKEN", "_KEY", "_SECRET", "_PASSWORD"];

/// The fixed `PATH` value assigned to all subprocesses.
pub const SANITIZED_PATH: &str = "/usr/bin:/bin";

/// Checks whether a variable name matches a known secret pattern (case-insensitive).
pub fn is_secret_pattern(name: &str) -> bool {
    let upper = name.to_uppercase();
    SECRET_PATTERNS.iter().any(|pat| upper.contains(*pat))
}

/// Build a sanitized environment HashMap from an allowlist of variable names.
///
/// - Only variables whose names appear in `allowed` are included.
/// - `PATH` is always overridden with `/usr/bin:/bin` (never inherited).
/// - Variables matching secret patterns are added but a warning string is pushed
///   into `warnings` so the caller can log or emit them.
///
/// Returns the environment map ready for `Command::envs()`.
pub fn build(allowed: &std::collections::HashSet<String>, warnings: &mut Vec<String>) -> HashMap<String, String> {
    let mut env = HashMap::new();

    for (key, value) in env::vars() {
        if !allowed.contains(&key) {
            continue;
        }
        if is_secret_pattern(&key) {
            warnings.push(format!("Secret-pattern env var passed to subprocess: {}", key));
        }
        env.insert(key, value);
    }

    // Always override PATH
    env.insert("PATH".to_string(), SANITIZED_PATH.to_string());

    env
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_is_secret_pattern() {
        assert!(is_secret_pattern("GITHUB_TOKEN"));
        assert!(is_secret_pattern("AWS_SECRET_ACCESS_KEY"));
        assert!(is_secret_pattern("API_KEY"));
        assert!(is_secret_pattern("DB_PASSWORD"));
        assert!(!is_secret_pattern("HOME"));
        assert!(!is_secret_pattern("PATH"));
        assert!(!is_secret_pattern("USER"));
    }

    #[test]
    fn test_build_filters_to_allowed() {
        let mut warnings = Vec::new();
        let allowed: HashSet<String> = ["HOME".into(), "USER".into()].iter().cloned().collect();
        let env = build(&allowed, &mut warnings);

        // HOME and USER should be present (if set in host env)
        // PATH should be overridden
        assert!(env.contains_key("PATH"));
        assert_eq!(env["PATH"], SANITIZED_PATH);
        // Non-allowed vars should not be present
        assert!(!env.contains_key("GITHUB_TOKEN"));
    }

    #[test]
    fn test_build_secret_warning() {
        let mut warnings = Vec::new();
        let allowed: HashSet<String> = ["GITHUB_TOKEN".into()].iter().cloned().collect();
        // Ensure the var exists in the environment for the test
        std::env::set_var("GITHUB_TOKEN", "ghp_test123");
        let _env = build(&allowed, &mut warnings);

        assert!(!warnings.is_empty());
        assert!(warnings[0].contains("GITHUB_TOKEN"));

        // Clean up
        std::env::remove_var("GITHUB_TOKEN");
    }

    #[test]
    fn test_build_path_always_overridden() {
        let mut warnings = Vec::new();
        let allowed: HashSet<String> = ["PATH".into()].iter().cloned().collect();
        let env = build(&allowed, &mut warnings);
        assert_eq!(env["PATH"], SANITIZED_PATH);
    }
}