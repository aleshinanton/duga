//! EditTool — targeted text replacement for existing workspace files.

use duga_tools::context::ToolContext;
use duga_tools::result::ToolCallResult;
use duga_tools::Tool;
use duga_types::error::ToolError;
use duga_types::tool_call::CallId;
use duga_types::tool_result::ToolResult;
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct EditArgs {
    #[schemars(
        description = "Brief human-readable description of what this step does (shown to user)"
    )]
    pub label: String,
    #[schemars(
        description = "Path to an existing file, absolute within the workspace or workspace-relative"
    )]
    pub path: String,
    #[serde(rename = "oldText")]
    #[schemars(description = "Exact unique text block to replace, including whitespace")]
    pub old_text: String,
    #[serde(rename = "newText")]
    #[schemars(description = "Replacement text to write in place of oldText")]
    pub new_text: String,
}

#[derive(Clone)]
pub struct EditTool {
    path_locks: Arc<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>>,
}

impl std::fmt::Debug for EditTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EditTool").finish()
    }
}

impl Default for EditTool {
    fn default() -> Self {
        Self::new()
    }
}

impl EditTool {
    pub fn new() -> Self {
        Self {
            path_locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn get_lock(&self, path: &Path) -> Arc<Mutex<()>> {
        let mut locks = self.path_locks.lock().await;
        locks
            .entry(path.to_path_buf())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
}

fn resolve_path(ctx: &ToolContext<'_>, path: &str) -> Result<PathBuf, ToolError> {
    let raw = PathBuf::from(path);
    if raw.is_absolute() {
        if raw
            .symlink_metadata()
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err(ToolError::Denied(format!("path is a symlink: {}", path)));
        }
        let absolute = raw
            .canonicalize()
            .map_err(|_| ToolError::Denied(format!("path escapes workspace: {}", path)))?;
        let relative = absolute
            .strip_prefix(ctx.workspace.root_path())
            .map_err(|_| ToolError::Denied(format!("path escapes workspace: {}", path)))?;
        return ctx
            .workspace
            .resolve(relative)
            .map_err(|_| ToolError::Denied(format!("path escapes workspace: {}", path)));
    }

    ctx.workspace
        .resolve(&raw)
        .map_err(|_| ToolError::Denied(format!("path escapes workspace: {}", path)))
}

impl Tool for EditTool {
    type Args = EditArgs;

    fn name(&self) -> &str {
        "edit"
    }

    fn description(&self) -> &str {
        "Perform a targeted text replacement in an existing file"
    }

    async fn execute(&self, ctx: ToolContext<'_>, args: Self::Args) -> ToolCallResult {
        let start = std::time::Instant::now();
        if args.old_text.is_empty() {
            return Err(ToolError::InvalidArgs("oldText must not be empty".into()));
        }

        let resolved = resolve_path(&ctx, &args.path)?;
        let lock = self.get_lock(&resolved).await;
        let _guard = lock.lock().await;

        crate::write::reject_symlink_components(&ctx, &resolved)?;

        let metadata = ctx
            .workspace
            .root_dir()
            .metadata(&resolved)
            .map_err(ToolError::from)?;
        if !metadata.is_file() {
            return Err(ToolError::InvalidArgs(format!(
                "path is not a file: {}",
                args.path
            )));
        }

        let mut file = ctx
            .workspace
            .root_dir()
            .open(&resolved)
            .map_err(ToolError::from)?;
        let mut content = String::new();
        file.read_to_string(&mut content).map_err(|e| {
            ToolError::InvalidArgs(format!("file is not valid UTF-8: {} ({})", args.path, e))
        })?;

        let matches = content.match_indices(&args.old_text).count();
        match matches {
            0 => {
                return Err(ToolError::InvalidArgs(format!(
                    "oldText not found in {}",
                    args.path
                )))
            }
            1 => {}
            count => {
                return Err(ToolError::InvalidArgs(format!(
                    "oldText appears {} times in {}; include more context for a unique replacement",
                    count, args.path
                )))
            }
        }

        let updated = content.replacen(&args.old_text, &args.new_text, 1);
        let temp_path = PathBuf::from(format!(".duga-edit-{}.tmp", CallId::new()));
        let mut temp = ctx
            .workspace
            .root_dir()
            .create(&temp_path)
            .map_err(ToolError::from)?;
        temp.write_all(updated.as_bytes())
            .map_err(ToolError::from)?;
        temp.sync_all().map_err(ToolError::from)?;

        match ctx
            .workspace
            .root_dir()
            .rename(&temp_path, ctx.workspace.root_dir(), &resolved)
        {
            Ok(()) => Ok(ToolResult {
                tool_call_id: CallId::new(),
                success: true,
                output: format!(
                    "Edited {}: replaced {} bytes with {} bytes",
                    args.path,
                    args.old_text.len(),
                    args.new_text.len()
                ),
                metadata: serde_json::json!({
                    "path": args.path,
                    "old_bytes": args.old_text.len(),
                    "new_bytes": args.new_text.len(),
                    "bytes_written": updated.len()
                }),
                duration_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
                stdout_bytes: 0,
                stderr_bytes: 0,
                truncated: false,
            }),
            Err(e) => {
                let _ = ctx.workspace.root_dir().remove_file(&temp_path);
                Err(ToolError::Io(format!("edit failed: {}", e)))
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
        }
    }

    fn args(path: &str, old_text: &str, new_text: &str) -> EditArgs {
        EditArgs {
            label: "Editing file".into(),
            path: path.into(),
            old_text: old_text.into(),
            new_text: new_text.into(),
        }
    }

    #[test]
    fn test_edit_replaces_unique_text() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello old world").unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();

        let result = rt
            .block_on(EditTool::new().execute(make_ctx(&ws), args("test.txt", "old", "new")))
            .unwrap();

        assert!(result.success);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("test.txt")).unwrap(),
            "hello new world"
        );
    }

