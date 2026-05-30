# EPIC-33: TUI Redesign — Modern, Dense, Professional Terminal UI

**Labels:** `epic/tui`, `epic/ux`  
**Crates:** `duga-tui` (primary), `duga-config`  
**Depends on:** EPIC-17 (Terminal UI), EPIC-32 (Thinking Streaming), EPIC-31 (Steering)

## Goal

Transform the duga TUI from a single-pane transcript+input layout into a modern, multi-pane terminal application optimized for developers spending 8+ hours/day inside the terminal. The redesign introduces a persistent sidebar with reasoning and event log panels, a rich header with model/token/cost metrics, bordered message cards, a focus model for keyboard navigation, theming, and performance optimizations.

**Current state**: single vertical split (status bar + transcript + separator + editor). Modal overlays for help, search, sessions, confirmations. No sidebar, no focus model, no theming, no footer, no event log.

**Target state** (per redesign prompt):
```
┌─ Header (1 row) ─────────────────────────────────────────────┐
│ duga-tui | qwen3-35b | Ready ● | 12.4k tokens | $0.0032     │
├─ Chat (75%) ───────────────┬── Sidebar (25%) ────────────────┤
│ ┌─ User ────────────────┐  │ Reasoning Panel                 │
│ │ Write a chess SVG     │  │ 🧠 Thinking (3.2s, 142 tokens)  │
│ └───────────────────────┘  │ Let me think about this...      │
│                            │ [collapsed by default]          │
│ ┌─ Assistant ───────────┐  │                                 │
│ │ Here's the chess SVG  │  │ Event Log                       │
│ │ ```svg                │  │ 14:32:01 ● Tool: write started  │
│ │ <svg>...</svg>        │  │ 14:32:02 ✓ Tool: write ok       │
│ │ ```                   │  │ 14:32:03 ● LLM token delta      │
│ └───────────────────────┘  │ 14:32:04 ⚠ Memory compressed    │
│                            │                                 │
├────────────────────────────┴─────────────────────────────────┤
│ ⚠ Error banner (full-width, dismissible)                     │
├─ Input (4 rows) ─────────────────────────────────────────────┤
│ > Write a chess SVG renderer in Rust...              [42/500]│
│                                                              │
├─ Footer (1 row) ─────────────────────────────────────────────┤
│ F1 Help | Ctrl+L Clear | Ctrl+R Retry | Tab Focus | q Quit  │
└──────────────────────────────────────────────────────────────┘
```

---

## Design Principles Applied

| Principle | How we apply it |
|---|---|
| **Information density** | Multi-pane layout: chat + sidebar. Header packs 5 metrics in 1 row. |
| **Readability** | Bordered message cards separate turns. Color-coded roles. Whitespace between messages. |
| **Minimal cognitive load** | Progressive disclosure: reasoning collapsed by default. Tool output hidden unless expanded. |
| **Keyboard-first navigation** | Focus model with Tab/Shift+Tab, j/k scroll, g/G top/bottom, r/e/i shortcuts. |
| **Streaming-first** | Spinner animation at 100ms intervals. Auto-scroll follows streaming. No redraw spam. |
| **Vertical space efficiency** | Header/footer are 1 row each. Sidebar reclaims thinking from inline. Error banners auto-dismiss. |
| **Progressive disclosure** | Reasoning collapsed. Tool blocks collapsed. Error details expandable. |

---

## What Changes

### Architecture Before (current)
```
App::render() → single vertical Layout:
  [Status bar 1 row]
  [Transcript (fills remaining)]
  [Separator 1 row]
  [Editor 3 rows]
+ Modal overlays on top
```

### Architecture After (target)
```
App::render() → multi-pane Layout:
  [Header 1 row]
  [Chat 75% | Sidebar 25%]
  [Error banner (conditional, 1 row)]
  [Input 4 rows]
  [Footer 1 row]
+ Modal overlays on top
+ Focus model routes keys to active pane
```

### Key Design Decisions

1. **Sidebar is persistent, not a modal** — Reasoning and event log are always accessible, not hidden behind key combos. Collapse with `r`/`e` as needed, but the panel structure stays.

2. **Focus model replaces ad-hoc key routing** — `Focus::Chat | Sidebar | Input` enum. Tab/Shift+Tab cycles. Dedicated shortcuts (`r`, `e`, `i`) jump directly. Esc returns to Chat. This eliminates the current problem where keybindings compete between editor and global actions.

3. **Message cards use ratatui Block borders** — Each user/assistant/system message wrapped in a bordered block with role-colored title. This creates clear visual separation between turns without wasting vertical space.

4. **Event log is a VecDeque, not transcript items** — System messages currently go into the transcript, mixing with conversation. The event log is a separate ring buffer (max 200) rendered in the sidebar. Only high-severity events also appear as error banners.

5. **Theme is a configurable palette** — Colors extracted into a `Theme` struct loaded from config. Every widget references `theme.colors.primary` etc. This enables custom themes without code changes.

6. **Virtualized rendering** — Only render visible messages (based on scroll position + viewport height). This is critical for long sessions (100+ messages) to maintain 60 FPS.

7. **No heap allocations in draw** — Pre-compute styles, use `&'static str` where possible, avoid `format!()` in hot paths. Use `Vec<Line<'static>>` with borrowed spans.

8. **Responsive breakpoint at 120 cols** — Below 120, sidebar collapses into overlay mode (toggle with `r`/`e` shows a floating panel). Above 120, sidebar is always visible.

---

## Development Rules

### 1. Step-by-step implementation with validation at every gate

Each task must be implemented and **validated** before moving to the next.

**Per-task gates:**
1. **Write the code** — minimal, focused diff for the task scope
2. **Compile check** — `cargo check -p duga-tui` must pass with zero warnings
3. **Run existing tests** — `cargo test -p duga-tui` must pass (no regressions)
4. **Run workspace tests** — `cargo test --workspace` must pass before merging the task
5. **Commit** — one commit per completed task with the task ID in the message

### 2. Never break existing functionality

This redesign touches the rendering path which is the user-facing surface. Every existing feature must continue working:

- Agent runs (submit, cancel, steering)
- Session management (save, load, delete)
- Overlays (help, search, session picker)
- Confirmation dialogs (tool execution, session deletion)
- Transcript items (user, assistant, system, tool, thinking, delegation, memory)
- Streaming (text deltas, thinking deltas)
- Mouse scroll
- Paste support
- All keybindings

### 3. Task dependency order is mandatory

```
Phase 1: Foundation
  TASK-33.1 (Theme system)
  TASK-33.2 (Layout engine)
      ↓
Phase 2: New Widgets
  TASK-33.3 (Header widget)
  TASK-33.4 (Chat panel — message cards)
  TASK-33.5 (Footer widget)
  TASK-33.6 (Sidebar container)
      ↓
Phase 3: Sidebar Content
  TASK-33.7 (Reasoning panel)
  TASK-33.8 (Event log)
      ↓
Phase 4: Interaction
  TASK-33.9 (Focus model)
  TASK-33.10 (Error banners)
      ↓
Phase 5: Polish
  TASK-33.11 (Input area enhancements)
  TASK-33.12 (Responsive mode)
  TASK-33.13 (Performance — virtualization)
  TASK-33.14 (Integration tests)
```

