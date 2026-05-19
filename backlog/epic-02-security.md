# EPIC-2: Security Layer

**§SPEC:** §13–16, §18, §20  
**Labels:** `epic/security`  
**Crates:** `duga-sandbox` (all tasks + extends with ShellSession)

## Goal
Implement capability-based filesystem isolation via `cap_std`, binary allowlist resolution, output limits enforcement, environment variable sanitization, and shell session state management.

---

### TASK-2.1: Workspace struct + cap_std Dir wrapper

- **§SPEC:** §13 (Workspace & Filesystem Isolation)
- **Labels:** `layer/security`, `priority/critical`
- **Description:** Create the `duga-sandbox` crate. Define `Workspace` struct wrapping `cap_std::fs::Dir` + `root_path: PathBuf` (the canonical path for display/logging). Implement `Workspace::open(root: &Path) -> Result<Workspace, WorkspaceError>` which creates a `cap_std::fs::Dir` from the given root. Implement `Workspace::root_dir(&self) -> &Dir` and `Workspace::root_path(&self) -> &Path`. The `Dir` capability rejects absolute paths and `..` traversal at the syscall layer.
- **Files affected:**
  - `crates/duga-sandbox/Cargo.toml` (new)
  - `crates/duga-sandbox/src/lib.rs` (new)
  - `crates/duga-sandbox/src/workspace.rs` (new)
  - `crates/duga-sandbox/src/error.rs` (new — `WorkspaceError` enum)
- **Types involved:** `Workspace`, `WorkspaceError`, `cap_std::fs::Dir`
- **Functions to implement:**
  - `Workspace::open(root: impl AsRef<Path>) -> Result<Self, WorkspaceError>`
  - `Workspace::root_dir(&self) -> &Dir`
  - `Workspace::root_path(&self) -> &Path`
  - `WorkspaceError` enum: `NotFound(PathBuf)`, `NotADirectory(PathBuf)`, `PermissionDenied(PathBuf)`, `Io(std::io::Error)`
- **Dependencies:** TASK-1.1 (duga-types must compile)
- **Implementation steps:**
  1. Create `crates/duga-sandbox/Cargo.toml` with deps: `duga-types`, `cap_std`, `tokio`, `which`, `tracing`
  2. Create `workspace.rs`: define `Workspace` struct
  3. Implement `Workspace::open` — validate path exists and is directory, then `cap_std::fs::Dir::open_ambient_dir(root, cap_std::ambient_authority())`
  4. Implement error variants with `thiserror`
  5. Create integration test: open a temp dir, verify root_dir() works
- **Edge cases:**
  - Root path is a symlink: `cap_std` resolves it; this is acceptable (symlink to workspace dir)
  - Root path doesn't exist → `WorkspaceError::NotFound`
  - Root path is a file → `WorkspaceError::NotADirectory`
  - MacOS: `cap_std` uses `O_NOFOLLOW` + manual checks (GAP G16) — test on macOS CI
- **Definition of Done:**
  - `cargo build -p duga-sandbox` succeeds
  - `cargo test -p duga-sandbox` passes
  - `cargo clippy` clean
- **Acceptance criteria:**
  - `Workspace::open("/tmp/duga-test")` succeeds and returns Workspace with correct root_path
  - `Workspace::open("/nonexistent/path")` → `Err(WorkspaceError::NotFound)`
  - Attempting `Dir::open("/etc/passwd")` through the workspace Dir fails (cap_std rejects absolute paths)
- **Test plan:**
  - unit: Test Workspace::open success and failure cases with temp dirs
  - integration: none (cap_std behavior is assumed correct; test at OS level on CI)
- **Estimated effort:** 4 hours

---

### TASK-2.2: Workspace::resolve path validation

- **§SPEC:** §14 (TOCTOU-Safe File Access)
- **Labels:** `layer/security`, `priority/critical`
- **Description:** Implement `Workspace::resolve(&self, relative_path: &Path) -> Result<PathBuf, WorkspaceError>` that takes a workspace-relative path string, validates it doesn't contain `..` components or absolute prefixes, and returns a path the workspace Dir can use. Reject paths with `..` segments, Windows drive letters, and any component that would escape the workspace. This is the single choke-point for all path access.
- **Files affected:**
  - `crates/duga-sandbox/src/workspace.rs` (append methods)
