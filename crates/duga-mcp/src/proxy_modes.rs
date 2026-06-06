//! Proxy mode implementations for the `mcp()` proxy tool.
//!
//! Each mode is a standalone function that takes the manager, cache,
//! definitions, prefix mode, and mode-specific args, and returns a
//! formatted result string.

use crate::cache::MetadataCache;
use crate::manager::ServerManager;
use crate::naming::{lookup_tool, prefix_tool, ToolPrefixMode};
use duga_config::McpServerDefinition;
use duga_types::error::ToolError;
use std::collections::HashMap;
use std::sync::Arc;

/// Mode: `tool` — call a specific MCP tool via proxy.
///
/// Finds the server that owns the tool, lazy-connects if needed,
/// calls `tools/call`, and returns the formatted result.
pub async fn execute_call(
    manager: &Arc<ServerManager>,
    cache: &Arc<MetadataCache>,
    definitions: &HashMap<String, McpServerDefinition>,
    prefix: &ToolPrefixMode,
    tool_name: &str,
    args: &str,
) -> Result<String, ToolError> {
    // Build the list of known tools per server.
    let tool_lists = build_tool_lists(cache, definitions);

    // Find the server.
    let server_tools: Vec<(&str, &[String])> = tool_lists
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_slice()))
        .collect();

    let (server_name, raw_tool_name, _prefixed_name) =
        lookup_tool(tool_name, &server_tools, prefix).ok_or_else(|| {
            let available: Vec<String> = server_tools
                .iter()
                .flat_map(|(s, tools)| tools.iter().map(|t| prefix_tool(s, t, prefix)))
                .collect();
            let hint = if available.is_empty() {
                "No MCP tools are currently available. Try connecting to a server first with `mcp({connect: \"server_name\"})`.".to_string()
            } else {
                format!("Available: {}", available.join(", "))
            };
            ToolError::InvalidArgs(format!("Unknown MCP tool '{}'. {}", tool_name, hint))
        })?;

    // Get the server definition.
    let def = definitions.get(server_name).ok_or_else(|| {
        ToolError::InvalidArgs(format!("Unknown server '{}'", server_name))
    })?;

    // Parse args into a JSON Map for the MCP call.
    let args_map: serde_json::Map<String, serde_json::Value> = if args.trim().is_empty() {
        serde_json::Map::new()
    } else {
        let parsed: serde_json::Value = serde_json::from_str(args).map_err(|e| {
            ToolError::InvalidArgs(format!("Invalid JSON args: {}", e))
        })?;
        match parsed {
            serde_json::Value::Object(map) => map,
            _ => {
                return Err(ToolError::InvalidArgs(
                    "Arguments must be a JSON object (e.g. {\"key\": \"value\"})".into(),
                ));
            }
        }
    };

    // Call the tool through the manager (handles lazy connect internally).
    manager
        .call_tool(server_name, def, raw_tool_name, args_map)
        .await
        .map_err(|e| ToolError::InvalidArgs(e))
}

/// Mode: `connect` — connect to a specific MCP server and populate its tool cache.
pub async fn execute_connect(
    manager: &Arc<ServerManager>,
    cache: &Arc<MetadataCache>,
    definitions: &HashMap<String, McpServerDefinition>,
    server_name: &str,
) -> Result<String, ToolError> {
    let def = definitions.get(server_name).ok_or_else(|| {
        let available: Vec<&str> = definitions.keys().map(|k| k.as_str()).collect();
        ToolError::InvalidArgs(format!(
            "Unknown MCP server '{}'. Available: {}",
            server_name,
            available.join(", ")
        ))
    })?;

    manager
        .connect(server_name, def)
        .await
        .map_err(|e| ToolError::InvalidArgs(format!("Failed to connect: {}", e)))?;

    // Populate the tool cache so subsequent calls work without a connection.
    match manager.populate_tool_cache(server_name, cache, def).await {
        Ok(()) => {}
        Err(e) => {
            tracing::warn!(server = %server_name, error = %e, "Failed to populate tool cache");
        }
    }

    // Count tools now in cache.
    let hash = duga_config::server_identity_hash(def);
    let tool_count = cache
        .get(server_name, &hash)
        .map(|e| e.tools.len())
        .unwrap_or(0);

    Ok(format!(
        "Connected to MCP server '{}'. {} tool(s) available. Use `mcp({{}})` to list them.",
        server_name, tool_count
    ))
}

/// Mode: `describe` — describe an MCP tool, optionally including its schema.
pub async fn execute_describe(
    cache: &Arc<MetadataCache>,
    definitions: &HashMap<String, McpServerDefinition>,
    prefix: &ToolPrefixMode,
    tool_name: &str,
    include_schemas: bool,
) -> Result<String, ToolError> {
    let tool_lists = build_tool_lists(cache, definitions);
    let server_tools: Vec<(&str, &[String])> =
        tool_lists.iter().map(|(k, v)| (k.as_str(), v.as_slice())).collect();

    let (server_name, raw_tool_name, prefixed_name) =
        lookup_tool(tool_name, &server_tools, prefix).ok_or_else(|| {
            ToolError::InvalidArgs(format!("Unknown MCP tool '{}'.", tool_name))
        })?;

    let hash = duga_config::server_identity_hash(
        definitions.get(server_name).ok_or_else(|| {
            ToolError::InvalidArgs(format!("Server '{}' not found", server_name))
        })?,
    );

    let entry = cache.get(server_name, &hash).ok_or_else(|| {
        ToolError::InvalidArgs(format!(
            "No cached metadata for server '{}'. Connect to the server first.",
            server_name
        ))
    })?;

    let cached_tool = entry
        .tools
        .iter()
        .find(|t| t.name == raw_tool_name)
        .ok_or_else(|| {
            ToolError::InvalidArgs(format!(
                "Tool '{}' not found in cached metadata for server '{}'.",
                raw_tool_name, server_name
            ))
        })?;

    let mut output = format!(
        "Tool: {}\nServer: {}\nDescription: {}",
        prefixed_name, server_name, cached_tool.description
    );

    if include_schemas {
        let schema =
            serde_json::to_string_pretty(&cached_tool.input_schema).unwrap_or_default();
        output.push_str(&format!("\nSchema:\n{}", schema));
    }

    Ok(output)
}

