# EPIC-18: Frontend Shared Runtime

**SPEC:** §5, §13-17, §21-26, §31-32
**Labels:** `epic/frontend-runtime`
**Crates:** `duga-runtime` (new), `duga-sandbox`, `duga-config`, `duga-llm`

## Goal

Extract neutral infrastructure used by CLI, Telegram, and TUI frontends. This epic prevents Telegram/TUI duplication and keeps frontend crates focused on transport and rendering.

---

### TASK-18.1: Extract shared runtime composition from harness

- **SPEC:** §5 (core loop), §32 (composition root)
- **Labels:** `layer/frontend`, `layer/runtime`, `priority/critical`
- **Description:** Move reusable setup logic from `duga-harness` into `duga-runtime`: provider resolution, LLM building, tool/plugin registration, memory setup, summarizer wiring, JSONL replay sink creation, and `AgentLoop` construction.
- **Files affected:**
  - `crates/duga-runtime/Cargo.toml` (new)
  - `crates/duga-runtime/src/lib.rs` (new)
  - `crates/duga-runtime/src/providers.rs` (new)
  - `crates/duga-runtime/src/tools.rs` (new)
  - `crates/duga-runtime/src/agent.rs` (new)
  - `crates/duga-harness/src/main.rs` (thin wrapper)
- **Types involved:** `RuntimeBuilder`, `RuntimeConfig`, `BuiltRuntime`, `ProviderSelection`, `RunRequest`, `RunResult`
- **Functions to implement:**
  - `resolve_provider(config: &Config) -> Result<ProviderSelection>`
  - `build_llm(selection: &ProviderSelection) -> Result<Arc<dyn LlmClient>>`
  - `build_dispatcher(config: &Config, workspace: Arc<Workspace>) -> Result<Arc<ToolDispatcher>>`
  - `build_agent(config: Config, sinks: RuntimeSinks) -> Result<AgentLoop>`
- **Dependencies:** TASK-12.5, TASK-8.3, TASK-8.4
- **Implementation steps:**
  1. Create `duga-runtime` crate.
  2. Move provider/model routing into `providers.rs`.
  3. Move built-in tool and plugin registration into `tools.rs`.
  4. Move memory/summarizer/event-sink assembly into `agent.rs`.
  5. Update `duga-harness` to call the shared builder.
- **Definition of Done:** CLI behavior is unchanged and frontend crates can build agents without copying harness code.
- **Acceptance criteria:**
  - CLI still runs with existing YAML config.
  - Provider routing tests live in `duga-runtime`.
  - No duplicated provider/tool/plugin wiring remains in `duga-harness`.
- **Test plan:** unit tests for provider resolution/tool registration; CLI smoke test.
- **Estimated effort:** 6 hours

---

### TASK-18.2: Shared typed frontend/runtime config

- **SPEC:** §32 (config loading), §20 (secrets)
- **Labels:** `layer/config`, `layer/frontend`, `priority/critical`
- **Description:** Add typed shared config used by all frontends. Use enums and defaults instead of untyped strings so old configs keep working and invalid values fail validation.
- **Files affected:**
  - `crates/duga-config/src/config.rs`
- **Types involved:** `ProviderKind`, `ThinkingLevel`, `SandboxMode`, `ProgressMode`, `FrontendConfig`
- **Proposed YAML:**
  ```yaml
  provider: "openai"
  model: "gpt-4.1"
  thinking_level: "off"
  context_window: 128000
  frontend:
    progress_mode: "summary" # final_only | summary | verbose
    confirmation_timeout: 60s
  sandbox:
    mode: "capability" # host | capability | docker
  ```
- **Dependencies:** TASK-12.1, TASK-12.3
- **Implementation steps:**
  1. Add typed enums with `#[serde(rename_all = "snake_case")]`.
  2. Add `#[serde(default)]` to optional frontend/runtime fields.
  3. Validate `thinking_level` through enum deserialization.
  4. Validate `context_window` if set: >= 4096 and <= 1,000,000.
  5. Keep `provider: openai` + `BASE_URL` as the path for Ollama/OpenAI-compatible local endpoints.
  6. Do not emit secret-pattern warnings for env var names such as `TELEGRAM_BOT_TOKEN`; only warn when actual secret values would be forwarded into subprocess env.