- **Types involved:** `Workspace`, `WorkspaceError`
- **Functions to implement:**
  - `Workspace::resolve(&self, path: &Path) -> Result<PathBuf, WorkspaceError>`
  - `WorkspaceError::PathEscapesWorkspace(PathBuf)` (new variant)
- **Dependencies:** TASK-2.1
- **Implementation steps:**
  1. Implement `resolve()`: check if `path.is_absolute()` → error
  2. Iterate through path components: if any component is `..` → error
  3. Check for empty path → return root (".")
  4. Sanitize: strip leading `/` or `./` if present, normalize
  5. Return cleaned relative path
  6. Test with boundary cases
- **Edge cases:**
  - `"."` → valid (workspace root)
  - `"./foo"` → valid → `"foo"`
  - `"foo/../../bar"` → rejected (contains `..`)
  - `"/etc/passwd"` → rejected (absolute)
  - `"C:\\Windows"` on Windows → rejected (absolute drive letter)
  - `"foo//bar"` → normalize to `"foo/bar"` (empty components are not `..`)
  - `"foo/./bar"` → normalize to `"foo/bar"` (`.` is harmless)
- **Definition of Done:**
  - All edge cases tested
  - `cargo clippy` clean
- **Acceptance criteria:**
  - `resolve(Path::new("src/main.rs"))` → `Ok(PathBuf::from("src/main.rs"))`
  - `resolve(Path::new("../etc/passwd"))` → `Err(WorkspaceError::PathEscapesWorkspace(_))`
  - `resolve(Path::new("/etc/passwd"))` → `Err(WorkspaceError::PathEscapesWorkspace(_))`
  - `resolve(Path::new("."))` → `Ok(PathBuf::from("."))`
- **Test plan:**
  - unit: Test all edge cases above; test with nested valid paths; test with Windows-style paths on all platforms (should reject)
- **Estimated effort:** 4 hours

---

### TASK-2.3: BinaryRegistry + resolve at startup

- **§SPEC:** §16 (Binary Resolution)
- **Labels:** `layer/security`, `priority/critical`
- **Description:** Implement `BinaryRegistry` that resolves allowed binary names to absolute paths at construction time. Use `which::which()` to locate each binary, then `std::fs::canonicalize()` to resolve symlinks. The resolved path is cached in a `HashMap<String, PathBuf>`. Implement `BinaryRegistry::resolve(&self, name: &str) -> Result<&Path, BinaryError>` for runtime lookup. Resolution is done once at startup; symlink changes are not picked up until restart.
- **Files affected:**
  - `crates/duga-sandbox/src/binary_registry.rs` (new)
  - `crates/duga-sandbox/src/lib.rs` (add module)
  - `crates/duga-sandbox/src/error.rs` (add `BinaryError`)
- **Types involved:** `BinaryRegistry`, `BinaryError`
- **Functions to implement:**
  - `BinaryRegistry::new(allowed: HashSet<String>) -> Result<Self, BinaryError>`
  - `BinaryRegistry::resolve(&self, name: &str) -> Result<&Path, BinaryError>`
  - `BinaryRegistry::is_allowed(&self, name: &str) -> bool`
  - `BinaryError` enum: `NotFound(String)`, `NotAllowed(String)`, `WhichFailed(String, std::io::Error)`
- **Dependencies:** TASK-1.1 (duga-types)
- **Implementation steps:**
  1. Define `BinaryRegistry` with `allowed: HashMap<String, PathBuf>` (name → resolved absolute path)
  2. Implement `new()`: for each name in `allowed`, call `which::which(name)` → `canonicalize()` → store
  3. If `which` fails → `BinaryError::WhichFailed`
  4. Implement `resolve()`: lookup in HashMap → `NotAllowed` if missing
  5. Implement `is_allowed()`: check HashMap contains key
  6. Add unit tests with binaries that exist on the system (`echo`, `ls`, `true`)
- **Edge cases:**
  - Binary name contains `/` → reject (prevent `./malicious` or `../../bin/sh`)
  - Empty name → reject
  - Symlink chain: `canonicalize` resolves entire chain → one absolute path stored
  - Binary with spaces in path → which finds it; Command::new handles it
- **Definition of Done:**
  - `cargo build` succeeds
  - `cargo test` passes
  - `cargo clippy` clean
