//! install-skill tool — creates a skill directory with SKILL.md and optional auxiliary files.
//!
//! Generic / frontend-agnostic: receives `global_skills_dir` and `channel_skills_base`
//! at construction time. Validates name format, description, and prevents path escaping.

use duga_tools::context::ToolContext;
use duga_tools::result::ToolCallResult;
use duga_tools::Tool;
use duga_types::error::ToolError;
use duga_types::tool_call::CallId;
use duga_types::tool_result::ToolResult;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Arguments for the install-skill tool.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct InstallSkillArgs {
    #[schemars(
        description = "Brief human-readable description of what this step does (shown to user)"
    )]
    pub label: String,

    /// Where to install: "global" (shared across all chats) or "channel" (specific to one chat).
    #[schemars(description = "Target scope: 'global' or 'channel'")]
    pub target: SkillTarget,

    /// Required when target is "channel". The Telegram chat ID (as a string).
    #[schemars(description = "Chat ID when target is 'channel' (optional for global)")]
    pub channel_id: Option<String>,

    /// Skill name — lowercase letters, digits, hyphens only. Max 64 chars.
    #[schemars(description = "Skill name: [a-z0-9-], max 64 chars, no leading/trailing/double hyphens")]
    pub name: String,

    /// Short description of what the skill does. Required, max 1024 chars.
    #[schemars(description = "Short description of what the skill does (required, max 1024 chars)")]
    pub description: String,

    /// Body of the SKILL.md file (markdown after YAML frontmatter).
    #[schemars(description = "Body content for SKILL.md (markdown after YAML frontmatter)")]
    pub body: String,

    /// Optional auxiliary files to create alongside SKILL.md.
    #[schemars(description = "Optional auxiliary files (paths relative to skill directory)")]
    pub files: Option<Vec<SkillFile>>,
}

/// Target scope for skill installation.
#[derive(Debug, Clone, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillTarget {
    Global,
    Channel,
}

impl std::fmt::Display for SkillTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SkillTarget::Global => write!(f, "global"),
            SkillTarget::Channel => write!(f, "channel"),
        }
    }
}

/// An auxiliary file to create in the skill directory.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SkillFile {
    /// Relative path within the skill directory (e.g. "search.py", "utils/helpers.js").
    #[schemars(description = "Relative path within the skill directory")]
    pub path: String,

    /// File content.
    #[schemars(description = "File content")]
    pub content: String,
}

/// The install-skill tool.
///
/// Writes through the sandbox workspace: global skills go to `workspace/skills/<name>/`,
/// channel skills go to `workspace/<chat_id>/skills/<name>/`.
#[derive(Clone)]
pub struct InstallSkillTool;

impl std::fmt::Debug for InstallSkillTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InstallSkillTool").finish()
    }
}

impl InstallSkillTool {
    pub fn new(_global_skills_dir: PathBuf, _channel_skills_base: PathBuf) -> Self {
        // Paths are kept for API compatibility with ListSkillsTool but unused —
        // skills are written through the workspace, which owns the filesystem root.
        Self
    }
}

impl Tool for InstallSkillTool {
    type Args = InstallSkillArgs;

    fn name(&self) -> &str {
        "install-skill"
    }

    fn description(&self) -> &str {
        "Create or update a skill with a SKILL.md file and optional auxiliary files"
    }

    fn retryable(&self) -> bool {
        // Not safe to retry — may partially create files
        false
    }

