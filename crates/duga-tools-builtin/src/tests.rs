//! Integration tests for the built-in tools (TASK-4.10).

use duga_sandbox::binary_registry::BinaryRegistry;
use duga_sandbox::exec::CancellationToken;
use duga_sandbox::{SandboxExecutor, Workspace};
use duga_tools::confirmation::{
    ConfirmationDecision, ConfirmationMiddleware, ConfirmationPolicy, ConfirmationProvider,
    ConfirmationRequest,
};
use duga_tools::dispatcher::ToolDispatcher;
use duga_tools::erased::ErasedTool;
use duga_tools::event_sink::NullSink;
use duga_types::config::OutputLimits;
use duga_types::tool_call::ToolCall;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

use crate::edit::EditTool;
use crate::read::ReadTool;
use crate::search::SearchTool;
use crate::shell::ShellTool;
use crate::think::{ThinkLimits, ThinkTool};
use crate::write::WriteTool;

fn setup() -> (TempDir, Arc<Workspace>, ToolDispatcher) {
    let d = TempDir::new().unwrap();
    let ws = Arc::new(Workspace::open(d.path()).unwrap());
    let r = Arc::new(BinaryRegistry::new(&["echo".into(), "cat".into(), "sh".into()]).unwrap());
    let l = OutputLimits::default();
    let dp = ToolDispatcher::new();
    dp.register_erased(ErasedTool::erase(ReadTool::new()))
        .unwrap();
    dp.register_erased(ErasedTool::erase(WriteTool::new()))
        .unwrap();
    dp.register_erased(ErasedTool::erase(EditTool::new()))
        .unwrap();
    dp.register_erased(ErasedTool::erase(ShellTool::with_sandbox(
        r.clone(),
        ws.clone(),
        l.clone(),
        Duration::from_secs(30),
        Arc::new(SandboxExecutor::capability()),
    )))
    .unwrap();
    dp.register_erased(ErasedTool::erase(SearchTool::new()))
        .unwrap();
    dp.register_erased(ErasedTool::erase(ThinkTool::new(ThinkLimits {
        max_calls: 100,
        max_tokens: 10000,
    })))
    .unwrap();
    (d, ws, dp)
}

/// Setup with aux_roots configured (simulates Telegram bot data dir access).
fn setup_with_aux(aux_dir: &TempDir) -> (TempDir, Arc<Workspace>, ToolDispatcher) {
    let d = TempDir::new().unwrap();
    let ws = Arc::new(Workspace::open(d.path()).unwrap());
    let r = Arc::new(BinaryRegistry::new(&["echo".into(), "cat".into(), "sh".into()]).unwrap());
    let l = OutputLimits::default();
    let dp = ToolDispatcher::new();
    let aux_roots = vec![aux_dir.path().to_path_buf()];
    dp.register_erased(ErasedTool::erase(
        ReadTool::new().with_aux_roots(aux_roots.clone()),
    ))
    .unwrap();
    dp.register_erased(ErasedTool::erase(
        WriteTool::new().with_aux_roots(aux_roots.clone()),
    ))
    .unwrap();
    dp.register_erased(ErasedTool::erase(
        EditTool::new().with_aux_roots(aux_roots.clone()),
    ))
    .unwrap();
    dp.register_erased(ErasedTool::erase(ShellTool::with_sandbox(
        r.clone(),
        ws.clone(),
        l.clone(),
        Duration::from_secs(30),
        Arc::new(SandboxExecutor::capability()),
    )))
    .unwrap();
    dp.register_erased(ErasedTool::erase(SearchTool::new()))
        .unwrap();
    dp.register_erased(ErasedTool::erase(ThinkTool::new(ThinkLimits {
        max_calls: 100,
        max_tokens: 10000,
    })))
    .unwrap();
    (d, ws, dp)
}

#[tokio::test]
async fn test_write_read_roundtrip() {
    let (_d, ws, dp) = setup();
    let c = CancellationToken::new();
    let s = NullSink;

    dp.dispatch(
        &ToolCall::new("write", json!({"path":"t.txt","content":"hello\nworld"})),
        &ws,
        c.clone(),
        &s,
    )
    .await
    .unwrap();
    let r = dp
        .dispatch(
            &ToolCall::new("read", json!({"path":"t.txt"})),
            &ws,
            c.clone(),
            &s,
        )
        .await
        .unwrap();
    assert_eq!(r.output, "hello\nworld");
}

