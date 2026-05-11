# EPIC-5: Execution Engine

**§SPEC:** §17, §19–20  
**Labels:** `epic/execution`  
**Crate:** `duga-sandbox` (extends existing)

## Goal
Refine the process execution subsystem: exit code handling, combined output formatting, timeout + cancellation interaction, and execution engine integration tests.

---

### TASK-5.1: Process exit code handling + output formatting in run_captured

- **§SPEC:** §17 (Safe Process Execution), §18 (Output Limits)
- **Labels:** `layer/sandbox`, `priority/critical`
- **Description:** Enhance `run_captured()` to properly handle process exit codes. A non-zero exit code does NOT mean `ToolError` — the process still succeeded as a tool. Format the `ToolResult.output` to interleave stdout and stderr with clear labels when stderr is non-empty. Set `ToolResult.success = true` even for non-zero exit (the tool executed, the LLM should see the exit code in the output). Only platform errors (spawn failure) result in `ToolResult.success = false`. Include exit code in `ToolResult.metadata`.
- **Files affected:**
  - `crates/duga-sandbox/src/exec.rs` (enhance run_captured)
- **Types involved:** `ToolResult`, `ToolError`, `OutputLimits`
- **Functions to implement:**
  - `fn format_output(stdout: &str, stderr: &str, exit_code: Option<i32>) -> String`
  - Enhanced `run_captured()` exit handling
- **Dependencies:** TASK-2.4
- **Implementation steps:**
  1. After process completes, get `output.status.code()` (may be None if killed by signal)
  2. Build `format_output()`:
      - If stderr empty: return `stdout`
      - If stderr non-empty: `format!("STDOUT:\n{}\nSTDERR:\n{}\nEXIT: {}", stdout, stderr, code)`
  3. Set `metadata`: `json!({"exit_code": code})`
  4. Set `success: true` always (tool errors like Timeout/Cancelled are caught before this point)
  5. Add test
- **Edge cases:**
  - Process killed by signal → `code = None` → format as "SIGNAL: {signal}"
  - stdout empty, stderr non-empty → still show both
  - Both stdout and stderr empty → output is empty string
- **Definition of Done:**
  - `cargo build` succeeds
  - `cargo test` passes
  - `cargo clippy` clean
- **Acceptance criteria:**
  - `run_captured("false", [])` → `ToolResult { success: true, output: "...EXIT: 1", metadata: {"exit_code": 1} }`
  - `run_captured("sh", ["-c", "echo out; echo err >&2"])` → output contains both "STDOUT" and "STDERR"
  - `run_captured("sh", ["-c", "echo out"])` → output is just "out\n", no STDERR section
- **Test plan:**
  - unit: Test exit 0, exit 1, signal kill, stdout only, stderr only, both
- **Estimated effort:** 4 hours

---

### TASK-5.2: Process timeout + cancellation interaction

- **§SPEC:** §26 (Cancellation), §15 (Sandbox timeout)
- **Labels:** `layer/sandbox`, `priority/critical`
- **Description:** Harden the interaction between per-process timeout and runtime cancellation in `run_captured()`. The current implementation uses `tokio::select!` between process wait and cancellation. Add `tokio::time::timeout` as a third branch. When timeout fires first → kill process → return `ToolError::Timeout`. When cancellation fires first → kill process → return `ToolError::Cancelled`. Ensure the process is always killed (not leaked) regardless of which branch wins.
- **Files affected:**
  - `crates/duga-sandbox/src/exec.rs` (enhance cancellation logic)
- **Types involved:** `ToolError::Timeout`, `ToolError::Cancelled`, `CancellationToken`
- **Functions to implement:** (enhanced `run_captured`)
- **Dependencies:** TASK-5.1
- **Implementation steps:**
  1. Add `tokio::time::timeout(timeout, child.wait())` as the process wait future
  2. Use `tokio::select!` with 3 branches: timeout future, cancellation, process wait
  3. On any branch: if process still running, `child.kill().await` + `child.wait().await`
  4. Determine final error: timeout → `ToolError::Timeout`, cancel → `ToolError::Cancelled`, process exit → continue
  5. Add test for race between timeout and cancellation
- **Edge cases:**
  - Timeout and cancellation fire simultaneously → cancellation wins (biased select order)
  - Process exits exactly at timeout boundary → process exit wins (Ok from timeout returns process result)
  - `child.kill()` fails (process already exited) → ignore, continue to wait
- **Definition of Done:**
  - Timeout + cancellation both work
  - Process never leaks
  - `cargo test` passes
- **Acceptance criteria:**
  - `run_captured("sleep", ["999"], timeout=100ms)` → returns `Err(ToolError::Timeout)` within ~150ms
  - Cancel token + running process → returns `Err(ToolError::Cancelled)`
  - Both timeout and cancel active → cancel wins (deterministic ordering)
- **Test plan:**
  - unit: Test timeout, test cancellation, test race condition (cancel token triggered while timeout approaching)
- **Estimated effort:** 4 hours

---

### TASK-5.3: OutputLimits truncation format + message

- **§SPEC:** §18 (Output Limits — truncation message)
- **Labels:** `layer/sandbox`, `priority/normal`
- **Description:** Ensure OutputLimits truncation in `run_captured()` produces a clear, machine-readable truncation message. Format: `"Tool output truncated:\nstdout exceeded {max} byte limit"` or `"stderr exceeded {max} byte limit"` or `"combined output exceeded {max} byte limit"`. Set `ToolResult.truncated = true` in all cases. The truncated output in the result should include the truncation notice appended.
- **Files affected:**
  - `crates/duga-sandbox/src/exec.rs` (enhance truncation)
