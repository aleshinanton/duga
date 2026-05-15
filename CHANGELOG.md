# Changelog

All notable changes to `duga` are documented here.

This project has not published versioned releases yet. Entries below summarize the current development line and major implementation milestones.

## [Unreleased]

### Added

- Added a project-wide changelog.
- Added shared tool confirmation middleware in `duga-tools` and re-exported it through `duga-runtime`.
- Added Docker sandbox executor with `CommandExecutor` trait, `CapabilityExecutor` (wraps `run_captured`), `DockerExecutor` (routes via `docker exec`), and `SandboxExecutor` enum.
- Added provider credential config fields (`provider_api_key`, `provider_api_key_env`, `provider_base_url`, `provider_base_url_env`) with cascading resolution (literal → env var name → provider default).
- Added OpenAI-compatible endpoint conformance tests in `crates/duga-llm/tests/openai_compat.rs` covering chat completions, tool calls, error responses, streaming SSE fixtures, Ollama-style edge cases, and auth behavior.
- Refactored `duga-harness` CLI to use shared `duga-runtime` provider resolution instead of duplicated `build_provider`/`resolve_provider`.

### Fixed

- Enforced Telegram tool confirmations for configured risky tools instead of only rendering approval UI.
- Wired Telegram `/stop` to the active agent cancellation token so long-running LLM/tool work is interrupted.
- Ensured failed Telegram agent runs unblock the renderer and clear active session state.
- Moved Telegram slash-command handling behind authorization checks.
- Wired Docker sandbox mode into bash execution, fixed Docker command argument handling, and drained Docker stdout/stderr concurrently.
- Redacted literal `provider_api_key` values from `Config` debug output.

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
