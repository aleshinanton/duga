//! `McpAdapter` — main bootstrap and lifecycle coordinator.
//!
//! Ties together all MCP components: cache, manager, lifecycle, naming,
//! proxy tool, and direct tools. Provides the bootstrap entry point
//! called from `build_dispatcher()` and lifecycle management called
//! from frontends.

use crate::cache::MetadataCache;
use crate::direct::resolve_direct_tools;
use crate::lifecycle::LifecycleManager;
use crate::manager::ServerManager;
use crate::naming::ToolPrefixMode;
use crate::proxy::McpProxyTool;
use duga_config::{McpPluginConfig, McpServerDefinition};
use duga_tools::ErasedTool;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;

/// The MCP adapter — coordinates all MCP server connections and tools.
#[allow(dead_code)]
pub struct McpAdapter {
    /// Server connection manager.
    pub manager: Arc<ServerManager>,
    /// Metadata cache.
    pub cache: Arc<MetadataCache>,
    /// Server definitions by name.
    definitions: HashMap<String, McpServerDefinition>,
    /// Tool name prefix mode.
    prefix_mode: ToolPrefixMode,
    /// Lifecycle manager (set after session_start).
    lifecycle: Mutex<Option<LifecycleManager>>,
    /// Generation counter for stale init result discard.
    generation: AtomicU64,
}

impl McpAdapter {
    /// Bootstrap the MCP adapter.
    ///
    /// This is a **synchronous** operation that:
    /// 1. Loads the metadata cache from disk.
    /// 2. Resolves direct tools from config + cache.
    /// 3. Builds the `mcp()` proxy tool.
    ///
    /// Returns `(Self, Vec<ErasedTool>)` where the tools should be
    /// registered in the `ToolDispatcher`.
    pub fn bootstrap(
        config: &McpPluginConfig,
    ) -> Result<(Arc<Self>, Vec<ErasedTool>), String> {
        let prefix_mode = ToolPrefixMode::from_config(
            config.settings.tool_prefix.as_deref(),
        );

        let cache = Arc::new(
            MetadataCache::load().map_err(|e| format!("Failed to load MCP cache: {}", e))?,
        );

        let manager = ServerManager::new();

        // Build definitions with names filled in.
        let definitions: HashMap<String, McpServerDefinition> = config
            .servers
            .iter()
            .map(|(k, v)| {
                let mut def = v.clone();
                if def.name.is_empty() {
                    def.name = k.clone();
                }
                (k.clone(), def)
            })
            .collect();

        // Populate cache with empty entries for servers without cached data.
        // This allows direct tools to work from cache even without
        // a connection (the cache was written by a previous session).
        for (name, def) in &definitions {
            let hash = duga_config::server_identity_hash(def);
            // Only put if not already cached (don't overwrite valid cache).
            if cache.get(name, &hash).is_none() {
                // Don't put empty — just skip servers with no cache.
            }
        }

        let adapter = Arc::new(Self {
            manager,
            cache,
            definitions,
            prefix_mode: prefix_mode.clone(),
            lifecycle: Mutex::new(None),
            generation: AtomicU64::new(0),
        });

        let mut tools: Vec<ErasedTool> = Vec::new();

        // Register the mcp() proxy tool (unless disabled).
        if !config.settings.disable_proxy_tool {
            let proxy_tool = McpProxyTool::new(
                adapter.manager.clone(),
                adapter.cache.clone(),
                adapter.definitions.clone(),
                prefix_mode.clone(),
            );
            tools.push(ErasedTool::erase(proxy_tool));
        }

        // Register direct tools from cache.
        let direct_tools = resolve_direct_tools(
            &adapter.definitions,
            &adapter.cache,
            &prefix_mode,
            &config.settings.direct_tools,
        );
        tools.extend(direct_tools);

        Ok((adapter, tools))
    }

