//! Process execution with output limits and cancellation.

use crate::workspace::Workspace;
use duga_types::config::OutputLimits;
use duga_types::error::ToolError;
use duga_types::tool_call::CallId;
use duga_types::tool_result::ToolResult;
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;

/// A cancellation token that can be signalled to abort running processes.
#[derive(Debug, Clone)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self { cancelled: Arc::new(AtomicBool::new(false)) }
    }
}

impl CancellationToken {
    pub fn new() -> Self { Self::default() }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

/// Execute a subprocess with captured output, limits, and cancellation.
#[allow(clippy::too_many_arguments)]
pub async fn run_captured(
    binary: &Path,
    args: &[String],
    workspace: &Workspace,
    cwd: &Path,
    env: &HashMap<String, String>,
    limits: &OutputLimits,
    _timeout: Duration,
    cancel: CancellationToken,
) -> Result<ToolResult, ToolError> {
    if cancel.is_cancelled() {
        return Err(ToolError::Cancelled);
    }

    let full_cwd = workspace.root_path().join(cwd);

    let mut child = tokio::process::Command::new(binary)
        .args(args)
        .current_dir(&full_cwd)
        .envs(env)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| ToolError::Io(e.to_string()))?;

    let stdout_fd = child.stdout.take().expect("stdout piped");
    let stderr_fd = child.stderr.take().expect("stderr piped");

    let start = Instant::now();

    let status = child.wait().await.map_err(|e| ToolError::Io(e.to_string()))?;

    // Read stdout with byte limit
    let mut stdout_buf = Vec::new();
    let mut stdout_reader = tokio::io::BufReader::new(stdout_fd);
    let mut stdout_bytes: u64 = 0;
    let mut stdout_truncated = false;
    let mut buf = vec![0u8; 8192];

    loop {
        let n = match stdout_reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        let remaining = limits.max_stdout_bytes.saturating_sub(stdout_bytes);
        if remaining == 0 {
            stdout_truncated = true;
            break;
        }
        let take = n.min(remaining as usize);
        stdout_buf.extend_from_slice(&buf[..take]);
        stdout_bytes += take as u64;
        if take < n {
            stdout_truncated = true;
            break;
        }
    }

    // Read stderr with byte limit
    let mut stderr_buf = Vec::new();
    let mut stderr_reader = tokio::io::BufReader::new(stderr_fd);
    let mut stderr_bytes: u64 = 0;
    let mut stderr_truncated = false;

    loop {
        let n = match stderr_reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        let remaining = limits.max_stderr_bytes.saturating_sub(stderr_bytes);
        if remaining == 0 {
            stderr_truncated = true;
            break;
        }
        let take = n.min(remaining as usize);
        stderr_buf.extend_from_slice(&buf[..take]);
        stderr_bytes += take as u64;
        if take < n {
            stderr_truncated = true;
            break;
        }
    }

    // Check combined limit
    let combined = stdout_bytes + stderr_bytes;
    let combined_truncated = combined > limits.max_combined_bytes;
    let truncated = stdout_truncated || stderr_truncated || combined_truncated;

    let duration_ms = start.elapsed().as_millis();

    let output = if stderr_buf.is_empty() {
        String::from_utf8_lossy(&stdout_buf).to_string()
    } else {
        format!(
            "{}\n{}",
            String::from_utf8_lossy(&stdout_buf),
            String::from_utf8_lossy(&stderr_buf)
        )
    };

    let success = status.success() && !truncated;

    Ok(ToolResult {
        tool_call_id: CallId::new(),
        success,
        output,
        metadata: serde_json::Value::Null,
        duration_ms,
        stdout_bytes,
        stderr_bytes,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use duga_types::config::OutputLimits;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_run_captured_echo() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let limits = OutputLimits::default();
        let cancel = CancellationToken::new();
        let binary = which::which("echo").unwrap();

        let result = run_captured(
            &binary, &["hello world".into()], &ws, Path::new("."),
            &HashMap::new(), &limits, Duration::from_secs(5), cancel,
        ).await.unwrap();

        assert!(result.output.contains("hello world"));
        assert!(result.success);
    }
}
