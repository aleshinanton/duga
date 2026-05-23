//! list-skills tool — re-reads from disk and returns a formatted list of installed skills.
//!
//! Generic / frontend-agnostic: receives `global_skills_dir` and `channel_skills_base`
//! at construction time. Performs its own lightweight metadata discovery (no `duga-runtime`
//! dependency to avoid a cycle).

use duga_tools::context::ToolContext;
use duga_tools::result::ToolCallResult;
use duga_tools::Tool;
use duga_types::error::ToolError;
use duga_types::tool_call::CallId;
use duga_types::tool_result::ToolResult;
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

/// Arguments for the list-skills tool.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ListSkillsArgs {
    #[schemars(
        description = "Brief human-readable description of what this step does (shown to user)"
    )]
    pub label: String,

    /// Optional chat/channel ID. When provided, channel-level skills are included
    /// alongside global skills. Channel skills shadow global ones on name collision.
    #[schemars(description = "Optional chat/channel ID to include channel-level skills")]
    pub channel_id: Option<String>,
}

// ── Lightweight skill index (duplicated from duga-runtime to avoid cycle) ───

#[derive(Debug, Clone)]
struct LiteIndex {
    name: String,
    description: Option<String>,
    source: LiteSource,
    requires: Option<LiteRequires>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LiteSource {
    Global,
    Channel,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct LiteRequires {
    bins: Option<Vec<String>>,
    env: Option<HashMap<String, String>>,
}

#[derive(Default, Deserialize)]
struct LiteFrontmatter {
    name: Option<String>,
    description: Option<String>,
    requires: Option<LiteRequires>,
}

/// Lightweight gate result.
#[derive(Debug)]
struct LiteGate {
    available: bool,
    missing_bins: Vec<String>,
    missing_env: Vec<String>,
}

/// The list-skills tool.
#[derive(Clone)]
pub struct ListSkillsTool {
    global_skills_dir: PathBuf,
    channel_skills_base: PathBuf,
}

impl std::fmt::Debug for ListSkillsTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ListSkillsTool")
            .field("global_skills_dir", &self.global_skills_dir)
            .field("channel_skills_base", &self.channel_skills_base)
            .finish()
    }
}

impl ListSkillsTool {
    /// Create a new list-skills tool.
    ///
    /// - `global_skills_dir`: where global skills live (e.g. `data/skills/`)
    /// - `channel_skills_base`: parent dir for channel skills (e.g. `data/`), channel skills in `<base>/<channel_id>/skills/`
    pub fn new(global_skills_dir: PathBuf, channel_skills_base: PathBuf) -> Self {
        Self {
            global_skills_dir,
            channel_skills_base,
        }
    }
}

impl Tool for ListSkillsTool {
    type Args = ListSkillsArgs;

    fn name(&self) -> &str {
        "list-skills"
    }

    fn description(&self) -> &str {
        "List installed skills with their descriptions and availability status"
    }

    fn retryable(&self) -> bool {
        true // Read-only, safe to retry
    }

