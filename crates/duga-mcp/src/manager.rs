//! Server manager — tracks connected MCP servers.
//!
//! Provides connection deduplication via in-flight connect tracking.
//! Manages connected servers with idle tracking for lifecycle management.
//! Exposes `call_tool` and `populate_tool_cache` for proxy and adapter use.

use crate::cache::{CachedTool, MetadataCache};
use crate::client::McpClientConnection;
use duga_config::McpServerDefinition;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, Notify};

/// State for a single managed server.
struct ServerState {
    /// The connected client, if any (Arc for shared access without holding lock).
    connection: Option<Arc<McpClientConnection>>,
    /// When the server was last used (for idle detection).
    last_used: Instant,
    /// Whether the server is currently being connected to.
    connecting: bool,
    /// Notification for waiters when connect completes.
    notify: Arc<Notify>,
}

impl ServerState {
    fn new() -> Self {
        Self {
            connection: None,
            last_used: Instant::now(),
            connecting: false,
            notify: Arc::new(Notify::new()),
        }
    }
}

/// Manages MCP server connections with deduplication.
pub struct ServerManager {
    /// Per-server state keyed by server name.
    servers: Mutex<HashMap<String, ServerState>>,
}

impl ServerManager {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            servers: Mutex::new(HashMap::new()),
        })
    }

    /// Connect to a server. Deduplicates concurrent connect calls —
    /// if a connect is already in flight, waits for it to complete.
    pub async fn connect(
        self: &Arc<Self>,
        name: &str,
        def: &McpServerDefinition,
    ) -> Result<(), String> {
        let notify = {
            let mut servers = self.servers.lock().await;
            if let Some(state) = servers.get(name) {
                if state.connection.is_some() {
                    return Ok(());
                }
                if state.connecting {
                    return {
                        let n = Arc::clone(&state.notify);
                        drop(servers);
                        n.notified().await;
                        // Re-check
                        let servers = self.servers.lock().await;
                        if servers.get(name).map(|s| s.connection.is_some()).unwrap_or(false) {
                            return Ok(());
                        }
                        // Retry connect if other failed
                        drop(servers);
                        Box::pin(self.connect(name, def)).await
                    };
                }
                let state = servers.get_mut(name).unwrap();
                state.connecting = true;
                return self.do_connect_inner(name, def).await;
            }
            let mut state = ServerState::new();
            state.connecting = true;
            servers.insert(name.to_string(), state);
            // Immediately connect below.
        };
        self.do_connect_inner(name, def).await
    }

    async fn do_connect_inner(
        self: &Arc<Self>,
        name: &str,
        def: &McpServerDefinition,
    ) -> Result<(), String> {
        let result = if def.transport == "http" || def.transport == "streamable-http" {
            crate::client::connect_http(def).await
        } else {
            crate::client::connect_stdio(def).await
        };

        let mut servers = self.servers.lock().await;

        let (connection, notify) = match result {
            Ok(connection) => {
                let notify = servers
                    .get(name)
                    .map(|s| Arc::clone(&s.notify))
                    .unwrap_or_else(|| Arc::new(Notify::new()));
                (Some(Arc::new(connection)), notify)
            }
            Err(e) => {
                if let Some(state) = servers.get_mut(name) {
                    state.connecting = false;
                    let n = Arc::clone(&state.notify);
                    n.notify_waiters();
                }
                return Err(format!("failed to connect to '{}': {}", name, e));
            }
        };

        let state = servers.get_mut(name).unwrap();
        state.connection = connection;
        state.connecting = false;
        state.last_used = Instant::now();
        notify.notify_waiters();
        Ok(())
    }

    /// Close a server connection.
    pub async fn close(&self, name: &str) -> Result<(), String> {
        let connection = {
            let mut servers = self.servers.lock().await;
            if let Some(state) = servers.get_mut(name) {
                state.connection.take()
            } else {
                return Ok(());
            }
        };

        if let Some(conn) = connection {
            let _ = conn.shutdown().await;
        }
        Ok(())
    }

    /// Get a reference to a connected server's client (for tool calling).
    async fn get_connection(&self, name: &str) -> Option<Arc<McpClientConnection>> {
        let servers = self.servers.lock().await;
        servers.get(name).and_then(|s| s.connection.clone())
    }

    /// Call a tool on a connected MCP server.
    /// Connects lazily if not already connected.
    pub async fn call_tool(
        self: &Arc<Self>,
        server_name: &str,
        def: &McpServerDefinition,
        tool_name: &str,
        args: serde_json::Map<String, serde_json::Value>,
    ) -> Result<String, String> {
        // Ensure connected (lazy connect).
        if !self.is_connected(server_name).await {
            self.connect(server_name, def).await?;
        }

        // Touch for idle tracking.
        self.touch(server_name).await;

        // Get the connection.
        let conn = self
            .get_connection(server_name)
            .await
            .ok_or_else(|| format!("Server '{}' not connected", server_name))?;

        // Call the tool.
        match conn.call_tool(tool_name, args).await {
            Ok(result) => {
                // Format the result for the LLM.
                let content: Vec<String> = result
                    .content
                    .iter()
                    .filter_map(|item| {
                        match item {
                            rust_mcp_sdk::schema::ContentBlock::TextContent(tc) => {
                                Some(tc.text.clone())
                            }
                            _ => None,
                        }
                    })
                    .collect();

                if content.is_empty() {
                    if result.is_error.unwrap_or(false) {
                        Err(format!(
                            "Tool '{}' returned an error with no text content",
                            tool_name
                        ))
                    } else {
                        Ok(format!(
                            "Tool '{}' completed successfully (no text output).",
                            tool_name
                        ))
                    }
                } else {
                    let output = content.join("\n");
                    if result.is_error.unwrap_or(false) {
                        Err(format!(
                            "Tool '{}' returned an error:\n{}",
                            tool_name, output
                        ))
                    } else {
                        Ok(output)
                    }
                }
            }
            Err(e) => {
                // If the SDK error is a protocol error, the server may be dead.
                // We log and disconnect.
                tracing::warn!(
                    "MCP tool call '{}::{}' failed: {}",
                    server_name, tool_name, e
                );
                Err(format!(
                    "MCP tool call '{}::{}' failed: {}",
                    server_name, tool_name, e
                ))
            }
        }
    }

    /// Populate the metadata cache for a connected server by fetching its tool list.
    pub async fn populate_tool_cache(
        self: &Arc<Self>,
        server_name: &str,
        cache: &MetadataCache,
        def: &McpServerDefinition,
    ) -> Result<(), String> {
        let conn = self
            .get_connection(server_name)
            .await
            .ok_or_else(|| format!("Server '{}' not connected", server_name))?;

        let tools = conn
            .list_tools()
            .await
            .map_err(|e| format!("Failed to list tools for '{}': {}", server_name, e))?;

        let hash = duga_config::server_identity_hash(def);

        let cached: Vec<CachedTool> = tools
            .into_iter()
            .map(|t| CachedTool {
                name: t.name,
                description: t.description.unwrap_or_default(),
                input_schema: serde_json::to_value(&t.input_schema).unwrap_or_default(),
            })
            .collect();

        cache.put(server_name, &hash, cached);

        let count = cache.get(server_name, &hash).map(|e| e.tools.len()).unwrap_or(0);
        tracing::info!("MCP tool cache populated for '{}': {} tool(s)", server_name, count);

        Ok(())
    }

    /// Update the last-used timestamp for a server (keep-alive ping).
    pub async fn touch(&self, name: &str) {
        let mut servers = self.servers.lock().await;
        if let Some(state) = servers.get_mut(name) {
            state.last_used = Instant::now();
        }
    }

    /// Check if a server is currently connected.
    pub async fn is_connected(&self, name: &str) -> bool {
        let servers = self.servers.lock().await;
        servers
            .get(name)
            .map(|s| s.connection.is_some())
            .unwrap_or(false)
    }

    /// Check if a server has been idle beyond the given timeout.
    pub async fn is_idle(&self, name: &str, timeout: std::time::Duration) -> bool {
        let servers = self.servers.lock().await;
        servers
            .get(name)
            .map(|s| s.last_used.elapsed() > timeout)
            .unwrap_or(false)
    }

    /// Get the names of all currently connected servers.
    pub async fn connected_names(&self) -> Vec<String> {
        let servers = self.servers.lock().await;
        servers
            .iter()
            .filter(|(_, s)| s.connection.is_some())
            .map(|(k, _)| k.clone())
            .collect()
    }

    /// Close all connections gracefully.
    pub async fn close_all(&self) {
        let names: Vec<String> = self.connected_names().await;
        for name in names {
            let _ = self.close(&name).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_manager_new() {
        let mgr = ServerManager::new();
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            assert!(mgr.connected_names().await.is_empty());
        });
    }

    #[test]
    fn test_is_idle_on_nonexistent() {
        let mgr = ServerManager::new();
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            assert!(!mgr.is_idle("nonexistent", std::time::Duration::from_secs(1)).await);
        });
    }

    #[test]
    fn test_is_connected_on_nonexistent() {
        let mgr = ServerManager::new();
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            assert!(!mgr.is_connected("nonexistent").await);
        });
    }
}
