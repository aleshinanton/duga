//! Metadata cache for MCP server tool lists.
//!
//! Disk-backed JSON at `~/.duga/mcp-cache.json`.
//! Caches tool schemas per server with a config hash for invalidation.
//! Valid entries: version matches, hash matches, and age < 7 days.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Current cache schema version. Bump to invalidate all cached entries.
const CACHE_VERSION: &str = "1";

/// Maximum age for a valid cache entry.
const MAX_CACHE_AGE: Duration = Duration::from_secs(7 * 24 * 3600);

/// A cached tool entry — minimal schema for re-registration.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CachedTool {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// Per-server cache entry.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ServerCacheEntry {
    /// Cached tool list.
    pub tools: Vec<CachedTool>,
    /// SHA-256 identity hash of the server definition at cache time.
    pub config_hash: String,
    /// Cache schema version at write time.
    pub version: String,
    /// Unix timestamp (seconds) when this entry was written.
    pub cached_at_secs: u64,
}

impl ServerCacheEntry {
    /// Check if this cache entry is still valid.
    pub fn is_valid(&self, current_hash: &str) -> bool {
        // Version must match.
        if self.version != CACHE_VERSION {
            return false;
        }
        // Config hash must match.
        if self.config_hash != current_hash {
            return false;
        }
        // Age must be within limit.
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if now_secs.saturating_sub(self.cached_at_secs) > MAX_CACHE_AGE.as_secs() {
            return false;
        }
        true
    }
}

/// Full cache file structure.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct McpCache {
    /// Per-server entries keyed by server name.
    servers: HashMap<String, ServerCacheEntry>,
}

/// In-memory cache handle with disk backing.
pub struct MetadataCache {
    /// Path to the cache file on disk.
    path: PathBuf,
    /// In-memory cache data.
    inner: RwLock<McpCache>,
}

impl MetadataCache {
    /// Load or create the cache at `~/.duga/mcp-cache.json`.
    pub fn load() -> Result<Self, CacheError> {
        let path = cache_path();
        let inner = if path.exists() {
            let raw = std::fs::read_to_string(&path)
                .map_err(|e| CacheError::Io(format!("reading cache: {}", e)))?;
            serde_json::from_str(&raw).unwrap_or_default()
        } else {
            McpCache::default()
        };

        Ok(Self {
            path,
            inner: RwLock::new(inner),
        })
    }

    /// Get a cached entry for a server, if valid.
    pub fn get(&self, server_name: &str, current_hash: &str) -> Option<ServerCacheEntry> {
        let cache = self.inner.read().unwrap();
        cache
            .servers
            .get(server_name)
            .filter(|entry| entry.is_valid(current_hash))
            .cloned()
    }

    /// Store or update a cached entry for a server.
    pub fn put(&self, server_name: &str, config_hash: &str, tools: Vec<CachedTool>) {
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let entry = ServerCacheEntry {
            tools,
            config_hash: config_hash.to_string(),
            version: CACHE_VERSION.to_string(),
            cached_at_secs: now_secs,
        };
        let mut cache = self.inner.write().unwrap();
        cache.servers.insert(server_name.to_string(), entry);
    }

    /// Flush the in-memory cache to disk atomically.
    pub fn flush(&self) -> Result<(), CacheError> {
        let cache = self.inner.read().unwrap();
        let json = serde_json::to_string_pretty(&*cache)
            .map_err(|e| CacheError::Serialize(format!("serializing cache: {}", e)))?;

        // Ensure parent directory exists.
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CacheError::Io(format!("creating cache dir: {}", e)))?;
        }

        // Atomic write via .tmp → rename().
        let tmp_path = self.path.with_extension("json.tmp");
        std::fs::write(&tmp_path, &json)
            .map_err(|e| CacheError::Io(format!("writing cache: {}", e)))?;
        std::fs::rename(&tmp_path, &self.path)
            .map_err(|e| CacheError::Io(format!("renaming cache: {}", e)))?;

        Ok(())
    }

    /// Get path for testing.
    pub fn with_path(path: PathBuf) -> Result<Self, CacheError> {
        let inner = if path.exists() {
            let raw = std::fs::read_to_string(&path)
                .map_err(|e| CacheError::Io(format!("reading cache: {}", e)))?;
            serde_json::from_str(&raw).unwrap_or_default()
        } else {
            McpCache::default()
        };
        Ok(Self {
            path,
            inner: RwLock::new(inner),
        })
    }
}

/// Determine the default cache path: `~/.duga/mcp-cache.json`.
fn cache_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".duga").join("mcp-cache.json")
}

#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("I/O error: {0}")]
    Io(String),
    #[error("Serialization error: {0}")]
    Serialize(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_cache_path(dir: &TempDir) -> PathBuf {
        dir.path().join("test-cache.json")
    }

    #[test]
    fn test_cache_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_cache_path(&dir);
        let cache = MetadataCache::with_path(path.clone()).unwrap();

        let hash = "abc123";
        let tools = vec![CachedTool {
            name: "test_tool".into(),
            description: "A test tool".into(),
            input_schema: serde_json::json!({"type": "object"}),
        }];

        cache.put("test_server", hash, tools.clone());
        cache.flush().unwrap();

        // Read back from a new cache instance.
        let cache2 = MetadataCache::with_path(path).unwrap();
        let entry = cache2.get("test_server", hash);
        assert!(entry.is_some());
        let entry = entry.unwrap();
        assert_eq!(entry.tools.len(), 1);
        assert_eq!(entry.tools[0].name, "test_tool");
    }

    #[test]
    fn test_cache_invalid_on_hash_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_cache_path(&dir);
        let cache = MetadataCache::with_path(path.clone()).unwrap();

        cache.put(
            "test_server",
            "hash_old",
            vec![CachedTool {
                name: "tool".into(),
                description: "desc".into(),
                input_schema: serde_json::json!({}),
            }],
        );
        cache.flush().unwrap();

        let cache2 = MetadataCache::with_path(path).unwrap();
        let entry = cache2.get("test_server", "hash_new");
        assert!(entry.is_none());
    }

    #[test]
    fn test_cache_valid_on_hash_match() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_cache_path(&dir);
        let cache = MetadataCache::with_path(path.clone()).unwrap();

        cache.put(
            "srv",
            "the_hash",
            vec![CachedTool {
                name: "t".into(),
                description: "d".into(),
                input_schema: serde_json::json!({}),
            }],
        );
        cache.flush().unwrap();

        let cache2 = MetadataCache::with_path(path).unwrap();
        assert!(cache2.get("srv", "the_hash").is_some());
    }

    #[test]
    fn test_cache_missing_server_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_cache_path(&dir);
        let cache = MetadataCache::with_path(path).unwrap();
        assert!(cache.get("nonexistent", "any").is_none());
    }

    #[test]
    fn test_cache_new_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_cache_path(&dir);
        let cache = MetadataCache::with_path(path).unwrap();
        assert!(cache.get("anything", "anything").is_none());
    }
}
