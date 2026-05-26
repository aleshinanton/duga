//! remove-skill tool — removes an installed skill directory (SKILL.md + auxiliary files).
//!
//! Generic / frontend-agnostic: receives `global_skills_dir` and `channel_skills_base`
//! at construction time. Validates name format and prevents path escaping.

use duga_tools::context::ToolContext;
use duga_tools::result::ToolCallResult;
use duga_tools::Tool;
use duga_types::error::ToolError;
use duga_types::tool_call::CallId;
use duga_types::tool_result::ToolResult;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Arguments for the remove-skill tool.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RemoveSkillArgs {
    #[schemars(
        description = "Brief human-readable description of what this step does (shown to user)"
    )]
    pub label: String,

    /// Which scope to remove from: "global" (shared across all chats) or "channel" (specific to one chat).
    #[schemars(description = "Target scope: 'global' or 'channel'")]
    pub target: SkillRemoveTarget,

    /// Required when target is "channel". The Telegram chat ID (as a string).
    #[schemars(description = "Chat ID when target is 'channel' (optional for global)")]
    pub channel_id: Option<String>,

    /// Skill name to remove. Must be an existing skill name.
    #[schemars(description = "Skill name to remove: [a-z0-9-], max 64 chars")]
    pub name: String,
}

/// Target scope for skill removal.
#[derive(Debug, Clone, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillRemoveTarget {
    Global,
    Channel,
}

impl std::fmt::Display for SkillRemoveTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SkillRemoveTarget::Global => write!(f, "global"),
            SkillRemoveTarget::Channel => write!(f, "channel"),
        }
    }
}

/// The remove-skill tool.
///
/// Removes the entire skill directory (SKILL.md + auxiliary files) from the workspace.
/// Paths are built the same way as install-skill for consistency.
#[derive(Clone)]
pub struct RemoveSkillTool;

impl std::fmt::Debug for RemoveSkillTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoveSkillTool").finish()
    }
}

impl RemoveSkillTool {
    pub fn new(_global_skills_dir: PathBuf, _channel_skills_base: PathBuf) -> Self {
        // Paths are kept for API compatibility with InstallSkillTool/ListSkillsTool but unused —
        // skills are removed through the workspace, which owns the filesystem root.
        Self
    }
}

impl Tool for RemoveSkillTool {
    type Args = RemoveSkillArgs;

    fn name(&self) -> &str {
        "remove-skill"
    }

    fn description(&self) -> &str {
        "Remove an installed skill (deletes SKILL.md and all auxiliary files)"
    }

    fn retryable(&self) -> bool {
        // Safe to retry — idempotent for already-removed skills
        true
    }

