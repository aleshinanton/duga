# Changelog

All notable changes to `duga` are documented here.

This project has not published versioned releases yet. Entries below summarize the current development line and major implementation milestones.

## [Unreleased]

### Added

- **EPIC-22: Step descriptions in frontend events.** Every built-in tool now has a `label` arg that the LLM fills with a human-readable description (e.g. `"ls -la"`, `"Reading config"`). This is threaded through `FrontendEvent::ToolCallStarted.description` to frontends. The Telegram renderer shows `🔧 bash: ls -la` instead of bare `🔧 bash`, collapsing duplicate `tool_name: tool_name` to just the tool name. Long step histories (>5 labels) and long final answers (>300 chars) are wrapped in Telegram `<blockquote expandable>` for a clean summary with a "Show more" toggle. The `tool_name` field on `Event::ToolCallFinished` is now populated (was always empty before), making `FrontendEvent::ToolCallFinished` self-contained.

### Changed

- **System prompt now guides LLM to use `think`.** Added explicit instructions to use the `think` tool for complex multi-step problems and to prefer it over exploratory `bash` commands. Applied to default prompt (`agent.rs`), Telegram bot runtime prompt, and CLI harness prompt.
- **Added environment context to system prompt.** The LLM is now told what execution environment it's in (Docker container vs direct host access) and what package managers to try. Added `sandbox_environment_context()` and `tool_guidance()` helpers in `duga-runtime`, used by all three frontends.
- **Added explicit tool guidance to system prompt.** Lists available tools with when-to-use hints (e.g., "use `think` FIRST for multi-step tasks", "use `bash` for package installation"), reducing reliance on JSON Schema alone.

### Fixed

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
- Wired Docker sandbox mode into bash execution, fixed Docker command argument handling, and drained Docker stdout/stderr concurrently.
- Redacted literal `provider_api_key` values from `Config` debug output.
- Switched Docker executor from `--workdir` to `-w` for broader Docker/Podman compatibility.
- Skipped binary validation at config load when `allow_all_binaries` is true, so stale or host-only entries in `allowed_binaries` don't block startup.
- Removed unnecessary `which::which()` pre-resolution in allow-all mode — bare names now flow straight to the executor in all sandbox modes.
- Fixed missing `exec` subcommand in Docker executor args (`docker exec ...` instead of `docker ...`).
- Skip Telegram bash confirmations when `allow_all_binaries` is enabled — other tools (write) still require approval.

## [0.1.0] - Initial development

### Added

- Created the Rust workspace and core crate structure for a hardened agent runtime.
- Added foundational typed primitives for messages, assistant responses, tool calls, tool schemas, tool results, token usage, summaries, and runtime errors.
- Added capability-bounded workspace access, binary allowlisting, sanitized subprocess environments, shell-session state, process execution, timeout handling, cancellation, and output truncation.
- Added the tool trait system with `ToolContext`, type-erased tools, dispatcher lookup, schema generation, JSON validation, and typed argument deserialization.
- Added built-in `read`, `write`, `bash`, `search`, and `think` tools with sandbox-aware execution and integration coverage.
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
