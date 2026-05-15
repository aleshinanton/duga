//! Command executor abstraction for duga sandbox.
//!
//! Defines `CommandExecutor` — a trait for running subprocesses with a uniform
//! interface. Two implementations exist:
//!
//! - `CapabilityExecutor` — the default, using capability-bounded host execution
//!   via `run_captured()` (TASK-5). Filesystem tools (read/write/search) remain
//!   capability-bounded regardless of executor mode.
//! - `DockerExecutor` — routes commands through `docker exec` inside a
//!   pre-existing container, while read/write/search still operate on the
//!   host workspace via bind-mount paths.
//!
//! The executor is chosen at runtime based on `SandboxConfig.mode`.

use duga_types::config::OutputLimits;
use duga_types::error::ToolError;
use duga_types::tool_call::CallId;
use duga_types::tool_result::ToolResult;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::exec::{self, format_output, truncation_notice, CancellationToken};
use crate::workspace::Workspace;

/// Sandbox execution mode — mirrors duga_config::SandboxMode to avoid
/// a circular dependency. Convert at the runtime/composition layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SandboxMode {
    Capability,
    Host,
    Docker,
}

impl SandboxMode {
    /// Convert from the config-level string representation.
    pub fn from_config_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "docker" => Self::Docker,
            "host" => Self::Host,
            _ => Self::Capability,
        }
    }
}

/// A command specification describing what to execute.
#[derive(Clone, Debug)]
pub struct CommandSpec {
    /// The binary to execute (resolved absolute path or bare name).
    pub program: String,
    /// Command-line arguments.
    pub args: Vec<String>,
    /// Working directory (workspace-relative).
    pub cwd: PathBuf,
    /// Environment variables.
    pub env: std::collections::HashMap<String, String>,
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn shell_command(container_cwd: &Path, program: &str, args: &[String]) -> String {
    let mut command_parts = vec![shell_quote(program)];
    for arg in args {
        command_parts.push(shell_quote(arg));
    }
    format!(
        "cd {} && {}",
        shell_quote(&container_cwd.display().to_string()),
        command_parts.join(" ")
    )
}

/// Trait object-safe executor interface.
///
/// Both `CapabilityExecutor` and `DockerExecutor` implement this so
/// that the sandbox mode can be selected at runtime from config.
#[async_trait::async_trait]
pub trait CommandExecutor: Send + Sync {
    /// Run a subprocess with the given limits and cancellation token.
    ///
    /// Returns `ToolResult` on success, `ToolError` on failure/timeout/cancel.
    async fn run(
        &self,
        spec: &CommandSpec,
        workspace: &Workspace,
        limits: &OutputLimits,
        timeout: std::time::Duration,
        cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError>;
}

/// Default executor using direct (capability-bounded) process spawning.
///
/// This is the same path as existing `run_captured()` — no Docker involved.
/// Used when `sandbox.mode == "capability"` or `"host"`.
#[derive(Debug, Clone, Default)]
pub struct CapabilityExecutor;

#[async_trait::async_trait]
impl CommandExecutor for CapabilityExecutor {
    async fn run(
        &self,
        spec: &CommandSpec,
        workspace: &Workspace,
        limits: &OutputLimits,
        timeout: std::time::Duration,
        cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        exec::run_captured(
            Path::new(&spec.program),
            &spec.args,
            workspace,
            &spec.cwd,
            &spec.env,
            limits,
            timeout,
            cancel,
        )
        .await
    }
}

/// Docker-based executor that routes commands into a running container.
///
/// The workspace directory is bind-mounted into the container at the configured
/// mount point. File read/write/search operations continue to use host-side
/// capability-bounded access (this executor only affects process spawning).
///
/// # Invariants
/// - The container must already exist and be running at construction time.
/// - `workspace_mount` is the container-side path where the host workspace
///   is bind-mounted (e.g. `/workspace`).
/// - Path translation: host workspace-relative path → container absolute path.
///
/// # Errors
/// - `ToolError::Denied` if the container is not running or not found.
/// - All other errors delegate to the Docker exec output.
#[derive(Debug, Clone)]
pub struct DockerExecutor {
    container: Arc<str>,
    workspace_mount: Arc<Path>,
    /// Resolved host binary paths (to avoid PATH resolution inside container).
    #[allow(dead_code)]
    binary_registry: Arc<Mutex<Vec<(String, PathBuf)>>>,
}

impl DockerExecutor {
    /// Create a new DockerExecutor, validating that the container exists and is
    /// running (if `validate` is true).
    pub fn new(container: &str, workspace_mount: &Path) -> Result<Self, ToolError> {
        Self::with_validation(container, workspace_mount, true)
    }

