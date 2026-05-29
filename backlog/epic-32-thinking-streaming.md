# EPIC-32: Thinking Streaming — Display LLM Reasoning in TUI

**Labels:** `epic/thinking`, `epic/tui`, `epic/llm`, `epic/events`  
**Crates:** `duga-llm` (primary), `duga-events`, `duga-runtime`, `duga-tui`, `duga-core`, `duga-harness`  
**Depends on:** EPIC-26 (Loop-Agnostic Core), EPIC-17 (Terminal UI), EPIC-18 (Frontend Shared Runtime), EPIC-08 (LLM Layer)

## Goal

Stream **thinking/reasoning content** from LLM providers (Anthropic extended thinking, OpenAI o-series reasoning, DeepSeek reasoning_content) to the duga TUI frontend in real time, displayed as a distinct, collapsible transcript item separate from the final assistant response.

Currently:
- LLM clients only support non-streaming responses — the full response is returned at once
- `Event::LlmTokenDelta` exists but is only emitted by `MockLlm` (not real providers)
- `AssistantMessage.reasoning_content` exists but is never populated by providers
- The TUI has no concept of "thinking" — only plain `AssistantMessage` text
- Streaming is gated behind `features.streaming: bool` which defaults to `false`

After this epic:
- Both Anthropic and OpenAI providers stream via SSE, emitting `LlmThinkingDelta` and `LlmTokenDelta` events as they arrive
- Thinking content appears in the TUI as a dimmed, italic, collapsible block — distinct from the assistant's final answer
- Session JSONL files capture thinking deltas for replay
- All existing functionality (non-streaming, non-thinking models) works identically — zero regression

---

## What Changes

### Architecture Before
```
LLM Provider (one-shot request)
  → full LlmResponse (text + tool_calls, no streaming)
  → core loop emits Event::LlmResponse (suppressed from frontend)
  → TUI shows nothing until run finishes
```

### Architecture After
```
LLM Provider (SSE stream)
  → Event::LlmThinkingDelta { delta: "Let me think..." }
  → Event::LlmThinkingDelta { delta: " about this step" }
  → Event::LlmTokenDelta { delta: "I'll" }
  → Event::LlmTokenDelta { delta: " read the file" }
  → full LlmResponse (accumulated, reasoning_content populated)
  ↓
FrontendEventSink maps both delta types
  ↓
TUI renders:
  🧠 Thinking: Let me think about this step  [collapsible, dimmed]
  🤖 Assistant: I'll read the file           [normal rendering]
```

### Thinking Block Lifecycle in Transcript

```
1. First LlmThinkingDelta arrives → create ThinkingBlock { is_streaming: true }
2. Subsequent LlmThinkingDelta → append to existing ThinkingBlock text
3. First LlmTokenDelta arrives → finish_streaming() on ThinkingBlock
4. LlmTokenDelta processing continues as before (AssistantMessage streaming)
```

---

## Key Design Decisions

1. **SSE streaming in `LlmClient::chat()` via `event_sink`** — The `event_sink` parameter is already passed to `chat()`. Providers emit deltas during streaming rather than returning a different type. This avoids changing the `LlmClient` trait signature.

2. **Separate event type for thinking** — `Event::LlmThinkingDelta` is distinct from `Event::LlmTokenDelta`. This keeps the event model self-describing and avoids overloading a single delta event with a `kind` discriminator.

3. **Streaming opt-in per call** — `LlmCallOptions { streaming: bool }` already exists. Providers only stream when `streaming: true`. Non-streaming calls work identically to today.

4. **Final response still returned** — Even when streaming, the accumulated full `LlmResponse` is returned at the end. Memory is updated from the final message, never from deltas. This matches the architecture doc §5.2.

5. **Thinking block is a separate TranscriptItem** — Not nested inside `AssistantMessage`. This keeps the transcript model flat and composable. A `ThinkingBlock` always precedes its corresponding `AssistantMessage`.

6. **Collapsible by default after completion** — Thinking blocks auto-collapse when streaming finishes (showing only "🧠 Thinking (42 words)"). Users can expand to read the full chain of thought. This keeps the TUI clean for users who don't need to see reasoning.

7. **Zero regression for non-thinking models** — When the model doesn't produce thinking content (no `LlmThinkingDelta` events), the TUI behaves exactly as before. The `ThinkingBlock` variant exists in the enum but is never instantiated.

---

## Development Rules

### 1. Step-by-step implementation with validation at every gate

Each task must be implemented and **validated** before moving to the next. Never batch untested changes across tasks.

**Per-task gates:**
1. **Write the code** — minimal, focused diff for the task scope
2. **Compile check** — `cargo check -p <crate>` must pass with zero warnings
3. **Write tests** — see Testing Requirements below
4. **Run crate test suite** — `cargo test -p <crate>` must pass (all existing + new tests)
5. **Run workspace tests** — `cargo test --workspace` must pass before merging the task
6. **Commit** — one commit per completed task with the task ID in the message

### 2. Never break existing functionality

This epic adds thinking streaming to a **live, working system**. Every existing test, every existing code path, and every existing user interaction must continue to work identically.

**Hard rules:**
- Non-streaming `chat()` calls must work identically — same request/response, just no delta events emitted
- `streaming: false` + any model → zero behavior change from today
- `streaming: true` + non-thinking model → `LlmTokenDelta` events emitted, no `LlmThinkingDelta` events
- `streaming: true` + thinking model → both delta event types emitted in correct order
- All existing `cargo test` assertions must pass verbatim after each task
- No changes to the `LlmClient` trait signature
- No changes to `Memory` public API
- No changes to `Event` variant names (only additive variants)
- No changes to `AssistantMessage` field names or types

### 3. Regression guardrails

Before starting each task, run the full test suite to establish a baseline:
```bash
cargo test --workspace 2>&1 | tail -5  # record passing count
```

After completing the task, the passing count must be **equal or greater** than the baseline. A drop means something broke.

**Critical regression paths to re-verify after every task:**
- `cargo test -p duga-llm -- anthropic` — Anthropic provider still works (non-streaming)
- `cargo test -p duga-llm -- openai` — OpenAI provider still works (non-streaming)
- `cargo test -p duga-events` — event serialization/deserialization intact
- `cargo test -p duga-runtime -- events` — frontend event mapping intact
- `cargo test -p duga-tui` — all TUI unit tests pass (state, transcript, key routing)
- `cargo test -p duga-core` — core loop tests pass (including e2e)
- `cargo test -p duga-harness` — harness loop e2e tests pass

### 4. Task dependency order is mandatory

