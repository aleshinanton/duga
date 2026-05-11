# EPIC-12: CLI + Config

**§SPEC:** §32–33  
**Labels:** `epic/cli`  
**Crates:** `duga-config`, `duga-harness`

## Goal
YAML config loading with validation, CLI arg parsing with clap, provider routing from model string, signal handling, and the composition root that wires everything together.

---

### TASK-12.1: Config struct + serde deserialize

- **§SPEC:** §32 (Config — all fields)
- **Labels:** `layer/cli`, `priority/critical`
- **Description:** Define the full `Config` struct hierarchy matching the YAML spec in SECT 32: `Config { model, agent: AgentConfig, sandbox: SandboxConfig, workspace: WorkspaceConfig, environment: EnvironmentConfig, memory: MemoryConfig, plugins: PluginConfig }`. All sub-structs derive `Serialize`, `Deserialize`. `AgentConfig` already exists in duga-types; SandboxConfig has `{ timeout: Duration, allowed_binaries: Vec<String> }`. WorkspaceConfig: `{ root: String }`. MemoryConfig: `{ max_tokens: usize, compress_at_ratio: f64 }`. PluginConfig: `{ dir: String, modules: Vec<PluginModuleConfig> }`.
- **Files affected:**
  - `crates/duga-config/Cargo.toml` (new)
  - `crates/duga-config/src/lib.rs` (new)
  - `crates/duga-config/src/config.rs` (new)
- **Types involved:** `Config`, `SandboxConfig`, `WorkspaceConfig`, `EnvironmentConfig`, `MemoryConfig`, `PluginConfig`, `PluginModuleConfig`
- **Functions to implement:** (struct definitions with derives)
- **Dependencies:** TASK-1.7 (AgentConfig)
- **Implementation steps:**
  1. Create `duga-config` crate
  2. Define all config structs
  3. `SandboxConfig.timeout` uses Duration deserialization from TASK-1.7's helper
  4. `EnvironmentConfig.allowed: HashSet<String>`
  5. `PluginModuleConfig { name: String, capabilities: WasiCapabilities }` — WasiCapabilities from TASK-11.6
  6. Test deserialization from YAML
- **Definition of Done:** All structs deserialize from YAML
- **Acceptance criteria:**
  - Full SECT 32 YAML → parses to valid Config
  - Partial YAML → error with helpful message
- **Test plan:** unit: Load test YAML fixtures
- **Estimated effort:** 4 hours

---

### TASK-12.2: Config::load — YAML parse + ~ expansion

- **§SPEC:** §32 (config loading, ~ expansion)
- **Labels:** `layer/cli`, `priority/critical`
- **Description:** Implement `Config::load(path: impl AsRef<Path>) -> Result<Config, ConfigError>`. Read YAML file, parse with `serde_yaml`, expand `~` in `workspace.root` using `$HOME` env var (no shell). `ConfigError` enum: `FileNotFound(PathBuf)`, `ParseError(String)`, `ValidationError(String)`.
- **Files affected:**
  - `crates/duga-config/src/lib.rs` (load function)
  - `crates/duga-config/src/error.rs` (new)
- **Types involved:** `Config`, `ConfigError`
- **Functions to implement:**
  - `Config::load(path: impl AsRef<Path>) -> Result<Self, ConfigError>`
  - `fn expand_tilde(path: &str) -> PathBuf`
- **Dependencies:** TASK-12.1
- **Implementation steps:**
  1. Read file to string
  2. Parse with `serde_yaml::from_str`
  3. `expand_tilde`: if path starts with `~/` → replace with `$HOME`; if `~` only → replace with `$HOME`
  4. Apply to `config.workspace.root`
  5. Return Config or error
- **Edge cases:**
  - `$HOME` not set → error (required for ~ expansion)
  - `~otheruser/` → not expanded (spec says `$HOME` only)
  - Windows: `~` not used; `%USERPROFILE%` → skip tilde expansion on Windows, use path as-is
- **Definition of Done:** Config loads, tilde expanded
- **Acceptance criteria:**
  - `Config::load("valid.yaml")` → Ok
  - `~/projects/demo` → `/home/user/projects/demo`
  - Nonexistent file → `Err(FileNotFound)`
