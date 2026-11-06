//! Explicit host execution, filesystem-only capabilities, and Docker execution.
use crate::{
    Workspace,
    exec::{self, CancellationToken},
};
use duga_types::{config::OutputLimits, error::ToolError, tool_result::ToolResult};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SandboxMode {
    Capability,
    Host,
    Docker,
}
impl SandboxMode {
    pub fn from_config_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "host" => Self::Host,
            "docker" => Self::Docker,
            _ => Self::Capability,
        }
    }
}
#[derive(Clone, Debug)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: std::collections::HashMap<String, String>,
}
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn shell_command(cwd: &Path, program: &str, args: &[String]) -> String {
    let mut parts = vec![shell_quote(program)];
    parts.extend(args.iter().map(|a| shell_quote(a)));
    format!(
        "cd {} && {}",
        shell_quote(&cwd.display().to_string()),
        parts.join(" ")
    )
}
#[async_trait::async_trait]
pub trait CommandExecutor: Send + Sync {
    async fn run(
        &self,
        spec: &CommandSpec,
        workspace: &Workspace,
        limits: &OutputLimits,
        timeout: Duration,
        cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError>;
    fn is_container_executor(&self) -> bool {
        false
    }
}

/// This mode bounds filesystem tools only. No host processes are permitted.
#[derive(Debug, Clone, Default)]
pub struct CapabilityExecutor;
#[async_trait::async_trait]
impl CommandExecutor for CapabilityExecutor {
    async fn run(
        &self,
        _: &CommandSpec,
        _: &Workspace,
        _: &OutputLimits,
        _: Duration,
        _: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        Err(ToolError::Denied("subprocesses are disabled in capability mode: use Docker for isolation or explicitly select trusted host mode".into()))
    }
}
/// Explicit opt-in to unrestricted host processes; not a security sandbox.
#[derive(Debug, Clone, Default)]
pub struct HostExecutor;
#[async_trait::async_trait]
impl CommandExecutor for HostExecutor {
    async fn run(
        &self,
        spec: &CommandSpec,
        workspace: &Workspace,
        limits: &OutputLimits,
        timeout: Duration,
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

#[derive(Debug, Clone)]
pub struct DockerExecutor {
    container: String,
    workspace_mount: PathBuf,
}
impl DockerExecutor {
    pub fn new(container: &str, workspace_mount: &Path) -> Result<Self, ToolError> {
        Self::with_validation(container, workspace_mount, true)
    }
    pub fn with_validation(
        container: &str,
        workspace_mount: &Path,
        validate: bool,
    ) -> Result<Self, ToolError> {
        if container.is_empty()
            || container.starts_with('-')
            || !workspace_mount.to_string_lossy().starts_with('/')
        {
            return Err(ToolError::Denied(
                "Docker needs a container name and an absolute workspace mount".into(),
            ));
        }
        if validate {
            let output = std::process::Command::new("docker")
                .args(["inspect", "--format={{.State.Running}}", container])
                .output()
                .map_err(|e| ToolError::Denied(format!("Docker validation failed: {e}")))?;
            if !output.status.success() || String::from_utf8_lossy(&output.stdout).trim() != "true"
            {
                return Err(ToolError::Denied(format!(
                    "Docker container '{container}' is not running"
                )));
            }
        }
        Ok(Self {
            container: container.into(),
            workspace_mount: workspace_mount.into(),
        })
    }
    #[cfg(test)]
    pub fn new_unchecked(container: &str, mount: &Path) -> Self {
        Self::with_validation(container, mount, false).unwrap()
    }
    pub fn translate_to_container(&self, relative: &Path) -> PathBuf {
        self.workspace_mount.join(relative)
    }
    pub fn translate_to_host(&self, path: &Path) -> Option<PathBuf> {
        path.strip_prefix(&self.workspace_mount)
            .ok()
            .map(PathBuf::from)
    }
}

/// Remote lifetime guard. Killing docker.exe alone does NOT stop docker exec.
/// Abort stops the dedicated container, including when cancellation races startup.
struct DockerRunGuard {
    binary: PathBuf,
    container: String,
    marker: String,
    armed: bool,
}
impl DockerRunGuard {
    /// On cancellation terminate the container, not just a process group:
    /// commands may have daemonized or created a new session. Containers must
    /// be dedicated to this agent/chat; restarting after cancellation is explicit.
    fn abort_command(&self) -> std::process::Command {
        let mut command = std::process::Command::new(&self.binary);
        command.args(["stop", "--time", "0", &self.container]);
        command
    }

    fn cleanup_command(&self) -> std::process::Command {
        let marker = shell_quote(&self.marker);
        let script = format!(
            "mkdir -p {marker} && touch {marker}/cancel; if test -f {marker}/pid; then p=$(cat {marker}/pid); case $p in ''|*[!0-9]*) exit 1;; esac; kill -KILL -\"$p\" 2>/dev/null || true; fi; rm -rf {marker}"
        );
        let mut cmd = std::process::Command::new(&self.binary);
        cmd.args(["exec", &self.container, "sh", "-c", &script]);
        cmd
    }
    async fn finish(&mut self, abort: bool) -> Result<(), ToolError> {
        let mut command = tokio::process::Command::from(if abort {
            self.abort_command()
        } else {
            self.cleanup_command()
        });
        command.kill_on_drop(true);
        let status = tokio::time::timeout(Duration::from_secs(5), command.status())
            .await
            .map_err(|_| ToolError::Io("Docker process cleanup timed out".into()))?
            .map_err(|e| ToolError::Io(format!("Docker process cleanup failed: {e}")))?;
        if !status.success() {
            return Err(ToolError::Io("Docker process cleanup failed".into()));
        }
        self.armed = false;
        Ok(())
    }
}
impl Drop for DockerRunGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let mut command = self.abort_command();
        // Drop may happen after the async runtime shuts down. Use a bounded OS worker.
        std::thread::spawn(move || {
            command
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            if let Ok(mut child) = command.spawn() {
                let started = std::time::Instant::now();
                loop {
                    match child.try_wait() {
                        Ok(Some(_)) | Err(_) => break,
                        _ => {}
                    }
                    if started.elapsed() >= Duration::from_secs(5) {
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        });
    }
}
#[async_trait::async_trait]
impl CommandExecutor for DockerExecutor {
    fn is_container_executor(&self) -> bool {
        true
    }
    async fn run(
        &self,
        spec: &CommandSpec,
        workspace: &Workspace,
        limits: &OutputLimits,
        timeout: Duration,
        cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        if cancel.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        if let Some(key) = spec
            .env
            .keys()
            .find(|key| crate::env::is_protected_var(key))
        {
            return Err(ToolError::Denied(format!(
                "protected environment variable cannot be set: {key}"
            )));
        }
        let cwd = workspace
            .resolve(&spec.cwd)
            .map_err(|e| ToolError::Denied(e.to_string()))?;
        let cwd = self.translate_to_container(&cwd);
        // Container paths are POSIX paths even when the client runs on Windows.
        let cwd = cwd.to_string_lossy().replace('\\', "/");
        let binary = which::which("docker").map_err(|e| ToolError::Io(e.to_string()))?;
        let mut guard = DockerRunGuard {
            binary: binary.clone(),
            container: self.container.clone(),
            marker: format!("/tmp/.duga-exec-{}", uuid::Uuid::new_v4()),
            armed: true,
        };
        let marker = shell_quote(&guard.marker);
        let body = shell_command(Path::new(&cwd), &spec.program, &spec.args);
        // setsid -w is required: wait even if setsid must fork because docker exec
        // started it as a process-group leader. Never report success before the command exits.
        let inner =
            format!("echo $$ > {marker}/pid; test ! -e {marker}/cancel || exit 125; {body}");
        let wrapper = format!(
            "umask 077; mkdir -p {marker} || exit 125; command -v setsid >/dev/null || exit 125; exec setsid -w sh -c {}",
            shell_quote(&inner)
        );
        let mut command = tokio::process::Command::new(binary);
        command.arg("exec");
        for (key, value) in &spec.env {
            command.arg("-e").arg(format!("{key}={value}"));
        }
        command
            .arg("-w")
            .arg(&cwd)
            .arg(&self.container)
            .args(["sh", "-c", &wrapper]);
        // docker client needs the host's Docker context/SystemRoot, not the agent's env.
        let result = exec::run_process(command, limits, timeout, cancel).await;
        let abort = matches!(result, Err(ToolError::Cancelled | ToolError::Timeout));
        guard.finish(abort).await?;
        result.map(|mut result| {
            result.success = result.metadata.get("exit_code").and_then(|v| v.as_i64()) == Some(0);
            result
        })
    }
}

#[derive(Debug, Clone)]
pub enum SandboxExecutor {
    Capability(CapabilityExecutor),
    Host(HostExecutor),
    Docker(DockerExecutor),
}
impl SandboxExecutor {
    pub fn capability() -> Self {
        Self::Capability(CapabilityExecutor)
    }
    pub fn host() -> Self {
        Self::Host(HostExecutor)
    }
    pub fn docker(container: &str, mount: &Path) -> Result<Self, ToolError> {
        Ok(Self::Docker(DockerExecutor::new(container, mount)?))
    }
    pub fn from_config(
        mode: &SandboxMode,
        container: Option<&str>,
        mount: Option<&str>,
    ) -> Result<Self, ToolError> {
        match mode {
            SandboxMode::Capability => Ok(Self::capability()),
            SandboxMode::Host => Ok(Self::host()),
            SandboxMode::Docker => Self::docker(
                container
                    .ok_or_else(|| ToolError::Denied("docker container is required".into()))?,
                Path::new(mount.unwrap_or("/workspace")),
            ),
        }
    }
}
#[async_trait::async_trait]
impl CommandExecutor for SandboxExecutor {
    fn is_container_executor(&self) -> bool {
        matches!(self, Self::Docker(_))
    }
    async fn run(
        &self,
        spec: &CommandSpec,
        workspace: &Workspace,
        limits: &OutputLimits,
        timeout: Duration,
        cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        match self {
            Self::Capability(e) => e.run(spec, workspace, limits, timeout, cancel).await,
            Self::Host(e) => e.run(spec, workspace, limits, timeout, cancel).await,
            Self::Docker(e) => e.run(spec, workspace, limits, timeout, cancel).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn capability_denies_even_allowed_programs() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let spec = CommandSpec {
            program: "echo".into(),
            args: vec![],
            cwd: ".".into(),
            env: Default::default(),
        };
        assert!(matches!(
            CapabilityExecutor
                .run(
                    &spec,
                    &ws,
                    &OutputLimits::default(),
                    Duration::from_secs(1),
                    CancellationToken::new()
                )
                .await,
            Err(ToolError::Denied(_))
        ));
    }
    #[test]
    fn host_requires_explicit_mode() {
        assert!(matches!(
            SandboxExecutor::from_config(&SandboxMode::Host, None, None).unwrap(),
            SandboxExecutor::Host(_)
        ));
        assert!(matches!(
            SandboxExecutor::from_config(&SandboxMode::Capability, None, None).unwrap(),
            SandboxExecutor::Capability(_)
        ));
        assert_eq!(
            SandboxMode::from_config_str("unknown"),
            SandboxMode::Capability
        );
    }
    #[test]
    fn docker_abort_stops_the_container_not_just_the_client() {
        let guard = DockerRunGuard {
            binary: "docker".into(),
            container: "dedicated-chat-11".into(),
            marker: "/tmp/.duga-test".into(),
            armed: false,
        };
        let command = guard.abort_command();
        let args: Vec<_> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, ["stop", "--time", "0", "dedicated-chat-11"]);
        let normal = guard.cleanup_command();
        assert_eq!(normal.get_args().next().unwrap(), "exec");
    }

    #[test]
    fn docker_paths_and_quoting() {
        let exec = DockerExecutor::new_unchecked("test", Path::new("/workspace"));
        assert_eq!(
            exec.translate_to_container(Path::new("a")),
            PathBuf::from("/workspace/a")
        );
        assert!(exec.translate_to_host(Path::new("/etc/passwd")).is_none());
        assert_eq!(
            shell_command(
                Path::new("/workspace/src dir"),
                "/bin/echo",
                &["it's ok".into()]
            ),
            "cd '/workspace/src dir' && '/bin/echo' 'it'\\''s ok'"
        );
        assert!(DockerExecutor::with_validation("-bad", Path::new("/workspace"), false).is_err());
    }
}
