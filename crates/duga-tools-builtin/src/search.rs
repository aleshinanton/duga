//! SearchTool — powered by ripgrep's engine (`grep-regex` + `grep-searcher`).
//!
//! Uses `ignore` for .gitignore-aware traversal + `grep-searcher` for
//! memory-mapped I/O, binary detection, and context lines — same engine
//! as ripgrep itself.

use duga_sandbox::CancellationToken;
use duga_tools::Tool;
use duga_tools::context::ToolContext;
use duga_tools::result::ToolCallResult;
use duga_types::error::ToolError;
use duga_types::tool_call::CallId;
use duga_types::tool_result::ToolResult;
use grep_regex::RegexMatcher;
use grep_searcher::{
    BinaryDetection, Searcher, SearcherBuilder, Sink, SinkContext, SinkContextKind, SinkFinish,
    SinkMatch,
};
use ignore::WalkBuilder;
use schemars::JsonSchema;
use serde::Deserialize;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SearchArgs {
    #[schemars(
        description = "Brief human-readable description of what this step does (shown to user)"
    )]
    pub label: String,

    /// Search pattern (regex by default, literal if literal=true).
    pub query: String,

    /// Optional sub-path within the workspace to restrict search.
    pub path: Option<String>,

    /// Treat query as a literal string (no regex escaping).
    pub literal: Option<bool>,

    /// Maximum number of match lines to return.
    pub max_results: Option<usize>,

    /// Number of context lines to show before and after each match.
    pub context_lines: Option<usize>,

    /// Comma-separated list of file extensions to include (e.g. "rs,py,ts").
    pub extensions: Option<String>,

    /// Glob pattern to filter files (e.g. "**/tests/**").
    pub glob: Option<String>,

    /// Skip hidden files and directories (default: true).
    pub no_hidden: Option<bool>,

    /// Maximum file size in bytes to search (default: 10 MB).
    pub max_file_size: Option<usize>,

    /// Maximum search depth (default: 50).
    pub max_depth: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct SearchTool;

impl SearchTool {
    pub fn new() -> Self {
        Self
    }
}

impl Tool for SearchTool {
    type Args = SearchArgs;
    fn name(&self) -> &str {
        "search"
    }
    fn description(&self) -> &str {
        "Fast file search powered by ripgrep: .gitignore-aware, context lines, \
         extension/glob filtering, binary detection, memory-mapped I/O."
    }

    async fn execute(&self, ctx: ToolContext<'_>, args: Self::Args) -> ToolCallResult {
        let start = std::time::Instant::now();

        // Resolve search root (sandboxed)
        let search_root = if let Some(p) = &args.path {
            ctx.workspace
                .resolve(&PathBuf::from(p))
                .map_err(|_| ToolError::Denied(format!("path escapes: {}", p)))?
        } else {
            PathBuf::from(".")
        };
        let full_root = ctx.workspace.root_path().join(&search_root);

        if ctx.is_cancelled() {
            return Err(ToolError::Cancelled);
        }

        // Build ripgrep regex matcher
        let raw_pattern = if args.literal.unwrap_or(false) {
            regex::escape(&args.query)
        } else {
            args.query.clone()
        };
        let matcher = RegexMatcher::new(&raw_pattern)
            .map_err(|e| ToolError::InvalidArgs(format!("Invalid regex: {}", e)))?;

        // Parse filters
        let exts: Option<Vec<String>> = args.extensions.as_ref().map(|s| {
            s.split(',')
                .map(|e| e.trim().trim_start_matches('.').to_lowercase())
                .filter(|e| !e.is_empty())
                .collect()
        });
        let glob_pattern = args.glob.as_ref().and_then(|g| glob::Pattern::new(g).ok());
        let no_hidden = args.no_hidden.unwrap_or(true);
        let max_depth = args.max_depth.unwrap_or(50);
        let max_file_size = args.max_file_size.unwrap_or(10_485_760); // 10 MB default
        let context_lines = args.context_lines.unwrap_or(0);
        let max_results = args.max_results.unwrap_or(200);

        // Configure ripgrep searcher with memory-mapped I/O and binary detection
        let mut searcher = SearcherBuilder::new()
            .binary_detection(BinaryDetection::quit(b'\x00'))
            .line_number(true)
            .before_context(context_lines)
            .after_context(context_lines)
            .build();

        let cancellation = ctx.cancellation.clone();
        let count = Arc::new(AtomicUsize::new(0));
        let count_clone = count.clone();

        let output = tokio::task::spawn_blocking(move || {
            search_files(
                &full_root,
                &mut searcher,
                &matcher,
                max_results,
                context_lines,
                no_hidden,
                max_depth,
                max_file_size,
                exts.as_deref(),
                glob_pattern.as_ref(),
                &cancellation,
                &count_clone,
            )
        })
        .await
        .map_err(|_| ToolError::Plugin("search panicked".into()))??;

        let match_count = count.load(Ordering::Relaxed);
        let steering_hint = if match_count > 50 {
            Some(format!(
                "Search returned {match_count} results. Consider narrowing with specific terms, \
                 extensions (e.g. extensions=\"rs,py\"), or a sub-path."
            ))
        } else {
            None
        };

        Ok(ToolResult {
            tool_call_id: CallId::new(),
            success: true,
            steering_hint,
            output,
            metadata: serde_json::json!({
                "match_count": match_count,
                "duration_ms": start.elapsed().as_millis()
            }),
            duration_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
            stdout_bytes: 0,
            stderr_bytes: 0,
            truncated: match_count >= max_results,
        })
    }
}

