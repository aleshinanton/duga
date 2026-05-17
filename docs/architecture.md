# Agent-First Architecture — Hardened Runtime Specification

**Version:** 1.2  
**Date:** 2026-05-10

-----

# 1. Core Principle

LLM decides behavior.

Runtime provides:

- tools,
- memory,
- execution environment,
- capability boundaries,
- observability,
- bounded execution.

No hardcoded workflows.

```text
User → Agent Loop → Tools → Agent Loop → Answer
```

The runtime is:

- small,
- **deterministic at the runtime layer** (the loop, memory bookkeeping,
  tool dispatch, and event ordering are reproducible given the same LLM
  outputs; the LLM itself is not deterministic and the runtime makes no
  attempt to make it so),
- observable,
- interruptible,
- capability-bounded,
- extensible.

-----

# 2. Runtime Guarantees

The runtime guarantees:

- bounded execution,
- bounded memory,
- bounded tool access,
- bounded stdout/stderr,
- recoverable failures,
- interruptibility,
- deterministic tool contracts,
- observable execution.

-----

# 3. Explicit Non-Goals

Deliberately excluded:

- multi-agent orchestration,
- workflow DAGs,
- vector databases,
- autonomous planning systems,
- distributed execution,
- Kubernetes integration,
- complex permission graphs,
- framework abstractions,
- hidden orchestration layers.

Reason:

- preserve simplicity,
- reduce failure surface,
- maximize inspectability,
- keep reasoning inside the LLM.

-----

# 4. Agent Runtime

```rust
struct AgentConfig {
    limits: AgentLimits,
    features: AgentFeatures,
    output: OutputLimits,
    think: ThinkLimits,
}

struct AgentLimits {
    /// Max iterations of the core loop. Loop returns MaxStepsReached when hit.
    max_steps: usize,

    /// Max total tool invocations across the run (all tools combined).
    max_tool_calls: usize,

    /// Wall-clock cap for the entire run.
    max_runtime: Duration,

    /// Per-tool retries on transient errors (Io, Plugin). Non-transient errors
    /// (InvalidArgs, Denied, OutputLimitExceeded) are NOT retried.
    retry_on_error: usize,
}

struct AgentFeatures {
    /// If true, the runtime forwards LLM token deltas to `EventSink` as they
    /// arrive. See §5 for the streaming contract.
    streaming: bool,
}
```

Think limits live in their own struct (§12) and are the single source of
truth for think governance.

-----

# 5. Core Loop

The loop is the only piece of orchestration in the runtime. It enforces every
limit declared in §4 and routes tool errors back to the LLM as feedback rather
than aborting.

```rust
async fn run(&mut self, task: String) -> Result<String, AgentError> {
    let started = Instant::now();
    self.memory.push_user(task);

    loop {
        // --- Limit checks ---------------------------------------------------
        if self.steps >= self.config.limits.max_steps {
            return Err(AgentError::MaxStepsReached);
        }
        if self.tool_calls >= self.config.limits.max_tool_calls {
            return Err(AgentError::MaxToolCallsReached);
        }
        if started.elapsed() > self.config.limits.max_runtime {
            return Err(AgentError::Timeout);
        }
        if self.cancel.is_cancelled() {
            return Err(AgentError::Cancelled);
        }

        // --- LLM call (cancellable) ----------------------------------------
        let response = tokio::select! {
            biased;
            _ = self.cancel.cancelled() => return Err(AgentError::Cancelled),
            r = self.llm.chat(
                self.memory.messages(),
                self.tools.schemas(),
                LlmCallOptions { streaming: self.config.features.streaming },
                &self.events,
            ) => r?,
        };

        // --- Memory bookkeeping --------------------------------------------
        // The full assistant message (text + tool_calls) is always appended,
        // even when streaming has already emitted token deltas via EventSink.
        self.memory.push_assistant(response.message.clone());

        // --- Termination condition -----------------------------------------
        // A response with no tool calls is the final answer. Free-form text
        // accompanying tool calls is preserved in memory but does not end
        // the loop.
        if response.message.tool_calls.is_empty() {
            self.events.emit(Event::FinalResponse).await;
            return Ok(response.message.text.unwrap_or_default());
        }

        // --- Tool execution -------------------------------------------------
        // Each tool call is executed; success or failure produces a tool
        // message that is appended to memory so the LLM can react.
        for call in response.message.tool_calls {
            let result = self.handle_tool_call(call).await;
            self.memory.push_tool_result(result);
            self.tool_calls += 1;
        }

        // --- Compression ----------------------------------------------------
        if self.memory.over_budget() {
            self.memory.compress(&*self.summarizer).await?;
            self.events.emit(Event::MemoryCompressed).await;
        }

        self.steps += 1;
    }
}
```

