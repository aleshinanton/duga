# duga — Engineering Task Backlog

**Repository:** `aleshinanton/duga`  
**Generated:** 2026-05-10  
**Updated:** 2026-05-30 (added EPIC-33 TUI redesign)  
**Spec version:** 1.2  
**Plan version:** Implementation-Grade Execution Plan  

---

## 1. Epic Breakdown Summary

| Epic | Name | Tasks | §SPEC | Est. Hours |
|------|------|-------|-------|------------|
| EPIC-1 | Foundation | 8 | §4, §6–10, §27 | 32 |
| EPIC-2 | Security Layer | 7 | §13–16, §18, §20 | 28 |
| EPIC-3 | Tool Trait System | 6 | §7–8, §10 | 24 |
| EPIC-4 | Built-in Tools | 10 | §11–12, §16–17 | 48 |
| EPIC-5 | Execution Engine | 5 | §17, §19–20 | 20 |
| EPIC-6 | Memory System | 7 | §21–23 | 28 |
| EPIC-7 | Event System | 8 | §6b, §24–25 | 32 |
| EPIC-8 | LLM Layer | 8 | §6a, §32 | 32 |
| EPIC-9 | Validation Pipeline | 5 | §8 | 20 |
| EPIC-10 | Core ReAct Loop | 9 | §5, §26–27 | 40 |
| EPIC-11 | WASM Plugin System | 10 | §28–30 | 40 |
| EPIC-12 | CLI + Config | 7 | §32–33 | 28 |
| EPIC-13 | Observability | 5 | §24, §25, §31 | 20 |
| EPIC-14 | Testing Harness | 6 | §6a, §6b, §25 | 24 |
| EPIC-15 | Build + CI | 6 | §33 | 24 |
| EPIC-16 | Telegram Bot Frontend | 12 | §5, §24, §26, §32 | 63 |
| EPIC-17 | Terminal UI Frontend | 9 | §5, §24, §26, §32 | 40 |
| EPIC-18 | Frontend Shared Runtime | 6 | §5, §13–17, §21–26, §31–32 | 30 |
| EPIC-19 | Provider Compatibility Follow-up | 4 | §6a, §32 | 14 |
| EPIC-20 | Provider API Key and Base URL Config | 4 | §32 | 14 |
| EPIC-21 | Binary Allowlist Patterns | 4 | §16, §17 | 11 |
| EPIC-22 | Step Descriptions | 5 | §24–25 | 18 |
| EPIC-23 | Task Anchoring Prefix | 3 | §21, §26 | 6 |
| EPIC-24 | Sliding Window & Budget Enforcement | 4 | §21–23 | 8.5 |
| EPIC-25 | Semantic Summarization | 4 | §21–23 | 8.5 |
| EPIC-26 | Loop-Agnostic Core | 15 | §5 (redesign) | 86 |
| EPIC-27 | Markdown → Telegram HTML | 5 | §5, §24 | 12 |
| EPIC-28 | File Attachment Sending | 7 | §5, §24, §11 | 21 |
| EPIC-29 | Core Skills Infrastructure | 6 | §5, §32 | 12 |
| EPIC-30 | Skills Tooling & Telegram Integration | 6 | §5, §11, §24 | 11 |
| EPIC-31 | Steering — Dynamic Mid-Loop Guidance | 12 | §5 (new) | 53 |
| EPIC-32 | Thinking Streaming — LLM Reasoning in TUI | 10 | §5, §24, §32 | 41.5 |
| EPIC-33 | TUI Redesign — Modern Professional Terminal UI | 14 | §5, §24, §32 | 53 |
| **Total** | | **245** | | **961** |

---

## 2. Full Dependency Graph

