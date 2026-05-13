//! Process execution with output limits and cancellation.
//!
//! Provides `run_captured()` — the single sandbox process-spawning function —
//! along with `CancellationToken`, `format_output()`, and `truncation_notice()`.

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
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::sync::watch;

// ── CancellationToken ────────────────────────────────────────────────────────

/// A cancellation token that can be signalled to abort running processes.
///
/// Uses a `tokio::sync::watch` channel internally so that `run_captured`
/// can `select!` on cancellation without busy-waiting.
#[derive(Debug, Clone)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
    tx:        Arc<watch::Sender<()>>,
}

impl Default for CancellationToken {
    fn default() -> Self {
        let (tx, _) = watch::channel(());
        Self { cancelled: Arc::new(AtomicBool::new(false)), tx: Arc::new(tx) }
    }
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// Signal that execution should be cancelled.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        // Best-effort: receiver may have been dropped if process already finished.
        let _ = self.tx.send(());
    }

    /// Returns `true` if cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Returns a watch receiver that resolves when `cancel()` is called.
    pub fn subscribe(&self) -> watch::Receiver<()> {
        self.tx.subscribe()
    }
}

// ── Output formatting ───────────────────────────────────────────────────────

/// Format combined stdout/stderr output for a completed process.
///
/// Rules:
/// - If stderr is empty → return stdout as-is
/// - If stderr is non-empty → prepend labels so the LLM can distinguish streams
/// - If the process was killed by a signal, show "SIGNAL: {signal}" instead of exit code
///
/// This is the function specified in TASK-5.1 (§17, §18).
pub fn format_output(stdout: &str, stderr: &str, exit_code: Option<i32>) -> String {
    let show_code = exit_code != Some(0); // signal kill = show
    let has_content = !stdout.is_empty() || !stderr.is_empty();

    match (has_content, stderr.is_empty(), show_code) {
        // Both streams empty, non-zero exit or signal: just show the exit code
        (false, _, true) => match exit_code {
            Some(code) => format!("EXIT: {}", code),
            None => String::from("SIGNAL: killed"),
        },
        // Both streams empty, exit 0: empty
        (false, _, false) => String::new(),
        // Only stdout, no stderr:
        (true, true, true) => match exit_code {
            Some(code) => format!("{}\nEXIT: {}", stdout, code),
            None => format!("{}\nSIGNAL: killed", stdout),
        },
        (true, true, false) => stdout.to_string(),
        // Stderr present:
        (_, false, _) => {
            let exit_str = match exit_code {
                Some(code) => format!("EXIT: {}", code),
                None => String::from("SIGNAL: killed"),
            };
            format!("STDOUT:\n{}\nSTDERR:\n{}\n{}", stdout, stderr, exit_str)
        }
    }
}

/// Build a truncation notice string for a specific limit type.
///
/// Format per TASK-5.3 (§18):
/// - `"stdout exceeded {max} byte limit"`
/// - `"stderr exceeded {max} byte limit"`
/// - `"combined output exceeded {max} byte limit"`
pub fn truncation_notice(limit_type: &str, max: usize) -> String {
    format!("{} exceeded {} byte limit", limit_type, max)
}

// ── Process spawning ─────────────────────────────────────────────────────────

