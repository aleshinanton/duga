# EPIC-21: Binary Allowlist Patterns

**SPEC:** §16, §17
**Labels:** `epic/sandbox`, `epic/security`
**Crates:** `duga-sandbox`, `duga-config`, `duga-tools-builtin`, `duga-runtime`

## Goal

Extend the binary allowlist to support wildcard patterns and an explicit "allow all" mode, so that sandboxed environments (Docker containers, VMs) can grant the agent unconstrained command execution without listing every binary individually.

Currently `sandbox.allowed_binaries` requires listing every binary by absolute path or bare name, which is impractical for Docker containers with hundreds of installed tools. Docker mode already provides OS-level isolation — the allowlist is defense-in-depth (§16.1), not the primary boundary — so a pattern-based or allow-all option is appropriate when the executor is a container.

---

### TASK-21.1: Add `allow_all_binaries` config flag

- **SPEC:** §16 (Binary Resolution), §17 (Sandbox)
- **Labels:** `layer/config`, `layer/sandbox`, `priority/high`
- **Description:** Add a `sandbox.allow_all_binaries` boolean flag to `SandboxConfig`. When `true`, the `BashTool` skips the `BinaryRegistry` check entirely for spawn commands. This flag only makes sense with `sandbox.mode: "docker"` (or future container/VM modes) — emit a config warning if set with capability/host mode.
- **Files affected:**
  - `crates/duga-config/src/config.rs`
  - `crates/duga-sandbox/src/binary_registry.rs`
  - `crates/duga-tools-builtin/src/bash.rs`
  - `crates/duga-runtime/src/tools.rs`
- **Types involved:** `SandboxConfig` (new field), `BinaryRegistry` (new `allow_all()` constructor), `BashTool` (conditional check)
- **YAML example:**
  ```yaml
  sandbox:
    mode: "docker"
    container: "duga-sandbox"
    allow_all_binaries: true
    # allowed_binaries may be omitted or empty
  ```
- **Dependencies:** TASK-18.6 (Docker executor)
- **Implementation steps:**
  1. Add `allow_all_binaries: bool` field to `SandboxConfig` with `#[serde(default)]`.
  2. Add `Config::validate()` warning when `allow_all_binaries: true` but `sandbox.mode` is `capability` or `host`.
  3. Add `BinaryRegistry::allow_all()` static constructor returning a sentinel value.
  4. Add `BinaryRegistry::is_allow_all(&self) -> bool` method.
  5. In `BashTool::execute()`, skip `registry.resolve()` when the registry is in allow-all mode; pass the bare binary name directly to the executor as the program.
  6. In `build_dispatcher()`, use `BinaryRegistry::allow_all()` when the config flag is set.
- **Definition of Done:** Setting `allow_all_binaries: true` in Docker mode bypasses the binary allowlist; capability/host mode produces a config warning.
- **Acceptance criteria:**
  - `allow_all_binaries: true` + `mode: docker` → any command passes the registry check.
  - `allow_all_binaries: true` + `mode: capability` → config loads but warns at startup.
  - `allow_all_binaries: false` (default) → existing behavior unchanged.
  - An empty or missing `allowed_binaries` list is valid when `allow_all_binaries: true`.
- **Test plan:** config parse tests for new field; bash tool tests with allow-all registry; Docker integration test.
- **Estimated effort:** 3 hours

---

### TASK-21.2: Add glob/wildcard pattern support to BinaryRegistry

- **SPEC:** §16 (Binary Resolution)
- **Labels:** `layer/sandbox`, `priority/high`
- **Description:** Extend `BinaryRegistry` to accept glob patterns (e.g., `/usr/bin/*`, `/usr/local/bin/g*`) alongside exact paths. Patterns are resolved at startup by walking the matching directories and registering each discovered binary. This gives operators a middle ground between listing every binary and allowing all.
- **Files affected:**
  - `crates/duga-sandbox/src/binary_registry.rs`
  - `crates/duga-config/src/config.rs` (optional pattern validation)
- **Types involved:** `BinaryRegistry`, `BinaryPattern` (new enum: `Exact(PathBuf)` | `Glob(String)`)
- **Config extension:**
  ```yaml
  sandbox:
    allowed_binaries:
      - /usr/bin/*           # all binaries in /usr/bin
      - /usr/local/bin/g*    # git, gcc, go, etc.
      - /home/user/.cargo/bin/cargo  # exact path still works
  ```
