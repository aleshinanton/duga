//! Central application state for the duga TUI.
//!
//! Owns the transcript, editor, overlays, event bridge, and agent runtime.
//! Rendered by `terminal.rs` on each tick.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use duga_config::{Config, TuiConfig};
use duga_core::steering::{SteeringReceiver, SteeringSender};
use duga_core::LoopResult;
use duga_runtime::{
    ConfirmationDecision, FrontendEvent, FrontendEventBridge, FrontendEventSink,
};
use duga_sandbox::CancellationToken;
use duga_types::error::AgentError;
use duga_types::message::Message;
use tokio::sync::{mpsc, oneshot};

use crate::banner::ErrorBanner;
use crate::editor::{Editor, EditorAction};
use crate::event_log::{EventLog, LogEntry, LogLevel};
use crate::focus::{Focus, FocusRouter, FocusAction};
use crate::keybindings::{GlobalAction, Keybindings};
use crate::layout::LayoutManager;
use crate::overlay::{self, Overlay, OverlayManager};
use crate::reasoning_panel::ReasoningPanel;
use crate::session::{SessionInfo, delete_session, list_sessions, load_conversation_history,
    load_session_transcript, upsert_session_index, touch_session};
use crate::sidebar::SidebarState;
use crate::theme::Theme;
use crate::transcript::{SystemLevel, Transcript, TranscriptItem};

// ── App state machine ──────────────────────────────────────────────────────

/// High-level application state driving the rendering loop.
#[derive(Clone, Debug)]
pub enum AppState {
    /// No agent run active. The editor is accepting input.
    Idle,
    /// Agent is executing. Editor stays active for steering input.
    Running {
        run_id: u64,
        cancel_requested: bool,
        /// Name of the active loop (e.g. "simple_react", "problem_solving").
        loop_name: String,
    },
}

/// Distinguishes what the active confirmation dialog is for.
#[derive(Clone, Debug)]
pub enum ConfirmKind {
    /// Confirming a tool execution request from the agent runtime.
    ToolExecution,
    /// Confirming deletion of a session.
    DeleteSession { session_id: String },
}

// ── Internal events ────────────────────────────────────────────────────────

/// Events produced by the crossterm reader, frontend bridge, or spawned tasks.
#[derive(Clone, Debug)]
pub enum AppEvent {
    Crossterm(crossterm::event::Event),
    Frontend(FrontendEvent),
    Tick,
    /// An agent run has completed (or errored).
    RunFinished {
        run_id: u64,
        result: Result<LoopResult, AgentError>,
    },
}

// ── Application ────────────────────────────────────────────────────────────

pub struct App {
    pub state: AppState,
    pub config: Config,
    pub tui_config: TuiConfig,

    /// Current theme (from config).
    pub theme: Theme,

    /// Layout engine for computing pane rectangles.
    pub layout_manager: LayoutManager,

    /// Sidebar panel state.
    pub sidebar_state: SidebarState,

    /// Current keyboard focus.
    pub focus: Focus,
    /// Focus to restore after overlay closes.
    pub previous_focus: Focus,

    /// Error/warning/cancel banner.
    pub banner: ErrorBanner,

    /// Structured event log.
    pub event_log: EventLog,
    /// Reasoning panel state.
    pub reasoning_panel: ReasoningPanel,

    /// Scroll offset for sidebar panels.
    pub sidebar_scroll: usize,

    /// Transcript of the conversation.
    pub transcript: Transcript,
    /// Multi-line input editor.
    pub editor: Editor,
    /// Overlay manager for modals.
    pub overlays: OverlayManager,
    /// Parsed keybindings.
    pub keybindings: Keybindings,
    /// Tool event format from config.
    pub tool_event_format: duga_config::ToolEventFormat,

    /// Sender for AppEvent — used by spawned tasks.
    pub event_tx: mpsc::UnboundedSender<AppEvent>,
    /// Frontend bridge for receiving agent progress.
    pub fe_bridge: FrontendEventBridge,

    /// Frontend event sink.
    pub fe_sink: Arc<FrontendEventSink>,

    /// Next run id counter.
    next_run_id: u64,
    /// Whether the app should quit.
    should_quit: bool,
    /// Track the currently-streaming tool call ID to map to TranscriptItem.
    current_tool_call_id: Option<String>,
    /// Active cancellation token (set when run starts).
    active_cancel_token: Option<CancellationToken>,
    /// Steering sender for injecting guidance mid-run.
    active_steer: Option<SteeringSender>,
    /// Receiver for pending confirmation requests from the agent.
    confirmation_rx: Option<tokio::sync::mpsc::UnboundedReceiver<crate::confirmation::PendingConfirmation>>,
    /// Oneshot sender to respond to the active confirmation.
    pending_confirm_tx: Option<oneshot::Sender<ConfirmationDecision>>,
    /// Last known terminal dimensions.
    term_width: u16,
    term_height: u16,

    // ── Session management ──────────────────────────────────────────────
    pub sessions_dir: PathBuf,
    /// UUID of the current active session (set on first message submit).
    pub current_session_id: Option<String>,
    /// Conversation history to restore on the next agent run (from a resumed session).
    pending_history: Option<Vec<Message>>,
    /// Receives the session ID selected by the session picker overlay.
    session_result_rx: Option<mpsc::UnboundedReceiver<String>>,
    /// Receives delete requests from the session picker overlay.
    delete_session_rx: Option<mpsc::UnboundedReceiver<String>>,
    /// Pending session ID to delete (waiting for confirmation).
    pub pending_delete_id: Option<String>,
    /// What the active confirmation dialog is for.
    pub confirm_kind: Option<ConfirmKind>,
    /// Currently active confirmation dialog (handled outside overlay manager).
    pub active_confirm_dialog: Option<crate::overlay::confirmation::ConfirmationDialog>,
}

