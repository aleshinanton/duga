# EPIC-4: Built-in Tools

**§SPEC:** §11–12, §16–17  
**Labels:** `epic/builtin-tools`  
**Crate:** `duga-tools-builtin`

## Goal
Implement all five built-in tools: `read`, `write`, `bash`, `search`, `think`. Each implements the `Tool` trait and uses `Workspace`, `BinaryRegistry`, and `ShellSession` from the security layer.

---

### TASK-4.1: ReadTool — cap_std read, offset, limit

- **§SPEC:** §11 (read tool)
- **Labels:** `layer/tools-builtin`, `priority/critical`
- **Description:** Implement `ReadTool` that implements `Tool` with `Args = ReadArgs { path: String, offset: Option<u64>, limit: Option<u64> }`. Uses `workspace.root_dir().open(path)` via cap_std. Supports byte offset (default 0) and limit (default 256 KiB, hard cap 4 MiB). Returns file content as `output` string. Non-existent file → `ToolError::Io`. Binary file detection is in TASK-4.2 — for now, always return text.
- **Files affected:**
  - `crates/duga-tools-builtin/Cargo.toml` (new)
  - `crates/duga-tools-builtin/src/lib.rs` (new)
  - `crates/duga-tools-builtin/src/read.rs` (new)
- **Types involved:** `ReadArgs`, `ReadTool`, `Tool`, `ToolResult`, `ToolError`, `Workspace`
- **Functions to implement:**
  - `ReadArgs` struct with `JsonSchema` derive
  - `ReadTool` struct with no fields (stateless — uses ToolContext)
  - `impl Tool for ReadTool { type Args = ReadArgs; fn execute(&self, ctx, args) -> ... }`
  - Inside execute: `workspace.resolve(args.path)` → open file → seek to offset → read up to limit → return `ToolResult`
- **Dependencies:** TASK-3.1 (Tool trait), TASK-3.5 (ErasedTool for registration), TASK-2.1 (Workspace), TASK-2.2 (Workspace::resolve)
- **Implementation steps:**
  1. Create `duga-tools-builtin` crate with deps: `duga-tools`, `duga-sandbox`, `duga-types`, `tempfile` (for tests)
  2. Define `ReadArgs` with all fields
  3. Implement `ReadTool` struct
  4. Implement `Tool for ReadTool`: `name()` → `"read"`, `description()` → `"Read a file from the workspace..."`
  5. Implement `execute()`:
      - Resolve path via `ctx.workspace.resolve(&PathBuf::from(&args.path))`
      - Open file: `ctx.workspace.root_dir().open(resolved_path)`
      - For offset: seek to `args.offset.unwrap_or(0)`
      - For limit: `args.limit.unwrap_or(256 * 1024).min(4 * 1024 * 1024)`
      - Read into String (valid UTF-8 check)
      - Return `ToolResult { output: content, ... }`
  6. Handle errors: path not found → `ToolError::Io`, permission → `ToolError::Io`, non-UTF-8 → handle in TASK-4.2
- **Edge cases:**
  - offset > file length → read returns empty string
  - limit 0 → read 0 bytes
  - File is empty → return empty string
  - File > 4 MiB with default limit → truncate at 256 KiB, not 4 MiB (hard cap is upper bound on `limit` argument)
  - Path is a directory → error
- **Definition of Done:**
  - `cargo build` succeeds
  - `cargo test` passes
  - `cargo clippy` clean
- **Acceptance criteria:**
  - `read { path: "test.txt" }` returns file contents
  - `read { path: "test.txt", offset: 5 }` skips first 5 bytes
  - `read { path: "test.txt", limit: 10 }` returns at most 10 bytes
  - `read { path: "nonexistent" }` → `Err(ToolError::Io(_))`
  - `read { path: "../outside" }` → `Err(ToolError::Io(_))` (Workspace::resolve rejects it)
- **Test plan:**
  - unit: Create temp workspace with test files; test read full, read with offset, read with limit, read nonexistent, read empty file, read with offset beyond length, read directory
  - integration: none (uses real cap_std)
- **Estimated effort:** 6 hours

---

### TASK-4.2: ReadTool — binary detection via NUL-byte sniffing