    /// Create without validation (for tests).
    #[cfg(test)]
    pub fn new_unchecked(container: &str, workspace_mount: &Path) -> Self {
        Self {
            container: container.into(),
            workspace_mount: workspace_mount.to_path_buf().into(),
            binary_registry: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Create with optional validation.
    pub fn with_validation(
        container: &str,
        workspace_mount: &Path,
        validate: bool,
    ) -> Result<Self, ToolError> {
        if container.is_empty() {
            return Err(ToolError::Denied(
                "Docker container name must not be empty".into(),
            ));
        }
        if workspace_mount.as_os_str().is_empty() {
            return Err(ToolError::Denied(
                "Docker workspace mount must not be empty".into(),
            ));
        }
        let executor = Self {
            container: container.into(),
            workspace_mount: workspace_mount.to_path_buf().into(),
            binary_registry: Arc::new(Mutex::new(Vec::new())),
        };
        if validate {
            // Synchronous validation via a blocking check.
            let status = std::process::Command::new("docker")
                .args(["inspect", "--format='{{.State.Running}}'", container])
                .output();
            match status {
                Ok(output) if output.status.success() => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    if !stdout.trim().contains("true") && !stdout.trim().contains("'true'") {
                        return Err(ToolError::Denied(format!(
                            "Docker container '{}' is not running",
                            container
                        )));
                    }
                }
                Ok(_) => {
                    return Err(ToolError::Denied(format!(
                        "Docker container '{}' not found",
                        container
                    )))
                }
                Err(e) => {
                    return Err(ToolError::Denied(format!(
                        "Failed to validate Docker container '{}': {}",
                        container, e
                    )))
                }
            }
        }
        Ok(executor)
    }

    /// Translate a workspace-relative path to its container-side absolute path.
    pub fn translate_to_container(&self, relative: &Path) -> PathBuf {
        self.workspace_mount.join(relative)
    }

    /// Translate a container-side path back to a workspace-relative path.
    pub fn translate_to_host(&self, container_path: &Path) -> Option<PathBuf> {
        container_path
            .strip_prefix(&*self.workspace_mount)
            .map(PathBuf::from)
            .ok()
    }

    /// Resolve a binary name using the host's PATH (cached for performance).
    #[allow(dead_code)]
    async fn resolve_binary(&self, name: &str) -> Result<PathBuf, ToolError> {
        // Check cache first.
        {
            let registry = self.binary_registry.lock().await;
            if let Some(path) = registry.iter().find(|(n, _)| n == name) {
                return Ok(path.1.clone());
            }
        }

        // Resolve on host via `which`.
        let path = which::which(name)
            .map_err(|_| ToolError::Denied(format!("binary '{}' not found on host PATH", name)))?;

        // Cache the resolved path.
        let mut registry = self.binary_registry.lock().await;
        registry.push((name.to_string(), path.clone()));
        Ok(path)
    }
}

#[async_trait::async_trait]
impl CommandExecutor for DockerExecutor {
    async fn run(
        &self,
        spec: &CommandSpec,
        workspace: &Workspace,
        limits: &OutputLimits,
        timeout: std::time::Duration,
        cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        if let Some(key) = spec
            .env
            .keys()
            .find(|key| crate::env::is_protected_var(key))
        {
            return Err(ToolError::Denied(format!(
                "protected environment variable cannot be set: {}",
                key
            )));
        }

        // Convert workspace-relative cwd to container-absolute path.
        let resolved_cwd = workspace
            .resolve(&spec.cwd)
            .map_err(|e| ToolError::Denied(e.to_string()))?;
        let container_cwd = self.translate_to_container(&resolved_cwd);

        // Build the docker exec command args.
        // docker exec [options] container command
        let mut args: Vec<String> = Vec::new();

        // Environment variables passed via -e flags.
        for (key, value) in &spec.env {
            args.push("-e".into());
            args.push(format!("{}={}", key, value));
        }

        // Working directory inside the container.
        args.push("--workdir".into());
        args.push(container_cwd.display().to_string());

        // Container name/image.
        args.push(self.container.to_string());

        // The actual command to run inside the container via sh -c.
        args.push("sh".into());
        args.push("-c".into());

        let cmd_string = shell_command(&container_cwd, &spec.program, &spec.args);
        args.push(cmd_string);

        // Locate the docker binary on the host.
        let docker_binary = which::which("docker")
            .map_err(|e| ToolError::Io(format!("docker binary not found: {}", e)))?;

        // Preserve host PATH for docker exec to resolve binaries inside container.
        let mut env = spec.env.clone();
        if let Ok(host_path) = std::env::var("PATH") {
            env.entry("PATH".into()).or_insert(host_path);
        }

        // Spawn docker exec process.
        let mut child = tokio::process::Command::new(&docker_binary)
            .args(&args)
            .env_clear()
            .envs(&env)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| ToolError::Io(format!("docker exec spawn failed: {}", e)))?;

