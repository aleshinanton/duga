# EPIC-17 REVISION: Analysis & Adjusted Task List

**Analysis Date:** 2026-05-27
**Based on:** Codebase at HEAD, revision document from 2025-05-26

---

## Executive Summary

The EPIC-17 revision document is broadly correct in its understanding of existing infrastructure. However, I found **10 concrete issues** ranging from API mismatches (steering, crossterm) to missing tasks (run spawning, tests/docs). Below is the detailed analysis followed by the adjusted document.

---

## Issue-by-Issue Analysis

### Issue 1: TASK-17.3 — Crossterm event stream + tokio::select! is broken

**Revision says:**
```rust
tokio::select! {
    Some(event) = crossterm_event_stream.next() => handle_crossterm(event),
    Some(fe) = fe_rx.recv() => handle_frontend_event(fe),
    _ = tokio::time::sleep(tick_rate) => handle_tick(),
}
```

**Problem:** `crossterm::event::EventStream` does **not** implement `futures::Stream` or `tokio_stream::Stream`. Its `.next()` returns `io::Result<Event>`, not `Option<Event>`. You cannot `.await` it directly in `tokio::select!` — crossterm reads from stdin which blocks, so it must run in a dedicated thread via `tokio::task::spawn_blocking`.

**Fix:** The standard pattern:
```rust
let (crossterm_tx, mut crossterm_rx) = mpsc::unbounded_channel();
tokio::task::spawn_blocking(move || {
    while let Ok(event) = crossterm::event::read() {
        if crossterm_tx.send(event).is_err() { break; }
    }
});

// Then in the event loop:
tokio::select! {
    Some(event) = crossterm_rx.recv() => handle_crossterm(event),
    Some(fe) = fe_rx.recv() => handle_frontend_event(fe),
    _ = tokio::time::sleep(tick_rate) => handle_tick(),
}
```

---

### Issue 2: TASK-17.3 — SteeringSender API mismatch

**Revision says:**
```rust
if let AppState::Running { .. } = state {
    if let Some(steer_tx) = &steer_sender {
        steer_tx.send(SteeringContextEvent::InjectGuidance {
            text: user_input,
            as_system: false,
            source: "human".into(),
        }).await.ok();
    }
}
```