    async fn execute(&self, ctx: ToolContext<'_>, args: Self::Args) -> ToolCallResult {
        let start = std::time::Instant::now();

        // 1. Validate name
        validate_skill_name(&args.name)?;

        // 2. Validate description
        validate_description(&args.description)?;

        // 3. channel_id is required for channel target
        if args.target == SkillTarget::Channel && args.channel_id.is_none() {
            return Err(ToolError::InvalidArgs(
                "channel_id is required when target is 'channel'".into(),
            ));
        }

        // 4. Build workspace-relative paths (skills live under workspace root)
        let md_rel = match args.target {
            SkillTarget::Global => format!("skills/{}/SKILL.md", args.name),
            SkillTarget::Channel => {
                let cid = args.channel_id.as_deref().unwrap();
                if cid.contains('/') || cid.contains('\\') || cid.contains("..") {
                    return Err(ToolError::InvalidArgs(
                        "channel_id must not contain path separators or '..'".into(),
                    ));
                }
                format!("{}/skills/{}/SKILL.md", cid, args.name)
            }
        };

        let resolved_md = ctx
            .workspace
            .resolve(&PathBuf::from(&md_rel))
            .map_err(|_| {
                ToolError::Denied(format!("path escapes workspace: {}", md_rel))
            })?;

        // 5. Build SKILL.md content
        let skill_md_content = build_skill_md(&args.name, &args.description, &args.body);

        // 6. Write SKILL.md through workspace (atomic tempfile + rename)
        if let Some(parent) = resolved_md.parent() {
            if parent != std::path::Path::new("") && parent != std::path::Path::new(".") {
                ctx.workspace
                    .root_dir()
                    .create_dir_all(parent)
                    .map_err(ToolError::from)?;
            }
        }

        let temp_path = PathBuf::from(format!(".duga-install-skill-{}.tmp", CallId::new()));
        {
            let mut temp = ctx
                .workspace
                .root_dir()
                .create(&temp_path)
                .map_err(ToolError::from)?;
            std::io::Write::write_all(&mut temp, skill_md_content.as_bytes())
                .map_err(ToolError::from)?;
            temp.sync_all().map_err(ToolError::from)?;
        }

        match ctx
            .workspace
            .root_dir()
            .rename(&temp_path, ctx.workspace.root_dir(), &resolved_md)
        {
            Ok(()) => {}
            Err(e) => {
                let _ = ctx.workspace.root_dir().remove_file(&temp_path);
                return Err(ToolError::Io(format!("write SKILL.md failed: {}", e)));
            }
        }

        // 7. Write auxiliary files
        let mut aux_count = 0;
        if let Some(ref files) = args.files {
            for file in files {
                validate_aux_path(&file.path)?;

                let aux_rel = match args.target {
                    SkillTarget::Global => format!("skills/{}/{}", args.name, file.path),
                    SkillTarget::Channel => {
                        let cid = args.channel_id.as_deref().unwrap_or("unknown");
                        format!("{}/skills/{}/{}", cid, args.name, file.path)
                    }
                };
                let resolved_aux = ctx.workspace.resolve(&PathBuf::from(&aux_rel)).map_err(|_| {
                    ToolError::Denied(format!("aux file path escapes workspace: {}", aux_rel))
                })?;

                if let Some(parent) = resolved_aux.parent() {
                    if parent != std::path::Path::new("") && parent != std::path::Path::new(".") {
                        ctx.workspace
                            .root_dir()
                            .create_dir_all(parent)
                            .map_err(ToolError::from)?;
                    }
                }

                let aux_temp = PathBuf::from(format!(".duga-install-skill-aux-{}.tmp", CallId::new()));
                {
                    let mut temp = ctx
                        .workspace
                        .root_dir()
                        .create(&aux_temp)
                        .map_err(ToolError::from)?;
                    std::io::Write::write_all(&mut temp, file.content.as_bytes())
                        .map_err(ToolError::from)?;
                    temp.sync_all().map_err(ToolError::from)?;
                }

                match ctx
                    .workspace
                    .root_dir()
                    .rename(&aux_temp, ctx.workspace.root_dir(), &resolved_aux)
                {
                    Ok(()) => aux_count += 1,
                    Err(e) => {
                        let _ = ctx.workspace.root_dir().remove_file(&aux_temp);
                        return Err(ToolError::Io(format!(
                            "write aux file '{}' failed: {}",
                            file.path, e
                        )));
                    }
                }
            }
        }

        let msg = if aux_count > 0 {
            format!(
                "Installed skill '{}' ({}) with {} auxiliary files to skills/{}/SKILL.md",
                args.name, args.target, aux_count, args.name,
            )
        } else {
            format!(
                "Installed skill '{}' ({}) to skills/{}/SKILL.md",
                args.name, args.target, args.name,
            )
        };

        Ok(ToolResult {
            tool_call_id: CallId::new(),
            success: true,
            output: msg,
            metadata: serde_json::json!({
                "name": args.name,
                "target": args.target,
                "aux_files": aux_count,
            }),
            duration_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
            stdout_bytes: 0,
            stderr_bytes: 0,
            truncated: false,
        })
    }
}

// ── Validation helpers ──────────────────────────────────────────────────────

