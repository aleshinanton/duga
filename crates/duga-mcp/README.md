# duga-mcp

MCP (Model Context Protocol) proxy adapter — connects duga to external MCP servers via a single `mcp()` proxy tool (~200 tokens) instead of registering every tool individually (thousands of tokens).

## Architecture

```
build_dispatcher()
  ├── register built-in tools (shell, read, write, edit, search, think)
  ├── load WASM plugins from plugins.wasm_dir
  └── McpAdapter::bootstrap()  ← if plugins.mcp is present
        ├── register mcp() proxy tool
        └── register direct tools (from cache, no server connection)

session_start() → connect eager/keep-alive servers → start health checks
session_shutdown() → flush cache → close all

mcp({ tool: "github_search_repos", args: '{"query":"rust"}' })
  → find server → lazy connect → tools/call → formatted result
```

## Config

MCP servers live under `plugins.mcp` alongside WASM plugins:

```yaml
plugins:
  wasm_dir: ./plugins

  mcp:
    servers:
      github:
        transport: stdio
        command: npx
        args: ["-y", "@modelcontextprotocol/server-github"]
        env:
          GITHUB_PERSONAL_ACCESS_TOKEN: "${GITHUB_TOKEN}"
        lifecycle: lazy

      filesystem:
        transport: stdio
        command: npx
        args: ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
        lifecycle: eager
        direct_tools:
          - read_file
          - write_file

    settings:
      tool_prefix: server
      idle_timeout: 600s
      disable_proxy_tool: false
```

## Features

- **Single proxy tool** — `mcp()` tool registers once in the dispatcher instead of registering every MCP tool
- **Stdio + HTTP transports** — stdio process launch or Streamable HTTP with bearer token auth
- **Lifecycle modes** — lazy (connect on first use), eager (connect at session start), keep-alive (reconnect on failure)
- **Metadata cache** — tool schemas cached to disk at `~/.duga/mcp-cache.json` with hash-based invalidation
- **Direct tool promotion** — frequently-used MCP tools can be registered directly instead of via proxy
- **Tool search** — fuzzy search across all configured servers with whitespace OR or regex matching
- **Abstract lifecycle** — frontends call `runtime.init_tools()` / `runtime.shutdown_tools()` without importing MCP types

## Dependencies

- `rust-mcp-sdk` v0.9 (client-only: `stdio`, `streamable-http`)
- `duga-config` — MCP config types
- `duga-tools` — `Tool` trait, `ErasedTool`, `ToolDispatcher`

## Testing

```bash
cargo test -p duga-mcp
```

Integration tests require a running MCP server (e.g., `@modelcontextprotocol/server-everything`).

## Future

- OAuth / authorization_code flow (deferred)
- Hot reload of MCP server configs (deferred)
- Config imports from cursor/claude-code/vscode (deferred)
