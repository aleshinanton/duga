# EPIC-34: MCP Proxy Adapter

**Labels:** `epic/mcp`, `epic/tools`, `epic/config`
**Crates:** `duga-mcp` (new), `duga-config`, `duga-runtime`, `duga-harness`, `duga-telegram-bot`, `duga-tui`
**Depends on:** EPIC-03 (Tool Trait System), EPIC-04 (Built-in Tools), EPIC-12 (CLI + Config), EPIC-18 (Frontend Shared Runtime)

## Goal

Add an MCP proxy adapter so duga connects to external MCP servers via a single `mcp()` proxy tool (~200 tokens) instead of registering every tool individually (thousands of tokens).

Supports stdio and HTTP transports, bearer token auth, lazy/eager/keep-alive lifecycles, metadata caching to disk, and optional direct tool promotion for frequently-used tools.

## Architecture

```
build_dispatcher()
  └── McpAdapter::bootstrap()
        ├── register mcp() proxy tool
        └── register direct tools (from cache, no server connection)

session_start() → connect eager/keep-alive servers → start health checks
session_shutdown() → flush cache → close all

mcp({ tool: "github_search_repos", args: '{"query":"rust"}' })
  → find server → lazy connect → tools/call → formatted result
```

## Decisions

- **M1:** Use `rust-mcp-sdk` v0.9 (client-only features: `client`, `stdio`, `streamable-http`). MIT, 150k+ downloads. Provides all transport + protocol.
- **M2:** Separate crate `crates/duga-mcp/`.
- **M3:** No `/mcp` commands. Proxy modes (search, describe, connect, etc.) serve as CLI.
- **M4:** MCP servers run outside duga's sandbox. `ToolContext` used only for cancellation.
- **M5:** Skip npx binary resolution — SDK's `StdioTransport::create_with_server_launch` handles it.
- **M6:** Telegram uses per-chat sessions (same pattern as EPIC-16 `ChannelQueue`).
- **M7:** Skip `McpObserver` telemetry — proxy tool output already logged via tool events.
- **OAuth:** Deferred. Bearer token auth included.
- **Config imports:** Deferred. Users copy-paste server blocks manually for v1.

---

## Tasks

### TASK-34.1: Add `mcpServers` config to `duga-config`

- **Labels:** `layer/config`, `priority/critical`
- **Description:** Add `McpConfig { mcpServers, settings }` to `Config`. Types: `ServerDefinition` (command/args/env/cwd or url/headers/auth/bearerToken), `McpSettings` (toolPrefix, idleTimeout, directTools, disableProxyTool), `LifecycleMode` (lazy/eager/keep-alive), `DirectToolsMode` (bool | list). Env var interpolation (`${VAR}`, `$env:VAR`), `~` expansion in `cwd`. Accept both `mcpServers` and `mcp-servers` keys.
- **Files:** `crates/duga-config/src/config.rs`
- **Dependencies:** None
- **Estimated effort:** 4 hours

### TASK-34.2: Create `duga-mcp` crate

- **Labels:** `layer/mcp`, `priority/critical`
- **Description:** New crate with `rust-mcp-sdk` (client-only). Module skeleton: `adapter`, `cache`, `client`, `direct`, `lifecycle`, `manager`, `naming`, `proxy`, `proxy_modes`.
- **Files:** `crates/duga-mcp/Cargo.toml`, `src/lib.rs`
- **Dependencies:** TASK-34.1
- **Estimated effort:** 1 hour

### TASK-34.3: MCP client wrapper

- **Labels:** `layer/mcp`, `priority/critical`
- **Description:** Thin wrapper over SDK's `client_runtime`. `connect_stdio(def)` / `connect_http(def)` with StreamableHTTP → SSE fallback. `call_tool(name, args)`. Bearer token via `TransportOptions`. Cursor-paginated `list_tools()` and `list_resources()`.
- **Files:** `crates/duga-mcp/src/client.rs`
- **Dependencies:** TASK-34.2
- **Estimated effort:** 5 hours

### TASK-34.4: Metadata cache

- **Labels:** `layer/mcp`, `priority/high`
- **Description:** Disk-backed JSON at `~/.duga/mcp-cache.json`. `configHash`: SHA-256 of identity fields with stable stringify. Validity: version + hash + age < 7 days. Atomic write via `.tmp` → `rename()`.
- **Files:** `crates/duga-mcp/src/cache.rs`
- **Dependencies:** TASK-34.2
- **Estimated effort:** 4 hours

### TASK-34.5: Server manager

- **Labels:** `layer/mcp`, `priority/high`
- **Description:** Connection dedup via in-flight connect promise map. `connect(name, def)`, `close(name)` (set Closed first), `touch()`, `in_flight` tracking, `is_idle(timeout)`.
- **Files:** `crates/duga-mcp/src/manager.rs`
- **Dependencies:** TASK-34.3, TASK-34.4
- **Estimated effort:** 5 hours

### TASK-34.6: Lifecycle manager