```mermaid
graph TD
    %% Foundation (root tasks)
    T1.1[1.1 Workspace crate scaffold] --> T1.2[1.2 Message, Role, ContentBlock]
    T1.1 --> T1.3[1.3 ToolCall, ToolSchema, AssistantMessage]
    T1.2 --> T1.4[1.4 ToolResult struct + builder]
    T1.3 --> T1.4
    T1.4 --> T1.5[1.5 ToolError enum + is_transient]
    T1.2 --> T1.6[1.6 AgentError enum]
    T1.2 --> T1.7[1.7 AgentConfig + AgentLimits + OutputLimits]
    T1.4 --> T1.8[1.8 LlmCallOptions, LlmResponse, TokenUsage, SummaryMessage]

    %% Security
    T1.1 --> T2.1[2.1 Workspace struct + cap_std Dir wrapper]
    T2.1 --> T2.2[2.2 Workspace::resolve path validation]
    T1.7 --> T2.3[2.3 BinaryRegistry + resolve at startup]
    T2.1 --> T2.4[2.4 run_captured + OutputLimits enforcement]
    T2.4 --> T2.5[2.5 SanitizedEnv + PATH override]
    T2.3 --> T2.6[2.6 ShellSession struct + SessionCommand enum]
    T2.5 --> T2.7[2.7 ShellSession::classify + apply mutations]

    %% Tool trait
    T1.4 --> T3.1[3.1 Tool trait definition]
    T1.3 --> T3.1
    T1.5 --> T3.1
    T2.1 --> T3.2[3.2 ToolContext struct]
    T3.1 --> T3.3[3.3 ToolDispatcher::new + schemas + get]
    T3.1 --> T3.4[3.4 ToolDispatcher::dispatch - tool resolution]
    T3.3 --> T3.5[3.5 Schema validation: parse JSON + validate + deserialize]
    T1.4 --> T3.6[3.6 ToolResult::from_outcome builder]

    %% Built-in tools
    T3.1 --> T4.1[4.1 ReadTool - cap_std read, offset, limit]
    T2.1 --> T4.1
    T3.1 --> T4.2[4.2 ReadTool - binary detection via NUL sniffing]
    T3.1 --> T4.3[4.3 WriteTool - atomic write via tempfile + rename]
    T2.1 --> T4.3
    T3.1 --> T4.4[4.4 WriteTool - per-path mutex serialization]
    T3.1 --> T4.5[4.5 ShellTool - dispatch to run_captured or ShellSession]
    T2.4 --> T4.5
    T2.7 --> T4.5
    T3.1 --> T4.6[4.6 ShellTool - Sandbox + timeout enforcement]
    T3.1 --> T4.7[4.7 SearchTool - walkdir + regex in spawn_blocking]
    T3.1 --> T4.8[4.8 SearchTool - literal mode + max_results cap]
    T3.1 --> T4.9[4.9 ThinkTool - echo thought + think limit enforcement]
    T1.7 --> T4.9
    T4.1 --> T4.10[4.10 Builtin tool integration tests]

    %% Execution engine
    T2.6 --> T5.1[5.1 ShellSession::classify command parser]
    T2.7 --> T5.2[5.2 ShellSession mutation apply]
    T2.4 --> T5.3[5.3 tokio::process::Command wrapper]
    T2.5 --> T5.4[5.4 SanitizedEnv builder from config]
    T2.4 --> T5.5[5.5 OutputLimits truncation in run_captured]

    %% Memory
    T1.2 --> T6.1[6.1 Memory struct + new constructor]
    T1.8 --> T6.2[6.2 Memory::push_user + push_assistant + push_tool_result]
    T6.1 --> T6.3[6.3 Memory::messages ordering]
    T6.1 --> T6.4[6.4 Memory::over_budget token counting]
    T6.4 --> T6.5[6.5 Context compression - oldest half summarization]
    T6.5 --> T6.6[6.6 Summary overflow rules + pinned facts]
    T6.6 --> T6.7[6.7 Memory integration tests with MockSummarizer]

    %% Events
    T1.2 --> T7.1[7.1 Event enum - all variants]
    T7.1 --> T7.2[7.2 EventSink trait + NullSink]
    T7.1 --> T7.3[7.3 SeqAllocator + seq attachment]
    T7.2 --> T7.4[7.4 JsonlSink with writer task]
    T7.2 --> T7.5[7.5 MultiSink fan-out]
    T7.1 --> T7.6[7.6 Redactor - secret pattern matching]
    T7.4 --> T7.7[7.7 Redactor applied before JsonlSink serialization]
    T1.7 --> T7.8[7.8 ToolCallStarted/Finished event emission]

    %% LLM
    T1.2 --> T8.1[8.1 LlmClient trait + LlmError]
    T7.2 --> T8.1
    T8.1 --> T8.2[8.2 ProviderRegistry + ProviderFactory trait]
    T8.2 --> T8.3[8.3 OpenAI LlmClient implementation]
    T8.2 --> T8.4[8.4 Anthropic LlmClient implementation]
    T8.2 --> T8.5[8.5 Ollama LlmClient implementation]
    T8.3 --> T8.6[8.6 Streaming SSE parser for OpenAI]
    T1.8 --> T8.7[8.7 count_tokens with tiktoken-rs]
    T8.2 --> T8.8[8.8 Provider conformance tests]

    %% Validation
    T3.1 --> T9.1[9.1 json_schema generation from Tool trait]
    T3.3 --> T9.2[9.2 JSON parse step in dispatch]
    T9.1 --> T9.3[9.3 jsonschema validate step]
    T9.2 --> T9.4[9.4 Typed deserialize from validated JSON]
    T3.4 --> T9.5[9.5 End-to-end validation pipeline tests]

    %% Core loop
    T3.3 --> T10.1[10.1 AgentLoop struct + constructor]
    T6.1 --> T10.2[10.2 AgentLoop::run - limit checks]
    T8.1 --> T10.3[10.3 AgentLoop::run - LLM call with cancellation]
    T10.1 --> T10.4[10.4 AgentLoop::run - memory bookkeeping]
    T10.1 --> T10.5[10.5 AgentLoop::run - termination condition]
    T3.4 --> T10.6[10.6 AgentLoop::handle_tool_call - dispatch + retry]
    T10.6 --> T10.7[10.7 handle_tool_call - ToolCallStarted/Finished events]
    T10.1 --> T10.8[10.8 AgentLoop::run - compression trigger]
    T10.2 --> T10.9[10.9 Core loop integration tests with mocks]

    %% WASM plugins
    T9.1 --> T11.1[11.1 WIT file + wit-bindgen generate host/guest]
    T11.1 --> T11.2[11.2 Example guest plugin build]
    T3.1 --> T11.3[11.3 WasmPluginAdapter implements Tool]
    T11.1 --> T11.4[11.4 load_plugins - enumerate + validate .wasm]
    T11.3 --> T11.5[11.5 Plugin instantiation + info() call]
    T11.5 --> T11.6[11.6 WASI capability gating per plugin config]
    T11.5 --> T11.7[11.7 Plugin execute with fresh Store per call]
    T11.6 --> T11.8[11.8 Plugin capability enforcement tests]
    T11.7 --> T11.9[11.9 Plugin cancellation - fuel or epoch]
    T11.4 --> T11.10[11.10 Plugin name collision with built-in tools]

    %% CLI + config
    T1.7 --> T12.1[12.1 Config struct + serde deserialize]
    T12.1 --> T12.2[12.2 Config::load - YAML parse + ~ expansion]
    T12.2 --> T12.3[12.3 Config validation - allowed_binaries existence]
    T12.3 --> T12.4[12.4 Config secret-pattern warnings]
    T2.1 --> T12.5[12.5 harness main - wiring all components]
    T12.5 --> T12.6[12.6 CLI arg parsing with clap]
    T12.6 --> T12.7[12.7 Signal handling - Ctrl+C cancellation]

    %% Observability
    T7.1 --> T13.1[13.1 tracing spans for AgentLoop::run]
    T7.4 --> T13.2[13.2 tracing spans for tool dispatch + execute]
    T7.4 --> T13.3[13.3 Structured log fields for latency + tokens]
    T13.1 --> T13.4[13.4 Replay JSONL format + roundtrip validation]
    T13.4 --> T13.5[13.5 Golden file replay fixtures]

    %% Testing harness
    T3.1 --> T14.1[14.1 MockTool - configurable responses]
    T8.1 --> T14.2[14.2 MockLlm - scripted LlmResponse queue]
    T7.2 --> T14.3[14.3 CapturingEventSink - in-memory event capture]
    T10.1 --> T14.4[14.4 E2E integration test harness]
    T14.1 --> T14.5[14.5 E2E smoke test - fibonacci scenario]
    T14.4 --> T14.6[14.6 Replay roundtrip test]

    %% Build + CI
    T12.6 --> T15.1[15.1 rust-toolchain.toml + deny.toml]
    T15.1 --> T15.2[15.2 Static build config - Linux musl]
    T15.2 --> T15.3[15.3 Static build config - macOS + Windows]
    T15.3 --> T15.4[15.4 cargo-xtask build + dist commands]
    T15.4 --> T15.5[15.5 GitHub Actions CI matrix]
    T15.5 --> T15.6[15.6 cargo deny + clippy in CI]

    %% Frontend shared runtime
    T12.5 --> T18.1[18.1 Shared runtime composition]
    T12.1 --> T18.2[18.2 Shared typed frontend/runtime config]
    T18.1 --> T18.3[18.3 MEMORY.md + SKILL.md loading]
    T7.2 --> T18.4[18.4 Non-blocking frontend event bridge]
    T3.4 --> T18.5[18.5 Shared tool confirmation middleware]
    T18.1 --> T18.5
    T2.4 --> T18.6[18.6 CommandExecutor abstraction + Docker mode]
    T18.2 --> T18.6

    %% Telegram bot frontend
    T18.2 --> T16.1[16.1 Telegram config + validation]
    T18.1 --> T16.2[16.2 duga-telegram-bot crate]
    T18.6 --> T16.2
    T16.1 --> T16.2
    T16.2 --> T16.3[16.3 Telegram auth + commands + callbacks]
    T16.3 --> T16.4[16.4 Per-chat session manager + ChannelQueue]
    T18.4 --> T16.5[16.5 Telegram renderer + live message editing]
    T16.4 --> T16.6[16.6 Run AgentLoop per message]
    T16.5 --> T16.6
    T18.1 --> T16.6
    T18.3 --> T16.6
    T16.6 --> T16.7[16.7 Telegram confirmation UI]
    T18.5 --> T16.7
    T16.6 --> T16.8[16.8 Scheduled events]
    T8.6 --> T16.9[16.9 Streaming LLM to Telegram]
    T16.5 --> T16.9
    T16.2 --> T16.10[16.10 Attachment downloading]
    T16.6 --> T16.10
    T16.4 --> T16.11[16.11 Message logging]
    T16.6 --> T16.12[16.12 Integration tests + docs]
    T16.7 --> T16.12
    T16.8 --> T16.12
    T16.9 --> T16.12
    T16.10 --> T16.12
    T16.11 --> T16.12

    %% Terminal UI frontend
    T18.1 --> T17.1[17.1 duga-tui crate + ratatui setup]
    T18.2 --> T17.1
    T18.6 --> T17.1
    T17.1 --> T17.2[17.2 TUI config + terminal lifecycle]
    T17.1 --> T17.3[17.3 App state + async event loop]
    T17.3 --> T17.4[17.4 Transcript pane + input composer]
    T18.4 --> T17.5[17.5 TUI renderer + UI event mapping]
    T17.3 --> T17.6[17.6 Run AgentLoop with cancellation]
    T17.5 --> T17.6
    T18.1 --> T17.6
    T18.3 --> T17.6
    T17.6 --> T17.7[17.7 Tool panels + confirmation dialogs]
    T18.5 --> T17.7
    T17.6 --> T17.8[17.8 Replay/session browser]
    T17.7 --> T17.9[17.9 TUI integration tests + docs]
    T17.8 --> T17.9

    %% Provider compatibility follow-up
    T18.2 --> T19.1[19.1 Provider history + examples]
    T8.3 --> T19.2[19.2 OpenAI-compatible conformance tests]
    T8.6 --> T19.2
    T19.1 --> T19.3[19.3 Provider config migration notes]
    T19.2 --> T19.4[19.4 Native Ollama decision gate]

    %% Binary allowlist patterns
    T18.6 --> T21.1[21.1 allow_all_binaries config flag]
    T2.3 --> T21.2[21.2 glob/wildcard pattern support]
    T21.1 --> T21.2
    T21.1 --> T21.3[21.3 bare command names in Docker allow-all]
    T21.1 --> T21.4[21.4 documentation]
    T21.2 --> T21.4
```

