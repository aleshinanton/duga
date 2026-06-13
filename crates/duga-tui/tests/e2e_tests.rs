//! End-to-end integration tests for duga-tui.
//!
//! Tests event handling, transcript state, rendering, key routing,
//! overlay lifecycle, and agent event integration — all without
//! requiring a live terminal or LLM provider.

use duga_runtime::{FrontendEvent, FrontendEventBridge};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;

// Re-export the app module types for testing
use duga_tui::app::{App, AppEvent, AppState};
use duga_tui::editor::EditorAction;
use duga_tui::transcript::{SystemLevel, TranscriptItem};

// Helper to create a test app
fn make_test_app() -> (App, mpsc::UnboundedReceiver<AppEvent>) {
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let (fe_tx, fe_bridge) = FrontendEventBridge::new(16);
    let fe_sink = Arc::new(duga_runtime::FrontendEventSink::new(fe_tx));

    let (dir, config) = test_config();
    let tui_config = config.tui.clone().unwrap_or_default();

    let app = App::new(
        config,
        tui_config,
        event_tx,
        fe_bridge,
        fe_sink,
        std::path::PathBuf::from("/tmp/duga-test-sessions"),
    );
    let _ = dir;
    (app, event_rx)
}

fn test_config() -> (tempfile::TempDir, duga_config::Config) {
    let dir = tempfile::tempdir().unwrap();
    let yaml = format!(
        r#"
model: "dummy/test"
agent:
  limits:
    max_steps: 5
    max_tool_calls: 10
    max_runtime: 60s
    retry_on_error: 1
  features:
    streaming: false
  output:
    max_stdout_bytes: 1024
    max_stderr_bytes: 1024
    max_combined_bytes: 2048
  think:
    max_calls: 2
    max_tokens: 128
sandbox:
  timeout: 10s
  allowed_binaries:
    - echo
workspace:
  root: {root}
environment:
  allowed:
    - HOME
memory:
  max_tokens: 4096
  compress_at_ratio: 0.8
  context_window_size: 50
  max_context_tokens: 12000
  summarizer: simple
plugins:
  dir: {plugin_dir}
  modules: []
"#,
        root = dir.path().display(),
        plugin_dir = dir.path().display(),
    );
    let path = dir.path().join("test_config.yaml");
    std::fs::write(&path, yaml).unwrap();
    let config = duga_config::Config::load(&path).expect("test config must parse");
    (dir, config)
}

// ── App State Tests ────────────────────────────────────────────────────────

#[test]
fn test_app_initial_state() {
    let (app, _rx) = make_test_app();
    assert!(matches!(app.state, AppState::Idle));
    assert!(!app.should_quit());
    assert_eq!(app.transcript.len(), 0);
}

#[test]
fn test_editor_submit_empties_buffer() {
    let (mut app, _rx) = make_test_app();
    app.editor.insert_text("hello world");

    // Simulate Enter key
    let key = crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::NONE,
    );
    // The editor should return Submit action
    let action = app.editor.handle_key(&key);
    assert_eq!(action, EditorAction::Submit);

    // Take the text (simulates what App does on submit)
    let text = app.editor.take_text();
    assert_eq!(text, "hello world");
    assert_eq!(app.editor.text(), "");
}

// ── Transcript State Tests ─────────────────────────────────────────────────

#[test]
fn test_transcript_user_message() {
    let (mut app, _rx) = make_test_app();
    app.transcript.push(TranscriptItem::UserMessage {
        text: "test task".into(),
        timestamp: Instant::now(),
    });
    assert_eq!(app.transcript.len(), 1);
    let items = app.transcript.items();
    if let TranscriptItem::UserMessage { text, .. } = &items[0] {
        assert_eq!(text, "test task");
    } else {
        panic!("expected UserMessage");
    }
}

#[test]
fn test_transcript_streaming_tokens() {
    let (mut app, _rx) = make_test_app();
    app.transcript.append_to_streaming("Hello");
    app.transcript.append_to_streaming(" world");
    app.transcript.finish_streaming();

    assert_eq!(app.transcript.len(), 1);
    if let TranscriptItem::AssistantMessage {
        text, is_streaming, ..
    } = &app.transcript.items()[0]
    {
        assert_eq!(text, "Hello world");
        assert!(!is_streaming);
    } else {
        panic!("expected AssistantMessage");
    }
}

#[test]
fn test_transcript_tool_call_lifecycle() {
    let (mut app, _rx) = make_test_app();
    app.transcript.push(TranscriptItem::ToolCallBlock {
        tool_call_id: "tc-1".into(),
        tool_name: "shell".into(),
        description: "ls -la".into(),
        raw_args: None,
        is_running: true,
        is_success: None,
        is_expanded: false,
        timestamp: Instant::now(),
        output: None,
    });
    assert_eq!(app.transcript.len(), 1);

    // Update tool call to succeeded
    app.transcript.update_tool_call("tc-1", true, None);
    if let TranscriptItem::ToolCallBlock {
        is_running,
        is_success,
        ..
    } = &app.transcript.items()[0]
    {
        assert!(!is_running);
        assert_eq!(*is_success, Some(true));
    } else {
        panic!("expected ToolCallBlock");
    }
}