- **Dependencies:** TASK-2.3 (BinaryRegistry), TASK-21.1 (allow_all flag)
- **Implementation steps:**
  1. Define `BinaryPattern` enum with `Exact(PathBuf)` and `Glob(String)` variants.
  2. Add `BinaryRegistry::from_patterns(patterns: &[BinaryPattern])` constructor.
  3. For each glob pattern, `glob::glob()` over matching host files; register each discovered executable.
  4. Reject patterns that contain path traversal (`..`, leading `./`).
  5. Emit a warning for globs that match zero files.
  6. Keep backward compatibility: bare names and absolute paths are treated as exact patterns.
  7. Resolve pattern entries eagerly at startup (same as exact paths).
- **Definition of Done:** Glob patterns in `allowed_binaries` expand to concrete binary entries at startup.
- **Acceptance criteria:**
  - `/usr/bin/*` matches all executables in `/usr/bin`.
  - `/nonexistent/*` emits a config warning (zero matches).
  - Exact paths still work as before.
  - Patterns with `..` are rejected.
  - A glob that matches 50 binaries registers all 50 in the registry.
- **Test plan:** unit tests for pattern expansion with temp directories; config parse tests for glob rejection.
- **Estimated effort:** 4 hours

---

### TASK-21.3: Allow bare command names in allow-all Docker mode

- **SPEC:** §16.1 (allowlist scope), §17 (process execution)
- **Labels:** `layer/sandbox`, `layer/tools`, `priority/normal`
- **Description:** When `allow_all_binaries: true`, pass the LLM-provided command name directly to the Docker executor without resolving it against the host. The Docker container's own `PATH` will resolve the binary. This avoids false "not found" errors when binaries exist in the container but not on the host.
- **Files affected:**
  - `crates/duga-tools-builtin/src/bash.rs`
  - `crates/duga-sandbox/src/executor.rs`
- **Types involved:** `BashTool`, `CommandSpec`, `DockerExecutor`
- **Dependencies:** TASK-21.1 (allow_all flag), TASK-18.6 (Docker executor)
- **Implementation steps:**
  1. In `BashTool::execute()`, when the registry is in allow-all mode and executor is Docker, set `spec.program` to the bare command name rather than a resolved host path.
  2. In `DockerExecutor::run()`, pass the bare program name to `docker exec` so the container resolves it.
  3. For `CapabilityExecutor`, keep resolving through host `which` even in allow-all mode (host has no container PATH).
  4. Document that `PATH` inside the container must include all desired tool locations.
- **Definition of Done:** Commands in Docker allow-all mode resolve inside the container, not on the host.
- **Acceptance criteria:**
  - `apt-get` works in a Debian/Ubuntu container even if the host doesn't have it.
  - Host-only binaries remain unresolvable in the container unless mapped.
  - Capability executor in allow-all mode still uses host `which`.
- **Test plan:** Docker integration test with a binary that exists only in the container image.
- **Estimated effort:** 2 hours

---

### TASK-21.4: Documentation — sandbox modes and allowlist patterns

- **SPEC:** §16, §32 (configuration)
- **Labels:** `layer/docs`, `priority/normal`
- **Description:** Document the new allowlist options in `docs/architecture.md` (§16), the README, and any config examples. Include guidance on when to use each option and the security tradeoffs.
- **Files affected:**
  - `docs/architecture.md`
  - `README.md`
  - Example config files (if any)
- **Dependencies:** TASK-21.1, TASK-21.2
- **Implementation steps:**
  1. Update §16 in architecture.md with `allow_all_binaries`, glob patterns, and Docker-specific resolution behavior.
  2. Add a config example for Docker + allow-all mode.
  3. Add a security note: allow-all removes defense-in-depth; only use with container/VM isolation.
  4. Add guidance on glob patterns as the recommended middle ground.
- **Definition of Done:** All new config options are documented with examples and security notes.
- **Acceptance criteria:**
  - Architecture spec covers all three modes (exact, glob, allow-all).
  - README has a Docker + allow-all quick-start.
  - Security caveats are prominent.
- **Test plan:** doc review only.
- **Estimated effort:** 2 hours

---

## Summary Table

| Task | Name | Est. Hours |
|------|------|------------|
| TASK-21.1 | Add `allow_all_binaries` config flag | 3 |
| TASK-21.2 | Add glob/wildcard pattern support | 4 |
| TASK-21.3 | Allow bare command names in allow-all Docker mode | 2 |
| TASK-21.4 | Documentation | 2 |
| **Total** | | **11 hours** |
