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
use duga_config::{Config, TuiConfig};
use duga_runtime::FrontendEventBridge;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal as RatatuiTerminal;
use std::io::{self, stdout};
use std::path::Path;
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
        execute!(stdout, EnterAlternateScreen, EnableFocusChange, EnableBracketedPaste)
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

    // Tick timer.
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

    // Build TUI config (default if not present in config file).
    let tui_config = config.tui.clone().unwrap_or_default();

    // Build the app.
    let mut app = App::new(config, tui_config, event_tx.clone(), fe_bridge);

    let mut terminal =
        RatatuiTerminal::new(CrosstermBackend::new(stdout())).context("creating terminal")?;

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
                // Stub — full rendering implemented in TASK-17.4
                let area = frame.area();
                frame.render_widget(
                    ratatui::widgets::Paragraph::new("duga-tui — starting up...")
                        .centered(),
                    area,
                );
            })
            .context("rendering frame")?;
    }

    // Cleanup.
    tick_handle.abort();
    let _ = drop(fe_tx);

    Ok(())
}
