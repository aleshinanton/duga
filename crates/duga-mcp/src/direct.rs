//! Direct tools registrar.
//!
//! For MCP servers configured with `direct_tools`, builds `ErasedTool`
//! wrappers for each promoted tool using cached metadata. This allows
//! frequently-used MCP tools to be called directly instead of via the
//! `mcp()` proxy tool, saving tokens.

use crate::cache::{CachedTool, MetadataCache};
use crate::naming::{is_excluded, prefix_tool, ToolPrefixMode};
use duga_config::{McpDirectToolsMode, McpServerDefinition};
use duga_tools::context::ToolContext;
use duga_tools::result::ToolCallResult;
use duga_tools::ErasedTool;
use duga_tools::Tool;
use duga_types::error::ToolError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// A direct tool wrapper — implements `Tool` for a single MCP tool.
/// When executed, it calls the MCP server via the proxy adapter.
#[allow(dead_code)]
struct DirectMCPTool {
    /// Prefixed tool name (e.g. "github_search_repos").
    name: String,
    /// Description from the tool's schema.
    description: String,
    /// Input schema from the tool.
    args_schema: serde_json::Value,
    /// Server name so the adapter can route the call.
    server_name: String,
    /// Original tool name on the server.
    original_name: String,
}

impl DirectMCPTool {
    fn new(
        name: String,
        description: String,
        args_schema: serde_json::Value,
        server_name: String,
        original_name: String,
    ) -> Self {
        Self {
            name,
            description,
            args_schema,
            server_name,
            original_name,
        }
    }
}

// Use a newtype wrapper that implements JsonSchema for raw JSON args.
#[derive(Debug, Default, Deserialize, Serialize)]
struct DirectMCPArgs {
    #[serde(flatten)]
    raw: serde_json::Value,
}

impl schemars::JsonSchema for DirectMCPArgs {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "DirectMCPArgs".into()
    }

    fn json_schema(_gen: &mut schemars::SchemaGenerator) -> schemars::Schema {
        // Return a simple object schema for flexible args.
        let mut schema = serde_json::Map::new();
        schema.insert("type".into(), serde_json::Value::String("object".into()));
        schema.insert(
            "additionalProperties".into(),
            serde_json::Value::Bool(true),
        );
        schemars::Schema::from(schema)
    }
}

impl Tool for DirectMCPTool {
    type Args = DirectMCPArgs;

    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn json_schema(&self) -> serde_json::Value {
        self.args_schema.clone()
    }

    async fn execute(
        &self,
        _ctx: ToolContext<'_>,
        _args: Self::Args,
    ) -> ToolCallResult {
        // Direct tools are "thin" — actual execution happens through
        // the McpAdapter which intercepts these calls.
        // For now, return a helpful error that this needs wiring.
        Err(ToolError::InvalidArgs(format!(
            "Direct MCP tool '{}' requires the MCP adapter to be wired. \
             Use mcp({{tool: \"{}\", args: ...}}) instead.",
            self.name, self.original_name
        )))
    }
}

/// Resolve direct tools from config + cache.
///
/// For each server with `direct_tools` configured and a valid cache entry,
/// builds `ErasedTool` wrappers for tools that aren't excluded.
///
/// Returns a list of `ErasedTool` ready for registration in the dispatcher.
pub fn resolve_direct_tools(
    config: &HashMap<String, McpServerDefinition>,
    cache: &Arc<MetadataCache>,
    prefix: &ToolPrefixMode,
    global_direct: &Option<McpDirectToolsMode>,
) -> Vec<ErasedTool> {
    let mut tools = Vec::new();

    for (server_name, def) in config {
        // Determine which tools to promote for this server.
        let direct_mode = def.direct_tools.as_ref().or(global_direct.as_ref());

        let promote_all = match direct_mode {
            Some(McpDirectToolsMode::All(true)) => true,
            Some(McpDirectToolsMode::All(false)) | None => continue,
            Some(McpDirectToolsMode::List(names)) => {
                let hash = duga_config::server_identity_hash(def);
                if let Some(entry) = cache.get(server_name, &hash) {
                    for cached_tool in &entry.tools {
                        let prefixed = prefix_tool(server_name, &cached_tool.name, prefix);
                        if !is_excluded(&cached_tool.name, &prefixed, &def.exclude_tools)
                            && names.contains(&cached_tool.name)
                        {
                            tools.push(build_direct_tool(
                                server_name,
                                cached_tool,
                                prefix,
                                def,
                            ));
                        }
                    }
                }
                continue;
            }
        };

        if promote_all {
            let hash = duga_config::server_identity_hash(def);
            if let Some(entry) = cache.get(server_name, &hash) {
                for cached_tool in &entry.tools {
                    let prefixed = prefix_tool(server_name, &cached_tool.name, prefix);
                    if !is_excluded(&cached_tool.name, &prefixed, &def.exclude_tools) {
                        tools.push(build_direct_tool(server_name, cached_tool, prefix, def));
                    }
                }
            }
        }
    }

    tools
}

