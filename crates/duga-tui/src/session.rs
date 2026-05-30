//! Session management for the TUI.
//!
//! Each session is stored as a JSONL file in the sessions directory.
//! A lightweight `sessions.json` index holds metadata (title, timestamps)
//! so the picker doesn't need to open every JSONL file.

use crate::transcript::{SystemLevel, TranscriptItem};
use duga_events::{Event, StoredEvent};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::Instant;

// ── Session metadata ──────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    /// First user message, truncated to 60 chars.
    pub title: String,
    /// RFC3339 creation timestamp.
    pub created_at: String,
    /// RFC3339 last-active timestamp.
    pub last_active: String,
}

impl SessionInfo {
    pub fn new(id: String, title: String) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self { id, title, created_at: now.clone(), last_active: now }
    }

    pub fn path(&self, sessions_dir: &Path) -> PathBuf {
        sessions_dir.join(format!("{}.jsonl", self.id))
    }

    /// Human-readable relative time for the picker list.
    pub fn relative_time(&self) -> String {
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&self.last_active) {
            let secs = (chrono::Utc::now() - dt.with_timezone(&chrono::Utc))
                .num_seconds()
                .max(0) as u64;
            match secs {
                0..=59 => "just now".into(),
                60..=3599 => format!("{}m ago", secs / 60),
                3600..=86399 => format!("{}h ago", secs / 3600),
                s => format!("{}d ago", s / 86400),
            }
        } else {
            self.last_active.clone()
        }
    }
}

// ── Index file ────────────────────────────────────────────────────────────

fn index_path(sessions_dir: &Path) -> PathBuf {
    sessions_dir.join("sessions.json")
}

/// Load all sessions from the index, sorted newest-first.
pub fn list_sessions(sessions_dir: &Path) -> Vec<SessionInfo> {
    let raw = std::fs::read_to_string(index_path(sessions_dir)).unwrap_or_default();
    let mut sessions: Vec<SessionInfo> = serde_json::from_str(&raw).unwrap_or_default();
    // Newest last_active first.
    sessions.sort_by(|a, b| b.last_active.cmp(&a.last_active));
    sessions
}

/// Create or update one session entry in the index.
pub fn upsert_session_index(sessions_dir: &Path, info: &SessionInfo) {
    let raw = std::fs::read_to_string(index_path(sessions_dir)).unwrap_or_default();
    let mut sessions: Vec<SessionInfo> = serde_json::from_str(&raw).unwrap_or_default();
    if let Some(existing) = sessions.iter_mut().find(|s| s.id == info.id) {
        *existing = info.clone();
    } else {
        sessions.push(info.clone());
    }
    if let Ok(json) = serde_json::to_string_pretty(&sessions) {
        let _ = std::fs::write(index_path(sessions_dir), json);
    }
}

/// Delete a session: removes the JSONL file and the index entry.
///
/// Returns `true` if the session was found and deleted.
pub fn delete_session(sessions_dir: &Path, id: &str) -> std::io::Result<bool> {
    let path = sessions_dir.join(format!("{id}.jsonl"));
    let existed = path.exists();
    if existed {
        std::fs::remove_file(&path)?;
    }

    // Remove from index
    let raw = std::fs::read_to_string(index_path(sessions_dir)).unwrap_or_default();
    let mut sessions: Vec<SessionInfo> = serde_json::from_str(&raw).unwrap_or_default();
    let before = sessions.len();
    sessions.retain(|s| s.id != id);
    let removed_from_index = sessions.len() < before;

    let json = serde_json::to_string_pretty(&sessions)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(index_path(sessions_dir), json)?;

    Ok(existed || removed_from_index)
}

/// Update the `last_active` timestamp of a session.
pub fn touch_session(sessions_dir: &Path, id: &str) {
    let raw = std::fs::read_to_string(index_path(sessions_dir)).unwrap_or_default();
    let mut sessions: Vec<SessionInfo> = serde_json::from_str(&raw).unwrap_or_default();
    if let Some(s) = sessions.iter_mut().find(|s| s.id == id) {
        s.last_active = chrono::Utc::now().to_rfc3339();
    }
    if let Ok(json) = serde_json::to_string_pretty(&sessions) {
        let _ = std::fs::write(index_path(sessions_dir), json);
    }
}

// ── Conversation history loading ──────────────────────────────────────────

/// Load the conversation history from a session file for memory restoration.
pub fn load_conversation_history(
    path: &Path,
    context_window_size: usize,
    max_context_tokens: usize,
) -> Option<Vec<duga_types::message::Message>> {
    duga_core::history::load_conversation_history(path, context_window_size, max_context_tokens)
}

// ── Transcript reconstruction ─────────────────────────────────────────────

