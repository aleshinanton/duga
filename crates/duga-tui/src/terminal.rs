//! Terminal lifecycle: raw mode, alternate screen, rendering loop.
//!
//! Uses ratatui for rendering and crossterm for input events.
//! The event loop merges crossterm input, frontend bridge events,
//! and periodic ticks.

use anyhow::{Context, Result};
use crossterm::event::{EnableBracketedPaste, EnableFocusChange};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use duga_config::Config;
use duga_runtime::{FrontendEventBridge, FrontendEventSink};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal as RatatuiTerminal;
use std::io::stdout;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::app::{App, AppEvent};

// ── Terminal guard ─────────────────────────────────────────────────────────

/// Restores terminal state on drop — ensures raw mode and alternate
/// screen are always cleaned up even if the app panics.
pub struct TerminalGuard;

impl TerminalGuard {
    /// Enter raw mode and alternate screen.
    pub fn enter() -> Result<Self> {
        enable_raw_mode().context("enabling raw mode")?;
        let mut stdout = stdout();
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableFocusChange,
            EnableBracketedPaste
        )
        .context("entering alternate screen")?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut stdout = stdout();
        let _ = execute!(stdout, LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

// ── Main TUI loop ──────────────────────────────────────────────────────────

/// Launch the TUI event loop. Blocks until the user quits.
pub async fn run_tui(config: Config, replay_dir: &Path) -> Result<()> {
    let _guard = TerminalGuard::enter()?;

    // Build the frontend event bridge.
    let (fe_tx, fe_bridge) = FrontendEventBridge::new(256);
    let fe_sink = Arc::new(FrontendEventSink::new(fe_tx));

    // Internal event channel (crossterm → main loop).
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<AppEvent>();

    // Spawn crossterm input reader in a blocking thread.
    // crossterm::event::read() blocks, so it must run off the async runtime.
    let ct_tx = event_tx.clone();
    std::thread::spawn(move || {
        while let Ok(event) = crossterm::event::read() {
            if ct_tx.send(AppEvent::Crossterm(event)).is_err() {
                break; // App dropped the receiver
            }
        }
    });

    // Tick timer for periodic updates.
    let tick_tx = event_tx.clone();
    let tick_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(50));
        loop {
            interval.tick().await;
            if tick_tx.send(AppEvent::Tick).is_err() {
                break;
            }
        }
    });

    // Frontend event forwarder (from bridge to app event channel).
    let fe_event_tx = event_tx.clone();
    let mut fe_bridge_clone = fe_bridge; // take ownership
    let fe_handle = tokio::spawn(async move {
        while let Some(fe) = fe_bridge_clone.recv().await {
            if fe_event_tx.send(AppEvent::Frontend(fe)).is_err() {
                break;
            }
        }
    });

    // Build TUI config (default if not present in config file).
    let tui_config = config.tui.clone().unwrap_or_default();

    // Build the app.
    let mut app = App::new(
        config.clone(),
        tui_config,
        event_tx.clone(),
        FrontendEventBridge::new(16).1, // dummy bridge; the real one is handled by fe_handle
        fe_sink,
    );

    // Pre-build the runtime for faster run starts
    // (skip this in tests/no-llm mode — just let it fail gracefully)
    let _ = &config; // used

    let mut terminal =
        RatatuiTerminal::new(CrosstermBackend::new(stdout())).context("creating terminal")?;

    // Welcome message
    {
        let mut transcript = crate::transcript::Transcript::new();
        transcript.push(crate::transcript::TranscriptItem::SystemMessage {
            text: format!(
                "duga-tui v{} — Model: {}",
                env!("CARGO_PKG_VERSION"),
                config.model
            ),
            level: crate::transcript::SystemLevel::Info,
            timestamp: std::time::Instant::now(),
        });
        transcript.push(crate::transcript::TranscriptItem::SystemMessage {
            text: "Type a task or question and press Enter. F1 for help, Ctrl+C to cancel, q to quit."
                .into(),
            level: crate::transcript::SystemLevel::Info,
            timestamp: std::time::Instant::now(),
        });
        app.transcript = transcript;
    }

    // Main event loop.
    loop {
        // Drain all pending events before rendering.
        while let Ok(event) = event_rx.try_recv() {
            app.update(event);
        }

        if app.should_quit() {
            break;
        }

        terminal
            .draw(|frame| {
                app.render(frame);
            })
            .context("rendering frame")?;
    }

    // Cleanup.
    tick_handle.abort();
    fe_handle.abort();
    // fe_tx was moved into fe_sink which was moved into app; drop happens naturally

    let _ = replay_dir;
    Ok(())
}