fn build_direct_tool(
    server_name: &str,
    cached_tool: &CachedTool,
    prefix: &ToolPrefixMode,
    _def: &McpServerDefinition,
) -> ErasedTool {
    let prefixed_name = prefix_tool(server_name, &cached_tool.name, prefix);

    let direct_tool = DirectMCPTool::new(
        prefixed_name,
        cached_tool.description.clone(),
        cached_tool.input_schema.clone(),
        server_name.to_string(),
        cached_tool.name.clone(),
    );

    ErasedTool::erase(direct_tool)
}

#[cfg(test)]
mod tests {
    use super::*;
    use duga_config::McpLifecycleMode;

    fn make_cache_with_tools() -> (MetadataCache, HashMap<String, McpServerDefinition>) {
        let dir = tempfile::tempdir().unwrap();
        let cache = MetadataCache::with_path(dir.path().join("test-cache.json")).unwrap();

        let mut defs = HashMap::new();
        let def = McpServerDefinition {
            name: "test_srv".into(),
            transport: "stdio".into(),
            command: Some("echo".into()),
            args: Some(vec!["hello".into()]),
            lifecycle: McpLifecycleMode::Lazy,
            exclude_tools: vec![],
            direct_tools: Some(McpDirectToolsMode::All(true)),
            ..Default::default()
        };

        let hash = duga_config::server_identity_hash(&def);
        cache.put(
            "test_srv",
            &hash,
            vec![
                CachedTool {
                    name: "tool_a".into(),
                    description: "Tool A".into(),
                    input_schema: serde_json::json!({"type": "object"}),
                },
                CachedTool {
                    name: "tool_b".into(),
                    description: "Tool B".into(),
                    input_schema: serde_json::json!({"type": "object"}),
                },
            ],
        );

        defs.insert("test_srv".to_string(), def);
        (cache, defs)
    }

    #[test]
    fn test_resolve_direct_tools_all() {
        let (cache, defs) = make_cache_with_tools();
        let cache = Arc::new(cache);
        let tools = resolve_direct_tools(&defs, &cache, &ToolPrefixMode::Server, &None);

        assert_eq!(tools.len(), 2);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"test_srv_tool_a"));
        assert!(names.contains(&"test_srv_tool_b"));
    }

    #[test]
    fn test_resolve_direct_tools_list() {
        let dir = tempfile::tempdir().unwrap();
        let cache = MetadataCache::with_path(dir.path().join("cache.json")).unwrap();

        let def = McpServerDefinition {
            name: "srv".into(),
            transport: "stdio".into(),
            command: Some("echo".into()),
            lifecycle: McpLifecycleMode::Lazy,
            exclude_tools: vec![],
            direct_tools: Some(McpDirectToolsMode::List(vec!["tool_a".into()])),
            ..Default::default()
        };
        let hash = duga_config::server_identity_hash(&def);
        cache.put(
            "srv",
            &hash,
            vec![
                CachedTool {
                    name: "tool_a".into(),
                    description: "A".into(),
                    input_schema: serde_json::json!({}),
                },
                CachedTool {
                    name: "tool_b".into(),
                    description: "B".into(),
                    input_schema: serde_json::json!({}),
                },
            ],
        );

        let mut defs = HashMap::new();
        defs.insert("srv".to_string(), def);

        let cache = Arc::new(cache);
        let tools = resolve_direct_tools(&defs, &cache, &ToolPrefixMode::Server, &None);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "srv_tool_a");
    }

    #[test]
    fn test_resolve_direct_tools_excluded() {
        let dir = tempfile::tempdir().unwrap();
        let cache = MetadataCache::with_path(dir.path().join("cache.json")).unwrap();

        let def = McpServerDefinition {
            name: "srv".into(),
            transport: "stdio".into(),
            command: Some("echo".into()),
            lifecycle: McpLifecycleMode::Lazy,
            exclude_tools: vec!["tool_b".into()],
            direct_tools: Some(McpDirectToolsMode::All(true)),
            ..Default::default()
        };
        let hash = duga_config::server_identity_hash(&def);
        cache.put(
            "srv",
            &hash,
            vec![
                CachedTool {
                    name: "tool_a".into(),
                    description: "A".into(),
                    input_schema: serde_json::json!({}),
                },
                CachedTool {
                    name: "tool_b".into(),
                    description: "B".into(),
                    input_schema: serde_json::json!({}),
                },
            ],
        );

        let mut defs = HashMap::new();
        defs.insert("srv".to_string(), def);

        let cache = Arc::new(cache);
        let tools = resolve_direct_tools(&defs, &cache, &ToolPrefixMode::Server, &None);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "srv_tool_a");
    }

    #[test]
    fn test_resolve_direct_tools_no_cache() {
        let mut defs = HashMap::new();
        defs.insert(
            "srv".to_string(),
            McpServerDefinition {
                name: "srv".into(),
                transport: "stdio".into(),
                command: Some("echo".into()),
                lifecycle: McpLifecycleMode::Lazy,
                direct_tools: Some(McpDirectToolsMode::All(true)),
                ..Default::default()
            },
        );

        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(
            MetadataCache::with_path(dir.path().join("cache.json")).unwrap(),
        );
        let tools = resolve_direct_tools(&defs, &cache, &ToolPrefixMode::Server, &None);
        assert!(tools.is_empty());
    }
}