Follow the dependency graph strictly:
```
TASK-32.1 (Event types)
    ↓
TASK-32.2 (MockLlm streaming + thinking support)
    ↓
TASK-32.3 (Anthropic SSE streaming)
    ↓
TASK-32.4 (OpenAI SSE streaming)
    ↓
TASK-32.5 (FrontendEvent mapping)
    ↓
TASK-32.6 (Transcript ThinkingBlock)
    ↓
TASK-32.7 (TUI rendering)
    ↓
TASK-32.8 (Session replay)
    ↓
TASK-32.9 (E2E integration tests)
    ↓
TASK-32.10 (Config + feature flag)
```

---

## Testing Requirements

### Every task must ship with tests — no exceptions

| Task | Minimum Tests Required | Test File |
|------|----------------------|-----------|
| TASK-32.1 | `LlmThinkingDelta` JSON roundtrip, redactor leaves model+delta intact, `StoredEvent` roundtrip preserves thinking delta | `duga-events/src/lib.rs` tests |
| TASK-32.2 | `MockLlm` emits `LlmThinkingDelta` before `LlmTokenDelta` when `streaming=true` + reasoning populated, no thinking deltas when `reasoning_content=None`, streaming=false emits no deltas at all, token counting unchanged | `duga-core/src/testing.rs` tests |
| TASK-32.3 | SSE response parsing: thinking_delta → `LlmThinkingDelta`, text_delta → `LlmTokenDelta`, non-streaming fallback still works, error response handling, empty stream handling, malformed SSE recovery | `duga-llm/src/anthropic.rs` tests |
| TASK-32.4 | SSE response parsing: `reasoning_content` delta → `LlmThinkingDelta`, `content` delta → `LlmTokenDelta`, non-streaming fallback still works, DeepSeek reasoning_content in final response, null content deltas skipped | `duga-llm/src/openai.rs` tests |
| TASK-32.5 | `Event::LlmThinkingDelta` → `FrontendEvent::LlmThinkingDelta`, event suppression of non-frontend events still correct, bridge roundtrip for thinking event | `duga-runtime/src/events.rs` tests |
| TASK-32.6 | `start_thinking` + `append_to_thinking` + `finish_thinking` lifecycle, `finish_thinking` marks `is_streaming=false`, thinking block auto-creates on first delta, scroll state tracks thinking blocks, clear() removes thinking blocks, thinking index reset on finish | `duga-tui/src/transcript.rs` tests |
| TASK-32.7 | `LlmThinkingDelta` frontend event handled → ThinkingBlock created, `LlmTokenDelta` after thinking → thinking finished then text started, thinking block rendered with dimmed italic style, collapsible toggle works, no thinking event → no ThinkingBlock created | `duga-tui/tests/e2e_tests.rs` tests |
| TASK-32.8 | `Event::LlmThinkingDelta` in JSONL → reconstructed as `ThinkingBlock`, multiple consecutive thinking deltas → single block, thinking followed by text → correct ordering, old session files without thinking events → zero ThinkingBlocks | `duga-tui/src/session.rs` tests |
| TASK-32.9 | Full pipeline: MockLlm with reasoning → `CapturingEventSink` → `FrontendEventSink` → `FrontendEventBridge` → `App::update` → transcript inspection. Fibonacci smoke test with thinking events. Session roundtrip with thinking events. | `duga-tui/tests/e2e_tests.rs` |
| TASK-32.10 | `show_thinking: true` → thinking blocks rendered, `show_thinking: false` → thinking events suppressed in TUI, backward compat (no `show_thinking` key → defaults to true), config deserialization roundtrip | `duga-types/src/config.rs` tests |

### Coverage targets

- **New code**: 100% of new code paths must have a test that exercises them
- **Modified code**: all branches added to existing functions must be covered
- **Error paths**: every SSE parse failure mode must have a test
- **Streaming states**: thinking-only, text-only, thinking+text, empty stream, error mid-stream — all must be tested

### Test naming convention

Use descriptive test names that document the scenario:
```rust
#[tokio::test]
async fn streaming_with_thinking_emits_both_delta_types_in_order() { }

#[tokio::test]
async fn non_streaming_call_emits_zero_delta_events() { }

#[tokio::test]
async fn transcript_thinking_block_auto_finishes_when_text_arrives() { }

#[tokio::test]
async fn e2e_fibonacci_with_thinking_streaming() { }
```

### Test infrastructure reused from existing codebase

- `CapturingEventSink` from `duga-core/src/testing.rs` — capture events for assertion
- `MockLlm` from `duga-core/src/testing.rs` — will be extended to emit thinking deltas
- `MockTool` from `duga-core/src/testing.rs` — script tool responses
- `NullSink` from `duga-events` — discard events when not needed
- `test_config()` from `duga-tui/tests/e2e_tests.rs` — build test app config
- `make_test_app()` from `duga-tui/tests/e2e_tests.rs` — test app factory

### Running tests during development

```bash
# After each code change:
cargo check -p duga-events -p duga-core -p duga-llm -p duga-runtime -p duga-tui

# Run affected crate tests:
cargo test -p duga-events
cargo test -p duga-core
cargo test -p duga-llm
cargo test -p duga-runtime
cargo test -p duga-tui

# Full workspace before committing:
cargo test --workspace
```

---

## Files Summary

| File | Change |
|------|--------|
| `crates/duga-events/src/lib.rs` | Add `Event::LlmThinkingDelta { model, delta }` variant |
| `crates/duga-core/src/testing.rs` | Extend `MockLlm` to emit `LlmThinkingDelta` when `reasoning_content` is set + `streaming=true` |
| `crates/duga-llm/src/anthropic.rs` | Add SSE streaming: `content_block_start/delta/stop` parsing, emit thinking/text deltas, populate `reasoning_content` in final response |
| `crates/duga-llm/src/openai.rs` | Add SSE streaming: `delta.reasoning_content` / `delta.content` parsing, emit thinking/text deltas, populate `reasoning_content` in final response |
| `crates/duga-llm/src/lib.rs` | (No trait changes needed — event_sink already passed) |
| `crates/duga-runtime/src/events.rs` | Add `FrontendEvent::LlmThinkingDelta`, map from `Event::LlmThinkingDelta` in `FrontendEventSink::map_event()` |
| `crates/duga-tui/src/transcript.rs` | Add `TranscriptItem::ThinkingBlock` variant, add `streaming_thinking_index`, add `start_thinking`/`append_to_thinking`/`finish_thinking`/`toggle_thinking` methods |
| `crates/duga-tui/src/app.rs` | Handle `FrontendEvent::LlmThinkingDelta` in `handle_frontend_event()`, render `ThinkingBlock` in `render_transcript()` |
| `crates/duga-tui/src/session.rs` | Reconstruct `ThinkingBlock` from `Event::LlmThinkingDelta` in `load_session_transcript()` |
| `crates/duga-tui/tests/e2e_tests.rs` | Add thinking streaming e2e tests (Task 32.9) |
| `crates/duga-types/src/config.rs` | Add `show_thinking: bool` to `AgentFeatures` (or `TuiConfig`) |

