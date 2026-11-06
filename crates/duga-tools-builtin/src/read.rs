//! ReadTool — read a file from the workspace with offset, limit, and binary detection.

use duga_tools::Tool;
use duga_tools::context::ToolContext;
use duga_tools::result::ToolCallResult;
use duga_types::error::ToolError;
use duga_types::tool_call::CallId;
use duga_types::tool_result::ToolResult;
use schemars::JsonSchema;
use serde::Deserialize;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ReadArgs {
    #[schemars(
        description = "Brief human-readable description of what this step does (shown to user)"
    )]
    pub label: String,
    pub path: String,
    #[serde(default)]
    pub offset: Option<u64>,
    #[serde(default)]
    pub limit: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct ReadTool {
    /// Optional secondary root directories for reading files outside the workspace
    /// (e.g., the bot's data directory for session logs and channel data).
    aux_roots: Vec<PathBuf>,
}

impl ReadTool {
    pub fn new() -> Self {
        Self {
            aux_roots: Vec::new(),
        }
    }

    /// Set secondary allowed roots for file reads (e.g., the Telegram data dir).
    pub fn with_aux_roots(mut self, paths: Vec<PathBuf>) -> Self {
        self.aux_roots = paths;
        self
    }

    #[cfg(test)]
    fn resolve_path(
        &self,
        ctx: &ToolContext<'_>,
        path: &str,
    ) -> Result<(PathBuf, bool), ToolError> {
        ctx.workspace
            .open_allowed_file(Path::new(path), &self.aux_roots)
            .map(|(relative, is_aux, _, _)| (relative, is_aux))
            .map_err(|e| {
                ToolError::Denied(format!("path not in workspace or auxiliary roots: {e}"))
            })
    }
}

impl Tool for ReadTool {
    type Args = ReadArgs;
    fn name(&self) -> &str {
        "read"
    }
    fn description(&self) -> &str {
        "Read a file from the workspace"
    }

    async fn execute(&self, ctx: ToolContext<'_>, args: Self::Args) -> ToolCallResult {
        let start = std::time::Instant::now();
        {
            let (_, _, _, mut file) = ctx
                .workspace
                .open_allowed_file(Path::new(&args.path), &self.aux_roots)
                .map_err(|e| ToolError::Denied(e.to_string()))?;
            let offset = args.offset.unwrap_or(0);
            let limit = args.limit.unwrap_or(256 * 1024).min(4 * 1024 * 1024);

            if offset > 0 {
                file.seek(std::io::SeekFrom::Start(offset))
                    .map_err(ToolError::from)?;
            }

            let detect_limit = 8192usize.min(limit as usize);
            let mut detect_buf = vec![0u8; detect_limit];
            let n = file.read(&mut detect_buf).map_err(ToolError::from)?;
            detect_buf.truncate(n);

            if detect_buf.contains(&0x00) {
                let hex = detect_buf
                    .iter()
                    .take(64)
                    .map(|b| format!("{:02x}", b))
                    .collect::<Vec<_>>()
                    .join(" ");
                return Ok(ToolResult {
                    tool_call_id: CallId::new(),
                    success: true,
                    output: format!("<binary file: {} bytes, first 64 bytes hex: {}>", n, hex),
                    steering_hint: None,
                    metadata: serde_json::json!({"is_binary": true, "byte_count": n}),
                    duration_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
                    stdout_bytes: 0,
                    stderr_bytes: 0,
                    truncated: false,
                });
            }

            let mut content = Vec::new();
            content.extend_from_slice(&detect_buf);
            let remaining = limit.saturating_sub(n as u64);
            if remaining > 0 {
                let mut buf = vec![0u8; remaining as usize];
                let m = file.read(&mut buf).map_err(ToolError::from)?;
                buf.truncate(m);
                content.extend_from_slice(&buf[..m]);
            }

            let output = String::from_utf8_lossy(&content).to_string();
            Ok(ToolResult {
                tool_call_id: CallId::new(),
                success: true,
                output,
                metadata: serde_json::json!({"is_binary": false, "bytes_read": content.len()}),
                duration_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
                steering_hint: None,
                stdout_bytes: content.len() as u64,
                stderr_bytes: 0,
                truncated: false,
            })
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
    fn test_read_basic() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello world").unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt
            .block_on(ReadTool::new().execute(
                make_ctx(&ws),
                ReadArgs {
                    label: "Reading test.txt".into(),
                    path: "test.txt".into(),
                    offset: None,
                    limit: None,
                },
            ))
            .unwrap();
        assert_eq!(r.output, "hello world");
    }

    #[test]
    fn test_read_nonexistent() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt.block_on(ReadTool::new().execute(
            make_ctx(&ws),
            ReadArgs {
                label: "Reading no.txt".into(),
                path: "no.txt".into(),
                offset: None,
                limit: None,
            },
        ));
        assert!(r.is_err());
    }

