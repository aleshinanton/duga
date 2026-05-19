# EPIC-22: Step Descriptions in Frontend Events

**SPEC:** §5 (agent loop), §24 (events), §32 (composition root)
**Labels:** `epic/frontend`, `epic/telegram`, `epic/tools`
**Crates:** `duga-tools-builtin`, `duga-runtime`, `duga-telegram-bot`, `duga-types`

## Goal

Add a `label` parameter to every built-in tool's args, filled in by the LLM. Thread that label through the event system so frontends (Telegram, TUI, CLI) can display a short human-readable description of each step — e.g. `→ ls -la` instead of just `🔧 shell`.

Currently the Telegram renderer only shows bare tool names (`🔧 shell`, `✅ shell`). The pi agent (pi-mom-telegram) shows descriptions because every tool schema has a `label` field that the LLM populates. This epic adds the same mechanism to duga.

---

### TASK-22.1: Add `label` field to built-in tool args

- **SPEC:** §11 (Built-in Tools)
- **Labels:** `layer/tools`, `priority/critical`
- **Description:** Add a `label: String` field to each built-in tool's args struct (`ShellArgs`, `ReadArgs`, `WriteArgs`, `SearchArgs`, `ThinkArgs`). The field's JSON Schema description tells the LLM to write a brief human-readable label (e.g. "ls -la", "Reading config file", "Searching for handle_error"). The label is shown to the user as the step description but is not used inside the tool's execute logic.
- **Files affected:**
  - `crates/duga-tools-builtin/src/shell.rs`
  - `crates/duga-tools-builtin/src/read.rs`
  - `crates/duga-tools-builtin/src/write.rs`
  - `crates/duga-tools-builtin/src/search.rs`
  - `crates/duga-tools-builtin/src/think.rs`
- **Types involved:** `ShellArgs`, `ReadArgs`, `WriteArgs`, `SearchArgs`, `ThinkArgs`
- **Dependencies:** none
- **Implementation steps:**
  1. Add `pub label: String` as the first field of each args struct with a `#[serde(default)]` or no default (required).
  2. Add `schemars` attribute: `#[schemars(description = "Brief description of what this step does (shown to user)")]`.
  3. Keep `label` as the first field so it appears prominently in the LLM's view of the schema.
  4. Do NOT consume `label` inside `execute()` — it is only read by the event layer.
  5. Update all existing unit tests that construct tool args to include `label`.
- **Definition of Done:** Every built-in tool's JSON schema includes a `label` field that the LLM will populate.
- **Acceptance criteria:**
  - `serde_json::to_value(ShellArgs { label: "ls".into(), command: vec!["ls".into()], session: None })` serializes with a `label` key.
  - `schemars::schema_for!(ShellArgs)` includes `label` as a required string property.
  - Existing tool unit tests still pass after adding `label` to arg constructors.
- **Test plan:** verify JSON schema output for each tool; update existing tool tests.
- **Estimated effort:** 2 hours

---

### TASK-22.2: Thread `description` through `FrontendEvent`

- **SPEC:** §24 (Event System)
- **Labels:** `layer/events`, `layer/runtime`, `priority/critical`
- **Description:** Add a `description: String` field to `FrontendEvent::ToolCallStarted`. In `map_event()`, extract the `label` from `ToolCall.raw_args` and use it as the description. If no label is present, fall back to the tool name. Also fix `FrontendEvent::ToolCallFinished.tool_name` which is currently always `String::new()` — the renderer works around this with a HashMap lookup, but the populated field makes the event self-contained.
- **Files affected:**
  - `crates/duga-runtime/src/events.rs`
- **Types involved:** `FrontendEvent`, `map_event()`
- **Dependencies:** TASK-22.1
- **Implementation steps:**
  1. Add `description: String` to `FrontendEvent::ToolCallStarted`.
  2. In `map_event` for `Event::ToolCallStarted`, extract `raw_args.get("label")`:
     ```rust
     let description = tool_call.raw_args
         .get("label")
         .and_then(|v| v.as_str())
         .map(|s| s.to_string())
         .unwrap_or_else(|| tool_call.tool.clone());
     ```
  3. Update the `map_event_tool_call` test to assert the description field.
  4. Add a test for the fallback path (tool without label → description equals tool_name).
- **Definition of Done:** `FrontendEvent::ToolCallStarted` carries a human-readable description populated from the LLM's `label` arg.
- **Acceptance criteria:**
  - `ToolCallStarted` with `raw_args.label = "ls -la"` produces `description = "ls -la"`.
  - `ToolCallStarted` with no `label` in `raw_args` produces `description = tool_name`.
  - Existing `FrontendEventBridge` tests still pass.
- **Test plan:** unit tests in `crates/duga-runtime/src/events.rs`.
- **Estimated effort:** 2 hours

---

### TASK-22.3: Update TelegramEventRenderer to display descriptions

- **SPEC:** §24 (events), §32 (Telegram frontend)
- **Labels:** `layer/telegram`, `layer/events`, `priority/critical`
- **Description:** Update `TelegramEventRenderer` to show the `description` field in action labels instead of just the raw tool name. The renderer already maintains a `tool_names: HashMap<String, String>` mapping `tool_call_id → tool_name` to compensate for `ToolCallFinished` lacking a name. Extend this to `HashMap<String, (String, String)>` mapping `tool_call_id → (tool_name, description)`. Use the stored description in both start and finish labels.
- **Files affected:**
  - `crates/duga-telegram-bot/src/render.rs`