/// Validate skill name: [a-z0-9-]+, ≤64 chars, no leading/trailing/double hyphens.
pub fn validate_skill_name(name: &str) -> Result<(), ToolError> {
    if name.is_empty() {
        return Err(ToolError::InvalidArgs("name is required".into()));
    }
    if name.len() > 64 {
        return Err(ToolError::InvalidArgs(format!(
            "name '{}' is too long ({} chars, max 64)",
            name,
            name.len()
        )));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err(ToolError::InvalidArgs(format!(
            "name '{}' must not start or end with a hyphen",
            name
        )));
    }
    if name.contains("--") {
        return Err(ToolError::InvalidArgs(format!(
            "name '{}' must not contain double hyphens",
            name
        )));
    }
    for (i, ch) in name.chars().enumerate() {
        if !ch.is_ascii_lowercase() && !ch.is_ascii_digit() && ch != '-' {
            if ch.is_ascii_uppercase() {
                return Err(ToolError::InvalidArgs(format!(
                    "name '{}' contains uppercase characters at position {} — use only lowercase [a-z]",
                    name, i
                )));
            }
            return Err(ToolError::InvalidArgs(format!(
                "name '{}' contains invalid character '{}' at position {} — use only [a-z0-9-]",
                name, ch, i
            )));
        }
    }
    Ok(())
}

/// Validate description: non-empty, ≤1024 chars.
fn validate_description(desc: &str) -> Result<(), ToolError> {
    let trimmed = desc.trim();
    if trimmed.is_empty() {
        return Err(ToolError::InvalidArgs("description is required".into()));
    }
    if trimmed.len() > 1024 {
        return Err(ToolError::InvalidArgs(format!(
            "description is too long ({} chars, max 1024)",
            trimmed.len()
        )));
    }
    Ok(())
}

/// Validate auxiliary file path: no `..`, no absolute paths, no empty segments.
fn validate_aux_path(path: &str) -> Result<(), ToolError> {
    if path.is_empty() {
        return Err(ToolError::InvalidArgs("auxiliary file path is empty".into()));
    }
    if path.contains("..") {
        return Err(ToolError::InvalidArgs(format!(
            "auxiliary file path '{}' contains '..' (not allowed)",
            path
        )));
    }
    if path.starts_with('/') || path.starts_with('\\') {
        return Err(ToolError::InvalidArgs(format!(
            "auxiliary file path '{}' must be relative, not absolute",
            path
        )));
    }
    if path.contains('\0') {
        return Err(ToolError::InvalidArgs("auxiliary file path contains null byte".into()));
    }
    Ok(())
}