---

## Task Breakdown

---

### TASK-32.1: Add LlmThinkingDelta event variant to duga-events

- **Labels:** `layer/events`, `priority/critical`
- **Description:** Add a `LlmThinkingDelta` variant to the `Event` enum in `duga-events`. This event is emitted by LLM providers during streaming when the model produces thinking/reasoning tokens (as opposed to final response text). It carries the `model` name and the `delta` text chunk.
- **Files affected:**
  - `crates/duga-events/src/lib.rs`
- **Types involved:** `Event`
- **Functions to implement:**
  - `Event::LlmThinkingDelta { model: String, delta: String }` — new variant
- **Dependencies:** None
- **Implementation steps:**
  1. Add variant to `Event` enum (after `LlmTokenDelta`):
     ```rust
     LlmThinkingDelta {
         model: String,
         delta: String,
     },
     ```
  2. Ensure variant roundtrips through JSON serialization (add test)
  3. Ensure `Redactor` does NOT redact `model` or `delta` fields (they are not sensitive — thinking content is model reasoning, not secrets)
  4. Add existing event tests must still pass (no variant removal, only addition)
  5. `cargo test -p duga-events` passes
- **Edge cases:**
  - `delta` may be empty string (whitespace-only chunks from SSE) — allow it, frontend filters if needed
  - `model` is the provider model name string (e.g. "claude-sonnet-4-5-20250929")
- **Definition of Done:** `Event::LlmThinkingDelta` variant exists, JSON roundtrips, all existing event tests pass
- **Acceptance criteria:**
  - `serde_json::to_string(&Event::LlmThinkingDelta { model: "test".into(), delta: "think".into() })` produces valid JSON with `"type": "llm_thinking_delta"`
  - Deserialized event preserves both `model` and `delta` fields exactly
  - All existing `duga-events` tests pass unchanged
- **Test plan:**
  - unit: JSON roundtrip with `model = "claude-3", delta = "Let me reason..."`
  - unit: JSON roundtrip with empty delta string
  - unit: `StoredEvent` wrapping `LlmThinkingDelta` roundtrip
  - unit: Verify variant tag is `"llm_thinking_delta"` in JSON output
- **Estimated effort:** 1 hour
- **Depends on:** None

---

### TASK-32.2: Extend MockLlm to support thinking streaming in tests

- **Labels:** `layer/testing`, `priority/critical`
- **Description:** Extend `MockLlm` in `duga-core/src/testing.rs` to emit `Event::LlmThinkingDelta` events during streaming when the `LlmResponse` contains `reasoning_content`. This enables end-to-end testing of the thinking pipeline without real LLM providers. The `MockLlm` already supports streaming token deltas by splitting text on whitespace; it should similarly split reasoning_content and emit thinking deltas.
- **Files affected:**
  - `crates/duga-core/src/testing.rs`
- **Types involved:** `MockLlm`, `AssistantMessage`
- **Functions to modify:**
  - `MockLlm::chat()` — add reasoning_content streaming before text streaming