---

## 3. Critical Path (MVP)

The minimum task sequence to reach a runnable agent that completes:

> *"Write fibonacci(n: u64) -> u64 in Rust with tests; make cargo test pass — using only read, edit, write, shell tools."*

```
T1.1 → T1.2 → T1.3 → T1.4 → T1.5 → T1.7
                                     ↓
T2.1 → T2.2 → T2.3 → T2.4 → T2.5 → T2.6 → T2.7
                                              ↓
T3.1 → T3.2 → T3.3 → T3.4 → T3.5
                              ↓
T4.1 → T4.2 → T4.3 → T4.4 → T4.5 → T4.6 → T4.9
                                              ↓
T6.1 → T6.2 → T6.3 → T6.4
                        ↓
T8.1 → T8.2 → T8.3 → T8.6 → T8.7
                                ↓
T7.1 → T7.2 → T7.3 → T7.4 → T7.7
                                ↓
T10.1 → T10.2 → T10.3 → T10.4 → T10.5 → T10.6 → T10.8
                                                    ↓
T12.1 → T12.2 → T12.5 → T12.6
                            ↓
T14.1 → T14.2 → T14.3 → T14.4 → T14.5
```

**MVP task count:** 52 tasks  
**Estimated MVP effort:** ~200 hours (5 weeks for one developer)

