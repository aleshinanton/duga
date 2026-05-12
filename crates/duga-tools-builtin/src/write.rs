//! WriteTool — atomic write via tempfile + rename with per-path mutex serialization.

use duga_tools::context::ToolContext;
use duga_tools::result::ToolCallResult;
use duga_tools::Tool;
use duga_types::error::ToolError;
use duga_types::tool_call::CallId;
use duga_types::tool_result::ToolResult;
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::NamedTempFile;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct WriteArgs {
    pub path: String,
    pub content: String,
}

#[derive(Clone)]
pub struct WriteTool {
    path_locks: Arc<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>>,
}

impl std::fmt::Debug for WriteTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WriteTool").finish()
    }
}

impl WriteTool {
    pub fn new() -> Self { Self { path_locks: Arc::new(Mutex::new(HashMap::new())) } }

    async fn get_lock(&self, path: &Path) -> Arc<Mutex<()>> {
        let mut locks = self.path_locks.lock().await;
        locks.entry(path.to_path_buf()).or_insert_with(|| Arc::new(Mutex::new(()))).clone()
    }
}

impl Default for WriteTool {
    fn default() -> Self { Self::new() }
}

impl Tool for WriteTool {
    type Args = WriteArgs;
    fn name(&self) -> &str { "write" }
    fn description(&self) -> &str { "Write content to a file" }

    async fn execute(&self, ctx: ToolContext<'_>, args: Self::Args) -> ToolCallResult {
        let start = std::time::Instant::now();
        let resolved = ctx.workspace.resolve(&PathBuf::from(&args.path))
            .map_err(|_| ToolError::Denied(format!("path escapes workspace: {}", args.path)))?;

        let lock = self.get_lock(&resolved).await;
        let _guard = lock.lock().await;

        if let Some(parent) = resolved.parent() {
            if parent != Path::new("") && parent != Path::new(".") {
                ctx.workspace.root_dir().create_dir_all(parent).map_err(ToolError::from)?;
            }
        }

        let temp = NamedTempFile::new_in(ctx.workspace.root_path())
            .map_err(|e| ToolError::Io(e.to_string()))?;
        let tp = temp.path().to_path_buf();
        std::fs::write(&tp, args.content.as_bytes()).map_err(|e| ToolError::Io(e.to_string()))?;
        temp.as_file().sync_all().map_err(|e| ToolError::Io(e.to_string()))?;

        let n = args.content.len();
        let p = args.path.clone();
        let target = ctx.workspace.root_path().join(&resolved);
        match std::fs::rename(&tp, &target) {
            Ok(()) => Ok(ToolResult {
                tool_call_id: CallId::new(), success: true,
                output: format!("Wrote {} bytes to {}", n, p),
                metadata: serde_json::json!({"bytes_written": n}),
                duration_ms: start.elapsed().as_millis(), stdout_bytes: 0, stderr_bytes: 0, truncated: false,
            }),
            Err(e) => match std::fs::copy(&tp, &target) {
                Ok(_) => { let _ = std::fs::remove_file(&tp);
                    Ok(ToolResult {
                        tool_call_id: CallId::new(), success: true,
                        output: format!("Wrote {} bytes to {} (fallback)", n, p),
                        metadata: serde_json::json!({"bytes_written": n, "method": "copy"}),
                        duration_ms: start.elapsed().as_millis(), stdout_bytes: 0, stderr_bytes: 0, truncated: false,
                    })
                }
                Err(_) => Err(ToolError::Io(format!("write failed: {}", e))),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use duga_sandbox::exec::CancellationToken;
    use duga_sandbox::Workspace;
    use duga_tools::event_sink::NullSink;
    use tempfile::tempdir;

    fn make_ctx(ws: &Workspace) -> ToolContext<'static> {
        let ws: &'static Workspace = unsafe { std::mem::transmute(ws) };
        ToolContext { workspace: ws, cancellation: CancellationToken::new(), event_sink: &NullSink }
    }

    #[test]
    fn test_write_basic() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt.block_on(WriteTool::new().execute(make_ctx(&ws), WriteArgs {
            path: "t.txt".into(), content: "hello".into(),
        })).unwrap();
        assert!(r.success);
        assert_eq!(std::fs::read_to_string(dir.path().join("t.txt")).unwrap(), "hello");
    }

    #[test]
    fn test_write_escaped() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt.block_on(WriteTool::new().execute(make_ctx(&ws), WriteArgs {
            path: "../x.txt".into(), content: "x".into(),
        }));
        assert!(r.is_err());
    }
}
