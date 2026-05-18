//! BashTool — dispatch to run_captured or ShellSession with timeout enforcement.

use duga_sandbox::binary_registry::BinaryRegistry;
use duga_sandbox::shell_session::{SessionCommand, ShellSession};
use duga_sandbox::{CommandExecutor, CommandSpec, SandboxExecutor, Workspace};
use duga_tools::context::ToolContext;
use duga_tools::result::ToolCallResult;
use duga_tools::Tool;
use duga_types::config::OutputLimits;
use duga_types::error::ToolError;
use duga_types::tool_call::CallId;
use duga_types::tool_result::ToolResult;
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct BashArgs {
    pub command: Vec<String>,
    pub session: Option<Uuid>,
}

pub struct BashTool {
    sessions: Arc<Mutex<HashMap<Uuid, ShellSession>>>,
    pub registry: Arc<BinaryRegistry>,
    pub workspace: Arc<Workspace>,
    pub limits: OutputLimits,
    pub timeout: Duration,
    executor: Arc<dyn CommandExecutor>,
}

impl std::fmt::Debug for BashTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BashTool")
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl Clone for BashTool {
    fn clone(&self) -> Self {
        Self {
            sessions: self.sessions.clone(),
            registry: self.registry.clone(),
            workspace: self.workspace.clone(),
            limits: self.limits.clone(),
            timeout: self.timeout,
            executor: self.executor.clone(),
        }
    }
}

impl BashTool {
    pub fn with_sandbox(
        registry: Arc<BinaryRegistry>,
        workspace: Arc<Workspace>,
        limits: OutputLimits,
        timeout: Duration,
        executor: Arc<dyn CommandExecutor>,
    ) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            registry,
            workspace,
            limits,
            timeout,
            executor,
        }
    }

    pub fn with_capability_sandbox(
        registry: Arc<BinaryRegistry>,
        workspace: Arc<Workspace>,
        limits: OutputLimits,
        timeout: Duration,
    ) -> Self {
        Self::with_sandbox(
            registry,
            workspace,
            limits,
            timeout,
            Arc::new(SandboxExecutor::capability()),
        )
    }
}

impl Default for BashTool {
    fn default() -> Self {
        Self::with_sandbox(
            Arc::new(BinaryRegistry::default()),
            Arc::new(Workspace::open("/tmp").unwrap()),
            OutputLimits::default(),
            Duration::from_secs(120),
            Arc::new(SandboxExecutor::capability()),
        )
    }
}

impl Tool for BashTool {
    type Args = BashArgs;
    fn name(&self) -> &str {
        "bash"
    }
    fn description(&self) -> &str {
        "Execute a command in the sandbox"
    }
    fn retryable(&self) -> bool {
        false
    }