/// Custom Sink that collects matches with context, respecting max_results and cancellation.
/// Results are pushed into a shared `Arc<Mutex<Vec<String>>>` so they accumulate across files.
struct SearchSink {
    results: Arc<Mutex<Vec<String>>>,
    path_display: String,
    max_results: usize,
    context_lines: usize,
    match_count: Arc<AtomicUsize>,
    cancellation: CancellationToken,
    pending_before: Vec<(u64, String)>, // context lines before the current match
}

impl Sink for SearchSink {
    type Error = io::Error;

    fn matched(&mut self, _searcher: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, io::Error> {
        if self.cancellation.is_cancelled() {
            return Err(io::Error::other("search cancelled"));
        }

        let line_num = mat.line_number().unwrap_or(0);
        let line_str = std::str::from_utf8(mat.bytes()).unwrap_or("<binary>");
        let line = line_str.trim_end_matches('\n').trim_end_matches('\r');

        self.match_count.fetch_add(1, Ordering::Relaxed);

        let result = if self.context_lines > 0 {
            let mut block = format!("{}:{line_num}:\n", self.path_display);
            for (ctx_ln, ctx_line) in self.pending_before.drain(..) {
                block.push_str(&format!(" {ctx_ln}: {ctx_line}\n"));
            }
            block.push_str(&format!(">{line_num}: {line}\n"));
            block
        } else {
            format!("{}:{line_num}: {line}", self.path_display)
        };

        let mut results = self.results.lock().unwrap();
        results.push(result);
        let len = results.len();

        Ok(len < self.max_results)
    }

    fn context(&mut self, _searcher: &Searcher, ctx: &SinkContext<'_>) -> Result<bool, io::Error> {
        if self.cancellation.is_cancelled() {
            return Err(io::Error::other("search cancelled"));
        }

        let line_num = ctx.line_number().unwrap_or(0);
        let line_str = std::str::from_utf8(ctx.bytes()).unwrap_or("<binary>");
        let line = line_str.trim_end_matches('\n').trim_end_matches('\r');

        match ctx.kind() {
            SinkContextKind::Before | SinkContextKind::Other => {
                self.pending_before.push((line_num, line.to_string()));
            }
            SinkContextKind::After => {
                let mut results = self.results.lock().unwrap();
                if let Some(last) = results.last_mut() {
                    last.push_str(&format!(" {line_num}: {line}\n"));
                }
            }
        }

        Ok(true)
    }

    fn context_break(&mut self, _searcher: &Searcher) -> Result<bool, io::Error> {
        self.pending_before.clear();
        Ok(true)
    }

    fn finish(&mut self, _searcher: &Searcher, _finish: &SinkFinish) -> Result<(), io::Error> {
        Ok(())
    }
}

fn search_files(
    root: &PathBuf,
    searcher: &mut Searcher,
    matcher: &RegexMatcher,
    max_results: usize,
    context_lines: usize,
    no_hidden: bool,
    max_depth: usize,
    max_file_size: usize,
    extensions: Option<&[String]>,
    glob_pattern: Option<&glob::Pattern>,
    cancellation: &CancellationToken,
    match_count: &Arc<AtomicUsize>,
) -> Result<String, ToolError> {
    let results: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let mut walk = WalkBuilder::new(root);
    walk.hidden(no_hidden);
    walk.git_ignore(true);
    walk.git_global(true);
    walk.git_exclude(true);
    walk.follow_links(false);
    walk.max_depth(Some(max_depth));
    walk.sort_by_file_name(|a, b| a.cmp(b));
    let walker = walk.build();

    for entry in walker {
        if cancellation.is_cancelled() {
            return Err(ToolError::Cancelled);
        }

        // Check if we already have enough results
        {
            let r = results.lock().unwrap();
            if r.len() >= max_results {
                break;
            }
        }

        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        let path = entry.path();
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }

        // Extension filter
        if let Some(exts) = extensions {
            let ext = path.extension().and_then(|e| e.to_str()).map(|s| s.to_lowercase());
            if !ext.as_ref().map_or(false, |e| exts.contains(e)) {
                continue;
            }
        }

        // Glob filter
        if let Some(glob) = glob_pattern {
            if !glob.matches_path(path) {
                continue;
            }
        }

        // File size check
        if let Ok(meta) = entry.metadata() {
            if meta.len() > max_file_size as u64 {
                continue;
            }
        }

        let rel_path = pathdisplay(path, root);
        let mut sink = SearchSink {
            results: results.clone(),
            path_display: rel_path,
            max_results,
            context_lines,
            match_count: match_count.clone(),
            cancellation: cancellation.clone(),
            pending_before: Vec::new(),
        };

        // ripgrep handles: reading, binary detection, memory-mapped I/O, context
        if let Err(e) = searcher.search_path(matcher, path, &mut sink) {
            if cancellation.is_cancelled() {
                return Err(ToolError::Cancelled);
            }
            tracing::warn!(?path, %e, "search_path failed");
        }
    }

