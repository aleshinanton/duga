//! SearchTool — walkdir + regex in spawn_blocking.

use duga_tools::context::ToolContext;
use duga_tools::result::ToolCallResult;
use duga_tools::Tool;
use duga_types::error::ToolError;
use duga_types::tool_call::CallId;
use duga_types::tool_result::ToolResult;
use regex::Regex;
use schemars::JsonSchema;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SearchArgs {
    #[schemars(
        description = "Brief human-readable description of what this step does (shown to user)"
    )]
    pub label: String,
    pub query: String,
    pub path: Option<String>,
    pub literal: Option<bool>,
    pub max_results: Option<usize>,
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
        "Search for a pattern in workspace files"
    }

    async fn execute(&self, ctx: ToolContext<'_>, args: Self::Args) -> ToolCallResult {
        let start = std::time::Instant::now();
        let search_root = if let Some(p) = &args.path {
            ctx.workspace
                .resolve(&PathBuf::from(p))
                .map_err(|_| ToolError::Denied(format!("path escapes: {}", p)))?
        } else {
            PathBuf::from(".")
        };
        let full_root = ctx.workspace.root_path().join(&search_root);

        let pattern = if args.literal.unwrap_or(false) {
            Regex::new(&regex::escape(&args.query))
        } else {
            Regex::new(&args.query)
        }
        .map_err(|e| ToolError::InvalidArgs(format!("Invalid regex: {}", e)))?;

        let max = args.max_results.unwrap_or(200);
        let output = tokio::task::spawn_blocking(move || search_files(&full_root, &pattern, max))
            .await
            .map_err(|_| ToolError::Plugin("search panicked".into()))?;

        Ok(ToolResult {
            tool_call_id: CallId::new(),
            success: true,
            output,
            metadata: serde_json::json!({}),
            duration_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
            stdout_bytes: 0,
            stderr_bytes: 0,
            truncated: false,
        })
    }
}

fn search_files(root: &PathBuf, pattern: &Regex, max_results: usize) -> String {
    let mut results: Vec<String> = Vec::new();
    for entry in walkdir::WalkDir::new(root)
        .min_depth(1)
        .max_depth(50)
        .follow_links(false)
    {
        if results.len() >= max_results {
            break;
        }
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if !entry.file_type().is_file() {
            continue;
        }
        if path.components().any(|c| c.as_os_str() == ".git") {
            continue;
        }
        let content = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        for (i, line) in content.lines().enumerate() {
            if results.len() >= max_results {
                break;
            }
            if pattern.is_match(line) {
                results.push(format!("{}:{}: {}", path.display(), i + 1, line));
            }
        }
    }
    let mut out = results.join("\n");
    if out.is_empty() {
        out = "No matches found".into();
    }
    if results.len() >= max_results {
        out.push_str(&format!(
            "\n... (truncated, reached max_results of {})",
            max_results
        ));
    }
    out
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
                },
            ))
            .unwrap();
        assert!(r.output.contains("hello"));
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
            },
        ));
        assert!(r.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn test_search_skips_file_symlink() {
        let workspace_dir = tempdir().unwrap();
        let outside_dir = tempdir().unwrap();
        std::fs::write(outside_dir.path().join("secret.txt"), "needle").unwrap();
        std::os::unix::fs::symlink(
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
                    label: "Searching for needle".into(),
                    query: "needle".into(),
                    path: None,
                    literal: None,
                    max_results: None,
                },
            ))
            .unwrap();

        assert_eq!(r.output, "No matches found");
    }
}