## 5.1 `handle_tool_call`

```rust
async fn handle_tool_call(&mut self, call: ToolCall) -> ToolResult {
    self.events.emit(Event::ToolCallStarted {
        id: call.id, tool: call.tool.clone(),
    }).await;

    let started = Instant::now();
    let mut attempt = 0;
    let outcome = loop {
        match self.tools.dispatch(&call, self.tool_ctx()).await {
            Ok(r) => break Ok(r),
            Err(e) if e.is_transient()
                  && attempt < self.config.limits.retry_on_error => {
                attempt += 1;
                continue;
            }
            Err(e) => break Err(e),
        }
    };

    let result = ToolResult::from_outcome(call.id, &call.tool, outcome, started);
    self.events.emit(Event::ToolCallFinished {
        id: call.id, tool: call.tool, success: result.success,
    }).await;
    result
}
```

## 5.2 Streaming Contract

When `features.streaming` is `true`:

- The `LlmClient` emits `Event::LlmTokenDelta { text }` events as tokens arrive.
- The final returned `LlmResponse` still contains the complete message; memory
  is updated from the final message, never from deltas.
- Streaming applies only to assistant text; tool-call arguments are delivered
  whole.

-----

# 6. ToolCall

```rust
struct ToolCall {
    id: Uuid,

    tool: String,

    raw_args: serde_json::Value,
}
```

Raw JSON is preserved for:

- replay,
- debugging,
- observability.

-----

# 6a. LlmClient Interface

The runtime depends on a single LLM trait. It is the **only** boundary that
providers (OpenAI, Anthropic, Ollama, local) must implement.

```rust
trait LlmClient: Send + Sync {
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        opts: LlmCallOptions,
        events: &dyn EventSink,
    ) -> Result<LlmResponse, LlmError>;

    /// Token counter used by the memory subsystem (§21) for budget tracking.
    /// Providers MUST expose a tokenizer; the runtime never guesses.
    fn count_tokens(&self, messages: &[Message]) -> usize;
}

struct LlmCallOptions {
    streaming: bool,
}

struct LlmResponse {
    message: AssistantMessage,
    usage: TokenUsage,
}

struct AssistantMessage {
    /// Free-form assistant text. May coexist with tool calls.
    text: Option<String>,
    tool_calls: Vec<ToolCall>,
}

struct TokenUsage {
    prompt: u32,
    completion: u32,
}
```

Provider routing in config (§32) uses `provider/model` strings (e.g.
`ollama/qwen3:35b`, `anthropic/claude-3-5-sonnet`). The runtime ships a small
registry mapping prefixes to `LlmClient` implementations.

-----

# 6b. EventSink Interface

```rust
trait EventSink: Send + Sync {
    async fn emit(&self, event: Event);
}
```

The runtime uses a non-blocking, fan-out sink. Built-in sinks:

- `JsonlSink` — writes one JSON object per line (§25).
- `TuiSink` — streams to the terminal UI.
- `NullSink` — discards events (tests).

Multiple sinks can be composed via `MultiSink`.

-----

# 7. Typed Tool Arguments

Tools do NOT execute directly on raw `Value`.