    let all_results = results.lock().unwrap();
    let mut out = if all_results.is_empty() {
        "No matches found".to_string()
    } else {
        all_results.join("\n")
    };
    if all_results.len() >= max_results {
        out.push_str(&format!(
            "\n... (truncated, reached max_results of {})",
            max_results
        ));
    }
    Ok(out)
}

/// Strip the workspace root prefix from a path for cleaner output.
fn pathdisplay(path: &std::path::Path, root: &PathBuf) -> String {
    path.strip_prefix(root).unwrap_or(path).display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use duga_sandbox::Workspace;
    use duga_sandbox::exec::CancellationToken;
    use duga_tools::event_sink::NullSink;
    use tempfile::tempdir;

    fn make_ctx(ws: &Workspace) -> ToolContext<'static> {
        make_ctx_with_cancellation(ws, CancellationToken::new())
    }

    fn make_ctx_with_cancellation(
        ws: &Workspace,
        cancellation: CancellationToken,
    ) -> ToolContext<'static> {
        let ws: &'static Workspace = unsafe { std::mem::transmute(ws) };
        ToolContext {
            workspace: ws,
            cancellation,
            event_sink: &NullSink,
            aux_root: None,
        }
    }

    #[test]
    fn test_search_basic() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("t.txt"), "hello world\nfoo\nhello again").unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt
            .block_on(SearchTool::new().execute(
                make_ctx(&ws),
                SearchArgs {
                    label: "Searching for hello".into(),
                    query: "hello".into(),
                    path: None,
                    literal: None,
                    max_results: None,
                    context_lines: None,
                    extensions: None,
                    glob: None,
                    no_hidden: None,
                    max_file_size: None,
                    max_depth: None,
                },
            ))
            .unwrap();
        assert!(r.output.contains("t.txt:1: hello world"));
        assert!(r.output.contains("t.txt:3: hello again"));
    }

    #[test]
    fn test_search_invalid_regex() {
        let dir = tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt.block_on(SearchTool::new().execute(
            make_ctx(&ws),
            SearchArgs {
                label: "Searching for [bad".into(),
                query: "[bad".into(),
                path: None,
                literal: None,
                max_results: None,
                context_lines: None,
                extensions: None,
                glob: None,
                no_hidden: None,
                max_file_size: None,
                max_depth: None,
            },
        ));
        assert!(r.is_err());
    }

    #[test]
    fn test_search_literal() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("t.txt"), "foo.bar[0]").unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt
            .block_on(SearchTool::new().execute(
                make_ctx(&ws),
                SearchArgs {
                    label: "Literal search".into(),
                    query: "foo.bar[0]".into(),
                    path: None,
                    literal: Some(true),
                    max_results: None,
                    context_lines: None,
                    extensions: None,
                    glob: None,
                    no_hidden: None,
                    max_file_size: None,
                    max_depth: None,
                },
            ))
            .unwrap();
        assert!(r.output.contains("foo.bar[0]"));
    }

    #[test]
    fn test_search_context_lines() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("t.txt"),
            "line1\nline2\nneedle\nline4\nline5",
        )
        .unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt
            .block_on(SearchTool::new().execute(
                make_ctx(&ws),
                SearchArgs {
                    label: "Context search".into(),
                    query: "needle".into(),
                    path: None,
                    literal: None,
                    max_results: None,
                    context_lines: Some(1),
                    extensions: None,
                    glob: None,
                    no_hidden: None,
                    max_file_size: None,
                    max_depth: None,
                },
            ))
            .unwrap();
        assert!(r.output.contains("line2"));
        assert!(r.output.contains("line4"));
        assert!(r.output.contains(">3: needle"));
    }

    #[test]
    fn test_search_extension_filter() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.path().join("b.py"), "def main(): pass").unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt
            .block_on(SearchTool::new().execute(
                make_ctx(&ws),
                SearchArgs {
                    label: "Extension filter".into(),
                    query: "main".into(),
                    path: None,
                    literal: None,
                    max_results: None,
                    context_lines: None,
                    extensions: Some("py".into()),
                    glob: None,
                    no_hidden: None,
                    max_file_size: None,
                    max_depth: None,
                },
            ))
            .unwrap();
        assert!(r.output.contains("b.py"));
        assert!(!r.output.contains("a.rs"));
    }

    #[test]
    fn test_search_skips_binary() {
        let dir = tempdir().unwrap();
        let bin_data = [0x00u8, 0x01, 0x02, 0x48, 0x65, 0x6c, 0x6c, 0x6f]; // null bytes
        std::fs::write(dir.path().join("binary.bin"), bin_data).unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt
            .block_on(SearchTool::new().execute(
                make_ctx(&ws),
                SearchArgs {
                    label: "Binary skip".into(),
                    query: "Hello".into(),
                    path: None,
                    literal: None,
                    max_results: None,
                    context_lines: None,
                    extensions: None,
                    glob: None,
                    no_hidden: None,
                    max_file_size: None,
                    max_depth: None,
                },
            ))
            .unwrap();
        assert_eq!(r.output, "No matches found");
    }

    #[test]
    fn test_search_cancelled_before_start() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("t.txt"), "hello world").unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt.block_on(SearchTool::new().execute(
            make_ctx_with_cancellation(&ws, cancellation),
            SearchArgs {
                label: "Cancel test".into(),
                query: "hello".into(),
                path: None,
                literal: None,
                max_results: None,
                context_lines: None,
                extensions: None,
                glob: None,
                no_hidden: None,
                max_file_size: None,
                max_depth: None,
            },
        ));

        assert!(matches!(r, Err(ToolError::Cancelled)));
    }

    #[cfg(unix)]
    #[test]
    fn test_search_does_not_follow_symlinks() {
        use std::os::unix::fs::symlink;
        let workspace_dir = tempdir().unwrap();
        let outside_dir = tempdir().unwrap();
        std::fs::write(outside_dir.path().join("secret.txt"), "needle").unwrap();
        symlink(
            outside_dir.path().join("secret.txt"),
            workspace_dir.path().join("link.txt"),
        )
        .unwrap();

        let ws = Workspace::open(workspace_dir.path()).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt
            .block_on(SearchTool::new().execute(
                make_ctx(&ws),
                SearchArgs {
                    label: "Symlink test".into(),
                    query: "needle".into(),
                    path: None,
                    literal: None,
                    max_results: None,
                    context_lines: None,
                    extensions: None,
                    glob: None,
                    no_hidden: None,
                    max_file_size: None,
                    max_depth: None,
                },
            ))
            .unwrap();

        assert_eq!(r.output, "No matches found");
    }
}