### 4. Config changes are additive

New config keys must have `#[serde(default)]` so existing config files work without changes. Example:

```yaml
tui:
  theme: "dark"            # NEW: "dark" | "light" | custom
  show_sidebar: true       # NEW: default true
  show_footer: true        # NEW: default true
  sidebar_width_pct: 25    # NEW: 15-40, default 25
```

---

## Testing Requirements

| Task | Minimum Tests Required |
|------|----------------------|
| TASK-33.1 | Theme struct creation, default palette values, color-from-config, all existing render tests pass with theme |
| TASK-33.2 | Layout engine splits area correctly at 80x24, 120x40, 200x60; sidebar width percentage rounding; min/max sidebar constraints |
| TASK-33.3 | Header line renders with model name, status dot, token count; truncation on narrow terminals |
| TASK-33.4 | Message card renders with border + role-colored title; user=cyan, assistant=green, system=blue, error=red; blank row between messages |
| TASK-33.5 | Footer renders shortcut bar; shortcuts match active keybindings; truncation on narrow terminals |
| TASK-33.6 | Sidebar split renders when enabled; empty state "No sidebar content" when panels are collapsed |
| TASK-33.7 | Reasoning panel shows duration + token count; collapsed state shows summary; expanded shows full text |
| TASK-33.8 | Event log pushes events; max 200 enforced; renders timestamp + severity icon + message; scroll within log |
| TASK-33.9 | Tab cycles Focus::Chat → Sidebar → Input; Shift+Tab cycles reverse; dedicated shortcuts (i, r, e, Esc) work; keys route to correct widget |
| TASK-33.10 | Error banner renders above input when error exists; dismiss key; auto-dismiss after 5s configurable |
| TASK-33.11 | Character counter shows in input area; live updates on keystroke; color changes at threshold |
| TASK-33.12 | Width < 120: sidebar hidden, overlay toggle works; width >= 120: sidebar persistent |
| TASK-33.13 | Only visible messages rendered; scroll preserves correct visible range; FPS stays stable with 500 messages |
| TASK-33.14 | Full integration: submit prompt → run agent → see messages in cards → events in log → cancel → error banner → session save/load with new layout |

---

## Files Summary

| File | Change |
|------|--------|
| `crates/duga-tui/src/lib.rs` | Add new modules: `theme`, `layout`, `header`, `chat`, `sidebar`, `footer`, `focus`, `event_log`, `banner` |
| `crates/duga-tui/src/theme.rs` | **New** — `Theme` struct, color palette, style constructors |
| `crates/duga-tui/src/layout.rs` | **New** — `LayoutManager` computing pane rects from terminal size |
| `crates/duga-tui/src/header.rs` | **New** — `HeaderWidget` rendering model/metrics line |
| `crates/duga-tui/src/chat.rs` | **New** — `ChatView` rendering bordered message cards |
| `crates/duga-tui/src/sidebar.rs` | **New** — `SidebarView` container for reasoning + event log |
| `crates/duga-tui/src/reasoning_panel.rs` | **New** — `ReasoningPanel` extracted from inline thinking blocks |
| `crates/duga-tui/src/event_log.rs` | **New** — `EventLog` struct with `VecDeque<LogEntry>`, max 200 |
| `crates/duga-tui/src/footer.rs` | **New** — `FooterView` rendering shortcut bar |
| `crates/duga-tui/src/focus.rs` | **New** — `Focus` enum + focus router |
| `crates/duga-tui/src/banner.rs` | **New** — `ErrorBanner` dismissible widget |
| `crates/duga-tui/src/app.rs` | **Modified** — Wire new widgets into render, add Focus field, add EventLog, integrate theme |
| `crates/duga-tui/src/transcript.rs` | **Modified** — Nothing removed; ChatView reads from existing transcript |
| `crates/duga-config/src/config.rs` | **Modified** — Add `theme`, `show_sidebar`, `sidebar_width_pct`, `show_footer` to `TuiConfig` |
| `crates/duga-tui/tests/e2e_tests.rs` | **Modified** — Add tests for new layout + focus + widgets |

---

## Task Breakdown

---

### TASK-33.1: Theme system with configurable color palette

- **Labels:** `layer/tui`, `priority/critical`
- **Description:** Extract all hardcoded colors from `app.rs` into a `Theme` struct. Define a color palette with named roles (primary, success, warning, error, muted, surface, text, text_dim). Load theme from `TuiConfig`. Every widget references `theme.colors.xxx` instead of raw `Color::Rgb(...)`.
- **Files affected:**
  - `crates/duga-tui/src/theme.rs` (new)
  - `crates/duga-tui/src/app.rs`
  - `crates/duga-config/src/config.rs`
- **Types involved:** `Theme`, `ThemeColors`, `TuiConfig`
- **Functions to implement:**
  - `Theme::dark()` — default dark theme matching redesign spec
  - `Theme::from_config(config: &TuiConfig) -> Self`
  - Style constructors: `theme.user_style()`, `theme.assistant_style()`, `theme.error_style()`, `theme.muted_style()`
- **Dependencies:** None
- **Implementation steps:**
  1. Define `ThemeColors` with fields: `bg`, `surface`, `primary` (cyan), `success` (green), `warning` (yellow), `error` (red), `muted` (dark gray), `text`, `text_dim`, `border`, `accent`
  2. Define `Theme` struct holding `colors: ThemeColors` plus style constructors
  3. Implement `Theme::dark()` with exact values from redesign spec: bg = RGB(10,12,16)
  4. Add `theme: Option<String>` to `TuiConfig` with `#[serde(default)]`
  5. Wire theme into `App::new()` and store as `app.theme`
  6. Replace hardcoded `Color::Cyan`, `Color::Green`, etc. in `app.rs` render methods with `self.theme.colors.primary` etc.
  7. `cargo test -p duga-tui` passes (all render tests unchanged — same colors, different source)
- **Edge cases:**
  - Config specifies unknown theme name → fall back to `Theme::dark()` with a warning
  - Config has no `theme` key → default to `Theme::dark()`
- **Definition of Done:** All colors come from Theme. Config supports theme selection. No visual change in default mode.
- **Acceptance criteria:**
  - `Theme::dark()` produces exact colors from spec
  - `app.render()` uses `self.theme` for all color decisions
  - Existing configs without `theme` key work identically
  - `cargo test -p duga-tui` passes
- **Test plan:**
  - unit: `Theme::dark()` struct has correct RGB values
  - unit: `Theme::from_config()` with `"dark"` returns dark theme
  - unit: `Theme::from_config()` with unknown name returns dark theme (fallback)
  - unit: Style constructors produce correct ratatui `Style` values