impl App {
    pub fn new(
        config: Config,
        tui_config: TuiConfig,
        event_tx: mpsc::UnboundedSender<AppEvent>,
        fe_bridge: FrontendEventBridge,
        fe_sink: Arc<FrontendEventSink>,
        sessions_dir: PathBuf,
    ) -> Self {
        let keybindings = Keybindings::from_config(&tui_config.keybindings);
        let tool_event_format = tui_config.tool_event_format.clone();
        let theme = Theme::from_config(&tui_config.theme.name);
        let layout_manager = LayoutManager::new(
            tui_config.sidebar_width_pct,
            tui_config.show_header,
            tui_config.show_sidebar,
            tui_config.show_footer,
            tui_config.responsive_breakpoint,
        );
        let banner_auto_dismiss_secs = tui_config.banner_auto_dismiss_secs;
        let event_log_max = tui_config.event_log_max_entries;

        Self {
            state: AppState::Idle,
            config,
            tui_config,
            theme,
            layout_manager,
            sidebar_state: SidebarState::new(),
            focus: Focus::default(),
            previous_focus: Focus::default(),
            banner: ErrorBanner::new(banner_auto_dismiss_secs),
            event_log: EventLog::new(event_log_max),
            reasoning_panel: ReasoningPanel::new(),
            sidebar_scroll: 0,
            transcript: Transcript::new(),
            editor: Editor::new(),
            overlays: OverlayManager::new(),
            keybindings,
            tool_event_format,
            event_tx,
            fe_bridge,
            fe_sink,
            next_run_id: 0,
            should_quit: false,
            current_tool_call_id: None,
            active_cancel_token: None,
            active_steer: None,
            confirmation_rx: None,
            active_confirm_dialog: None,
            pending_confirm_tx: None,
            term_width: 80,
            term_height: 24,
            sessions_dir,
            current_session_id: None,
            pending_history: None,
            session_result_rx: None,
            delete_session_rx: None,
            pending_delete_id: None,
            confirm_kind: None,
        }
    }

    /// Push an overlay and transition focus to Overlay mode.
    fn push_overlay(&mut self, overlay: Box<dyn Overlay>) {
        if !self.overlays.has_overlay() {
            self.previous_focus = self.focus;
            self.focus = Focus::Overlay;
        }
        self.overlays.push(overlay);
    }

    /// Pop the topmost overlay. When no overlays remain, restore focus.
    fn pop_overlay(&mut self) {
        self.overlays.pop();
        if !self.overlays.has_overlay() {
            self.focus = self.previous_focus;
        }
    }

    /// Show the sidebar as a floating overlay (for narrow terminals).
    pub fn show_sidebar_overlay(&mut self) {
        // This is a lightweight overlay that just signals the sidebar
        // should be displayed. In practice, the sidebar panel content
        // is still rendered in the main layout when overlay mode is active.
        // For now, we use a placeholder overlay.
        // Full implementation would create a dedicated SidebarOverlay.
        self.focus = Focus::Sidebar;
        self.sidebar_state.toggle_reasoning();
    }

    /// Cycle the thinking/reasoning effort level through Off → Low → Medium → High → Off.
    pub fn cycle_thinking_level(&mut self) {
        use duga_config::ThinkingLevel;
        self.config.thinking_level = match self.config.thinking_level {
            ThinkingLevel::Off => ThinkingLevel::Low,
            ThinkingLevel::Low => ThinkingLevel::Medium,
            ThinkingLevel::Medium => ThinkingLevel::High,
            ThinkingLevel::High => ThinkingLevel::Off,
        };
        self.event_log.push(LogEntry::new(
            LogLevel::Info,
            "⚙",
            format!("Thinking: {:?}", self.config.thinking_level),
        ));
    }

    /// Handle a single `AppEvent` and update internal state.
    pub fn update(&mut self, event: AppEvent) {
        // Poll for pending confirmation requests before processing events.
        self.poll_confirmations();

        match event {
            AppEvent::Crossterm(ct_event) => self.handle_crossterm(ct_event),
            AppEvent::Frontend(fe) => self.handle_frontend_event(fe),
            AppEvent::Tick => self.handle_tick(),
            AppEvent::RunFinished { run_id, result } => {
                self.handle_run_finished(run_id, result);
            }
        }
    }

    fn handle_crossterm(&mut self, event: crossterm::event::Event) {
        match event {
            crossterm::event::Event::Key(key) => self.handle_key(&key),
            crossterm::event::Event::Resize(w, h) => {
                self.term_width = w;
                self.term_height = h;
            }
            crossterm::event::Event::Paste(text) => {
                self.editor.insert_text(&text);
            }
            crossterm::event::Event::Mouse(mouse) => self.handle_mouse(&mouse),
            crossterm::event::Event::FocusGained => {
                // Cursor visibility handled by terminal
            }
            crossterm::event::Event::FocusLost => {
                // Cursor visibility handled by terminal
            }
        }
    }

    fn handle_mouse(&mut self, event: &crossterm::event::MouseEvent) {
        use crossterm::event::MouseEventKind;
        // Small fixed step: trackpads fire many events per second (30+),
        // so 3 lines per tick yields smooth, fast scrolling naturally.
        // Mouse wheels fire fewer events and feel best with PageUp/PageDown.
        const MOUSE_SCROLL_LINES: usize = 3;
        match event.kind {
            MouseEventKind::ScrollUp => {
                self.transcript.scroll_mut().scroll_up(MOUSE_SCROLL_LINES);
            }
            MouseEventKind::ScrollDown => {
                self.transcript.scroll_mut().scroll_down(MOUSE_SCROLL_LINES);
            }
            _ => {}
        }
    }

    /// Half a transcript page, with a floor of 1.
    fn scroll_page_amount(&self) -> usize {
        let transcript_height = (self.term_height.saturating_sub(5) as usize).max(1);
        (transcript_height / 2).max(1)
    }

    /// Log reasoning block completion to the event log.
    fn log_reasoning_completed(&mut self) {
        if let Some((wc, dur)) = self.reasoning_panel.last_block_stats() {
            self.event_log.push(LogEntry::new(
                LogLevel::Success,
                "🧠",
                format!("Reasoning done — {wc} words, {dur}"),
            ));
        }
    }

/// Returns true if the key event represents a typing/editing action
/// (characters, backspace, delete, arrows, home, end) that should
/// auto-switch focus to the Input pane.
fn is_typing_key(key: &KeyEvent) -> bool {
    match key.code {
        KeyCode::Char(_) if key.modifiers == KeyModifiers::NONE || key.modifiers == KeyModifiers::SHIFT => true,
        KeyCode::Backspace | KeyCode::Delete | KeyCode::Enter => true,
        KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down => true,
        KeyCode::Home | KeyCode::End => true,
        _ => false,
    }
}

