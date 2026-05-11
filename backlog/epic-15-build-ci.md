# EPIC-15: Build + CI

**§SPEC:** §33  
**Labels:** `epic/build`  
**Crates:** `xtask/`, `ci/`

## Goal
Static binary builds for Linux (musl), macOS, and Windows. GitHub Actions CI matrix with build, test, clippy, and cargo-deny. Cross-compilation via cargo-xtask.

---

### TASK-15.1: rust-toolchain.toml + deny.toml + workspace config

- **§SPEC:** §33 (build configuration)
- **Labels:** `layer/build`, `priority/critical`
- **Description:** Finalize `rust-toolchain.toml` with pinned stable channel + `wasm32-wasip2` target. Complete `deny.toml` for `cargo-deny`: allow only MIT/Apache-2.0/BSD licenses, ban known-vulnerable crates, allow-list specific crate duplicates. Configure `.cargo/config.toml` with `[target.wasm32-wasip2]` runner. Add `cargo-deny` to CI.
- **Files affected:**
  - `rust-toolchain.toml`
  - `deny.toml`
  - `.cargo/config.toml`
- **Types involved:** (build config)
- **Dependencies:** TASK-1.1 (initial scaffold)
- **Implementation steps:**
  1. Pin Rust version: `channel = "1.85.0"` (or latest stable)
  2. Add targets: `targets = ["wasm32-wasip2"]`
  3. Configure deny.toml: licenses, advisories (vulnerabilities), bans (duplicates)
  4. `.cargo/config.toml`: add `[target.wasm32-wasip2]` section
  5. Run `cargo deny check` — fix any issues
- **Edge cases:**
  - Some dependencies may use non-standard licenses → review and add exceptions
  - wasmtime has its own license (Apache-2.0 with LLVM exception) → add to allow list
- **Definition of Done:** `cargo deny check` passes
- **Acceptance criteria:**
  - `cargo deny check licenses` → 0 errors
  - `cargo deny check advisories` → 0 errors
  - `cargo +stable build` works
- **Test plan:** Run `cargo deny check` in CI
- **Estimated effort:** 3 hours

---

### TASK-15.2: Static build config — Linux musl

- **§SPEC:** §33 (Static builds)
- **Labels:** `layer/build`, `priority/critical`
- **Description:** Configure static linking for `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` targets. Add rustflags for `+crt-static` and set `rust-lld` as linker. Verify the produced binary is statically linked (no dynamic library dependencies) using `ldd` or `file`.
- **Files affected:**
  - `.cargo/config.toml` (add musl targets)
- **Types involved:** (build config)
- **Dependencies:** TASK-15.1
- **Implementation steps:**
  1. Add to `.cargo/config.toml`:
     ```toml
     [target.x86_64-unknown-linux-musl]
     linker = "rust-lld"
     rustflags = ["-C", "target-feature=+crt-static"]
     ```
  2. Install musl target: `rustup target add x86_64-unknown-linux-musl`
  3. Build: `cargo build --release --target x86_64-unknown-linux-musl`
  4. Verify: `file target/x86_64-unknown-linux-musl/release/harness` → "statically linked"
  5. Verify: `ldd target/.../harness` → "not a dynamic executable"
- **Edge cases:**
  - wasmtime bundles its own C libraries → works with musl out of the box
  - reqwest needs `native-tls` or `rustls` → use `rustls` (no OpenSSL dep)
  - tokio signal handling differs on musl → test Ctrl+C on musl binary
  - aarch64 musl: cross-compile from x86_64 with `cargo-zigbuild` or use `cross`
- **Definition of Done:** Static musl binary produced
- **Acceptance criteria:**
  - `file harness` reports "statically linked"
  - Binary runs on Alpine Linux (no glibc)
  - Binary size < 50MB (wasmtime is ~30MB alone)
- **Test plan:** Build + file check + `ldd` check
- **Estimated effort:** 4 hours

---

### TASK-15.3: Static build config — macOS + Windows

- **§SPEC:** §33 (cross-platform builds)
- **Labels:** `layer/build`, `priority/high`
- **Description:** Configure builds for `aarch64-apple-darwin`, `x86_64-apple-darwin`, and `x86_64-pc-windows-msvc`. macOS: add `-dead_strip` linker flag. Windows: enable `+crt-static`. Verify binaries on target platforms.
- **Files affected:**
  - `.cargo/config.toml` (add macOS/Windows targets)
- **Types involved:** (build config)
- **Dependencies:** TASK-15.2
- **Implementation steps:**
  1. macOS config:
     ```toml
     [target.aarch64-apple-darwin]
     rustflags = ["-C", "link-arg=-dead_strip"]
     [target.x86_64-apple-darwin]
     rustflags = ["-C", "link-arg=-dead_strip"]
     ```
  2. Windows config:
     ```toml
     [target.x86_64-pc-windows-msvc]
     rustflags = ["-C", "target-feature=+crt-static"]
     ```
  3. Build for each target (cross-compile from Linux with `cargo-zigbuild`)
  4. Verify: `lipo -info` on macOS binary; no MSVC redist dependency on Windows
- **Edge cases:**
  - macOS code signing not required for CLI tools (but Gatekeeper may warn)
  - Windows: signal handling differs (tokio Ctrl+C works via `SetConsoleCtrlHandler`)
- **Definition of Done:** Binaries for all three platforms
- **Acceptance criteria:**
  - macOS binary runs on macOS
  - Windows binary runs on Windows without MSVC redist
