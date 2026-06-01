//! WriteTool — atomic write via tempfile + rename with per-path mutex serialization.

use duga_tools::Tool;
use duga_tools::context::ToolContext;
use duga_tools::result::ToolCallResult;
use duga_types::error::ToolError;
use duga_types::tool_call::CallId;
use duga_types::tool_result::ToolResult;
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct WriteArgs {
    #[schemars(
        description = "Brief human-readable description of what this step does (shown to user)"
    )]
    pub label: String,
    pub path: String,
    pub content: String,
}

#[derive(Clone)]
pub struct WriteTool {
    path_locks: Arc<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>>,
    /// Optional secondary root directories for writing files outside the workspace
    /// (e.g., the bot's data directory for session logs and channel data).
    aux_roots: Vec<PathBuf>,
}

impl std::fmt::Debug for WriteTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WriteTool").finish()
    }
}

impl WriteTool {
    pub fn new() -> Self {
        Self {
            path_locks: Arc::new(Mutex::new(HashMap::new())),
            aux_roots: Vec::new(),
        }
    }

    /// Set secondary allowed roots for file writes (e.g., the Telegram data dir).
    pub fn with_aux_roots(mut self, paths: Vec<PathBuf>) -> Self {
        self.aux_roots = paths;
        self
    }

    /// Resolve a path for writing.
    ///
    /// Relative paths are always workspace-relative so normal file creation does
    /// not silently spill into auxiliary roots. Auxiliary writes must use an
    /// explicit absolute path under an auxiliary root.
    fn resolve_write_path(
        &self,
        ctx: &ToolContext<'_>,
        path_str: &str,
    ) -> Result<(PathBuf, bool), ToolError> {
        let raw = PathBuf::from(path_str);

        // Relative path: workspace only. `execute` creates parent dirs.
        if !raw.is_absolute() {
            return ctx
                .workspace
                .resolve(&raw)
                .map(|resolved| (resolved, false))
                .map_err(|_| ToolError::Denied(format!("path escapes workspace: {}", path_str)));
        }

        // Absolute path under the workspace root.
        if let Ok(relative) = raw.strip_prefix(ctx.workspace.root_path()) {
            return ctx
                .workspace
                .resolve(relative)
                .map(|resolved| (resolved, false))
                .map_err(|_| ToolError::Denied(format!("path escapes workspace: {}", path_str)));
        }

        // Absolute path as seen from Docker/podman sandbox.
        const KNOWN_MOUNT_PREFIXES: &[&str] = &["/workspace/", "/workspace"];
        for prefix in KNOWN_MOUNT_PREFIXES {
            if let Ok(relative) = raw.strip_prefix(prefix) {
                return ctx
                    .workspace
                    .resolve(relative)
                    .map(|resolved| (resolved, false))
                    .map_err(|_| {
                        ToolError::Denied(format!("path escapes workspace: {}", path_str))
                    });
            }
        }

        // Explicit absolute path under an auxiliary root.
        for aux in &self.aux_roots {
            let aux_canonical = aux.canonicalize().unwrap_or_else(|_| aux.clone());
            let relative = raw
                .strip_prefix(aux)
                .or_else(|_| raw.strip_prefix(&aux_canonical));
            if let Ok(relative) = relative {
                if relative.components().any(|component| {
                    matches!(
                        component,
                        std::path::Component::ParentDir
                            | std::path::Component::RootDir
                            | std::path::Component::Prefix(_)
                    )
                }) {
                    return Err(ToolError::Denied(format!(
                        "path escapes auxiliary root: {}",
                        path_str
                    )));
                }
                return Ok((PathBuf::from(relative), true));
            }
        }

        Err(ToolError::Denied(format!(
            "path not in workspace or auxiliary roots: {}",
            path_str
        )))
    }

    /// Find which aux_root contains the parent directory for a relative path.
    /// Returns the full target path and rejects symlink components.
    fn resolve_aux_path(&self, resolved: &Path) -> Result<PathBuf, ToolError> {
        for aux in &self.aux_roots {
            let aux_full = aux.join(resolved);
            if aux_full.exists() || aux_full.parent().is_some_and(|p| p.exists()) {
                Self::reject_aux_symlinks(aux, resolved)?;
                return Ok(aux_full);
            }
        }
        Err(ToolError::Denied(format!(
            "path not found in auxiliary roots: {}",
            resolved.display()
        )))
    }

    /// Reject paths that contain symlink components under an aux_root.
    fn reject_aux_symlinks(aux_root: &Path, rel: &Path) -> Result<(), ToolError> {
        let mut current = aux_root.to_path_buf();
        for component in rel.components() {
            let std::path::Component::Normal(part) = component else {
                continue;
            };
            current.push(part);
            match std::fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(ToolError::Denied(format!(
                        "path contains symlink: {}",
                        current.display()
                    )));
                }
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => return Err(ToolError::Io(e.to_string())),
            }
        }
        Ok(())
    }

    async fn get_lock(&self, path: &Path) -> Arc<Mutex<()>> {
        let mut locks = self.path_locks.lock().await;
        locks
            .entry(path.to_path_buf())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
}