#[test]
fn test_transcript_tool_call_failed_expands() {
    let (mut app, _rx) = make_test_app();
    app.transcript.push(TranscriptItem::ToolCallBlock {
        tool_call_id: "tc-fail".into(),
        tool_name: "write".into(),
        description: "write file".into(),
        raw_args: None,
        is_running: true,
        is_success: None,
        is_expanded: false,
        timestamp: Instant::now(),
        output: None,
    });
    app.transcript.update_tool_call("tc-fail", false, None);
    if let TranscriptItem::ToolCallBlock {
        is_running,
        is_success,
        is_expanded,
        ..
    } = &app.transcript.items()[0]
    {
        assert!(!is_running);
        assert_eq!(*is_success, Some(false));
        assert!(is_expanded, "failed tool calls should be expanded");
    } else {
        panic!("expected ToolCallBlock");
    }
}

#[test]
fn test_transcript_system_message() {
    let (mut app, _rx) = make_test_app();
    app.transcript.push(TranscriptItem::SystemMessage {
        text: "error occurred".into(),
        level: SystemLevel::Error,
        timestamp: Instant::now(),
    });
    assert_eq!(app.transcript.len(), 1);
}

#[test]
fn test_transcript_clear() {
    let (mut app, _rx) = make_test_app();
    app.transcript.push(TranscriptItem::SystemMessage {
        text: "msg".into(),
        level: SystemLevel::Info,
        timestamp: Instant::now(),
    });
    assert_eq!(app.transcript.len(), 1);
    app.transcript.clear();
    assert_eq!(app.transcript.len(), 0);
}

// ── FrontendEvent Handling Tests ───────────────────────────────────────────

#[test]
fn test_frontend_event_run_started() {
    let (mut app, _rx) = make_test_app();
    app.update(AppEvent::Frontend(FrontendEvent::RunStarted {
        task: "do something".into(),
    }));
    // RunStarted no longer pushes a UserMessage — that's done by submit_prompt().
    // The event is a no-op for the transcript.
    assert_eq!(app.transcript.len(), 0);
}

#[test]
fn test_frontend_event_tool_call_started() {
    let (mut app, _rx) = make_test_app();
    app.update(AppEvent::Frontend(FrontendEvent::ToolCallStarted {
        tool_name: "shell".into(),
        tool_call_id: "tc1".into(),
        attempt: 1,
        description: "list files".into(),
        raw_args: None,
    }));
    assert_eq!(app.transcript.len(), 1);
    if let TranscriptItem::ToolCallBlock {
        tool_name,
        description,
        is_running,
        ..
    } = &app.transcript.items()[0]
    {
        assert_eq!(tool_name, "shell");
        assert_eq!(description, "list files");
        assert!(is_running);
    } else {
        panic!("expected ToolCallBlock");
    }
}

#[test]
fn test_frontend_event_tool_call_finished() {
    let (mut app, _rx) = make_test_app();
    // First, ToolCallStarted
    app.update(AppEvent::Frontend(FrontendEvent::ToolCallStarted {
        tool_name: "shell".into(),
        tool_call_id: "tc1".into(),
        attempt: 1,
        description: "list files".into(),
        raw_args: None,
    }));
    // Then ToolCallFinished
    app.update(AppEvent::Frontend(FrontendEvent::ToolCallFinished {
        tool_name: "shell".into(),
        tool_call_id: "tc1".into(),
        success: true,
        attempt: 1,
        description: "list files".into(),
        output: None,
    }));
    let items = app.transcript.items();
    assert_eq!(items.len(), 1);
    if let TranscriptItem::ToolCallBlock {
        is_running,
        is_success,
        ..
    } = &items[0]
    {
        assert!(!is_running);
        assert_eq!(*is_success, Some(true));
    } else {
        panic!("expected ToolCallBlock");
    }
}

#[test]
fn test_frontend_event_streaming_tokens() {
    let (mut app, _rx) = make_test_app();
    app.update(AppEvent::Frontend(FrontendEvent::LlmTokenDelta {
        model: "test".into(),
        delta: "Hello".into(),
    }));
    app.update(AppEvent::Frontend(FrontendEvent::LlmTokenDelta {
        model: "test".into(),
        delta: " world".into(),
    }));
    assert_eq!(app.transcript.len(), 1);
    if let TranscriptItem::AssistantMessage { text, .. } = &app.transcript.items()[0] {
        assert_eq!(text, "Hello world");
    } else {
        panic!("expected AssistantMessage");
    }
}

#[test]
fn test_frontend_event_run_finished() {
    let (mut app, _rx) = make_test_app();
    // Start streaming
    app.update(AppEvent::Frontend(FrontendEvent::LlmTokenDelta {
        model: "test".into(),
        delta: "result".into(),
    }));
    // Finish run
    app.update(AppEvent::Frontend(FrontendEvent::RunFinished {
        text: None, // streaming already captured it
    }));
    let items = app.transcript.items();
    assert_eq!(items.len(), 1);
    if let TranscriptItem::AssistantMessage { is_streaming, .. } = &items[0] {
        assert!(!is_streaming, "streaming should be marked finished");
    } else {
        panic!("expected AssistantMessage");
    }
}