- **Definition of Done:** Shared config is typed, backward-compatible, and frontend-neutral.
- **Acceptance criteria:**
  - Existing configs without frontend fields still parse.
  - Invalid enum values fail at load time.
  - Ollama is documented as `provider: openai` + `BASE_URL=http://localhost:11434/v1`.
- **Test plan:** config parse/validation tests.
- **Estimated effort:** 4 hours

---

### TASK-18.3: Shared MEMORY.md and SKILL.md loading

- **SPEC:** §21-23 (memory), §32 (configuration)
- **Labels:** `layer/memory`, `layer/frontend`, `priority/high`
- **Description:** Load persistent memory and skills from workspace/frontend directories and inject them into the system prompt consistently for CLI, Telegram, and TUI.
- **Files affected:**
  - `crates/duga-runtime/src/memory_context.rs` (new)
  - `crates/duga-runtime/src/skills.rs` (new)
- **Types involved:** `PersistentMemory`, `Skill`, `SkillSource`
- **Functions to implement:**
  - `load_persistent_memory(workspace: &Path, channel_dir: Option<&Path>) -> Result<PersistentMemory>`
  - `load_skills(workspace: &Path, channel_dir: Option<&Path>) -> Result<Vec<Skill>>`
  - `format_memory_for_prompt(memory: &PersistentMemory) -> String`
  - `format_skills_for_prompt(skills: &[Skill]) -> String`
- **Dependencies:** TASK-18.1, TASK-6.x
- **Implementation steps:**
  1. Read workspace-level `MEMORY.md`.
  2. Optionally read channel/session-level `MEMORY.md`.
  3. Walk `skills/` directories and parse YAML frontmatter.
  4. Resolve `{baseDir}` placeholders to the skill directory.
  5. Prefer channel/session skills over workspace skills on name collision.
- **Definition of Done:** Persistent memory and skills are loaded once through shared runtime.
- **Acceptance criteria:**
  - MEMORY.md appears in the system prompt when present.
  - SKILL.md files are parsed with collision handling.
  - Load errors are explicit and testable.
- **Test plan:** fixture-based tests for memory and skill loading.
- **Estimated effort:** 5 hours

---

### TASK-18.4: Non-blocking frontend event bridge

- **SPEC:** §24 (events), §31 (observability)
- **Labels:** `layer/events`, `layer/frontend`, `priority/critical`
- **Description:** Add a shared bridge from `EventSink` to frontend UI channels. The bridge must not perform network or terminal rendering in `EventSink::emit()` and must not fail the agent run for transient UI delivery errors.
- **Files affected:**
  - `crates/duga-runtime/src/events.rs` (new)
  - `crates/duga-events/src/lib.rs` (optional helper types)
- **Types involved:** `FrontendEventSink`, `FrontendEvent`, `FrontendEventBridge`, `EventDeliveryPolicy`
- **Functions to implement:**
  - `FrontendEventSink::new(tx, policy) -> Self`
  - `impl EventSink for FrontendEventSink`
  - `map_event(event: Event) -> Option<FrontendEvent>`
- **Dependencies:** TASK-7.2
- **Implementation steps:**
  1. Use a bounded `tokio::sync::mpsc` channel from runtime to frontend renderer.
  2. Convert high-volume events into coalescible frontend events.
  3. Keep JSONL replay sinks strict; keep frontend sinks best-effort.
  4. Log dropped/coalesced UI events instead of returning an `EventSinkFailed` to the agent.
  5. Ensure UI workers own Telegram API calls or terminal redraws.
- **Definition of Done:** Frontend rendering cannot block or fail the core agent loop.
- **Acceptance criteria:**
  - Slow Telegram/TUI rendering does not slow LLM/tool execution.
  - Full JSONL event history is still preserved.
  - Channel overflow is visible in logs/metrics.
- **Test plan:** tests for bounded channel behavior, coalescing, and non-fatal UI errors.
- **Estimated effort:** 4 hours

---

### TASK-18.5: Shared tool confirmation middleware

- **SPEC:** §13-16 (sandbox), §20 (operator control)
- **Labels:** `layer/security`, `layer/tools`, `layer/frontend`, `priority/critical`
- **Description:** Add shared tool confirmation middleware so risky tool calls can be approved by CLI/TUI/Telegram without duplicating dispatch logic.
- **Files affected:**
  - `crates/duga-runtime/src/confirmation.rs` (new)
  - `crates/duga-runtime/src/tools.rs`
  - `crates/duga-tools/src/dispatcher.rs` (optional middleware hook)
