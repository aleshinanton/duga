# Changelog

All notable changes to `duga` are documented here.

This project has not published versioned releases yet. Entries below summarize the current development line and major implementation milestones.

## [Unreleased]

### Added

- **EPIC-31: Steering — Dynamic mid-loop guidance injection.**  Allows human users, tools, config rules, and the loop itself to inject guidance, observations, or constraints into the LLM context *during* a loop run (not just at startup). Key additions:
  - **Steering types** (`duga-core::steering`) — `SteeringContextEvent` (InjectGuidance, ResetTask, AdjustLimits, InjectToolResult), `SteeringControlEvent` (Cancel, ForceComplete, Reprompt), `SteeringSender`/`SteeringReceiver` async channel pair with two-pass priority drain (Cancel > ForceComplete > Reprompt).
  - **Three injection points** in `SimpleReActLoop` — POINT 0 (before LLM call), POINT 2 (after tool result, Reprompt buffered), POINT 3 (end of step, flushes buffered).
  - **`Event::SteeringApplied`** — emitted on every steering action with `source` (human, self-diagnosis, tool, policy) and `kind` (guidance, cancel, reprompt, reset, limit) for observability.
  - **`LoopContext.steer`/`steer_limits`** — the receiving end of the steering channel (owned by the loop run), plus limit overrides from `AdjustLimits`. Child loops get `steer: None` but inherit `steer_limits`.
  - **Self-steering** — after 3 consecutive tool failures, the loop injects system-level guidance telling the LLM to pivot approach (source: `"self-diagnosis"`).
  - **Policy steer** — `agent.steering.rules` YAML config: rules with `guidance`, `as_system`, and `repeat` fields inject context events at POINT 0 (once or every iteration).
  - **Tool steer** — `ToolResult.steering_hint` optional field; tools (e.g., `search` when results > 50) return hints the loop injects as `InjectGuidance` (source: `"tool"`).
  - **Telegram integration** — `ChatSessionState.steer_tx` stores the sender; mid-run messages automatically inject `Reprompt` steering; `/steer <text>` command for explicit guidance; `/stop` still cancels via `CancellationToken` (priority over steer). Race condition (channel closed between `is_active()` and `send()`) handled gracefully by falling back to a new task.
  - **Specialized loop checkpoints** — `check_steer()` calls added to ProblemSolving, Verification, Decomposition, and Search loops at natural cycle/phase boundaries (no-op when `steer: None`, but ready for future child-loop steering).

- **EPIC-26: Loop-Agnostic Core — Agent Loop System Redesign.**  Complete architectural change: the core agent loop is now fully loop-agnostic.  Any loop type can be added without touching the dispatcher, bot, classifier prompt, or output validation.  Key changes:
  - **`Loop` trait** (`duga-core::Loop`) — defines the contract every loop implements: `id()`, `name()`, `description()`, and `run()`.  Object-safe via manual `LoopRunFuture` type alias (same pattern as `Summarizer`/`SummaryFuture`).
  - **`LoopContext`** — borrowed runtime context passed to every `Loop::run()` invocation.  Holds all dependencies (config, memory, LLM, tools, workspace, event sink, summarizer, cancellation, registry, delegation depth).
  - **`LoopResult`** — unified return type (supersedes `AgentRunResult`) with `loop_id` field recording which loop produced the result.
  - **`LoopRegistry`** — maps `loop_id → Box<dyn Loop>`, with `build_strategies_prompt()` auto-generating the "Available Strategies" prompt section from registered + enabled loops.
  - **`SimpleReActLoop`** — the existing ReAct logic extracted into a loop implementation.  Zero behavior change.  Always the entry point for bots.
  - **`DelegateTool`** — built-in tool visible to the LLM, with dynamic description listing enabled loops.  The LLM can emit `{"tool": "delegate", "args": {"loop": "problem_solving", "reason": "..."}}` to hand control to a specialized loop.
  - **Delegation intercept** — `SimpleReActLoop` intercepts `delegate` calls before tool dispatch.  Validates depth limit, looks up the target loop in the registry, runs it with an incremented depth context, and returns the delegated result as the final answer.
  - **`LoopConfig`** — new `agent.loop` config block with `enabled_loops`, `max_refinement_iterations` (default 3), and `max_delegation_depth` (default 2).  Backward-compatible: existing configs without `loop:` block get sensible defaults.
  - **`Event::LoopDelegated`** — new event emitted on every successful delegation, carrying `from`, `to`, `reason`, and `depth` fields.
  - **`BuiltRuntime`** restructured** — now holds all infrastructure as public fields (memory, registry, llm, dispatcher, etc.) instead of a monolithic `AgentLoop`.  Frontends build a `LoopContext` and run via `SimpleReActLoop`.
  - **System prompt integration** — the "Available Strategies" section is injected into the system prompt at agent construction time, derived from config's `enabled_loops`.