- **Test plan:** unit: Test with temp YAML files
- **Estimated effort:** 3 hours

---

### TASK-12.3: Config validation — allowed_binaries existence

- **§SPEC:** §16 (binary resolution at startup), §32 (config validation)
- **Labels:** `layer/cli`, `priority/critical`
- **Description:** After loading config, validate that all `allowed_binaries` paths exist and are executable. If any path does not exist → `ConfigError::ValidationError("binary not found: /path/to/bin")`. Also validate `workspace.root` exists and is a directory. Validate `max_runtime > 0`, `max_steps > 0`, `max_tokens > 0`.
- **Files affected:**
  - `crates/duga-config/src/lib.rs` (validate function)
- **Types involved:** `Config`, `ConfigError`
- **Functions to implement:**
  - `Config::validate(&self) -> Result<(), ConfigError>`
- **Dependencies:** TASK-12.2
- **Implementation steps:**
  1. Check `sandbox.allowed_binaries` — each path exists and is file
  2. Check `workspace.root` exists and is directory
  3. Check `agent.limits.max_steps > 0`
  4. Check `agent.limits.max_tool_calls > 0`
  5. Check `agent.limits.max_runtime > Duration::ZERO`
  6. Check `memory.max_tokens > 0`
  7. Check `memory.compress_at_ratio` between 0.0 and 1.0
  8. Return all validation errors at once (collect, don't fail-fast)
- **Edge cases:**
  - Binary path is relative → reject (spec requires absolute paths in config)
  - Workspace root is a symlink to outside → cap_std will handle at runtime
- **Definition of Done:** All validations pass for valid config
- **Acceptance criteria:**
  - Valid config → validate() returns Ok
  - Nonexistent binary → ValidationError with all missing binaries listed
  - max_steps: 0 → ValidationError
- **Test plan:** unit: Test with valid and invalid config files
- **Estimated effort:** 3 hours

---

### TASK-12.4: Config secret-pattern warnings

- **§SPEC:** §20 (secret-pattern warnings at startup)
- **Labels:** `layer/cli`, `priority/critical`
- **Description:** During config validation, check `environment.allowed` for variables matching secret patterns (`*_TOKEN`, `*_KEY`, `*_SECRET`, `*_PASSWORD`). For each match, emit a `tracing::warn!` message identifying the variable. The variable is still allowed — the operator owns the decision.
- **Files affected:**
  - `crates/duga-config/src/lib.rs` (add secret check)
- **Types involved:** `Config`, `SanitizedEnv::is_secret_pattern` (imported from duga-sandbox)
- **Dependencies:** TASK-12.3, TASK-2.5
- **Implementation steps:**
  1. Iterate `environment.allowed`
  2. For each: if `SanitizedEnv::is_secret_pattern(name)` → `tracing::warn!("Secret-pattern variable '{}' in environment allowlist — this variable will be available to subprocesses", name)`
  3. Test
- **Edge cases:** None — just warnings
- **Definition of Done:** Warnings emitted
- **Estimated effort:** 1 hour

---

### TASK-12.5: Harness main — wiring all components

- **§SPEC:** §32–33 (composition root)
- **Labels:** `layer/cli`, `priority/critical`
- **Description:** Implement `main()` in `duga-harness` that wires all components: (1) load config, (2) open workspace, (3) build BinaryRegistry, (4) build SanitizedEnv, (5) build ProviderRegistry + LlmClient, (6) register built-in tools + load plugins, (7) build ToolDispatcher, (8) build EventSinks → MultiSink, (9) setup CancellationToken (SIGINT), (10) build Memory + Summarizer, (11) build AgentLoop, (12) run. This is the ONLY file with business logic — all other crates are libraries.
- **Files affected:**
  - `crates/duga-harness/Cargo.toml` (new)
  - `crates/duga-harness/src/main.rs` (new)
- **Types involved:** All types from all crates
- **Functions to implement:**
  - `main() -> anyhow::Result<()>`
  - `fn build_provider(registry: &ProviderRegistry, model: &str, ...) -> Result<Arc<dyn LlmClient>>`
  - `fn build_tools(workspace, registry, limits, timeout) -> ToolDispatcher`
  - `fn build_sinks(config: &Config) -> Result<Arc<MultiSink>>`
- **Dependencies:** All epics
- **Implementation steps:**
  1. Create `duga-harness` binary crate with deps on ALL other crates
  2. Wire components in order
  3. Use `anyhow::Context` for error messages
  4. Print startup banner with config summary
  5. Run agent loop
  6. Print result
  7. Flush event sink on shutdown
- **Edge cases:**
  - Any component fails to build → print clear error + exit
  - Plugin loading fails → warn, continue without plugins
  - Event sink file can't be opened → error, exit
- **Definition of Done:** `./harness --config test.yaml "echo hello"` runs
- **Acceptance criteria:**
  - Harness starts, loads config, builds loop, runs task
  - Missing config → error message
  - Invalid config → error message with details
- **Test plan:** integration: Run harness with test config, verify no panic
- **Estimated effort:** 6 hours

---

### TASK-12.6: CLI arg parsing with clap

- **§SPEC:** §33 (CLI interface)
- **Labels:** `layer/cli`, `priority/critical`
- **Description:** Implement CLI argument parsing with `clap`. Arguments: `--config <PATH>` (required), `--task <STRING>` (positional or flag), `--model <PROVIDER/MODEL>` (override config), `--tui` (optional, feature-gated), `--replay-dir <PATH>` (directory for replay files), `--verbose` (-v). Print help text with examples.
- **Files affected:**
  - `crates/duga-harness/src/cli.rs` (new)
  - `crates/duga-harness/src/main.rs` (use cli)
- **Types involved:** `clap::Parser`
- **Functions to implement:** `Cli` struct with `#[derive(Parser)]`
- **Dependencies:** TASK-12.5
- **Implementation steps:**
  1. Define `Cli` struct with clap attributes
  2. `--config`: `PathBuf`, required
  3. `--task`: `String`, positional or `-t` flag
  4. `--model`: override config model
  5. `--replay-dir`: defaults to `./replays`
  6. `--tui`: feature-gated behind `tui` feature
  7. `--verbose`: `bool`, flag
  8. Parse in main()
- **Edge cases:**
  - Missing --config → clap error message (auto)
  - Task contains shell special chars → user must quote
- **Definition of Done:** `./harness --help` works
- **Acceptance criteria:**
  - `./harness --config cfg.yaml "task"` → parses correctly
  - `./harness --help` → shows usage
  - Missing --config → clap error
- **Test plan:** unit: Parse args with various combinations
- **Estimated effort:** 3 hours

---

### TASK-12.7: Signal handling — Ctrl+C cancellation

- **§SPEC:** §26 (Cancellation — Ctrl+C)
- **Labels:** `layer/cli`, `priority/critical`
- **Description:** Implement SIGINT (Ctrl+C) handling using `tokio::signal`. On SIGINT, call `cancel_token.cancel()`. On second SIGINT, force-exit the process. Report cancellation reason in the final output. Ensure graceful shutdown: flush event sink, print summary.
- **Files affected:**
  - `crates/duga-harness/src/main.rs` (signal handler)
  - `crates/duga-harness/src/signal.rs` (new)
- **Types involved:** `CancellationToken`, `tokio::signal`
- **Functions to implement:**
  - `fn setup_signal_handler(cancel: CancellationToken) -> tokio::task::JoinHandle<()>`
- **Dependencies:** TASK-12.5
- **Implementation steps:**
  1. Spawn task that listens for `tokio::signal::ctrl_c()`
  2. On first Ctrl+C: `cancel.cancel()`, print "Cancelling... (press Ctrl+C again to force quit)"
  3. On second Ctrl+C: `std::process::exit(1)`
  4. Join the signal handler task on shutdown
- **Edge cases:**
  - Agent finishes normally → signal handler still running → drop cancels it
  - Signal during shutdown → second Ctrl+C works even during cleanup
  - Windows: Ctrl+C handling via `tokio::signal` works the same
- **Definition of Done:** Ctrl+C cancels agent
- **Acceptance criteria:**
  - Ctrl+C during agent run → agent returns AgentError::Cancelled
  - Second Ctrl+C → immediate exit
  - Normal exit without Ctrl+C → clean shutdown
- **Test plan:** integration: Start long-running task, send SIGINT, verify cancellation
- **Estimated effort:** 4 hours