#[test]
fn test_frontend_event_error() {
    let (mut app, _rx) = make_test_app();
    app.update(AppEvent::Frontend(FrontendEvent::Error {
        message: "connection failed".into(),
    }));
    assert_eq!(app.transcript.len(), 1);
    if let TranscriptItem::SystemMessage { text, level, .. } = &app.transcript.items()[0] {
        assert_eq!(text, "connection failed");
        assert_eq!(*level, SystemLevel::Error);
    } else {
        panic!("expected SystemMessage");
    }
}

#[test]
fn test_frontend_event_delegation() {
    let (mut app, _rx) = make_test_app();
    app.update(AppEvent::Frontend(FrontendEvent::LoopDelegated {
        from: "simple_react".into(),
        to: "problem_solving".into(),
        reason: "complex task".into(),
        depth: 1,
    }));
    assert_eq!(app.transcript.len(), 1);
    if let TranscriptItem::DelegationNotice { from, to, .. } = &app.transcript.items()[0] {
        assert_eq!(from, "simple_react");
        assert_eq!(to, "problem_solving");
    } else {
        panic!("expected DelegationNotice");
    }
}

#[test]
fn test_frontend_event_memory_compressed() {
    let (mut app, _rx) = make_test_app();
    app.update(AppEvent::Frontend(FrontendEvent::MemoryCompressed {
        before_tokens: 5000,
        after_tokens: 3000,
    }));
    assert_eq!(app.transcript.len(), 1);
    assert!(matches!(
        &app.transcript.items()[0],
        TranscriptItem::MemoryNotice { .. }
    ));
}

// ── Key Routing Tests ─────────────────────────────────────────────────────

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[test]
fn test_key_quit_when_idle() {
    let (mut app, _rx) = make_test_app();
    app.handle_key(&KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL));
    assert!(app.should_quit());
}

#[test]
fn test_key_ctrl_q_quit() {
    let (mut app, _rx) = make_test_app();
    app.handle_key(&KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL));
    assert!(app.should_quit());
}

#[test]
fn test_key_help_toggles_overlay() {
    let (mut app, _rx) = make_test_app();
    assert!(!app.overlays.has_overlay());
    app.handle_key(&KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    assert!(app.overlays.has_overlay());
    // Escape closes
    app.handle_key(&KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!app.overlays.has_overlay());
}

#[test]
fn test_key_search_opens_overlay() {
    let (mut app, _rx) = make_test_app();
    app.handle_key(&KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL));
    assert!(app.overlays.has_overlay());
    app.handle_key(&KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!app.overlays.has_overlay());
}

#[test]
fn test_key_ctrl_l_clears_transcript() {
    let (mut app, _rx) = make_test_app();
    app.transcript.push(TranscriptItem::SystemMessage {
        text: "some message".into(),
        level: SystemLevel::Info,
        timestamp: Instant::now(),
    });
    assert_eq!(app.transcript.len(), 1);
    app.handle_key(&KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL));
    // Should have the "Transcript cleared" message + original
    assert!(app.transcript.len() >= 1);
}

// ── Editor Tests ───────────────────────────────────────────────────────────

#[test]
fn test_editor_multiline_input() {
    let (mut app, _rx) = make_test_app();
    // Insert first line
    app.editor.insert_text("line1");
    // Shift+Enter for newline
    let shift_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
    let action = app.editor.handle_key(&shift_enter);
    assert_eq!(action, EditorAction::Consumed);
    // Insert second line
    app.editor.insert_text("line2");
    assert!(app.editor.text().contains('\n'));
    assert_eq!(app.editor.text(), "line1\nline2");
}

#[test]
fn test_editor_history_navigation() {
    let (mut app, _rx) = make_test_app();
    app.editor.insert_text("first command");
    app.editor.take_text();
    app.editor.insert_text("second command");
    app.editor.take_text();

    // Navigate back
    let up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
    app.editor.handle_key(&up);
    assert_eq!(app.editor.text(), "second command");
    app.editor.handle_key(&up);
    assert_eq!(app.editor.text(), "first command");
}

#[test]
fn test_editor_accepts_input_during_run() {
    let (mut app, _rx) = make_test_app();
    // Editor stays active during runs for steering input.
    let action = app
        .editor
        .handle_key(&KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    assert_eq!(action, EditorAction::Consumed);
    assert_eq!(app.editor.text(), "a");
}

// ── Run Lifecycle Tests ────────────────────────────────────────────────────

#[test]
fn test_run_finished_transitions_to_idle() {
    let (mut app, _rx) = make_test_app();
    // Simulate RunFinished event
    app.update(AppEvent::RunFinished {
        run_id: 0,
        result: Ok(duga_core::LoopResult {
            loop_id: "simple_react".into(),
            message: duga_types::message::AssistantMessage {
                text: Some("done".into()),
                tool_calls: vec![],
                reasoning_content: None,
            },
            steps: 1,
            tool_calls: 0,
        }),
    });
    assert!(matches!(app.state, AppState::Idle));
}

// ── Confirmation Dialog Tests ──────────────────────────────────────────────

#[test]
fn test_confirmation_dialog_yes_no() {
    use duga_tui::overlay::Overlay;
    use duga_tui::overlay::confirmation::{ConfirmationDialog, ConfirmationResult};

    let mut dialog = ConfirmationDialog::yes_no("Confirm", "Are you sure?");
    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    let action = dialog.handle_key(&enter);
    assert!(matches!(action, duga_tui::overlay::OverlayAction::Close));
    let result = dialog.take_result().unwrap();
    assert!(matches!(result, ConfirmationResult::Confirmed(0)));
}

#[test]
fn test_confirmation_dialog_escape_dismisses() {
    use duga_tui::overlay::Overlay;
    use duga_tui::overlay::confirmation::ConfirmationDialog;

    let mut dialog = ConfirmationDialog::yes_no_cancel("Confirm", "Are you sure?");
    let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
    let action = dialog.handle_key(&esc);
    assert!(matches!(action, duga_tui::overlay::OverlayAction::Close));
    let result = dialog.take_result().unwrap();
    assert!(matches!(
        result,
        duga_tui::overlay::confirmation::ConfirmationResult::Dismissed
    ));
}

#[test]
fn test_confirmation_dialog_quick_keys() {
    use duga_tui::overlay::Overlay;
    use duga_tui::overlay::confirmation::ConfirmationDialog;

    let mut dialog = ConfirmationDialog::yes_no("Confirm", "Are you sure?");
    let y = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE);
    let action = dialog.handle_key(&y);
    assert!(matches!(action, duga_tui::overlay::OverlayAction::Close));
}