- **Dependencies:** TASK-32.1 (Event::LlmThinkingDelta)
- **Implementation steps:**
  1. In `MockLlm::chat()`, after the `if options.streaming` block and before text delta emission, add:
     ```rust
     if let Some(reasoning) = &response.message.reasoning_content {
         for word in reasoning.split_whitespace() {
             event_sink
                 .emit(Event::LlmThinkingDelta {
                     model: self.model.clone(),
                     delta: word.into(),
                 })
                 .await
                 .map_err(|e| LlmError::Provider(e.to_string()))?;
         }
         // Emit a newline between thinking and text
         event_sink
             .emit(Event::LlmThinkingDelta {
                 model: self.model.clone(),
                 delta: "\n".into(),
             })
             .await
             .map_err(|e| LlmError::Provider(e.to_string()))?;
     }
     ```
  2. Ensure existing MockLlm tests still pass (they don't set reasoning_content, so no new events)
  3. Add new test: MockLlm with reasoning_content + streaming=true emits both delta types in order
  4. Add new test: MockLlm with reasoning_content=None + streaming=true emits only text deltas
  5. Add new test: MockLlm with streaming=false emits zero delta events of any kind
  6. `cargo test -p duga-core` passes
- **Edge cases:**
  - `reasoning_content` is `Some("")` — emit nothing (or a single empty delta — TUI should handle)
  - `reasoning_content` contains newlines — split by whitespace treats newlines as word boundaries
  - Token counting via `count_tokens()` must be unchanged (reasoning_content doesn't affect token estimates)
- **Definition of Done:** `MockLlm` emits `LlmThinkingDelta` before `LlmTokenDelta` when reasoning_content is populated and streaming is enabled. All existing tests pass.
- **Acceptance criteria:**
  - `CapturingEventSink` captures `LlmThinkingDelta` events before `LlmTokenDelta` events
  - Zero delta events when `streaming: false`
  - Zero thinking deltas when `reasoning_content: None`
  - Same total count of text deltas as before (reasoning deltas are additive)
- **Test plan:**
  - unit: `MockLlm` with `reasoning_content: Some("step by step")` + `streaming: true` → `CapturingEventSink` has `LlmThinkingDelta("step")`, `LlmThinkingDelta("by")`, `LlmThinkingDelta("step")`, then `LlmTokenDelta` events for text
  - unit: `MockLlm` with `reasoning_content: None` + `streaming: true` → only `LlmTokenDelta` events
  - unit: `MockLlm` with any reasoning + `streaming: false` → zero delta events in sink
  - unit: `count_tokens()` returns same value regardless of reasoning_content
- **Estimated effort:** 2 hours
- **Depends on:** TASK-32.1

---

### TASK-32.3: Add SSE streaming to AnthropicClient with thinking support

- **Labels:** `layer/llm`, `priority/critical`
- **Description:** Modify `AnthropicClient::chat()` to support SSE streaming when `options.streaming` is `true`. Parse the Anthropic Messages API streaming events: `content_block_start` (identifies block type as `thinking` or `text`), `content_block_delta` (emits `LlmThinkingDelta` or `LlmTokenDelta`), `content_block_stop`, and `message_delta` (stop_reason, usage). The non-streaming path must remain unchanged and tested.
- **Files affected:**
  - `crates/duga-llm/src/anthropic.rs`
- **Types involved:** `AnthropicClient`, `AnthropicRequest`, streaming SSE event structs
- **Functions to modify:**
  - `AnthropicClient::chat()` — branch on `options.streaming`, add SSE parsing loop
- **New structs to define:**
  - `AnthropicStreamEvent` — deserializes the SSE `data:` JSON
  - `AnthropicContentBlockStart` — `type: "thinking" | "text"`, block index
  - `AnthropicContentBlockDelta` — `type: "thinking_delta" | "text_delta"`, `thinking: str`, `text: str`
  - `AnthropicMessageDelta` — `stop_reason`, `usage`
- **Dependencies:** TASK-32.1 (Event::LlmThinkingDelta), TASK-32.2 (MockLlm — for testing pattern reference)
- **Implementation steps:**
  1. Add `"stream": true` to `AnthropicRequest` when `options.streaming` is true
  2. In `chat()`, when streaming: use `reqwest::Response::bytes_stream()` to read SSE chunks
  3. Parse each SSE line: skip `event:` lines, parse `data:` lines as `AnthropicStreamEvent`
  4. Track active block type per block index (from `content_block_start`)
  5. On `content_block_delta` with type `thinking_delta`: emit `Event::LlmThinkingDelta`
  6. On `content_block_delta` with type `text_delta`: emit `Event::LlmTokenDelta`
  7. Accumulate full text and thinking content for final `LlmResponse`
  8. On `message_delta`: extract `stop_reason` and `usage`
  9. Return accumulated `LlmResponse` with `reasoning_content` populated
  10. Non-streaming path: unchanged — same code as today, just gated on `!options.streaming`
  11. `cargo check -p duga-llm` passes with zero warnings
- **Edge cases:**
  - Empty SSE stream → return error `LlmError::Provider("empty stream")`
  - Server sends `error` event → parse and return `LlmError::Provider(...)`
  - thinking block with `signature` field → capture signature for API roundtrip requirement
  - Multiple thinking blocks → accumulate in order
  - thinking delta with no preceding `content_block_start` → defensive: infer block type "thinking"
  - Connection drops mid-stream → return partial accumulated content as error
  - SSE lines split across TCP chunks → buffer incomplete lines
- ### ⚠️ CRITICAL — Anthropic Thinking Signature Requirement
  Anthropic's API requires that thinking blocks with `signature` be passed back in subsequent requests. The `signature` field must be preserved in `AssistantMessage` (add a `thinking_signature: Option<String>` field if not already present). Without this, the API returns HTTP 400 on the next request. **This field already exists in `AssistantMessage` as a planned field — verify and implement if missing.**
- **Definition of Done:** AnthropicClient streams SSE responses, emits thinking and text deltas, returns accumulated response with reasoning_content. Non-streaming path unchanged. All existing tests pass.
- **Acceptance criteria:**
  - `streaming: true` + Claude model with extended thinking → `LlmThinkingDelta` events emitted during thinking, `LlmTokenDelta` events emitted during text
  - `streaming: false` → single LlmResponse, no delta events, identical behavior to before this task
  - Final `LlmResponse.message.reasoning_content` is populated from accumulated thinking blocks
  - Thinking `signature` is preserved in the response for API roundtrip
- **Test plan:**
  - unit: Parse `content_block_start` with type "thinking" → correctly identifies block
  - unit: Parse `content_block_delta` with thinking_delta → extracts thinking text
  - unit: Parse `content_block_delta` with text_delta → extracts text
  - unit: Full SSE stream (mock server) → all deltas emitted in order, final response correct
  - unit: Empty SSE stream → `LlmError::Provider`
  - unit: SSE with error event → `LlmError::Provider`
  - unit: Split SSE line across chunks → correctly reassembled
  - unit: Non-streaming request → no regressions (existing tests pass)
  - **integration:** `cargo test -p duga-llm -- anthropic` all pass
- **Estimated effort:** 6 hours
- **Depends on:** TASK-32.1

---

### TASK-32.4: Add SSE streaming to OpenAiClient with reasoning_content support

- **Labels:** `layer/llm`, `priority/critical`
- **Description:** Modify `OpenAiClient::chat()` to support SSE streaming when `options.streaming` is `true`. Parse OpenAI-compatible SSE chunks. For OpenAI o-series models, `delta.reasoning_content` maps to `LlmThinkingDelta`. For DeepSeek and compatible providers, `delta.reasoning_content` is the primary thinking field. `delta.content` maps to `LlmTokenDelta`. The non-streaming path must remain unchanged.
- **Files affected:**
  - `crates/duga-llm/src/openai.rs`
- **Types involved:** `OpenAiClient`, `OpenAiRequest`, SSE chunk structs
- **Functions to modify:**
  - `OpenAiClient::chat()` — branch on `options.streaming`, add SSE parsing loop
- **New structs to define:**
  - `OpenAiStreamChunk` — deserializes `{"choices":[{"delta":{"content":..., "reasoning_content":..., "tool_calls":...}}], "usage":...}`
  - `OpenAiStreamDelta` — `content: Option<String>`, `reasoning_content: Option<String>`, `tool_calls: Option<Vec<...>>`
- **Dependencies:** TASK-32.1 (Event::LlmThinkingDelta)
- **Implementation steps:**
  1. Add `"stream": true` and `"stream_options": {"include_usage": true}` to `OpenAiRequest` when `options.streaming` is true
  2. In `chat()`, when streaming: use `reqwest::Response::bytes_stream()` to read SSE chunks
  3. Parse each SSE line: skip `event:` lines, parse `data:` lines (skip `[DONE]`)
  4. On `data:` line: deserialize as `OpenAiStreamChunk`
  5. If `delta.reasoning_content` is `Some(text)`: emit `Event::LlmThinkingDelta`
  6. If `delta.content` is `Some(text)`: emit `Event::LlmTokenDelta`
  7. If `delta.tool_calls` is `Some(...)`: accumulate tool call fragments (OpenAI streams tool calls incrementally)
  8. Accumulate full `content`, `reasoning_content`, and tool calls from all chunks
  9. If `usage` is present (final chunk with `stream_options.include_usage`): extract token counts
  10. Return accumulated `LlmResponse` with `reasoning_content` populated
  11. Non-streaming path: unchanged — same code as today
  12. `cargo check -p duga-llm` passes with zero warnings
- **Edge cases:**
  - `[DONE]` sentinel → stop stream, return accumulated response
  - `delta.content` is `null` or missing → skip, don't emit empty delta
  - `delta.reasoning_content` is `null` or missing → skip, don't emit empty delta
  - Tool call streaming: function name arrives first, then arguments in chunks → accumulate and parse at end
  - Empty stream (immediate `[DONE]`) → return `LlmResponse` with empty message
  - Connection drops mid-stream → return partial accumulated content as error
  - DeepSeek format: `reasoning_content` may arrive in final response instead of deltas — handle both
  - `stream_options` may not be supported by all providers → make it optional, handle 400 gracefully
  - Chunk may have both `reasoning_content` and `content` in same delta → emit thinking first, then text (matching logical order)
- **Definition of Done:** OpenAiClient streams SSE responses, emits thinking and text deltas, handles tool call streaming, returns accumulated response. Non-streaming path unchanged.
- **Acceptance criteria:**
  - `streaming: true` + o-series model → `LlmThinkingDelta` for reasoning_content, `LlmTokenDelta` for content
  - `streaming: true` + DeepSeek model → `LlmThinkingDelta` for reasoning_content
  - `streaming: true` + standard GPT model → only `LlmTokenDelta` (no reasoning_content in response)
  - `streaming: false` → single LlmResponse, no delta events, identical behavior to before
  - Tool calls streamed correctly: accumulated and parsed into `ToolCall` vec
  - Final `LlmResponse.message.reasoning_content` is populated from accumulated chunks
- **Test plan:**
  - unit: Parse chunk with `delta.reasoning_content` → emits `LlmThinkingDelta`
  - unit: Parse chunk with `delta.content` → emits `LlmTokenDelta`
  - unit: Parse chunk with both reasoning_content and content → both deltas emitted in order
  - unit: Parse chunk with `delta.tool_calls` → accumulates tool call fragments
  - unit: Parse `[DONE]` → stream ends, response returned
  - unit: Parse null delta fields → no events emitted for those fields
  - unit: Full SSE stream (mock server) → all deltas emitted, final response correct
  - unit: Non-streaming request → no regressions (existing tests pass)
  - **integration:** `cargo test -p duga-llm -- openai` all pass
- **Estimated effort:** 6 hours
- **Depends on:** TASK-32.1

---

### TASK-32.5: Map LlmThinkingDelta through FrontendEventSink

- **Labels:** `layer/runtime`, `priority/high`
- **Description:** Add `FrontendEvent::LlmThinkingDelta` to the frontend event enum and map `Event::LlmThinkingDelta` to it in `FrontendEventSink::map_event()`. This bridges the gap between internal event system and the TUI's event loop. The mapping is straightforward — same pattern as `LlmTokenDelta`.
- **Files affected:**
  - `crates/duga-runtime/src/events.rs`
- **Types involved:** `FrontendEvent`, `FrontendEventSink`
- **Functions to modify:**
  - `FrontendEventSink::map_event()` — add arm for `Event::LlmThinkingDelta`
- **Dependencies:** TASK-32.1 (Event::LlmThinkingDelta)
- **Implementation steps:**
  1. Add variant to `FrontendEvent` enum:
     ```rust
     LlmThinkingDelta {
         model: String,
         delta: String,
     },
     ```
  2. In `map_event()`, add mapping arm:
     ```rust
     Event::LlmThinkingDelta { model, delta } => {
         Some(FrontendEvent::LlmThinkingDelta { model, delta })
     }
     ```
  3. Ensure `LlmThinkingDelta` is NOT in the suppressed list (unlike `LlmResponse` which is suppressed)
  4. `cargo test -p duga-runtime` passes
- **Edge cases:**
  - `LlmThinkingDelta` is a high-frequency event during streaming — channel buffer must handle it (existing 16-buffer should suffice for testing)
  - TUI channel full → event is dropped (existing behavior for all frontend events), logged at `warn`
- **Definition of Done:** `FrontendEvent::LlmThinkingDelta` exists, mapped from `Event::LlmThinkingDelta`, all existing runtime tests pass
- **Acceptance criteria:**
  - `FrontendEventSink::map_event(Event::LlmThinkingDelta { model: "x".into(), delta: "y".into() })` returns `Some(FrontendEvent::LlmThinkingDelta { ... })`
  - Event is not suppressed (not in the `StepStarted | StepFinished | LlmRequest | LlmResponse` suppression list)
  - Bridge roundtrip: send `LlmThinkingDelta` through channel, receive identical event
- **Test plan:**
  - unit: `map_event()` on `Event::LlmThinkingDelta` → `Some(FrontendEvent::LlmThinkingDelta)`
  - unit: `LlmThinkingDelta` not suppressed (unlike `Event::LlmResponse`)
  - unit: Bridge send/receive roundtrip with `FrontendEvent::LlmThinkingDelta`
  - unit: Existing `map_event` tests pass unchanged
- **Estimated effort:** 1 hour
- **Depends on:** TASK-32.1

---

### TASK-32.6: Add ThinkingBlock to Transcript with lifecycle methods

- **Labels:** `layer/tui`, `priority/high`
- **Description:** Add `TranscriptItem::ThinkingBlock` variant to the transcript enum. Add lifecycle tracking (`streaming_thinking_index`) and methods (`start_thinking`, `append_to_thinking`, `finish_thinking`, `toggle_thinking_expand`) to `Transcript`. This is a pure data model change with no rendering — rendering is in Task 32.7.
- **Files affected:**
  - `crates/duga-tui/src/transcript.rs`
- **Types involved:** `TranscriptItem`, `Transcript`
- **Functions to implement:**
  - `TranscriptItem::ThinkingBlock { text, is_streaming, is_expanded, timestamp }` — new variant
  - `Transcript::start_thinking(&mut self)` — create ThinkingBlock, track index
  - `Transcript::append_to_thinking(&mut self, delta: &str)` — append or auto-start
  - `Transcript::finish_thinking(&mut self)` — mark complete, clear streaming index
  - `Transcript::toggle_thinking_expand(&mut self, idx: usize)` — toggle collapse
  - `Transcript::thinking_is_streaming(&self) -> bool` — convenience check
- **Dependencies:** None (pure data model change)
- **Implementation steps:**
  1. Add `ThinkingBlock` variant to `TranscriptItem`:
     ```rust
     ThinkingBlock {
         text: String,
         is_streaming: bool,
         is_expanded: bool,
         timestamp: Instant,
     },
     ```
  2. Add `streaming_thinking_index: Option<usize>` field to `Transcript`
  3. Implement `start_thinking()`:
     - If already streaming thinking, do nothing (defensive)
     - Push new `ThinkingBlock { is_streaming: true, is_expanded: true }` (expanded during streaming)
     - Set `streaming_thinking_index` to the new item index
  4. Implement `append_to_thinking(delta)`:
     - If `streaming_thinking_index` is `Some`, append `delta` to that item's text
     - If `None`, call `start_thinking()` first, then append
  5. Implement `finish_thinking()`:
     - If `streaming_thinking_index` is `Some`, set `is_streaming = false`, `is_expanded = false` (auto-collapse on completion)
     - Set `streaming_thinking_index = None`
  6. Implement `toggle_thinking_expand(idx)`:
     - Toggle `is_expanded` on the ThinkingBlock at `idx`
  7. Implement `thinking_is_streaming()`:
     - Returns `streaming_thinking_index.is_some()`
  8. Update `Transcript::clear()` to reset `streaming_thinking_index`
  9. `cargo test -p duga-tui` passes (existing transcript tests + new)
- **Edge cases:**
  - `append_to_thinking` called while `AssistantMessage` is also streaming → thinking block is separate, both can stream concurrently (though in practice thinking ends before text begins)
  - `finish_thinking` called with no active thinking → no-op (don't crash)
  - `clear()` during active thinking → reset all indices
  - Multiple consecutive `finish_thinking` calls → only first one applies, subsequent are no-ops
  - `start_thinking` while already streaming → no-op (don't create orphan blocks)
- **Definition of Done:** `ThinkingBlock` variant exists, lifecycle methods work correctly, all transcript tests pass
- **Acceptance criteria:**
  - `append_to_thinking("Hello")` + `append_to_thinking(" world")` → single ThinkingBlock with text "Hello world"
  - `finish_thinking()` → `is_streaming = false`, `streaming_thinking_index = None`
  - `clear()` removes all blocks including streaming thinking, resets index
  - `thinking_is_streaming()` returns `true` while thinking is active, `false` otherwise
  - Auto-collapse: when thinking finishes, `is_expanded = false`
- **Test plan:**
  - unit: `append_to_thinking` auto-creates ThinkingBlock on first call
  - unit: Multiple `append_to_thinking` calls accumulate into same block
  - unit: `finish_thinking` marks complete and clears streaming index
  - unit: `append_to_thinking` after `finish_thinking` creates a new ThinkingBlock
  - unit: `clear()` during active thinking resets all state
  - unit: `toggle_thinking_expand` toggles is_expanded
  - unit: `Transcript::len()` counts ThinkingBlocks
  - unit: Scroll state unaffected by thinking blocks
  - unit: Existing tests (`push_and_retrieve_items`, `append_to_streaming`, etc.) pass unchanged
- **Estimated effort:** 3 hours
- **Depends on:** None (can start in parallel with TASK-32.1–32.5)

---

### TASK-32.7: Handle thinking events and render ThinkingBlock in TUI

- **Labels:** `layer/tui`, `priority/high`
- **Description:** Wire `FrontendEvent::LlmThinkingDelta` into `App::handle_frontend_event()` and render `TranscriptItem::ThinkingBlock` in `App::render_transcript()`. The rendering must distinguish thinking from assistant text: dimmed gray color, italic modifier, collapsible. When a `LlmTokenDelta` arrives while thinking is streaming, auto-finish the thinking block first.
- **Files affected:**
  - `crates/duga-tui/src/app.rs`
- **Types involved:** `App`, `TranscriptItem`
- **Functions to modify:**
  - `App::handle_frontend_event()` — add `FrontendEvent::LlmThinkingDelta` arm
  - `App::render_transcript()` — add `TranscriptItem::ThinkingBlock` rendering
- **Dependencies:** TASK-32.5 (FrontendEvent mapping), TASK-32.6 (Transcript ThinkingBlock)
- **Implementation steps:**
  1. In `handle_frontend_event()`, before the `LlmTokenDelta` arm, add:
     ```rust
     FrontendEvent::LlmThinkingDelta { delta, .. } => {
         self.transcript.append_to_thinking(&delta);
     }
     ```
  2. Modify the `LlmTokenDelta` arm to auto-finish thinking:
     ```rust
     FrontendEvent::LlmTokenDelta { delta, .. } => {
         // Auto-finish thinking if it was streaming
         if self.transcript.thinking_is_streaming() {
             self.transcript.finish_thinking();
         }
         self.transcript.append_to_streaming(&delta);
     }
     ```
  3. Modify `RunFinished` arm to also finish any lingering thinking:
     ```rust
     FrontendEvent::RunFinished { .. } => {
         if self.transcript.thinking_is_streaming() {
             self.transcript.finish_thinking();
         }
         // ... existing RunFinished handling
     }
     ```
  4. In `render_transcript()`, add rendering for `ThinkingBlock`:
     ```rust
     TranscriptItem::ThinkingBlock { text, is_streaming, is_expanded, .. } => {
         let prefix = if *is_streaming { "⟳ " } else { "🧠" };
         lines.push(Line::from(Span::styled(
             format!("{prefix} Thinking:"),
             Style::default()
                 .fg(Color::DarkGray)
                 .add_modifier(Modifier::ITALIC),
         )));
         if *is_expanded {
             for wrapped in crate::text::wrap_text(text, available_width.saturating_sub(2)) {
                 lines.push(Line::from(Span::styled(
                     format!("  {wrapped}"),
                     Style::default()
                         .fg(Color::DarkGray)
                         .add_modifier(Modifier::ITALIC),
                 )));
             }
         } else if !is_streaming {
             // Show summary when collapsed
             let word_count = text.split_whitespace().count();
             lines.push(Line::from(Span::styled(
                 format!("  ({} words — press Tab to expand)", word_count),
                 Style::default().fg(Color::DarkGray),
             )));
         }
     }
     ```
  5. Add thinking block toggle to `ToggleTool` handling or a new keybinding
  6. `cargo check -p duga-tui` passes with zero warnings
  7. `cargo test -p duga-tui` passes
- **Edge cases:**
  - Thinking finishes but no text follows (model only produces thinking, no answer) → `RunFinished` with no text should still show thinking block as completed
  - Thinking and text interleaved (shouldn't happen with current APIs, but defensive) → only auto-finish on first `LlmTokenDelta`, don't re-finish
  - `ToolCallStarted` arrives while thinking is streaming → auto-finish thinking before showing tool call (thinking happens before tool calls in the logical flow)
  - Multiple thinking blocks in one run (compaction/reprompt scenarios) → each is a separate ThinkingBlock
- **Definition of Done:** Thinking events create ThinkingBlocks in the transcript. Thinking blocks render with dimmed, italic style and are collapsible. Text deltas auto-finish thinking. All existing TUI tests pass.
- **Acceptance criteria:**
  - Sending `LlmThinkingDelta` frontend event → ThinkingBlock appears in transcript
  - Thinking block renders with dark gray + italic style
  - Collapsed thinking block shows word count summary
  - Sending `LlmTokenDelta` after thinking → thinking auto-finishes, text starts
  - Sending `ToolCallStarted` while thinking → thinking auto-finishes
  - Run finishes with only thinking → thinking block is completed, not left as streaming
  - No thinking events in a run → no ThinkingBlocks created (zero visual change)
- **Test plan:**
  - unit: `FrontendEvent::LlmThinkingDelta` → `ThinkingBlock` created with correct text
  - unit: `FrontendEvent::LlmTokenDelta` after thinking → thinking.is_streaming = false, text streaming starts
  - unit: `FrontendEvent::ToolCallStarted` during thinking → thinking auto-finishes
  - unit: `FrontendEvent::RunFinished` during thinking → thinking auto-finishes
  - unit: No thinking events → transcript has zero ThinkingBlocks
  - unit: Collapse/expand toggle works
  - unit: Existing e2e tests (all `test_frontend_event_*`) pass unchanged
- **Estimated effort:** 4 hours
- **Depends on:** TASK-32.5, TASK-32.6

---

### TASK-32.8: Reconstruct ThinkingBlocks from session JSONL

- **Labels:** `layer/tui`, `priority/medium`
- **Description:** Update `load_session_transcript()` in `duga-tui/src/session.rs` to reconstruct `TranscriptItem::ThinkingBlock` entries from stored `Event::LlmThinkingDelta` events in session JSONL files. This ensures that when a user resumes a session, previously streamed thinking content is visible in the transcript. Consecutive `LlmThinkingDelta` events for the same thinking block should be merged into a single `ThinkingBlock`.
- **Files affected:**
  - `crates/duga-tui/src/session.rs`
- **Types involved:** `Event`, `StoredEvent`, `TranscriptItem`
- **Functions to modify:**
  - `load_session_transcript()` — add `Event::LlmThinkingDelta` arm
- **Dependencies:** TASK-32.1 (Event::LlmThinkingDelta), TASK-32.6 (Transcript ThinkingBlock)
- **Implementation steps:**
  1. In `load_session_transcript()`, add a match arm for `Event::LlmThinkingDelta`:
     ```rust
     Event::LlmThinkingDelta { delta, .. } => {
         // Merge consecutive thinking deltas into the same block
         if let Some(TranscriptItem::ThinkingBlock { text, is_streaming: false, .. }) = items.last_mut() {
             text.push_str(&delta);
         } else if let Some(TranscriptItem::AssistantMessage { .. }) = items.last() {
             // Thinking arrived after text — shouldn't happen, but create a block anyway
             items.push(TranscriptItem::ThinkingBlock {
                 text: delta,
                 is_streaming: false,
                 is_expanded: false, // collapsed in replay
                 timestamp: Instant::now(),
             });
         } else if let Some(TranscriptItem::ThinkingBlock { text, .. }) = items.last_mut() {
             // Still building the same thinking block
             text.push_str(&delta);
         } else {
             // First thinking delta — create new block
             items.push(TranscriptItem::ThinkingBlock {
                 text: delta,
                 is_streaming: false,
                 is_expanded: false,
                 timestamp: Instant::now(),
             });
         }
     }
     ```
  2. Ensure that thinking deltas followed by text deltas produce the correct ordering: ThinkingBlock then AssistantMessage
  3. Ensure that old session files without `LlmThinkingDelta` events load identically to before
  4. `cargo test -p duga-tui` passes (including session tests)
- **Edge cases:**
  - Session has thinking deltas but no text deltas (model only produced thinking) → ThinkingBlock(s) exist, no AssistantMessage
  - Session has interleaved thinking and text deltas (shouldn't happen but defensive) → preserve file order
  - Session has empty thinking delta strings → append them (they don't affect display)
  - Thinking block at the very end of the file (streaming was in progress when session was saved mid-run) → `is_streaming: false` (we're replaying, not live)
- **Definition of Done:** Session replay reconstructs ThinkingBlocks from stored `LlmThinkingDelta` events. Old sessions load identically.
- **Acceptance criteria:**
  - JSONL with three consecutive `LlmThinkingDelta` events → single ThinkingBlock with concatenated text
  - JSONL with thinking → text → thinking → text → correct alternating order in transcript
  - JSONL without any `LlmThinkingDelta` events → zero ThinkingBlocks (identical to before)
  - Thinking blocks are collapsed by default in replay (`is_expanded: false`)
- **Test plan:**
  - unit: JSONL with `LlmThinkingDelta` events → transcript has ThinkingBlocks
  - unit: Consecutive thinking deltas → merged into single block
  - unit: Thinking followed by text → correct transcript order
  - unit: Old session format (no thinking events) → parses without errors, no ThinkingBlocks
  - unit: Existing `load_session_transcript` tests pass unchanged
- **Estimated effort:** 2 hours
- **Depends on:** TASK-32.1, TASK-32.6

---

### TASK-32.9: End-to-end integration tests for thinking streaming

- **Labels:** `layer/testing`, `priority/high`
- **Description:** Write comprehensive end-to-end tests that verify the full thinking streaming pipeline: `MockLlm` with reasoning → event emission → frontend mapping → TUI event handling → transcript state. Tests must cover: thinking-only run, thinking+text run, thinking+tool_calls run, non-streaming run (regression), session roundtrip with thinking events, and the Fibonacci smoke test with thinking streaming enabled.
- **Files affected:**
  - `crates/duga-tui/tests/e2e_tests.rs`
- **Types involved:** `App`, `AppEvent`, `FrontendEvent`, `TranscriptItem`, `CapturingEventSink`
- **Dependencies:** TASK-32.2 (MockLlm thinking), TASK-32.5 (FrontendEvent mapping), TASK-32.6 (Transcript), TASK-32.7 (TUI rendering)
- **Implementation steps:**
  1. Add test: `thinking_streaming_creates_thinking_block()`
     - Send `LlmThinkingDelta` frontend events via `App::update`
     - Assert transcript has a `ThinkingBlock` with correct accumulated text
     - Assert `is_streaming: true` during streaming, `false` after finish
  2. Add test: `thinking_auto_finishes_when_text_arrives()`
     - Send thinking deltas, then text deltas
     - Assert thinking block `is_streaming: false`
     - Assert assistant message `is_streaming: true`
  3. Add test: `thinking_auto_finishes_when_tool_call_arrives()`
     - Send thinking deltas, then `ToolCallStarted`
     - Assert thinking is finished
     - Assert tool call block exists
  4. Add test: `thinking_auto_finishes_on_run_finished()`
     - Send thinking deltas, then `RunFinished`
     - Assert thinking is finished (not left as streaming)
  5. Add test: `no_thinking_events_produces_no_thinking_blocks()`
     - Run a full event sequence without any `LlmThinkingDelta`
     - Assert zero `ThinkingBlock` items in transcript
  6. Add test: `non_streaming_regression_no_delta_events()`
     - Same as existing `test_frontend_event_*` tests but with `streaming: false`
     - Assert transcript items are correct, no delta events processed
  7. Add test: `thinking_session_roundtrip()`
     - Create a session JSONL with `LlmThinkingDelta` events
     - Call `load_session_transcript()`
     - Assert `ThinkingBlock` items are reconstructed correctly
  8. Add test: `thinking_collapse_expand_toggle()`
     - Create thinking block, verify collapsed after finish
     - Toggle expand, verify state
  9. `cargo test -p duga-tui` passes (all existing + new)
- **Edge cases:**
  - Empty thinking delta → ThinkingBlock created with empty text
  - Single thinking delta followed by `RunFinished` → ThinkingBlock completed, no AssistantMessage
  - Multiple runs in one session — each has its own thinking blocks, properly separated
  - Thinking block alongside tool calls → correct ordering in transcript
  - Very large thinking content (1000+ words) → collapsible prevents UI overflow
- **Definition of Done:** All e2e thinking streaming tests pass. Full workspace test suite passes. No regressions.
- **Acceptance criteria:**
  - `thinking_streaming_creates_thinking_block` passes
  - `thinking_auto_finishes_when_text_arrives` passes
  - `no_thinking_events_produces_no_thinking_blocks` passes
  - All existing e2e tests (`test_frontend_event_*`, `test_transcript_*`, etc.) pass unchanged
  - `cargo test --workspace` has same or greater passing count vs baseline
- **Test plan:** (tests listed above in implementation steps)
- **Estimated effort:** 4 hours
- **Depends on:** TASK-32.2, TASK-32.5, TASK-32.6, TASK-32.7, TASK-32.8

---

### TASK-32.10: Configuration flag for thinking display

- **Labels:** `layer/config`, `priority/low`
- **Description:** Add a `show_thinking` boolean configuration option to control whether thinking content is displayed in the TUI. When `false`, `FrontendEvent::LlmThinkingDelta` events are dropped before reaching the transcript (like how `Event::LlmResponse` is suppressed). When `true` (default), thinking blocks appear normally. This allows users who find thinking output noisy to disable it without losing the underlying event logging.
- **Files affected:**
  - `crates/duga-types/src/config.rs`
  - `crates/duga-tui/src/app.rs`
- **Types involved:** `AgentFeatures` or `TuiConfig`
- **Functions to modify:**
  - `App::handle_frontend_event()` — gate `LlmThinkingDelta` on `show_thinking`
- **Dependencies:** TASK-32.7 (TUI thinking rendering)
- **Implementation steps:**
  1. Decide where to put the flag:
     - Option A: `AgentFeatures { streaming: bool, show_thinking: bool }` — lives with other feature flags
     - Option B: `TuiConfig { show_thinking: bool, ... }` — TUI-specific config
     - **Recommendation:** `TuiConfig` since thinking display is purely a frontend concern (event logging still happens regardless)
  2. Add field with `#[serde(default = "default_show_thinking")]` where `default_show_thinking() -> bool { true }`
  3. In `App::handle_frontend_event()`, gate the `LlmThinkingDelta` arm:
     ```rust
     FrontendEvent::LlmThinkingDelta { delta, .. } => {
         if self.tui_config.show_thinking {
             self.transcript.append_to_thinking(&delta);
         }
     }
     ```
  4. Auto-finish thinking still happens even when display is off (to keep state machine correct)
  5. `cargo test -p duga-types` passes
  6. `cargo test -p duga-tui` passes
- **Edge cases:**
  - Config file missing `show_thinking` key → defaults to `true` (backward compatible)
  - `show_thinking: false` + session replay → thinking events still in JSONL, just not displayed in TUI
  - Toggle at runtime: not supported (config is static per session) — document this limitation
- **Definition of Done:** `show_thinking` config flag exists, defaults to true, gates TUI display. All tests pass.
- **Acceptance criteria:**
  - Config with `show_thinking: false` → thinking events don't create ThinkingBlocks in TUI
  - Config without `show_thinking` key → defaults to `true`, thinking displayed
  - Config roundtrip: serialize + deserialize preserves `show_thinking` value
  - State machine still correct: thinking auto-finish happens even when display is off
- **Test plan:**
  - unit: Config deserialization with `show_thinking: false`
  - unit: Config deserialization without `show_thinking` → defaults to `true`
  - unit: Config roundtrip preserves value
  - unit: TUI app with `show_thinking: false` → `LlmThinkingDelta` events don't create blocks
  - unit: TUI app with `show_thinking: false` → `LlmTokenDelta` still auto-finishes thinking state
- **Estimated effort:** 1.5 hours
- **Depends on:** TASK-32.7

---

## Implementation Order Summary

```
Phase 1: Foundation (TASK-32.1 → TASK-32.2)
  └── Event types + MockLlm support (enables testing from day 1)

Phase 2: Provider Streaming (TASK-32.3 → TASK-32.4, can be parallel)
  └── Anthropic SSE + OpenAI SSE (real provider integration)

Phase 3: Frontend Bridge (TASK-32.5)
  └── Wire events through to TUI

Phase 4: TUI Display (TASK-32.6 → TASK-32.7, sequential)
  └── Transcript model → Rendering

Phase 5: Polish (TASK-32.8 → TASK-32.9 → TASK-32.10, can be semi-parallel)
  └── Session replay → E2E tests → Config flag
```

## Rollback Plan

If thinking streaming causes issues in production (performance, API errors, visual glitches):

1. Set `features.streaming: false` in config — reverts to non-streaming mode (already tested)
2. Or set `tui.show_thinking: false` — keeps streaming but hides thinking in TUI
3. Or revert to previous commit — each task is a single commit, easy to bisect
4. The `LlmClient` trait is unchanged — non-streaming code path is the original code, untouched