- **Estimated effort:** 3 hours
- **Depends on:** None

---

### TASK-33.2: Layout engine — multi-pane responsive layout

- **Labels:** `layer/tui`, `priority/critical`
- **Description:** Implement a `LayoutManager` that computes pane rectangles from terminal dimensions. Supports header (1 row), chat (variable), sidebar (25% width), error banner (conditional 1 row), input (4 rows), footer (1 row). When width < 120, sidebar collapses to 0 width. When sidebar is hidden, chat takes 100% width.
- **Files affected:**
  - `crates/duga-tui/src/layout.rs` (new)
  - `crates/duga-tui/src/app.rs`
- **Types involved:** `LayoutManager`, `PaneRects`
- **Functions to implement:**
  - `LayoutManager::new(config: &TuiConfig) -> Self`
  - `LayoutManager::compute(term_width: u16, term_height: u16, show_banner: bool) -> PaneRects`
  - `PaneRects` struct with fields: `header`, `chat`, `sidebar`, `banner`, `input`, `footer`
- **Dependencies:** TASK-33.1 (theme not strictly needed but config integration is)
- **Implementation steps:**
  1. Define `PaneRects` struct with all area fields
  2. Implement `compute()` using ratatui `Layout`:
     ```
     Vertical: [Length(1) header, Min(5) chat, Length(0|1) banner, Length(4) input, Length(1) footer]
     Chat row split horizontally: [Percentage(100-pct) chat, Percentage(pct) sidebar]
     ```
  3. When `terminal_width < 120` (configurable threshold), set sidebar width to 0
  4. When `show_banner` is false, banner constraint is Length(0)
  5. Store `LayoutManager` in `App` and call `compute()` at the start of `render()`
  6. Pass `PaneRects` to each widget render function
  7. `cargo test -p duga-tui` passes
- **Edge cases:**
  - Terminal too short for 4-row input: min constraints collapse gracefully (ratatui handles this)
  - Terminal < 80 cols: header may truncate, sidebar always hidden
  - Resize event: recompute layout on next render (no special handling needed)
- **Definition of Done:** Layout engine produces correct pane rects for all terminal sizes. Sidebar collapses below threshold.
- **Acceptance criteria:**
  - 200x60 terminal: sidebar is 50 cols wide (25%)
  - 80x24 terminal: sidebar is 0 cols wide (below threshold)
  - 120x40 terminal: sidebar is 30 cols wide (25%)
  - Banner height is 0 when `show_banner = false`
  - Banner height is 1 when `show_banner = true`
- **Test plan:**
  - unit: `compute(200, 60, false)` → correct rects
  - unit: `compute(80, 24, false)` → sidebar width = 0
  - unit: `compute(120, 24, false)` → sidebar width = 30 (25%)
  - unit: `compute(200, 60, true)` → banner rect is 1 row, input shifts down
  - unit: Resize from 200→80 cols → sidebar collapses
- **Estimated effort:** 3 hours
- **Depends on:** TASK-33.1 (for config access)

---

### TASK-33.3: Header widget — model, status, tokens, cost, session

- **Labels:** `layer/tui`, `priority/high`
- **Description:** Implement `HeaderWidget` that renders a single-line header showing: app name (cyan), current model, status dot (green ● / yellow ◌ / red ✗), token count, estimated cost, and current session name. Format: `duga-tui | qwen3-35b | Ready ● | 12.4k tokens | $0.0032 | Session: chess-svg`.
- **Files affected:**
  - `crates/duga-tui/src/header.rs` (new)
  - `crates/duga-tui/src/app.rs`
- **Types involved:** `HeaderWidget`, `AppState`
- **Functions to implement:**
  - `HeaderWidget::render(area: Rect, buf: &mut Buffer, app: &App, theme: &Theme)`
- **Dependencies:** TASK-33.2 (layout), TASK-33.1 (theme)
- **Implementation steps:**
  1. Create `HeaderWidget` with no state (stateless render)
  2. Extract model name from `app.config.model` (or active provider)
  3. Status dot: `●` green when Idle, `◌` yellow when Running, `✗` red on error
  4. Token count: cumulative from transcript or last run's usage
  5. Cost: compute from token count × provider pricing (hardcode approximate rates for now)
  6. Session name: from `app.current_session_id` → lookup title from session index
  7. Render as single `Paragraph` with styled `Span`s
  8. Truncate with `…` if terminal too narrow
  9. Wire into `App::render()` using `pane_rects.header`
- **Edge cases:**
  - No active session → show "Session: —"
  - Token count unknown → show "— tokens"
  - Very narrow terminal (< 60 cols) → show only "duga-tui | model"
- **Definition of Done:** Header renders with correct metrics, updates on state changes, handles all edge cases.
- **Acceptance criteria:**
  - Idle state shows green ● and "Ready"
  - Running state shows yellow ◌ and "Running…"
  - Session name updates when switching sessions
  - Narrow terminal truncates gracefully
- **Test plan:**
  - unit: Header renders with Idle state → contains "Ready ●"
  - unit: Header renders with Running state → contains "Running… ◌"
  - unit: Header with session → contains session title
  - unit: Header without session → contains "Session: —"
  - unit: Truncation at width 40 → no panic, shows abbreviated text
- **Estimated effort:** 3 hours
- **Depends on:** TASK-33.2

---

### TASK-33.4: Chat panel — bordered message cards

- **Labels:** `layer/tui`, `priority/critical`
- **Description:** Rewrite transcript rendering to use bordered message cards instead of plain label prefixes. Each user/assistant/system/error message is wrapped in a `ratatui::widgets::Block` with a colored border and role title. Assistant messages include nested thinking blocks. Empty row between messages. Code blocks rendered separately with syntax highlighting placeholder.
- **Files affected:**
  - `crates/duga-tui/src/chat.rs` (new)
  - `crates/duga-tui/src/app.rs` (move rendering out)
- **Types involved:** `ChatView`, `TranscriptItem`
- **Functions to implement:**
  - `ChatView::render(area: Rect, buf: &mut Buffer, transcript: &Transcript, scroll: &ScrollState, theme: &Theme)`
  - `ChatView::render_message_card(…)` — per-item card renderer
  - `ChatView::render_code_block(…)` — code block with language label
- **Dependencies:** TASK-33.2 (layout), TASK-33.1 (theme)
- **Implementation steps:**
  1. Create `ChatView` struct (stateless renderer)
  2. For each visible transcript item, create a `Block` with:
     - Border style: role color
     - Title: role name (e.g., "You" cyan, "Assistant" green, "System" blue, "Error" red)
     - Title style: bold, role color
  3. Inside block, render content with 2-space left padding
  4. For `UserMessage`: plain text
  5. For `AssistantMessage`: markdown-rendered text + nested ThinkingBlock if adjacent
  6. For `SystemMessage`: dimmed text with severity icon (ℹ ⚠ ✗)
  7. For `ToolCallBlock`: collapsible within card or as separate card
  8. For `ThinkingBlock` standalone: card with dimmed border
  9. Add 1 blank row between cards
  10. Code blocks: render with `Block` border and language label, monospace content
  11. Move `render_transcript()` logic from `app.rs` into `ChatView`
