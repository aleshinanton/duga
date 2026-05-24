//! ReadTool — read a file from the workspace with offset, limit, and binary detection.

use duga_tools::context::ToolContext;
use duga_tools::result::ToolCallResult;
use duga_tools::Tool;
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
        Self { aux_roots: Vec::new() }
    }

    /// Set secondary allowed roots for file reads (e.g., the Telegram data dir).
    pub fn with_aux_roots(mut self, paths: Vec<PathBuf>) -> Self {
        self.aux_roots = paths;
        self
    }

    /// Try to resolve a path against workspace first, then aux_roots.
    ///
    /// LLMs inside Docker sandbox often pass absolute paths like
    /// `/workspace/skills/...` — we strip known mount prefixes before resolving.
    fn resolve_path(&self, ctx: &ToolContext<'_>, path_str: &str) -> Result<(PathBuf, bool), ToolError> {
        let raw = PathBuf::from(path_str);

        // Helper: try workspace resolution, verify the entry exists.
        let try_workspace = |p: &PathBuf| -> Option<PathBuf> {
            ctx.workspace.resolve(p).ok().and_then(|resolved| {
                if ctx.workspace.root_dir().metadata(&resolved).is_ok() {
                    Some(resolved)
                } else {
                    None
                }
            })
        };

        // Helper: try aux_roots for a relative path (no parent traversal).
        let try_aux = |rel: &Path| -> Option<PathBuf> {
            if rel.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
                return None;
            }
            for aux in &self.aux_roots {
                let aux_path = aux.join(rel);
                if aux_path.is_file() || aux_path.is_dir() {
                    return Some(rel.to_path_buf());
                }
            }
            None
        };

        // Relative path: workspace first, then aux_roots.
        if !raw.is_absolute() {
            if let Some(resolved) = try_workspace(&raw) {
                return Ok((resolved, false));
            }
            if let Some(rel) = try_aux(&raw) {
                return Ok((rel, true));
            }
            return Err(ToolError::Denied(format!("path not in workspace or auxiliary roots: {}", path_str)));
        }

        // Absolute path: try multiple strategies.
        // 1. Strip workspace root prefix.
        if let Ok(relative) = raw.strip_prefix(ctx.workspace.root_path()) {
            let rel = PathBuf::from(relative);
            if let Some(resolved) = try_workspace(&rel) {
                return Ok((resolved, false));
            }
            if let Some(found) = try_aux(Path::new(relative)) {
                return Ok((found, true));
            }
        }

        // 2. Canonicalize then strip workspace root.
        if let Ok(canonical) = raw.canonicalize() {
            if let Ok(relative) = canonical.strip_prefix(ctx.workspace.root_path()) {
                let rel = PathBuf::from(relative);
                if let Some(resolved) = try_workspace(&rel) {
                    return Ok((resolved, false));
                }
                if let Some(found) = try_aux(Path::new(relative)) {
                    return Ok((found, true));
                }
            }
            // Check aux_roots with canonical path.
            for aux in &self.aux_roots {
                let aux_canonical = aux.canonicalize().unwrap_or_else(|_| aux.clone());
                if let Ok(relative) = canonical.strip_prefix(&aux_canonical) {
                    let aux_full = aux.join(Path::new(relative));
                    if aux_full.is_file() || aux_full.is_dir() {
                        return Ok((PathBuf::from(relative), true));
                    }
                }
            }
        }

        // 3. Strip known sandbox mount prefixes (e.g. /workspace/).
        const KNOWN_MOUNT_PREFIXES: &[&str] = &["/workspace/", "/workspace"];
        for prefix in KNOWN_MOUNT_PREFIXES {
            if let Ok(relative) = raw.strip_prefix(prefix) {
                let rel = PathBuf::from(relative);
                if let Some(resolved) = try_workspace(&rel) {
                    return Ok((resolved, false));
                }
                if let Some(found) = try_aux(Path::new(relative)) {
                    return Ok((found, true));
                }
            }
        }

        // 4. Try each aux_root with the original absolute path stripped.
        for aux in &self.aux_roots {
            let aux_canonical = aux.canonicalize().unwrap_or_else(|_| aux.clone());
            if let Ok(relative) = raw.strip_prefix(&aux_canonical) {
                let aux_full = aux.join(Path::new(relative));
                if aux_full.is_file() || aux_full.is_dir() {
                    return Ok((PathBuf::from(relative), true));
                }
            }
        }

        Err(ToolError::Denied(format!("path not in workspace or auxiliary roots: {}", path_str)))
    }

    /// Build a ToolResult from raw bytes (used for aux_root reads).
    fn build_read_result(&self, content: &[u8], start: std::time::Instant) -> ToolCallResult {
        if content.contains(&0x00) {
            let hex = content
                .iter()
                .take(64)
                .map(|b| format!("{:02x}", b))
                .collect::<Vec<_>>()
                .join(" ");
            return Ok(ToolResult {
                tool_call_id: CallId::new(),
                success: true,
                output: format!("<binary file: {} bytes, first 64 bytes hex: {}>", content.len(), hex),
                metadata: serde_json::json!({"is_binary": true, "byte_count": content.len()}),
                duration_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
                stdout_bytes: 0,
                stderr_bytes: 0,
                truncated: false,
            });
        }
        let output = String::from_utf8_lossy(content).to_string();
        Ok(ToolResult {
            tool_call_id: CallId::new(),
            success: true,
            output,
            metadata: serde_json::json!({"is_binary": false, "bytes_read": content.len()}),
            duration_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
            stdout_bytes: content.len() as u64,
            stderr_bytes: 0,
            truncated: false,
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
        let (resolved, is_aux) = self.resolve_path(&ctx, &args.path)?;

        if is_aux {
            // Read from aux_roots using std::fs — find which root has the file
            let aux_full = self.aux_roots.iter()
                .map(|r| r.join(&resolved))
                .find(|p| p.exists())
                .ok_or_else(|| ToolError::Denied(format!(
                    "path not found in auxiliary roots: {}", args.path
                )))?;
            let full_content = std::fs::read(&aux_full).map_err(|e| {
                ToolError::Io(format!("cannot read {}: {}", aux_full.display(), e))
            })?;
            let offset = args.offset.unwrap_or(0) as usize;
            let limit = args.limit.unwrap_or(256 * 1024).min(4 * 1024 * 1024) as usize;
            let slice = if offset >= full_content.len() {
                &[] as &[u8]
            } else {
                let end = (offset + limit).min(full_content.len());
                &full_content[offset..end]
            };
            self.build_read_result(slice, start)
        } else {
            let mut file = ctx
                .workspace
                .root_dir()
                .open(&resolved)
                .map_err(ToolError::from)?;
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
}