Each tool defines strongly-typed arguments. `JsonSchema` is the
[`schemars`](https://docs.rs/schemars) v0.8+ derive; the schema dialect emitted
is Draft 2020-12 with the OpenAI/Anthropic-compatible subset (no `$ref`
cycles, no `oneOf` at the top level).

```rust
trait Tool: Send + Sync {
    type Args: DeserializeOwned + JsonSchema;

    fn name(&self) -> &str;

    fn description(&self) -> &str;

    /// True if errors of kind `Io`/`Plugin` should be retried per
    /// `AgentLimits::retry_on_error`. Defaults to false.
    fn retryable(&self) -> bool { false }

    async fn execute(
        &self,
        ctx: ToolContext<'_>,
        args: Self::Args,
    ) -> Result<ToolResult, ToolError>;
}
```

-----

# 8. Schema Validation Pipeline

```text
LLM output
    ↓
Parse JSON
    ↓
Validate schema
    ↓
Deserialize typed args
    ↓
Execute tool
```

Invalid schema becomes tool feedback:

```text
Tool 'write' failed:
missing required field 'content'
```

No panic.

-----

# 9. ToolResult

```rust
struct ToolResult {
    tool_call_id: Uuid,

    success: bool,

    /// The string fed back to the LLM as the tool message body. For process
    /// tools this is a formatted view (e.g. interleaved stdout/stderr or a
    /// summary). It is NOT a verbatim copy of either stream.
    output: String,

    /// Tool-specific structured data (e.g. exit code, file path). Not sent
    /// to the LLM unless the tool explicitly includes it in `output`.
    metadata: serde_json::Value,

    duration_ms: u64,

    /// Raw byte counts of the underlying streams BEFORE formatting/truncation,
    /// for telemetry only. Zero for tools that do not spawn processes.
    stdout_bytes: usize,
    stderr_bytes: usize,

    /// True if `output` was truncated to fit OutputLimits.
    truncated: bool,
}
```

-----

# 10. ToolContext

```rust
struct ToolContext<'a> {
    workspace: &'a Workspace,

    cancellation: CancellationToken,

    event_sink: &'a dyn EventSink,
}
```

Tools/plugins do NOT receive:

- mutable memory,
- orchestration control,
- prompt injection access.

This prevents capability explosion.

-----

# 11. Built-in Tools

## bash

```rust
#[derive(Deserialize, JsonSchema)]
struct BashArgs {
    /// Argv vector. The first element is resolved against the binary allowlist
    /// (§16). No shell interpretation is performed.
    command: Vec<String>,

    /// Optional persistent session id (§19). When present, cwd/env mutations
    /// recorded in the session apply to this call.
    session: Option<Uuid>,
}
```

Example:

```json
{
  "command": ["cargo", "test"]
}
```

No shell strings allowed. The tool name `bash` is historical; no `bash`
binary is invoked.

-----

## read

```rust
#[derive(Deserialize, JsonSchema)]
struct ReadArgs {
    path: String,

    /// 0-based byte offset. Defaults to 0.
    offset: Option<u64>,

    /// Max bytes to return. Defaults to 256 KiB. Hard cap: 4 MiB.
    limit: Option<u64>,
}
```

Binary files are detected via NUL-byte sniffing in the first 8 KiB; binary
content is returned as a hex/length summary, not raw bytes.

-----

## write

```rust
#[derive(Deserialize, JsonSchema)]
struct WriteArgs {
    path: String,
    content: String,
}
```

Writes are atomic: content is staged to a temp file in the same workspace
directory and renamed over the target. Concurrent writes to the same path
within one runtime are serialized through a per-path mutex.

-----

## search

```rust
#[derive(Deserialize, JsonSchema)]
struct SearchArgs {
    /// Rust-regex syntax (https://docs.rs/regex). Anchors and lookarounds are
    /// not supported; this matches ripgrep's default engine.
    query: String,

    /// Workspace-relative path to search. Defaults to workspace root.
    path: Option<String>,

    /// If true, treat `query` as a literal string, not a regex.
    literal: Option<bool>,

    /// Cap on result lines returned. Defaults to 200.
    max_results: Option<usize>,
}
```

-----

## think

```rust
#[derive(Deserialize, JsonSchema)]
struct ThinkArgs {
    thought: String,
}
```

-----

# 12. Think Governance

```rust
struct ThinkLimits {
    max_calls: usize,

    max_tokens: usize,
}
```

Purpose:

- prevent infinite self-reflection,
- prevent reasoning collapse loops.

-----

# 13. Workspace & Filesystem Isolation

Filesystem access uses capability-based APIs.

```rust
struct Workspace {
    root_dir: cap_std::fs::Dir,
}
```

All filesystem operations occur relative to the workspace capability.

No raw absolute filesystem access allowed.

-----

# 14. TOCTOU-Safe File Access

Do NOT do:

```rust
canonicalize()
then open()
```

Filesystem operations go through the workspace `Dir` capability, which
rejects absolute paths and `..` traversal at the syscall layer (using
`openat2(RESOLVE_BENEATH)` on Linux, equivalents elsewhere).

Symlinks within the workspace are followed only with `O_NOFOLLOW` semantics:
any symlink encountered during a path resolution that points outside the
workspace causes the open to fail. By default, even in-workspace symlinks
are not followed by `read`/`write`; tools that opt in must use
`OpenOptions::follow(true)` and accept the residual race surface.

This prevents:

- path traversal,
- cross-workspace escapes via symlink,
- the classic open-after-canonicalize TOCTOU race.

It does NOT prevent:

- in-workspace TOCTOU between two cooperating tools (out of scope: the LLM
  controls both ends).

-----

# 15. Sandbox

```rust
struct Sandbox {
    workspace: Workspace,

    allowed_binaries: HashSet<PathBuf>,

    timeout: Duration,
}
```

-----

# 16. Binary Resolution

The binary registry supports three modes of operation:

1. **Exact paths** (default): each entry is an absolute path or bare name
   resolved at startup via `which`. The resolved absolute path is cached.
2. **Glob patterns**: entries containing `*`, `?`, or `[` are expanded at
   startup by walking matching directories and registering each discovered
   executable.
3. **Allow-all**: a sentinel mode that skips all registry checks. The bare
   command name from the LLM is passed directly to the executor.

### 16.1 Exact Mode

```rust
cargo   -> /usr/bin/cargo
python3 -> /usr/bin/python3
```

Resolution is performed via `which`, then `realpath` once, and the resulting
absolute path is cached. Symlinks (e.g. rustup shims) are resolved at startup
only; later changes to the symlink target are not picked up until restart.

The runtime executes ONLY absolute resolved paths. This prevents:

- PATH hijacking,
- local executable shadowing,
- workspace poisoning of `./cargo` or similar.

### 16.2 Glob Mode

```yaml
sandbox:
  allowed_binaries:
    - /usr/bin/*           # all executables in /usr/bin
    - /usr/local/bin/g*    # git, gcc, go, etc.
    - /usr/bin/cat         # exact paths still work alongside globs
```

Glob patterns are expanded eagerly at startup. Each discovered executable
is registered in the allowlist. Patterns that match zero files emit a
startup warning. Path traversal (`..`) and relative paths (`./`) are
rejected in all pattern types.

Glob mode is the recommended middle ground between listing every binary
individually and allowing all.

### 16.3 Allow-All Mode

```yaml
sandbox:
  mode: "docker"
  container: "duga-sandbox"
  allow_all_binaries: true
  # allowed_binaries may be omitted or empty
```

When `allow_all_binaries: true`, the binary registry check is skipped
entirely. This mode is intended **only** for container/VM sandbox modes
(`docker`) where OS-level isolation provides the primary security boundary.

**Behavior by executor:**
- **Docker executor**: the bare command name is passed to `docker exec`.
  The container's own `PATH` resolves the binary. This allows binaries
  that exist only inside the container image (e.g. `apt-get` in a Debian
  container).
- **Capability/host executor**: the binary is still resolved via host
  `which` for safety. The host has no container `PATH`.

**Security warning**: allow-all mode removes the defense-in-depth layer.
A startup warning is emitted if `allow_all_binaries: true` is combined
with `mode: capability` or `mode: host`.

### 16.4 Scope of the allowlist

The allowlist controls **which programs may be launched**. It does NOT
constrain what those programs do once running. `cargo build` will execute
`build.rs` scripts; `python3 foo.py` will run arbitrary Python. Treat
everything in `allowed_binaries` as a fully-trusted gateway to arbitrary
code execution within the sandbox.

For stronger isolation, run the entire runtime inside an OS-level sandbox
(container, VM, `bwrap`, `landlock`); the binary allowlist is a
defense-in-depth layer, not a primary boundary.

-----

# 17. Safe Process Execution

No shell interpretation, ever.

```rust
Command::new(binary_path)
    .args(args)
```

NOT:

```rust
sh -c "..."
```

This eliminates:

- shell injection,
- command chaining (`;`, `&&`, `|`),
- subshell abuse.

There is no long-lived `bash -i` subprocess anywhere in the runtime. State
that would normally live in a shell (cwd, env vars, activated venv) is held
by the runtime itself — see §19.

-----

# 18. Output Limits

All process output is bounded.

```rust
struct OutputLimits {
    max_stdout_bytes: usize,

    max_stderr_bytes: usize,

    max_combined_bytes: usize,
}
```

If exceeded:

```text
Tool output truncated:
stdout exceeded 4MB limit
```

This prevents:

- OOM crashes,
- runaway stdout,
- context explosion.

-----

# 19. Persistent Shell Sessions

Some workflows require stateful execution across multiple tool calls.

```rust
struct ShellSession {
    id: Uuid,
    cwd: PathBuf,             // workspace-relative
    env: HashMap<String, String>,
}
```

## 19.1 Implementation

There is **no persistent shell process**. A `ShellSession` is a runtime-side
struct. When a `bash` tool call references a session id, the runtime applies
the session's `cwd` and `env` to a fresh `Command` for that single invocation.

## 19.2 Supported state changes

The runtime recognizes a small, fixed set of state-mutating commands and
applies them to the session **without spawning a process**:

| Command                    | Effect                                     |
|----------------------------|--------------------------------------------|
| `cd <path>`                | Update `session.cwd`                       |
| `export KEY=VALUE`         | Set `session.env[KEY]` (subject to §20)    |
| `unset KEY`                | Remove `session.env[KEY]`                  |
| `pwd`                      | Print `session.cwd`                        |

All other argv vectors are spawned directly. There is no `source`, no
`eval`, no `&&`. To activate a venv, set `PATH` and `VIRTUAL_ENV` via
`export` (the runtime exposes a helper tool `venv_activate` for this in
the standard tool set).

Session state persists between tool calls; sessions are dropped on agent
shutdown and never persisted to disk.

-----

# 20. Environment Variable Sanitization

Subprocesses inherit ONLY explicitly allowed variables. The default allowlist
is intentionally minimal:

```yaml
allowed_env:
  - HOME
  - USER
  - LANG
  - LC_ALL
  - TERM
```

`PATH` is **not** inherited. The runtime sets `PATH` to a fixed minimal value
(`/usr/bin:/bin`) for every spawned process. Because all binaries are launched
by absolute path (§16), subprocesses do not need a meaningful `PATH` —
leaking the host `PATH` would re-introduce the shadowing risk §16 closes.

Sensitive variables are excluded by default and cannot be added without an
explicit config opt-in:

- `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`
- `GITHUB_TOKEN`, `GH_TOKEN`
- `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`
- anything matching `*_TOKEN`, `*_KEY`, `*_SECRET`, `*_PASSWORD`

A matching variable that appears in `allowed_env` produces a startup warning
and is allowed; the operator owns the decision.

-----

# 21. Memory System

```rust
struct Memory {
    /// Pinned at construction; never compressed.
    system_messages: Vec<Message>,

    /// At most one summary message exists at any time. Pinned task facts
    /// (§22) live inside it, prefixed and protected from re-compression.
    summary: Option<SummaryMessage>,

    /// FIFO of messages since the last compression. Bounded only by tokens.
    recent_messages: VecDeque<Message>,

    /// Soft budget. Counted via `LlmClient::count_tokens` (§6a) using the
    /// active provider's tokenizer — the runtime never guesses.
    max_tokens: usize,
}

impl Memory {
    /// True when system + summary + recent exceeds `compress_at_ratio *
    /// max_tokens` (default 0.8). Compression is triggered eagerly so the
    /// next LLM call still fits.
    fn over_budget(&self) -> bool { /* ... */ }
}
```

Only ONE compressed summary exists at a time. This prevents recursive
summary degradation (§22).

-----

# 22. Context Compression

When the memory budget is exceeded:

```text
old messages (oldest half of recent_messages)
    +