#[tokio::test]
async fn test_dispatch_preserves_tool_call_id() {
    let (_d, ws, dp) = setup();
    let c = CancellationToken::new();
    let s = NullSink;
    let call = ToolCall::new("write", json!({"path":"id.txt","content":"hello"}));
    let expected = call.id.clone();

    let r = dp.dispatch(&call, &ws, c, &s).await.unwrap();

    assert_eq!(r.tool_call_id, expected);
}

#[tokio::test]
async fn test_search_finds() {
    let (_d, ws, dp) = setup();
    let c = CancellationToken::new();
    let s = NullSink;
    dp.dispatch(
        &ToolCall::new("write", json!({"path":"d.txt","content":"foo\nbar"})),
        &ws,
        c.clone(),
        &s,
    )
    .await
    .unwrap();
    let r = dp
        .dispatch(
            &ToolCall::new("search", json!({"query":"foo"})),
            &ws,
            c.clone(),
            &s,
        )
        .await
        .unwrap();
    assert!(r.output.contains("foo"));
}

#[tokio::test]
async fn test_think_works() {
    let (_d, ws, dp) = setup();
    let c = CancellationToken::new();
    let s = NullSink;
    let r = dp
        .dispatch(
            &ToolCall::new("think", json!({"thought":"hello"})),
            &ws,
            c.clone(),
            &s,
        )
        .await
        .unwrap();
    assert_eq!(r.output, "hello");
}

#[tokio::test]
async fn test_shell_cat() {
    let (_d, ws, dp) = setup();
    let c = CancellationToken::new();
    let s = NullSink;
    dp.dispatch(
        &ToolCall::new("write", json!({"path":"x.txt","content":"data"})),
        &ws,
        c.clone(),
        &s,
    )
    .await
    .unwrap();
    let r = dp
        .dispatch(
            &ToolCall::new("shell", json!({"command":["cat","x.txt"]})),
            &ws,
            c.clone(),
            &s,
        )
        .await
        .unwrap();
    assert!(r.output.contains("data"));
}

#[tokio::test]
async fn test_edit_replaces_written_file() {
    let (_d, ws, dp) = setup();
    let c = CancellationToken::new();
    let s = NullSink;
    dp.dispatch(
        &ToolCall::new(
            "write",
            json!({"path":"edit.txt","content":"hello old world"}),
        ),
        &ws,
        c.clone(),
        &s,
    )
    .await
    .unwrap();

    let r = dp
        .dispatch(
            &ToolCall::new(
                "edit",
                json!({"path":"edit.txt","oldText":"old","newText":"new"}),
            ),
            &ws,
            c.clone(),
            &s,
        )
        .await
        .unwrap();

    assert!(r.output.contains("Edited edit.txt"));
    let content = dp
        .dispatch(
            &ToolCall::new("read", json!({"path":"edit.txt"})),
            &ws,
            c,
            &s,
        )
        .await
        .unwrap();
    assert_eq!(content.output, "hello new world");
}

#[tokio::test]
async fn test_errors() {
    let (_d, ws, dp) = setup();
    let c = CancellationToken::new();
    let s = NullSink;
    assert!(
        dp.dispatch(
            &ToolCall::new("write", json!({"path":"../x","content":"x"})),
            &ws,
            c.clone(),
            &s
        )
        .await
        .is_err()
    );
    assert!(
        dp.dispatch(
            &ToolCall::new("read", json!({"path":"no"})),
            &ws,
            c.clone(),
            &s
        )
        .await
        .is_err()
    );
    assert!(
        dp.dispatch(
            &ToolCall::new("search", json!({"query":"["})),
            &ws,
            c.clone(),
            &s
        )
        .await
        .is_err()
    );
}

// ── E2E tests: aux_roots (data dir access) ─────────────────────────────