    pub fn handle_key(&mut self, key: &KeyEvent) {
        // 0. Confirmation dialog takes priority over everything.
        if let Some(ref mut dialog) = self.active_confirm_dialog {
            let action = dialog.handle_key(key);
            if matches!(action, overlay::OverlayAction::Close | overlay::OverlayAction::Consumed) {
                self.process_confirm_result();
                return;
            }
        }

        // 1. Overlays get first crack
        if self.overlays.has_overlay() {
            let consumed = self.overlays.handle_key(key);
            // If overlay was closed, restore focus
            if !self.overlays.has_overlay() && self.focus == Focus::Overlay {
                self.focus = self.previous_focus;
            }
            if consumed {
                return;
            }
            // Escape always closes overlays
            if key.code == KeyCode::Esc {
                self.pop_overlay();
                return;
            }
        }

        // 1.5. Focus routing (Tab, dedicated shortcuts, Esc)
        let sidebar_visible = self
            .layout_manager
            .is_sidebar_visible(self.term_width);
        let has_overlay = self.overlays.has_overlay();
        let is_running = matches!(self.state, AppState::Running { .. });

        match FocusRouter::route(
            key,
            &mut self.focus,
            sidebar_visible,
            has_overlay,
            is_running,
        ) {
            FocusAction::Consumed => return,
            FocusAction::PassToOverlay => {
                // Key should be handled by overlay (already done above when has_overlay is true).
                // If overlay handler already consumed the key, we wouldn't be here.
                // So pass through to global keybindings.
            }
            FocusAction::ToggleReasoning => {
                self.sidebar_state.toggle_reasoning();
                return;
            }
            FocusAction::ToggleEvents => {
                self.sidebar_state.toggle_events();
                return;
            }
            FocusAction::CycleThinkingEffort => {
                self.cycle_thinking_level();
                return;
            }
            FocusAction::PassThrough => {}
        }

        // 2. Global keybindings
        let action = self.keybindings.match_key(key);
        match action {
            GlobalAction::Submit => {
                self.submit_prompt();
                return;
            }
            GlobalAction::Cancel => {
                self.cancel_run();
                return;
            }
            GlobalAction::Quit => {
                if matches!(self.state, AppState::Idle) {
                    self.should_quit = true;
                }
                return;
            }
            GlobalAction::Help => {
                self.push_overlay(Box::new(overlay::help::HelpOverlay::new(
                    &self.keybindings,
                    &self.theme,
                )));
                return;
            }
            GlobalAction::Search => {
                let mut search = overlay::search::SearchOverlay::new();
                search.search(&self.transcript);
                self.push_overlay(Box::new(search));
                return;
            }
            GlobalAction::ScrollUp => {
                let amount = self.scroll_page_amount();
                self.transcript.scroll_mut().scroll_up(amount);
                return;
            }
            GlobalAction::ScrollDown => {
                let amount = self.scroll_page_amount();
                self.transcript.scroll_mut().scroll_down(amount);
                return;
            }
            GlobalAction::ToggleTool => {
                // Toggle tool call blocks
                let any_expanded = self
                    .transcript
                    .items()
                    .iter()
                    .any(|item| {
                        matches!(item, TranscriptItem::ToolCallBlock { is_expanded: true, .. })
                    });
                let target = !any_expanded;
                let to_toggle: Vec<usize> = self
                    .transcript
                    .items()
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, item)| {
                        if let TranscriptItem::ToolCallBlock { is_expanded, .. } = item {
                            if *is_expanded != target {
                                return Some(idx);
                            }
                        }
                        None
                    })
                    .collect();
                for idx in to_toggle {
                    self.transcript.toggle_tool_expand(idx);
                }
                return;
            }
            GlobalAction::ToggleThink => {
                // Toggle thinking blocks (Ctrl+O)
                let think_ids: Vec<usize> = self
                    .transcript
                    .items()
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, item)| {
                        if matches!(item, TranscriptItem::ThinkingBlock { .. }) {
                            Some(idx)
                        } else {
                            None
                        }
                    })
                    .collect();
                for idx in think_ids {
                    self.transcript.advance_thinking_scroll(idx, 8);
                }

                // Also toggle tool call blocks (Ctrl+O expands/collapses both)
                let any_expanded = self
                    .transcript
                    .items()
                    .iter()
                    .any(|item| {
                        matches!(item, TranscriptItem::ToolCallBlock { is_expanded: true, .. })
                    });
                let target = !any_expanded;
                let to_toggle: Vec<usize> = self
                    .transcript
                    .items()
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, item)| {
                        if let TranscriptItem::ToolCallBlock { is_expanded, .. } = item {
                            if *is_expanded != target {
                                return Some(idx);
                            }
                        }
                        None
                    })
                    .collect();
                for idx in to_toggle {
                    self.transcript.toggle_tool_expand(idx);
                }
                return;
            }
            GlobalAction::Steer => {
                if matches!(self.state, AppState::Running { .. }) {
                    self.send_steering();
                }
                return;
            }
            GlobalAction::None => {}
        }

        // 3. Additional global keys not in config
        match key {
            // Enter dismisses banner (when no overlay)
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                if self.banner.is_active() && !self.overlays.has_overlay() {
                    self.banner.dismiss();
                    return;
                }
            }
            // Ctrl+S: open session picker
            KeyEvent {
                code: KeyCode::Char('s'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => {
                let (tx, rx) = mpsc::unbounded_channel();
                self.session_result_rx = Some(rx);
                let (delete_tx, delete_rx) = mpsc::unbounded_channel();
                self.delete_session_rx = Some(delete_rx);
                let sessions = list_sessions(&self.sessions_dir);
                self.push_overlay(Box::new(
                    overlay::session_picker::SessionPickerOverlay::new(
                        sessions, tx, Some(delete_tx),
                    ),
                ));
                return;
            }
            // Ctrl+L: clear transcript
            KeyEvent {
                code: KeyCode::Char('l'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => {
                self.transcript.clear();
                self.event_log.clear();
                self.reasoning_panel.reset();
                self.transcript.push(TranscriptItem::SystemMessage {
                    text: "Transcript cleared.".into(),
                    level: SystemLevel::Info,
                    timestamp: Instant::now(),
                });
                return;
            }
            // Ctrl+Q: quit
            KeyEvent {
                code: KeyCode::Char('q'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => {
                self.should_quit = true;
                return;
            }
            // Escape: close overlays / cancel when running
            KeyEvent {
                code: KeyCode::Esc,
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                if let AppState::Running { .. } = &self.state {
                    self.cancel_run();
                }
                return;
            }
            _ => {}
        }

        // 4. Focus-aware key routing for editor + chat scrolling
        if !self.overlays.has_overlay() {
            match self.focus {
                Focus::Chat => {
                    // j/k scroll in Chat mode
                    match key.code {
                        KeyCode::Char('j') if key.modifiers == KeyModifiers::NONE => {
                            self.transcript.scroll_mut().scroll_down(3);
                            return;
                        }
                        KeyCode::Char('k') if key.modifiers == KeyModifiers::NONE => {
                            self.transcript.scroll_mut().scroll_up(3);
                            return;
                        }
                        KeyCode::Char('g') if key.modifiers == KeyModifiers::NONE => {
                            self.transcript.scroll_mut().scroll_to_bottom();
                            return;
                        }
                        _ => {}
                    }
                    // Any other character key: auto-switch to Input and insert
                    if Self::is_typing_key(key) {
                        self.focus = Focus::Input;
                        self.editor.handle_key(key);
                        return;
                    }
                }
                Focus::Input => {
                    // All keys go to editor
                    match self.editor.handle_key(key) {
                        EditorAction::Submit => {
                            if matches!(self.state, AppState::Running { .. }) {
                                self.send_steering();
                            } else {
                                self.submit_prompt();
                            }
                        }
                        EditorAction::Ignored => {}
                        EditorAction::Consumed => {}
                    }
                }
                Focus::Sidebar => {
                    // j/k scroll event log, g/G top/bottom
                    match key.code {
                        KeyCode::Char('j') if key.modifiers == KeyModifiers::NONE => {
                            self.sidebar_scroll = self.sidebar_scroll.saturating_add(1);
                            return;
                        }
                        KeyCode::Char('k') if key.modifiers == KeyModifiers::NONE => {
                            self.sidebar_scroll = self.sidebar_scroll.saturating_sub(1);
                            return;
                        }
                        KeyCode::Char('g') if key.modifiers == KeyModifiers::NONE => {
                            self.sidebar_scroll = 0; // bottom
                            return;
                        }
                        KeyCode::Char('G') if key.modifiers == KeyModifiers::SHIFT => {
                            self.sidebar_scroll = usize::MAX; // top
                            return;
                        }
                        _ => {}
                    }
                    // Any typing key: auto-switch to Input
                    if Self::is_typing_key(key) {
                        self.focus = Focus::Input;
                        self.editor.handle_key(key);
                        return;
                    }
                }
                Focus::Overlay => {
                    // Overlay handles its own keys; nothing to do here
                }
            }
        }
    }

    fn handle_frontend_event(&mut self, event: FrontendEvent) {
        match event {
            FrontendEvent::RunStarted { .. } => {
                // User message already added by submit_prompt().
                // Streaming tokens will arrive via LlmTokenDelta.
            }
            FrontendEvent::RunFinished { text } => {
                // Auto-finish any lingering thinking block
                if self.transcript.thinking_is_streaming() {
                    self.transcript.finish_thinking();
                    self.reasoning_panel.finish_current_block();
                    self.log_reasoning_completed();
                }
                // Check if we had streaming output before clearing it.
                let had_streaming = self.transcript.streaming_index().is_some();
                self.transcript.finish_streaming();

                // Push the final answer unless it's a duplicate of the
                // just-finished streaming output (non-delegation case).
                // Delegation produces a different final text that must be shown.
                if let Some(t) = text {
                    if !t.is_empty() {
                        let is_duplicate = had_streaming
                            && self.transcript.items().last()
                                .map(|item| matches!(item,
                                    TranscriptItem::AssistantMessage { text: existing, .. }
                                    if existing == &t))
                                .unwrap_or(false);

                        if !is_duplicate {
                            self.transcript.push(TranscriptItem::AssistantMessage {
                                text: t,
                                timestamp: Instant::now(),
                                is_streaming: false,
                            });
                        }
                    }
                }
            }
            FrontendEvent::ToolCallStarted {
                tool_name,
                tool_call_id,
                description,
                raw_args,
                ..
            } => {
                // Auto-finish thinking if it was streaming
                if self.transcript.thinking_is_streaming() {
                    self.transcript.finish_thinking();
                    self.reasoning_panel.finish_current_block();
                    self.log_reasoning_completed();
                }
                // Log to event log
                self.event_log.push(LogEntry::new(
                    LogLevel::Info,
                    "●",
                    format!("Tool: {tool_name} started — {description}"),
                ));
                let is_expanded = match self.tool_event_format {
                    duga_config::ToolEventFormat::Full => true,
                    duga_config::ToolEventFormat::Collapsed => false,
                    duga_config::ToolEventFormat::FinalOnly => false, // Hide until finished
                };
                self.current_tool_call_id = Some(tool_call_id.clone());
                if self.tool_event_format != duga_config::ToolEventFormat::FinalOnly {
                    self.transcript.push(TranscriptItem::ToolCallBlock {
                        tool_call_id,
                        tool_name,
                        description,
                        raw_args,
                        is_running: true,
                        is_success: None,
                        is_expanded,
                        timestamp: Instant::now(),
                        output: None,
                    });
                }
            }
            FrontendEvent::ToolCallFinished {
                tool_name,
                tool_call_id,
                success,
                description,
                output,
                ..
            } => {
                // Log to event log
                let (level, icon) = if success {
                    (LogLevel::Success, "✓")
                } else {
                    (LogLevel::Error, "✗")
                };
                self.event_log.push(LogEntry::new(
                    level,
                    icon,
                    format!("Tool: {tool_name} {} — {description}",
                        if success { "ok" } else { "failed" }),
                ));

                // Update existing tool block if found
                self.transcript.update_tool_call(&tool_call_id, success, output.clone());
                // If FinalOnly mode and tool wasn't shown during running, show now
                if self.tool_event_format == duga_config::ToolEventFormat::FinalOnly {
                    // Check if this tool was added during ToolCallStarted
                    let found = self.transcript.find_tool_call(&tool_call_id);
                    if found.is_none() {
                        self.transcript.push(TranscriptItem::ToolCallBlock {
                            tool_call_id,
                            tool_name,
                            description,
                            raw_args: None,
                            is_running: false,
                            is_success: Some(success),
                            is_expanded: !success, // Expand on failure
                            timestamp: Instant::now(),
                            output,
                        });
                    }
                }
                self.current_tool_call_id = None;
            }
            FrontendEvent::LlmTokenDelta { delta, .. } => {
                if self.transcript.thinking_is_streaming() {
                    self.transcript.finish_thinking();
                    self.reasoning_panel.finish_current_block();
                    self.log_reasoning_completed();
                }
                self.transcript.append_to_streaming(&delta);
            }
            FrontendEvent::LlmThinkingDelta { delta, .. } => {
                if self.tui_config.show_thinking {
                    // Log start of a new reasoning block
                    if !self.transcript.thinking_is_streaming() {
                        self.event_log.push(LogEntry::new(
                            LogLevel::Info,
                            "🧠",
                            "Reasoning started".into(),
                        ));
                    }
                    // Track reasoning in the reasoning panel (sidebar)
                    self.reasoning_panel.add_block(&delta);
                    self.transcript.append_to_thinking(&delta);
                }
            }
            FrontendEvent::Error { message } => {
                self.event_log.push(LogEntry::new(
                    LogLevel::Error,
                    "✗",
                    message.clone(),
                ));
                self.transcript.push(TranscriptItem::SystemMessage {
                    text: message.clone(),
                    level: SystemLevel::Error,
                    timestamp: Instant::now(),
                });
                self.banner.show(crate::banner::BannerLevel::Error, message);
            }
            FrontendEvent::LoopDelegated {
                from,
                to,
                reason,
                depth,
            } => {
                self.event_log.push(LogEntry::new(
                    LogLevel::Info,
                    "→",
                    format!("{from} → {to}: {reason}"),
                ));
                // Update status bar to show the new loop.
                if let AppState::Running { ref mut loop_name, .. } = self.state {
                    *loop_name = to.clone();
                }
                self.transcript.push(TranscriptItem::DelegationNotice {
                    from,
                    to,
                    reason,
                    depth,
                    timestamp: Instant::now(),
                });
            }
            FrontendEvent::MemoryCompressed {
                before_tokens,
                after_tokens,
            } => {
                self.event_log.push(LogEntry::new(
                    LogLevel::Warning,
                    "⚠",
                    format!("Memory: {before_tokens}→{after_tokens} tokens"),
                ));
                self.transcript.push(TranscriptItem::MemoryNotice {
                    before_tokens,
                    after_tokens,
                    timestamp: Instant::now(),
                });
            }
        }
    }

    pub fn handle_tick(&mut self) {
        // Auto-dismiss banner
        self.banner.tick();

        // Poll for a session selection from the picker overlay.
        if let Some(ref mut rx) = self.session_result_rx {
            if let Ok(id) = rx.try_recv() {
                self.session_result_rx = None;
                self.switch_session(id);
            }
        }

        // Poll for delete requests from the session picker overlay.
        if let Some(ref mut rx) = self.delete_session_rx {
            if let Ok(id) = rx.try_recv() {
                self.pending_delete_id = Some(id);
            }
        }

        // Show delete confirmation dialog if a delete is pending and no dialog is active.
        if self.pending_delete_id.is_some()
            && self.active_confirm_dialog.is_none()
            && self.pending_confirm_tx.is_none()
        {
            self.show_delete_confirmation();
        }
    }

    fn handle_run_finished(&mut self, run_id: u64, result: Result<LoopResult, AgentError>) {
        if let AppState::Running {
            run_id: active_id, ..
        } = &self.state
        {
            if *active_id != run_id {
                return;
            }
        }

        // Frontend events (LlmTokenDelta, RunFinished) already handled
        // transcript updates. Only push an error notice here if needed.
        if let Err(err) = &result {
            // Error notice only if RunFinished didn't already show one
            self.transcript.push(TranscriptItem::SystemMessage {
                text: format!("Agent error: {err}"),
                level: SystemLevel::Error,
                timestamp: Instant::now(),
            });
        }
        self.state = AppState::Idle;
        self.active_cancel_token = None;
        self.active_steer = None;
        self.confirmation_rx = None;
        self.active_confirm_dialog = None;
        self.pending_confirm_tx = None;
        self.confirm_kind = None;

        // Load conversation history for the next run in this session so
        // that subsequent messages carry the full context.
        if let Some(ref id) = self.current_session_id {
            let path = self.sessions_dir.join(format!("{id}.jsonl"));
            let history = load_conversation_history(
                &path,
                self.config.memory.context_window_size,
                self.config.memory.max_context_tokens,
            );
            self.pending_history = history;
        }
    }

    // ── Session management ──────────────────────────────────────────────

    /// Resume a session: clear transcript, load history, set as active session.
    fn switch_session(&mut self, id: String) {
        let path = self.sessions_dir.join(format!("{id}.jsonl"));

        // Reconstruct display transcript from the session file.
        let mut new_transcript = Transcript::new();
        for item in load_session_transcript(&path) {
            new_transcript.push(item);
        }

        // Find the session title from the index for the notice.
        let sessions = list_sessions(&self.sessions_dir);
        let title = sessions
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.title.as_str())
            .unwrap_or("session");
        new_transcript.push(TranscriptItem::SystemMessage {
            text: format!("── Resumed: {title}"),
            level: SystemLevel::Info,
            timestamp: Instant::now(),
        });

        self.transcript = new_transcript;
        self.scroll_to_bottom();

        // Populate reasoning panel with thinking blocks from the session.
        self.reasoning_panel.load_from_transcript_items(self.transcript.items());

        // Load conversation history for memory restoration on the next run.
        let history = load_conversation_history(
            &path,
            self.config.memory.context_window_size,
            self.config.memory.max_context_tokens,
        );
        self.pending_history = history;
        self.current_session_id = Some(id.clone());

        // Touch the last_active timestamp in the index.
        touch_session(&self.sessions_dir, &id);
    }

    /// Scroll transcript to the most recent content.
    fn scroll_to_bottom(&mut self) {
        self.transcript.scroll_mut().scroll_to_bottom();
    }

    // ── Run management ──────────────────────────────────────────────────

    /// Submit the current prompt and start an agent run.
    pub fn submit_prompt(&mut self) {
        let task = self.editor.take_text();
        if task.trim().is_empty() {
            return;
        }

        // Add user message to transcript
        self.transcript.push(TranscriptItem::UserMessage {
            text: task.clone(),
            timestamp: Instant::now(),
        });

        // Assign a session if this is the first message.
        if self.current_session_id.is_none() {
            let id = uuid::Uuid::new_v4().to_string();
            let title: String = task.chars().take(60).collect();
            let info = SessionInfo::new(id.clone(), title);
            upsert_session_index(&self.sessions_dir, &info);
            self.current_session_id = Some(id);
        } else {
            // Update last_active for the current session.
            if let Some(ref id) = self.current_session_id.clone() {
                touch_session(&self.sessions_dir, id);
            }
        }

        let session_path = self.current_session_id.as_ref().map(|id| {
            self.sessions_dir.join(format!("{id}.jsonl"))
        });
        let history = self.pending_history.take();

        let cancellation = CancellationToken::new();
        let _cancel_clone = cancellation.clone();
        self.active_cancel_token = Some(cancellation.clone());

        let run_id = self.next_run_id;
        self.next_run_id += 1;

        // Determine loop name for the status bar.
        let loop_name = self.tui_config.default_loop.clone();

        self.state = AppState::Running {
            run_id,
            cancel_requested: false,
            loop_name,
        };
        // Create steering channel for mid-run guidance.
        let (steer_tx, steer_rx) = tokio::sync::mpsc::unbounded_channel();
        self.active_steer = Some(SteeringSender::new(steer_tx));

        // Create confirmation channel for tool execution approval.
        let (confirm_tx, confirm_rx) = tokio::sync::mpsc::unbounded_channel();
        self.confirmation_rx = Some(confirm_rx);

        // Build the runtime and run the agent in a tokio task.
        let config = self.config.clone();
        let fe_sink = self.fe_sink.clone();
        let event_tx = self.event_tx.clone();

        tokio::spawn(async move {
            let result = crate::runtime::run_agent(
                &config,
                task,
                fe_sink,
                cancellation,
                Some(SteeringReceiver::new(steer_rx)),
                Some(confirm_tx),
                session_path,
                history,
            )
            .await;

            let _ = event_tx.send(AppEvent::RunFinished {
                run_id,
                result,
            });
        });
    }

    /// Cancel the currently running agent.
    pub fn cancel_run(&mut self) {
        if let AppState::Running {
            cancel_requested, ..
        } = &self.state
        {
            if !cancel_requested {
                if let Some(ref token) = self.active_cancel_token {
                    token.cancel();
                }
                self.transcript.push(TranscriptItem::SystemMessage {
                    text: "Cancelling…".into(),
                    level: SystemLevel::Warn,
                    timestamp: Instant::now(),
                });
                self.banner.show(
                    crate::banner::BannerLevel::Cancel,
                    "Request cancelled. Press Ctrl+R to retry.".into(),
                );
                let (run_id, loop_name) = if let AppState::Running { run_id, loop_name, .. } = &self.state {
                    (*run_id, loop_name.clone())
                } else {
                    return;
                };
                self.state = AppState::Running {
                    run_id,
                    cancel_requested: true,
                    loop_name,
                };
            }
        }
    }

    /// Send the current editor text as a steering guidance message.
    fn send_steering(&mut self) {
        let text = self.editor.take_text();
        if text.trim().is_empty() {
            return;
        }

        if let Some(ref steer) = self.active_steer {
            match steer.guide(&text) {
                Ok(()) => {
                    self.transcript.push(TranscriptItem::SystemMessage {
                        text: format!("Steering sent: {text}"),
                        level: SystemLevel::Info,
                        timestamp: Instant::now(),
                    });
                }
                Err(e) => {
                    self.transcript.push(TranscriptItem::SystemMessage {
                        text: format!("Steering failed: {e}"),
                        level: SystemLevel::Error,
                        timestamp: Instant::now(),
                    });
                }
            }
        } else {
            // No active steering channel — this shouldn't happen.
            self.transcript.push(TranscriptItem::SystemMessage {
                text: "No active run to steer.".into(),
                level: SystemLevel::Warn,
                timestamp: Instant::now(),
            });
        }
    }

    /// Poll for pending confirmation requests and show the dialog.
    fn poll_confirmations(&mut self) {
        // Don't show a new confirmation if one is already active.
        if self.active_confirm_dialog.is_some() {
            return;
        }

        if let Some(ref mut rx) = self.confirmation_rx {
            while let Ok(pending) = rx.try_recv() {
                use crate::overlay::confirmation::ConfirmationDialog;
                let dialog = ConfirmationDialog::yes_no(
                    "Confirm Tool Execution",
                    pending.request.label.clone(),
                );
                self.active_confirm_dialog = Some(dialog);
                self.pending_confirm_tx = Some(pending.response_tx);
                self.confirm_kind = Some(ConfirmKind::ToolExecution);
                break; // Only handle one at a time
            }
        }
    }

    /// Show a confirmation dialog for session deletion.
    fn show_delete_confirmation(&mut self) {
        let session_id = match self.pending_delete_id.take() {
            Some(id) => id,
            None => return,
        };

        let sessions = list_sessions(&self.sessions_dir);
        let title = sessions
            .iter()
            .find(|s| s.id == session_id)
            .map(|s| s.title.as_str())
            .unwrap_or("session");

        use crate::overlay::confirmation::ConfirmationDialog;
        let dialog = ConfirmationDialog::yes_no(
            "Delete Session",
            format!("Delete session \"{title}\"? This cannot be undone."),
        );
        self.active_confirm_dialog = Some(dialog);
        self.confirm_kind = Some(ConfirmKind::DeleteSession {
            session_id,
        });
    }

    /// Check if the confirmation dialog was dismissed and send the response.
    fn process_confirm_result(&mut self) {
        if let Some(ref mut dialog) = self.active_confirm_dialog {
            if let Some(result) = dialog.take_result() {
                let kind = self.confirm_kind.take();
                match kind {
                    Some(ConfirmKind::ToolExecution) => {
                        use crate::overlay::confirmation::ConfirmationResult;
                        let decision = match result {
                            ConfirmationResult::Confirmed(idx) if idx == 0 => {
                                ConfirmationDecision::Approved
                            }
                            _ => ConfirmationDecision::Denied,
                        };
                        if let Some(tx) = self.pending_confirm_tx.take() {
                            let _ = tx.send(decision);
                        }
                    }
                    Some(ConfirmKind::DeleteSession { session_id }) => {
                        use crate::overlay::confirmation::ConfirmationResult;
                        if matches!(result, ConfirmationResult::Confirmed(0)) {
                            let sessions_dir = self.sessions_dir.clone();
                            match delete_session(&sessions_dir, &session_id) {
                                Ok(true) => {
                                    self.transcript.push(TranscriptItem::SystemMessage {
                                        text: "Session deleted.".into(),
                                        level: SystemLevel::Info,
                                        timestamp: Instant::now(),
                                    });
                                }
                                Ok(false) => {
                                    self.transcript.push(TranscriptItem::SystemMessage {
                                        text: "Session not found.".into(),
                                        level: SystemLevel::Warn,
                                        timestamp: Instant::now(),
                                    });
                                }
                                Err(e) => {
                                    self.transcript.push(TranscriptItem::SystemMessage {
                                        text: format!("Failed to delete session: {e}"),
                                        level: SystemLevel::Error,
                                        timestamp: Instant::now(),
                                    });
                                }
                            }
                        }
                        // Close the session picker overlay after delete confirmation.
                        if self.overlays.has_overlay() {
                            self.pop_overlay();
                        }
                        // Clean up the delete channel since the overlay was closed.
                        self.delete_session_rx = None;
                    }
                    None => {
                        // No kind set — shouldn't happen, but handle gracefully.
                    }
                }
                self.active_confirm_dialog = None;
            }
        }
    }

    /// Check if the app should exit.
    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    // ── Rendering ───────────────────────────────────────────────────────

    /// Render the entire TUI.
    pub fn render(&mut self, frame: &mut ratatui::Frame) {
        let area = frame.area();

        // Compute pane rects from the layout manager
        let mut pane_rects = self.layout_manager.compute(
            area.width,
            area.height,
            self.banner.is_active(),
        );

        // Collapse sidebar when both panels are hidden — let chat expand into that space
        if !self.sidebar_state.has_content() && pane_rects.sidebar.width > 0 {
            pane_rects.chat.width += pane_rects.sidebar.width;
            pane_rects.sidebar.width = 0;
        }

        // ── Status bar (header area) ──────────────────────────────────
        if pane_rects.header.height > 0 {
            self.render_header(frame, pane_rects.header);
        }

        // ── Transcript ────────────────────────────────────────────────
        self.render_transcript(frame, pane_rects.chat);

        // ── Sidebar ───────────────────────────────────────────────────
        if pane_rects.sidebar.width > 0 {
            crate::sidebar::SidebarView::render(
                pane_rects.sidebar,
                frame.buffer_mut(),
                &self.sidebar_state,
                &self.reasoning_panel,
                &self.event_log,
                self.sidebar_scroll,
                &self.theme,
            );
        }

        // ── Banner ────────────────────────────────────────────────────
        if pane_rects.banner.height > 0 {
            self.banner.render(pane_rects.banner, frame.buffer_mut(), &self.theme);
        }

        // ── Editor (input area) ───────────────────────────────────────
        self.render_editor(frame, pane_rects.input);

        // ── Footer ───────────────────────────────────────────────────
        if pane_rects.footer.height > 0 {
            crate::footer::FooterView::render(pane_rects.footer, frame.buffer_mut(), self, &self.theme);
        }

        // ── Overlays ──────────────────────────────────────────────────
        if self.overlays.has_overlay() {
            self.overlays.render_all(frame.buffer_mut(), area);
        }

        // ── Confirmation dialog (on top of overlays) ─────────────────
        if let Some(ref dialog) = self.active_confirm_dialog {
            dialog.render(area, frame.buffer_mut());
        }
    }

    fn render_header(&self, frame: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        crate::header::HeaderWidget::render(area, frame.buffer_mut(), self, &self.theme);
    }

    fn render_transcript(&self, frame: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        use ratatui::widgets::Widget;
        let focused = self.focus == crate::focus::Focus::Chat;
        let border_color = if focused {
            self.theme.colors.primary
        } else {
            self.theme.colors.border
        };
        let block = ratatui::widgets::Block::default()
            .borders(ratatui::widgets::Borders::ALL)
            .border_set(ratatui::symbols::border::ROUNDED)
            .border_style(ratatui::style::Style::default().fg(border_color))
            .title(" Chat ");
        let inner = block.inner(area);
        block.render(area, frame.buffer_mut());
        crate::chat::ChatView::render(inner, frame.buffer_mut(), &self.transcript, &self.theme);
    }

    fn render_editor(&mut self, frame: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        use ratatui::style::Style;
        use ratatui::widgets::{Block, Borders, Paragraph};

        let is_running = matches!(self.state, AppState::Running { .. });
        let is_focused = self.focus == crate::focus::Focus::Input;

        // Character counter
        let max_chars = self.tui_config.input_max_chars;
        let current_len = self.editor.text().len();
        let title = if is_running {
            format!(" Input (steer) [{current_len}/{max_chars}] ")
        } else {
            format!(" Input [{current_len}/{max_chars}] ")
        };

        // Color for character counter based on fill level
        let fill_ratio = current_len as f64 / max_chars as f64;
        let border_color = if is_focused {
            self.theme.colors.primary
        } else if is_running {
            self.theme.colors.warning
        } else if fill_ratio > 0.95 {
            self.theme.colors.error
        } else if fill_ratio > 0.8 {
            self.theme.colors.warning
        } else {
            self.theme.colors.border
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_set(ratatui::symbols::border::ROUNDED)
            .title(title)
            .border_style(Style::default().fg(border_color));
        let inner = block.inner(area);

        // Available space for text: inner width minus "> " prompt prefix.
        let text_width = inner.width.saturating_sub(2);
        let visible_rows = inner.height as usize;

        // Auto-scroll to keep cursor visible.
        if visible_rows > 0 {
            self.editor.scroll_to_cursor(text_width, visible_rows);
        }

        // Render the block border first.
        frame.render_widget(block, area);

        // Build the display text (pre-wrapped + scrolled).
        let style = if is_running {
            Style::default().fg(self.theme.colors.warning)
        } else {
            self.theme.text_style()
        };

        let display_text = if self.editor.text().is_empty() {
            if is_running {
                "> Steering… (Enter to send guidance)".to_string()
            } else {
                format!("> {}", self.editor.placeholder())
            }
        } else {
            let visible = self.editor.visible_text(text_width, visible_rows);
            if visible.is_empty() {
                String::new()
            } else {
                format!("> {}", visible)
            }
        };

        frame.render_widget(Paragraph::new(display_text).style(style), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_app() -> App {
        let (event_tx, _event_rx) = mpsc::unbounded_channel();
        let (fe_tx, fe_bridge) = FrontendEventBridge::new(16);
        let fe_sink = Arc::new(FrontendEventSink::new(fe_tx));
        let (_config_dir, config) = test_config();
        App::new(
            config,
            TuiConfig::default(),
            event_tx,
            fe_bridge,
            fe_sink,
            std::path::PathBuf::from("/tmp/duga-test-sessions"),
        )
    }

    fn test_config() -> (tempfile::TempDir, Config) {
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
    - /usr/bin/echo
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
  summarizer: semantic
plugins:
  dir: {plugin_dir}
  modules: []
"#,
            root = dir.path().display(),
            plugin_dir = dir.path().display(),
        );
        let path = dir.path().join("test_config.yaml");
        std::fs::write(&path, yaml).unwrap();
        let config = Config::load(&path).expect("test config must parse");
        (dir, config)
    }

    #[test]
    fn app_starts_in_idle_state() {
        let app = make_app();
        assert!(matches!(app.state, AppState::Idle));
        assert!(!app.should_quit());
    }

    #[test]
    fn app_ignores_run_finished_with_wrong_id() {
        let mut app = make_app();
        app.update(AppEvent::RunFinished {
            run_id: 99,
            result: Ok(LoopResult {
                loop_id: "simple_react".into(),
                message: duga_types::message::AssistantMessage {
                    text: Some("ok".into()),
                    tool_calls: vec![],
                    reasoning_content: None,
                },
                steps: 1,
                tool_calls: 0,
            }),
        });
        assert!(matches!(app.state, AppState::Idle));
    }

    #[test]
    fn editor_submits_prompt() {
        let mut app = make_app();
        app.editor.insert_text("hello world");
        let action = app.editor.handle_key(&KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        ));
        assert_eq!(action, EditorAction::Submit);
    }

    #[test]
    fn global_quit_when_idle() {
        let mut app = make_app();
        app.handle_key(&KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL));
        assert!(app.should_quit());
    }

    #[test]
    fn help_overlay_toggles() {
        let mut app = make_app();
        assert!(!app.overlays.has_overlay());
        app.handle_key(&KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
        assert!(app.overlays.has_overlay());
        // Escape closes
        app.handle_key(&KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.overlays.has_overlay());
    }

    #[test]
    fn frontend_event_tool_call_creates_block() {
        let mut app = make_app();
        app.update(AppEvent::Frontend(FrontendEvent::ToolCallStarted {
            tool_name: "shell".into(),
            tool_call_id: "tc1".into(),
            attempt: 1,
            description: "ls -la".into(),
            raw_args: None,
        }));
        assert_eq!(app.transcript.len(), 1);
        let items = app.transcript.items();
        assert!(matches!(items[0], TranscriptItem::ToolCallBlock { .. }));
    }

    #[test]
    fn frontend_event_streaming_tokens() {
        let mut app = make_app();
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
    fn clear_transcript() {
        let mut app = make_app();
        app.transcript.push(TranscriptItem::SystemMessage {
            text: "test".into(),
            level: SystemLevel::Info,
            timestamp: Instant::now(),
        });
        assert_eq!(app.transcript.len(), 1);
        app.handle_key(&KeyEvent::new(
            KeyCode::Char('l'),
            KeyModifiers::CONTROL,
        ));
        assert!(app.transcript.len() > 0); // Has the "cleared" message
    }

    #[test]
    fn cycle_thinking_level_cycles_off_low_medium_high() {
        let mut app = make_app();
        use duga_config::ThinkingLevel;
        app.config.thinking_level = ThinkingLevel::Off;
        app.cycle_thinking_level();
        assert_eq!(app.config.thinking_level, ThinkingLevel::Low);
        app.cycle_thinking_level();
        assert_eq!(app.config.thinking_level, ThinkingLevel::Medium);
        app.cycle_thinking_level();
        assert_eq!(app.config.thinking_level, ThinkingLevel::High);
        app.cycle_thinking_level();
        assert_eq!(app.config.thinking_level, ThinkingLevel::Off);
    }

    #[test]
    fn shift_tab_handles_cycle_thinking_effort() {
        let mut app = make_app();
        use duga_config::ThinkingLevel;
        let initial = app.config.thinking_level;
        app.handle_key(&KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
        // Should have cycled to the next level
        assert_eq!(app.config.thinking_level, match initial {
            ThinkingLevel::Off => ThinkingLevel::Low,
            ThinkingLevel::Low => ThinkingLevel::Medium,
            ThinkingLevel::Medium => ThinkingLevel::High,
            ThinkingLevel::High => ThinkingLevel::Off,
        });
    }
}
