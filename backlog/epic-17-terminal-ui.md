# EPIC-17: Terminal UI Frontend

**§SPEC:** §5, §24, §26, §32  
**Labels:** `epic/tui`  
**Crates:** `duga-runtime`, `duga-tui` (new)

## Goal

Add an interactive terminal UI frontend on top of the shared duga runtime. The TUI should provide a local chat/task interface, live agent progress, tool-call visibility, cancellation, replay browsing, and safe confirmation UX for risky operations without duplicating harness wiring.

This epic relies on shared infrastructure from EPIC-18. The following features are **not duplicated** — they come from shared crates:
- **Provider resolution + LLM building** → `duga-runtime`
- **Tool registration (builtins + WASM plugins)** → `duga-runtime`
- **MEMORY.md persistent context** → `duga-runtime` (shared with Telegram, CLI)
- **SKILL.md loading + prompt injection** → `duga-runtime` (shared with Telegram, CLI)
- **Tool confirmation hooks** → `duga-runtime` (TUI provides the modal dialog UI)
- **Docker sandbox executor** → `duga-sandbox` (shared with Telegram, CLI)
- **Non-blocking frontend event bridge** → `duga-runtime` (TUI renders from an mpsc channel)
- **SSE streaming** → `duga-llm` (shared)
- **Real token counting** → `duga-llm` (shared)

---

### TASK-17.1: Create duga-tui crate with ratatui setup

- **§SPEC:** §32 (composition root)
- **Labels:** `layer/tui`, `layer/frontend`, `priority/critical`
- **Description:** Add a `duga-tui` binary crate using `ratatui` and `crossterm`. It should load the same config as `duga-harness`, initialize tracing, and render a basic terminal shell.
- **Files affected:**
  - `crates/duga-tui/Cargo.toml` (new)
  - `crates/duga-tui/src/main.rs` (new)
  - `crates/duga-tui/src/app.rs` (new)
  - `crates/duga-tui/src/ui.rs` (new)
- **Types involved:** `Cli`, `App`, `TerminalGuard`
- **Functions to implement:**
  - `main() -> anyhow::Result<()>`
  - `run_tui(config: Config) -> anyhow::Result<()>`
  - `draw(frame: &mut Frame, app: &App)`
- **Dependencies:** TASK-18.1 (shared runtime), TASK-18.2 (typed config), TASK-18.6 (Docker executor), TASK-12.6
- **Implementation steps:**
  1. Add crate to the workspace.
  2. Add `ratatui`, `crossterm`, `tokio`, `anyhow`, and `tracing-subscriber`.
  3. Parse `--config <PATH>` and optional `--task`.
  4. Render a static shell with transcript, status, and input areas.
- **Definition of Done:** `cargo run -p duga-tui -- --config <file>` opens and exits cleanly.
- **Acceptance criteria:**
  - `q` exits when no run is active.
  - Terminal is restored after normal exit.
  - Config loading matches `duga-harness`.
- **Test plan:** command parsing unit tests and terminal startup smoke test.
- **Estimated effort:** 4 hours

---

### TASK-17.2: TUI config and terminal lifecycle

- **§SPEC:** §32 (config loading), §26 (cancellation)
- **Labels:** `layer/tui`, `layer/cli`, `priority/critical`
- **Description:** Add optional TUI config for rendering mode, theme, progress verbosity, keybindings, and confirmation defaults. Implement guarded raw-mode and alternate-screen lifecycle. Shared runtime fields and Docker sandbox mode come from EPIC-18 typed config.
- **Files affected:**
  - `crates/duga-config/src/config.rs`
  - `crates/duga-tui/src/terminal.rs` (new)
- **Types involved:** `TuiConfig`, `TerminalGuard`, `ThemeConfig`, `KeybindingsConfig`
- **Functions to implement:**
  - `TerminalGuard::enter() -> Result<Self>`
  - `impl Drop for TerminalGuard`
  - config validation for keybindings and theme names
- **Proposed YAML:**
  ```yaml
  tui:
    theme: "default"
    send_tool_events: true
    final_only: false
    confirm_risky_tools: true
    keybindings:
      cancel: "ctrl-c"
      submit: "enter"
      quit: "q"
  ```
- **Dependencies:** TASK-17.1, TASK-12.1, TASK-12.3
- **Implementation steps:**
  1. Add `tui: Option<TuiConfig>` with `#[serde(default)]`.
  2. Use typed keybinding/theme config where practical and validate non-empty values.
  3. Add terminal guard cleanup for raw mode and alternate screen.
  4. Install panic cleanup for terminal restoration.
- **Definition of Done:** TUI config parses and terminal cleanup is reliable.
- **Acceptance criteria:**
  - Invalid keybinding config returns a validation error.
  - Panic or early error restores the terminal.
- **Test plan:** config tests and lifecycle smoke tests.
- **Estimated effort:** 4 hours

---

### TASK-17.3: App state and async event loop

