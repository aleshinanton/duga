//! `McpProxyTool` — the main `mcp()` proxy tool.
//!
//! Implements `duga_tools::Tool` so it can be registered as an `ErasedTool`
//! in the `ToolDispatcher`. Dispatches to the appropriate proxy mode
//! based on the provided arguments.
//!
//! Args shape:
//! ```json
//! {
//!   "tool": "tool_name",        // call a specific MCP tool
//!   "args": "{\"key\":\"val\"}", // JSON string args for the tool
//!   "connect": "server_name",   // connect to a server
//!   "describe": "tool_name",    // describe a tool
//!   "search": "query",          // search for tools
//!   "server": "name",           // server status (or omit for all)
//!   "regex": false,             // use regex in search
//!   "includeSchemas": false,    // include schemas in describe
//!   "label": ""                 // optional label
//! }
//! ```

use crate::cache::MetadataCache;
use crate::manager::ServerManager;
use crate::naming::ToolPrefixMode;
use crate::proxy_modes;
use duga_config::McpServerDefinition;
use duga_tools::context::ToolContext;
use duga_tools::result::ToolCallResult;
use duga_tools::Tool;
use duga_types::error::ToolError;
use duga_types::tool_call::CallId;
use duga_types::tool_result::ToolResultBuilder;
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

/// Arguments for the `mcp()` proxy tool.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct McpProxyArgs {
    /// Name of the tool to call via MCP.
    #[serde(default)]
    pub tool: Option<String>,
    /// JSON string of arguments to pass to the tool.
    #[serde(default)]
    pub args: Option<String>,
    /// Name of the MCP server to connect to.
    #[serde(default)]
    pub connect: Option<String>,
    /// Name of the tool to describe (fuzzy match).
    #[serde(default)]
    pub describe: Option<String>,
    /// Search query for finding tools.
    #[serde(default)]
    pub search: Option<String>,
    /// Server name to get status for (omit for all servers).
    #[serde(default)]
    pub server: Option<String>,
    /// Use regex pattern in search mode.
    #[serde(default)]
    pub regex: Option<bool>,
    /// Include JSON schemas in describe output.
    #[serde(default)]
    #[serde(rename = "includeSchemas")]
    pub include_schemas: Option<bool>,
    /// Optional user-facing label (for logging).
    #[serde(default)]
    pub label: Option<String>,
}

/// The `mcp()` proxy tool.
///
/// Registered as `mcp` in the tool dispatcher. Provides a single
/// entry point for all MCP operations: connect, search, describe,
/// server status, and tool calling.
pub struct McpProxyTool {
    /// Server manager for connections.
    manager: Arc<ServerManager>,
    /// Metadata cache for tool listings.
    cache: Arc<MetadataCache>,
    /// Server definitions by name.
    definitions: Arc<HashMap<String, McpServerDefinition>>,
    /// Tool name prefix mode.
    prefix_mode: ToolPrefixMode,
    /// Stored description (dynamic, built from server list at construction).
    description: String,
}

impl McpProxyTool {
    pub fn new(
        manager: Arc<ServerManager>,
        cache: Arc<MetadataCache>,
        definitions: HashMap<String, McpServerDefinition>,
        prefix_mode: ToolPrefixMode,
    ) -> Self {
        let description = Self::build_description(&definitions);
        Self {
            manager,
            cache,
            definitions: Arc::new(definitions),
            prefix_mode,
            description,
        }
    }

    /// Build the description dynamically from the server list.
    pub fn build_description(definitions: &HashMap<String, McpServerDefinition>) -> String {
        let server_names: Vec<&str> = definitions.keys().map(|k| k.as_str()).collect();
        let server_list = if server_names.is_empty() {
            "none configured".to_string()
        } else {
            server_names.join(", ")
        };
        format!(
            "MCP proxy tool — interact with configured MCP servers ({}). \
             Modes: tool (call a tool), connect (connect to a server), \
             describe (get tool details with schema), search (find tools across servers), \
             server (server status), or no mode (list all tools). \
             Use `connect` first to establish a connection, then `search` or list to discover tools.",
            server_list
        )
    }
}

impl Tool for McpProxyTool {
    type Args = McpProxyArgs;

    fn name(&self) -> &str {
        "mcp"
    }

    fn description(&self) -> &str {
        &self.description
    }

    async fn execute(
        &self,
        _ctx: ToolContext<'_>,
        args: Self::Args,
    ) -> ToolCallResult {
        let result = execute_inner(
            &self.manager,
            &self.cache,
            &self.definitions,
            &self.prefix_mode,
            args,
        )
        .await;

        match result {
            Ok(output) => {
                let call_id = CallId::new();
                Ok(ToolResultBuilder::new()
                    .tool_call_id(call_id)
                    .success(true)
                    .output(output)
                    .build()
                    .unwrap())
            }
            Err(e) => Err(e),
        }
    }
}

async fn execute_inner(
    manager: &Arc<ServerManager>,
    cache: &Arc<MetadataCache>,
    definitions: &HashMap<String, McpServerDefinition>,
    prefix: &ToolPrefixMode,
    args: McpProxyArgs,
) -> Result<String, ToolError> {
    // Dispatch priority: tool > connect > describe > search > server > status

    if let Some(ref tool_name) = args.tool {
        let tool_args = args.args.as_deref().unwrap_or("{}");
        return proxy_modes::execute_call(manager, cache, definitions, prefix, tool_name, tool_args)
            .await;
    }

    if let Some(ref server_name) = args.connect {
        return proxy_modes::execute_connect(manager, cache, definitions, server_name).await;
    }

    if let Some(ref tool_name) = args.describe {
        let include_schemas = args.include_schemas.unwrap_or(false);
        return proxy_modes::execute_describe(
            cache,
            definitions,
            prefix,
            tool_name,
            include_schemas,
        )
        .await;
    }

    if let Some(ref query) = args.search {
        let use_regex = args.regex.unwrap_or(false);
        return proxy_modes::execute_search(cache, definitions, prefix, query, use_regex).await;
    }

    if let Some(ref name) = args.server {
        return proxy_modes::execute_status(manager, definitions, Some(name)).await;
    }

    // Default: show all tools.
    proxy_modes::execute_list(cache, definitions, prefix).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use duga_config::McpLifecycleMode;

    fn make_defs() -> HashMap<String, McpServerDefinition> {
        let mut map = HashMap::new();
        map.insert(
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
        map
    }

    #[test]
    fn test_proxy_tool_name_and_description() {
        let manager = ServerManager::new();
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(
            MetadataCache::with_path(dir.path().join("cache.json")).unwrap(),
        );
        let defs = make_defs();

        let tool = McpProxyTool::new(manager, cache, defs, ToolPrefixMode::Server);
        assert_eq!(tool.name(), "mcp");
        assert!(tool.description().contains("test_srv"));
        assert!(tool.description().contains("MCP proxy tool"));
    }

    #[test]
    fn test_build_description_empty() {
        let defs = HashMap::new();
        let desc = McpProxyTool::build_description(&defs);
        assert!(desc.contains("none configured"));
    }

    #[test]
    fn test_build_description_with_servers() {
        let defs = make_defs();
        let desc = McpProxyTool::build_description(&defs);
        assert!(desc.contains("test_srv"));
        assert!(desc.contains("MCP proxy tool"));
    }
}
