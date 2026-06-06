//! duga-mcp: MCP (Model Context Protocol) proxy adapter.
//!
//! Connects duga to external MCP servers via a single `mcp()` proxy tool
//! (~200 tokens) instead of registering every tool individually.
//!
//! MCP servers are treated as **plugins** — they live under
//! `plugins.mcp` in config alongside WASM plugins. Both WASM and MCP
//! tools register into the same `ToolDispatcher` via `ErasedTool`.
//!
//! # Architecture
//!
//! ```text
//! build_dispatcher()
//!   ├── register built-in tools
//!   ├── load WASM plugins
//!   └── McpAdapter::bootstrap()
//!         ├── register mcp() proxy tool
//!         └── register direct tools (from cache)
//!
//! session_start() → connect eager/keep-alive servers → start health checks
//! session_shutdown() → flush cache → close all
//! ```

pub mod adapter;
pub mod cache;
pub mod client;
pub mod direct;
pub mod lifecycle;
pub mod manager;
pub mod naming;
pub mod proxy;
pub mod proxy_modes;

pub use adapter::McpAdapter;
pub use naming::{lookup_tool, ToolPrefixMode};
pub use proxy::McpProxyTool;