- **Edge cases:**
  - Message wider than chat area → wrap within card inner area
  - Very long single-line message → wrap at card width
  - Empty message text → still render card with "(empty)" placeholder
  - Streaming message → card border shows spinner in title
- **Definition of Done:** All messages render as bordered cards with correct role colors. Code blocks are visually distinct. No regressions in content rendering.
- **Acceptance criteria:**
  - User card has cyan border + "You" title in bold cyan
  - Assistant card has green border + "Assistant" title in bold green
  - System card has blue/dimmed border + severity icon
  - Error card has red border + "Error" title
  - Blank row between consecutive cards
  - Code blocks have gray border + language identifier
  - Thinking blocks inside assistant cards are dimmed and collapsible
  - All existing transcript content renders correctly (markdown, tool blocks, delegation notices, memory notices)
- **Test plan:**
  - unit: User card renders with correct border color and title
  - unit: Assistant card with markdown content renders correctly
  - unit: Two consecutive cards have empty line between them
  - unit: Code block renders with language label
  - unit: Error card renders with red border
  - unit: Streaming assistant card shows spinner indicator
  - unit: Existing e2e tests pass (content assertions updated for card borders)
- **Estimated effort:** 6 hours
- **Depends on:** TASK-33.2

---

### TASK-33.5: Footer widget — shortcut bar

- **Labels:** `layer/tui`, `priority/high`
- **Description:** Implement `FooterView` that renders a single-line footer showing keyboard shortcuts. Format: `F1 Help | Ctrl+L Clear | Ctrl+R Retry | Tab Focus | q Quit`. Shortcuts are context-aware: show different shortcuts based on current focus and app state.
- **Files affected:**
  - `crates/duga-tui/src/footer.rs` (new)
  - `crates/duga-tui/src/app.rs`
- **Types involved:** `FooterView`, `Focus`, `AppState`
- **Functions to implement:**
  - `FooterView::render(area: Rect, buf: &mut Buffer, focus: Focus, state: &AppState, theme: &Theme)`
- **Dependencies:** TASK-33.2 (layout), TASK-33.1 (theme), TASK-33.9 (Focus enum)
- **Implementation steps:**
  1. Create `FooterView` (stateless renderer)
  2. Define shortcut sets based on context:
     - **Global (always shown)**: `F1 Help | q Quit`
     - **Idle + Chat focus**: `j/k Scroll | g/G Top/Bottom | Tab→Input | i Input`
     - **Idle + Input focus**: `Enter Submit | Esc→Chat | ↑↓ History`
     - **Running**: `Ctrl+C Cancel | Ctrl+R Retry | s Steer`
     - **Sidebar focus**: `j/k Scroll | Tab→Input | r Toggle Reason | e Toggle Events`
  3. Render as single `Paragraph` with dimmed text on subtle background
  4. Truncate with `…` on narrow terminals
  5. Separator between groups: `|`
- **Edge cases:**
  - Terminal < 80 cols → show only most important shortcuts
  - Overlay visible → hide footer or show overlay-specific shortcuts
- **Definition of Done:** Footer shows context-appropriate shortcuts that update on focus/state change.
- **Acceptance criteria:**
  - Idle + Chat focus shows scroll + focus navigation shortcuts
  - Running state shows cancel/retry shortcuts
  - Input focus shows submit/history shortcuts
  - Shortcuts match actual configured keybindings (not hardcoded)
- **Test plan:**
  - unit: Footer with Idle + Chat focus → contains "j/k Scroll" and "Tab→Input"
  - unit: Footer with Running state → contains "Ctrl+C Cancel"
  - unit: Footer with Input focus → contains "Enter Submit"
  - unit: Narrow terminal truncation → no panic
- **Estimated effort:** 3 hours
- **Depends on:** TASK-33.2, TASK-33.9 (can start before 33.9, just pass placeholder Focus)

---

### TASK-33.6: Sidebar container — split pane with resize handle

- **Labels:** `layer/tui`, `priority/high`
- **Description:** Implement `SidebarView` container that splits the sidebar area into two vertical panels: reasoning panel (top) and event log (bottom). Each panel can be collapsed independently. When both are collapsed, the sidebar shows an empty state. The divider between panels is configurable (50/50 default).
- **Files affected:**
  - `crates/duga-tui/src/sidebar.rs` (new)
  - `crates/duga-tui/src/app.rs`
- **Types involved:** `SidebarView`, `SidebarState`
- **Functions to implement:**
  - `SidebarView::new() -> Self`
  - `SidebarView::toggle_reasoning(&mut self)`
  - `SidebarView::toggle_events(&mut self)`
  - `SidebarView::render(area: Rect, buf: &mut Buffer, ...)`
- **Dependencies:** TASK-33.2 (layout provides sidebar rect)
- **Implementation steps:**
  1. Define `SidebarState` with `reasoning_expanded: bool`, `events_expanded: bool`
  2. Compute sub-panes: if both expanded, 50/50 split; if one collapsed, other takes 100%
  3. If both collapsed, render "No sidebar content — press r for reasoning, e for events" in dimmed text
  4. Panel headers: "Reasoning" and "Event Log" with toggle hint
  5. Empty panel states: "No reasoning data" / "No events yet"
  6. Panel borders: dimmed style, distinct from chat cards
  7. Render reasoning panel content via `ReasoningPanel` (TASK-33.7)
  8. Render event log content via `EventLog` (TASK-33.8)
- **Edge cases:**
  - Sidebar collapsed (width 0 in responsive mode) → don't render at all
  - Only reasoning panel expanded → event log area shows collapsed header
  - Terminal too short for both panels → min 3 rows per panel, scroll if needed
- **Definition of Done:** Sidebar renders with toggle-able reasoning and event log panels.
- **Acceptance criteria:**
  - Both panels expanded → 50/50 vertical split
  - Reasoning collapsed → event log takes full sidebar height
  - Both collapsed → empty state message
  - Sidebar hidden (width=0) → nothing rendered, no panic
- **Test plan:**
  - unit: Both expanded → correct sub-rects
  - unit: One collapsed → other takes full height
  - unit: Both collapsed → empty state text rendered
  - unit: Zero-width area → no-op render
- **Estimated effort:** 4 hours
- **Depends on:** TASK-33.2

---

### TASK-33.7: Reasoning panel — extracted from inline thinking