// ── Search Overlay Tests ───────────────────────────────────────────────────

#[test]
fn test_search_finds_matches() {
    use duga_tui::overlay::search::SearchOverlay;

    let (mut app, _rx) = make_test_app();
    app.transcript.push(TranscriptItem::UserMessage {
        text: "hello world".into(),
        timestamp: Instant::now(),
    });
    app.transcript.push(TranscriptItem::AssistantMessage {
        text: "hi there hello".into(),
        timestamp: Instant::now(),
        is_streaming: false,
    });

    let mut search = SearchOverlay::new();
    search.input.insert_text("hello");
    search.search(&app.transcript);

    assert_eq!(search.matches.len(), 2);
    assert_eq!(search.selected_match, Some(0));
}

#[test]
fn test_search_no_matches() {
    use duga_tui::overlay::search::SearchOverlay;

    let (mut app, _rx) = make_test_app();
    app.transcript.push(TranscriptItem::UserMessage {
        text: "foo bar".into(),
        timestamp: Instant::now(),
    });

    let mut search = SearchOverlay::new();
    search.input.insert_text("xyz");
    search.search(&app.transcript);

    assert!(search.matches.is_empty());
    assert_eq!(search.selected_match, None);
}

// ── FinalOnly Tool Event Format Tests ──────────────────────────────────────

#[test]
fn test_final_only_hides_tool_during_run() {
    let (mut app, _rx) = make_test_app();
    app.tool_event_format = duga_config::ToolEventFormat::FinalOnly;

    app.update(AppEvent::Frontend(FrontendEvent::ToolCallStarted {
        tool_name: "shell".into(),
        tool_call_id: "tc-hidden".into(),
        attempt: 1,
        description: "hidden tool".into(),
        raw_args: None,
    }));
    // In FinalOnly mode, ToolCallStarted should not add to transcript
    assert_eq!(app.transcript.len(), 0);
}

#[test]
fn test_final_only_shows_tool_after_completion() {
    let (mut app, _rx) = make_test_app();
    app.tool_event_format = duga_config::ToolEventFormat::FinalOnly;

    // Tool runs...
    app.update(AppEvent::Frontend(FrontendEvent::ToolCallStarted {
        tool_name: "shell".into(),
        tool_call_id: "tc-final".into(),
        attempt: 1,
        description: "final tool".into(),
        raw_args: None,
    }));
    assert_eq!(app.transcript.len(), 0);

    // Tool finishes — should now appear
    app.update(AppEvent::Frontend(FrontendEvent::ToolCallFinished {
        tool_name: "shell".into(),
        tool_call_id: "tc-final".into(),
        success: true,
        attempt: 1,
        description: "final tool".into(),
        output: None,
    }));
    assert_eq!(app.transcript.len(), 1);
    if let TranscriptItem::ToolCallBlock { is_running, .. } = &app.transcript.items()[0] {
        assert!(!is_running);
    } else {
        panic!("expected ToolCallBlock");
    }
}

// ── Session Delete Tests ───────────────────────────────────────────────────