    async fn execute(&self, _ctx: ToolContext<'_>, args: Self::Args) -> ToolCallResult {
        let start = std::time::Instant::now();

        // Resolve channel dir if channel_id provided
        let channel_dir = if let Some(ref cid) = args.channel_id {
            if cid.contains('/') || cid.contains('\\') || cid.contains("..") {
                return Err(ToolError::InvalidArgs(
                    "channel_id must not contain path separators or '..'".into(),
                ));
            }
            Some(self.channel_skills_base.join(cid).join("skills"))
        } else {
            None
        };

        // Re-read from disk — always fresh
        let indexes = discover_lite(&self.global_skills_dir, channel_dir.as_deref());

        if indexes.is_empty() {
            return Ok(ToolResult {
                tool_call_id: CallId::new(),
                success: true,
                output: "(no skills installed)".into(),
                metadata: serde_json::json!({"count": 0}),
                duration_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
                stdout_bytes: 0,
                stderr_bytes: 0,
                truncated: false,
            });
        }

        // Group by source
        let global_count = indexes.iter().filter(|i| i.source == LiteSource::Global).count();
        let channel_count = indexes.len() - global_count;

        let mut lines: Vec<String> = Vec::new();

        // Header
        if global_count > 0 && channel_count > 0 {
            lines.push(format!(
                "## Installed Skills ({} global, {} channel-specific)\n",
                global_count, channel_count
            ));
        } else if global_count > 0 {
            lines.push(format!(
                "## Installed Skills ({} global)\n",
                global_count
            ));
        } else {
            lines.push(format!(
                "## Installed Skills ({} channel-specific)\n",
                channel_count
            ));
        }

        // List skills
        for idx in &indexes {
            let gate = gate_lite(idx);
            let status = if gate.available {
                "✅".to_string()
            } else {
                let mut missing = Vec::new();
                if !gate.missing_bins.is_empty() {
                    missing.push(format!("missing: {}", gate.missing_bins.join(", ")));
                }
                if !gate.missing_env.is_empty() {
                    missing.push(format!("missing env: {}", gate.missing_env.join(", ")));
                }
                format!("⚠️ ({})", missing.join("; "))
            };

            let source_label = if idx.source == LiteSource::Channel {
                " [channel]"
            } else {
                ""
            };

            let desc = idx
                .description
                .as_deref()
                .unwrap_or("(no description)");

            lines.push(format!("- {status} **{}**{} — {}", idx.name, source_label, desc));
        }

        lines.push(String::new());
        lines.push(
            "Use `read skills/<name>/SKILL.md` to load full instructions for any skill."
                .into(),
        );

        Ok(ToolResult {
            tool_call_id: CallId::new(),
            success: true,
            output: lines.join("\n"),
            metadata: serde_json::json!({
                "count": indexes.len(),
                "global_count": global_count,
                "channel_count": channel_count,
            }),
            duration_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
            stdout_bytes: 0,
            stderr_bytes: 0,
            truncated: false,
        })
    }
}

// ── Inline discovery/gating (lightweight copy of duga-runtime logic) ────────

fn discover_lite(global_dir: &std::path::Path, channel_dir: Option<&std::path::Path>) -> Vec<LiteIndex> {
    let mut map: HashMap<String, LiteIndex> = HashMap::new();

    // Global first
    for idx in scan_dir(global_dir, LiteSource::Global) {
        map.insert(idx.name.clone(), idx);
    }

    // Channel overrides
    if let Some(ch_dir) = channel_dir {
        for idx in scan_dir(ch_dir, LiteSource::Channel) {
            map.insert(idx.name.clone(), idx);
        }
    }

    map.into_values().collect()
}

fn scan_dir(dir: &std::path::Path, source: LiteSource) -> Vec<LiteIndex> {
    let mut indexes = Vec::new();
    if !dir.exists() || !dir.is_dir() {
        return indexes;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return indexes,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let skill_md = path.join("SKILL.md");
        if !skill_md.exists() {
            continue;
        }
        let raw = match std::fs::read_to_string(&skill_md) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let (fm, _body) = extract_frontmatter_lite(&raw);
        let name = fm
            .name
            .unwrap_or_else(|| path.file_name().unwrap().to_string_lossy().to_string());

        indexes.push(LiteIndex {
            name,
            description: fm.description,
            source: source.clone(),
            requires: fm.requires,
        });
    }
    indexes
}

fn extract_frontmatter_lite(raw: &str) -> (LiteFrontmatter, String) {
    let trimmed = raw.trim_start();
    if !trimmed.starts_with("---") {
        return (LiteFrontmatter::default(), raw.to_string());
    }
    let without_opening = &trimmed[3..];
    if let Some(end_idx) = without_opening.find("\n---") {
        let yaml_str = &without_opening[..end_idx];
        let body = without_opening[end_idx + 4..].trim_start().to_string();
        let frontmatter: LiteFrontmatter =
            serde_yaml::from_str(yaml_str).unwrap_or_default();
        (frontmatter, body)
    } else {
        (LiteFrontmatter::default(), raw.to_string())
    }
}

