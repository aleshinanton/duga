//! Persistent memory loading from MEMORY.md files.
//!
//! All frontends (CLI, Telegram, TUI) use `load_persistent_memory`
//! to inject workspace-level and channel/session-level MEMORY.md content
//! into the system prompt.

use anyhow::Result;
use std::path::{Path, PathBuf};

/// Loaded persistent memory from MEMORY.md files.
#[derive(Clone, Debug)]
pub struct PersistentMemory {
    /// Content from the workspace root MEMORY.md (if present).
    pub workspace_memory: Option<String>,
    /// Content from the channel/session-specific MEMORY.md (if present).
    pub channel_memory: Option<String>,
    /// The canonical path used to load workspace memory.
    pub workspace_path: PathBuf,
}

/// Load persistent memory from a workspace directory and optional channel directory.
///
/// Reads `MEMORY.md` from:
/// 1. `<workspace>/MEMORY.md` — workspace-level memory.
/// 2. `<channel_dir>/MEMORY.md` — channel/session-level memory (overrides workspace).
pub fn load_persistent_memory(
    workspace: &Path,
    channel_dir: Option<&Path>,
) -> Result<PersistentMemory> {
    let workspace_path = workspace.join("MEMORY.md");
    let workspace_memory = if workspace_path.exists() {
        Some(std::fs::read_to_string(&workspace_path)?)
    } else {
        None
    };

    let channel_memory = channel_dir.and_then(|dir| {
        let path = dir.join("MEMORY.md");
        if path.exists() {
            std::fs::read_to_string(&path).ok()
        } else {
            None
        }
    });

    Ok(PersistentMemory {
        workspace_memory,
        channel_memory,
        workspace_path,
    })
}

/// Format persistent memory as a string for injection into the system prompt.
pub fn format_memory_for_prompt(memory: &PersistentMemory) -> String {
    let mut parts = Vec::new();

    if let Some(ref text) = memory.channel_memory {
        parts.push(format!(
            "## Session Memory\n{}",
            text
        ));
    }

    if let Some(ref text) = memory.workspace_memory {
        parts.push(format!(
            "## Workspace Memory\n{}",
            text
        ));
    }

    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn load_persistent_memory_workspace_only() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("MEMORY.md"), "# Project\n\nThis is a test project.")
            .unwrap();

        let memory = load_persistent_memory(dir.path(), None).unwrap();
        assert!(memory.workspace_memory.is_some());
        assert!(memory.channel_memory.is_none());
        assert!(memory
            .workspace_memory
            .unwrap()
            .contains("test project"));
    }

    #[test]
    fn load_persistent_memory_with_channel_override() {
        let ws = tempfile::tempdir().unwrap();
        let ch = tempfile::tempdir().unwrap();
        fs::write(ws.path().join("MEMORY.md"), "Workspace memory.").unwrap();
        fs::write(ch.path().join("MEMORY.md"), "Channel memory.").unwrap();

        let memory = load_persistent_memory(ws.path(), Some(ch.path())).unwrap();
        assert_eq!(memory.workspace_memory.as_deref(), Some("Workspace memory."));
        assert_eq!(memory.channel_memory.as_deref(), Some("Channel memory."));
    }

    #[test]
    fn format_memory_for_prompt_order() {
        let memory = PersistentMemory {
            workspace_memory: Some("Workspace".into()),
            channel_memory: Some("Channel".into()),
            workspace_path: "/tmp/MEMORY.md".into(),
        };

        let formatted = format_memory_for_prompt(&memory);
        // Channel memory must come first in the prompt.
        let ch_idx = formatted.find("Channel").unwrap();
        let ws_idx = formatted.find("Workspace").unwrap();
        assert!(ch_idx < ws_idx);
    }
}