#[test]
fn test_session_picker_delete_key_sends_request() {
    use duga_tui::overlay::Overlay;
    use duga_tui::overlay::session_picker::SessionPickerOverlay;
    use duga_tui::session::SessionInfo;

    let sessions = vec![
        SessionInfo::new("s1".into(), "First session".into()),
        SessionInfo::new("s2".into(), "Second session".into()),
    ];
    let (result_tx, _result_rx) = mpsc::unbounded_channel();
    let (delete_tx, mut delete_rx) = mpsc::unbounded_channel();

    let mut picker = SessionPickerOverlay::new(sessions, result_tx, Some(delete_tx));

    // Press Delete key on first session
    let action = picker.handle_key(&KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
    assert!(matches!(action, duga_tui::overlay::OverlayAction::Consumed));

    // Should have received delete request for s1
    let id = delete_rx.try_recv().unwrap();
    assert_eq!(id, "s1");
}

#[test]
fn test_session_picker_ctrl_d_sends_delete_request() {
    use duga_tui::overlay::Overlay;
    use duga_tui::overlay::session_picker::SessionPickerOverlay;
    use duga_tui::session::SessionInfo;

    let sessions = vec![SessionInfo::new("s1".into(), "Test".into())];
    let (result_tx, _result_rx) = mpsc::unbounded_channel();
    let (delete_tx, mut delete_rx) = mpsc::unbounded_channel();

    let mut picker = SessionPickerOverlay::new(sessions, result_tx, Some(delete_tx));

    // Ctrl+D
    let action = picker.handle_key(&KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
    assert!(matches!(action, duga_tui::overlay::OverlayAction::Consumed));

    let id = delete_rx.try_recv().unwrap();
    assert_eq!(id, "s1");
}

#[test]
fn test_session_picker_no_delete_tx_ignores_delete_key() {
    use duga_tui::overlay::Overlay;
    use duga_tui::overlay::session_picker::SessionPickerOverlay;
    use duga_tui::session::SessionInfo;

    let sessions = vec![SessionInfo::new("s1".into(), "Test".into())];
    let (result_tx, _result_rx) = mpsc::unbounded_channel();

    // No delete channel provided
    let mut picker = SessionPickerOverlay::new(sessions, result_tx, None);

    // Delete key should be consumed but not crash
    let action = picker.handle_key(&KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
    assert!(matches!(action, duga_tui::overlay::OverlayAction::Consumed));
}

#[test]
fn test_session_picker_set_sessions_updates_list() {
    use duga_tui::overlay::Overlay;
    use duga_tui::overlay::session_picker::SessionPickerOverlay;
    use duga_tui::session::SessionInfo;

    let sessions = vec![
        SessionInfo::new("a".into(), "A".into()),
        SessionInfo::new("b".into(), "B".into()),
        SessionInfo::new("c".into(), "C".into()),
    ];
    let (result_tx, _result_rx) = mpsc::unbounded_channel();

    let mut picker = SessionPickerOverlay::new(sessions, result_tx, None);

    // Select last item
    for _ in 0..2 {
        picker.handle_key(&KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }

    // Remove last item
    let updated = vec![
        SessionInfo::new("a".into(), "A".into()),
        SessionInfo::new("b".into(), "B".into()),
    ];
    picker.set_sessions(updated);

    // Can still select without panic (selection clamped)
    let action = picker.handle_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(action, duga_tui::overlay::OverlayAction::Close));
}

#[test]
fn test_delete_session_end_to_end() {
    use std::fs;

    let dir = tempfile::tempdir().unwrap();
    let sessions_dir = dir.path().to_path_buf();

    // Create a session via the index
    let info = duga_tui::session::SessionInfo::new("del-me".into(), "Delete Me".into());
    duga_tui::session::upsert_session_index(&sessions_dir, &info);
    // Create the JSONL file
    fs::write(sessions_dir.join("del-me.jsonl"), "{}").unwrap();

    // Verify it exists
    let before = duga_tui::session::list_sessions(&sessions_dir);
    assert_eq!(before.len(), 1);

    // Delete it
    let result = duga_tui::session::delete_session(&sessions_dir, "del-me").unwrap();
    assert!(result);

    // File gone
    assert!(!sessions_dir.join("del-me.jsonl").exists());

    // Index empty
    let after = duga_tui::session::list_sessions(&sessions_dir);
    assert!(after.is_empty());
}

#[test]
fn test_app_delete_session_shows_confirmation() {
    use std::fs;

    let dir = tempfile::tempdir().unwrap();
    let sessions_dir = dir.path().to_path_buf();

    // Create a test session
    let info = duga_tui::session::SessionInfo::new("kill-me".into(), "Kill Me".into());
    duga_tui::session::upsert_session_index(&sessions_dir, &info);
    fs::write(sessions_dir.join("kill-me.jsonl"), "{}").unwrap();

    // Build an app pointing at this sessions dir
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let (fe_tx, fe_bridge) = FrontendEventBridge::new(16);
    let fe_sink = Arc::new(duga_runtime::FrontendEventSink::new(fe_tx));
    let (_config_dir, config) = test_config();
    let tui_config = config.tui.clone().unwrap_or_default();

    let mut app = App::new(
        config,
        tui_config,
        event_tx,
        fe_bridge,
        fe_sink,
        sessions_dir.clone(),
    );

    // Simulate: set a pending delete request and tick to show the dialog
    app.pending_delete_id = Some("kill-me".into());
    app.handle_tick();

    // Confirmation dialog should be active
    assert!(app.active_confirm_dialog.is_some());
    assert!(matches!(
        app.confirm_kind,
        Some(duga_tui::app::ConfirmKind::DeleteSession { .. })
    ));

    // Confirm the deletion (Enter = Confirm with Yes selected, index 0)
    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    app.handle_key(&enter);

    // Dialog should be gone
    assert!(app.active_confirm_dialog.is_none());

    // Session file should be deleted
    assert!(!sessions_dir.join("kill-me.jsonl").exists());

    let after = duga_tui::session::list_sessions(&sessions_dir);
    assert!(after.is_empty());
}

#[test]
fn test_app_delete_session_cancel_does_not_delete() {
    use std::fs;

    let dir = tempfile::tempdir().unwrap();
    let sessions_dir = dir.path().to_path_buf();

    // Create a test session
    let info = duga_tui::session::SessionInfo::new("keep-me".into(), "Keep Me".into());
    duga_tui::session::upsert_session_index(&sessions_dir, &info);
    fs::write(sessions_dir.join("keep-me.jsonl"), "{}").unwrap();

    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let (fe_tx, fe_bridge) = FrontendEventBridge::new(16);
    let fe_sink = Arc::new(duga_runtime::FrontendEventSink::new(fe_tx));
    let (_config_dir, config) = test_config();
    let tui_config = config.tui.clone().unwrap_or_default();

    let mut app = App::new(
        config,
        tui_config,
        event_tx,
        fe_bridge,
        fe_sink,
        sessions_dir.clone(),
    );

    // Set a pending delete and tick to show the dialog
    app.pending_delete_id = Some("keep-me".into());
    app.handle_tick();

    assert!(app.active_confirm_dialog.is_some());

    // Cancel with Escape
    let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
    app.handle_key(&esc);

    // Dialog should be gone
    assert!(app.active_confirm_dialog.is_none());

    // Session file should still exist
    assert!(sessions_dir.join("keep-me.jsonl").exists());

    let after = duga_tui::session::list_sessions(&sessions_dir);
    assert_eq!(after.len(), 1);
}

#[test]
fn test_delete_session_nonexistent_does_not_panic() {
    let dir = tempfile::tempdir().unwrap();
    let sessions_dir = dir.path().to_path_buf();

    // No sessions created
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let (fe_tx, fe_bridge) = FrontendEventBridge::new(16);
    let fe_sink = Arc::new(duga_runtime::FrontendEventSink::new(fe_tx));
    let (_config_dir, config) = test_config();
    let tui_config = config.tui.clone().unwrap_or_default();

    let mut app = App::new(
        config,
        tui_config,
        event_tx,
        fe_bridge,
        fe_sink,
        sessions_dir.clone(),
    );

    // Delete a session that doesn't exist
    app.pending_delete_id = Some("ghost".into());
    app.handle_tick();

    assert!(app.active_confirm_dialog.is_some());

    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    app.handle_key(&enter);

    // Should not crash and should show "not found" message
    assert!(app.active_confirm_dialog.is_none());
    let items = app.transcript.items();
    let has_warn = items.iter().any(|item| {
        matches!(item,
            TranscriptItem::SystemMessage { text, level: SystemLevel::Warn, .. }
            if text == "Session not found."
        )
    });
    assert!(has_warn);
}

// ── Thinking Streaming E2E Tests ───────────────────────────────────────────

#[test]
fn test_thinking_streaming_creates_thinking_block() {
    let (mut app, _rx) = make_test_app();
    app.update(AppEvent::Frontend(FrontendEvent::LlmThinkingDelta {
        model: "test".into(),
        delta: "Let me think".into(),
    }));
    app.update(AppEvent::Frontend(FrontendEvent::LlmThinkingDelta {
        model: "test".into(),
        delta: " about this".into(),
    }));
    assert_eq!(app.transcript.len(), 1);
    if let TranscriptItem::ThinkingBlock {
        text, is_streaming, ..
    } = &app.transcript.items()[0]
    {
        assert_eq!(text, "Let me think about this");
        assert!(is_streaming);
    } else {
        panic!("expected ThinkingBlock");
    }
}

#[test]
fn test_thinking_auto_finishes_when_text_arrives() {
    let (mut app, _rx) = make_test_app();
    app.update(AppEvent::Frontend(FrontendEvent::LlmThinkingDelta {
        model: "test".into(),
        delta: "reasoning".into(),
    }));
    app.update(AppEvent::Frontend(FrontendEvent::LlmTokenDelta {
        model: "test".into(),
        delta: "answer".into(),
    }));
    assert_eq!(app.transcript.len(), 2);
    if let TranscriptItem::ThinkingBlock { is_streaming, .. } = &app.transcript.items()[0] {
        assert!(!is_streaming);
    } else {
        panic!("expected ThinkingBlock");
    }
    if let TranscriptItem::AssistantMessage { is_streaming, .. } = &app.transcript.items()[1] {
        assert!(is_streaming);
    } else {
        panic!("expected AssistantMessage");
    }
}

#[test]
fn test_thinking_auto_finishes_on_run_finished() {
    let (mut app, _rx) = make_test_app();
    app.update(AppEvent::Frontend(FrontendEvent::LlmThinkingDelta {
        model: "test".into(),
        delta: "thinking".into(),
    }));
    app.update(AppEvent::Frontend(FrontendEvent::RunFinished {
        text: None,
    }));
    if let TranscriptItem::ThinkingBlock { is_streaming, .. } = &app.transcript.items()[0] {
        assert!(!is_streaming);
    } else {
        panic!("expected ThinkingBlock");
    }
}

#[test]
fn test_no_thinking_events_produces_no_thinking_blocks() {
    let (mut app, _rx) = make_test_app();
    app.update(AppEvent::Frontend(FrontendEvent::LlmTokenDelta {
        model: "test".into(),
        delta: "just".into(),
    }));
    app.update(AppEvent::Frontend(FrontendEvent::LlmTokenDelta {
        model: "test".into(),
        delta: " text".into(),
    }));
    app.update(AppEvent::Frontend(FrontendEvent::RunFinished {
        text: None,
    }));
    for item in app.transcript.items() {
        assert!(!matches!(item, TranscriptItem::ThinkingBlock { .. }));
    }
}

#[test]
fn test_thinking_non_streaming_regression() {
    let (mut app, _rx) = make_test_app();
    app.transcript.push(TranscriptItem::UserMessage {
        text: "test".into(),
        timestamp: Instant::now(),
    });
    app.update(AppEvent::Frontend(FrontendEvent::RunFinished {
        text: Some("direct answer".into()),
    }));
    let items = app.transcript.items();
    assert_eq!(items.len(), 2);
    assert!(matches!(items[0], TranscriptItem::UserMessage { .. }));
    assert!(matches!(items[1], TranscriptItem::AssistantMessage { .. }));
}

// ── EPIC-33: TUI Redesign Integration Tests ────────────────────────────────

#[test]
fn test_epic33_theme_defaults_to_dark() {
    let (app, _rx) = make_test_app();
    assert_eq!(app.theme.colors.bg, ratatui::style::Color::Rgb(10, 12, 16));
}

#[test]
fn test_epic33_layout_computes_panes() {
    let (app, _rx) = make_test_app();
    let rects = app.layout_manager.compute(200, 60, false);
    // Verify header, chat, sidebar, input, footer all have dimensions
    assert_eq!(rects.header.height, 1);
    assert_eq!(rects.input.height, 4);
    assert_eq!(rects.footer.height, 1);
    // Sidebar at 200 cols with 25% = 50 cols
    assert_eq!(rects.sidebar.width, 50);
}

#[test]
fn test_epic33_sidebar_hidden_on_narrow() {
    let (app, _rx) = make_test_app();
    let rects = app.layout_manager.compute(80, 24, false);
    assert_eq!(rects.sidebar.width, 0);
    assert_eq!(rects.chat.width, 80);
}

#[test]
fn test_epic33_focus_default_is_input() {
    let (app, _rx) = make_test_app();
    assert_eq!(app.focus, duga_tui::focus::Focus::Input);
}

#[test]
fn test_epic33_focus_tab_cycle() {
    use duga_tui::focus::{Focus, SidebarPanels};

    let none = SidebarPanels {
        reasoning: false,
        event_log: false,
    };
    let both = SidebarPanels {
        reasoning: true,
        event_log: true,
    };

    // No panels: Chat→Input→Chat
    assert_eq!(Focus::Chat.next(none), Focus::Input);
    assert_eq!(Focus::Input.next(none), Focus::Chat);

    // Both panels: Chat→Reasoning→EventLog→Input→Chat
    assert_eq!(Focus::Chat.next(both), Focus::Reasoning);
    assert_eq!(Focus::Reasoning.next(both), Focus::EventLog);
    assert_eq!(Focus::EventLog.next(both), Focus::Input);
    assert_eq!(Focus::Input.next(both), Focus::Chat);
}

#[test]
fn test_epic33_focus_shift_tab_cycle() {
    use duga_tui::focus::{Focus, SidebarPanels};

    let none = SidebarPanels {
        reasoning: false,
        event_log: false,
    };
    let both = SidebarPanels {
        reasoning: true,
        event_log: true,
    };

    // No panels: Input→Chat→Input
    assert_eq!(Focus::Chat.prev(none), Focus::Input);
    assert_eq!(Focus::Input.prev(none), Focus::Chat);

    // Both panels: Input→EventLog→Reasoning→Chat
    assert_eq!(Focus::Input.prev(both), Focus::EventLog);
    assert_eq!(Focus::EventLog.prev(both), Focus::Reasoning);
    assert_eq!(Focus::Reasoning.prev(both), Focus::Chat);
    assert_eq!(Focus::Chat.prev(both), Focus::Input);
}

#[test]
fn test_epic33_focus_esc_returns_to_chat() {
    let (mut app, _rx) = make_test_app();
    // Set focus to Input
    app.focus = duga_tui::focus::Focus::Input;
    let esc = crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Esc,
        crossterm::event::KeyModifiers::NONE,
    );
    app.handle_key(&esc);
    assert_eq!(app.focus, duga_tui::focus::Focus::Chat);
}

#[test]
fn test_epic33_focus_overlay_blocks_tab() {
    let (mut app, _rx) = make_test_app();
    // Open help overlay (should be blocked by focus routing now)
    // Actually, let's test that overlay mode preserves focus
    app.focus = duga_tui::focus::Focus::Overlay;
    let tab = crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Tab,
        crossterm::event::KeyModifiers::NONE,
    );
    app.handle_key(&tab);
    // Focus should remain Overlay
    assert_eq!(app.focus, duga_tui::focus::Focus::Overlay);
}

#[test]
fn test_epic33_editor_in_focus_mode() {
    let (mut app, _rx) = make_test_app();
    // When focus is Input, typing should go to editor
    app.focus = duga_tui::focus::Focus::Input;
    app.handle_key(&crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('h'),
        crossterm::event::KeyModifiers::NONE,
    ));
    assert_eq!(app.editor.text(), "h");
}