- **Labels:** `layer/tui`, `priority/high`
- **Description:** Render the active/previous thinking block in the sidebar reasoning panel instead of (or in addition to) inline within assistant messages. Shows duration (time from first to last thinking delta), token count, and the thinking text. Collapsed by default after completion — shows summary only. During active streaming, auto-expands.
- **Files affected:**
  - `crates/duga-tui/src/reasoning_panel.rs` (new)
  - `crates/duga-tui/src/app.rs`
  - `crates/duga-tui/src/chat.rs` (optionally hide thinking from chat when sidebar visible)
- **Types involved:** `ReasoningPanel`, `TranscriptItem::ThinkingBlock`
- **Functions to implement:**
  - `ReasoningPanel::new() -> Self`
  - `ReasoningPanel::update(thinking_block: &ThinkingBlock)`
  - `ReasoningPanel::render(area: Rect, buf: &mut Buffer, theme: &Theme)`
- **Dependencies:** TASK-33.6 (sidebar container)
- **Implementation steps:**
  1. Create `ReasoningPanel` struct holding reference to latest thinking block
  2. Compute duration from first thinking delta timestamp to finish
  3. Count thinking tokens (words as proxy, or actual token count if available)
  4. Render header: "🧠 Reasoning (3.2s, 142 tokens)" with dimmed italic style
  5. When collapsed: show only header + first line of thinking text with "…"
  6. When expanded: show full thinking text with word-wrap at panel width
  7. During streaming: auto-expand, auto-scroll to bottom
  8. Scroll support within panel: j/k when sidebar focused
  9. In `ChatView`: if sidebar is visible and reasoning panel is expanded, optionally show a condensed "See reasoning in sidebar →" hint instead of full thinking block
- **Edge cases:**
  - No thinking data → "No reasoning data for this run"
  - Multiple thinking blocks (compaction scenario) → show latest, "N previous" indicator
  - Thinking text is very long → scroll within panel
- **Definition of Done:** Reasoning panel shows thinking content with duration and token count. Updates during streaming. Collapsible.
- **Acceptance criteria:**
  - During streaming thinking → panel auto-expands, shows live text
  - After thinking finishes → panel auto-collapses to summary
  - Summary shows duration and word/token count
  - Expanded view shows full thinking text with scroll
  - Toggle with `r` key (sidebar focus) or global `r`
- **Test plan:**
  - unit: Panel with active thinking block → renders "🧠 Reasoning (streaming…)" with live text
  - unit: Panel with completed thinking block → renders collapsed summary with duration + count
  - unit: Toggle expand/collapse → renders full text
  - unit: Empty state → "No reasoning data"
- **Estimated effort:** 4 hours
- **Depends on:** TASK-33.6

---

### TASK-33.8: Event log — real-time structured log in sidebar

- **Labels:** `layer/tui`, `priority/high`
- **Description:** Implement an `EventLog` that captures structured events (tool calls, LLM requests, memory compressions, errors, delegations) in a `VecDeque<LogEntry>` with max 200 entries. Render in the sidebar with timestamp, severity icon, and message. Supports scroll. Events are pushed from `App::handle_frontend_event()`.
- **Files affected:**
  - `crates/duga-tui/src/event_log.rs` (new)
  - `crates/duga-tui/src/app.rs`
- **Types involved:** `EventLog`, `LogEntry`, `LogLevel`
- **Functions to implement:**
  - `EventLog::new(max_entries: usize) -> Self`
  - `EventLog::push(&mut self, entry: LogEntry)`
  - `EventLog::render(area: Rect, buf: &mut Buffer, theme: &Theme, scroll_offset: usize)`
- **Dependencies:** TASK-33.6 (sidebar container)
- **Implementation steps:**
  1. Define `LogEntry`:
     ```rust
     struct LogEntry {
         timestamp: Instant,
         level: LogLevel,
         icon: &'static str,   // ● ◌ ✓ ✗ ⚠ ℹ 🧠
         message: String,
     }
     ```
  2. Define `LogLevel`: `Info | Success | Warning | Error | Debug`
  3. Implement `EventLog` with `VecDeque<LogEntry>`, max capacity enforcement
  4. In `App::handle_frontend_event()`, push entries:
     - `ToolCallStarted` → level Info, icon "●", "Tool: {name} started"
     - `ToolCallFinished { success: true }` → level Success, icon "✓", "Tool: {name} ok"
     - `ToolCallFinished { success: false }` → level Error, icon "✗", "Tool: {name} failed"
     - `LlmThinkingDelta` (first) → level Debug, icon "🧠", "LLM thinking…"
     - `LlmTokenDelta` (first) → level Debug, icon "●", "LLM streaming…"
     - `MemoryCompressed` → level Warning, icon "⚠", "Memory: {before}→{after} tokens"
     - `LoopDelegated` → level Info, icon "→", "{from} → {to}: {reason}"
     - `Error` → level Error, icon "✗", "{message}"
  5. Render: each entry is one line `HH:MM:SS ICON message` in dimmed/monospace style
  6. Latest entry at bottom, scroll up for history
  7. Focus sidebar → j/k scrolls event log, not chat
  8. Color-code by level: Info=dim, Success=green, Warning=yellow, Error=red
- **Edge cases:**
  - 200+ entries → oldest evicted (FIFO)
  - Rapid events (many token deltas) → deduplicate: only log first token delta, not every one
  - Empty log → "No events yet"
  - Very long messages → truncate to panel width with "…"
- **Definition of Done:** Event log captures structured events, renders with timestamps and icons, scrolls, respects max capacity.
- **Acceptance criteria:**
  - Tool start → "● Tool: write started" appears in log
  - Tool success → "✓ Tool: write ok" appears
  - Tool failure → "✗ Tool: write failed" appears in red
  - Memory compression → "⚠ Memory: 5000→2000 tokens" appears
  - 200+ events → oldest removed, newest kept
  - Log is scrollable with j/k when sidebar focused
- **Test plan:**
  - unit: `push()` adds entry, `len()` increases
  - unit: 201st entry evicts 1st entry, `len()` stays at 200
  - unit: `push()` with different levels → correct icons
  - unit: Rendering shows timestamps, icons, messages in correct order
  - unit: Scroll offset skips oldest N entries
- **Estimated effort:** 5 hours
- **Depends on:** TASK-33.6

---

### TASK-33.9: Focus model — keyboard navigation router

- **Labels:** `layer/tui`, `priority/critical`
- **Description:** Implement a `Focus` enum (`Chat | Sidebar | Input`) and a focus router that directs key events to the correct widget. Tab/Shift+Tab cycles through focus. Dedicated shortcuts: `i` → Input, `r` → Sidebar (reasoning toggle), `e` → Sidebar (events toggle), `Esc` → Chat. The focus model replaces the ad-hoc key routing in `App::handle_key()`.
- **Files affected:**
  - `crates/duga-tui/src/focus.rs` (new)
  - `crates/duga-tui/src/app.rs`