---

## 4. High-Risk Tasks

| Task | Risk | Impact | Likelihood | Mitigation |
|------|------|--------|------------|------------|
| **T11.1** WIT + wit-bindgen | wit-bindgen 0.30 API instability; version mismatch with wasmtime 25+ | Blocks all WASM plugins | Medium | Pin exact wit-bindgen/wasmtime versions in Cargo.toml. Use build.rs with `anyhow` for clear error messages. Test host+guest compilation as first step. |
| **T11.6** WASI capability gating | cap_std Dir preopen conflicts with wasmtime WASI preview2 expectations | Plugin filesystem broken | Medium | Test plugin FS access in integration test immediately after T11.5. Use `wasmtime_wasi::Dir` directly for FD 3 preopen. |
| **T8.6** Streaming SSE parser | OpenAI, Anthropic, Ollama differ in SSE framing (data: prefix, [DONE] sentinel, empty lines) | Streaming broken across providers | High | Each provider gets its own SSE parser module. Test with recorded HTTP responses (no live API required). Token delta ordering GAP G3 must be resolved first. |
| **T6.5** Context compression correctness | Summarizer may drop critical context; pinned fact preservation bug | Agent loses task context silently | High | Test with known input/output pairs. Verify pinned facts survive verbatim. Test summary-never-summarizes-summaries rule explicitly. Add golden file tests. |
| **T11.9** Plugin cancellation | wasmtime fuel vs epoch choice (GAP G10); fuel is deterministic but may not interrupt infinite loops; epoch is wall-clock but non-deterministic | Plugin hangs block agent | Medium | Benchmark both approaches in P10. Default to fuel with generous limit; expose epoch as config option. Test with intentional infinite-loop plugin. |
| **T2.2** Workspace path validation on macOS/Windows | cap_std `RESOLVE_BENEATH` only on Linux; macOS uses `O_NOFOLLOW` + manual checks; Windows uses `OBJ_DONT_REPARSE` | Path traversal on non-Linux | Medium | CI must run on all 3 OS. Test absolute path rejection, `..` traversal rejection, and symlink escape on each platform. File cap_std issues if behavior differs. |
| **T10.3** LLM call cancellation | `tokio::select!` biased mode may starve LLM response; reqwest cancellation differs from tokio::process cancellation | Cancellation not responsive | Medium | Test with slow LLM mock that takes >1s. Verify cancellation returns within 100ms. Use `tokio::time::timeout` as backup. |
| **T16.7** Telegram confirmation UI | Remote chat can trigger file writes or shell commands without local terminal context | Unsafe remote execution | High | Restrict allowed chat IDs, require shared confirmation middleware for risky tools with Telegram inline keyboard UI, default to conservative progress, and log decisions to JSONL replay. |
| **T18.4** Frontend event bridge | Slow Telegram API calls or TUI rendering could block `AgentLoop::emit` if implemented directly in an event sink | Agent slowed or failed by UI transport | High | Use bounded mpsc channels and renderer workers; JSONL sinks remain strict, UI sinks are best-effort with logged drops. |
| **T18.6** Docker sandbox executor | `docker exec` requires Docker daemon; path translation between host and container may be fragile | Agent can't execute commands | Medium | Introduce `CommandExecutor`, validate container at startup, keep file tools host/capability-bounded, test path translation, and provide clear fix instructions. |
| **T17.2** Terminal raw-mode restoration | Panic or cancellation can leave the terminal in raw/alternate-screen mode | Broken local terminal session | Medium | Use a terminal guard with Drop cleanup, install panic cleanup, and add smoke tests for startup/shutdown paths. |