- **Acceptance criteria:**
  - `BinaryRegistry::new(hashset!["echo".into()])` resolves `echo` → `/usr/bin/echo` (or `/bin/echo`)
  - `registry.resolve("nonexistent")` → `Err(BinaryError::NotAllowed("nonexistent"))`
  - `registry.resolve("/bin/sh")` → rejected in constructor (name check)
- **Test plan:**
  - unit: Test with system binaries (`echo`, `true`, `false`), test unknown binary returns NotAllowed, test which failure handling with `nonexistent_binary_xyz`
- **Estimated effort:** 4 hours

---

### TASK-2.4: run_captured + OutputLimits enforcement

- **§SPEC:** §17 (Safe Process Execution), §18 (Output Limits)
- **Labels:** `layer/security`, `priority/critical`
- **Description:** Implement `run_captured()` — the single function that spawns a process via `tokio::process::Command`, captures stdout/stderr with byte limits enforced per `OutputLimits`, applies a per-process timeout from `Sandbox.timeout`, and returns a `ToolResult`. The process receives only the sanitized environment and the current working directory from the shell session (if any). This is the ONLY place where processes are spawned.
- **Files affected:**
  - `crates/duga-sandbox/src/exec.rs` (new)
  - `crates/duga-sandbox/src/lib.rs` (add module)
- **Types involved:** `ToolResult`, `ToolError`, `OutputLimits`, `Workspace`, `CancellationToken`
- **Functions to implement:**
  - `pub async fn run_captured(binary: &Path, args: &[String], workspace: &Workspace, cwd: &Path, env: &HashMap<String, String>, limits: &OutputLimits, timeout: Duration, cancel: CancellationToken) -> Result<ToolResult, ToolError>`
- **Dependencies:** TASK-2.1 (Workspace), TASK-1.4 (ToolResult), TASK-1.5 (ToolError), TASK-1.7 (OutputLimits)
- **Implementation steps:**
  1. Build `tokio::process::Command` from `binary` + `args` — no shell, ever
  2. Set `current_dir(workspace.root_path().join(cwd))`
  3. Set all env vars from `env` HashMap
  4. Spawn with `stdout(Stdio::piped())` + `stderr(Stdio::piped())`
  5. `tokio::select!` between process wait + cancellation
  6. Read stdout with `.take(max_stdout_bytes)`, same for stderr
  7. After both complete, check combined bytes against `max_combined_bytes`
  8. Build ToolResult with `success`, `output`, `stdout_bytes`, `stderr_bytes`, `truncated`, `duration_ms`
  9. If timeout → `ToolError::Timeout`; if cancelled → `ToolError::Cancelled`; if exit != 0 → still success=true but output includes stderr
- **Edge cases:**
  - Process writes exactly at the byte limit boundary → truncated flag set
  - Process exits with non-zero code but within limits → `success: true`, output contains exit code note
  - Stderr empty but stdout truncated → truncated flag based on stdout only
  - Cancellation during process spawn (between Command::new and spawn) → check cancel before spawning
  - Timeout fires after process already exited → tokio::time::timeout returns Ok if process finished
- **Definition of Done:**
  - `cargo build` succeeds
  - `cargo test` passes
  - `cargo clippy` clean
- **Acceptance criteria:**
  - `run_captured("echo", ["hello"], ...)` → `ToolResult { success: true, output: "hello\n", ... }`
  - `run_captured("cat", ["/dev/zero"], limits_with_1KB_cap, ...)` → `ToolResult { truncated: true, stdout_bytes: 1024, ... }`
  - `run_captured("sleep", ["999"], timeout=1s, ...)` → `Err(ToolError::Timeout)`
  - Cancelled token → `Err(ToolError::Cancelled)`
- **Test plan:**
  - unit: Test with `echo`, `true`, `false` commands; test output truncation with `head -c` or `dd`; test timeout with `sleep`; test cancellation via token
  - integration: Test with a binary from BinaryRegistry (TASK-2.3)
- **Estimated effort:** 8 hours

---

### TASK-2.5: SanitizedEnv + PATH override

- **§SPEC:** §20 (Environment Variable Sanitization)
- **Labels:** `layer/security`, `priority/critical`
- **Description:** Implement `SanitizedEnv` that builds the subprocess environment from an allowlist. Takes the host environment, filters to only allowed variable names, sets `PATH` to `/usr/bin:/bin` (never inherited), and warns on secret-pattern variables (`*_TOKEN`, `*_KEY`, `*_SECRET`, `*_PASSWORD`) that appear in the allowlist. Returns a `HashMap<String, String>` suitable for `Command::envs()`.
- **Files affected:**
  - `crates/duga-sandbox/src/env.rs` (new)
  - `crates/duga-sandbox/src/lib.rs` (add module)