- **Types involved:** `Focus`, `FocusRouter`
- **Functions to implement:**
  - `Focus::next()` — Tab order: Chat → Sidebar → Input → Chat
  - `Focus::prev()` — Shift+Tab: Chat → Input → Sidebar → Chat
  - `FocusRouter::route(key: &KeyEvent, focus: Focus, app: &mut App) -> bool` — returns true if consumed
- **Dependencies:** None (pure logic, integrates with existing key handler)
- **Implementation steps:**
  1. Define `Focus` enum: `Chat, Sidebar, Input`
  2. Implement `Focus::next()` and `Focus::prev()` as methods
  3. Add `focus: Focus` field to `App`, default `Focus::Chat`
  4. In `App::handle_key()`, add focus routing **before** existing overlay/global checks:
     ```
     // 0.5. Focus-mode keys (Tab, Shift+Tab, dedicated shortcuts)
     match key {
       Tab => self.focus = self.focus.next(),
       Shift+Tab => self.focus = self.focus.prev(),
       'i' if no overlay => self.focus = Focus::Input,
       'r' if no overlay => {
         self.focus = Focus::Sidebar;
         self.sidebar.toggle_reasoning();
       }
       'e' if no overlay => {
         self.focus = Focus::Sidebar;
         self.sidebar.toggle_events();
       }
       Esc => self.focus = Focus::Chat,
       _ => {}
     }
     ```
  5. After focus routing, route keys based on focus:
     - `Focus::Chat` → scroll keys (j/k/g/G), forwarding to transcript scroll
     - `Focus::Sidebar` → scroll keys for event log + reasoning panel, r/e toggles
     - `Focus::Input` → all keys to editor (except global shortcuts)
  6. Global shortcuts (Ctrl+C, Ctrl+L, F1, q) still work regardless of focus
  7. Mouse click on a pane sets focus to that pane (future enhancement, not required)
- **Edge cases:**
  - Overlay visible → focus routing disabled, overlay captures all keys
  - Running state → Tab still works, user can read sidebar while agent runs
  - Sidebar hidden (responsive mode) → Focus::Sidebar skipped in Tab cycle
- **Definition of Done:** Tab/Shift+Tab cycle focus correctly. Dedicated shortcuts jump to specific panes. Keys route to correct widget.
- **Acceptance criteria:**
  - Default focus is Chat
  - Tab: Chat → Sidebar → Input → Chat
  - Shift+Tab: Chat → Input → Sidebar → Chat
  - `i` jumps to Input from any focus
  - `Esc` returns to Chat from any focus
  - `r`/`e` open sidebar + toggle panels + set focus to Sidebar
  - Chat focus: j/k scrolls transcript
  - Sidebar focus: j/k scrolls event log / reasoning
  - Input focus: typing goes to editor
  - Overlay open: focus routing disabled
  - Sidebar hidden (width=0): focus skips Sidebar
  - All existing global shortcuts (Ctrl+C, Ctrl+L, F1, q) work in every focus mode
- **Test plan:**
  - unit: `Focus::next()` cycles correctly
  - unit: `Focus::prev()` cycles correctly
  - unit: Tab key routes to next focus
  - unit: `i` key sets Focus::Input
  - unit: `Esc` sets Focus::Chat
  - unit: Key routing: j/k scroll chat in Chat mode, edit buffer in Input mode
  - unit: Overlay blocks focus routing
  - unit: Existing keybinding tests pass
- **Estimated effort:** 4 hours
- **Depends on:** TASK-33.6 (sidebar exists for focus to target)

---

### TASK-33.10: Error banners — dismissible full-width notifications

- **Labels:** `layer/tui`, `priority/high`
- **Description:** Implement `ErrorBanner` that displays a full-width, dismissible banner above the input area for errors, warnings, and cancellations. Banner auto-dismisses after a configurable timeout (default 5 seconds) or on user dismissal key (Enter/Esc). Multiple concurrent banners stack? No — show only the most recent.
- **Files affected:**
  - `crates/duga-tui/src/banner.rs` (new)
  - `crates/duga-tui/src/app.rs`
- **Types involved:** `ErrorBanner`, `BannerLevel`
- **Functions to implement:**
  - `ErrorBanner::new() -> Self`
  - `ErrorBanner::show(&mut self, level: BannerLevel, message: String)`
  - `ErrorBanner::dismiss(&mut self)`
  - `ErrorBanner::tick(&mut self)` — auto-dismiss countdown
  - `ErrorBanner::render(area: Rect, buf: &mut Buffer, theme: &Theme)`
- **Dependencies:** TASK-33.2 (layout provides banner rect)
- **Implementation steps:**
  1. Define `BannerLevel`: `Info, Warn, Error, Cancel`
  2. Define `ErrorBanner` with `active: Option<(BannerLevel, String, Instant)>` and `auto_dismiss_secs: u64`
  3. In `App::handle_frontend_event()`, show banner for:
     - `FrontendEvent::Error { message }` → BannerLevel::Error
     - Cancellation → BannerLevel::Cancel, message "Request cancelled. Press Ctrl+R to retry."
     - Memory compression (optional) → BannerLevel::Info
  4. In `App::handle_tick()`, check auto-dismiss timeout
  5. Render banner as full-width `Paragraph` with colored background:
     - Error: red bg, white text
     - Warn: yellow bg, black text
     - Cancel: yellow bg, black text
     - Info: blue bg, white text
  6. Include hint text: "Press Enter to dismiss" or countdown "Dismissing in 3s…"
  7. Dismiss on: Enter key (when no overlay) or auto-timeout
  8. Layout engine: `show_banner` parameter maps to `banner.is_active()`