pub(crate) fn reject_symlink_components(
    ctx: &ToolContext<'_>,
    path: &Path,
) -> Result<(), ToolError> {
    let mut current = PathBuf::new();
    for component in path.components() {
        let std::path::Component::Normal(part) = component else {
            continue;
        };
        current.push(part);
        match ctx.workspace.root_dir().symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(ToolError::Denied(format!(
                    "path contains symlink: {}",
                    current.display()
                )));
            }
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(ToolError::Io(e.to_string())),
        }
    }
    Ok(())
}

impl Default for WriteTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for WriteTool {
    type Args = WriteArgs;
    fn name(&self) -> &str {
        "write"
    }
    fn description(&self) -> &str {
        "Write content to a file"
    }

    async fn execute(&self, ctx: ToolContext<'_>, args: Self::Args) -> ToolCallResult {
        let start = std::time::Instant::now();
        let (resolved, is_aux) = self.resolve_write_path(&ctx, &args.path)?;

        let lock = self.get_lock(&resolved).await;
        let _guard = lock.lock().await;

        if is_aux {
            // Write to aux_roots using std::fs with atomic tempfile+rename
            let aux_full = self.resolve_aux_path(&resolved)?;
            if let Some(parent) = aux_full.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| ToolError::Io(format!("cannot create parent dirs: {}", e)))?;
            }
            let temp_aux = aux_full.with_extension(format!("duga-write-{}.tmp", CallId::new()));
            std::fs::write(&temp_aux, &args.content).map_err(|e| {
                ToolError::Io(format!("write to {} failed: {}", temp_aux.display(), e))
            })?;
            std::fs::rename(&temp_aux, &aux_full).map_err(|e| {
                let _ = std::fs::remove_file(&temp_aux);
                ToolError::Io(format!("rename to {} failed: {}", aux_full.display(), e))
            })?;
            let n = args.content.len();
            let p = args.path.clone();
            Ok(ToolResult {
                tool_call_id: CallId::new(),
                success: true,
                output: format!("Wrote {} bytes to {}", n, p),
                steering_hint: None,
                metadata: serde_json::json!({"bytes_written": n}),
                duration_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
                stdout_bytes: 0,
                stderr_bytes: 0,
                truncated: false,
            })
        } else {
            reject_symlink_components(&ctx, &resolved)?;

            if let Some(parent) = resolved.parent() {
                if parent != Path::new("") && parent != Path::new(".") {
                    ctx.workspace
                        .root_dir()
                        .create_dir_all(parent)
                        .map_err(ToolError::from)?;
                }
            }

            reject_symlink_components(&ctx, &resolved)?;

            let temp_path = PathBuf::from(format!(".duga-write-{}.tmp", CallId::new()));
            let mut temp = ctx
                .workspace
                .root_dir()
                .create(&temp_path)
                .map_err(ToolError::from)?;
            temp.write_all(args.content.as_bytes())
                .map_err(ToolError::from)?;
            temp.sync_all().map_err(ToolError::from)?;

            let n = args.content.len();
            let p = args.path.clone();
            match ctx
                .workspace
                .root_dir()
                .rename(&temp_path, ctx.workspace.root_dir(), &resolved)
            {
                Ok(()) => Ok(ToolResult {
                    tool_call_id: CallId::new(),
                    success: true,
                    output: format!("Wrote {} bytes to {}", n, p),
                    steering_hint: None,
                    metadata: serde_json::json!({"bytes_written": n}),
                    duration_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
                    stdout_bytes: 0,
                    stderr_bytes: 0,
                    truncated: false,
                }),
                Err(e) => {
                    let _ = ctx.workspace.root_dir().remove_file(&temp_path);
                    Err(ToolError::Io(format!("write failed: {}", e)))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use duga_sandbox::Workspace;
    use duga_sandbox::exec::CancellationToken;
    use duga_tools::event_sink::NullSink;
    use tempfile::tempdir;

    fn make_ctx(ws: &Workspace) -> ToolContext<'static> {
        let ws: &'static Workspace = unsafe { std::mem::transmute(ws) };
        ToolContext {
            workspace: ws,
            cancellation: CancellationToken::new(),
            event_sink: &NullSink,
            aux_root: None,
        }
    }

    #[test]
    fn test_write_basic() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt
            .block_on(WriteTool::new().execute(
                make_ctx(&ws),
                WriteArgs {
                    label: "Writing t.txt".into(),
                    path: "t.txt".into(),
                    content: "hello".into(),
                },
            ))
            .unwrap();
        assert!(r.success);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("t.txt")).unwrap(),
            "hello"
        );
    }

    #[test]
    fn test_write_escaped() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt.block_on(WriteTool::new().execute(
            make_ctx(&ws),
            WriteArgs {
                label: "Writing ../x.txt".into(),
                path: "../x.txt".into(),
                content: "x".into(),
            },
        ));
        assert!(r.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn test_write_rejects_symlink_parent() {
        let workspace_dir = tempdir().unwrap();
        let outside_dir = tempdir().unwrap();
        std::os::unix::fs::symlink(outside_dir.path(), workspace_dir.path().join("link")).unwrap();

        let ws = Workspace::open(workspace_dir.path()).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt.block_on(WriteTool::new().execute(
            make_ctx(&ws),
            WriteArgs {
                label: "Writing link/pwn.txt".into(),
                path: "link/pwn.txt".into(),
                content: "x".into(),
            },
        ));

        assert!(r.is_err());
        assert!(!outside_dir.path().join("pwn.txt").exists());
    }

    // ── resolve_write_path tests ─────────────────────────────────────

    #[test]
    fn test_resolve_write_relative_in_workspace() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let ctx = make_ctx(&ws);
        let tool = WriteTool::new();
        let (resolved, is_aux) = tool.resolve_write_path(&ctx, "sub/new.txt").unwrap();
        assert!(!is_aux);
        assert_eq!(resolved, PathBuf::from("sub/new.txt"));
    }

    #[test]
    fn test_resolve_write_relative_root_level() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let ctx = make_ctx(&ws);
        let tool = WriteTool::new();
        let (resolved, is_aux) = tool.resolve_write_path(&ctx, "new.txt").unwrap();
        assert!(!is_aux);
        assert_eq!(resolved, PathBuf::from("new.txt"));
    }

    #[test]
    fn test_resolve_write_relative_prefers_workspace_with_aux_root() {
        let dir = tempdir().unwrap();
        let aux = tempdir().unwrap();
        std::fs::create_dir_all(aux.path().join("logs")).unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let ctx = make_ctx(&ws);
        let tool = WriteTool::new().with_aux_roots(vec![aux.path().to_path_buf()]);
        let (resolved, is_aux) = tool.resolve_write_path(&ctx, "logs/output.log").unwrap();
        assert!(!is_aux);
        assert_eq!(resolved, PathBuf::from("logs/output.log"));
    }

    #[test]
    fn test_resolve_write_absolute_aux_path_to_aux() {
        let dir = tempdir().unwrap();
        let aux = tempdir().unwrap();
        std::fs::create_dir_all(aux.path().join("logs")).unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let ctx = make_ctx(&ws);
        let tool = WriteTool::new().with_aux_roots(vec![aux.path().to_path_buf()]);
        let path = aux.path().join("logs/output.log");
        let (resolved, is_aux) = tool
            .resolve_write_path(&ctx, path.to_str().unwrap())
            .unwrap();
        assert!(is_aux);
        assert_eq!(resolved, PathBuf::from("logs/output.log"));
    }

    #[test]
    fn test_resolve_write_absolute_aux_path_rejects_parent_traversal() {
        let dir = tempdir().unwrap();
        let aux = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let ctx = make_ctx(&ws);
        let tool = WriteTool::new().with_aux_roots(vec![aux.path().to_path_buf()]);
        let path = aux.path().join("../outside.txt");
        let result = tool.resolve_write_path(&ctx, path.to_str().unwrap());

        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_write_absolute_workspace_prefix_to_workspace() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let ctx = make_ctx(&ws);
        let tool = WriteTool::new();
        let (resolved, is_aux) = tool
            .resolve_write_path(&ctx, "/workspace/README.md")
            .unwrap();
        assert!(!is_aux);
        assert_eq!(resolved, PathBuf::from("README.md"));
    }

    #[test]
    fn test_resolve_write_not_found() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let ctx = make_ctx(&ws);
        let tool = WriteTool::new();
        let (resolved, is_aux) = tool
            .resolve_write_path(&ctx, "/workspace/deep/nested/new.txt")
            .unwrap();
        assert!(!is_aux);
        assert_eq!(resolved, PathBuf::from("deep/nested/new.txt"));
    }

    #[test]
    fn test_resolve_write_workspace_priority() {
        let dir = tempdir().unwrap();
        let aux = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("logs")).unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let ctx = make_ctx(&ws);
        let tool = WriteTool::new().with_aux_roots(vec![aux.path().to_path_buf()]);
        let (resolved, is_aux) = tool.resolve_write_path(&ctx, "logs/app.log").unwrap();
        assert!(!is_aux, "workspace should have priority");
        assert_eq!(resolved, PathBuf::from("logs/app.log"));
    }
}
