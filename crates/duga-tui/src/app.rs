//! Central application state for the duga TUI.
//!
//! Owns the runtime, transcript, editor, overlays, and event bridges.
//! Rendered by `terminal.rs` on each tick.

use duga_config::{Config, TuiConfig};
use duga_core::loop_context::LoopContext;
use duga_core::loops::SimpleReActLoop;
use duga_runtime::{FrontendEvent, FrontendEventBridge, FrontendEventSink};
use duga_sandbox::CancellationToken;
use duga_types::error::AgentError;
use std::sync::Arc;
use tokio::sync::mpsc;

// ── App state machine ──────────────────────────────────────────────────────

/// High-level application state driving the rendering loop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppState {
    /// No agent run active. The editor is accepting input.
    Idle,
    /// Agent is executing. The editor is disabled; a loader is shown.
    Running {
        run_id: u64,
        cancel_requested: bool,
    },
    /// Cancellation is in progress (debounce period between cancel request
    /// and the loop actually stopping).
    Cancelling,
}

// ── Internal events ────────────────────────────────────────────────────────

/// Events produced by the crossterm reader thread, frontend bridge,
/// or spawned tasks.
#[derive(Clone, Debug)]
pub enum AppEvent {
    Crossterm(crossterm::event::Event),
    Frontend(FrontendEvent),
    Tick,
    /// An agent run has completed (or errored).
    RunFinished {
        run_id: u64,
        result: Result<duga_core::LoopResult, AgentError>,
    },
}

// ── Application ────────────────────────────────────────────────────────────

pub struct App {
    pub state: AppState,
    pub config: Config,
    pub tui_config: TuiConfig,

    /// Sender for AppEvent — used by spawned tasks to push events into the
    /// main event loop.
    pub event_tx: mpsc::UnboundedSender<AppEvent>,

    /// Frontend bridge for receiving agent progress events.
    pub fe_bridge: FrontendEventBridge,

    /// Next run id counter.
    next_run_id: u64,

    // Owned runtime — built at startup.
    // Will be wired up in a follow-up task (TASK-17.3 full impl).
}

impl App {
    pub fn new(
        config: Config,
        tui_config: TuiConfig,
        event_tx: mpsc::UnboundedSender<AppEvent>,
        fe_bridge: FrontendEventBridge,
    ) -> Self {
        Self {
            state: AppState::Idle,
            config,
            tui_config,
            event_tx,
            fe_bridge,
            next_run_id: 0,
        }
    }

    /// Handle a single `AppEvent` and update internal state.
    pub fn update(&mut self, event: AppEvent) {
        match event {
            AppEvent::Crossterm(ct_event) => self.handle_crossterm(ct_event),
            AppEvent::Frontend(fe) => self.handle_frontend_event(fe),
            AppEvent::Tick => self.handle_tick(),
            AppEvent::RunFinished { run_id, result } => {
                // Only handle if it matches the active run
                if let AppState::Running {
                    run_id: active_id, ..
                } = &self.state
                {
                    if *active_id == run_id {
                        tracing::info!(run_id, "run finished");
                        self.state = AppState::Idle;
                    }
                }
                let _ = result; // Will be rendered in transcript in a follow-up
            }
        }
    }

    fn handle_crossterm(&mut self, _event: crossterm::event::Event) {
        // Stub — full key routing implemented in TASK-17.3
    }

    fn handle_frontend_event(&mut self, event: FrontendEvent) {
        match event {
            FrontendEvent::RunStarted { task: _ } => {
                // Stub — TASK-17.3
            }
            FrontendEvent::RunFinished { text: _ } => {
                // Stub — TASK-17.3
            }
            FrontendEvent::ToolCallStarted { .. } => {
                // Stub — TASK-17.5
            }
            FrontendEvent::ToolCallFinished { .. } => {
                // Stub — TASK-17.5
            }
            FrontendEvent::LlmTokenDelta { delta: _, model: _ } => {
                // Stub — TASK-17.5
            }
            FrontendEvent::Error { message: _ } => {
                // Stub — TASK-17.5
            }
            FrontendEvent::LoopDelegated { .. } => {
                // Stub — TASK-17.5
            }
            FrontendEvent::MemoryCompressed { .. } => {
                // Stub — TASK-17.5
            }
        }
    }

    fn handle_tick(&mut self) {
        // Tick-driven updates: cursor blink, loader animation, debounced re-renders.
    }

    /// Check if the app should exit.
    pub fn should_quit(&self) -> bool {
        false // Stub — full quit logic in TASK-17.3
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_app() -> App {
        let (event_tx, _event_rx) = mpsc::unbounded_channel();
        let (fe_tx, fe_bridge) = FrontendEventBridge::new(16);
        drop(fe_tx); // Not used in these tests
        let (_config_dir, config) = test_config();
        App::new(config, TuiConfig::default(), event_tx, fe_bridge)
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
        assert_eq!(app.state, AppState::Idle);
        assert!(!app.should_quit());
    }

    #[test]
    fn app_ignores_run_finished_with_wrong_id() {
        let mut app = make_app();
        // RunFinished without a matching active run should not change state.
        app.update(AppEvent::RunFinished {
            run_id: 99,
            result: Ok(duga_core::LoopResult {
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
        assert_eq!(app.state, AppState::Idle);
    }
}