existing summary (if any, included as input only — never re-summarized)
    ↓
summarizer.summarize(...)
    ↓
single new SummaryMessage  (replaces previous summary)
```

Rules:

- **Summaries never summarize summaries.** The previous summary is included
  in the summarizer's input as plain context, but the output replaces it
  wholesale.
- **Pinned task facts** (the original user task and any messages explicitly
  marked `pinned: true`) are passed to the summarizer as a verbatim block
  that must appear unchanged in the output.
- **Recent context preserved.** The newest half of `recent_messages` is
  retained verbatim; only the older half is fed to the summarizer.

## 22.1 Summary overflow

If, after compression, system + summary + retained-recent still exceeds
`max_tokens`, the runtime:

1. Drops the oldest non-pinned messages from the retained-recent window
   one at a time until the budget is met.
2. If the budget still cannot be met with only system + summary + pinned
   messages, returns `AgentError::ContextOverflow`. The runtime never
   silently truncates pinned content.

-----

# 23. Summarizer Interface

```rust
trait Summarizer {
    async fn summarize(
        &self,
        messages: &[Message],
    ) -> Result<SummaryMessage>;
}
```

Allows:

- local summarizers,
- remote summarizers,
- configurable compression strategies.

-----

# 24. Event System

Events carry enough payload for replay and offline analysis. Sensitive
fields are redacted before emission per §24.1.

```rust
enum Event {
    LoopIteration { step: usize, t: Timestamp },