- **Types involved:** `SanitizedEnv`
- **Functions to implement:**
  - `SanitizedEnv::build(allowed: &HashSet<String>, warnings: &mut Vec<String>) -> HashMap<String, String>`
  - `SanitizedEnv::is_secret_pattern(name: &str) -> bool` (static, used internally and by EPIC-7 redaction)
  - `SanitizedEnv::PATH` constant = `/usr/bin:/bin`
- **Dependencies:** TASK-1.1 (duga-types)
- **Implementation steps:**
  1. Define `SECRET_PATTERNS: &[&str]` = `["*_TOKEN", "*_KEY", "*_SECRET", "*_PASSWORD"]`
  2. Implement `is_secret_pattern()`: match name against patterns using `ends_with` after uppercasing
  3. Implement `build()`: iterate `std::env::vars()`, filter by `allowed`, skip if not allowed
  4. Override `PATH` with fixed value
  5. For allowed vars matching secret patterns → push warning string
  6. Return HashMap
  7. Add unit tests
- **Edge cases:**
  - `HOME` not set → skip, don't error (some CI environments)
  - `allowed` contains `PATH` → ignore it, always override with fixed PATH
  - Secret pattern `GITHUB_TOKEN` in `allowed` → included in env but warning emitted
  - Case sensitivity: env vars on Windows are case-insensitive; on Unix they're case-sensitive. Match case-sensitively on Unix; test on CI.
- **Definition of Done:**
  - `cargo build` succeeds
  - `cargo test` passes
  - `cargo clippy` clean
- **Acceptance criteria:**
  - `SanitizedEnv::build({"HOME".into()}, &mut warnings)` includes HOME from host
  - `PATH` key is always `/usr/bin:/bin`
  - `GITHUB_TOKEN` not in allowed → not in output
  - `GITHUB_TOKEN` in allowed → in output, warning emitted
  - `is_secret_pattern("AWS_SECRET_ACCESS_KEY")` → `true`
  - `is_secret_pattern("HOME")` → `false`
- **Test plan:**
  - unit: Mock `std::env::vars` or set test env vars; test filtering, test PATH override, test secret warnings, test is_secret_pattern for all pattern classes
- **Estimated effort:** 4 hours

---

### TASK-2.6: ShellSession struct + SessionCommand enum

- **§SPEC:** §19 (Persistent Shell Sessions)
- **Labels:** `layer/security`, `priority/critical`
- **Description:** Define `ShellSession` struct with `id: Uuid`, `cwd: PathBuf` (workspace-relative), `env: HashMap<String, String>`. Define `SessionCommand` enum with variants `Cd(PathBuf)`, `Export(String, String)`, `Unset(String)`, `Pwd`, `Spawn(Vec<String>)` (for commands that actually execute). No persistent shell process exists — state is held in Rust and applied fresh for each tool call.
- **Files affected:**
  - `crates/duga-sandbox/src/shell_session.rs` (new)
  - `crates/duga-sandbox/src/lib.rs` (add module)
- **Types involved:** `ShellSession`, `SessionCommand`
- **Functions to implement:**
  - `ShellSession::new(cwd: PathBuf) -> Self` (GAP G11: explicit constructor, not implicit)
  - `ShellSession::id(&self) -> Uuid`
  - `ShellSession::cwd(&self) -> &Path`
  - `ShellSession::env(&self) -> &HashMap<String, String>`
- **Dependencies:** TASK-1.1 (duga-types for Uuid)
- **Implementation steps:**
  1. Define `ShellSession` struct with `id`, `cwd`, `env`
  2. Define `SessionCommand` enum with all 5 variants
  3. Implement `ShellSession::new()` — generates fresh Uuid
  4. Implement accessor methods
  5. Add unit tests
- **Edge cases:**
  - Session lifecycle managed by ShellTool via `Mutex<HashMap<Uuid, ShellSession>>` — not implemented here, just the struct
  - `cwd` is workspace-relative (`"."` initially)
  - `env` starts empty — no host env leakage; TASK-2.5 env applies at execution time
  - GAP G11: resolved as explicit constructor — session created when ShellTool receives a new `session` UUID not in its map