- **Types involved:** `ToolMiddleware`, `ConfirmationProvider`, `ConfirmationRequest`, `ConfirmationDecision`, `ConfirmationPolicy`
- **Functions to implement:**
  - `requires_confirmation(tool_name: &str, policy: &ConfirmationPolicy) -> bool`
  - `confirm_tool_call(request: ConfirmationRequest) -> ConfirmationDecision`
  - `dispatch_with_confirmation(...)`
- **Dependencies:** TASK-3.4, TASK-18.1
- **Implementation steps:**
  1. Define a frontend-neutral `ConfirmationProvider` async trait or callback.
  2. Use `oneshot` channels to resolve approve/deny/timeout decisions.
  3. Pause only the pending tool call, not the whole UI worker.
  4. Return explicit `ToolError::Denied` on deny or timeout.
  5. Reuse the same middleware for TUI modals and Telegram inline buttons.
- **Definition of Done:** Tool confirmation is shared and frontend-specific code only renders the prompt.
- **Acceptance criteria:**
  - Shell/edit/write can require confirmation.
  - Denial and timeout are explicit tool errors.
  - Frontends can provide different UIs over the same middleware.
- **Test plan:** middleware tests for approve, deny, timeout, cancellation.
- **Estimated effort:** 5 hours

---

### TASK-18.6: CommandExecutor abstraction and Docker mode

- **SPEC:** §15 (Sandbox), §17 (process execution)
- **Labels:** `layer/sandbox`, `priority/critical`
- **Description:** Add an executor abstraction before adding Docker. Capability mode remains the default. Docker mode affects process execution (`shell`) through `docker exec`; read/write/search remain capability-bounded host workspace tools operating on the bind-mounted workspace.
- **Files affected:**
  - `crates/duga-sandbox/src/executor.rs` (new)
  - `crates/duga-sandbox/src/docker.rs` (new)
  - `crates/duga-sandbox/src/exec.rs`
  - `crates/duga-config/src/config.rs`
- **Types involved:** `CommandExecutor`, `CapabilityExecutor`, `DockerExecutor`, `SandboxMode`
- **Functions to implement:**
  - `CommandExecutor::run(command: CommandSpec, cancel: CancellationToken) -> Result<ToolResult, ToolError>`
  - `DockerExecutor::new(container: &str, workspace_mount: &str) -> Self`
  - `validate_container(container: &str) -> Result<()>`
  - `translate_to_container(path: &Path) -> Result<PathBuf>`
  - `translate_to_host(path: &Path) -> Result<PathBuf>`
- **Config extension:**
  ```yaml
  sandbox:
    mode: "docker"                  # host | capability | docker
    container: "duga-sandbox"
    workspace_mount: "/workspace"
    timeout: 120s
  ```
- **Dependencies:** TASK-2.4, TASK-2.5, TASK-18.2
- **Implementation steps:**
  1. Introduce `CommandExecutor` and adapt existing `run_captured()` behind `CapabilityExecutor`.
  2. Implement `DockerExecutor` with `docker exec <container> sh -c <command>`.
  3. Apply the same timeout, cancellation, and output limits.
  4. Validate container existence/running state at startup.
  5. Keep file tools host/capability-based for now; document this explicitly in the system prompt.
  6. Add a separate future task if all filesystem tools should execute through container APIs.
- **Definition of Done:** CLI/TUI/Telegram can opt into Docker process execution via shared config.
- **Acceptance criteria:**
  - Docker mode runs `shell` commands inside the configured container.
  - Files created under `/workspace` are visible in host workspace due to bind mount.
  - Read/write/search behavior remains consistent and capability-bounded.
  - Output limits and cancellation still work.
  - Missing/stopped container gives a clear startup error.
- **Test plan:** unit tests for path translation; Docker integration test gated by Docker availability.
- **Estimated effort:** 6 hours

---

## Summary Table

| Task | Name | Est. Hours |
|------|------|------------|
| TASK-18.1 | Shared runtime composition | 6 |
| TASK-18.2 | Shared typed frontend/runtime config | 4 |
| TASK-18.3 | MEMORY.md and SKILL.md loading | 5 |
| TASK-18.4 | Non-blocking frontend event bridge | 4 |
| TASK-18.5 | Shared tool confirmation middleware | 5 |
| TASK-18.6 | CommandExecutor abstraction and Docker mode | 6 |
| **Total** | | **30 hours** |