fn gate_lite(idx: &LiteIndex) -> LiteGate {
    let Some(ref requires) = idx.requires else {
        return LiteGate {
            available: true,
            missing_bins: Vec::new(),
            missing_env: Vec::new(),
        };
    };

    let mut missing_bins = Vec::new();
    let mut missing_env = Vec::new();

    if let Some(ref bins) = requires.bins {
        for bin in bins {
            if which::which(bin).is_err() {
                missing_bins.push(bin.clone());
            }
        }
    }

    if let Some(ref env_vars) = requires.env {
        for (key, template) in env_vars {
            let var_name = extract_env_var_name(template);
            match std::env::var(&var_name) {
                Ok(val) if !val.is_empty() => { /* OK */ }
                _ => missing_env.push(key.clone()),
            }
        }
    }

    LiteGate {
        available: missing_bins.is_empty() && missing_env.is_empty(),
        missing_bins,
        missing_env,
    }
}

fn extract_env_var_name(template: &str) -> String {
    let s = template.trim();
    if s.starts_with("${") && s.ends_with('}') {
        let inner = &s[2..s.len() - 1];
        if let Some(colon_idx) = inner.find(":-") {
            inner[..colon_idx].to_string()
        } else {
            inner.to_string()
        }
    } else {
        s.to_string()
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn list_skills_empty_directory() {
        use duga_sandbox::{CancellationToken, Workspace};
        use duga_tools::dispatcher::ToolDispatcher;
        use duga_tools::erased::ErasedTool;
        use duga_tools::event_sink::NullSink;
        use duga_types::tool_call::ToolCall;
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(Workspace::open(dir.path()).unwrap());
        let dp = ToolDispatcher::new();

        let tool = ListSkillsTool::new(
            dir.path().join("skills"),
            dir.path().to_path_buf(),
        );
        dp.register_erased(ErasedTool::erase(tool)).unwrap();

        let call = ToolCall::new(
            "list-skills",
            serde_json::json!({"label": "List skills"}),
        );

        let result = dp
            .dispatch(&call, &ws, CancellationToken::new(), &NullSink)
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("no skills installed"));
    }

    #[tokio::test]
    async fn list_skills_with_installed() {
        use duga_sandbox::{CancellationToken, Workspace};
        use duga_tools::dispatcher::ToolDispatcher;
        use duga_tools::erased::ErasedTool;
        use duga_tools::event_sink::NullSink;
        use duga_types::tool_call::ToolCall;
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(Workspace::open(dir.path()).unwrap());

        // Pre-create some skill directories
        let skills_dir = dir.path().join("skills");
        std::fs::create_dir_all(skills_dir.join("code-review")).unwrap();
        std::fs::write(
            skills_dir.join("code-review").join("SKILL.md"),
            "---\nname: code-review\ndescription: Review code changes\n---\n\n# Review",
        )
        .unwrap();

        std::fs::create_dir_all(skills_dir.join("vinted-scraper")).unwrap();
        std::fs::write(
            skills_dir.join("vinted-scraper").join("SKILL.md"),
            "---\nname: vinted-scraper\ndescription: Search Vinted listings\n---\n\n# Scraper",
        )
        .unwrap();

        let dp = ToolDispatcher::new();
        let tool = ListSkillsTool::new(skills_dir.clone(), dir.path().to_path_buf());
        dp.register_erased(ErasedTool::erase(tool)).unwrap();

        let call = ToolCall::new(
            "list-skills",
            serde_json::json!({"label": "List skills"}),
        );

        let result = dp
            .dispatch(&call, &ws, CancellationToken::new(), &NullSink)
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("code-review"));
        assert!(result.output.contains("Review code changes"));
        assert!(result.output.contains("vinted-scraper"));
        assert!(result.output.contains("Search Vinted listings"));
        assert!(result.output.contains("2 global"));
        assert!(result.output.contains("read skills/"));
    }

    #[tokio::test]
    async fn list_skills_with_channel_id() {
        use duga_sandbox::{CancellationToken, Workspace};
        use duga_tools::dispatcher::ToolDispatcher;
        use duga_tools::erased::ErasedTool;
        use duga_tools::event_sink::NullSink;
        use duga_types::tool_call::ToolCall;
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(Workspace::open(dir.path()).unwrap());

        let global_dir = dir.path().join("skills");
        std::fs::create_dir_all(global_dir.join("global-skill")).unwrap();
        std::fs::write(
            global_dir.join("global-skill").join("SKILL.md"),
            "---\nname: global-skill\ndescription: A global skill\n---\n\n# Global",
        )
        .unwrap();

        let ch_dir = dir.path().join("12345").join("skills").join("global-skill");
        std::fs::create_dir_all(&ch_dir).unwrap();
        std::fs::write(
            ch_dir.join("SKILL.md"),
            "---\nname: global-skill\ndescription: Channel override\n---\n\n# Channel",
        )
        .unwrap();

        let dp = ToolDispatcher::new();
        let tool = ListSkillsTool::new(global_dir.clone(), dir.path().to_path_buf());
        dp.register_erased(ErasedTool::erase(tool)).unwrap();

        let call = ToolCall::new(
            "list-skills",
            serde_json::json!({"label": "List skills", "channel_id": "12345"}),
        );

        let result = dp
            .dispatch(&call, &ws, CancellationToken::new(), &NullSink)
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("Channel override"));
        assert!(result.output.contains("[channel]"));
    }

    #[tokio::test]
    async fn list_skills_shows_unavailable_markers() {
        use duga_sandbox::{CancellationToken, Workspace};
        use duga_tools::dispatcher::ToolDispatcher;
        use duga_tools::erased::ErasedTool;
        use duga_tools::event_sink::NullSink;
        use duga_types::tool_call::ToolCall;
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(Workspace::open(dir.path()).unwrap());

        let skills_dir = dir.path().join("skills");
        std::fs::create_dir_all(skills_dir.join("scraper")).unwrap();
        std::fs::write(
            skills_dir.join("scraper").join("SKILL.md"),
            r#"---
name: scraper
description: Scrape things
requires:
  bins: [nonexistent-bin-xyz-12345]
---

# Scraper
"#,
        )
        .unwrap();

        std::fs::create_dir_all(skills_dir.join("review")).unwrap();
        std::fs::write(
            skills_dir.join("review").join("SKILL.md"),
            "---\nname: review\ndescription: Review code\n---\n\n# Review",
        )
        .unwrap();

        let dp = ToolDispatcher::new();
        let tool = ListSkillsTool::new(skills_dir.clone(), dir.path().to_path_buf());
        dp.register_erased(ErasedTool::erase(tool)).unwrap();

        let call = ToolCall::new(
            "list-skills",
            serde_json::json!({"label": "List skills"}),
        );

        let result = dp
            .dispatch(&call, &ws, CancellationToken::new(), &NullSink)
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("✅"));
        assert!(result.output.contains("⚠️"));
        assert!(result.output.contains("nonexistent-bin-xyz-12345"));
    }

    #[tokio::test]
    async fn list_skills_rejects_bad_channel_id() {
        use duga_sandbox::{CancellationToken, Workspace};
        use duga_tools::dispatcher::ToolDispatcher;
        use duga_tools::erased::ErasedTool;
        use duga_tools::event_sink::NullSink;
        use duga_types::tool_call::ToolCall;
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(Workspace::open(dir.path()).unwrap());
        let dp = ToolDispatcher::new();

        let tool = ListSkillsTool::new(
            dir.path().join("skills"),
            dir.path().to_path_buf(),
        );
        dp.register_erased(ErasedTool::erase(tool)).unwrap();

        let call = ToolCall::new(
            "list-skills",
            serde_json::json!({"label": "List", "channel_id": "../escape"}),
        );

        let err = dp
            .dispatch(&call, &ws, CancellationToken::new(), &NullSink)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("channel_id"));
    }
}