### Changed

- **`AgentLoop` and `AgentRunResult` are deprecated** — consumers should use `SimpleReActLoop` + `LoopContext` + `LoopResult` instead.  Backward-compatible shim remains.
- **System prompt now includes a "Available Strategies" section** that tells the LLM about the `delegate` tool and lists enabled loop types with descriptions.
- **All frontends (CLI harness, Telegram bot) now use the loop system** — they build a `LoopContext` and run via `SimpleReActLoop`.

### Added (continued)

- **EPIC-23: Task anchoring prefix.** A persistent `CURRENT TASK: ...` system message is pinned at position 0 of every LLM request so the model cannot drift to older topics in a saturated context window. The anchor is never compressed, never evicted by the token budget, and excluded from summarization input. A reminder suffix (`"Reminder: Focus exclusively on the current task: {task}"`) is appended to the user message as belt-and-suspenders.
- **EPIC-24: Sliding window and token budget enforcement.** Replaces the previous "load ALL messages" approach with configurable limits. New `memory.context_window_size` (default 50) caps the number of recent messages loaded from session history. `memory.max_context_tokens` (default 12000) provides a hard token budget — oldest messages are dropped until the estimate fits. Uses character-based token estimation (`estimate_tokens()` in `duga-core`) with no LLM round-trip. Applied in both `load_conversation_history()` (Telegram) and `Memory::enforce_window()` (CLI/TUI). Pinned messages are never removed. Set either to 0 to disable (backward-compatible).
- **EPIC-25: Semantic summarization.** Replaces the trivial role-label summarizer (`"Compressed context:\n- user\n- assistant..."`) with an LLM-driven `SemanticSummarizer` that produces 3-5 bullet points preserving topic identity, key findings, decisions, and unresolved questions. Supports incremental updates — subsequent compressions include the previous summary as context so the LLM extends rather than rewrites. Falls back to role-label summary on LLM error or timeout (15s). Configurable via `memory.summarizer: "semantic"` (default) or `"simple"`.
- Added an `edit` built-in tool for targeted text replacement in existing files.
- **EPIC-22: Step descriptions in frontend events.** Every built-in tool now has a `label` arg that the LLM fills with a human-readable description (e.g. `"ls -la"`, `"Reading config"`). This is threaded through `FrontendEvent::ToolCallStarted.description` to frontends. The Telegram renderer shows `🔧 shell: ls -la` instead of bare `🔧 shell`, collapsing duplicate `tool_name: tool_name` to just the tool name. Long step histories (>5 labels) and long final answers (>300 chars) are wrapped in Telegram `<blockquote expandable>` for a clean summary with a "Show more" toggle. The `tool_name` field on `Event::ToolCallFinished` is now populated (was always empty before), making `FrontendEvent::ToolCallFinished` self-contained.

### Changed

- Renamed the built-in command execution tool from `bash` to platform-neutral `shell`; legacy confirmation config entries named `bash` are normalized to `shell`.
- **System prompt now guides LLM to use `think`.** Added explicit instructions to use the `think` tool for complex multi-step problems and to prefer it over exploratory `shell` commands. Applied to default prompt (`agent.rs`), Telegram bot runtime prompt, and CLI harness prompt.
- **Added environment context to system prompt.** The LLM is now told what execution environment it's in (Docker container vs direct host access) and what package managers to try. Added `sandbox_environment_context()` and `tool_guidance()` helpers in `duga-runtime`, used by all three frontends.
- **Added explicit tool guidance to system prompt.** Lists available tools with when-to-use hints (e.g., "use `think` FIRST for multi-step tasks", "use `shell` for package installation"), reducing reliance on JSON Schema alone.