    LlmRequest {
        messages: Vec<Message>,
        tools: Vec<ToolSchema>,
        opts: LlmCallOptions,
    },

    LlmTokenDelta { text: String },

    LlmResponse {
        message: AssistantMessage,
        usage: TokenUsage,
        latency_ms: u64,
    },

    ToolCallStarted {
        id: Uuid,
        tool: String,
        args: serde_json::Value,   // raw, after redaction
    },

    ToolCallFinished {
        id: Uuid,
        tool: String,
        success: bool,
        output: String,            // after redaction + truncation
        duration_ms: u64,
    },

    ToolCallFailed {
        id: Uuid,
        tool: String,
        error: String,
    },

    MemoryCompressed { before_tokens: usize, after_tokens: usize },

    FinalResponse,
}
```

All events carry a monotonic sequence number and a wall-clock timestamp
added by the sink layer; they are not part of the variant payload.

## 24.1 Redaction

Before emission, the runtime walks event payloads and replaces values
matching the secret patterns in §20 (`*_TOKEN`, `*_KEY`, `*_SECRET`,
`*_PASSWORD`, plus any operator-configured regexes) with the literal
string `"<REDACTED>"`. Redaction applies to:

- environment variable values surfaced in tool args/output,
- string fields in `LlmRequest.messages`,
- `ToolCallStarted.args` and `ToolCallFinished.output`.

Redaction is best-effort. Operators handling regulated data should run
the runtime in an environment without the secrets in scope.

-----

# 25. Replay Format

Events are exported as JSON Lines, one event per line. Each line is
self-describing:

```json
{"seq":42,"ts":"2026-05-10T18:31:04.221Z","event":"ToolCallStarted","id":"6b1...","tool":"read","args":{"path":"src/main.rs"}}
{"seq":43,"ts":"2026-05-10T18:31:04.231Z","event":"ToolCallFinished","id":"6b1...","tool":"read","success":true,"output":"...","duration_ms":10}
```

Replay requires three things, all present in the stream:

1. The `LlmRequest` payload (full message list + tool schemas) so a
   different LLM can be substituted.
2. The `LlmResponse` payload (full assistant message + tool calls) so the
   recorded run can be played back without an LLM at all.
3. `ToolCallStarted.args` and `ToolCallFinished.output` so tool execution
   can be mocked deterministically.

Enables:

- replay (with or without a live LLM),
- debugging,
- telemetry,
- offline analysis.

-----

# 26. Cancellation

```rust
CancellationToken
```

Supports:

- Ctrl+C,
- user interruption,
- timeout cancellation,
- graceful shutdown.

All tools receive cancellation via `ToolContext`.

-----

# 27. Error Model

```rust
enum ToolError {
    Timeout,
    Cancelled,
    Denied(String),
    InvalidArgs(String),
    Io(std::io::Error),
    Plugin(String),
    OutputLimitExceeded,
}