- **Definition of Done:**
  - Struct and enum compile
  - `cargo clippy` clean
- **Acceptance criteria:**
  - `ShellSession::new(PathBuf::from("."))` creates session with id, cwd=".", env={}
  - `SessionCommand::Cd(PathBuf::from("src"))` matches correctly
- **Test plan:**
  - unit: Test constructors, test accessors, test SessionCommand enum variants
- **Estimated effort:** 2 hours

---

### TASK-2.7: ShellSession::classify + apply mutations

- **§SPEC:** §19.2 (Supported state changes)
- **Labels:** `layer/security`, `priority/critical`
- **Description:** Implement `ShellSession::classify(command: &[String]) -> SessionCommand` — a pure function that inspects the argv vector and classifies it. `["cd", "/tmp"]` → `Cd`, `["export", "FOO=bar"]` → `Export`, `["unset", "FOO"]` → `Unset`, `["pwd"]` → `Pwd`, anything else → `Spawn`. Implement `apply(&mut self, cmd: SessionCommand) -> Option<String>` — mutates the session for Cd/Export/Unset, returns output string for Pwd, returns None for Spawn (caller executes). Export values are subject to SanitizedEnv checks (TASK-2.5).
- **Files affected:**
  - `crates/duga-sandbox/src/shell_session.rs` (append methods)
- **Types involved:** `ShellSession`, `SessionCommand`
- **Functions to implement:**
  - `ShellSession::classify(command: &[String]) -> SessionCommand`
  - `ShellSession::apply(&mut self, cmd: SessionCommand, workspace: &Workspace) -> Result<Option<String>, ShellSessionError>`
- **Dependencies:** TASK-2.6, TASK-2.1 (Workspace for cwd resolution)
- **Implementation steps:**
  1. Implement `classify()`: first element determines variant
      - `"cd"` → parse second arg as `PathBuf` → `Cd`
      - `"export"` → parse `KEY=value` split → `Export`
      - `"unset"` → second arg is key → `Unset`
      - `"pwd"` → `Pwd`
      - else → `Spawn` (return original command)
  2. Implement `apply()`:
      - `Cd(path)` → validate via `workspace.resolve(path)`, update `self.cwd`
      - `Export(k, v)` → update `self.env`
      - `Unset(k)` → remove from `self.env`
      - `Pwd` → return `Some(self.cwd.display().to_string())`
      - `Spawn(_)` → return `None`
  3. `Cd` with invalid path → `Err(ShellSessionError::InvalidCwd)`
  4. `Export` with secret-pattern key → warn via tracing, still allow
  5. Add unit tests covering all 5 command types
- **Edge cases:**
  - `cd` with no argument → treat as `cd $HOME`? No — spec says `cd <path>`. Missing argument → error.
  - `cd ..` → resolved through Workspace::resolve → rejected (contains `..`)
  - `export KEY` with no `=` → error
  - `export KEY=` → set to empty string
  - `unset` with no argument → error
  - `pwd` with extra args → ignore extras, still return cwd
  - `cd` within workspace → normalized by resolve
- **Definition of Done:**
  - `cargo build` succeeds
  - `cargo test` passes
  - `cargo clippy` clean
- **Acceptance criteria:**
  - `classify(&["cd", "src"])` → `SessionCommand::Cd(PathBuf::from("src"))`
  - `classify(&["export", "FOO=bar"])` → `SessionCommand::Export("FOO".into(), "bar".into())`
  - `classify(&["unset", "FOO"])` → `SessionCommand::Unset("FOO".into())`
  - `classify(&["pwd"])` → `SessionCommand::Pwd`
  - `classify(&["cargo", "test"])` → `SessionCommand::Spawn(vec!["cargo".into(), "test".into()])`
  - `apply(Cd("src"), ws)` updates cwd to "src"
  - `apply(Pwd, ws)` returns `Some("src")`
  - `apply(Spawn(...), ws)` returns `None`
- **Test plan:**
  - unit: Test classify for all 5 command types, test apply for all 5 (Cd updates cwd, Export updates env, Unset removes, Pwd returns cwd, Spawn returns None), test edge cases (missing args, invalid cd path, empty export value)
- **Estimated effort:** 6 hours