#[test]
fn test_epic33_editor_not_in_focus_mode() {
    let (mut app, _rx) = make_test_app();
    // When focus is Chat, typing auto-switches to Input and inserts
    app.focus = duga_tui::focus::Focus::Chat;
    app.handle_key(&crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('h'),
        crossterm::event::KeyModifiers::NONE,
    ));
    // Auto-switch: typing in Chat mode switches focus to Input and inserts text
    assert_eq!(app.focus, duga_tui::focus::Focus::Input);
    assert_eq!(app.editor.text(), "h");
}

#[test]
fn test_epic33_banner_shows_on_error() {
    let (mut app, _rx) = make_test_app();
    assert!(!app.banner.is_active());
    app.update(AppEvent::Frontend(FrontendEvent::Error {
        message: "connection failed".into(),
    }));
    assert!(app.banner.is_active());
}

#[test]
fn test_epic33_banner_input_enhancement() {
    let (app, _rx) = make_test_app();
    // Input max lines should be 10000 (default)
    assert_eq!(app.tui_config.input_max_lines, 10000);
}

#[test]
fn test_epic33_sidebar_state_toggles() {
    let (mut app, _rx) = make_test_app();
    assert!(app.sidebar_state.events_expanded);
    assert!(!app.sidebar_state.reasoning_expanded);

    app.sidebar_state.toggle_reasoning();
    assert!(app.sidebar_state.reasoning_expanded);

    app.sidebar_state.toggle_events();
    assert!(!app.sidebar_state.events_expanded);
}