    #[test]
    fn test_read_binary() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("b.bin"),
            vec![0x48, 0x65, 0x6c, 0x6c, 0x6f, 0x00],
        )
        .unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt
            .block_on(ReadTool::new().execute(
                make_ctx(&ws),
                ReadArgs {
                    label: "Reading b.bin".into(),
                    path: "b.bin".into(),
                    offset: None,
                    limit: None,
                },
            ))
            .unwrap();
        assert!(r.output.starts_with("<binary file:"));
    }

    // ── resolve_path tests ────────────────────────────────────────────

    fn make_ctx_with_aux(ws: &Workspace) -> ToolContext<'static> {
        let ws: &'static Workspace = unsafe { std::mem::transmute(ws) };
        ToolContext {
            workspace: ws,
            cancellation: CancellationToken::new(),
            event_sink: &NullSink,
            aux_root: None,
        }
    }

    #[test]
    fn test_resolve_relative_in_workspace() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("hello.txt"), b"hello").unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let ctx = make_ctx_with_aux(&ws);
        let tool = ReadTool::new();
        let (resolved, is_aux) = tool.resolve_path(&ctx, "hello.txt").unwrap();
        assert!(!is_aux);
        assert_eq!(resolved, PathBuf::from("hello.txt"));
    }

    #[test]
    fn test_resolve_relative_in_aux_root() {
        let dir = tempdir().unwrap();
        let aux = tempdir().unwrap();
        std::fs::write(aux.path().join("skills.md"), b"# Skills").unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let ctx = make_ctx_with_aux(&ws);
        let tool = ReadTool::new().with_aux_roots(vec![aux.path().to_path_buf()]);
        let (resolved, is_aux) = tool.resolve_path(&ctx, "skills.md").unwrap();
        assert!(is_aux);
        assert_eq!(resolved, PathBuf::from("skills.md"));
    }

    #[test]
    fn test_resolve_absolute_workspace_prefix_to_aux() {
        let dir = tempdir().unwrap();
        let aux = tempdir().unwrap();
        std::fs::write(aux.path().join("config.yaml"), b"key: val").unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let ctx = make_ctx_with_aux(&ws);
        let tool = ReadTool::new().with_aux_roots(vec![aux.path().to_path_buf()]);
        let (resolved, is_aux) = tool.resolve_path(&ctx, "/workspace/config.yaml").unwrap();
        assert!(is_aux);
        assert_eq!(resolved, PathBuf::from("config.yaml"));
    }

    #[test]
    fn test_resolve_absolute_workspace_prefix_to_workspace() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("data.csv"), b"a,b,c").unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let ctx = make_ctx_with_aux(&ws);
        let tool = ReadTool::new();
        let (resolved, is_aux) = tool.resolve_path(&ctx, "/workspace/data.csv").unwrap();
        assert!(!is_aux);
        assert_eq!(resolved, PathBuf::from("data.csv"));
    }

    #[test]
    fn test_resolve_not_found() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let ctx = make_ctx_with_aux(&ws);
        let tool = ReadTool::new();
        let err = tool.resolve_path(&ctx, "missing.txt").unwrap_err();
        assert!(err.to_string().contains("not in workspace"));
    }

    #[test]
    fn test_resolve_workspace_priority_over_aux() {
        let dir = tempdir().unwrap();
        let aux = tempdir().unwrap();
        std::fs::write(dir.path().join("shared.md"), b"workspace").unwrap();
        std::fs::write(aux.path().join("shared.md"), b"aux").unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let ctx = make_ctx_with_aux(&ws);
        let tool = ReadTool::new().with_aux_roots(vec![aux.path().to_path_buf()]);
        let (resolved, is_aux) = tool.resolve_path(&ctx, "shared.md").unwrap();
        assert!(!is_aux, "workspace should have priority");
        assert_eq!(resolved, PathBuf::from("shared.md"));
    }

    #[test]
    fn test_resolve_absolute_not_found() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let ctx = make_ctx_with_aux(&ws);
        let tool = ReadTool::new();
        let err = tool.resolve_path(&ctx, "/workspace/ghost.txt").unwrap_err();
        assert!(err.to_string().contains("not in workspace"));
    }

    #[test]
    fn test_resolve_absolute_canonicalize() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub").join("img.png"), b"png").unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let ctx = make_ctx_with_aux(&ws);
        let tool = ReadTool::new();
        let abs = dir
            .path()
            .join("sub")
            .join("img.png")
            .canonicalize()
            .unwrap();
        let (resolved, is_aux) = tool.resolve_path(&ctx, &abs.display().to_string()).unwrap();
        assert!(!is_aux);
        assert_eq!(resolved, PathBuf::from("sub/img.png"));
    }

    #[test]
    fn test_resolve_subdirectory_in_aux() {
        let dir = tempdir().unwrap();
        let aux = tempdir().unwrap();
        std::fs::create_dir_all(aux.path().join("skills").join("test")).unwrap();
        std::fs::write(
            aux.path().join("skills").join("test").join("SKILL.md"),
            b"# Skill",
        )
        .unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let ctx = make_ctx_with_aux(&ws);
        let tool = ReadTool::new().with_aux_roots(vec![aux.path().to_path_buf()]);
        let (resolved, is_aux) = tool.resolve_path(&ctx, "skills/test/SKILL.md").unwrap();
        assert!(is_aux);
        assert_eq!(resolved, PathBuf::from("skills/test/SKILL.md"));
    }
}
