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

    /// Try to resolve a path for writing: workspace first, then aux_roots.
    fn resolve_write_path(&self, ctx: &ToolContext<'_>, path_str: &str) -> Result<(PathBuf, bool), ToolError> {
        let raw = PathBuf::from(path_str);
        // Only accept workspace if the file OR its parent directory exists there.
        if let Ok(resolved) = ctx.workspace.resolve(&raw) {
            let exists_in_ws = ctx.workspace.root_dir().metadata(&resolved).is_ok();
            let is_root_level = resolved.parent().map_or(true, |p| {
                p == Path::new("") || p == Path::new(".")
            });
            let parent_exists_in_ws = !is_root_level && resolved.parent().map_or(false, |p| {
                ctx.workspace.root_dir().metadata(p).is_ok()
            });
            if exists_in_ws || is_root_level || parent_exists_in_ws {
                return Ok((resolved, false));
            }
        }
        // Then try aux_roots — only for relative paths where parent exists.
        // Reject paths with parent traversal to prevent escaping aux_root.
        if !raw.is_absolute() && !self.aux_roots.is_empty() {
            if raw.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
                return Err(ToolError::Denied(format!(
                    "path escapes auxiliary roots: {}", path_str
                )));
            }
            for aux in &self.aux_roots {
                let aux_full = aux.join(&raw);
                let exists_in_aux = aux_full.exists();
                let parent_exists_in_aux = aux_full.parent().map_or(false, |p| {
                    p.exists()
                });
                if exists_in_aux || parent_exists_in_aux {
                    return Ok((raw, true));
                }
            }
        }
        Err(ToolError::Denied(format!("path not in workspace or auxiliary roots: {}", path_str)))
    }

    /// Find which aux_root contains the parent directory for a relative path.
    /// Returns the full target path and rejects symlink components.
    fn resolve_aux_path(&self, resolved: &Path) -> Result<PathBuf, ToolError> {
        // Pick the first aux_root — write should create under it.
        let aux = self.aux_roots.first().ok_or_else(|| {
            ToolError::Denied("no auxiliary roots configured".into())
        })?;
        let aux_full = aux.join(resolved);
        // Symlink check: walk each component from aux_root to the target
        Self::reject_aux_symlinks(aux, resolved)?;
        Ok(aux_full)
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
                std::fs::create_dir_all(parent).map_err(|e| {
                    ToolError::Io(format!("cannot create parent dirs: {}", e))
                })?;
            }
            let temp_aux = aux_full.with_extension(format!(
                "duga-write-{}.tmp",
                CallId::new()
            ));
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
    use duga_sandbox::exec::CancellationToken;
    use duga_sandbox::Workspace;
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
}