---

## 5. Suggested Execution Order (Day-by-Day)

### Week 1 — Foundation + Security (Days 1–5)

| Day | Tasks | Hours |
|-----|-------|-------|
| 1 | T1.1, T1.2, T1.3 | 8 |
| 2 | T1.4, T1.5, T1.6, T1.7 | 8 |
| 3 | T1.8, T2.1, T2.2 | 8 |
| 4 | T2.2 (cont.), T2.3, T2.4 | 8 |
| 5 | T2.5, T2.6, T2.7 | 8 |

### Week 2 — Tools + Execution Engine (Days 6–10)

| Day | Tasks | Hours |
|-----|-------|-------|
| 6 | T3.1, T3.2, T3.3 | 8 |
| 7 | T3.4, T3.5, T3.6 | 8 |
| 8 | T4.1, T4.2, T4.3 | 8 |
| 9 | T4.4, T4.5, T4.6 | 8 |
| 10 | T4.7, T4.8, T4.9, T5.1 | 8 |

### Week 3 — Memory + Events + LLM (Days 11–15)

| Day | Tasks | Hours |
|-----|-------|-------|
| 11 | T5.2–T5.5, T6.1, T6.2 | 8 |
| 12 | T6.3–T6.6 | 8 |
| 13 | T6.7, T7.1–T7.4 | 8 |
| 14 | T7.5–T7.8, T8.1, T8.2 | 8 |
| 15 | T8.3–T8.7 | 8 |