        // Drain output with limits, same logic as run_captured.
        let stdout_fd = child.stdout.take().expect("stdout piped");
        let stderr_fd = child.stderr.take().expect("stderr piped");

        async fn read_limited<R: tokio::io::AsyncReadExt + Unpin + Send + 'static>(
            mut reader: R,
            limit: u64,
        ) -> Result<(u64, Vec<u8>, bool), ToolError> {
            let mut buf = Vec::new();
            let mut scratch = vec![0u8; 8192];
            let mut total: u64 = 0;
            let mut truncated = false;

            loop {
                match reader.read(&mut scratch).await {
                    Ok(0) => break,
                    Ok(n) => {
                        total = total.saturating_add(n as u64);
                        let remaining = limit.saturating_sub(buf.len() as u64) as usize;
                        if remaining == 0 {
                            truncated = true;
                            continue;
                        }
                        let take = n.min(remaining);
                        buf.extend_from_slice(&scratch[..take]);
                        if take < n {
                            truncated = true;
                        }
                    }
                    Err(e) => return Err(ToolError::Io(e.to_string())),
                }
            }

            Ok((total, buf, truncated))
        }

        let stdout_task = tokio::spawn(read_limited(
            tokio::io::BufReader::new(stdout_fd),
            limits.max_stdout_bytes,
        ));
        let stderr_task = tokio::spawn(read_limited(
            tokio::io::BufReader::new(stderr_fd),
            limits.max_stderr_bytes,
        ));

        let mut cancel_rx = cancel.subscribe();

        let exit_status = tokio::select! {
            biased;

            result = tokio::time::timeout(timeout, child.wait()) => {
                match result {
                    Ok(Ok(status)) => status,
                    Ok(Err(e)) => {
                        stdout_task.abort();
                        stderr_task.abort();
                        return Err(ToolError::Io(e.to_string()));
                    }
                    Err(_) => {
                        let _ = child.kill().await;
                        stdout_task.abort();
                        stderr_task.abort();
                        return Err(ToolError::Timeout);
                    }
                }
            }

            _ = cancel_rx.changed() => {
                let _ = child.kill().await;
                stdout_task.abort();
                stderr_task.abort();
                return Err(ToolError::Cancelled);
            }
        };

        let (stdout_bytes, mut stdout_buf, stdout_trunc) = stdout_task
            .await
            .map_err(|e| ToolError::Io(format!("stdout reader failed: {}", e)))??;
        let (stderr_bytes, mut stderr_buf, stderr_trunc) = stderr_task
            .await
            .map_err(|e| ToolError::Io(format!("stderr reader failed: {}", e)))??;

        // Combined limit check
        let combined_bytes = stdout_bytes.saturating_add(stderr_bytes);
        let combined_trunc = combined_bytes > limits.max_combined_bytes;
        if combined_trunc {
            let max = limits.max_combined_bytes as usize;
            if stdout_buf.len() >= max {
                stdout_buf.truncate(max);
                stderr_buf.clear();
            } else {
                stderr_buf.truncate(max - stdout_buf.len());
            }
        }
        let truncated = stdout_trunc || stderr_trunc || combined_trunc;

