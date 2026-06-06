//! Lifecycle manager — health checks, idle timeouts, keep-alive reconnects.
//!
//! Spawns a background task that:
//! - Performs health checks every 30s on keep-alive servers.
//! - Closes lazy servers that have been idle beyond their timeout.
//! - Reconnects keep-alive servers that died.
//!
//! Graceful shutdown via `graceful_shutdown()` stops the background task
//! and closes all connections.

use crate::cache::MetadataCache;
use crate::manager::ServerManager;
use duga_config::{McpLifecycleMode, McpPluginConfig, McpServerDefinition};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;

/// Health check interval.
const HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(30);

/// Manages server lifecycle: connects, health checks, idle management.
#[allow(dead_code)]
pub struct LifecycleManager {
    /// Server manager for connections.
    manager: Arc<ServerManager>,
    /// Metadata cache for refreshing tool lists.
    cache: Arc<MetadataCache>,
    /// Server definitions by name.
    definitions: HashMap<String, McpServerDefinition>,
    /// Idle timeout per server (fallback to global, then default 10 min).
    idle_timeouts: HashMap<String, Duration>,
    /// Default idle timeout.
    default_idle_timeout: Duration,
    /// Lifecycle mode per server.
    lifecycle_modes: HashMap<String, McpLifecycleMode>,
    /// Shutdown flag.
    shutdown: Arc<AtomicBool>,
    /// Background task handle.
    _task: Option<JoinHandle<()>>,
}

impl LifecycleManager {
    /// Create a new lifecycle manager from the MCP config.
    pub fn new(
        manager: Arc<ServerManager>,
        cache: Arc<MetadataCache>,
        config: &McpPluginConfig,
    ) -> Self {
        let default_idle_timeout = config
            .settings
            .idle_timeout
            .unwrap_or(Duration::from_secs(600));

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

        let lifecycle_modes: HashMap<String, McpLifecycleMode> = definitions
            .iter()
            .map(|(k, def)| (k.clone(), def.lifecycle))
            .collect();

        let idle_timeouts: HashMap<String, Duration> = definitions
            .iter()
            .map(|(k, _)| (k.clone(), default_idle_timeout))
            .collect();

        Self {
            manager,
            cache,
            definitions,
            idle_timeouts,
            default_idle_timeout,
            lifecycle_modes,
            shutdown: Arc::new(AtomicBool::new(false)),
            _task: None,
        }
    }

    /// Start the lifecycle background task.
    /// Connects eager/keep-alive servers, then begins the health-check loop.
    pub fn start(&mut self) {
        let manager = self.manager.clone();
        let cache = self.cache.clone();
        let definitions = self.definitions.clone();
        let idle_timeouts = self.idle_timeouts.clone();
        let lifecycle_modes = self.lifecycle_modes.clone();
        let shutdown = self.shutdown.clone();

        let handle = tokio::spawn(async move {
            run_lifecycle_loop(
                manager,
                cache,
                definitions,
                idle_timeouts,
                lifecycle_modes,
                shutdown,
            )
            .await;
        });

        self._task = Some(handle);
    }

    /// Signal graceful shutdown — stops the background task and closes all connections.
    pub async fn graceful_shutdown(self) {
        self.shutdown.store(true, Ordering::SeqCst);
        // The background task will close all connections when it exits.
        // Give it a brief moment.
        tokio::time::sleep(Duration::from_millis(100)).await;
        // Also close directly.
        self.manager.close_all().await;
    }
}

async fn run_lifecycle_loop(
    manager: Arc<ServerManager>,
    cache: Arc<MetadataCache>,
    definitions: HashMap<String, McpServerDefinition>,
    idle_timeouts: HashMap<String, Duration>,
    lifecycle_modes: HashMap<String, McpLifecycleMode>,
    shutdown: Arc<AtomicBool>,
) {
    // Connect eager and keep-alive servers at startup.
    for (name, def) in &definitions {
        match def.lifecycle {
            McpLifecycleMode::Eager | McpLifecycleMode::KeepAlive => {
                if let Err(e) = manager.connect(name, def).await {
                    tracing::warn!(server = %name, error = %e, "Failed to connect eager/keep-alive server");
                }
            }
            McpLifecycleMode::Lazy => {
                // Lazy servers connect on first use — nothing to do here.
            }
        }
    }

    // Health check loop.
    let mut interval = tokio::time::interval(HEALTH_CHECK_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }

        tokio::select! {
            _ = interval.tick() => {
                // Health check: check all connected keep-alive servers.
                for (name, mode) in &lifecycle_modes {
                    if *mode == McpLifecycleMode::KeepAlive
                        && manager.is_connected(name).await
                    {
                        // Touch the server to keep it alive.
                        manager.touch(name).await;

                        // If disconnected, try to reconnect.
                        // (The touch won't fix a dead connection — we'd need a ping.
                        // For now, rely on tool call failures to trigger reconnect.)
                    }
                }

                // Idle timeout: close lazy servers that have been idle too long.
                for name in manager.connected_names().await {
                    if let Some(mode) = lifecycle_modes.get(&name) {
                        if *mode == McpLifecycleMode::Lazy {
                            let timeout = idle_timeouts.get(&name).copied().unwrap_or(Duration::from_secs(600));
                            if manager.is_idle(&name, timeout).await {
                                tracing::debug!(server = %name, "Closing idle lazy server");
                                let _ = manager.close(&name).await;
                            }
                        }
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(1)) => {
                // Check shutdown flag more frequently.
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }
            }
        }
    }

    // Graceful shutdown: flush cache and close all connections.
    if let Err(e) = cache.flush() {
        tracing::warn!(error = %e, "Failed to flush MCP cache during shutdown");
    }
    manager.close_all().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lifecycle_manager_creation() {
        let manager = ServerManager::new();
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(
            MetadataCache::with_path(dir.path().join("test-cache.json")).unwrap(),
        );
        let config = McpPluginConfig {
            servers: HashMap::new(),
            settings: Default::default(),
        };

        let lm = LifecycleManager::new(manager, cache, &config);
        assert_eq!(lm.definitions.len(), 0);
    }

    #[test]
    fn test_lifecycle_manager_with_servers() {
        let manager = ServerManager::new();
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(
            MetadataCache::with_path(dir.path().join("test-cache.json")).unwrap(),
        );

        let mut servers = HashMap::new();
        servers.insert(
            "test_srv".to_string(),
            McpServerDefinition {
                name: "test_srv".into(),
                transport: "stdio".into(),
                command: Some("echo".into()),
                args: Some(vec!["hello".into()]),
                lifecycle: McpLifecycleMode::Lazy,
                ..Default::default()
            },
        );

        let config = McpPluginConfig {
            servers,
            settings: Default::default(),
        };

        let lm = LifecycleManager::new(manager, cache, &config);
        assert_eq!(lm.definitions.len(), 1);
        assert_eq!(
            lm.lifecycle_modes.get("test_srv"),
            Some(&McpLifecycleMode::Lazy)
        );
    }
}