- **§SPEC:** §11 (read tool — binary detection)
- **Labels:** `layer/tools-builtin`, `priority/critical`
- **Description:** Add binary file detection to `ReadTool::execute()`. Read the first 8 KiB of the file into a byte buffer. If any byte is `0x00` (NUL), treat as binary: return a hex/length summary instead of raw content. Format: `"<binary file: {len} bytes, first 64 bytes hex: {hex_dump}>"`.
- **Files affected:**
  - `crates/duga-tools-builtin/src/read.rs` (modify execute)
- **Types involved:** `ReadTool`
- **Functions to implement:**
  - `fn is_binary(bytes: &[u8]) -> bool` — true if any byte is 0x00
  - `fn hex_dump(bytes: &[u8], max_len: usize) -> String` — format as hex pairs
  - `fn binary_summary(len: u64, hex: &str) -> String` — format the summary
- **Dependencies:** TASK-4.1
- **Implementation steps:**
  1. Read first 8192 bytes into `Vec<u8>` instead of `String`
  2. Check `is_binary(&bytes)`
  3. If binary: truncate to 64 bytes for hex dump, format summary, return
  4. If not binary: convert to `String::from_utf8_lossy()`, apply offset+limit as before
  5. Update tests to include binary file scenario
- **Edge cases:**
  - File < 8 KiB: check entire file
  - File < 64 bytes: hex dump all bytes
  - File with NUL at position 8193+ → not detected → treated as text (this is the spec: "first 8 KiB")
  - Valid UTF-8 with NUL (e.g., binary serialization) → treated as binary (spec says NUL-byte sniffing)
  - File is valid UTF-8 with no NUL → treated as text, even if not human-readable
- **Definition of Done:**
  - Binary file detection works
  - `cargo clippy` clean
- **Acceptance criteria:**
  - File containing NUL byte → `output` starts with `"<binary file: "`
  - File without NUL byte → text content returned normally
  - Binary file summary includes total byte count
  - Binary file summary includes hex dump of first 64 bytes (max)
- **Test plan:**
  - unit: Create binary file with NUL at position 0, mid, end; test detection at each; test file with no NUL; test 1-byte binary file
- **Estimated effort:** 3 hours

---

### TASK-4.3: WriteTool — atomic write via tempfile + rename

- **§SPEC:** §11 (write tool)
- **Labels:** `layer/tools-builtin`, `priority/critical`
- **Description:** Implement `WriteTool` with `Args = WriteArgs { path: String, content: String }`. Writes are atomic: content is staged to a temp file in the same workspace directory, then renamed over the target. Uses `tempfile::NamedTempFile::new_in()` for staging. If the parent directory doesn't exist, create it first. Returns `ToolResult { output: "Wrote {n} bytes to {path}" }`.
- **Files affected:**
  - `crates/duga-tools-builtin/src/write.rs` (new)
  - `crates/duga-tools-builtin/src/lib.rs` (add module)
- **Types involved:** `WriteArgs`, `WriteTool`, `Tool`, `ToolResult`, `ToolError`, `Workspace`
- **Functions to implement:**
  - `WriteArgs` struct with `JsonSchema` derive
  - `WriteTool` struct
  - `impl Tool for WriteTool { ... }`
  - Inside execute: `workspace.resolve(path)` → create parent dirs → write temp file → rename
- **Dependencies:** TASK-3.1 (Tool trait), TASK-3.5 (ErasedTool), TASK-2.1 (Workspace), TASK-2.2 (resolve)
- **Implementation steps:**
  1. Implement `WriteTool` struct
  2. `name()` → `"write"`, `description()` → `"Write content to a file"`
  3. `execute()`:
      - Resolve path via workspace
      - Ensure parent directory exists (use `workspace.root_dir().create_dir_all()` — cap_std equivalent)
      - Stage to temp file in parent dir: `tempfile::NamedTempFile::new_in(parent_dir)` or write to workspace with `.tmp` suffix
      - Write `args.content.as_bytes()` to temp file
      - Flush + sync
      - Rename temp file to target path (in-workspace `std::fs::rename` is safe since both are within workspace)
      - Return success ToolResult
  4. Handle errors: path escapes workspace → `ToolError::Denied`, IO error → `ToolError::Io`