- **Edge cases:**
  - New banner while one is active → replace (don't stack)
  - User dismisses → clear immediately
  - Auto-dismiss at 0 → clear, don't render
  - Very long message → truncate to 2 lines max with "…"
- **Definition of Done:** Errors/cancellations show dismissible banners above input. Auto-dismiss works.
- **Acceptance criteria:**
  - Agent error → red banner with error message
  - Cancellation → yellow banner "Request cancelled. Press Ctrl+R to retry."
  - Enter dismisses banner
  - Banner auto-dismisses after 5 seconds
  - New banner replaces old banner (no stacking)
  - No banner → banner area is 0 height
- **Test plan:**
  - unit: `show()` sets active banner
  - unit: `dismiss()` clears banner
  - unit: `tick()` after 5 seconds dismisses auto-dismiss banner
  - unit: `show()` while active replaces banner
  - unit: Render produces correct background color per level
  - unit: Layout includes banner row when active
- **Estimated effort:** 3 hours
- **Depends on:** TASK-33.2

---

### TASK-33.11: Input area enhancements — character counter, visual focus

- **Labels:** `layer/tui`, `priority/medium`
- **Description:** Enhance the input area with: (1) character counter showing `[N/M]` where M is configurable max (500 default), (2) stronger visual focus indicator (brighter border or accent color when Input is focused), (3) placeholder text when empty, (4) streaming indicator when running (input is for steering).
- **Files affected:**
  - `crates/duga-tui/src/app.rs` (render_editor method)
  - `crates/duga-config/src/config.rs`
- **Types involved:** `Editor`, `Focus`, `AppState`
- **Functions to modify:**
  - `App::render_editor()` or equivalent new render path
- **Dependencies:** TASK-33.2 (layout), TASK-33.9 (focus)
- **Implementation steps:**
  1. Add `input_max_chars: Option<usize>` to `TuiConfig` (default 500)
  2. Add `input_placeholder: Option<String>` to `TuiConfig` (default "Type a task or question…")
  3. Render input area with `Block` border:
     - Title: "Input" (left) + "[42/500]" (right)
     - Border style: bright cyan when `Focus::Input`, dimmed gray otherwise
  4. Character counter at right side of input block title:
     - Normal (< 80%): dimmed gray
     - Warning (80-95%): yellow
     - Danger (> 95%): red
  5. Inner area: editor text with cursor, word-wrap, scroll
  6. Placeholder when editor buffer is empty
  7. When Running state: change placeholder to "Steering… (Enter to send guidance)"
  8. Cursor always visible when `Focus::Input`
- **Edge cases:**
  - No max chars configured → no counter shown
  - Counter at 0/500 → show normally
  - Window too short for 4-row input → ratatui handles collapse
- **Definition of Done:** Input area shows character counter, focus indicator, and context-aware placeholder.
- **Acceptance criteria:**
  - Input focused → cyan border, cursor visible
  - Input unfocused → dimmed border, no cursor
  - Text entered → counter shows "[42/500]"
  - Counter at 450/500 → yellow color
  - Counter at 490/500 → red color
  - Empty buffer → placeholder shown
  - Running state → steering placeholder
- **Test plan:**
  - unit: Counter renders correctly at different fill levels
  - unit: Focus changes border style
  - unit: Placeholder shows when buffer empty
  - unit: Placeholder changes during Running state
- **Estimated effort:** 3 hours
- **Depends on:** TASK-33.2, TASK-33.9

---

### TASK-33.12: Responsive mode — sidebar collapse + overlay fallback

- **Labels:** `layer/tui`, `priority/medium`
- **Description:** Implement responsive layout behavior. When terminal width < 120 cols, sidebar collapses to 0 width and its content becomes accessible via overlay toggles (r/e keys open a floating sidebar overlay). When width >= 120, sidebar is persistent. The threshold is configurable via `tui.responsive_breakpoint`.
- **Files affected:**
  - `crates/duga-tui/src/layout.rs`
  - `crates/duga-tui/src/app.rs`
  - `crates/duga-config/src/config.rs`
- **Types involved:** `LayoutManager`
- **Functions to modify:**
  - `LayoutManager::compute()` — already handles threshold from TASK-33.2; this task adds overlay mode
  - `Focus::next()` — skip sidebar when hidden
- **Dependencies:** TASK-33.2 (layout), TASK-33.9 (focus), TASK-33.6 (sidebar)
- **Implementation steps:**
  1. Add `responsive_breakpoint: Option<u16>` to `TuiConfig` (default 120)
  2. `LayoutManager::is_sidebar_visible()` method
  3. When sidebar hidden (width=0):
     - Focus skips Sidebar in Tab cycle
     - `r` key opens reasoning as a floating overlay (reuse overlay system)
     - `e` key opens event log as a floating overlay
     - Overlay positioned at right 40% of screen, full height
  4. Create `SidebarOverlay` that renders reasoning panel or event log in an overlay-sized rect
  5. When terminal resizes to >= breakpoint: close sidebar overlay, restore persistent sidebar
  6. When terminal resizes to < breakpoint: collapse sidebar (content state preserved), any open sidebar overlay stays
- **Edge cases:**
  - Resize rapidly between modes → don't toggle overlay state, only layout
  - Sidebar overlay open + user hits `Esc` → close overlay, return to previous focus
- **Definition of Done:** Responsive layout works. Sidebar content accessible via overlays on narrow terminals. No data loss on resize.
- **Acceptance criteria:**
  - Width >= 120 → sidebar persistent
  - Width < 120 → sidebar hidden, chat takes full width
  - `r` on narrow terminal → reasoning overlay appears
  - `e` on narrow terminal → event log overlay appears
  - Esc closes sidebar overlay
  - Resize from narrow to wide → overlay closes, sidebar appears
  - Resize from wide to narrow → sidebar collapses, overlay not auto-opened
- **Test plan:**
  - unit: `compute(119, 40, _)` → sidebar width = 0
  - unit: `compute(120, 40, _)` → sidebar width = 30
  - unit: Focus skips Sidebar when hidden
  - unit: `r` key opens overlay when sidebar hidden
  - unit: Resize event transitions correctly
- **Estimated effort:** 4 hours
- **Depends on:** TASK-33.2, TASK-33.6, TASK-33.9

---

### TASK-33.13: Performance — virtualized message rendering

- **Labels:** `layer/tui`, `priority/medium`
- **Description:** Optimize chat rendering to only process and render visible messages. Compute the visible range from scroll position and viewport height. Skip off-screen items entirely. Pre-allocate line buffers to avoid allocations during draw. Target: 60 FPS stable with 500+ messages.
- **Files affected:**
  - `crates/duga-tui/src/chat.rs`
  - `crates/duga-tui/src/transcript.rs`
- **Types involved:** `ChatView`, `Transcript`, `ScrollState`
- **Functions to modify:**
  - `ChatView::render()` — compute visible range, only iterate visible items
  - `Transcript::visible_range(scroll, viewport_height) -> Range<usize>` — compute which items could be visible
- **Dependencies:** TASK-33.4 (chat panel)
- **Implementation steps:**
  1. Measure average rendered height per transcript item type:
     - UserMessage: ~3 lines (border + title + 1 line text average)
     - AssistantMessage: ~5 lines (border + title + 3 lines text average)
     - ToolCallBlock (collapsed): ~2 lines
     - SystemMessage: ~1 line
  2. Implement `Transcript::estimate_visible_range(scroll: &ScrollState, viewport_height: u16) -> Range<usize>`:
     - Walk items from scroll position, accumulating estimated heights
     - Return range of item indices that could intersect the viewport
     - Add 2 item padding above and below for safety
  3. In `ChatView::render()`, iterate only over visible range
  4. Pre-allocate `Vec<Line<'static>>` with capacity based on viewport height
  5. Use `Line::from(vec![Span::styled(...)])` — no `format!()` calls in render loop
  6. Cache computed line counts per item until transcript changes
  7. Add `#[cfg(debug_assertions)]` frame timing log
- **Edge cases:**
  - Very long messages (hundreds of lines) → single item could fill viewport
  - Scroll position near end → range clamped to items.len()
  - Empty transcript → skip rendering, show welcome
  - Streaming updates → invalidate cache for the streaming item only
- **Definition of Done:** Rendering is O(visible items), not O(total items). Frame time stable regardless of transcript length.
- **Acceptance criteria:**
  - 5-item transcript: all items rendered (no clipping)
  - 500-item transcript scrolled to top: only first ~10 items rendered
  - 500-item transcript scrolled to bottom: only last ~10 items rendered
  - Frame time < 16ms (60 FPS) for 500 messages
  - No `format!()` calls in `render()` hot path
- **Test plan:**
  - unit: `estimate_visible_range()` for various scroll positions and viewport heights
  - unit: Visible range is a subset of total items
  - unit: Scrolled to bottom → last item is visible
  - unit: Empty transcript → no items iterated
  - integration: 500 messages in transcript, render, measure frame count (no FPS drop assertion)
- **Estimated effort:** 5 hours
- **Depends on:** TASK-33.4

---

### TASK-33.14: Integration tests and config migration

- **Labels:** `layer/tui`, `layer/testing`, `priority/high`
- **Description:** Write comprehensive integration tests for the redesigned TUI. Verify full end-to-end flows: prompt submission, agent run with streaming, event log entries, sidebar reasoning panel, error banners, focus navigation, session save/load with new layout. Ensure backward compatibility with existing config files.
- **Files affected:**
  - `crates/duga-tui/tests/e2e_tests.rs`
  - `crates/duga-config/src/config.rs`
- **Types involved:** `App`, `AppEvent`, all new widgets
- **Dependencies:** TASK-33.1 through TASK-33.13
- **Implementation steps:**
  1. **Config backward compat**: Test that config without `theme`, `show_sidebar`, etc. loads correctly with defaults
  2. **Layout integration**: Create app, render at different terminal sizes, verify pane rects
  3. **Message card flow**: Submit prompt → receive assistant response → verify card borders and colors
  4. **Event log flow**: Submit prompt → run agent with tool calls → verify log entries appear
  5. **Reasoning panel flow**: Send thinking deltas → verify panel shows content
  6. **Error banner flow**: Trigger error → verify banner appears → auto-dismisses
  7. **Focus navigation**: Verify Tab/Shift+Tab cycle, dedicated shortcuts, key routing
  8. **Session roundtrip**: Save session with new layout → load session → verify rendering
  9. **Responsive mode**: Change terminal size → verify sidebar visibility
  10. **Full agent run**: `test_config()` → `make_test_app()` → submit → streaming → tool calls → finish → verify transcript + log + banner
  11. `cargo test -p duga-tui` passes (all existing + new)
  12. `cargo test --workspace` passes
- **Edge cases:**
  - Existing e2e tests may need minor assertion updates for card borders
  - Mock events must produce correct LogEntry types
- **Definition of Done:** All integration tests pass. Full workspace test suite passes. Config backward compatibility verified.
- **Acceptance criteria:**
  - `test_full_agent_run_with_new_layout()` passes
  - `test_event_log_entries_on_tool_calls()` passes
  - `test_reasoning_panel_on_thinking_deltas()` passes
  - `test_error_banner_on_cancellation()` passes
  - `test_focus_navigation_tab_cycle()` passes
  - `test_responsive_sidebar_collapse()` passes
  - `test_config_backward_compat()` passes
  - All existing e2e tests pass (updated for card borders where needed)
  - `cargo test --workspace` pass count >= baseline
- **Test plan:** (tests listed above in implementation steps)
- **Estimated effort:** 6 hours
- **Depends on:** TASK-33.1 through TASK-33.13

---

## Implementation Order Summary

```
Phase 1: Foundation (Week 1)
  TASK-33.1 → TASK-33.2
  └── Theme + Layout (everything depends on these)

Phase 2: New Widgets (Week 2)
  TASK-33.3 (Header) ─┐
  TASK-33.4 (Chat)   ─┤ All can be done in parallel
  TASK-33.5 (Footer) ─┤ once layout is done
  TASK-33.6 (Sidebar)─┘

Phase 3: Sidebar Content (Week 2–3)
  TASK-33.7 (Reasoning) ─┐ Both depend on sidebar,
  TASK-33.8 (Event Log) ─┘ can be done in parallel

Phase 4: Interaction (Week 3)
  TASK-33.9 (Focus)    ─┐
  TASK-33.10 (Banners) ─┘ Both can be parallel

Phase 5: Polish (Week 3–4)
  TASK-33.11 (Input enhance) ─┐
  TASK-33.12 (Responsive)   ─┤ All can be parallel
  TASK-33.13 (Performance)   ─┘
  TASK-33.14 (Integration tests) ← last
```

**Total estimated effort:** 53 hours (~7 days for one developer)

---

## Risk Assessment

| Risk | Impact | Likelihood | Mitigation |
|------|--------|------------|------------|
| Card rendering breaks markdown layout | Card borders consume horizontal space — word-wrap calculations shift | Medium | Use inner `Rect` for content, subtract border width (2) from available width. Test with long lines. |
| Performance regression from border rendering | ratatui `Block` rendering is more expensive than plain `Paragraph` | Medium | Virtualization (TASK-33.13) mitigates. Measure FPS before/after. Cache card styles. |
| Focus model conflicts with existing keybindings | Users have custom keybindings; adding Tab/r/e/i as dedicated shortcuts may conflict | Medium | Dedicated shortcuts only work when no overlay. User keybindings take priority over focus shortcuts. Document override mechanism. |
| Sidebar overlay on narrow terminals feels clunky | Overlay covers chat → users lose context when checking reasoning | Low | Overlay uses 40% width, positioned right. Chat remains partially visible. Acceptable tradeoff per redesign spec. |
| Theme migration breaks user configs | Adding `theme` field is additive; defaults preserve existing look | Low | `#[serde(default)]` on all new fields. Test with existing config files. |

---

## Config Changes Summary

Added to `TuiConfig`:

```yaml
tui:
  # Existing fields preserved
  theme: "dark"                  # NEW: "dark" (default)
  show_sidebar: true             # NEW: default true
  sidebar_width_pct: 25          # NEW: 15-40, default 25
  show_footer: true              # NEW: default true
  show_header: true              # NEW: default true
  responsive_breakpoint: 120     # NEW: default 120
  input_max_chars: 500           # NEW: default 500
  banner_auto_dismiss_secs: 5    # NEW: default 5
  event_log_max_entries: 200     # NEW: default 200
```

All fields use `#[serde(default)]` — existing configs work without changes.

---

## Rollback Plan

If the redesign causes critical regressions:

1. The new widgets are in separate modules (`theme.rs`, `layout.rs`, `chat.rs`, `header.rs`, `sidebar.rs`, `footer.rs`, `focus.rs`, `event_log.rs`, `banner.rs`, `reasoning_panel.rs`)
2. `App::render()` can switch between old and new render paths via a feature flag or config key `tui.use_new_layout: bool`
3. Implement `tui.use_new_layout: false` (default `true`) that calls the original `render()` method
4. This allows shipping the redesign with an immediate kill switch