### Week 4 — Loop + Validation + CLI (Days 16–20)

| Day | Tasks | Hours |
|-----|-------|-------|
| 16 | T8.8, T9.1–T9.4 | 8 |
| 17 | T9.5, T10.1–T10.3 | 8 |
| 18 | T10.4–T10.7 | 8 |
| 19 | T10.8–T10.9, T12.1–T12.3 | 8 |
| 20 | T12.4–T12.7 | 8 |

### Week 5 — WASM + Testing + Remaining (Days 21–25)

| Day | Tasks | Hours |
|-----|-------|-------|
| 21 | T11.1–T11.4 | 8 |
| 22 | T11.5–T11.8 | 8 |
| 23 | T11.9–T11.10, T14.1–T14.3 | 8 |
| 24 | T14.4–T14.6, T13.1–T13.3 | 8 |
| 25 | T13.4–T13.5, Buffer/overflow day | 8 |

### Week 6 — Build/CI + Polish (Days 26–30, optional)

| Day | Tasks | Hours |
|-----|-------|-------|
| 26–27 | T15.1–T15.4 | 16 |
| 28–29 | T15.5–T15.6, T4.10, T5.x integration | 16 |
| 30 | Final integration, clippy clean, doc review | 8 |

### Week 7 — Frontends (optional)