    async fn execute(&self, ctx: ToolContext<'_>, args: Self::Args) -> ToolCallResult {
        let start = std::time::Instant::now();

        // 1. Validate name
        super::skill_install::validate_skill_name(&args.name)?;

        // 2. channel_id is required for channel target
        if args.target == SkillRemoveTarget::Channel && args.channel_id.is_none() {
            return Err(ToolError::InvalidArgs(
                "channel_id is required when target is 'channel'".into(),
            ));
        }

        // 3. Build workspace-relative path to the skill directory
        let dir_rel = match args.target {
            SkillRemoveTarget::Global => format!("skills/{}", args.name),
            SkillRemoveTarget::Channel => {
                let cid = args.channel_id.as_deref().unwrap();
                if cid.contains('/') || cid.contains('\\') || cid.contains("..") {
                    return Err(ToolError::InvalidArgs(
                        "channel_id must not contain path separators or '..'".into(),
                    ));
                }
                format!("{}/skills/{}", cid, args.name)
            }
        };

        // 4. Check if the skill directory exists
        let resolved_dir = ctx.workspace.resolve(&PathBuf::from(&dir_rel)).map_err(|_| {
            ToolError::Denied(format!("path escapes workspace: {}", dir_rel))
        })?;

        if !ctx.workspace.is_dir(&resolved_dir) {
            return Ok(ToolResult {
                tool_call_id: CallId::new(),
                success: true,
                output: format!(
                    "Skill '{}' ({}) does not exist — nothing to remove.",
                    args.name, args.target
                ),
                steering_hint: None,
                metadata: serde_json::json!({
                    "name": args.name,
                    "target": args.target,
                    "removed": false,
                }),
                duration_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
                stdout_bytes: 0,
                stderr_bytes: 0,
                truncated: false,
            });
        }

        // 5. Remove the entire skill directory recursively
        ctx.workspace
            .remove_dir_all(&resolved_dir)
            .map_err(|e| ToolError::Io(format!("failed to remove skill '{}': {}", args.name, e)))?;

        let msg = format!(
            "Removed skill '{}' ({}) — directory skills/{} and all auxiliary files deleted.",
            args.name, args.target, args.name,
        );

        Ok(ToolResult {
            tool_call_id: CallId::new(),
            success: true,
            output: msg,
            steering_hint: None,
            metadata: serde_json::json!({
                "name": args.name,
                "target": args.target,
                "removed": true,
            }),
            duration_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
            stdout_bytes: 0,
            stderr_bytes: 0,
            truncated: false,
        })
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_install::InstallSkillTool;
    use duga_sandbox::{CancellationToken, Workspace};
    use duga_tools::dispatcher::ToolDispatcher;
    use duga_tools::erased::ErasedTool;
    use duga_tools::event_sink::NullSink;
    use duga_types::tool_call::ToolCall;
    use std::sync::Arc;

    /// Helper: install a skill through the dispatcher, verify success, return the workspace path.
    async fn install_test_skill(
        dp: &ToolDispatcher,
        ws: &Arc<Workspace>,
        name: &str,
        target: &str,
        channel_id: Option<&str>,
        body: &str,
        files: Option<serde_json::Value>,
    ) {
        let mut args = serde_json::json!({
            "label": format!("Install {}", name),
            "target": target,
            "name": name,
            "description": format!("{} description", name),
            "body": body,
        });
        if let Some(cid) = channel_id {
            args["channel_id"] = serde_json::Value::String(cid.to_string());
        }
        if let Some(f) = files {
            args["files"] = f;
        }

        let call = ToolCall::new("install-skill", args);
        let result = dp
            .dispatch(&call, ws, CancellationToken::new(), &NullSink)
            .await
            .unwrap();
        assert!(result.success, "install failed: {:?}", result);
    }

    /// Helper: remove a skill through the dispatcher, return the result.
    async fn remove_test_skill(
        dp: &ToolDispatcher,
        ws: &Arc<Workspace>,
        name: &str,
        target: &str,
        channel_id: Option<&str>,
    ) -> ToolResult {
        let mut args = serde_json::json!({
            "label": format!("Remove {}", name),
            "target": target,
            "name": name,
        });
        if let Some(cid) = channel_id {
            args["channel_id"] = serde_json::Value::String(cid.to_string());
        }

        let call = ToolCall::new("remove-skill", args);
        dp.dispatch(&call, ws, CancellationToken::new(), &NullSink)
            .await
            .unwrap()
    }

    // ── Basic remove tests ──────────────────────────────────────────────

    #[tokio::test]
    async fn remove_skill_deletes_directory_and_files() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(Workspace::open(dir.path()).unwrap());
        let dp = ToolDispatcher::new();

        dp.register_erased(ErasedTool::erase(InstallSkillTool::new(
            dir.path().join("skills"),
            dir.path().to_path_buf(),
        )))
        .unwrap();
        dp.register_erased(ErasedTool::erase(RemoveSkillTool::new(
            dir.path().join("skills"),
            dir.path().to_path_buf(),
        )))
        .unwrap();

        // Install a skill with auxiliary files
        install_test_skill(
            &dp, &ws, "my-skill", "global", None,
            "# My Skill\n\nDo stuff.",
            Some(serde_json::json!([
                {"path": "helper.py", "content": "print('hello')"},
                {"path": "README.md", "content": "# Helper docs"},
            ])),
        )
        .await;

        // Verify files exist
        let skill_dir = dir.path().join("skills/my-skill");
        assert!(skill_dir.exists());
        assert!(skill_dir.join("SKILL.md").exists());
        assert!(skill_dir.join("helper.py").exists());
        assert!(skill_dir.join("README.md").exists());

        // Remove the skill
        let result = remove_test_skill(&dp, &ws, "my-skill", "global", None).await;
        assert!(result.success);
        assert!(result.output.contains("Removed skill"));
        assert!(result.output.contains("my-skill"));

        // Verify directory is gone
        assert!(!skill_dir.exists());
    }

    #[tokio::test]
    async fn remove_skill_without_aux_files() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(Workspace::open(dir.path()).unwrap());
        let dp = ToolDispatcher::new();