**Problem:** Three errors:
1. `SteeringSender` does not have a `.send().await` method — it uses `inject(SteeringEvent)` which is synchronous.
2. The type should be `SteeringEvent::Context(SteeringContextEvent::InjectGuidance {...})`, not bare `SteeringContextEvent`.
3. For steering-as-you-type, `InjectGuidance` is the right semantic (adds to memory, doesn't force immediate LLM re-call). But `guide(&text)` convenience method creates a `Reprompt` *control* event instead — that forces re-call. If we want guidance-only, we need `InjectGuidance`.

**Fix:**
```rust
if let AppState::Running { .. } = state {
    if let Some(steer_tx) = &steer_sender {
        let _ = steer_tx.inject(SteeringEvent::Context(
            SteeringContextEvent::InjectGuidance {
                text: user_input,
                as_system: false,
                source: "human".into(),
            }
        ));
    }
}
```

---

### Issue 3: TASK-17.4/17.5 — ToolCallFinished.description is always empty

**Actual code in `duga-runtime/src/events.rs`:**
```rust
Event::ToolCallFinished { result, attempt, tool_name } => {
    Some(FrontendEvent::ToolCallFinished {
        tool_name: tool_name.clone(),
        tool_call_id: result.tool_call_id.to_string(),
        success: result.success,
        attempt,
        description: String::new(),  // <-- ALWAYS EMPTY
    })
}
```

**Problem:** The `Event::ToolCallFinished` variant doesn't carry the `label`/description from the original tool call. The TUI would try to display tool names in the collapsible block but won't have the human-readable description.

**Fix (two options):**
1. **Runtime fix (preferred):** Modify `map_event` to accept a description lookup map, or modify `Event::ToolCallFinished` to carry the label.
2. **TUI workaround:** Maintain a `HashMap<String, String>` mapping `tool_call_id → description`, populated on `ToolCallStarted` and consumed on `ToolCallFinished`. Document this in the task.

**Recommendation:** Add a new subtask or note to TASK-17.4: maintain `tool_descriptions: HashMap<String, String>` in `App` state, populated from `ToolCallStarted`, consumed by `ToolCallFinished`.

---

### Issue 4: Missing RunAgentLoop spawning (original TASK-17.6)

**Problem:** The original EPIC-17 had TASK-17.6 "Run AgentLoop with cancellation from TUI" which covered:
- Building the agent via `build_agent()`
- Spawning `loop_impl.run(task, &mut ctx)` in a tokio task
- Cancellation token wiring
- Replay JSONL sink

The revision absorbed this into TASK-17.3 (App state + event loop) but never explicitly describes *who calls `loop_impl.run()` and how*. TASK-17.3 handles event *receiving* from the frontend bridge, but not the *spawning* side.

**Fix:** Add a new TASK-17.3b or expand TASK-17.3 to include:

```rust
// In App::submit_prompt():
let cancellation = CancellationToken::new();
let cancel_clone = cancellation.clone();

// Build runtime (or reuse pre-built runtime)
let mut runtime = build_agent(&config, llm, dispatcher, workspace, sinks, None)?;
let mut ctx = LoopContext { ... };

let loop_impl = SimpleReActLoop;
let task_text = self.input.take();

let app_tx = self.event_tx.clone();
tokio::spawn(async move {
    let result = loop_impl.run(task_text, &mut ctx).await;
    app_tx.send(AppEvent::RunFinished(result)).ok();
});
```

---

### Issue 5: TASK-17.2 — Missing `TuiConfig` field on `Config`

**Problem:** The revision describes `TuiConfig`, `ThemeConfig`, `KeybindingsConfig` structs but never explicitly states: "add `tui: Option<TuiConfig>` field to the `Config` struct in `duga-config`."

Currently the Config struct has:
```rust
pub struct Config {
    ...
    pub frontend: FrontendConfig,
    pub telegram: Option<TelegramConfig>,
    // NO tui field
}
```

**Fix:** The task should explicitly say: add `#[serde(default)] pub tui: Option<TuiConfig>` to `Config`, mirroring how `telegram: Option<TelegramConfig>` works.

---

### Issue 6: TASK-17.13 — Kitty protocol should use crossterm native API

**Revision says:**
```rust
async fn query_kitty_protocol() -> Result<bool> {
    // Send query: \x1b[?u
    // Listen for: \x1b[?1u (supported)
}
```

**Problem:** crossterm supports Kitty keyboard protocol natively via `crossterm::event::PushKeyboardEnhancementFlags` and `crossterm::event::KeyboardEnhancementFlags`. The raw escape sequence approach is unnecessary and fragile.

**Fix:**
```rust
use crossterm::event::{PushKeyboardEnhancementFlags, KeyboardEnhancementFlags};

// Enable flags:
execute!(stdout, PushKeyboardEnhancementFlags(
    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
    | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
    | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
))?;
```

The revision should also note that terminal support detection can be done via `$TERM` (kitty, wezterm, foot, ghostty) or `$TERMINAL_EMULATOR` vars, rather than a protocol query that may hang.

---

### Issue 7: Missing tests and docs task (original TASK-17.9)

**Problem:** The original EPIC-17 had TASK-17.9 "TUI integration tests and docs" (6 hours). The revision drops it entirely. This covers offline tests with ratatui test backend, mock LLM, mock runtime, and documentation.

**Fix:** Restore as TASK-17.14.

---

### Issue 8: TASK-17.7 — ConfirmationMiddleware source crate

**Revision says:** "Integration with ConfirmationMiddleware (exists in duga-runtime)"

**Actual:** `ConfirmationMiddleware` is defined in `duga-tools/src/confirmation.rs`. `duga-runtime/src/confirmation.rs` just re-exports: `pub use duga_tools::confirmation::*;`

This is a minor documentation issue — both paths work, but for clarity the TUI should depend on `duga-runtime` and use the re-export.

---

### Issue 9: Missing `ratatui` feature in crossterm

**Revision's Cargo.toml:**
```toml
crossterm = { version = "0.28", features = ["event-stream"] }
```

**Problem:** `crossterm 0.28` doesn't have an `event-stream` feature — that was pre-0.28. In 0.28, `EventStream` is always available. Also, ratatui 0.29 uses crossterm 0.28 internally; you typically don't need to depend on crossterm directly for event handling if you use ratatui's built-in `Terminal::draw()`. However, for raw mode control and event reading, you do want crossterm directly.

**Fix:**
```toml
crossterm = { version = "0.28", features = ["bracketed-paste"] }
```

Or better:
```toml
crossterm = "0.28"
```

Since `event-stream` doesn't exist as a feature in 0.28.

---

### Issue 10: ratatui 0.29 API notes

**Revision uses:** `ratatui = "0.29"`

**Note:** ratatui 0.29 was a major release. Key API changes from 0.28:
- `Frame` → still exists, `render_stateful_widget` → `render_stateful_widget` (same)
- `Paragraph::new()` → same
- `Block::bordered()` → introduced in 0.23, standard in 0.29
- `Layout::vertical()` / `Layout::horizontal()` → standard
- `Stylize` trait for inline styling: `"text".red().bold()`

The revision's rendering code is compatible. No changes needed.

---

## Additional Recommendations

### A: Session history and context restoration

The TUI should support resuming from a prior session by loading messages from JSONL replay files. The `duga-replay` crate's `JsonlReader` can read stored events, and `BuiltRuntime::memory_mut()` allows pre-populating memory before a run. This should be added to TASK-17.6 (Multiple runs).

### B: Global keybindings priority

The TUI must handle key event routing carefully. When an overlay is active, keys go to the overlay first. When the editor is focused, text keys go to the editor. Global keys (Ctrl+C, F1, q on idle) have highest priority. The event routing table should be explicit.

### C: Error recovery

If `build_agent()` fails (e.g., provider resolution error), the TUI should show the error in the transcript area rather than crashing. The TUI should be resilient to runtime errors and allow the user to retry.

### D: Paste handling

crossterm supports bracketed paste. Enable it via `crossterm::event::EnableBracketedPaste` and handle `Event::Paste(String)` in the event loop. The revision mentions this in TASK-17.3's event table but doesn't describe the setup.

---

## Adjusted EPIC-17 Document

Below is the corrected and adjusted version of the EPIC-17 revision, incorporating all findings above.

---

# EPIC-17: Terminal UI Frontend (ADJUSTED)

**Revision Date:** 2026-05-27 (adjusted from 2025-05-26 revision)
**Original:** `/backlog/epic-17-terminal-ui.md`
**Current state:** No `duga-tui` crate exists. All prerequisite infrastructure (EPIC-18, 26, 27, 28, 29/30, 31, replay) is in place.

---

## Changes from Original Epic

- **EPIC-18** (Frontend Shared Runtime) implemented → `FrontendEventBridge`, `FrontendEventSink`, `build_agent()`, `build_dispatcher()` exist in `duga-runtime`
- **EPIC-26** (Loop-Agnostic Core) implemented → `LoopRegistry`, `LoopContext`, delegation, multiple loop types
- **EPIC-27** (Markdown→HTML) implemented → Markdown rendering pipeline exists (though TUI uses pulldown-cmark → ratatui, not HTML)
- **EPIC-28** (File Attachment) implemented → `SendFileTool` exists
- **EPIC-29/30** (Skills) implemented → Skill discovery/wiring in runtime
- **EPIC-31** (Steering) implemented → `SteeringSender`/`SteeringReceiver`, `check_steer()` in `LoopContext`
- **Replay system** (`duga-replay` crate) exists for session playback

**No `duga-tui` crate exists yet.** The `tui` feature flag in `duga-harness` is empty.

---

## Dependency Inversion: What the TUI calls and what calls the TUI

```
┌──────────────────────────────────────────────────────────┐
│ duga-tui (new)                                           │
│                                                          │
│  crossterm event loop ──► App state ──► ratatui render   │
│         ▲                      │                         │
│         │                      ▼                         │
│  spawn_blocking thread   submit_prompt()                 │
│  reads stdin             spawns tokio task               │
│                              │                           │
│                    ┌─────────▼──────────┐                │
│                    │  loop_impl.run()   │                │
│                    │  (tokio::spawn)    │                │
│                    └─────────┬──────────┘                │
│                              │                           │
│                    FrontendEventSink                     │
│                         │                                │
│                    FrontendEventBridge.recv() ────────────┤
│                         │                                │
│                    App::handle_frontend_event()          │
└──────────────────────────────────────────────────────────┘
                              │
                              ▼
┌──────────────────────────────────────────────────────────┐
│ duga-runtime (exists)                                    │
│   build_agent() → BuiltRuntime {                         │
│     Memory, Summarizer, LoopRegistry,                    │
│     EventSink (FrontendEventSink),                       │
│     CancellationToken                                    │
│   }                                                      │
│                                                          │
│ duga-core (exists)                                       │
│   LoopContext { steer: Option<SteeringReceiver>, ... }   │
│   SimpleReActLoop::run()                                 │
│   SteeringSender (unbounded mpsc)                        │
└──────────────────────────────────────────────────────────┘
```

---

## UPDATED TASK LIST

### TASK-17.1: Create duga-tui crate with ratatui setup [UPDATE]

**Changes from original:** Now integrates with `duga-runtime` and `FrontendEventBridge`.

**Dependencies now resolved:**
- TASK-18.1 (shared runtime) → DONE: `duga-runtime::build_agent()`
- TASK-18.2 (typed config) → DONE: `Config` with `FrontendConfig`
- TASK-12.6 (CLI arg parsing) → DONE: `Cli` in `duga-harness`

**New crate setup:**
```toml
[package]
name = "duga-tui"
version = "0.1.0"
edition = "2021"

[dependencies]
ratatui = "0.29"
crossterm = { version = "0.28", features = ["bracketed-paste"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread", "signal", "sync"] }
anyhow = "1"
clap = { version = "4", features = ["derive"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
duga-config = { path = "../duga-config" }
duga-runtime = { path = "../duga-runtime" }
duga-core = { path = "../duga-core" }
duga-events = { path = "../duga-events" }
duga-sandbox = { path = "../duga-sandbox" }
duga-tools = { path = "../duga-tools" }
duga-tools-builtin = { path = "../duga-tools-builtin" }
duga-llm = { path = "../duga-llm" }
duga-types = { path = "../duga-types" }
duga-replay = { path = "../duga-replay" }
pulldown-cmark = { workspace = true }
unicode-width = "0.2"
```

**Files:**
- `crates/duga-tui/Cargo.toml` (new)
- `crates/duga-tui/src/main.rs` (new)
- `crates/duga-tui/src/app.rs` (new)
- `crates/duga-tui/src/terminal.rs` (new)

**Definition of Done:** `cargo run -p duga-tui -- --config <file>` opens and exits cleanly.
**Estimated effort:** 3 hours

---

### TASK-17.2: TUI config and terminal lifecycle [UPDATE]

**Changes from original:** Now reuses `FrontendConfig` from `duga-config` and adds TUI-specific fields. **Must explicitly add `tui: Option<TuiConfig>` to the `Config` struct** (mirroring how `telegram: Option<TelegramConfig>` works).

**Use existing config:**
- `Config::frontend.progress_mode` already exists (`FinalOnly`, `Summary`, `Verbose`)
- `Config::frontend.confirmation_timeout` already exists

**New TUI-specific config to add to `duga-config::Config`:**
```rust
/// Add this field to the Config struct:
/// #[serde(default)]
/// pub tui: Option<TuiConfig>,

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TuiConfig {
    #[serde(default = "default_true")]
    pub ime_support: bool,
    #[serde(default = "default_true")]
    pub protocol_detection: bool,
    #[serde(default)]
    pub tool_event_format: ToolEventFormat,
    #[serde(default)]
    pub theme: ThemeConfig,
    #[serde(default)]
    pub keybindings: KeybindingsConfig,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolEventFormat {
    Full,       // Always expanded
    Collapsed,  // Collapsed by default
    FinalOnly,  // Only show tool names after completion
}

impl Default for ToolEventFormat {
    fn default() -> Self { Self::Collapsed }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ThemeConfig {
    #[serde(default = "default_theme_name")]
    pub name: String,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self { name: default_theme_name() }
    }
}

fn default_theme_name() -> String { "default".into() }

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct KeybindingsConfig {
    #[serde(default = "default_submit_key")]
    pub submit: String,
    #[serde(default = "default_cancel_key")]
    pub cancel: String,
    #[serde(default = "default_quit_key")]
    pub quit: String,
    #[serde(default = "default_help_key")]
    pub help: String,
    #[serde(default = "default_search_key")]
    pub search: String,
}

impl Default for KeybindingsConfig {
    fn default() -> Self {
        Self {
            submit: default_submit_key(),
            cancel: default_cancel_key(),
            quit: default_quit_key(),
            help: default_help_key(),
            search: default_search_key(),
        }
    }
}

fn default_submit_key() -> String { "enter".into() }
fn default_cancel_key() -> String { "ctrl-c".into() }
fn default_quit_key() -> String { "q".into() }
fn default_help_key() -> String { "f1".into() }
fn default_search_key() -> String { "ctrl-f".into() }
```

**Proposed YAML addition:**
```yaml
tui:
  ime_support: true
  protocol_detection: true
  tool_event_format: "collapsed"
  theme:
    name: "default"
  keybindings:
    submit: "enter"
    cancel: "ctrl-c"
    quit: "q"
    help: "f1"
    search: "ctrl-f"
```

**Files affected:**
- `crates/duga-config/src/config.rs` (add `TuiConfig` + `tui: Option<TuiConfig>` field on `Config` + deserialize)
- `crates/duga-tui/src/terminal.rs` (new)

**NOTE on `TerminalGuard` naming:** ratatui has its own `Terminal` type. The TUI's `TerminalGuard` handles raw mode / alternate screen lifecycle and should be clearly distinguished in docs.

**Estimated effort:** 3 hours

---

### TASK-17.3: App state, event loop, and run spawning [UPDATE — merged original 17.3 + 17.6]

**Description:** Implement the central app state, event loop, AND agent run spawning. This merges the original TASK-17.3 (event loop) and TASK-17.6 (run spawning), since they are tightly coupled.

**State machine:**
```rust
pub enum AppState {
    /// No agent run active. Editor is accepting input.
    Idle,
    /// Agent is processing. Editor is disabled with loader showing.
    Running {
        run_id: u64,
        cancel_requested: bool,
    },
    /// Cancellation in progress (debounce period)
    Cancelling,
}
```

**FrontendEventBridge integration:**
```rust
// In main.rs or tui bootstrap:
let (fe_tx, mut fe_bridge) = FrontendEventBridge::new(256);
let fe_sink = Arc::new(FrontendEventSink::new(fe_tx));

// Build agent with fe_sink (along with JSONL replay sink)
let runtime = build_agent(&config, llm, dispatcher, workspace,
    vec![fe_sink, jsonl_sink], None)?;
```

**Crossterm event handling — CORRECTED PATTERN:**

crossterm's `EventStream` is NOT compatible with `tokio::select!`. Instead, spawn a blocking thread:

```rust
use crossterm::event::{Event, KeyEvent, EnableBracketedPaste};
use tokio::sync::mpsc;

// Enable bracketed paste for paste support
execute!(stdout(), EnableBracketedPaste)?;

// Spawn crossterm reader in blocking thread
let (ct_tx, mut ct_rx) = mpsc::unbounded_channel::<Event>();
std::thread::spawn(move || {
    while let Ok(event) = crossterm::event::read() {
        if ct_tx.send(event).is_err() { break; } // App dropped
    }
});

// Event loop:
loop {
    tokio::select! {
        Some(event) = ct_rx.recv() => {
            match event {
                Event::Key(key) => app.handle_key(key),
                Event::Resize(w, h) => app.handle_resize(w, h),
                Event::Paste(text) => app.handle_paste(text),
                Event::FocusGained => app.handle_focus(true),
                Event::FocusLost => app.handle_focus(false),
                _ => {}
            }
        }
        Some(fe) = fe_bridge.recv() => app.handle_frontend_event(fe),
        _ = tokio::time::sleep(tick_rate) => app.handle_tick(),
        else => break, // All senders dropped
    }

    terminal.draw(|f| app.render(f))?;
}
```

**Agent run spawning:**
```rust
impl App {
    pub fn submit_prompt(&mut self) {
        let task = self.editor.take_text();
        if task.is_empty() { return; }

        let cancellation = CancellationToken::new();
        let cancel_clone = cancellation.clone();

        // Build LoopContext
        let mut ctx = LoopContext {
            config: &self.runtime.config.agent,
            memory: &mut self.runtime.memory,
            llm: &self.runtime.llm,
            tools: &self.runtime.dispatcher,
            workspace: &self.runtime.workspace,
            event_sink: &self.runtime.event_sink,
            summarizer: &self.runtime.summarizer,
            cancellation: &cancellation,
            registry: &self.runtime.registry,
            max_refinement_iterations: self.runtime.config.agent.loop_config.max_refinement_iterations,
            max_delegation_depth: self.runtime.config.agent.loop_config.max_delegation_depth,
            delegation_depth: 0,
            steer: self.steer_receiver.take(),
            steer_limits: None,
            // ^─ only one run at a time, so take() is safe
        };

        let run_id = self.next_run_id;
        self.next_run_id += 1;
        self.state = AppState::Running { run_id, cancel_requested: false };

        // Spawn the agent loop in a tokio task
        let app_tx = self.event_tx.clone();
        tokio::spawn(async move {
            let loop_impl = SimpleReActLoop;
            let result = loop_impl.run(task.clone(), &mut ctx).await;
            let _ = app_tx.send(AppEvent::RunFinished {
                run_id,
                result,
            });
        });
    }
}
```

**Steering integration — CORRECTED:**

`SteeringSender` uses `inject(SteeringEvent)` (sync), not `.send().await`:

```rust
// When user types while agent is running, send InjectGuidance context event
if let AppState::Running { .. } = &self.state {
    if let Some(steer_tx) = &self.steer_sender {
        let _ = steer_tx.inject(SteeringEvent::Context(
            SteeringContextEvent::InjectGuidance {
                text: user_input,
                as_system: false,
                source: "human".into(),
            }
        ));
    }
}
```

**Events table (unchanged from revision):**

| Source | Event | Handler |
|---|---|---|
| crossterm | `KeyEvent` | Route to focused widget or global keybind |
| crossterm | `Resize(w,h)` | Update terminal size, re-render |
| crossterm | `FocusGained/FocusLost` | IME cursor visibility |
| crossterm | `Paste(String)` | Insert into editor |
| FrontendEvent | `RunStarted { task }` | State → Running, show in transcript |
| FrontendEvent | `RunFinished { text }` | State → Idle, render final answer |
| FrontendEvent | `ToolCallStarted { .. }` | Add collapsible tool block |
| FrontendEvent | `ToolCallFinished { .. }` | Update tool block with result |
| FrontendEvent | `LlmTokenDelta { delta }` | Stream token into last assistant message |
| FrontendEvent | `Error { message }` | Show error in transcript |
| FrontendEvent | `LoopDelegated { .. }` | Show delegation indicator |
| FrontendEvent | `MemoryCompressed { .. }` | Show compression notification |
| Tick | `()` | Animate loader, blink cursor |

**Key event routing priority:**
1. Global keys (Ctrl+C cancel, q quit when idle, F1 help)
2. Active overlay (if any)
3. Focused widget (editor or transcript scroll)

**Files:**
- `crates/duga-tui/src/app.rs` (new)
- `crates/duga-tui/src/events.rs` (new)

**Estimated effort:** 5 hours (increased from 4 to account for crossterm threading + run spawning)

---

### TASK-17.4: Transcript pane and input composer [UPDATE]

**Description:** Build the transcript and input composer. The TUI must maintain a `tool_descriptions` map because `FrontendEvent::ToolCallFinished.description` is always empty in the current runtime (Issue #3).

**IMPORTANT: tool_descriptions lookup:**
```rust
/// In App state:
tool_descriptions: HashMap<String, String>, // tool_call_id → description

// On ToolCallStarted:
tool_descriptions.insert(event.tool_call_id.clone(), event.description.clone());

// On ToolCallFinished:
let description = tool_descriptions.remove(&event.tool_call_id)
    .unwrap_or_else(|| event.tool_name.clone());
```

**Transcript items:**
```rust
pub enum TranscriptItem {
    SystemMessage {
        text: String,
        level: SystemLevel,
        timestamp: Instant,
    },
    UserMessage {
        text: String,
        timestamp: Instant,
    },
    AssistantMessage {
        text: String,
        timestamp: Instant,
        is_streaming: bool,
    },
    ToolCallBlock {
        tool_call_id: String,
        tool_name: String,
        description: String,   // populated from tool_descriptions map
        is_running: bool,
        is_success: Option<bool>,
        is_expanded: bool,
        timestamp: Instant,
    },
    DelegationNotice {
        from: String,
        to: String,
        reason: String,
        depth: u32,
    },
    MemoryNotice {
        before_tokens: usize,
        after_tokens: usize,
    },
}
```

**Markdown rendering** (pulldown-cmark → ratatui):
- Walk `pulldown_cmark::Parser` event stream
- Map `Heading` → bold + color, `Code` → inverse background, `Strong` → bold, `Emphasis` → italic
- Word-wrap output to terminal width using `textwrap` or custom `wrap_text`
- Code blocks get background fill
- User messages styled differently from assistant messages

**Input composer:**
```rust
pub struct InputComposer {
    buffer: String,
    cursor: usize,           // byte offset within buffer
    scroll_offset: usize,    // vertical scroll position
    history: Vec<String>,
    history_index: Option<usize>,
    placeholder: String,
    disabled: bool,           // true while agent is running
}
```

**Text utilities:**
- `visible_width(s: &str) -> usize` — Display width (use `unicode_width`)
- `wrap_text(s: &str, width: usize) -> Vec<String>` — Word-wrap for rendering

**Files:**
- `crates/duga-tui/src/transcript.rs` (new)
- `crates/duga-tui/src/editor.rs` (new)
- `crates/duga-tui/src/markdown.rs` (new)
- `crates/duga-tui/src/text.rs` (new)

**Estimated effort:** 9 hours (increased from 8 due to tool_descriptions map workaround)

---

### TASK-17.5: TUI event renderer for live progress [UPDATE]

**Description:** Map `FrontendEvent` values to `TranscriptItem` updates. Tool calls render as collapsible bordered blocks.

**Collapsible tool call block rendering:**
```
┌─ 🔧 run_shell ──────────────────────────────────┐
│ label: Check OS version                          │
│                                                   │
│ $ cat /etc/os-release                             │
│ PRETTY_NAME="Alpine Linux v3.21"                 │
│ ✓ Exit: 0                                          │
└───────────────────────────────────────────────────┘
```

States handled via tool_descriptions map from TASK-17.4:
- **Running** (ToolCallStarted received, ToolCallFinished pending): Always expanded, spinner icon
- **Completed** (ToolCallFinished, success=true): Collapsed by default, shows ✓ and tool name
- **Error** (ToolCallFinished, success=false): Always expanded, shows ✗ and error

**Streaming tokens** (LlmTokenDelta):
- Append delta to the currently rendering `AssistantMessage`'s text
- Re-render the markdown for that message on each tick (debounced to 50ms via tick rate)

**Files:**
- `crates/duga-tui/src/renderer.rs` (new)
- `crates/duga-tui/src/widgets/tool_block.rs` (new)

**Estimated effort:** 4 hours

---

### TASK-17.6: Multiple runs and session history [KEEP]

**No substantive changes.** Maintain in-memory run history indexed by run_id. Persistent history uses the replay system (duga-replay).

**Bonus:** Support resuming from a prior session by loading messages from JSONL replay files via `duga_replay::JsonlReader::read()` and populating memory via `runtime.memory_mut().push_msg(...)`.

**Estimated effort:** 2 hours

---

### TASK-17.7: Confirmation dialogs and overlay system [NEW]

**Description:** Implement overlay system + confirmation dialogs for risky tool execution.

**IMPORTANT:** `ConfirmationMiddleware` is defined in `duga-tools` and re-exported by `duga-runtime`. The TUI should use the re-export: `use duga_runtime::confirmation::*`.

**Overlay architecture:**
```rust
pub trait Overlay {
    fn render(&self, area: Rect, buf: &mut Buffer);
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction;
    fn position(&self, terminal_width: u16, terminal_height: u16) -> Rect;
}

pub enum OverlayAction {
    Consumed,
    Close,       // Close this overlay
    Ignored,     // Pass to next overlay or widget
}

pub struct OverlayManager {
    stack: Vec<Box<dyn Overlay>>,
}
```

**ConfirmationDialog overlay:**
```rust
pub struct ConfirmationDialog {
    title: String,
    prompt: String,
    options: Vec<String>,
    selected: usize,
    result_tx: oneshot::Sender<ConfirmationResult>,
}
```

**Integration with ConfirmationMiddleware:**
```rust
/// TUI implements ConfirmationProvider and shows overlay
pub struct TuiConfirmationProvider {
    overlay_tx: mpsc::UnboundedSender<ConfirmationRequest>,
}

#[async_trait::async_trait]
impl ConfirmationProvider for TuiConfirmationProvider {
    async fn confirm(
        &self,
        request: &ConfirmationRequest,
        timeout: Duration,
    ) -> ConfirmationDecision {
        // Send request to TUI overlay manager
        // Wait for user response with timeout
        // Return decision
    }
}
```

This is then wired into `ConfirmationMiddleware::new(policy, Arc::new(tui_provider))`.

**Overlay types:**
1. `ConfirmationDialog` — Yes/No/Cancel for risky tool execution
2. `HelpOverlay` — Keybinding reference (TASK-17.8)
3. `SearchOverlay` — Transcript search (TASK-17.9)

**Files:**
- `crates/duga-tui/src/overlay.rs` (new)
- `crates/duga-tui/src/overlays/confirmation.rs` (new)
- `crates/duga-tui/src/overlays/mod.rs` (new)

**Estimated effort:** 6 hours

---

### TASK-17.8: Help screen overlay [NEW]

**Description:** Help screen rendered as an overlay showing keybindings and commands. The keybindings shown should be dynamically generated from the user's config rather than hardcoded.

**Files:**
- `crates/duga-tui/src/overlays/help.rs` (new)

**Estimated effort:** 2 hours

---

### TASK-17.9: Inline search overlay [NEW]

**Description:** Search within the current transcript using an overlay with a search input.

**Files:**
- `crates/duga-tui/src/overlays/search.rs` (new)

**Estimated effort:** 4 hours

---

### TASK-17.10: Keybindings customization [UPDATE]

**Description:** Allow users to customize keybindings via config file. Now integrated with `TuiConfig` and the overlay help screen.

**Key pattern matching:**
```rust
#[derive(Clone, Debug, Deserialize)]
pub struct KeyPattern {
    pub key: String,          // "enter", "ctrl-c", "f1", etc.
}

impl KeyPattern {
    /// Match against a crossterm KeyEvent
    pub fn matches(&self, event: &KeyEvent) -> bool;
}
```

**Key pattern format:**
```
"enter"         -> KeyCode::Enter
"ctrl-c"        -> KeyCode::Char('c') + Ctrl
"ctrl-shift-c"  -> KeyCode::Char('C') + Ctrl
"f1"            -> KeyCode::F(1)
"page-up"       -> KeyCode::PageUp
"escape"        -> KeyCode::Esc
"tab"           -> KeyCode::Tab
"backspace"     -> KeyCode::Backspace
"shift-enter"   -> KeyCode::Enter + Shift
```

**Files:**
- `crates/duga-config/src/config.rs` (add `KeybindingsConfig` — see TASK-17.2)
- `crates/duga-tui/src/keybindings.rs` (new)

**Estimated effort:** 3 hours

---

### TASK-17.11: IME & Unicode support [NEW]

**Description:** Implement proper IME composition tracking and CJK character handling.

**Files:**
- `crates/duga-tui/src/ime.rs` (new)
- `crates/duga-tui/src/grapheme.rs` (new)

**Dependencies:** TASK-17.4 (editor component)
**Estimated effort:** 6 hours

---

### TASK-17.12: Replay browser [KEEP]

**Description:** Browse replay sessions using `duga_replay::JsonlReader`.

**Files:**
- `crates/duga-tui/src/replay.rs` (new)

**Dependencies:** `duga-replay` crate (already exists)
**Estimated effort:** 4 hours

---

### TASK-17.13: Kitty protocol & advanced key handling [NEW — CORRECTED]

**Description:** Optional Kitty keyboard protocol support for enhanced key detection. Use crossterm's native API, not raw escape sequences.

**CORRECTED Implementation:**
```rust
use crossterm::event::{PushKeyboardEnhancementFlags, KeyboardEnhancementFlags};
use crossterm::execute;
use std::io::stdout;

/// Enable Kitty keyboard protocol if supported.
/// crossterm handles the protocol negotiation internally.
pub fn enable_kitty_protocol() -> anyhow::Result<()> {
    execute!(
        stdout(),
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
            | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
            | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
        )
    )?;
    Ok(())
}
```

**Terminal support detection:**
Rather than a protocol query that may hang, check `$TERM` or `$TERMINAL_EMULATOR`:
```rust
pub fn terminal_supports_kitty() -> bool {
    let term = std::env::var("TERM").unwrap_or_default();
    let emulator = std::env::var("TERMINAL_EMULATOR").unwrap_or_default();
    term.contains("kitty") || emulator.contains("kitty")
        || term.contains("xterm-kitty") || std::env::var("KITTY_WINDOW_ID").is_ok()
}
```

If the terminal doesn't support it, crossterm falls back to normal key events gracefully.

**Files:**
- `crates/duga-tui/src/keys.rs` (new)

**Estimated effort:** 3 hours (reduced from 4 due to crossterm-native approach)

---

### TASK-17.14: Integration tests and docs [RESTORED from original TASK-17.9]

**Description:** Add offline tests and documentation for the TUI frontend.

**Tests:**
- ratatui test backend for render assertion
- Mock LLM (`duga_replay::ReplayMockLlm`) for agent flow tests
- Event-to-TranscriptItem mapping unit tests
- Confirmation dialog state tests
- Keybinding matching tests

**Docs:**
- `docs/tui.md` — TUI setup, keybindings, config, limitations
- Update `README.md` with TUI section

**Files:**
- `crates/duga-tui/tests/tui_flow.rs` (new)
- `docs/tui.md` (new)
- `README.md` (update)

**Estimated effort:** 6 hours

---

## Updated Dependency Graph

```mermaid
graph TD
    17.1[17.1 Create duga-tui crate] --> 17.3[17.3 App state + event loop + run spawning]
    17.1 --> 17.2[17.2 TUI config + lifecycle]
    17.3 --> 17.4[17.4 Transcript + Input composer]
    17.3 --> 17.5[17.5 Event renderer]
    17.3 --> 17.7[17.7 Overlay system + Confirmations]
    17.4 --> 17.11[17.11 IME & Unicode support]
    17.4 --> 17.12[17.12 Replay browser]
    17.5 --> 17.6[17.6 Multiple runs / history]
    17.7 --> 17.8[17.8 Help screen overlay]
    17.7 --> 17.9[17.9 Search overlay]
    17.1 --> 17.13[17.13 Kitty protocol]
    17.2 --> 17.10[17.10 Keybindings customization]
    17.10 --> 17.8[17.8 Help screen (dynamic keybinds)]
    17.3 --> 17.14[17.14 Integration tests and docs]
    17.4 --> 17.14
    17.7 --> 17.14
```

---

## Estimated Total Effort

| Task | Hours | Status |
|---|---|---|
| 17.1 Crate setup | 3 | [UPDATE] |
| 17.2 Config + lifecycle | 3 | [UPDATE] |
| 17.3 App state + event loop + run spawning | 5 | [UPDATE, merged 17.6] |
| 17.4 Transcript + editor | 9 | [UPDATE, tool_descriptions fix] |
| 17.5 Event renderer | 4 | [UPDATE] |
| 17.6 Multiple runs | 2 | [KEEP] |
| 17.7 Confirmation overlays | 6 | [NEW] |
| 17.8 Help overlay | 2 | [NEW] |
| 17.9 Search overlay | 4 | [NEW] |
| 17.10 Keybindings | 3 | [UPDATE] |
| 17.11 IME + Unicode | 6 | [NEW] |
| 17.12 Replay browser | 4 | [KEEP] |
| 17.13 Kitty protocol | 3 | [NEW, corrected] |
| 17.14 Integration tests and docs | 6 | [RESTORED] |
| **Total** | **60** | (+7 from 53 in revision) |

---

## Summary of Changes from the 2025-05-26 Revision

| # | What | Why |
|---|---|---|
| 1 | Fixed crossterm `tokio::select!` pattern | `EventStream` is not `Stream`; must use `spawn_blocking` + channel |
| 2 | Fixed `SteeringSender` API usage | Uses sync `inject()`, not async `.send().await` |
| 3 | Documented `tool_descriptions` workaround | `ToolCallFinished.description` is always empty in current runtime |
| 4 | Merged original TASK-17.6 into TASK-17.3 | Run spawning is inseparable from the event loop |
| 5 | Explicitly added `tui: Option<TuiConfig>` field directive | Revision described types but not the Config struct modification |
| 6 | Corrected Kitty protocol to use crossterm API | Raw escape sequences unnecessary; crossterm handles this natively |
| 7 | Restored TASK-17.14 (tests + docs) | Original TASK-17.9 was dropped in revision |
| 8 | Increased TASK-17.4 from 8→9 hours | Extra work for `tool_descriptions` map |
| 9 | Reduced TASK-17.13 from 4→3 hours | crossterm-native approach is simpler |
| 10 | Fixed crossterm Cargo.toml features | `event-stream` feature doesn't exist in crossterm 0.28 |
