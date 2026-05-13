use crate::ConfigError;
pub use duga_plugin_abi::{PluginModuleConfig, WasiCapabilities};
use duga_sandbox::env::is_secret_pattern;
use duga_types::config::AgentConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Config {
    #[serde(default)]
    pub provider: Option<String>,
    pub model: String,
    pub agent: AgentConfig,
    pub sandbox: SandboxConfig,
    pub workspace: WorkspaceConfig,
    pub environment: EnvironmentConfig,
    pub memory: MemoryConfig,
    pub plugins: PluginConfig,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SandboxConfig {
    #[serde(with = "duration_format")]
    pub timeout: Duration,
    pub allowed_binaries: Vec<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct WorkspaceConfig {
    pub root: PathBuf,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EnvironmentConfig {
    pub allowed: HashSet<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MemoryConfig {
    pub max_tokens: usize,
    pub compress_at_ratio: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PluginConfig {
    pub dir: PathBuf,
    #[serde(default)]
    pub modules: Vec<PluginModuleConfig>,
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let raw =
            std::fs::read_to_string(path).map_err(|_| ConfigError::FileNotFound(path.into()))?;
        let mut config: Self =
            serde_yaml::from_str(&raw).map_err(|e| ConfigError::ParseError(e.to_string()))?;
        config.workspace.root = expand_tilde(&config.workspace.root)?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        let mut errors = Vec::new();

        if !self.workspace.root.exists() {
            errors.push(format!(
                "workspace root not found: {}",
                self.workspace.root.display()
            ));
        } else if !self.workspace.root.is_dir() {
            errors.push(format!(
                "workspace root is not a directory: {}",
                self.workspace.root.display()
            ));
        }

        for binary in &self.sandbox.allowed_binaries {
            if !binary.is_absolute() {
                errors.push(format!(
                    "binary path must be absolute: {}",
                    binary.display()
                ));
                continue;
            }
            if !binary.exists() {
                errors.push(format!("binary not found: {}", binary.display()));
                continue;
            }
            if !binary.is_file() {
                errors.push(format!("binary is not a file: {}", binary.display()));
                continue;
            }
            if !is_executable(binary) {
                errors.push(format!("binary is not executable: {}", binary.display()));
            }
        }

        if self.agent.limits.max_steps == 0 {
            errors.push("agent.limits.max_steps must be greater than 0".into());
        }
        if self.agent.limits.max_tool_calls == 0 {
            errors.push("agent.limits.max_tool_calls must be greater than 0".into());
        }
        if self.agent.limits.max_runtime.is_zero() {
            errors.push("agent.limits.max_runtime must be greater than 0".into());
        }
        if self.memory.max_tokens == 0 {
            errors.push("memory.max_tokens must be greater than 0".into());
        }
        if self
            .provider
            .as_deref()
            .map(str::trim)
            .is_some_and(str::is_empty)
        {
            errors.push("provider must not be empty when specified".into());
        }
        if !(0.0..=1.0).contains(&self.memory.compress_at_ratio)
            || self.memory.compress_at_ratio == 0.0
            || self.memory.compress_at_ratio.is_nan()
        {
            errors.push("memory.compress_at_ratio must be between 0.0 and 1.0".into());
        }

        for name in &self.environment.allowed {
            if is_secret_pattern(name) {
                tracing::warn!(
                    "Secret-pattern variable '{}' in environment allowlist; this variable will be available to subprocesses",
                    name
                );
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(ConfigError::ValidationError(errors.join("\n")))
        }
    }
}

fn expand_tilde(path: &Path) -> Result<PathBuf, ConfigError> {
    let raw = path.to_string_lossy();
    if cfg!(windows) || !raw.starts_with('~') {
        return Ok(path.to_path_buf());
    }
    if raw == "~" || raw.starts_with("~/") {
        let home = std::env::var("HOME").map_err(|_| {
            ConfigError::ValidationError("HOME must be set to expand '~' paths".into())
        })?;
        if raw == "~" {
            Ok(PathBuf::from(home))
        } else {
            Ok(PathBuf::from(home).join(&raw[2..]))
        }
    } else {
        Ok(path.to_path_buf())
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

mod duration_format {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!("{}s", duration.as_secs()))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        let raw = raw.trim();
        let parse = |value: &str| {
            value
                .parse::<u64>()
                .map_err(|_| serde::de::Error::custom(format!("invalid duration '{raw}'")))
        };
        let duration = if let Some(value) = raw.strip_suffix("ms") {
            Duration::from_millis(parse(value)?)
        } else if let Some(value) = raw.strip_suffix('s') {
            Duration::from_secs(parse(value)?)
        } else if let Some(value) = raw.strip_suffix('m') {
            Duration::from_secs(parse(value)? * 60)
        } else if let Some(value) = raw.strip_suffix('h') {
            Duration::from_secs(parse(value)? * 3600)
        } else {
            return Err(serde::de::Error::custom(
                "duration must use ms, s, m, or h suffix",
            ));
        };
        if duration.is_zero() {
            return Err(serde::de::Error::custom("duration must be positive"));
        }
        Ok(duration)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn executable_file(dir: &tempfile::TempDir) -> PathBuf {
        let path = dir.path().join("tool");
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
        }
        path
    }

    fn yaml(dir: &tempfile::TempDir, binary: &Path) -> String {
        format!(
            r#"
model: "dummy/test"
agent:
  limits:
    max_steps: 5
    max_tool_calls: 10
    max_runtime: 60s
    retry_on_error: 1
  features:
    streaming: false
  output:
    max_stdout_bytes: 1024
    max_stderr_bytes: 1024
    max_combined_bytes: 2048
  think:
    max_calls: 2
    max_tokens: 128
sandbox:
  timeout: 10s
  allowed_binaries:
    - {}
workspace:
  root: {}
environment:
  allowed:
    - HOME
memory:
  max_tokens: 4096
  compress_at_ratio: 0.8
plugins:
  dir: {}
  modules:
    - name: format-code.wasm
      capabilities:
        workspace_fs: true
"#,
            binary.display(),
            dir.path().display(),
            dir.path().join("plugins").display()
        )
    }

    #[test]
    fn load_parses_yaml_and_validates() {
        let dir = tempfile::tempdir().unwrap();
        let binary = executable_file(&dir);
        let path = dir.path().join("duga.yaml");
        std::fs::write(&path, yaml(&dir, &binary)).unwrap();

        let config = Config::load(&path).unwrap();

        assert_eq!(config.model, "dummy/test");
        assert_eq!(config.provider, None);
        assert_eq!(config.sandbox.timeout, Duration::from_secs(10));
        assert_eq!(config.plugins.modules[0].name, "format-code.wasm");
    }

    #[test]
    fn load_accepts_separate_provider_field() {
        let dir = tempfile::tempdir().unwrap();
        let binary = executable_file(&dir);
        let path = dir.path().join("duga.yaml");
        let mut raw = yaml(&dir, &binary);
        raw = raw.replacen(
            "model: \"dummy/test\"",
            "provider: \"ollama\"\nmodel: \"qwen\"",
            1,
        );
        std::fs::write(&path, raw).unwrap();

        let config = Config::load(&path).unwrap();

        assert_eq!(config.provider.as_deref(), Some("ollama"));
        assert_eq!(config.model, "qwen");
    }

    #[test]
    fn missing_file_is_specific_error() {
        let err = Config::load("/no/such/config.yaml").unwrap_err();
        assert!(matches!(err, ConfigError::FileNotFound(_)));
    }

    #[test]
    fn validate_collects_errors() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("missing");
        let mut config: Config = serde_yaml::from_str(&yaml(&dir, &binary)).unwrap();
        config.agent.limits.max_steps = 0;
        config.memory.max_tokens = 0;

        let err = config.validate().unwrap_err().to_string();

        assert!(err.contains("binary not found"));
        assert!(err.contains("max_steps"));
        assert!(err.contains("memory.max_tokens"));
    }

    #[test]
    fn tilde_expands_from_home() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", dir.path());
        let expanded = expand_tilde(Path::new("~/demo")).unwrap();
        assert_eq!(expanded, dir.path().join("demo"));
    }

    #[test]
    fn partial_yaml_is_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("duga.yaml");
        let mut file = std::fs::File::create(&path).unwrap();
        writeln!(file, "model: dummy/test").unwrap();

        let err = Config::load(&path).unwrap_err();

        assert!(matches!(err, ConfigError::ParseError(_)));
    }
}