- **Test plan:** Build check on target platform or cross-compile
- **Estimated effort:** 4 hours

---

### TASK-15.4: cargo-xtask build + dist commands

- **§SPEC:** §33 (build tooling)
- **Labels:** `layer/build`, `priority/high`
- **Description:** Implement `cargo xtask` with `build` and `dist` subcommands. `cargo xtask build --target <TARGET>` → builds the harness for the specified target. `cargo xtask dist --target <TARGET>` → builds, strips symbols, and packages the binary for distribution. Implement as a Rust binary in `xtask/`.
- **Files affected:**
  - `xtask/Cargo.toml` (new)
  - `xtask/src/main.rs` (new)
- **Types involved:** (build tooling)
- **Dependencies:** TASK-15.2, TASK-15.3
- **Implementation steps:**
  1. Create `xtask/` crate (no deps on duga — it's a build tool)
  2. `build` subcommand: run `cargo build --release --target <target>`
  3. `dist` subcommand: build + `strip` (or `objcopy --strip-all` for Linux) + create archive (`.tar.gz` for Linux/macOS, `.zip` for Windows)
  4. Also build plugin examples: `cargo build --target wasm32-wasip2 --release` for plugins
  5. `cargo xtask build-all` → builds all targets
  6. `cargo xtask dist-all` → builds and packages all targets
- **Edge cases:**
  - Missing target toolchain → clear error message with `rustup target add` instructions
  - `strip` not available → warn, skip stripping
  - Cross-compilation: use `cross` or `cargo-zigbuild` if native toolchain missing
- **Definition of Done:** `cargo xtask dist --target x86_64-unknown-linux-musl` produces a tarball
- **Acceptance criteria:**
  - `cargo xtask build --target x86_64-unknown-linux-musl` → binary
  - `cargo xtask dist --target x86_64-unknown-linux-musl` → `harness-x86_64-unknown-linux-musl.tar.gz`
  - Archive contains `harness` binary + `README.md` + `LICENSE`
- **Test plan:** Run xtask commands locally
- **Estimated effort:** 5 hours

---

### TASK-15.5: GitHub Actions CI matrix

- **§SPEC:** §33 (CI pipeline)
- **Labels:** `layer/build`, `priority/high`
- **Description:** Create `.github/workflows/ci.yml` with a matrix build across 5 targets: `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`, `x86_64-apple-darwin`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`. Steps: checkout, install Rust, build, test (host target only), WASM plugin build test, dist. Run on push to main and PRs.
- **Files affected:**
  - `.github/workflows/ci.yml` (new)
- **Types involved:** (CI config)
- **Dependencies:** TASK-15.4
- **Implementation steps:**
  1. Use `actions/checkout@v4`
  2. Use `actions-rust-lang/setup-rust-toolchain@v1` with toolchain from `rust-toolchain.toml`
  3. Matrix strategy with 5 targets
  4. Build step: `cargo xtask build --target ${{ matrix.target }}`
  5. Test step (only on linux-gnu): `cargo test --workspace`
  6. WASM plugin build: `cargo build --target wasm32-wasip2 --manifest-path plugins-examples/format-code/Cargo.toml`
  7. Dist step: `cargo xtask dist --target ${{ matrix.target }}`
  8. Upload artifacts
- **Edge cases:**
  - macOS CI: GitHub-hosted macOS runners are available
  - Windows CI: needs `windows-latest` runner
  - aarch64-linux-musl: cross-compile from x86_64 runner (no native aarch64 GitHub runner)
  - WASM plugin build should use the wasip2 target; install via rustup in CI
- **Definition of Done:** CI passes on all targets
- **Acceptance criteria:**
  - All 5 targets build in CI (< 30 minutes total)
  - Test step passes
  - WASM plugin builds
  - Dist artifacts uploaded
- **Test plan:** Push to branch, verify CI runs
- **Estimated effort:** 4 hours

---

### TASK-15.6: cargo deny + clippy + rustfmt in CI

- **§SPEC:** §33 (quality gates)
- **Labels:** `layer/build`, `priority/high`
- **Description:** Add quality gates to CI: `cargo deny check` (licenses + advisories), `cargo clippy --workspace -- -D warnings` (strict linting), `cargo fmt --check` (formatting). These run on every PR. Also add a `cargo audit` step (or use deny's advisory DB).
- **Files affected:**
  - `.github/workflows/ci.yml` (add steps)
  - `.github/workflows/lint.yml` (new — separate lint workflow)
- **Types involved:** (CI config)
- **Dependencies:** TASK-15.5, TASK-15.1
- **Implementation steps:**
  1. Create `lint.yml` workflow (runs on every push, fast feedback)
  2. Steps: `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo deny check`
  3. Run in parallel with build matrix (separate job)
  4. Add `clippy.toml` if needed for custom lint config
  5. Add `rustfmt.toml` with project formatting rules (if different from defaults)
- **Edge cases:**
  - Clippy may have false positives → fix or add `#[allow(...)]` with justification comment
  - `cargo deny` may need updates as deps change → part of maintenance
- **Definition of Done:** All lint gates pass in CI
- **Acceptance criteria:**
  - `cargo fmt --check` → clean
  - `cargo clippy --workspace -- -D warnings` → clean
  - `cargo deny check` → clean
- **Test plan:** Run gates locally before opening PR
- **Estimated effort:** 3 hours