impl ToolError {
    /// Used by the loop to decide whether `retry_on_error` applies.
    fn is_transient(&self) -> bool {
        matches!(self, ToolError::Io(_) | ToolError::Plugin(_))
    }
}
```

Errors become tool feedback, not loop terminations:

```text
Tool 'bash' failed:
process timed out after 120s
```

The loop appends a tool message containing this text to memory and continues
(see §5). Only `AgentError` variants — `MaxStepsReached`,
`MaxToolCallsReached`, `Timeout`, `Cancelled`, `ContextOverflow` — terminate
the run.

-----

# 28. WASM Plugin Model

Plugins are WebAssembly Component Model components targeting `wasm32-wasip2`.
The runtime hosts them via [Wasmtime](https://wasmtime.dev) with the
component model enabled.

## 28.1 Plugin contract (WIT)

Every plugin exports the `tool` interface:

```wit
package duga:plugin@0.1.0;

interface tool {
    record tool-info {
        name: string,
        description: string,
        /// JSON Schema (Draft 2020-12) for the tool's argument object.
        args-schema: string,
    }

    record invocation {
        /// JSON-encoded arguments, already validated against args-schema.
        args-json: string,
        /// Workspace-relative paths the plugin may read/write. Enforced
        /// host-side; the plugin sees a pre-opened directory at fd 3.
        workspace-root: string,
    }