    /// Start a new session: connect eager/keep-alive servers, start health checks.
    ///
    /// Increments the generation counter to discard stale init results.
    /// Any previous lifecycle manager is gracefully shut down.
    pub async fn session_start(self: &Arc<Self>) {
        let generation_val = self.generation.fetch_add(1, Ordering::SeqCst) + 1;

        // Shut down any previous lifecycle manager.
        {
            let mut lc = self.lifecycle.lock().await;
            if let Some(prev) = lc.take() {
                prev.graceful_shutdown().await;
            }

            // Build the MCP config for lifecycle manager.
            let mcp_config = McpPluginConfig {
                servers: self.definitions.clone(),
                settings: Default::default(),
            };

            // Create and start a new lifecycle manager.
            let mut new_lc = LifecycleManager::new(
                self.manager.clone(),
                self.cache.clone(),
                &mcp_config,
            );
            new_lc.start();

            *lc = Some(new_lc);
        }

        // Connect eager/keep-alive servers and populate metadata.
        for (name, def) in &self.definitions {
            match def.lifecycle {
                duga_config::McpLifecycleMode::Eager | duga_config::McpLifecycleMode::KeepAlive => {
                    let result = self.manager.connect(name, def).await;
                    if let Err(ref e) = result {
                        tracing::warn!("Failed to connect server '{}' at session start: {}", name, e);
                    }
                }
                duga_config::McpLifecycleMode::Lazy => {
                    // Lazy servers connect on first use.
                }
            }
        }

        // Populate metadata from connected servers.
        let connected = self.manager.connected_names().await;
        for name in &connected {
            if let Some(def) = self.definitions.get(name) {
                let _ = self.populate_metadata(name, def).await;
            }
        }

        tracing::info!(
            "MCP session started (generation={}), connected servers: {:?}",
            generation_val, connected
        );
    }

    /// End the current session: flush cache, shutdown lifecycle.
    pub async fn session_shutdown(self: &Arc<Self>) {
        let mut lc = self.lifecycle.lock().await;
        if let Some(mgr) = lc.take() {
            mgr.graceful_shutdown().await;
        }

        // Close all remaining connections.
        self.manager.close_all().await;

        // Flush cache to disk.
        if let Err(e) = self.cache.flush() {
            tracing::warn!("Failed to flush MCP cache during shutdown: {}", e);
        }

        self.generation.fetch_add(1, Ordering::SeqCst);

        tracing::info!("MCP session shut down");
    }

    /// Populate metadata (tool list) for a connected server.
    /// This fetches the tool list from the server and caches it.
    async fn populate_metadata(
        &self,
        name: &str,
        def: &McpServerDefinition,
    ) -> Result<(), String> {
        let hash = duga_config::server_identity_hash(def);

        // If we already have a valid cache entry, skip.
        if self.cache.get(name, &hash).is_some() {
            return Ok(());
        }

        // Delegate to the manager to fetch and cache the tool list.
        self.manager.populate_tool_cache(name, &self.cache, def).await
    }

    /// Try to connect and call a tool on an MCP server.
    /// This is the main entry point for calling a tool through the proxy.
    pub async fn call_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        args: serde_json::Map<String, serde_json::Value>,
    ) -> Result<String, String> {
        let def = self.definitions.get(server_name)
            .ok_or_else(|| format!("Unknown server '{}'", server_name))?;

        self.manager.call_tool(server_name, def, tool_name, args).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use duga_config::{McpLifecycleMode, McpSettings};

    fn make_config() -> McpPluginConfig {
        let mut servers = HashMap::new();
        servers.insert(
            "test_srv".into(),
            McpServerDefinition {
                name: "test_srv".into(),
                transport: "stdio".into(),
                command: Some("echo".into()),
                args: Some(vec!["hello".into()]),
                lifecycle: McpLifecycleMode::Lazy,
                ..Default::default()
            },
        );

        McpPluginConfig {
            servers,
            settings: McpSettings::default(),
        }
    }

    #[test]
    fn test_bootstrap_creates_adapter() {
        let config = make_config();
        let result = McpAdapter::bootstrap(&config);
        assert!(result.is_ok());

        let (_adapter, tools) = result.unwrap();
        // Should have at least the mcp() proxy tool.
        assert!(!tools.is_empty());
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"mcp"));
    }

    #[test]
    fn test_bootstrap_disables_proxy_tool() {
        let mut config = make_config();
        config.settings.disable_proxy_tool = true;

        let result = McpAdapter::bootstrap(&config);
        assert!(result.is_ok());

        let (_adapter, tools) = result.unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(!names.contains(&"mcp"));
    }

    #[test]
    fn test_bootstrap_empty_servers() {
        let config = McpPluginConfig {
            servers: HashMap::new(),
            settings: McpSettings::default(),
        };

        let result = McpAdapter::bootstrap(&config);
        assert!(result.is_ok());

        let (_adapter, tools) = result.unwrap();
        // Should still have the mcp() proxy tool.
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"mcp"));
    }
}