/// Mode: `search` — search for tools across all MCP servers.
pub async fn execute_search(
    cache: &Arc<MetadataCache>,
    definitions: &HashMap<String, McpServerDefinition>,
    prefix: &ToolPrefixMode,
    query: &str,
    use_regex: bool,
) -> Result<String, ToolError> {
    let tool_lists = build_tool_lists(cache, definitions);

    let mut results: Vec<String> = Vec::new();

    for (server_name, tools) in &tool_lists {
        for tool_name in tools {
            let prefixed = prefix_tool(server_name, tool_name, prefix);

            let matches = if use_regex {
                regex::Regex::new(query)
                    .map(|re| re.is_match(tool_name) || re.is_match(&prefixed))
                    .unwrap_or(false)
            } else {
                query.split_whitespace().any(|part| {
                    let p = part.to_lowercase();
                    tool_name.to_lowercase().contains(&p)
                        || prefixed.to_lowercase().contains(&p)
                        || tool_name.to_lowercase().replace('_', "-").contains(&p)
                })
            };

            if matches {
                let desc = get_tool_description(cache, definitions, server_name, tool_name)
                    .unwrap_or_default();
                results.push(format!("- {} ({}): {}", prefixed, server_name, desc));
            }
        }
    }

    if results.is_empty() {
        Ok(format!("No tools found matching '{}'.", query))
    } else {
        results.sort();
        results.insert(
            0,
            format!("Found {} tool(s) matching '{}':", results.len(), query),
        );
        Ok(results.join("\n"))
    }
}

/// Mode: `list` — list all known tools across all servers.
pub async fn execute_list(
    cache: &Arc<MetadataCache>,
    definitions: &HashMap<String, McpServerDefinition>,
    prefix: &ToolPrefixMode,
) -> Result<String, ToolError> {
    let tool_lists = build_tool_lists(cache, definitions);

    let mut lines: Vec<String> = Vec::new();
    for (server_name, tools) in &tool_lists {
        if tools.is_empty() {
            lines.push(format!("[{}] (no tools cached — try `mcp({{connect: \"{}\"}})` first)", server_name, server_name));
            continue;
        }
        lines.push(format!("[{}]", server_name));
        for tool_name in tools {
            let prefixed = prefix_tool(server_name, tool_name, prefix);
            let desc =
                get_tool_description(cache, definitions, server_name, tool_name).unwrap_or_default();
            lines.push(format!("  - {}: {}", prefixed, desc));
        }
    }

    if lines.is_empty() {
        Ok("No MCP tools registered. Add MCP servers under `plugins.mcp` in config.".to_string())
    } else {
        Ok(lines.join("\n"))
    }
}

/// Mode: `server` — show status of connected servers, or all known servers.
pub async fn execute_status(
    manager: &Arc<ServerManager>,
    definitions: &HashMap<String, McpServerDefinition>,
    server_name: Option<&str>,
) -> Result<String, ToolError> {
    if let Some(name) = server_name {
        let def = definitions.get(name).ok_or_else(|| {
            ToolError::InvalidArgs(format!("Unknown server '{}'.", name))
        })?;
        let connected = manager.is_connected(name).await;
        Ok(format!(
            "Server '{}': transport={}, lifecycle={:?}, connected={}",
            name, def.transport, def.lifecycle, connected
        ))
    } else {
        let mut lines = vec!["MCP Server Status:".to_string()];
        for (name, def) in definitions {
            let connected = manager.is_connected(name).await;
            lines.push(format!(
                "  {}: {:?} ({}), connected={}",
                name, def.lifecycle, def.transport, connected,
            ));
        }
        if definitions.is_empty() {
            lines.push("  No MCP servers configured.".to_string());
        }
        Ok(lines.join("\n"))
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Build a map of server_name → tool_names from the cache.
fn build_tool_lists(
    cache: &Arc<MetadataCache>,
    definitions: &HashMap<String, McpServerDefinition>,
) -> HashMap<String, Vec<String>> {
    let mut result = HashMap::new();
    for (name, def) in definitions {
        let hash = duga_config::server_identity_hash(def);
        if let Some(entry) = cache.get(name, &hash) {
            let tools: Vec<String> = entry.tools.iter().map(|t| t.name.clone()).collect();
            result.insert(name.clone(), tools);
        } else {
            result.insert(name.clone(), Vec::new());
        }
    }
    result
}

/// Get a tool description from the cache.
fn get_tool_description(
    cache: &Arc<MetadataCache>,
    definitions: &HashMap<String, McpServerDefinition>,
    server_name: &str,
    tool_name: &str,
) -> Option<String> {
    let def = definitions.get(server_name)?;
    let hash = duga_config::server_identity_hash(def);
    let entry = cache.get(server_name, &hash)?;
    entry
        .tools
        .iter()
        .find(|t| t.name == tool_name)
        .map(|t| t.description.clone())
}