/// Build a display transcript from a session's JSONL events.
///
/// Reconstructs UserMessage, AssistantMessage, ToolCallBlock, DelegationNotice,
/// and MemoryNotice items by replaying the stored events in order.
pub fn load_session_transcript(path: &Path) -> Vec<TranscriptItem> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return vec![],
    };
    let reader = BufReader::new(file);

    let mut items: Vec<TranscriptItem> = Vec::new();
    // Pending tool call block (ToolCallStarted without a matching Finished yet)
    let mut pending_tool: Option<(String, String, Option<String>)> = None; // (id, name, raw_args)

    for line in reader.lines().flatten() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let stored: StoredEvent = match serde_json::from_str(trimmed) {
            Ok(e) => e,
            Err(_) => continue,
        };
        match stored.event {
            Event::AgentStarted { task } => {
                items.push(TranscriptItem::UserMessage {
                    text: task,
                    timestamp: Instant::now(),
                });
            }
            Event::AgentFinished { text } => {
                if let Some(text) = text {
                    if !text.is_empty() {
                        items.push(TranscriptItem::AssistantMessage {
                            text,
                            timestamp: Instant::now(),
                            is_streaming: false,
                        });
                    }
                }
            }
            Event::ToolCallStarted { tool_call, .. } => {
                let raw_args = serde_json::to_string_pretty(&tool_call.raw_args).ok();
                let description = tool_call
                    .raw_args
                    .get("label")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| tool_call.tool.clone());
                pending_tool = Some((tool_call.id.to_string(), description, raw_args));
            }
            Event::ToolCallFinished { result, tool_name, .. } => {
                if let Some((id, description, raw_args)) = pending_tool.take() {
                    let output = if result.output.is_empty() {
                        None
                    } else {
                        Some(result.output)
                    };
                    items.push(TranscriptItem::ToolCallBlock {
                        tool_call_id: id,
                        tool_name,
                        description,
                        raw_args,
                        is_running: false,
                        is_success: Some(result.success),
                        is_expanded: false,
                        timestamp: Instant::now(),
                        output,
                    });
                }
            }
            Event::LoopDelegated { from, to, reason, depth } => {
                items.push(TranscriptItem::DelegationNotice {
                    from,
                    to,
                    reason,
                    depth,
                    timestamp: Instant::now(),
                });
            }
            Event::MemoryCompressed { before_tokens, after_tokens } => {
                items.push(TranscriptItem::MemoryNotice {
                    before_tokens,
                    after_tokens,
                    timestamp: Instant::now(),
                });
            }
            Event::Error { message } => {
                items.push(TranscriptItem::SystemMessage {
                    text: format!("Error: {message}"),
                    level: SystemLevel::Error,
                    timestamp: Instant::now(),
                });
            }
            Event::LlmThinkingDelta { delta, .. } => {
                if let Some(TranscriptItem::ThinkingBlock { text, .. }) = items.last_mut() {
                    text.push_str(&delta);
                } else {
                    items.push(TranscriptItem::ThinkingBlock {
                        text: delta,
                        is_streaming: false,
                        is_expanded: false,
                        scroll_offset: 0,
                        timestamp: Instant::now(),
                    });
                }
            }
            _ => {}
        }
    }

    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup_sessions_dir() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let sessions_dir = dir.path().to_path_buf();
        (dir, sessions_dir)
    }

    fn add_test_session(sessions_dir: &Path, id: &str, title: &str) {
        let info = SessionInfo::new(id.into(), title.into());
        upsert_session_index(sessions_dir, &info);
        // Create a dummy JSONL file.
        let path = sessions_dir.join(format!("{id}.jsonl"));
        fs::write(&path, "{}").unwrap();
    }

    #[test]
    fn test_delete_session_removes_file_and_index() {
        let (_dir, sessions_dir) = setup_sessions_dir();
        add_test_session(&sessions_dir, "s1", "Test Session");
        add_test_session(&sessions_dir, "s2", "Another");

        let before = list_sessions(&sessions_dir);
        assert_eq!(before.len(), 2);

        let result = delete_session(&sessions_dir, "s1").unwrap();
        assert!(result);

        // File should be gone.
        assert!(!sessions_dir.join("s1.jsonl").exists());

        // Index should have only s2.
        let after = list_sessions(&sessions_dir);
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].id, "s2");
    }

    #[test]
    fn test_delete_session_nonexistent_returns_false() {
        let (_dir, sessions_dir) = setup_sessions_dir();
        let result = delete_session(&sessions_dir, "nope").unwrap();
        assert!(!result);
    }

    #[test]
    fn test_delete_session_cleans_index_even_if_file_missing() {
        let (_dir, sessions_dir) = setup_sessions_dir();
        add_test_session(&sessions_dir, "orphan", "Orphan");

        // Remove the JSONL file but leave the index entry.
        fs::remove_file(sessions_dir.join("orphan.jsonl")).unwrap();

        let result = delete_session(&sessions_dir, "orphan").unwrap();
        assert!(result, "should report true because index entry was removed");

        let after = list_sessions(&sessions_dir);
        assert!(after.is_empty());
    }
}