        dp.register_erased(ErasedTool::erase(InstallSkillTool::new(
            dir.path().join("skills"),
            dir.path().to_path_buf(),
        )))
        .unwrap();
        dp.register_erased(ErasedTool::erase(RemoveSkillTool::new(
            dir.path().join("skills"),
            dir.path().to_path_buf(),
        )))
        .unwrap();

        // Install a minimal skill
        install_test_skill(
            &dp, &ws, "simple", "global", None,
            "# Simple\n\nJust a test.",
            None,
        )
        .await;

        assert!(dir.path().join("skills/simple/SKILL.md").exists());

        // Remove
        let result = remove_test_skill(&dp, &ws, "simple", "global", None).await;
        assert!(result.success);
        assert!(!dir.path().join("skills/simple").exists());
    }

    // ── Channel target tests ────────────────────────────────────────────

    #[tokio::test]
    async fn remove_channel_skill() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(Workspace::open(dir.path()).unwrap());
        let dp = ToolDispatcher::new();

        dp.register_erased(ErasedTool::erase(InstallSkillTool::new(
            dir.path().join("skills"),
            dir.path().to_path_buf(),
        )))
        .unwrap();
        dp.register_erased(ErasedTool::erase(RemoveSkillTool::new(
            dir.path().join("skills"),
            dir.path().to_path_buf(),
        )))
        .unwrap();

        // Install channel skill
        install_test_skill(
            &dp, &ws, "chat-tool", "channel", Some("12345"),
            "# Chat Tool\n\nChannel-specific.",
            None,
        )
        .await;

        assert!(dir.path().join("12345/skills/chat-tool/SKILL.md").exists());

        // Remove
        let result = remove_test_skill(&dp, &ws, "chat-tool", "channel", Some("12345")).await;
        assert!(result.success);
        assert!(result.output.contains("chat-tool"));
        assert!(!dir.path().join("12345/skills/chat-tool").exists());
    }

    #[tokio::test]
    async fn remove_channel_skill_does_not_affect_global() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(Workspace::open(dir.path()).unwrap());
        let dp = ToolDispatcher::new();

        dp.register_erased(ErasedTool::erase(InstallSkillTool::new(
            dir.path().join("skills"),
            dir.path().to_path_buf(),
        )))
        .unwrap();
        dp.register_erased(ErasedTool::erase(RemoveSkillTool::new(
            dir.path().join("skills"),
            dir.path().to_path_buf(),
        )))
        .unwrap();

        // Install global and channel skills with the same name
        install_test_skill(
            &dp, &ws, "shared", "global", None,
            "# Global version",
            None,
        )
        .await;
        install_test_skill(
            &dp, &ws, "shared", "channel", Some("99999"),
            "# Channel version",
            None,
        )
        .await;

        assert!(dir.path().join("skills/shared/SKILL.md").exists());
        assert!(dir.path().join("99999/skills/shared/SKILL.md").exists());

        // Remove only the channel version
        let result = remove_test_skill(&dp, &ws, "shared", "channel", Some("99999")).await;
        assert!(result.success);

        // Global should remain, channel should be gone
        assert!(dir.path().join("skills/shared/SKILL.md").exists());
        assert!(!dir.path().join("99999/skills/shared").exists());
    }

    // ── Idempotency tests ───────────────────────────────────────────────

    #[tokio::test]
    async fn remove_nonexistent_skill_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(Workspace::open(dir.path()).unwrap());
        let dp = ToolDispatcher::new();

        dp.register_erased(ErasedTool::erase(RemoveSkillTool::new(
            dir.path().join("skills"),
            dir.path().to_path_buf(),
        )))
        .unwrap();

        // Remove a skill that was never installed
        let result = remove_test_skill(&dp, &ws, "never-installed", "global", None).await;
        assert!(result.success);
        assert!(result.output.contains("does not exist"));
        assert!(result.output.contains("nothing to remove"));
    }

    #[tokio::test]
    async fn remove_twice_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(Workspace::open(dir.path()).unwrap());
        let dp = ToolDispatcher::new();

        dp.register_erased(ErasedTool::erase(InstallSkillTool::new(
            dir.path().join("skills"),
            dir.path().to_path_buf(),
        )))
        .unwrap();
        dp.register_erased(ErasedTool::erase(RemoveSkillTool::new(
            dir.path().join("skills"),
            dir.path().to_path_buf(),
        )))
        .unwrap();

        // Install
        install_test_skill(
            &dp, &ws, "redo", "global", None,
            "# Redo\n\nTest.",
            None,
        )
        .await;

        // First remove
        let r1 = remove_test_skill(&dp, &ws, "redo", "global", None).await;
        assert!(r1.success);
        assert!(r1.output.contains("Removed"));

        // Second remove — should report not found, not error
        let r2 = remove_test_skill(&dp, &ws, "redo", "global", None).await;
        assert!(r2.success);
        assert!(r2.output.contains("does not exist"));
    }

    // ── Validation tests ────────────────────────────────────────────────

    #[tokio::test]
    async fn remove_skill_rejects_invalid_name() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(Workspace::open(dir.path()).unwrap());
        let dp = ToolDispatcher::new();

        dp.register_erased(ErasedTool::erase(RemoveSkillTool::new(
            dir.path().join("skills"),
            dir.path().to_path_buf(),
        )))
        .unwrap();

        let call = ToolCall::new(
            "remove-skill",
            serde_json::json!({
                "label": "Bad remove",
                "target": "global",
                "name": "BAD-Name",
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
    async fn remove_skill_rejects_empty_name() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(Workspace::open(dir.path()).unwrap());
        let dp = ToolDispatcher::new();

        dp.register_erased(ErasedTool::erase(RemoveSkillTool::new(
            dir.path().join("skills"),
            dir.path().to_path_buf(),
        )))
        .unwrap();

        let call = ToolCall::new(
            "remove-skill",
            serde_json::json!({
                "label": "Empty name",
                "target": "global",
                "name": "",
            }),
        );

        let err = dp
            .dispatch(&call, &ws, CancellationToken::new(), &NullSink)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("required"));
    }

    #[tokio::test]
    async fn remove_skill_rejects_channel_without_id() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(Workspace::open(dir.path()).unwrap());
        let dp = ToolDispatcher::new();

        dp.register_erased(ErasedTool::erase(RemoveSkillTool::new(
            dir.path().join("skills"),
            dir.path().to_path_buf(),
        )))
        .unwrap();

        let call = ToolCall::new(
            "remove-skill",
            serde_json::json!({
                "label": "Bad channel remove",
                "target": "channel",
                "name": "test",
            }),
        );

        let err = dp
            .dispatch(&call, &ws, CancellationToken::new(), &NullSink)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("channel_id"));
    }

    #[tokio::test]
    async fn remove_skill_rejects_bad_channel_id() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(Workspace::open(dir.path()).unwrap());
        let dp = ToolDispatcher::new();

        dp.register_erased(ErasedTool::erase(RemoveSkillTool::new(
            dir.path().join("skills"),
            dir.path().to_path_buf(),
        )))
        .unwrap();

        let call = ToolCall::new(
            "remove-skill",
            serde_json::json!({
                "label": "Escape attempt",
                "target": "channel",
                "channel_id": "../evil",
                "name": "test",
            }),
        );

        let err = dp
            .dispatch(&call, &ws, CancellationToken::new(), &NullSink)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("channel_id"));
    }

    // ── Metadata test ───────────────────────────────────────────────────

    #[tokio::test]
    async fn remove_skill_metadata_reflects_removed_status() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(Workspace::open(dir.path()).unwrap());
        let dp = ToolDispatcher::new();

        dp.register_erased(ErasedTool::erase(InstallSkillTool::new(
            dir.path().join("skills"),
            dir.path().to_path_buf(),
        )))
        .unwrap();
        dp.register_erased(ErasedTool::erase(RemoveSkillTool::new(
            dir.path().join("skills"),
            dir.path().to_path_buf(),
        )))
        .unwrap();

        // Install
        install_test_skill(
            &dp, &ws, "meta-test", "global", None,
            "# Meta\n\nTest.",
            None,
        )
        .await;

        // Remove (should have removed: true)
        let r1 = remove_test_skill(&dp, &ws, "meta-test", "global", None).await;
        assert!(r1.success);
        assert_eq!(r1.metadata["removed"], true);

        // Remove again (should have removed: false)
        let r2 = remove_test_skill(&dp, &ws, "meta-test", "global", None).await;
        assert!(r2.success);
        assert_eq!(r2.metadata["removed"], false);
    }
}