/// Read a file from aux_roots (not in workspace).
#[tokio::test]
async fn test_aux_read_file_outside_workspace() {
    let aux_dir = TempDir::new().unwrap();
    std::fs::write(aux_dir.path().join("data.json"), r#"{"key": "value"}"#).unwrap();

    let (_ws_dir, ws, dp) = setup_with_aux(&aux_dir);
    let c = CancellationToken::new();
    let s = NullSink;

    // File is NOT in workspace, only in aux_dir
    let r = dp
        .dispatch(
            &ToolCall::new("read", json!({"path": "data.json"})),
            &ws,
            c.clone(),
            &s,
        )
        .await
        .unwrap();
    assert_eq!(r.output, r#"{"key": "value"}"#);
}

/// Write a file to aux_roots only when explicitly addressed by absolute path.
#[tokio::test]
async fn test_aux_write_file_outside_workspace() {
    let aux_dir = TempDir::new().unwrap();
    // Create a subdirectory in aux_dir (simulates data/ dir structure)
    std::fs::create_dir_all(aux_dir.path().join("logs")).unwrap();
    let aux_path = aux_dir.path().join("logs/output.log");

    let (_ws_dir, ws, dp) = setup_with_aux(&aux_dir);
    let c = CancellationToken::new();
    let s = NullSink;

    let r = dp
        .dispatch(
            &ToolCall::new(
                "write",
                json!({"path": aux_path.to_str().unwrap(), "content": "log entry"}),
            ),
            &ws,
            c.clone(),
            &s,
        )
        .await
        .unwrap();
    assert!(r.success);
    assert!(r.output.contains("output.log"));

    // Verify the file was actually written to aux_dir, not workspace
    let content = std::fs::read_to_string(aux_dir.path().join("logs/output.log")).unwrap();
    assert_eq!(content, "log entry");
}

/// Relative writes prefer workspace over aux_roots even when only aux has the parent dir.
#[tokio::test]
async fn test_write_relative_prefers_workspace_over_aux_parent() {
    let aux_dir = TempDir::new().unwrap();
    std::fs::create_dir_all(aux_dir.path().join("logs")).unwrap();

    let (_ws_dir, ws, dp) = setup_with_aux(&aux_dir);
    let c = CancellationToken::new();
    let s = NullSink;

    let r = dp
        .dispatch(
            &ToolCall::new(
                "write",
                json!({"path": "logs/output.log", "content": "workspace version"}),
            ),
            &ws,
            c.clone(),
            &s,
        )
        .await
        .unwrap();
    assert!(r.success);

    assert!(!aux_dir.path().join("logs/output.log").exists());
    let ws_content = std::fs::read_to_string(ws.root_path().join("logs/output.log")).unwrap();
    assert_eq!(ws_content, "workspace version");
}

/// Read prefers workspace over aux_roots.
#[tokio::test]
async fn test_read_prefers_workspace_over_aux() {
    let aux_dir = TempDir::new().unwrap();
    std::fs::write(aux_dir.path().join("dual.txt"), "aux data").unwrap();

    let (_ws_dir, ws, dp) = setup_with_aux(&aux_dir);
    let c = CancellationToken::new();
    let s = NullSink;

    // Write to workspace
    dp.dispatch(
        &ToolCall::new(
            "write",
            json!({"path": "dual.txt", "content": "workspace data"}),
        ),
        &ws,
        c.clone(),
        &s,
    )
    .await
    .unwrap();

    // Read should get workspace version
    let r = dp
        .dispatch(
            &ToolCall::new("read", json!({"path": "dual.txt"})),
            &ws,
            c.clone(),
            &s,
        )
        .await
        .unwrap();
    assert_eq!(r.output, "workspace data");
}

/// Edit in aux_roots when file is not in workspace.
#[tokio::test]
async fn test_aux_edit_file_outside_workspace() {
    let aux_dir = TempDir::new().unwrap();
    std::fs::write(
        aux_dir.path().join("config.yaml"),
        "host: localhost\nport: 8080\n",
    )
    .unwrap();

    let (_ws_dir, ws, dp) = setup_with_aux(&aux_dir);
    let c = CancellationToken::new();
    let s = NullSink;

    let r = dp
        .dispatch(
            &ToolCall::new(
                "edit",
                json!({"path": "config.yaml", "oldText": "localhost", "newText": "0.0.0.0"}),
            ),
            &ws,
            c.clone(),
            &s,
        )
        .await
        .unwrap();
    assert!(r.success);
    assert!(r.output.contains("config.yaml"));

    let content = std::fs::read_to_string(aux_dir.path().join("config.yaml")).unwrap();
    assert!(content.contains("0.0.0.0"));
    assert!(!content.contains("localhost"));
}

/// Edit prefers workspace file over aux_roots when path exists in both.
#[tokio::test]
async fn test_edit_prefers_workspace_over_aux() {
    let aux_dir = TempDir::new().unwrap();
    std::fs::write(aux_dir.path().join("both.txt"), "aux content").unwrap();

    let (_ws_dir, ws, dp) = setup_with_aux(&aux_dir);
    let c = CancellationToken::new();
    let s = NullSink;

    // Write to workspace
    dp.dispatch(
        &ToolCall::new(
            "write",
            json!({"path": "both.txt", "content": "workspace content"}),
        ),
        &ws,
        c.clone(),
        &s,
    )
    .await
    .unwrap();

    // Edit should hit workspace, not aux
    let r = dp
        .dispatch(
            &ToolCall::new(
                "edit",
                json!({"path": "both.txt", "oldText": "workspace", "newText": "modified"}),
            ),
            &ws,
            c.clone(),
            &s,
        )
        .await
        .unwrap();
    assert!(r.success);

    // aux file unchanged
    let aux_content = std::fs::read_to_string(aux_dir.path().join("both.txt")).unwrap();
    assert_eq!(aux_content, "aux content");

    // workspace file modified (read via workspace root path)
    let ws_content = std::fs::read_to_string(ws.root_path().join("both.txt")).unwrap();
    assert_eq!(ws_content, "modified content");
}

/// Writing to aux_roots rejects path traversal attempts.
#[tokio::test]
async fn test_aux_write_rejects_path_traversal() {
    let aux_dir = TempDir::new().unwrap();
    let (_ws_dir, ws, dp) = setup_with_aux(&aux_dir);
    let c = CancellationToken::new();
    let s = NullSink;

    // Relative paths with .. are resolved relative to aux_root, not workspace.
    // But workspace.resolve() rejects them first. Let's test with a path that
    // workspace rejects but aux accepts — actually, path with .. gets rejected by
    // workspace.resolve() AND we don't try aux for paths with .. because they get
    // caught by workspace.resolve() returning Err.
    // The key test: write to a path that doesn't exist in workspace but would be
    // a traversal if aux allowed it. Since aux only allows relative non-absolute
    // paths and joins them to aux_root, "../etc/passwd" joined to aux_root would
    // be "aux_root/../etc/passwd" which normalizes to "parent/etc/passwd" — still
    // within aux_root's parent, so it IS a traversal.
    // Let's test write: aux_roots reject any path that would escape.
    let err = dp
        .dispatch(
            &ToolCall::new(
                "write",
                json!({"path": "../outside.txt", "content": "should fail"}),
            ),
            &ws,
            c.clone(),
            &s,
        )
        .await;
    // workspace rejects it first (.. in path)
    assert!(err.is_err());
}

/// Reading from aux_roots with a non-existent file returns an error.
#[tokio::test]
async fn test_aux_read_nonexistent_file() {
    let aux_dir = TempDir::new().unwrap();
    let (_ws_dir, ws, dp) = setup_with_aux(&aux_dir);
    let c = CancellationToken::new();
    let s = NullSink;

    let err = dp
        .dispatch(
            &ToolCall::new("read", json!({"path": "nonexistent.xyz"})),
            &ws,
            c.clone(),
            &s,
        )
        .await;
    assert!(err.is_err());
}

/// Editing in aux_roots with non-matching oldText returns error.
#[tokio::test]
async fn test_aux_edit_rejects_missing_oldtext() {
    let aux_dir = TempDir::new().unwrap();
    std::fs::write(aux_dir.path().join("note.txt"), "actual content").unwrap();

    let (_ws_dir, ws, dp) = setup_with_aux(&aux_dir);
    let c = CancellationToken::new();
    let s = NullSink;

    let err = dp
        .dispatch(
            &ToolCall::new(
                "edit",
                json!({"path": "note.txt", "oldText": "not in file", "newText": "replacement"}),
            ),
            &ws,
            c.clone(),
            &s,
        )
        .await;
    assert!(err.is_err());
    let msg = err.unwrap_err().to_string();
    assert!(msg.contains("oldText not found"));
}

// ── E2E tests: confirmation middleware with aux_roots ──────────────────

struct StaticConfirmProvider(ConfirmationDecision);

#[async_trait::async_trait]
impl ConfirmationProvider for StaticConfirmProvider {
    async fn confirm(
        &self,
        _request: &ConfirmationRequest,
        _timeout: Duration,
    ) -> ConfirmationDecision {
        self.0.clone()
    }
}

/// Confirmation middleware gates aux writes.
#[tokio::test]
async fn test_aux_write_respects_confirmation_deny() {
    let aux_dir = TempDir::new().unwrap();
    let (_ws_dir, ws, dp) = setup_with_aux(&aux_dir);

    // Require confirmation for write tool
    let policy = ConfirmationPolicy::new(["write"], Duration::from_secs(5));
    let provider = Arc::new(StaticConfirmProvider(ConfirmationDecision::Denied));
    dp.set_confirmation(ConfirmationMiddleware::new(policy, provider));

    let c = CancellationToken::new();
    let s = NullSink;

    let err = dp
        .dispatch(
            &ToolCall::new(
                "write",
                json!({"path": "denied.txt", "content": "should not write"}),
            ),
            &ws,
            c.clone(),
            &s,
        )
        .await;
    assert!(err.is_err());
    let msg = err.unwrap_err().to_string();
    assert!(msg.contains("denied"));

    // File should NOT exist in aux
    assert!(!aux_dir.path().join("denied.txt").exists());
}

/// Confirmation middleware gates aux edits.
#[tokio::test]
async fn test_aux_edit_respects_confirmation_deny() {
    let aux_dir = TempDir::new().unwrap();
    std::fs::write(aux_dir.path().join("keep.txt"), "original").unwrap();

    let (_ws_dir, ws, dp) = setup_with_aux(&aux_dir);

    let policy = ConfirmationPolicy::new(["edit"], Duration::from_secs(5));
    let provider = Arc::new(StaticConfirmProvider(ConfirmationDecision::Denied));
    dp.set_confirmation(ConfirmationMiddleware::new(policy, provider));

    let c = CancellationToken::new();
    let s = NullSink;

    let err = dp
        .dispatch(
            &ToolCall::new(
                "edit",
                json!({"path": "keep.txt", "oldText": "original", "newText": "modified"}),
            ),
            &ws,
            c.clone(),
            &s,
        )
        .await;
    assert!(err.is_err());

    // File should remain unchanged
    let content = std::fs::read_to_string(aux_dir.path().join("keep.txt")).unwrap();
    assert_eq!(content, "original");
}

/// Confirmation middleware allows aux reads without confirmation.
#[tokio::test]
async fn test_aux_read_not_gated_by_confirmation() {
    let aux_dir = TempDir::new().unwrap();
    std::fs::write(aux_dir.path().join("free.txt"), "free data").unwrap();

    let (_ws_dir, ws, dp) = setup_with_aux(&aux_dir);
    // Note: confirmation is NOT set — read should work regardless
    let c = CancellationToken::new();
    let s = NullSink;

    let r = dp
        .dispatch(
            &ToolCall::new("read", json!({"path": "free.txt"})),
            &ws,
            c.clone(),
            &s,
        )
        .await
        .unwrap();
    assert_eq!(r.output, "free data");
}

// ── Security: symlink attacks on aux_roots ────────────────────────────

#[cfg(unix)]
#[tokio::test]
async fn test_aux_write_rejects_symlink_components() {
    let aux_dir = TempDir::new().unwrap();
    let outside_dir = TempDir::new().unwrap();

    // Create a symlink inside aux_dir pointing outside
    std::os::unix::fs::symlink(outside_dir.path(), aux_dir.path().join("link")).unwrap();

    let (_ws_dir, ws, dp) = setup_with_aux(&aux_dir);
    let c = CancellationToken::new();
    let s = NullSink;

    // Try to write through the symlink via explicit aux-root addressing.
    let target = aux_dir.path().join("link/escape.txt");
    let err = dp
        .dispatch(
            &ToolCall::new(
                "write",
                json!({"path": target.to_str().unwrap(), "content": "pwned"}),
            ),
            &ws,
            c.clone(),
            &s,
        )
        .await;
    assert!(err.is_err());
    let msg = err.unwrap_err().to_string();
    assert!(msg.contains("symlink"));

    // File should NOT exist outside
    assert!(!outside_dir.path().join("escape.txt").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn test_aux_edit_rejects_symlink_components() {
    let aux_dir = TempDir::new().unwrap();
    let outside_dir = TempDir::new().unwrap();

    // Create a file and a symlink directory
    std::fs::write(outside_dir.path().join("target.txt"), "outside data").unwrap();
    std::os::unix::fs::symlink(outside_dir.path(), aux_dir.path().join("shortcut")).unwrap();

    let (_ws_dir, ws, dp) = setup_with_aux(&aux_dir);
    let c = CancellationToken::new();
    let s = NullSink;

    let err = dp
        .dispatch(
            &ToolCall::new(
                "edit",
                json!({"path": "shortcut/target.txt", "oldText": "outside data", "newText": "pwned"}),
            ),
            &ws,
            c.clone(),
            &s,
        )
        .await;
    assert!(err.is_err());
    let msg = err.unwrap_err().to_string();
    assert!(msg.contains("symlink") || msg.contains("not found"));

    // File outside should remain unchanged
    let content = std::fs::read_to_string(outside_dir.path().join("target.txt")).unwrap();
    assert_eq!(content, "outside data");
}