- **Types involved:** `TelegramEventRenderer`
- **Dependencies:** TASK-22.2
- **Implementation steps:**
  1. Change `tool_names: HashMap<String, String>` to `tool_info: HashMap<String, (String, String)>`.
  2. On `ToolCallStarted`:
     - Store `(tool_name.clone(), description.clone())` keyed by `tool_call_id`.
     - Format label as `🔧 {tool_name}: {description}` (or `🔧 {tool_name}: {description} (attempt {N})`).
  3. On `ToolCallFinished`:
     - Look up `(tool_name, description)` from `tool_info`.
     - Format label as `✅ {tool_name}: {description}` or `❌ {tool_name}: {description}`.
     - Fall back to `"tool"` / empty description if not found (backward compat).
  4. Keep the label inside Telegram's character limit (already handled by truncating to last 5 labels and capping delta buffer).
- **Definition of Done:** Telegram process message shows `🔧 shell: ls -la` / `✅ shell: ls -la` instead of `🔧 shell` / `✅ shell`.
- **Acceptance criteria:**
  - Each tool call shows its LLM-provided label.
  - Tool calls without labels (older or non-builtin tools) show just the tool name as before.
  - Retry attempts still show `(attempt N)`.
  - Process message edit does not exceed Telegram length limits.
- **Test plan:** unit tests for label formatting; manual smoke test with a real bot.
- **Estimated effort:** 3 hours

---

### TASK-22.5: Collapse long process messages and final answers

- **SPEC:** §24 (events), §32 (Telegram frontend)
- **Labels:** `layer/telegram`, `priority/high`
- **Description:** When a process message accumulates many step labels (>5) or the final answer is long (>300 chars), wrap the content in Telegram's `<blockquote expandable>` element so the user sees a clean summary with a "Show more" toggle instead of a wall of text. This matches pi-mom-telegram's `formatMessage()` behavior. Requires switching `edit_message_text` and `send_message` calls from plain text to HTML parse_mode.
- **Files affected:**
  - `crates/duga-telegram-bot/src/render.rs`
  - `crates/duga-telegram-bot/src/formatting.rs`
- **Types involved:** `TelegramEventRenderer`, `format_final_message`, `chunk_message`
- **Dependencies:** TASK-22.3
- **Implementation steps:**
  1. Add `escape_html()` and `maybe_collapse(text, threshold)` helpers to `formatting.rs`.
  2. In `finalize_process_message()`:
     - If `action_labels.len() > 5`, wrap the step history in `<blockquote expandable>...</blockquote>`.
     - Otherwise show labels inline (current behavior).
     - Switch the edit call to `ParseMode::Html`.
  3. In `finalize_process_message()` final answer send:
     - If text > 300 chars, wrap in `<blockquote expandable>...</blockquote>`.
     - Switch send calls to `ParseMode::Html`.
  4. In `edit_process_message()` (live updates):
     - Already capped at 5 labels + 200-char delta — no collapse needed for live edits.
     - Keep plain text parse mode (no HTML needed for short live messages).
  5. Handle HTML-aware chunking: `chunk_message()` must not split inside HTML tags. Approach: chunk the raw text FIRST, then wrap each chunk in `<blockquote expandable>...</blockquote>` individually. This avoids splitting inside tags and ensures each chunk is independently collapsible. For the step history case, build the full label string, chunk it, then wrap each chunk. For the final answer, chunk the formatted text, then wrap each chunk.
- **Definition of Done:** Long step histories and long final answers render with a collapsible "Show more" toggle in Telegram.
- **Acceptance criteria:**
  - 6+ steps → step history is collapsed; tapping shows all labels.
  - ≤5 steps → labels shown inline (no toggle).
  - Final answer ≤300 chars → sent as plain inline text.
  - Final answer >300 chars → wrapped in expandable blockquote.
  - HTML special characters in content don't break the blockquote.
  - Messages still chunk correctly when near Telegram's 4096 limit.
- **Test plan:** unit tests for `maybe_collapse`; manual smoke with multi-step runs.
- **Estimated effort:** 2 hours

---

### TASK-22.4: TUI and CLI renderer updates (optional follow-up)

- **SPEC:** §24 (events), §31 (observability)
- **Labels:** `layer/tui`, `layer/cli`, `priority/low`
- **Description:** The `description` field is already in `FrontendEvent::ToolCallStarted` after TASK-22.2. This task updates the TUI and CLI frontends (if they exist) to use it. If only the Telegram frontend exists, this task is a no-op and can be closed without implementation.
- **Files affected:**
  - `crates/duga-tui/` (if it exists)
  - `crates/duga-harness/` (CLI output)
- **Types involved:** TUI event renderer, CLI progress printer
- **Dependencies:** TASK-22.2
- **Implementation steps:**
  1. Check if `duga-tui` crate exists and uses `FrontendEvent`.
  2. If yes, update its renderer to display `description` alongside `tool_name`.
  3. If CLI harness prints step info, update it similarly.
  4. If neither exists, mark task as `wontfix`/`not-applicable`.
- **Definition of Done:** All existing frontends that render step progress use the `description` field, or the task is explicitly skipped.
- **Acceptance criteria:**
  - No regression in existing frontend behavior.
  - New `description` field does not cause compile errors in other crates.
- **Test plan:** compile check across all crates; visual check if frontends exist.
- **Estimated effort:** 1 hour

---

## Summary Table

| Task | Name | Est. Hours |
|------|------|------------|
| TASK-22.1 | Add `label` field to built-in tool args | 2 |
| TASK-22.2 | Thread `description` through `FrontendEvent` | 2 |
| TASK-22.3 | Update TelegramEventRenderer to display descriptions | 3 |
| TASK-22.4 | TUI/CLI renderer updates (optional) | 1 |
| TASK-22.5 | Collapse long process messages and final answers | 2 |
| **Total** | | **10 hours** |