    async fn execute(&self, ctx: ToolContext<'_>, args: Self::Args) -> ToolCallResult {
        let start = std::time::Instant::now();
        if args.command.is_empty() {
            return Err(ToolError::InvalidArgs("Empty command".into()));
        }
        if ctx.is_cancelled() {
            return Err(ToolError::Cancelled);
        }

        let sc = ShellSession::classify(&args.command);

        match sc {
            SessionCommand::Cd(path) => {
                let resolved = self
                    .workspace
                    .resolve(&path)
                    .map_err(|_| ToolError::Denied("cd failed".into()))?;
                if let Some(sid) = args.session {
                    let mut s = self.sessions.lock().await;
                    let sess = s
                        .entry(sid)
                        .or_insert_with(|| ShellSession::new(PathBuf::from(".")));
                    sess.apply(SessionCommand::Cd(resolved.clone()), &self.workspace)
                        .map_err(|e| ToolError::Denied(e.to_string()))?;
                }
                Ok(ToolResult {
                    tool_call_id: CallId::new(),
                    success: true,
                    output: resolved.display().to_string(),
                    metadata: serde_json::json!({"action": "cd"}),
                    duration_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
                    stdout_bytes: 0,
                    stderr_bytes: 0,
                    truncated: false,
                })
            }
            SessionCommand::Export(key, value) => {
                if let Some(sid) = args.session {
                    let mut s = self.sessions.lock().await;
                    let sess = s
                        .entry(sid)
                        .or_insert_with(|| ShellSession::new(PathBuf::from(".")));
                    sess.apply(
                        SessionCommand::Export(key.clone(), value.clone()),
                        &self.workspace,
                    )
                    .map_err(|e| ToolError::Denied(e.to_string()))?;
                }
                Ok(ToolResult {
                    tool_call_id: CallId::new(),
                    success: true,
                    output: format!("exported {}={}", key, value),
                    metadata: serde_json::json!({"action": "export"}),
                    duration_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
                    stdout_bytes: 0,
                    stderr_bytes: 0,
                    truncated: false,
                })
            }
            SessionCommand::Unset(key) => {
                if let Some(sid) = args.session {
                    let mut s = self.sessions.lock().await;
                    let sess = s
                        .entry(sid)
                        .or_insert_with(|| ShellSession::new(PathBuf::from(".")));
                    sess.apply(SessionCommand::Unset(key.clone()), &self.workspace)
                        .map_err(|e| ToolError::Denied(e.to_string()))?;
                }
                Ok(ToolResult {
                    tool_call_id: CallId::new(),
                    success: true,
                    output: format!("unset {}", key),
                    metadata: serde_json::json!({"action": "unset"}),
                    duration_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
                    stdout_bytes: 0,
                    stderr_bytes: 0,
                    truncated: false,
                })
            }
            SessionCommand::Pwd => {
                let cwd = if let Some(sid) = args.session {
                    let s = self.sessions.lock().await;
                    s.get(&sid)
                        .map(|x| x.cwd().clone())
                        .unwrap_or_else(|| PathBuf::from("."))
                } else {
                    PathBuf::from(".")
                };
                Ok(ToolResult {
                    tool_call_id: CallId::new(),
                    success: true,
                    output: cwd.display().to_string(),
                    metadata: serde_json::json!({"action": "pwd"}),
                    duration_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
                    stdout_bytes: 0,
                    stderr_bytes: 0,
                    truncated: false,
                })
            }
            SessionCommand::Spawn(cmd_parts) => {
                let bin = cmd_parts
                    .first()
                    .ok_or_else(|| ToolError::InvalidArgs("no binary".into()))?;

                // In allow-all mode, pass the bare command name directly to the executor.
                // The executor resolves it: Docker via container PATH, capability via
                // the child process's PATH (/usr/bin:/bin). No pre-resolution is done.
                let bpath = if self.registry.is_allow_all() {
                    PathBuf::from(bin)
                } else {
                    self.registry
                        .resolve(bin)
                        .map_err(|e| ToolError::Denied(e.to_string()))?
                        .to_path_buf()
                };
                let (cwd, env) = if let Some(sid) = args.session {
                    let s = self.sessions.lock().await;
                    s.get(&sid)
                        .map(|x| (x.cwd().clone(), x.env().clone()))
                        .unwrap_or_default()
                } else {
                    (PathBuf::from("."), HashMap::new())
                };
                let cancel = ctx.cancellation.clone();
                let spec = CommandSpec {
                    program: bpath.display().to_string(),
                    args: cmd_parts[1..].to_vec(),
                    cwd,
                    env,
                };
                let r = self
                    .executor
                    .run(&spec, &self.workspace, &self.limits, self.timeout, cancel)
                    .await?;
                Ok(r)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use duga_sandbox::exec::CancellationToken;
    use duga_tools::event_sink::NullSink;
    use tempfile::tempdir;

    fn make_ctx(ws: &Workspace) -> ToolContext<'static> {
        let ws: &'static Workspace = unsafe { std::mem::transmute(ws) };
        ToolContext {
            workspace: ws,
            cancellation: CancellationToken::new(),
            event_sink: &NullSink,
        }
    }

    #[test]
    fn test_bash_echo() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let r = Arc::new(BinaryRegistry::new(&["echo".into()]).unwrap());
        let tool = BashTool::with_sandbox(
            r,
            Arc::new(ws.clone()),
            OutputLimits::default(),
            Duration::from_secs(30),
            Arc::new(SandboxExecutor::capability()),
        );
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt
            .block_on(tool.execute(
                make_ctx(&ws),
                BashArgs {
                    command: vec!["echo".into(), "hello".into()],
                    session: None,
                },
            ))
            .unwrap();
        assert!(result.output.contains("hello"));
    }

    #[test]
    fn test_bash_empty() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let r = Arc::new(BinaryRegistry::new(&["echo".into()]).unwrap());
        let tool = BashTool::with_sandbox(
            r,
            Arc::new(ws.clone()),
            OutputLimits::default(),
            Duration::from_secs(30),
            Arc::new(SandboxExecutor::capability()),
        );
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(tool.execute(
            make_ctx(&ws),
            BashArgs {
                command: vec![],
                session: None,
            },
        ));
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn test_bash_respects_cancelled_context() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let r = Arc::new(BinaryRegistry::new(&["sleep".into()]).unwrap());
        let tool = BashTool::with_sandbox(
            r,
            Arc::new(ws.clone()),
            OutputLimits::default(),
            Duration::from_secs(30),
            Arc::new(SandboxExecutor::capability()),
        );
        let cancel = CancellationToken::new();
        cancel.cancel();
        let sink = NullSink;
        let ctx = ToolContext {
            workspace: &ws,
            cancellation: cancel,
            event_sink: &sink,
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(tool.execute(
            ctx,
            BashArgs {
                command: vec!["sleep".into(), "1".into()],
                session: None,
            },
        ));
        assert!(matches!(result, Err(ToolError::Cancelled)));
    }
}