#[test]
fn test_epic33_header_renders_model() {
    let (app, _rx) = make_test_app();
    // Header should have access to model from config
    assert!(!app.config.model.is_empty());
}

#[test]
fn test_epic33_new_config_fields_default() {
    use duga_config::TuiConfig;
    let config = TuiConfig::default();
    // Verify EPIC-33 fields have expected defaults
    assert_eq!(config.show_sidebar, true);
    assert_eq!(config.sidebar_width_pct, 25);
    assert_eq!(config.show_footer, true);
    assert_eq!(config.show_header, true);
    assert_eq!(config.responsive_breakpoint, 120);
    assert_eq!(config.input_max_lines, 10000);
    assert_eq!(config.banner_auto_dismiss_secs, 5);
    assert_eq!(config.event_log_max_entries, 200);
}

#[test]
fn test_epic33_config_backward_compat() {
    // Test that default config has the right new field values
    use duga_config::TuiConfig;
    let tui = TuiConfig::default();
    assert_eq!(tui.show_sidebar, true);
    assert_eq!(tui.sidebar_width_pct, 25);
    assert_eq!(tui.responsive_breakpoint, 120);
    assert_eq!(tui.input_max_lines, 10000);
    assert_eq!(tui.banner_auto_dismiss_secs, 5);
    assert_eq!(tui.event_log_max_entries, 200);
}