| Day | Tasks | Hours |
|-----|-------|-------|
| 31 | T18.1–T18.2 (shared runtime + typed config) | 10 |
| 32 | T18.3–T18.5 (memory, skills, event bridge, confirmation middleware) | 14 |
| 33 | T18.6, T16.1–T16.2 (Docker executor + Telegram startup) | 14 |
| 34 | T16.3–T16.5 (auth, sessions, renderer) | 18 |
| 35 | T16.6–T16.8 (run agent, confirmation UI, events) | 19 |
| 36 | T16.9–T16.12 (streaming, attachments, logging, tests/docs) | 18 |
| 37 | T17.1–T17.4 (TUI setup, state, input) | 16 |
| 38 | T17.5–T17.7 (TUI renderer, runtime, confirmations) | 14 |
| 39 | T17.8–T17.9 (replay browser + tests/docs) | 10 |
| 40 | T19.1–T19.4 (provider compatibility follow-up) | 14 |

---

## 6. GAP Resolution Required Before Tasks

Tasks blocked until their GAP is resolved:

| GAP | Blocks Tasks | Question |
|-----|-------------|----------|
| G1 (concurrency) | T10.6 | Sequential or parallel tool calls? |
| G2 (emit semantics) | T7.2, T7.4, T7.5 | Bounded or unbounded channel? Drop on overflow? |
| G3 (delta ordering) | T8.6, T10.3 | Must deltas have seq < LlmResponse? |
| G4 (replay binary) | T14.6 | Binary or format only? |
| G5 (search impl) | T4.7 | In-process or shell-out to rg? |
| G6 (count_tokens) | T6.4, T8.7 | Async HTTP or local approximation? |
| G7 (concurrent access) | T3.2, T2.1 | Shared &Workspace for concurrent tools? |
| G8 (write mutex key) | T4.4 | Raw path or canonicalized? |
| G9 (symlink default) | T4.1, T4.3 | Open fails or special object? |
| G10 (plugin cancel) | T11.9 | Fuel or epoch? |
| G11 (session lifecycle) | T2.6, T5.1 | Implicit or explicit creation? |
| G12 (think accounting) | T4.9 | thought text or assistant tokens? |
| G13 (tokenizer source) | T6.4 | Arc<dyn LlmClient> or Arc<dyn Tokenizer>? |
| G14 (redaction seq) | T7.7 | Pre- or post-serialization? |
| G15 (hot reload) | T11.4 | Post-MVP; not blocking MVP |
| G16 (platform cap_std) | T2.1, T2.2 | Confirm cap_std equivalents |
| G17 (provider suite) | T8.8 | Mandatory conformance fixtures? |
| G18 (Telegram session policy) | T16.4, T16.6 | One active run per chat, queue, or parallel runs? -> **Resolved: one run per chat with ChannelQueue serialization; different chats may run in parallel.** |
| G19 (confirmation middleware) | T16.7, T17.7, T18.5 | Pause tool call for confirm/deny, or fail and ask user to retry? -> **Resolved: shared middleware pauses only the pending tool call; Telegram/TUI provide UI-specific approval.** |
| G20 (TUI event loop model) | T17.3, T17.6 | Single async loop, actor model, or channels between UI/runtime tasks? -> **Resolved: channels between UI/runtime tasks using the EPIC-18 frontend event bridge.** |
| G21 (Docker filesystem scope) | T18.6 | Do all tools execute in Docker, or only process execution? -> **Resolved: Docker mode applies to `shell`/process execution; read/write/search remain host capability tools over the bind-mounted workspace.** |
| G22 (allowlist bypass in Docker) | T21.1, T21.3 | Should Docker mode support `allow_all_binaries` to bypass the host binary allowlist, since the container provides OS-level isolation? -> **Resolved: yes; added as EPIC-21 with an explicit opt-in flag and config warning for non-container modes.** |

---

## 7. Label Taxonomy

Applied consistently across all tasks:

```
# Type
epic/foundation, epic/security, epic/tools, epic/builtin-tools,
epic/execution, epic/memory, epic/events, epic/llm, epic/validation,
epic/loop, epic/core, epic/wasm, epic/cli, epic/observability, epic/testing,
epic/build, epic/telegram, epic/tui, epic/frontend-runtime,
epic/provider-compatibility, epic/steering

# Layer
layer/foundation, layer/security, layer/tools, layer/tools-builtin,
layer/sandbox, layer/memory, layer/events, layer/llm,
layer/validation, layer/loop, layer/wasm, layer/cli,
layer/observability, layer/testing, layer/build, layer/frontend,
layer/telegram, layer/tui, layer/runtime, layer/config,
layer/provider, layer/docs, layer/steering

# Priority
priority/critical (MVP blocker, 52 tasks)
priority/high (QA/safety tasks)
priority/normal (polish, optional epics)

# Status
status/blocked (waiting on GAP or dependency)
status/ready (unblocked, can be picked up)
```

---

## Files in This Directory

| File | Contents |
|------|----------|
| `00-overview.md` | This file — summary, graphs, critical path, risks |
| `epic-01-foundation.md` | EPIC-1: Foundation (8 tasks) |
| `epic-02-security.md` | EPIC-2: Security Layer (7 tasks) |
| `epic-03-tool-trait.md` | EPIC-3: Tool Trait System (6 tasks) |
| `epic-04-builtin-tools.md` | EPIC-4: Built-in Tools (10 tasks) |
| `epic-05-execution-engine.md` | EPIC-5: Execution Engine (5 tasks) |
| `epic-06-memory.md` | EPIC-6: Memory System (7 tasks) |
| `epic-07-events.md` | EPIC-7: Event System (8 tasks) |
| `epic-08-llm-layer.md` | EPIC-8: LLM Layer (8 tasks) |
| `epic-09-validation.md` | EPIC-9: Validation Pipeline (5 tasks) |
| `epic-10-core-loop.md` | EPIC-10: Core ReAct Loop (9 tasks) |
| `epic-11-wasm-plugins.md` | EPIC-11: WASM Plugin System (10 tasks) |
| `epic-12-cli-config.md` | EPIC-12: CLI + Config (7 tasks) |
| `epic-13-observability.md` | EPIC-13: Observability (5 tasks) |
| `epic-14-testing.md` | EPIC-14: Testing Harness (6 tasks) |
| `epic-15-build-ci.md` | EPIC-15: Build + CI (6 tasks) |
| `epic-16-telegram-bot.md` | EPIC-16: Telegram Bot Frontend (12 tasks) |
| `epic-17-terminal-ui.md` | EPIC-17: Terminal UI Frontend (9 tasks) |
| `epic-18-frontend-shared-runtime.md` | EPIC-18: Frontend Shared Runtime (6 tasks) |
| `epic-19-provider-compatibility.md` | EPIC-19: Provider Compatibility Follow-up (4 tasks) |
| `epic-20-provider-config.md` | EPIC-20: Provider API Key and Base URL Config (4 tasks) |
| `epic-21-binary-allowlist-patterns.md` | EPIC-21: Binary Allowlist Patterns (4 tasks) |
| `epic-22-step-descriptions.md` | EPIC-22: Step Descriptions in Frontend Events (5 tasks) |
| `epic-23-task-anchoring.md` | EPIC-23: Task Anchoring Prefix (3 tasks) |
| `epic-24-sliding-window.md` | EPIC-24: Sliding Window & Budget Enforcement (4 tasks) |
| `epic-25-semantic-summarization.md` | EPIC-25: Semantic Summarization (4 tasks) |
| `epic-26-loop-agnostic-core.md` | EPIC-26: Loop-Agnostic Core — Agent Loop System Redesign (15 tasks) |
| `epic-27-markdown-html-formatting.md` | EPIC-27: Markdown → Telegram HTML Conversion (5 tasks) |
| `epic-28-file-attachment-send.md` | EPIC-28: File Attachment Sending (7 tasks) |
| `epic-31-steering.md` | EPIC-31: Steering — Dynamic Mid-Loop Guidance Injection (12 tasks) |
| `epic-32-thinking-streaming.md` | EPIC-32: Thinking Streaming — LLM Reasoning Display in TUI (10 tasks) |
| `epic-33-tui-redesign.md` | EPIC-33: TUI Redesign — Modern, Dense, Professional Terminal UI (14 tasks) |