### Fixed

- **Specialized loops now isolate context between independent iterations.**  Verification answer attempts, Decomposition subtasks, and Search cycles were all contaminating each other through shared `Memory` — attempt N saw all tool calls and LLM responses from attempts 0..N-1, breaking the independence contract.  Added `Memory::checkpoint()`/`Memory::restore()` and wrapped each independent iteration with save/restore.  Also fixed ProblemSolving to prefer the audit's clean `final_answer` over the noisy `accumulated_output` (which contained LLM meta-commentary and tool logs), and to use the audit answer (not raw execute_plan output) when refining tasks across iterations.
- **Telegram bot no longer leaks raw tool-call XML to users.** Added `sanitize_tool_call_syntax()` in the Telegram formatter that strips `</tool_calls>`, `<invoke>`, and `<parameter>` XML fragments from final answers. Some LLMs (especially DeepSeek when primed with tool-call examples in context) generate text containing literal tool-call syntax that Telegram HTML parse mode would interpret as tags, leaking partial artifacts.
- **Malformed LLM JSON no longer kills the agent run.** Added `repair_json()` and `balance_json()` to fix common LLM-generated JSON errors (unescaped newlines, invalid escape sequences, truncated braces/brackets). As a last resort, falls back to an empty object `{}` so the tool's schema validation can catch missing fields and the LLM can retry — exactly like pi-mom's `parseStreamingJson()`.
- **Think limits reset per agent run.** Added `reset_limits()` to the `Tool` trait (default no-op), propagated through `ErasedExecute` → `ErasedTool` → `ToolDispatcher`. The agent loop calls it at the start of every run. Fixes the bug where think call/token counters accumulated across all chats in a long-running bot process, eventually denying think to all users.
- Added a project-wide changelog.
- Added shared tool confirmation middleware in `duga-tools` and re-exported it through `duga-runtime`.
- Added Docker sandbox executor with `CommandExecutor` trait, `CapabilityExecutor` (wraps `run_captured`), `DockerExecutor` (routes via `docker exec`), and `SandboxExecutor` enum.
- Added provider credential config fields (`provider_api_key`, `provider_api_key_env`, `provider_base_url`, `provider_base_url_env`) with cascading resolution (literal → env var name → provider default).
- Added OpenAI-compatible endpoint conformance tests in `crates/duga-llm/tests/openai_compat.rs` covering chat completions, tool calls, error responses, streaming SSE fixtures, Ollama-style edge cases, and auth behavior.
- Refactored `duga-harness` CLI to use shared `duga-runtime` provider resolution instead of duplicated `build_provider`/`resolve_provider`.
- Added `allow_all_binaries` config flag. When set, the binary registry check is skipped entirely and bare command names are passed directly to the executor (Docker's container PATH or capability mode's `/usr/bin:/bin` resolves them). A startup warning is emitted if used with `mode: capability` or `mode: host`.
- Added glob/wildcard pattern support to `allowed_binaries`. Entries containing `*`, `?`, or `[` are expanded at startup by walking matching directories and registering each discovered executable. Path traversal (`..`, `./`) is rejected. Zero-match globs emit a startup warning.
- Added `BinaryPattern` enum (`Exact` / `Glob`), `BinaryRegistry::allow_all()` sentinel, `from_patterns()` constructor, and `is_allow_all()` / `resolve_to_pathbuf()` methods.
- Added `CommandExecutor::is_container_executor()` to distinguish Docker from capability executors at runtime.
- Extended `docs/architecture.md` §16 with three binary resolution modes (Exact, Glob, Allow-All) and §32 config example. Added Docker + allow-all and glob pattern quick-start to README.

### Fixed