- **Edge cases:**
  - Content is empty string → write empty file (valid)
  - Content contains NUL bytes → write as-is (it's text from the LLM perspective, but may contain binary)
  - File already exists → overwrite (rename replaces)
  - Parent directory path is file → error
  - Target path is a directory → error
  - Rename fails (cross-device) → fallback to copy + delete temp
  - VERY large content (10MB+) → write succeeds but may be slow; OutputLimits don't apply here (they apply to process output, not file writes)
- **Definition of Done:**
  - Atomic write works
  - `cargo test` passes
  - `cargo clippy` clean
- **Acceptance criteria:**
  - `write { path: "out.txt", content: "hello" }` → file exists with "hello"
  - Crash mid-write (simulated) → target file unaffected (temp file may exist, but not target)
  - Write to `"newdir/out.txt"` → parent dir created if missing
  - Write to `"../escape"` → `Err(ToolError::Denied)`
- **Test plan:**
  - unit: Test basic write, overwrite, empty content, nested path creation, escaped path rejection
- **Estimated effort:** 5 hours

---

### TASK-4.4: WriteTool — per-path mutex serialization

- **§SPEC:** §11 (write tool — per-path mutex)
- **Labels:** `layer/tools-builtin`, `priority/critical`
- **Description:** Add serialization of concurrent writes to the same path. `WriteTool` holds a `Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>` — a map from workspace-relative paths to per-path locks. Before writing, acquire the lock for that path. After writing (success or failure), release the lock. This prevents two concurrent write tool calls from interleaving/racing on the same file. GAP G8: the mutex key is the workspace-relative canonicalized path (the output of `Workspace::resolve`).
- **Files affected:**
  - `crates/duga-tools-builtin/src/write.rs` (modify struct + execute)
- **Types involved:** `WriteTool`, `Mutex`, `PathBuf`, `Arc`
- **Functions to implement:**
  - `WriteTool::new() -> Self` — initializes empty lock map
  - Inside `execute()`: `let lock = self.get_or_create_lock(resolved_path)` → `let _guard = lock.lock().await` → proceed with write
  - `fn get_or_create_lock(&self, path: &Path) -> Arc<tokio::sync::Mutex<()>>`
- **Dependencies:** TASK-4.3
- **Implementation steps:**
  1. Add `path_locks: Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>` to `WriteTool` struct
  2. Implement `get_or_create_lock()`: lock outer mutex → get or insert inner mutex → return Arc
  3. In `execute()`: after resolution, get lock, acquire, do write, drop guard
  4. Cleanup: the lock map grows unbounded over the agent run → acceptable (agent runs are bounded by max_steps). A `DashMap` could be used for lock-free access, but Mutex+HashMap is simpler.
- **Edge cases:**
  - GAP G8 resolution: key is canonicalized relative path, not raw arg — "foo/./bar" and "foo/bar" share the same lock
  - Lock held across async boundary (file I/O) → use `tokio::sync::Mutex`, not `std::sync::Mutex`
  - Deadlock with concurrent reads? No — ReadTool doesn't use locks (TOCTOU accepted per SECT 14)
  - Lock never released if execute panics → `tokio::sync::Mutex` is not poisoned on panic
- **Definition of Done:**
  - Concurrent writes to same path are serialized
  - `cargo test` passes (including concurrent write test)
  - `cargo clippy` clean
- **Acceptance criteria:**
  - Two concurrent writes to same path → both succeed, file contains last write
  - Two concurrent writes to different paths → both run in parallel
  - Same path via different representations ("a/./b" and "a/b") → serialized
- **Test plan:**
  - unit: Spawn 2 concurrent writes to same path, verify final content is from last write; spawn 2 concurrent writes to different paths, verify both complete; test normalized path keys
- **Estimated effort:** 4 hours

---

### TASK-4.5: BashTool — dispatch to run_captured or ShellSession

- **§SPEC:** §11 (bash tool), §19 (Shell Sessions)
- **Labels:** `layer/tools-builtin`, `priority/critical`
- **Description:** Implement `BashTool` with `Args = BashArgs { command: Vec<String>, session: Option<Uuid> }`. The tool combines binary allowlist checking, ShellSession state management, and process execution. On each call: (1) classify the command via `ShellSession::classify()`, (2) if Cd/Export/Unset/Pwd → apply to session (no process spawned), (3) if Spawn → resolve binary against `BinaryRegistry`, apply session cwd/env, call `run_captured`. Sessions are stored in `BashTool.sessions: Mutex<HashMap<Uuid, ShellSession>>`. GAP G11: sessions are created explicitly when a new UUID is encountered.
- **Files affected:**
  - `crates/duga-tools-builtin/src/bash.rs` (new)
  - `crates/duga-tools-builtin/src/lib.rs` (add module)
- **Types involved:** `BashArgs`, `BashTool`, `ShellSession`, `SessionCommand`, `BinaryRegistry`, `Tool`, `ToolResult`, `ToolError`
- **Functions to implement:**
  - `BashArgs` struct with `JsonSchema` derive
  - `BashTool` struct with `sessions: Mutex<HashMap<Uuid, ShellSession>>`
  - `BashTool::new() -> Self`
  - `impl Tool for BashTool { ... }`
  - Inside execute: classify → if state command, apply to session → if Spawn, resolve binary + run_captured
- **Dependencies:** TASK-2.7 (ShellSession::classify + apply), TASK-2.3 (BinaryRegistry), TASK-2.4 (run_captured), TASK-3.1 (Tool trait)
- **Implementation steps:**
  1. Define `BashTool` struct with `sessions` field
  2. Implement `Tool for BashTool`: `name()` → `"bash"`, `description()` → `"Execute a command in the sandbox"`
  3. `execute()`:
      - Get or create session: if `args.session` is Some, lookup/create in `self.sessions`; if None, use ephemeral session (no persistence)
      - Classify command: `ShellSession::classify(&args.command)`
      - Match on `SessionCommand`:
        - `Cd`, `Export`, `Unset` → `session.apply(cmd, workspace)` → return ToolResult with output
        - `Pwd` → `session.apply(Pwd, ws)` → return ToolResult with cwd output
        - `Spawn(cmd_parts)` → resolve `cmd_parts[0]` via BinaryRegistry → `run_captured(binary, &cmd_parts[1..], workspace, session.cwd(), session.env(), limits, timeout, cancel)`
  4. Handle binary not allowed → `ToolError::Denied`
- **Edge cases:**
  - Session UUID not seen before → create new `ShellSession::new(PathBuf::from("."))`
  - No session UUID provided → use ephemeral session (cwd=".", env={})
  - Command is empty list → `ToolError::InvalidArgs`
  - Export with secret-pattern key → allowed (warn in SanitizedEnv, but that runs at exec time)
  - Session map grows unbounded → acceptable bounded by max_steps
- **Definition of Done:**
  - `cargo build` succeeds
  - `cargo test` passes
  - `cargo clippy` clean
- **Acceptance criteria:**
  - `bash { command: ["echo", "hello"] }` → `ToolResult { output: "hello\n", success: true }`
  - `bash { command: ["pwd"], session: uuid }` → output is cwd
  - `bash { command: ["cd", "subdir"], session: uuid }` + `bash { command: ["pwd"], session: uuid }` → cwd updated
  - `bash { command: ["export", "FOO=bar"], session: uuid }` → env set
  - `bash { command: ["nonexistent_binary"] }` → `Err(ToolError::Denied)`
  - `bash { command: [] }` → `Err(ToolError::InvalidArgs)`
- **Test plan:**
  - unit: Test with system binaries (echo, true, false); test shell session state tracking with cd/export/pwd/unset; test disallowed binary; test empty command; test session creation on first use
- **Estimated effort:** 6 hours

---

### TASK-4.6: BashTool — Sandbox + timeout enforcement

- **§SPEC:** §15 (Sandbox), §4 (AgentLimits.max_runtime for per-process timeout)
- **Labels:** `layer/tools-builtin`, `priority/critical`
- **Description:** Add per-process timeout from `Sandbox.timeout` config to BashTool. The timeout is passed to `run_captured()` which applies it via `tokio::time::timeout`. Also implement `BashTool::retryable() -> false` (bash tools are not retried — IO errors from subprocesses are not transient in the tool sense). The sandbox struct from SECT 15 is not a separate Rust struct — it's a logical container combining `Workspace` + `BinaryRegistry` + timeout.
- **Files affected:**
  - `crates/duga-tools-builtin/src/bash.rs` (modify execute)
- **Types involved:** `BashTool`, `OutputLimits`
- **Functions to implement:**
  - `BashTool::with_sandbox(registry: Arc<BinaryRegistry>, workspace: Arc<Workspace>, limits: OutputLimits, timeout: Duration) -> Self`
- **Dependencies:** TASK-4.5, TASK-2.3 (BinaryRegistry), TASK-2.4 (run_captured timeout)
- **Implementation steps:**
  1. Add fields to `BashTool`: `registry: Arc<BinaryRegistry>`, `workspace: Arc<Workspace>`, `limits: OutputLimits`, `timeout: Duration`
  2. Update `execute()` to pass limits + timeout to `run_captured()`
  3. Implement `Tool for BashTool`: `retryable()` → `false`
  4. Test timeout with `sleep` command
- **Edge cases:**
  - Timeout fires mid-process → `run_captured` returns `ToolError::Timeout`
  - Timeout is 0 → treat as no timeout (or immediate timeout)? → Config validation rejects zero (TASK-12.3)
- **Definition of Done:**
  - Timeout enforced
  - `cargo test` passes
- **Acceptance criteria:**
  - Command exec with timeout=1s, command=sleep 999 → returns after ~1s with ToolError::Timeout
  - `BashTool.retryable()` → `false`
- **Test plan:**
  - unit: Test timeout with sleep command (use short sleep, 100ms timeout)
- **Estimated effort:** 3 hours

---

### TASK-4.7: SearchTool — walkdir + regex in spawn_blocking

- **§SPEC:** §11 (search tool)
- **Labels:** `layer/tools-builtin`, `priority/high`
- **Description:** Implement `SearchTool` with `Args = SearchArgs { query: String, path: Option<String>, literal: Option<bool>, max_results: Option<usize> }`. Performs a recursive file search within the workspace using `walkdir` + `regex` crate, running in `spawn_blocking` to avoid blocking the async runtime. Matches `query` against each line in text files (skip binary files via NUL detection). GAP G5: use in-process regex, not shell-out to `rg`. The regex engine matches ripgrep's default (no lookarounds, no backreferences).
- **Files affected:**
  - `crates/duga-tools-builtin/src/search.rs` (new)
  - `crates/duga-tools-builtin/src/lib.rs` (add module)
- **Types involved:** `SearchArgs`, `SearchTool`, `Tool`, `ToolResult`, `ToolError`
- **Functions to implement:**
  - `SearchArgs` struct with `JsonSchema` derive
  - `SearchTool` struct (stateless)
  - `impl Tool for SearchTool { ... }`
  - `fn search_files(workspace: &Workspace, root: &Path, pattern: &Regex, max_results: usize) -> Result<String, ToolError>` — workhorse
- **Dependencies:** TASK-3.1 (Tool trait), TASK-2.1 (Workspace), TASK-2.2 (resolve)
- **Implementation steps:**
  1. Implement `SearchTool`
  2. `name()` → `"search"`, `description()` → `"Search for a pattern in workspace files"`
  3. `execute()`:
      - Resolve `args.path.unwrap_or(".".into())` via workspace
      - Compile regex: if `literal`, `regex::escape(&query)` then compile; else compile directly
      - Spawn `tokio::task::spawn_blocking(move || search_files(...))`
      - Inside `search_files`: walkdir from root, skip hidden files? → No spec guidance, skip `.git/` but search hidden files
      - For each text file: read line by line, grep for pattern, collect matches
      - Cap at `args.max_results.unwrap_or(200)`
      - Format output: `"{file}:{line_num}: {line_content}\n"`
  4. Return ToolResult with formatted output
- **Edge cases:**
  - Invalid regex → `ToolError::InvalidArgs(format!("Invalid regex: {}", e))`
  - No files match → return "No matches found"
  - Binary file encountered → skip silently (don't crash, don't search)
  - Max results reached → append "... (truncated, reached max_results of {n})"
  - Deep directory tree → walkdir may be slow for large trees; 120s timeout from Sandbox applies
  - Path is a file, not directory → search just that file
- **Definition of Done:**
  - `cargo build` succeeds
  - `cargo test` passes
  - `cargo clippy` clean
- **Acceptance criteria:**
  - Search for `"fn main"` in workspace → returns matching lines
  - Search with `literal: true` for `"fn.main"` → matches literal `fn.main`, not `fn main`
  - Search with invalid regex → `Err(ToolError::InvalidArgs)`
  - Search with `max_results: 2` → at most 2 results
  - Search in nonexistent path → `Err(ToolError::Io)` (resolve fails)
- **Test plan:**
  - unit: Create temp workspace with known files; test regex match, literal match, no matches, max_results cap, invalid regex, search specific subdirectory
  - Note: `spawn_blocking` converts to async → test with `tokio::test` + `tokio::time::timeout` to ensure it doesn't block forever
- **Estimated effort:** 6 hours

---

### TASK-4.8: SearchTool — literal mode + max_results cap + file filtering

- **§SPEC:** §11 (search tool — all args)
- **Labels:** `layer/tools-builtin`, `priority/normal`
- **Description:** Polish the SearchTool implementation: handle `literal: true` correctly (escape all regex meta-characters), respect `max_results` cap with truncation message, skip binary files, skip `.git/` directory, handle file-not-found-UTF8 gracefully. This is a quality/completeness task.
- **Files affected:**
  - `crates/duga-tools-builtin/src/search.rs` (enhance)
- **Types involved:** `SearchTool`
- **Functions to implement:** (enhancements to existing)
- **Dependencies:** TASK-4.7
- **Implementation steps:**
  1. Ensure `literal: true` uses `regex::escape()` before compiling
  2. Add skip for `.git/` directory (check component name)
  3. Add file type filtering: skip files with NUL in first 8 KiB (reuse is_binary from TASK-4.2)
  4. Add truncation message when max_results hit
  5. Handle non-UTF-8 files: use `String::from_utf8_lossy()` for reading
  6. Add reporting of number of files searched / skipped in output? → Spec doesn't require, but useful. Add as `metadata.files_searched` and `metadata.files_skipped`.
  7. Update tests
- **Edge cases:**
  - File becomes binary after first 8 KiB → detected when line read fails → skip with warning
  - Literal query contains regex syntax (`.`, `*`, `[`) → escaped correctly, matches literally
  - 0 bytes read from file (empty) → skip
- **Definition of Done:**
  - All edge cases handled
  - `cargo test` passes
  - `cargo clippy` clean
- **Acceptance criteria:**
  - Search with `literal: true, query: "fn.main("` matches `fn.main(` literally in file
  - Search skips `.git/` directory
  - Search skips binary files (any file with NUL)
  - Output includes truncation notice when max_results reached
  - `metadata.files_searched` and `metadata.files_skipped` present in ToolResult
- **Test plan:**
  - unit: Test literal mode with regex chars, test .git skip, test binary skip, test max_results with truncation message, test metadata fields
- **Estimated effort:** 4 hours

---

### TASK-4.9: ThinkTool — echo thought + think limit enforcement

- **§SPEC:** §11 (think tool), §12 (Think Governance)
- **Labels:** `layer/tools-builtin`, `priority/critical`
- **Description:** Implement `ThinkTool` with `Args = ThinkArgs { thought: String }`. The tool is a no-op: it echoes the thought back as output (so the LLM sees its own thought in the tool result). Also implement think limit enforcement: track call count and token count within the tool. When `max_calls` or `max_tokens` is exceeded, return `ToolError::Denied`. GAP G12: count tokens on the `thought` text, not on assistant tokens that produced the call. Use a simple word-based token estimate (words * 1.3) or a configurable tokenizer — for MVP, use the word heuristic.
- **Files affected:**
  - `crates/duga-tools-builtin/src/think.rs` (new)
  - `crates/duga-tools-builtin/src/lib.rs` (add module)
- **Types involved:** `ThinkArgs`, `ThinkTool`, `ThinkLimits`, `Tool`, `ToolResult`, `ToolError`
- **Functions to implement:**
  - `ThinkArgs` struct
  - `ThinkTool` struct with `call_count: AtomicUsize`, `token_count: AtomicUsize`, `limits: ThinkLimits`
  - `ThinkTool::new(limits: ThinkLimits) -> Self`
  - `impl Tool for ThinkTool { ... }`
  - `fn estimate_tokens(text: &str) -> usize` — words * 1.3 heuristic
- **Dependencies:** TASK-3.1 (Tool trait), TASK-1.7 (ThinkLimits)
- **Implementation steps:**
  1. Define `ThinkTool` with atomic counters
  2. Implement `Tool for ThinkTool`:
      - `name()` → `"think"`, `description()` → `"Think through a problem step by step"`
      - `execute()`:
        - Check call_count >= limits.max_calls → `Denied("think call limit reached")`
        - Check token_count + estimated_tokens(&args.thought) > limits.max_tokens → `Denied("think token limit reached")`
        - Increment counters
        - Return ToolResult with output = `args.thought` (echoed)
  3. Reset on tool creation only — counters persist for entire agent run (ThinkTool is not re-created)
- **Edge cases:**
  - GAP G12: counting `thought` text tokens means the LLM can't bypass by using short tool-calls. If counting assistant tokens, the LLM could make short think calls but use many tokens thinking. Word heuristic is approximate — acceptable for MVP.
  - Atomic counters must be thread-safe (BashTool may be called concurrently — GAP G1)
  - Token estimate overflow → use `usize::saturating_add`
  - `max_calls: 0` → all think calls denied
  - `max_tokens: 0` → all think calls denied
- **Definition of Done:**
  - Think tool works and enforces limits
  - `cargo test` passes
  - `cargo clippy` clean
- **Acceptance criteria:**
  - `think { thought: "I should check the file" }` → returns thought as output
  - After max_calls reached → `Err(ToolError::Denied("think call limit reached"))`
  - After max_tokens reached → `Err(ToolError::Denied("think token limit reached"))`
  - Counters are atomic and thread-safe
- **Test plan:**
  - unit: Test basic echo, test call limit enforcement, test token limit enforcement, test concurrent calls don't race on atomic counters
- **Estimated effort:** 4 hours

---

### TASK-4.10: Builtin tool integration tests

- **§SPEC:** §11 (all built-in tools working together)
- **Labels:** `layer/tools-builtin`, `priority/high`
- **Description:** Create integration tests that exercise all five built-in tools together in a realistic sequence: (1) write a file, (2) read it back, (3) search for content, (4) execute a command on it, (5) use think to reflect. Also test error handling: write to escaped path, read nonexistent file, bash with disallowed binary, search with bad regex. These tests validate the ToolDispatcher + ToolContext + built-in tool integration.
- **Files affected:**
  - `crates/duga-tools-builtin/tests/integration.rs` (new)
  - `crates/duga-tools-builtin/Cargo.toml` (add dev-deps)
- **Types involved:** All five tools, `ToolDispatcher`, `Workspace`, `BinaryRegistry`, `ToolContext`
- **Functions to implement:** (test functions)
- **Dependencies:** TASK-4.1 through TASK-4.9
- **Implementation steps:**
  1. Set up test harness: create temp workspace, populate BinaryRegistry with `echo`, `cat`, `true`, `false`
  2. Create ToolDispatcher with all five tools registered
  3. Test sequence: write("test.txt", "hello world\nfoo bar\n") → read("test.txt") → search("foo", literal=true) → bash(["cat", "test.txt"]) → think("all good")
  4. Test error scenarios per tool
  5. Test ToolContext.is_cancelled() propagation
- **Edge cases:**
  - Tests must not depend on system-specific binary paths (use only binaries from BinaryRegistry test set)
  - Temp workspace cleanup — use `tempfile::TempDir` wrapped in `Workspace::open`
- **Definition of Done:**
  - All integration tests pass
  - `cargo test --test integration` succeeds
- **Acceptance criteria:**
  - Write → Read roundtrip: content matches
  - Bash on written file: cat returns content
  - Search finds content in written file
  - All error scenarios return correct error types
- **Test plan:**
  - integration: Tests described above
- **Estimated effort:** 6 hours