- **§SPEC:** §24 (events), §26 (cancellation)
- **Labels:** `layer/tui`, `layer/events`, `priority/critical`
- **Description:** Implement the central app state and event loop that combines terminal input, tick events, resize events, and agent/runtime events.
- **Files affected:**
  - `crates/duga-tui/src/app.rs`
  - `crates/duga-tui/src/events.rs` (new)
- **Types involved:** `AppState`, `AppEvent`, `UiCommand`, `RunStatus`
- **Functions to implement:**
  - `App::update(event: AppEvent) -> AppAction`
  - `event_loop(app, terminal, runtime)`
  - `spawn_input_reader(tx)`
- **Dependencies:** TASK-17.1
- **Implementation steps:**
  1. Define app state for transcript, input, status, active run, and selected pane.
  2. Convert `crossterm` events into `AppEvent`.
  3. Use channels for UI-to-runtime and runtime-to-UI messages.
  4. Keep rendering deterministic from state.
- **Definition of Done:** TUI reacts to keys, resize, ticks, and mock runtime events.
- **Acceptance criteria:**
  - UI does not block while a run is active.
  - State updates are testable without a real terminal.
- **Test plan:** reducer-style tests for `App::update`.
- **Estimated effort:** 4 hours

---

### TASK-17.4: Transcript pane and input composer

- **§SPEC:** §5 (task input), §24 (events)
- **Labels:** `layer/tui`, `priority/high`
- **Description:** Build the interactive transcript and input composer. Support multi-line input, history navigation, scrollback, status bar, and simple command shortcuts.
- **Files affected:**
  - `crates/duga-tui/src/ui.rs`
  - `crates/duga-tui/src/input.rs` (new)
- **Types involved:** `InputComposer`, `TranscriptItem`, `ScrollState`
- **Functions to implement:**
  - `InputComposer::handle_key(key)`
  - `render_transcript(frame, area, app)`
  - `render_input(frame, area, app)`
- **Dependencies:** TASK-17.3
- **Implementation steps:**
  1. Implement editable input buffer.
  2. Add submit, cancel, scroll, and focus key handling.
  3. Render transcript entries for user, assistant, tool, and system messages.
  4. Add command history for prior prompts.
- **Definition of Done:** User can type, submit, scroll, and review output in the terminal.
- **Acceptance criteria:**
  - Multi-line prompts render correctly.
  - Scrollback is stable during live updates.
  - `/help`, `/cancel`, and `/quit` are recognized.
- **Test plan:** input composer unit tests and snapshot-style render tests.
- **Estimated effort:** 4 hours

---

### TASK-17.5: TUI event renderer for live progress

- **§SPEC:** §24 (events), §31 (observability)
- **Labels:** `layer/tui`, `layer/events`, `priority/high`
- **Description:** Consume frontend events from the EPIC-18 non-blocking event bridge and map them into TUI state updates. Rendering must happen from the TUI event loop, never inside `AgentLoop::emit()`.
- **Files affected:**
  - `crates/duga-tui/src/sink.rs` (new)
  - `crates/duga-tui/src/formatting.rs` (new)
- **Types involved:** `TuiEventRenderer`, `UiEventFormatter`
- **Functions to implement:**
  - `TuiEventRenderer::new(rx, options)`
  - `format_event(event: &Event) -> Option<TranscriptItem>`
- **Dependencies:** TASK-18.4, TASK-17.3
- **Implementation steps:**
  1. Map LLM, tool, error, cancellation, and final frontend events.
  2. Preserve structured fields for tool panels.
  3. Redact secrets consistently with JSONL sinks.
  4. Coalesce high-volume token/progress events before redraw.
- **Definition of Done:** Live agent progress appears in the TUI without blocking the core loop.
- **Acceptance criteria:**
  - Final answer is always visible.
  - Tool events can be collapsed or expanded.
  - High-volume updates do not freeze rendering or fail the agent run.
- **Test plan:** event-to-UI mapping tests with captured events.
- **Estimated effort:** 4 hours

---

### TASK-17.6: Run AgentLoop with cancellation from TUI

- **§SPEC:** §5 (loop), §26 (cancellation), §32 (composition root)
- **Labels:** `layer/tui`, `layer/loop`, `priority/critical`
- **Description:** Wire submitted prompts into `duga-runtime::build_agent`. Spawn an agent run, attach the EPIC-18 non-blocking TUI event bridge plus JSONL replay sink, and support cancellation from keyboard or command input.
- **Files affected:**
  - `crates/duga-tui/src/runtime.rs` (new)
  - `crates/duga-tui/src/app.rs`
- **Types involved:** `TuiRuntime`, `RunRequest`, `RunHandle`
- **Functions to implement:**
  - `start_run(prompt, config, tx) -> RunHandle`
  - `cancel_active_run()`
- **Dependencies:** TASK-18.1, TASK-18.3, TASK-18.4, TASK-17.3, TASK-17.5
- **Implementation steps:**
  1. Build agent through `duga-runtime`.
  2. Create per-run cancellation token.
  3. Spawn `AgentLoop::run`.
  4. Send completion or error back into app state through the UI channel.