- **Labels:** `layer/mcp`, `priority/high`
- **Description:** Health check interval (30s). Keep-alive: reconnect + update cache. Lazy: close if idle. Timeout: per-server → global → 10 min default. `graceful_shutdown()`.
- **Files:** `crates/duga-mcp/src/lifecycle.rs`
- **Dependencies:** TASK-34.5
- **Estimated effort:** 4 hours

### TASK-34.7: Tool naming + fuzzy lookup

- **Labels:** `layer/mcp`, `priority/normal`
- **Description:** Prefix modes: `server` → `github_search_repos`, `short` → strips `-mcp`, `none` → raw name. Fuzzy: normalize to lowercase, `_` ↔ `-`. `excludeTools` matches both original and prefixed names.
- **Files:** `crates/duga-mcp/src/naming.rs`
- **Dependencies:** None
- **Estimated effort:** 2 hours

### TASK-34.8: Proxy modes

- **Labels:** `layer/mcp`, `priority/high`
- **Description:** `execute_status()`, `execute_search()` (whitespace OR, regex), `execute_describe()` (fuzzy + schema), `execute_list()`, `execute_connect()`, `execute_call()` (find server → lazy connect → `tools/call` → transform). Handle `isError` with schema hint. Prefix-based lazy connect for unknown tools. `args` is JSON string.
- **Files:** `crates/duga-mcp/src/proxy_modes.rs`
- **Dependencies:** TASK-34.5, TASK-34.7
- **Estimated effort:** 8 hours

### TASK-34.9: `McpProxyTool` — the `mcp()` tool

- **Labels:** `layer/mcp`, `priority/high`
- **Description:** Implement `duga_tools::Tool`. Args: `{ tool?, args?, connect?, describe?, search?, server?, regex?, includeSchemas?, label }`. Dispatch: `tool` > `connect` > `describe` > `search` > `server` > status. Dynamic description from server list. `disableProxyTool` skips registration.
- **Files:** `crates/duga-mcp/src/proxy.rs`
- **Dependencies:** TASK-34.3, TASK-34.5, TASK-34.8
- **Estimated effort:** 4 hours

### TASK-34.10: Direct tools registrar

- **Labels:** `layer/mcp`, `priority/normal`
- **Description:** `resolve_direct_tools(config, cache, prefix)`: for servers with `directTools` + valid cache, build `ErasedTool` per tool not excluded. Proxy-disable guard: if `disableProxyTool`, all `directTools` servers must have valid cache.
- **Files:** `crates/duga-mcp/src/direct.rs`
- **Dependencies:** TASK-34.4, TASK-34.7
- **Estimated effort:** 4 hours

### TASK-34.11: `McpAdapter` — bootstrap + lifecycle

- **Labels:** `layer/mcp`, `priority/high`
- **Description:** `bootstrap()` (sync): load config + cache → resolve direct tools → build proxy → return `(Self, Vec<ErasedTool>)`. `session_start()`: gen counter → shutdown previous → connect eager/keep-alive → populate metadata → start health checks. `session_shutdown()`: flush cache → graceful shutdown. Stale init results discarded via `AtomicU64` generation counter.
- **Files:** `crates/duga-mcp/src/adapter.rs`
- **Dependencies:** TASK-34.4, TASK-34.5, TASK-34.6, TASK-34.9, TASK-34.10
- **Estimated effort:** 5 hours

### TASK-34.12: Wire into `duga-runtime` tool construction

- **Labels:** `layer/runtime`, `priority/high`
- **Description:** Modify `build_dispatcher` to return `(Arc<ToolDispatcher>, Option<McpAdapter>)`. Bootstrap adapter if `config.mcp` present, register its tools.
- **Files:** `crates/duga-runtime/src/tools.rs`
- **Dependencies:** TASK-34.11
- **Estimated effort:** 2 hours

### TASK-34.13: Wire lifecycle into frontends

- **Labels:** `layer/frontend`, `priority/high`
- **Description:** CLI: `session_start()` before loop, `session_shutdown()` after + signal. Telegram: per-chat sessions. TUI: start on input, shutdown on quit. Document M4.
- **Files:** `crates/duga-harness/src/main.rs`, `crates/duga-telegram-bot`, `crates/duga-tui`
- **Dependencies:** TASK-34.12
- **Estimated effort:** 3 hours

### TASK-34.14: Unit tests

- **Labels:** `layer/testing`, `priority/normal`
- **Description:** Config parsing, cache hash/validity, naming/fuzzy, manager dedup/idle, lifecycle idle/keep-alive, proxy modes, adapter bootstrap/stale discard.
- **Files:** `crates/duga-mcp/src/*.rs`
- **Dependencies:** TASK-34.1–34.11
- **Estimated effort:** 6 hours

### TASK-34.15: Integration test with real MCP server

- **Labels:** `layer/testing`, `priority/normal`
- **Description:** Connect → list tools → call tool. Lazy connect on first call. Idle timeout disconnect. Direct tools from cache without server.
- **Files:** `crates/duga-mcp/tests/`
- **Dependencies:** TASK-34.13
- **Estimated effort:** 4 hours

## Deferred

- OAuth / authorization_code flow
- `/mcp` CLI commands
- MCP UI integration
- Config imports (cursor, claude-code, vscode detection)
- Hot reload of MCP server configs