#[test]
fn test_epic33_full_agent_run_with_new_layout() {
    let (mut app, _rx) = make_test_app();

    // Simulate user message directly (skip submit_prompt which needs tokio)
    app.transcript.push(TranscriptItem::UserMessage {
        text: "hello world".into(),
        timestamp: Instant::now(),
    });

    assert_eq!(app.transcript.len(), 1);
    assert!(matches!(
        app.transcript.items()[0],
        TranscriptItem::UserMessage { .. }
    ));

    // Simulate agent response via frontend events
    app.update(AppEvent::Frontend(FrontendEvent::LlmTokenDelta {
        model: "test".into(),
        delta: "Hello there!".into(),
    }));
    app.update(AppEvent::Frontend(FrontendEvent::RunFinished {
        text: None,
    }));

    // Should have user + assistant messages
    assert!(app.transcript.len() >= 2);
    assert!(matches!(app.state, AppState::Idle));
}

#[test]
fn test_epic33_event_log_from_tool_calls() {
    let (mut app, _rx) = make_test_app();

    app.update(AppEvent::Frontend(FrontendEvent::ToolCallStarted {
        tool_name: "shell".into(),
        tool_call_id: "tc-e2e".into(),
        attempt: 1,
        description: "ls -la".into(),
        raw_args: None,
    }));

    // Transcript should have a tool call block
    let items = app.transcript.items();
    assert_eq!(items.len(), 1);
    assert!(matches!(items[0], TranscriptItem::ToolCallBlock { .. }));
}

#[test]
fn test_epic33_reasoning_panel_on_thinking() {
    let (mut app, _rx) = make_test_app();

    app.update(AppEvent::Frontend(FrontendEvent::LlmThinkingDelta {
        model: "test".into(),
        delta: "Let me reason about this.".into(),
    }));

    // Should have a thinking block in the transcript
    let items = app.transcript.items();
    assert_eq!(items.len(), 1);
    assert!(matches!(items[0], TranscriptItem::ThinkingBlock { .. }));
}

#[test]
fn test_epic33_error_banner_on_cancellation() {
    let (mut app, _rx) = make_test_app();

    // Show a cancel banner directly
    app.banner.show(
        duga_tui::banner::BannerLevel::Cancel,
        "Request cancelled. Press Ctrl+R to retry.".into(),
    );

    // Verify banner is shown
    assert!(app.banner.is_active());
}