/// Build SKILL.md content with YAML frontmatter.
fn build_skill_md(name: &str, description: &str, body: &str) -> String {
    // Escape any triple-dashes in body to avoid frontmatter confusion.
    // (Rust string replace)
    let body = body.replace("---", "- - -");
    format!(
        "---\nname: {name}\ndescription: {description}\n---\n\n{body}",
        name = name,
        description = description,
        body = body.trim(),
    )
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Name validation tests ──────────────────────────────────────────

    #[test]
    fn valid_name_lowercase() {
        assert!(validate_skill_name("code-review").is_ok());
        assert!(validate_skill_name("my-skill-123").is_ok());
        assert!(validate_skill_name("a").is_ok());
    }

    #[test]
    fn name_rejects_empty() {
        let err = validate_skill_name("").unwrap_err().to_string();
        assert!(err.contains("required"));
    }

    #[test]
    fn name_rejects_too_long() {
        let long = "a".repeat(65);
        let err = validate_skill_name(&long).unwrap_err().to_string();
        assert!(err.contains("too long"));
    }

    #[test]
    fn name_rejects_uppercase() {
        let err = validate_skill_name("MySkill").unwrap_err().to_string();
        assert!(err.contains("uppercase"));
    }

    #[test]
    fn name_rejects_special_chars() {
        let err = validate_skill_name("my_skill").unwrap_err().to_string();
        assert!(err.contains("invalid character"));
        let err = validate_skill_name("my.skill").unwrap_err().to_string();
        assert!(err.contains("invalid character"));
    }

    #[test]
    fn name_rejects_leading_hyphen() {
        let err = validate_skill_name("-start").unwrap_err().to_string();
        assert!(err.contains("start or end with a hyphen"));
    }

    #[test]
    fn name_rejects_trailing_hyphen() {
        let err = validate_skill_name("end-").unwrap_err().to_string();
        assert!(err.contains("start or end with a hyphen"));
    }

    #[test]
    fn name_rejects_double_hyphen() {
        let err = validate_skill_name("my--skill").unwrap_err().to_string();
        assert!(err.contains("double hyphens"));
    }

    // ── Description validation tests ────────────────────────────────────

    #[test]
    fn description_accepts_valid() {
        assert!(validate_description("Does things").is_ok());
        assert!(validate_description("  Has whitespace  ").is_ok());
    }

    #[test]
    fn description_rejects_empty() {
        let err = validate_description("").unwrap_err().to_string();
        assert!(err.contains("required"));
        let err = validate_description("   ").unwrap_err().to_string();
        assert!(err.contains("required"));
    }

    #[test]
    fn description_rejects_too_long() {
        let long = "x".repeat(1025);
        let err = validate_description(&long).unwrap_err().to_string();
        assert!(err.contains("too long"));
    }

    // ── Aux file path validation tests ──────────────────────────────────

    #[test]
    fn aux_path_accepts_valid() {
        assert!(validate_aux_path("search.py").is_ok());
        assert!(validate_aux_path("utils/helpers.js").is_ok());
        assert!(validate_aux_path("data/config.json").is_ok());
    }

    #[test]
    fn aux_path_rejects_empty() {
        let err = validate_aux_path("").unwrap_err().to_string();
        assert!(err.contains("empty"));
    }

    #[test]
    fn aux_path_rejects_parent_traversal() {
        let err = validate_aux_path("../escape.py").unwrap_err().to_string();
        assert!(err.contains(".."));
    }

    #[test]
    fn aux_path_rejects_absolute() {
        let err = validate_aux_path("/etc/passwd").unwrap_err().to_string();
        assert!(err.contains("relative"));
    }

    #[test]
    fn aux_path_rejects_null_byte() {
        let err = validate_aux_path("good\0bad.py").unwrap_err().to_string();
        assert!(err.contains("null"));
    }

    // ── build_skill_md tests ────────────────────────────────────────────

    #[test]
    fn build_skill_md_produces_valid_frontmatter() {
        let md = build_skill_md("my-skill", "Does things", "# Hello\n\nWorld");
        assert!(md.starts_with("---\nname: my-skill\ndescription: Does things\n---"));
        assert!(md.contains("# Hello"));
        assert!(md.contains("World"));
    }

    #[test]
    fn build_skill_md_escapes_triple_dashes_in_body() {
        let md = build_skill_md("test", "Test skill", "---\nbody\n---");
        // Triple dashes should be escaped/munged
        assert!(!md.contains("\n---\nbody\n---"));
        assert!(md.contains("- - -"));
    }

    // ── Integration test: execute via dispatcher ────────────────────────

    #[tokio::test]
    async fn install_skill_creates_md_file() {
        use duga_sandbox::{CancellationToken, Workspace};
        use duga_tools::dispatcher::ToolDispatcher;
        use duga_tools::erased::ErasedTool;
        use duga_tools::event_sink::NullSink;
        use duga_types::tool_call::ToolCall;
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(Workspace::open(dir.path()).unwrap());
        let dp = ToolDispatcher::new();

        let tool = InstallSkillTool::new(
            dir.path().join("skills"),
            dir.path().to_path_buf(),
        );
        dp.register_erased(ErasedTool::erase(tool)).unwrap();

        let call = ToolCall::new(
            "install-skill",
            serde_json::json!({
                "label": "Install test skill",
                "target": "global",
                "name": "test-skill",
                "description": "A test skill for testing",
                "body": "# Test Skill\n\nDo the test thing."
            }),
        );

        let result = dp
            .dispatch(&call, &ws, CancellationToken::new(), &NullSink)
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("Installed skill"));
        assert!(result.output.contains("test-skill"));

        // Verify file was created
        let skill_md = dir.path().join("skills/test-skill/SKILL.md");
        assert!(skill_md.exists());
        let content = std::fs::read_to_string(&skill_md).unwrap();
        assert!(content.contains("name: test-skill"));
        assert!(content.contains("description: A test skill for testing"));
        assert!(content.contains("# Test Skill"));
        assert!(content.contains("Do the test thing."));
    }

    #[tokio::test]
    async fn install_skill_with_channel_target() {
        use duga_sandbox::{CancellationToken, Workspace};
        use duga_tools::dispatcher::ToolDispatcher;
        use duga_tools::erased::ErasedTool;
        use duga_tools::event_sink::NullSink;
        use duga_types::tool_call::ToolCall;
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(Workspace::open(dir.path()).unwrap());
        let dp = ToolDispatcher::new();

        let tool = InstallSkillTool::new(
            dir.path().join("global-skills"),
            dir.path().to_path_buf(),
        );
        dp.register_erased(ErasedTool::erase(tool)).unwrap();

        let call = ToolCall::new(
            "install-skill",
            serde_json::json!({
                "label": "Install channel skill",
                "target": "channel",
                "channel_id": "12345",
                "name": "chat-skill",
                "description": "A channel-specific skill",
                "body": "# Chat\n\nDo chat things."
            }),
        );

        let result = dp
            .dispatch(&call, &ws, CancellationToken::new(), &NullSink)
            .await
            .unwrap();

        assert!(result.success);
        // Verify file at channel-specific path
        let skill_md = dir.path().join("12345/skills/chat-skill/SKILL.md");
        assert!(skill_md.exists());
        let content = std::fs::read_to_string(&skill_md).unwrap();
        assert!(content.contains("name: chat-skill"));
    }

    #[tokio::test]
    async fn install_skill_rejects_invalid_name() {
        use duga_sandbox::{CancellationToken, Workspace};
        use duga_tools::dispatcher::ToolDispatcher;
        use duga_tools::erased::ErasedTool;
        use duga_tools::event_sink::NullSink;
        use duga_types::tool_call::ToolCall;
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(Workspace::open(dir.path()).unwrap());
        let dp = ToolDispatcher::new();

        let tool = InstallSkillTool::new(
            dir.path().join("skills"),
            dir.path().to_path_buf(),
        );
        dp.register_erased(ErasedTool::erase(tool)).unwrap();

        let call = ToolCall::new(
            "install-skill",
            serde_json::json!({
                "label": "Bad install",
                "target": "global",
                "name": "BAD-Name",
                "description": "Test",
                "body": "body"
            }),
        );

        let err = dp
            .dispatch(&call, &ws, CancellationToken::new(), &NullSink)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("uppercase"));
    }

    #[tokio::test]
    async fn install_skill_rejects_empty_description() {
        use duga_sandbox::{CancellationToken, Workspace};
        use duga_tools::dispatcher::ToolDispatcher;
        use duga_tools::erased::ErasedTool;
        use duga_tools::event_sink::NullSink;
        use duga_types::tool_call::ToolCall;
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(Workspace::open(dir.path()).unwrap());
        let dp = ToolDispatcher::new();

        let tool = InstallSkillTool::new(
            dir.path().join("skills"),
            dir.path().to_path_buf(),
        );
        dp.register_erased(ErasedTool::erase(tool)).unwrap();

        let call = ToolCall::new(
            "install-skill",
            serde_json::json!({
                "label": "Bad install",
                "target": "global",
                "name": "test",
                "description": "",
                "body": "body"
            }),
        );

        let err = dp
            .dispatch(&call, &ws, CancellationToken::new(), &NullSink)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("description is required"));
    }

    #[tokio::test]
    async fn install_skill_rejects_aux_parent_path() {
        use duga_sandbox::{CancellationToken, Workspace};
        use duga_tools::dispatcher::ToolDispatcher;
        use duga_tools::erased::ErasedTool;
        use duga_tools::event_sink::NullSink;
        use duga_types::tool_call::ToolCall;
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(Workspace::open(dir.path()).unwrap());
        let dp = ToolDispatcher::new();

        let tool = InstallSkillTool::new(
            dir.path().join("skills"),
            dir.path().to_path_buf(),
        );
        dp.register_erased(ErasedTool::erase(tool)).unwrap();

        let call = ToolCall::new(
            "install-skill",
            serde_json::json!({
                "label": "Bad install",
                "target": "global",
                "name": "test",
                "description": "Test skill",
                "body": "body",
                "files": [{"path": "../escape.py", "content": "print('bad')"}]
            }),
        );

        let err = dp
            .dispatch(&call, &ws, CancellationToken::new(), &NullSink)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains(".."));
    }

    #[tokio::test]
    async fn install_skill_with_auxiliary_files() {
        use duga_sandbox::{CancellationToken, Workspace};
        use duga_tools::dispatcher::ToolDispatcher;
        use duga_tools::erased::ErasedTool;
        use duga_tools::event_sink::NullSink;
        use duga_types::tool_call::ToolCall;
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(Workspace::open(dir.path()).unwrap());
        let dp = ToolDispatcher::new();

        let tool = InstallSkillTool::new(
            dir.path().join("skills"),
            dir.path().to_path_buf(),
        );
        dp.register_erased(ErasedTool::erase(tool)).unwrap();

        let call = ToolCall::new(
            "install-skill",
            serde_json::json!({
                "label": "Install with helpers",
                "target": "global",
                "name": "scraper",
                "description": "A scraper tool",
                "body": "# Scraper\n\nScrape things.",
                "files": [
                    {"path": "scrape.py", "content": "import requests\nprint('scraping')"},
                    {"path": "README.md", "content": "# Scraper helper docs"}
                ]
            }),
        );

        let result = dp
            .dispatch(&call, &ws, CancellationToken::new(), &NullSink)
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("2 auxiliary files"));

        // Verify files exist
        assert!(dir.path().join("skills/scraper/SKILL.md").exists());
        assert!(dir.path().join("skills/scraper/scrape.py").exists());
        assert!(dir.path().join("skills/scraper/README.md").exists());

        let py_content = std::fs::read_to_string(dir.path().join("skills/scraper/scrape.py")).unwrap();
        assert!(py_content.contains("import requests"));
    }

    #[tokio::test]
    async fn install_skill_overwrites_existing() {
        use duga_sandbox::{CancellationToken, Workspace};
        use duga_tools::dispatcher::ToolDispatcher;
        use duga_tools::erased::ErasedTool;
        use duga_tools::event_sink::NullSink;
        use duga_types::tool_call::ToolCall;
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(Workspace::open(dir.path()).unwrap());
        let dp = ToolDispatcher::new();

        let tool = InstallSkillTool::new(
            dir.path().join("skills"),
            dir.path().to_path_buf(),
        );
        dp.register_erased(ErasedTool::erase(tool)).unwrap();

        // First install
        let call1 = ToolCall::new(
            "install-skill",
            serde_json::json!({
                "label": "Install v1",
                "target": "global",
                "name": "upgrade-test",
                "description": "Version 1",
                "body": "# V1\n\nOld body."
            }),
        );
        let r1 = dp
            .dispatch(&call1, &ws, CancellationToken::new(), &NullSink)
            .await
            .unwrap();
        assert!(r1.success);

        // Second install (overwrite)
        let call2 = ToolCall::new(
            "install-skill",
            serde_json::json!({
                "label": "Install v2",
                "target": "global",
                "name": "upgrade-test",
                "description": "Version 2",
                "body": "# V2\n\nNew body."
            }),
        );
        let r2 = dp
            .dispatch(&call2, &ws, CancellationToken::new(), &NullSink)
            .await
            .unwrap();
        assert!(r2.success);

        // Verify new content
        let content =
            std::fs::read_to_string(dir.path().join("skills/upgrade-test/SKILL.md")).unwrap();
        assert!(content.contains("Version 2"));
        assert!(content.contains("New body"));
        assert!(!content.contains("Version 1"));
    }

    #[tokio::test]
    async fn install_skill_rejects_channel_without_id() {
        use duga_sandbox::{CancellationToken, Workspace};
        use duga_tools::dispatcher::ToolDispatcher;
        use duga_tools::erased::ErasedTool;
        use duga_tools::event_sink::NullSink;
        use duga_types::tool_call::ToolCall;
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(Workspace::open(dir.path()).unwrap());
        let dp = ToolDispatcher::new();

        let tool = InstallSkillTool::new(
            dir.path().join("skills"),
            dir.path().to_path_buf(),
        );
        dp.register_erased(ErasedTool::erase(tool)).unwrap();

        let call = ToolCall::new(
            "install-skill",
            serde_json::json!({
                "label": "Bad channel install",
                "target": "channel",
                "name": "test",
                "description": "Test",
                "body": "body"
            }),
        );

        let err = dp
            .dispatch(&call, &ws, CancellationToken::new(), &NullSink)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("channel_id"));
    }
}