        let output = format_output(
            &String::from_utf8_lossy(&stdout_buf),
            &String::from_utf8_lossy(&stderr_buf),
            exit_status.code(),
        );

        let output = if truncated {
            let mut o = output;
            if stdout_trunc {
                o.push_str("\n[TRUNCATED] ");
                o.push_str(&truncation_notice(
                    "stdout",
                    limits.max_stdout_bytes as usize,
                ));
            }
            if stderr_trunc {
                o.push_str("\n[TRUNCATED] ");
                o.push_str(&truncation_notice(
                    "stderr",
                    limits.max_stderr_bytes as usize,
                ));
            }
            if combined_trunc {
                o.push_str("\n[TRUNCATED] ");
                o.push_str(&truncation_notice(
                    "combined output",
                    limits.max_combined_bytes as usize,
                ));
            }
            o
        } else {
            output
        };

        let exit_code = exit_status.code();
        let metadata = match exit_code {
            Some(code) => serde_json::json!({ "exit_code": code }),
            None => serde_json::json!({ "exit_code": null, "signal": "killed" }),
        };

        Ok(ToolResult {
            tool_call_id: CallId::new(),
            success: true,
            output,
            metadata,
            duration_ms: 0,
            stdout_bytes,
            stderr_bytes,
            truncated,
        })
    }
}

/// Select the appropriate executor based on configured sandbox mode.
#[derive(Debug, Clone)]
pub enum SandboxExecutor {
    /// Direct capability-bounded execution (default).
    Capability(CapabilityExecutor),
    /// Containerized execution via Docker.
    Docker(DockerExecutor),
}

impl SandboxExecutor {
    pub fn capability() -> Self {
        Self::Capability(CapabilityExecutor)
    }

    pub fn docker(container: &str, workspace_mount: &Path) -> Result<Self, ToolError> {
        Ok(Self::Docker(DockerExecutor::new(
            container,
            workspace_mount,
        )?))
    }