/// Execute a subprocess with captured output, limits, timeout, and cancellation.
///
/// # Behaviour
/// - **Exit codes**: a non-zero exit code does NOT set `success: false`. The
///   tool ran to completion; the LLM sees the exit code in the output. Only
///   platform errors (spawn failure) result in `ToolError`.
/// - **Timeout**: if the process exceeds `timeout`, it is killed and
///   `ToolError::Timeout` is returned.
/// - **Cancellation**: if `cancel` is signalled first, the process is killed
///   and `ToolError::Cancelled` is returned.
/// - **Truncation**: if stdout or stderr exceed their individual limits, or
///   the combined output exceeds the combined limit, output is truncated and
///   a machine-readable notice is appended.
///
/// # Errors
/// - `ToolError::Timeout` — process exceeded the timeout duration
/// - `ToolError::Cancelled` — cancellation requested before/while waiting
/// - `ToolError::Io` — process spawn or I/O failure
#[allow(clippy::too_many_arguments)]
pub async fn run_captured(
    binary: &Path,
    args: &[String],
    workspace: &Workspace,
    cwd: &Path,
    env: &HashMap<String, String>,
    limits: &OutputLimits,
    timeout: Duration,
    cancel: CancellationToken,
) -> Result<ToolResult, ToolError> {
    // Early cancellation check
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

    // ── Wait future with timeout ─────────────────────────────────────────
    // Cancellation watcher via tokio::sync::watch
    let mut cancel_rx = cancel.subscribe();

    // Wrap child.wait() in a timeout. The resulting future is !Unpin
    // (because Child::wait borrows Child which is !Unpin), so pin it.
    tokio::pin! {
        let child_wait = tokio::time::timeout(timeout, child.wait());
    }

    // Biased select: timeout checked first, then cancellation, then natural exit.
    let exit_status = tokio::select! {
        biased;

        result = child_wait.as_mut() => {
            match result {
                Ok(Ok(status)) => status,
                Ok(Err(e)) => return Err(ToolError::Io(e.to_string())),
                Err(_) => {
                    // Timeout elapsed — child is dropped at return, killing it (kill_on_drop)
                    return Err(ToolError::Timeout);
                }
            }
        }

        // Cancellation branch
        _ = cancel_rx.changed() => {
            // child is dropped on return, killing it (kill_on_drop)
            return Err(ToolError::Cancelled);
        }
    };

    // ── Process exited normally; read output with limits ─────────────────

    // Helper: read up to `limit` bytes from an async reader.
    async fn read_limited<R: AsyncReadExt + Unpin>(
        reader: &mut R,
        limit: u64,
    ) -> (u64, Vec<u8>, bool) {
        let mut buf = Vec::new();
        let mut scratch = vec![0u8; 8192];
        let mut total: u64 = 0;
        let mut truncated = false;

        loop {
            let remaining = limit.saturating_sub(total);
            if remaining == 0 {
                truncated = true;
                break;
            }
            let to_read = remaining.min(scratch.len() as u64) as usize;
            match reader.read(&mut scratch[..to_read]).await {
                Ok(0) => break,
                Ok(n) => {
                    let take = n.min(remaining as usize);
                    buf.extend_from_slice(&scratch[..take]);
                    total += take as u64;
                    if take < n {
                        truncated = true;
                        break;
                    }
                }
                Err(_) => break,
            }
        }

        (total, buf, truncated)
    }

    let mut stdout_reader = tokio::io::BufReader::new(stdout_fd);
    let (stdout_bytes, stdout_buf, stdout_trunc) =
        read_limited(&mut stdout_reader, limits.max_stdout_bytes).await;

    let mut stderr_reader = tokio::io::BufReader::new(stderr_fd);
    let (stderr_bytes, stderr_buf, stderr_trunc) =
        read_limited(&mut stderr_reader, limits.max_stderr_bytes).await;

    // Combined limit check
    let combined_bytes = stdout_bytes + stderr_bytes;
    let combined_trunc = combined_bytes > limits.max_combined_bytes;
    let truncated = stdout_trunc || stderr_trunc || combined_trunc;

    // ── Build output string ──────────────────────────────────────────────
    let output = format_output(
        &String::from_utf8_lossy(&stdout_buf),
        &String::from_utf8_lossy(&stderr_buf),
        exit_status.code(),
    );

    // Append truncation notices (TASK-5.3).
    let output = if truncated {
        let mut o = output;
        if stdout_trunc {
            let notice = truncation_notice("stdout", limits.max_stdout_bytes as usize);
            o.push_str("\n[TRUNCATED] ");
            o.push_str(&notice);
        }
        if stderr_trunc {
            let notice = truncation_notice("stderr", limits.max_stderr_bytes as usize);
            o.push_str("\n[TRUNCATED] ");
            o.push_str(&notice);
        }
        if combined_trunc {
            let notice = truncation_notice("combined output", limits.max_combined_bytes as usize);
            o.push_str("\n[TRUNCATED] ");
            o.push_str(&notice);
        }
        o
    } else {
        output
    };

    // ── Build ToolResult ─────────────────────────────────────────────────
    let exit_code = exit_status.code();
    let metadata = match exit_code {
        Some(code) => serde_json::json!({ "exit_code": code }),
        None => serde_json::json!({ "exit_code": null, "signal": "killed" }),
    };

    Ok(ToolResult {
        tool_call_id: CallId::new(),
        success: true, // Tool ran; non-zero exit is still a successful execution
        output,
        metadata,
        duration_ms: 0, // Filled by caller via ToolResult::from_outcome if needed
        stdout_bytes,
        stderr_bytes,
        truncated,
    })
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use duga_types::config::OutputLimits;
    use std::time::Duration;

    fn default_limits() -> OutputLimits {
        OutputLimits {
            max_stdout_bytes: 4 * 1024 * 1024,
            max_stderr_bytes: 4 * 1024 * 1024,
            max_combined_bytes: 8 * 1024 * 1024,
        }
    }

    // ── TASK-5.1: exit code handling and output formatting ──────────────

    #[cfg(unix)]
    #[tokio::test]
    async fn test_exit_code_zero() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let binary = which::which("true").unwrap();
        let cancel = CancellationToken::new();

        let result = run_captured(
            &binary,
            &[],
            &ws,
            Path::new("."),
            &HashMap::new(),
            &default_limits(),
            Duration::from_secs(5),
            cancel,
        )
        .await
        .unwrap();

        assert!(result.success, "exit 0 should be success=true");
        assert!(result.metadata["exit_code"].is_number());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_exit_code_nonzero_not_error() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let binary = which::which("false").unwrap();
        let cancel = CancellationToken::new();

        let result = run_captured(
            &binary,
            &[],
            &ws,
            Path::new("."),
            &HashMap::new(),
            &default_limits(),
            Duration::from_secs(5),
            cancel,
        )
        .await
        .unwrap();

        assert!(result.success, "non-zero exit should still be success=true");
        assert_eq!(result.metadata["exit_code"], 1);
        assert!(result.output.contains("EXIT: 1"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_format_output_stderr_present() {
        let binary = which::which("sh").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let cancel = CancellationToken::new();

        let result = run_captured(
            &binary,
            &["-c".into(), "echo out; echo err >&2".into()],
            &ws,
            Path::new("."),
            &HashMap::new(),
            &default_limits(),
            Duration::from_secs(5),
            cancel,
        )
        .await
        .unwrap();

        assert!(result.output.contains("STDOUT:"), "should label stdout");
        assert!(result.output.contains("STDERR:"), "should label stderr");
        assert!(result.output.contains("EXIT: 0"), "should show exit code");
        assert!(result.success);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_format_output_stderr_only() {
        let binary = which::which("sh").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let cancel = CancellationToken::new();

        let result = run_captured(
            &binary,
            &["-c".into(), "echo err message >&2".into()],
            &ws,
            Path::new("."),
            &HashMap::new(),
            &default_limits(),
            Duration::from_secs(5),
            cancel,
        )
        .await
        .unwrap();

        assert!(result.output.contains("STDERR:"), "stderr-only should still label");
        assert!(result.output.contains("STDOUT:"), "stdout label present even if empty");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_format_output_stdout_only() {
        let binary = which::which("echo").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let cancel = CancellationToken::new();

        let result = run_captured(
            &binary,
            &["hello".into()],
            &ws,
            Path::new("."),
            &HashMap::new(),
            &default_limits(),
            Duration::from_secs(5),
            cancel,
        )
        .await
        .unwrap();

        // stdout only → no labels, just the text
        assert!(!result.output.contains("STDOUT:"), "stdout only should not have label");
        assert!(!result.output.contains("STDERR:"), "stdout only should not have STDERR label");
        assert!(result.output.contains("hello"));
    }

    #[tokio::test]
    async fn test_format_output_format_output_fn() {
        // Test the standalone format_output function directly
        assert_eq!(format_output("hello", "", Some(0)), "hello");
        assert_eq!(format_output("", "err", Some(1)), "STDOUT:\n\nSTDERR:\nerr\nEXIT: 1");
        assert!(format_output("out", "err", Some(0)).contains("STDOUT:"));
        assert!(format_output("out", "err", None).contains("SIGNAL"));
    }

    // ── TASK-5.2: timeout + cancellation interaction ───────────────────

    #[cfg(unix)]
    #[tokio::test]
    async fn test_timeout_kills_process() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let binary = which::which("sleep").unwrap();
        let cancel = CancellationToken::new();

        let result = run_captured(
            &binary,
            &["0.2".into()],
            &ws,
            Path::new("."),
            &HashMap::new(),
            &default_limits(),
            Duration::from_millis(50),
            cancel,
        )
        .await;

        assert!(matches!(result, Err(ToolError::Timeout)), "sleep 0.2 with 50ms timeout should timeout");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_cancellation_kills_process() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let binary = which::which("sleep").unwrap();
        let cancel = CancellationToken::new();

        // Cancel immediately in background
        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel_clone.cancel();
        });

        let result = run_captured(
            &binary,
            &["0.5".into()],
            &ws,
            Path::new("."),
            &HashMap::new(),
            &default_limits(),
            Duration::from_secs(10),
            cancel,
        )
        .await;

        assert!(matches!(result, Err(ToolError::Cancelled)), "cancelled sleep should return Cancelled");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_timeout_before_cancel_wins() {
        // Both timeout and cancel armed; timeout fires first due to bias
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let binary = which::which("sleep").unwrap();
        let cancel = CancellationToken::new();

        // Cancel scheduled for later than the timeout
        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            cancel_clone.cancel();
        });

        let result = run_captured(
            &binary,
            &["0.5".into()],
            &ws,
            Path::new("."),
            &HashMap::new(),
            &default_limits(),
            Duration::from_millis(100),
            cancel,
        )
        .await;

        assert!(matches!(result, Err(ToolError::Timeout)), "timeout should win over later cancel");
    }

    // ── TASK-5.3: truncation formatting ────────────────────────────────

    #[tokio::test]
    async fn test_truncation_notice_stdout() {
        let notice = truncation_notice("stdout", 4096);
        assert!(notice.contains("stdout"));
        assert!(notice.contains("4096"));
    }

    #[tokio::test]
    async fn test_truncation_notice_stderr() {
        let notice = truncation_notice("stderr", 4096);
        assert!(notice.contains("stderr"));
    }

    #[tokio::test]
    async fn test_truncation_notice_combined() {
        let notice = truncation_notice("combined output", 8192);
        assert!(notice.contains("combined output"));
    }

    #[tokio::test]
    async fn test_stdout_truncation() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let binary = which::which("sh").unwrap();
        let cancel = CancellationToken::new();

        // Generate more output than a tiny limit allows
        let limits = OutputLimits {
            max_stdout_bytes: 16, // very small
            max_stderr_bytes: 4 * 1024 * 1024,
            max_combined_bytes: 8 * 1024 * 1024,
        };

        let result = run_captured(
            &binary,
            &["-c".into(), "echo 'this is a long output that exceeds the tiny limit'".into()],
            &ws,
            Path::new("."),
            &HashMap::new(),
            &limits,
            Duration::from_secs(5),
            cancel,
        )
        .await
        .unwrap();

        assert!(result.truncated, "should be marked truncated");
        assert!(result.output.contains("[TRUNCATED]"), "should contain truncation notice");
        assert!(result.output.contains("stdout"), "should mention stdout in notice");
    }

    #[tokio::test]
    async fn test_no_truncation_within_limits() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let binary = which::which("echo").unwrap();
        let cancel = CancellationToken::new();

        let result = run_captured(
            &binary,
            &["hello".into()],
            &ws,
            Path::new("."),
            &HashMap::new(),
            &default_limits(),
            Duration::from_secs(5),
            cancel,
        )
        .await
        .unwrap();

        assert!(!result.truncated, "short output should not be truncated");
        assert!(!result.output.contains("[TRUNCATED]"), "no truncation notice");
    }

    // ── TASK-5.5: edge cases & combined scenarios ───────────────────────

    #[cfg(unix)]
    #[tokio::test]
    async fn test_signal_kill_exit_code_none() {
        let output = format_output("", "", None);
        assert!(output.contains("SIGNAL"), "signal kill should show SIGNAL");
    }

    #[tokio::test]
    async fn test_both_stdout_and_stderr_empty() {
        let output = format_output("", "", Some(0));
        assert!(output.is_empty(), "both empty → empty output");
    }

    #[tokio::test]
    async fn test_cancellation_is_cancelled_flag() {
        let token = CancellationToken::new();
        assert!(!token.is_cancelled());
        token.cancel();
        assert!(token.is_cancelled());
    }
}