    record outcome {
        success: bool,
        output: string,
        metadata-json: string,
    }

    info: func() -> tool-info;
    execute: func(inv: invocation) -> result<outcome, string>;
}

world plugin {
    export tool;
}
```

```rust
trait WasmPlugin: Tool {
    fn module_path(&self) -> &Path;
    fn capabilities(&self) -> &WasiCapabilities;
}
```

Plugins are:

- isolated (separate Wasmtime `Store` per invocation),
- capability-limited (§29),
- runtime-loaded from disk (§30),
- subject to the same `OutputLimits`, cancellation, and timeout as built-in
  tools.

-----

# 29. WASI Capability Matrix

Default plugin capabilities:

|Capability          |Allowed|
|--------------------|-------|
|Workspace filesystem|Yes    |
|External filesystem |No     |
|Network access      |No     |
|Socket access       |No     |
|Process spawning    |No     |
|Randomness          |Yes    |
|Clocks              |Yes    |

Plugins receive ONLY explicitly granted capabilities.

-----

# 30. Plugin Loading

```rust
fn load_plugins(
    dir: &Path,
    config: &PluginConfig,
) -> Result<Vec<Box<dyn Tool>>, PluginError> {
    // 1. Enumerate *.wasm files in `dir`.
    // 2. Validate each is a WASI Preview 2 component.
    // 3. Instantiate, call `info()`, register under returned name.
    // 4. Resolve per-plugin capabilities from `config` (§29).
}
```

Only `.wasm` component-model modules are accepted. Core modules (non-component
WASM) are rejected at load time. A plugin whose `info()` call fails or whose
name collides with a built-in tool is logged and skipped.

-----

# 31. Observability

The runtime exposes:

- structured event stream,
- token usage telemetry,
- tool execution timing,
- replay logs.

This enables:

- debugging,
- benchmarking,
- operational monitoring.

-----

# 32. Config

The model string is `provider/model`; the runtime maps the prefix to a
registered `LlmClient` (§6a). `~` in `workspace.root` is expanded by the
runtime (using `$HOME`), not by a shell — there is no shell.

```yaml
model: "ollama/qwen3:35b"