- **Types involved:** `OutputLimits`, `ToolResult`
- **Functions to implement:**
  - `fn truncation_notice(limit_type: &str, max: usize) -> String`
- **Dependencies:** TASK-5.1
- **Implementation steps:**
  1. After reading stdout/stderr (with `.take(max_bytes)`), check if the  `take()` adapter reached limit
  2. Track which limit was exceeded: stdout, stderr, or combined
  3. Format truncation notice
  4. Append notice to output
  5. Set `truncated = true`
  6. Add test
- **Edge cases:**
  - Multiple limits exceeded (stdout AND combined) → report each with separate notices
  - Output exactly at limit (not exceeded) → no truncation notice
  - truncation notice itself pushes output over limit → accept (it's a small string)
- **Definition of Done:**
  - Truncation messages are clear and consistent
  - `cargo test` passes
- **Acceptance criteria:**
  - stdout truncated → output ends with "...stdout exceeded 4MB limit"
  - combined truncated → output ends with "...combined output exceeded 6MB limit"
  - Not truncated → `truncated = false`, no notice
- **Test plan:**
  - unit: Test with small limits (1KB stdout, 2KB combined), generate known-size output
- **Estimated effort:** 3 hours

---

### TASK-5.4: Directory creation helper + workspace path utilities

- **§SPEC:** §13 (Workspace operations), §11 (write tool parent dir creation)
- **Labels:** `layer/sandbox`, `priority/normal`
- **Description:** Add utility methods to `Workspace` for common operations: `create_dir_all(path)`, `remove_file(path)`, `exists(path)`, `is_file(path)`, `is_dir(path)`. These wrap cap_std equivalents and apply `resolve()` internally. Used by `WriteTool` (parent dir creation), `SearchTool` (path existence check), and future tools.
- **Files affected:**
  - `crates/duga-sandbox/src/workspace.rs` (add methods)
- **Types involved:** `Workspace`, `WorkspaceError`
- **Functions to implement:**
  - `Workspace::create_dir_all(&self, path: &Path) -> Result<(), WorkspaceError>`
  - `Workspace::remove_file(&self, path: &Path) -> Result<(), WorkspaceError>`
  - `Workspace::exists(&self, path: &Path) -> bool`
  - `Workspace::is_file(&self, path: &Path) -> bool`
  - `Workspace::is_dir(&self, path: &Path) -> bool`
- **Dependencies:** TASK-2.1, TASK-2.2
- **Implementation steps:**
  1. Each method: `resolve(path)` → delegate to `self.root_dir` equivalent
  2. `create_dir_all`: use `dir.create_dir_all(resolved)`
  3. `remove_file`: use `dir.remove_file(resolved)`
  4. `exists`: `dir.metadata(resolved).is_ok()`
  5. `is_file`/`is_dir`: check metadata
  6. Add tests
- **Edge cases:**
  - `create_dir_all(".")` → no-op (dir exists)
  - `remove_file` on directory → error (use `remove_dir_all` if needed later)
  - Path escapes workspace → rejected by resolve
  - Symlinks: cap_std follows workspace symlinks if configured (GAP G9)
- **Definition of Done:**
  - All methods work
  - `cargo test` passes
  - `cargo clippy` clean
- **Acceptance criteria:**
  - `create_dir_all("a/b/c")` → creates all three dirs
  - `exists("a/b/c")` → true after creation
  - `is_dir("a/b/c")` → true
  - `remove_file` + `exists` → false after removal
  - Path with `..` → error
- **Test plan:**
  - unit: Test each method with temp workspace
- **Estimated effort:** 4 hours

---

### TASK-5.5: Execution engine integration tests

- **§SPEC:** §17–20 (execution subsystem)
- **Labels:** `layer/sandbox`, `priority/high`
- **Description:** Create integration tests for the execution engine combining `run_captured`, `BinaryRegistry`, `SanitizedEnv`, `ShellSession`, and `OutputLimits`. Test realistic scenarios: (1) execute allowed binary with session cwd + env, (2) non-zero exit with stderr, (3) timeout + truncation, (4) cancellation mid-execution, (5) denied binary, (6) env sanitization.
- **Files affected:**
  - `crates/duga-sandbox/tests/integration.rs` (new)
  - `crates/duga-sandbox/Cargo.toml` (add dev-deps)
- **Types involved:** All sandbox types
- **Dependencies:** TASK-5.1 through TASK-5.4, TASK-2.3, TASK-2.5, TASK-2.7
- **Implementation steps:**
  1. Set up test harness with temp workspace, BinaryRegistry, SanitizedEnv
  2. Test scenario 1: `run_captured("echo", ["hello"], cwd=".", env={HOME...})` → success + output
  3. Test scenario 2: `run_captured("false", [])` → success=false, exit_code=1
  4. Test scenario 3: `run_captured("yes", [], limits=1KB)` → truncated
  5. Test scenario 4: cancel token during `sleep 10` → Cancelled
  6. Test scenario 5: BinaryRegistry.resolve("nonexistent") → Denied
  7. Test scenario 6: env with GITHUB_TOKEN → not inherited
- **Edge cases:**
  - Tests assume POSIX shell utilities available — mark as `#[cfg(unix)]` where needed
  - Windows: use PowerShell cmdlets or skip these tests on Windows
- **Definition of Done:**
  - All integration tests pass on Linux
  - Tests properly skipped on Windows where POSIX-specific
- **Acceptance criteria:** Each scenario above passes
- **Test plan:** integration tests as described
- **Estimated effort:** 5 hours