    /// Build from config values, returning the appropriate executor variant.
    pub fn from_config(
        mode: &SandboxMode,
        container: Option<&str>,
        workspace_mount: Option<&str>,
    ) -> Result<Self, ToolError> {
        match mode {
            SandboxMode::Capability | SandboxMode::Host => Ok(Self::capability()),
            SandboxMode::Docker => {
                let c = container.ok_or_else(|| {
                    ToolError::Denied(
                        "sandbox.mode is 'docker' but no container name configured".into(),
                    )
                })?;
                let mount = workspace_mount.unwrap_or("/workspace");
                Self::docker(c, Path::new(mount))
            }
        }
    }
}

#[async_trait::async_trait]
impl CommandExecutor for SandboxExecutor {
    async fn run(
        &self,
        spec: &CommandSpec,
        workspace: &Workspace,
        limits: &OutputLimits,
        timeout: std::time::Duration,
        cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        match self {
            Self::Capability(exec) => exec.run(spec, workspace, limits, timeout, cancel).await,
            Self::Docker(exec) => exec.run(spec, workspace, limits, timeout, cancel).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Workspace;
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_sandbox_mode_from_config_str() {
        assert_eq!(
            SandboxMode::from_config_str("capability"),
            SandboxMode::Capability
        );
        assert_eq!(SandboxMode::from_config_str("host"), SandboxMode::Host);
        assert_eq!(SandboxMode::from_config_str("docker"), SandboxMode::Docker);
        assert_eq!(
            SandboxMode::from_config_str("unknown"),
            SandboxMode::Capability
        );
    }

    #[test]
    fn test_sandbox_executor_capability_default() {
        let exec = SandboxExecutor::capability();
        match exec {
            SandboxExecutor::Capability(_) => {}
            other => panic!("expected Capability, got {:?}", other),
        }
    }

    #[test]
    fn test_sandbox_executor_docker_creation_fails_without_container() {
        let result = SandboxExecutor::docker("", Path::new("/workspace"));
        assert!(result.is_err());
    }

    #[test]
    fn test_docker_executor_path_translation() {
        let exec = DockerExecutor::new_unchecked("test-container", Path::new("/workspace"));
        assert_eq!(
            exec.translate_to_container(Path::new("src/main.rs")),
            PathBuf::from("/workspace/src/main.rs")
        );
        assert_eq!(
            exec.translate_to_host(Path::new("/workspace/src/main.rs")),
            Some(PathBuf::from("src/main.rs"))
        );
        assert!(exec.translate_to_host(Path::new("/etc/passwd")).is_none());
    }

    #[test]
    fn test_docker_shell_command_quotes_args_without_chaining_each_arg() {
        let command = shell_command(
            Path::new("/workspace/src dir"),
            "/bin/echo",
            &["hello world".into(), "it's ok".into()],
        );

        assert_eq!(
            command,
            "cd '/workspace/src dir' && '/bin/echo' 'hello world' 'it'\\''s ok'"
        );
    }

    #[test]
    fn test_docker_executor_rejects_empty_mount() {
        let result = DockerExecutor::new("test", Path::new(""));
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_capability_executor_runs_echo() {
        let exec = CapabilityExecutor::default();
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let spec = CommandSpec {
            program: "echo".into(),
            args: vec!["hello".into()],
            cwd: PathBuf::from("."),
            env: Default::default(),
        };
        let cancel = CancellationToken::new();
        let limits = OutputLimits {
            max_stdout_bytes: 4 * 1024 * 1024,
            max_stderr_bytes: 4 * 1024 * 1024,
            max_combined_bytes: 8 * 1024 * 1024,
        };
        let result = exec
            .run(&spec, &ws, &limits, Duration::from_secs(10), cancel)
            .await;
        assert!(result.is_ok(), "echo should succeed: {:?}", result);
        let r = result.unwrap();
        assert!(
            r.output.contains("hello"),
            "output should contain 'hello': {}",
            r.output
        );
    }

    #[tokio::test]
    async fn test_capability_executor_timeout() {
        let exec = CapabilityExecutor::default();
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let spec = CommandSpec {
            program: "sleep".into(),
            args: vec!["10".into()],
            cwd: PathBuf::from("."),
            env: Default::default(),
        };
        let cancel = CancellationToken::new();
        let limits = OutputLimits {
            max_stdout_bytes: 4 * 1024 * 1024,
            max_stderr_bytes: 4 * 1024 * 1024,
            max_combined_bytes: 8 * 1024 * 1024,
        };
        let result = exec
            .run(&spec, &ws, &limits, Duration::from_millis(50), cancel)
            .await;
        assert!(matches!(result, Err(ToolError::Timeout)));
    }

    #[tokio::test]
    async fn test_capability_executor_cancellation() {
        let exec = CapabilityExecutor::default();
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let spec = CommandSpec {
            program: "sleep".into(),
            args: vec!["10".into()],
            cwd: PathBuf::from("."),
            env: Default::default(),
        };
        let cancel = CancellationToken::new();
        let limits = OutputLimits {
            max_stdout_bytes: 4 * 1024 * 1024,
            max_stderr_bytes: 4 * 1024 * 1024,
            max_combined_bytes: 8 * 1024 * 1024,
        };

        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel_clone.cancel();
        });

        let result = exec
            .run(&spec, &ws, &limits, Duration::from_secs(10), cancel)
            .await;
        assert!(matches!(result, Err(ToolError::Cancelled)));
    }

    #[test]
    fn test_from_config_capability_mode() {
        let mode = SandboxMode::Capability;
        let result = SandboxExecutor::from_config(&mode, None, None);
        assert!(matches!(result, Ok(SandboxExecutor::Capability(_))));
    }

    #[test]
    fn test_from_config_host_mode() {
        let mode = SandboxMode::Host;
        let result = SandboxExecutor::from_config(&mode, None, None);
        assert!(matches!(result, Ok(SandboxExecutor::Capability(_))));
    }

    #[test]
    fn test_from_config_docker_missing_container() {
        let mode = SandboxMode::Docker;
        let result = SandboxExecutor::from_config(&mode, None, None);
        assert!(result.is_err());
        assert!(format!("{:?}", result).contains("docker"));
    }
}