agent:
  limits:
    max_steps: 50
    max_tool_calls: 100
    max_runtime: 10m
    retry_on_error: 2

  features:
    streaming: true

  output:
    max_stdout_bytes: 4194304
    max_stderr_bytes: 4194304
    max_combined_bytes: 6291456

  think:
    max_calls: 8
    max_tokens: 4096

sandbox:
  mode: "capability"   # capability | host | docker
  timeout: 120s

  # Binary allowlist — supports exact paths and glob patterns.
  # Omit or leave empty when allow_all_binaries is true.
  allowed_binaries:
    - /usr/bin/cargo
    - /usr/bin/git
    - /usr/bin/python3
    # - /usr/bin/*       # glob: all executables in /usr/bin

  # Docker-specific (only used when mode: docker):
  # container: "duga-sandbox"
  # workspace_mount: "/workspace"

  # Allow-all mode — skips binary registry checks.
  # WARNING: only use with container/VM isolation.
  # allow_all_binaries: true

workspace:
  root: "~/projects/demo"

environment:
  allowed:
    - HOME
    - USER
    - LANG
    - TERM
  # PATH is set internally to /usr/bin:/bin and is NOT inherited (§20).

memory:
  max_tokens: 8192
  compress_at_ratio: 0.8

plugins:
  dir: "./plugins"
  modules:
    - name: format_code.wasm
      capabilities:
        workspace_fs: true
    - name: analyze_project.wasm
      capabilities:
        workspace_fs: true
```

-----

# 33. Build

```bash
cargo build --release
```

Result:

```text
./harness
```

Plugins:

```bash
cargo build \
  --target wasm32-wasip2 \
  --release
```

-----

# 34. Threat Model

The runtime assumes:

- **untrusted LLM output** (jailbreaks, malicious tool calls),
- **untrusted tool output** (read-back content can carry prompt injection;
  the runtime treats every byte that re-enters the LLM context as adversarial
  input),
- partially trusted plugins,
- trusted runtime binary,
- trusted local operator.

The runtime protects against:

- shell injection (§17),
- path traversal and cross-workspace symlink escape (§14),
- TOCTOU on open-after-canonicalize (§14),
- runaway execution (timeouts §4, §15),
- stdout/stderr memory exhaustion (§18),
- unauthorized filesystem access outside the workspace (§13),
- secret leakage into event logs (§24.1).

The runtime does NOT attempt to defend against:

- prompt injection itself — the runtime limits *blast radius* (sandbox,
  capabilities, output caps) but cannot stop the LLM from being convinced
  to do something within its allowed capabilities,
- kernel exploits,
- malicious local root users,
- compromised host OS,
- side-channel attacks,
- network egress from built-in tools — `bash` may run `curl`, `git fetch`,
  `cargo build` (downloads crates). Network isolation is delegated to
  OS-level controls (containers, firewall, `unshare -n`). The plugin
  capability matrix (§29) does block plugin network access.

-----

# 35. Final Philosophy

The runtime is NOT:

- an orchestration platform,
- an IDE replacement,
- a workflow engine,
- an enterprise automation framework.

It is:

```text
LLM + tools + memory + sandbox + loop
```

Nothing more.