- **Definition of Done:** TUI can run the same agent flow as the CLI and cancel it.
- **Acceptance criteria:**
  - Submitted prompt produces a final response.
  - `Ctrl+C` or `/cancel` cancels the active run.
  - Replay JSONL is written per run.
- **Test plan:** integration test with mock runtime and mock LLM.
- **Estimated effort:** 5 hours

---

### TASK-17.7: Tool panels and confirmation dialogs

- **§SPEC:** §13-16 (sandbox), §24 (events), §26 (cancellation)
- **Labels:** `layer/tui`, `layer/security`, `priority/critical`
- **Description:** Add tool-call panels with arguments, status, duration, truncated output, and explicit confirmation UI for risky tools when configured. Confirmation enforcement itself is shared middleware from EPIC-18.
- **Files affected:**
  - `crates/duga-tui/src/tools.rs` (new)
  - `crates/duga-tui/src/confirm.rs` (new)
- **Types involved:** `ToolPanel`, `ConfirmationDialog`, `ConfirmationRequest`, `ConfirmationDecision`
- **Functions to implement:**
  - `render_tool_panel(frame, area, tool_call)`
  - `show_confirmation(request) -> ConfirmationDecision`
  - `confirm_active()` / `deny_active()`
- **Dependencies:** TASK-17.6, TASK-18.5
- **Implementation steps:**
  1. Track tool-call lifecycle from events.
  2. Render collapsed and expanded tool views (output in transcript, details in side panel).
  3. Wire the shared confirmation provider from `duga-runtime`: TUI provides a modal dialog callback. On tool requiring confirmation, show modal with tool name + args + [y/n] prompt. Accept `y`/`n` keys.
  4. Return deny/timeout as explicit tool errors via the shared hook.
- **Definition of Done:** Users can inspect and approve/deny risky local actions from the TUI.
- **Acceptance criteria:**
  - Shell/edit/write actions can require confirmation.
  - Denied actions are visible in the transcript.
  - Long outputs are truncated with a way to inspect details.
- **Test plan:** UI state tests for tool lifecycle and confirmation decisions.
- **Estimated effort:** 5 hours

---

### TASK-17.8: Replay and session browser

- **§SPEC:** §25 (replay), §31 (observability)
- **Labels:** `layer/tui`, `layer/observability`, `priority/high`
- **Description:** Add a local browser for past JSONL runs. Users can list sessions, open a replay summary, and inspect transcript/tool events without rerunning the agent.
- **Files affected:**
  - `crates/duga-tui/src/replay.rs` (new)
  - `crates/duga-tui/src/ui.rs`
- **Types involved:** `ReplayBrowser`, `ReplayRunSummary`, `ReplayEventView`
- **Functions to implement:**
  - `load_replay_index(path) -> Result<Vec<ReplayRunSummary>>`
  - `load_replay_run(path) -> Result<Vec<Event>>`
  - `render_replay_browser(frame, area, app)`
- **Dependencies:** TASK-13.4, TASK-17.4
- **Implementation steps:**
  1. Read replay directories configured for the runtime.
  2. Build lightweight summaries from JSONL metadata.
  3. Add keyboard navigation for replay mode.
  4. Render replay events using the same transcript/tool components.
- **Definition of Done:** TUI can inspect prior runs offline.
- **Acceptance criteria:**
  - Corrupt replay files surface clear errors.
  - Replay browsing does not mutate current runtime state.
- **Test plan:** fixture-based replay browser tests.
- **Estimated effort:** 4 hours

---

### TASK-17.9: TUI integration tests and docs

- **§SPEC:** §25 (replay), §31 (observability), §32 (config)
- **Labels:** `layer/tui`, `layer/testing`, `priority/high`
- **Description:** Add offline tests and documentation for the TUI frontend. Tests should not require a live terminal or provider credentials.
- **Files affected:**
  - `crates/duga-tui/tests/tui_flow.rs` (new)
  - `README.md` (TUI setup section)
  - `docs/tui.md` (new)
- **Types involved:** test backend, mock runtime, mock LLM, `CapturingEventSink`
- **Functions to implement:** test helpers for simulated key input and rendered frame assertions
- **Dependencies:** TASK-17.1 through TASK-17.8, TASK-18.1 through TASK-18.6
- **Implementation steps:**
  1. Use ratatui test backend for deterministic render checks.
  2. Simulate prompt submission, progress, cancellation, and confirmation.
  3. Verify replay browser uses JSONL fixtures.
  4. Document install/run commands, keybindings, config, and limitations.
- **Definition of Done:** TUI behavior is covered offline and documented.
- **Acceptance criteria:**
  - Tests pass without provider credentials.
  - README contains minimal TUI setup.
  - docs page explains keyboard shortcuts and safety model.
- **Test plan:** `cargo test -p duga-tui`.
- **Estimated effort:** 6 hours