    #[test]
    fn test_edit_accepts_absolute_workspace_path() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "hello old world").unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();

        rt.block_on(
            EditTool::new().execute(make_ctx(&ws), args(file.to_str().unwrap(), "old", "new")),
        )
        .unwrap();

        assert_eq!(std::fs::read_to_string(file).unwrap(), "hello new world");
    }

    #[test]
    fn test_edit_rejects_missing_old_text() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello world").unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();

        let result =
            rt.block_on(EditTool::new().execute(make_ctx(&ws), args("test.txt", "missing", "new")));

        assert!(matches!(result, Err(ToolError::InvalidArgs(_))));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("test.txt")).unwrap(),
            "hello world"
        );
    }

    #[test]
    fn test_edit_rejects_duplicate_old_text() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "old and old").unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();

        let result =
            rt.block_on(EditTool::new().execute(make_ctx(&ws), args("test.txt", "old", "new")));

        assert!(matches!(result, Err(ToolError::InvalidArgs(_))));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("test.txt")).unwrap(),
            "old and old"
        );
    }

    #[test]
    fn test_edit_rejects_escaped_path() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();

        let result = rt
            .block_on(EditTool::new().execute(make_ctx(&ws), args("../outside.txt", "old", "new")));

        assert!(matches!(result, Err(ToolError::Denied(_))));
    }

    #[cfg(unix)]
    #[test]
    fn test_edit_rejects_symlink_path() {
        let workspace_dir = tempdir().unwrap();
        let outside_dir = tempdir().unwrap();
        let outside_file = outside_dir.path().join("outside.txt");
        std::fs::write(&outside_file, "old").unwrap();
        std::os::unix::fs::symlink(&outside_file, workspace_dir.path().join("link.txt")).unwrap();

        let ws = Workspace::open(workspace_dir.path()).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result =
            rt.block_on(EditTool::new().execute(make_ctx(&ws), args("link.txt", "old", "new")));

        assert!(matches!(result, Err(ToolError::Denied(_))));
        assert_eq!(std::fs::read_to_string(outside_file).unwrap(), "old");
    }

    #[test]
    fn edit_schema_uses_camel_case_fields() {
        let schema = schemars::schema_for!(EditArgs);
        let value = serde_json::to_value(schema).unwrap();
        let properties = value["properties"].as_object().unwrap();
        assert!(properties.contains_key("oldText"));
        assert!(properties.contains_key("newText"));
        assert!(!properties.contains_key("old_text"));
        assert!(!properties.contains_key("new_text"));
    }
}