- Enforced Telegram tool confirmations for configured risky tools instead of only rendering approval UI.
- Wired Telegram `/stop` to the active agent cancellation token so long-running LLM/tool work is interrupted.
- Ensured failed Telegram agent runs unblock the renderer and clear active session state.
- Moved Telegram slash-command handling behind authorization checks.
- Wired Docker sandbox mode into shell execution, fixed Docker command argument handling, and drained Docker stdout/stderr concurrently.
- Redacted literal `provider_api_key` values from `Config` debug output.
- Switched Docker executor from `--workdir` to `-w` for broader Docker/Podman compatibility.
- Skipped binary validation at config load when `allow_all_binaries` is true, so stale or host-only entries in `allowed_binaries` don't block startup.
- Removed unnecessary `which::which()` pre-resolution in allow-all mode — bare names now flow straight to the executor in all sandbox modes.
- Fixed missing `exec` subcommand in Docker executor args (`docker exec ...` instead of `docker ...`).
- Skip Telegram shell confirmations when `allow_all_binaries` is enabled — other file-modifying tools (`edit`, `write`) still require approval.
- **Bot amnesia: conversation context now persists across messages.** Previously each message created a fresh agent with empty memory. Now `load_conversation_history()` reads the last `LlmRequest` from the chat's `session.jsonl` and restores all previous messages into the agent's memory via `AgentLoop::restore_history()`.

## [0.1.0] - Initial development

### Added

- Created the Rust workspace and core crate structure for a hardened agent runtime.
- Added foundational typed primitives for messages, assistant responses, tool calls, tool schemas, tool results, token usage, summaries, and runtime errors.
- Added capability-bounded workspace access, binary allowlisting, sanitized subprocess environments, shell-session state, process execution, timeout handling, cancellation, and output truncation.
- Added the tool trait system with `ToolContext`, type-erased tools, dispatcher lookup, schema generation, JSON validation, and typed argument deserialization.
- Added built-in `read`, `write`, `edit`, `shell`, `search`, and `think` tools with sandbox-aware execution and integration coverage.
- Added memory management with ordered message history, token-budget checks, summarization, compression, and overflow handling.
- Added the event system with event sinks, JSONL replay logging, fan-out, sequence allocation, redaction, and replay validation.
- Added the async LLM layer with provider registry support and real OpenAI and Anthropic clients.
- Added OpenAI-compatible `BASE_URL` configuration for local or proxy APIs, including Ollama-compatible `/v1` endpoints via the OpenAI provider.
- Added the core ReAct-style agent loop with limits, event emission, LLM calls, tool execution, retries, cancellation, and memory compression.
- Added WASM plugin ABI and host scaffolding, plugin loading/validation, and example plugin structure.
- Added YAML configuration loading/validation and the `duga-harness` CLI for wiring providers, workspace, tools, memory, events, plugins, and cancellation.
- Added tracing fields, replay observability, deterministic mocks, E2E smoke coverage, and replay roundtrip tests.
- Added shared frontend runtime primitives for provider resolution, dispatcher construction, skill loading, memory context, frontend event bridges, and confirmation policy types.
- Added the Telegram bot frontend with auth, per-chat sessions, progress rendering, final-answer rendering, attachments, scheduled events, logs, skill/memory context, and configuration docs.
- Added backlog epics for Telegram, terminal UI, shared frontend runtime, and provider compatibility follow-up.

### Changed

- Refined provider configuration to use a single OpenAI-compatible `BASE_URL` override instead of a separate Ollama runtime provider path.
- Split frontend architecture so shared runtime concerns live outside Telegram-specific and TUI-specific backlog epics.
- Updated Telegram auth to support allowed `@username` entries as well as numeric chat IDs.
- Adjusted Telegram final-answer rendering to avoid duplicate final messages while preserving step history.
- Disabled thinking mode in the DeepSeek-oriented config path and switched to `deepseek-chat`.

### Fixed

- Hardened sandbox tool execution and command classification.
- Fixed duration deserialization for Telegram confirmation timeout configuration.
- Fixed Telegram username-only auth validation.
- Fixed Telegram auth-time username checking.
- Fixed test-only Telegram `UserId` imports.
- Ignored generated bot `data/` output.

### Documentation

- Added and reorganized architecture, implementation-plan, backlog, provider configuration, Telegram bot, and frontend planning documentation.
- Added README setup notes for OpenAI, Anthropic, OpenAI-compatible `BASE_URL`, local Ollama-compatible usage, and Telegram bot startup.
