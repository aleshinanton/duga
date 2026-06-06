//! Tool naming and fuzzy lookup for MCP tools.
//!
//! Supports three prefix modes:
//! - `server`: full prefix `{server}_{tool_name}`
//! - `short`: strips `-mcp` suffix from server name
//! - `none`: raw tool name (no prefix)
//!
//! Fuzzy matching normalizes to lowercase and treats `_` and `-` as equivalent.

/// How to prefix tool names from MCP servers.
#[derive(Clone, Debug, PartialEq)]
pub enum ToolPrefixMode {
    /// Full server name prefix: `github_search_repos`
    Server,
    /// Short prefix stripping `-mcp`: `github` from `github-mcp`
    Short,
    /// No prefix — use raw tool names
    None,
}

impl ToolPrefixMode {
    /// Parse from a config string: "server", "short", or "none".
    pub fn from_config(s: Option<&str>) -> Self {
        match s {
            Some("server") => ToolPrefixMode::Server,
            Some("short") => ToolPrefixMode::Short,
            _ => ToolPrefixMode::None,
        }
    }
}

/// Normalize a tool name for fuzzy matching.
/// Converts to lowercase and replaces `_` with `-`.
pub fn normalize(name: &str) -> String {
    name.to_lowercase().replace('_', "-")
}

/// Build the prefixed tool name for a given server and raw tool name.
pub fn prefix_tool(server_name: &str, tool_name: &str, mode: &ToolPrefixMode) -> String {
    match mode {
        ToolPrefixMode::Server => format!("{}_{}", server_name, tool_name),
        ToolPrefixMode::Short => {
            let short = server_name
                .strip_suffix("-mcp")
                .unwrap_or(server_name);
            format!("{}_{}", short, tool_name)
        }
        ToolPrefixMode::None => tool_name.to_string(),
    }
}

/// Check if a tool name matches a fuzzy search term.
/// Both are normalized before comparison.
fn fuzzy_match(tool_name: &str, term: &str) -> bool {
    normalize(tool_name).contains(&normalize(term))
}

/// Check if a tool should be excluded based on the server's `exclude_tools` list.
/// Matches against both the original tool name and the prefixed name.
pub fn is_excluded(tool_name: &str, prefixed_name: &str, exclude_tools: &[String]) -> bool {
    exclude_tools.iter().any(|ex| {
        normalize(ex) == normalize(tool_name) || normalize(ex) == normalize(prefixed_name)
    })
}

/// Find a tool across multiple servers by fuzzy name.
/// Returns `(server_name, tool_name, prefixed_name)` if found.
pub fn lookup_tool<'a>(
    needle: &str,
    servers: &'a [(&str, &[String])], // (server_name, tool_names)
    mode: &ToolPrefixMode,
) -> Option<(&'a str, &'a str, String)> {
    let normalized_needle = normalize(needle);

    // First try exact match on prefixed name
    for (server_name, tool_names) in servers {
        for tool_name in *tool_names {
            let prefixed = prefix_tool(server_name, tool_name, mode);
            if normalize(&prefixed) == normalized_needle {
                return Some((server_name, tool_name, prefixed));
            }
        }
    }

    // Then try exact match on raw tool name
    for (server_name, tool_names) in servers {
        for tool_name in *tool_names {
            if normalize(tool_name) == normalized_needle {
                let prefixed = prefix_tool(server_name, tool_name, mode);
                return Some((server_name, tool_name, prefixed));
            }
        }
    }

    // Fuzzy match — return first match
    for (server_name, tool_names) in servers {
        for tool_name in *tool_names {
            if fuzzy_match(tool_name, needle) {
                let prefixed = prefix_tool(server_name, tool_name, mode);
                return Some((server_name, tool_name, prefixed));
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_prefix_mode_from_config() {
        assert_eq!(
            ToolPrefixMode::from_config(Some("server")),
            ToolPrefixMode::Server
        );
        assert_eq!(
            ToolPrefixMode::from_config(Some("short")),
            ToolPrefixMode::Short
        );
        assert_eq!(
            ToolPrefixMode::from_config(Some("none")),
            ToolPrefixMode::None
        );
        assert_eq!(ToolPrefixMode::from_config(None), ToolPrefixMode::None);
        assert_eq!(
            ToolPrefixMode::from_config(Some("unknown")),
            ToolPrefixMode::None
        );
    }

    #[test]
    fn test_prefix_tool_server_mode() {
        let result = prefix_tool("github", "search_repos", &ToolPrefixMode::Server);
        assert_eq!(result, "github_search_repos");
    }

    #[test]
    fn test_prefix_tool_short_mode() {
        let result = prefix_tool("github-mcp", "search_repos", &ToolPrefixMode::Short);
        assert_eq!(result, "github_search_repos");
    }

    #[test]
    fn test_prefix_tool_short_mode_no_mcp_suffix() {
        let result = prefix_tool("github", "search_repos", &ToolPrefixMode::Short);
        assert_eq!(result, "github_search_repos");
    }

    #[test]
    fn test_prefix_tool_none_mode() {
        let result = prefix_tool("github", "search_repos", &ToolPrefixMode::None);
        assert_eq!(result, "search_repos");
    }

    #[test]
    fn test_normalize_lowercase() {
        assert_eq!(normalize("GitHub_Search"), "github-search");
    }

    #[test]
    fn test_normalize_underscore_to_dash() {
        assert_eq!(normalize("foo_bar"), "foo-bar");
    }

    #[test]
    fn test_is_excluded_by_original_name() {
        let excluded = vec!["dangerous_tool".to_string()];
        assert!(is_excluded(
            "dangerous_tool",
            "server_dangerous_tool",
            &excluded
        ));
    }

    #[test]
    fn test_is_excluded_by_prefixed_name() {
        let excluded = vec!["server_dangerous_tool".to_string()];
        assert!(is_excluded(
            "dangerous_tool",
            "server_dangerous_tool",
            &excluded
        ));
    }

    #[test]
    fn test_is_excluded_normalized_match() {
        let excluded = vec!["dangerous-tool".to_string()];
        assert!(is_excluded(
            "dangerous_tool",
            "server_dangerous_tool",
            &excluded
        ));
    }

    #[test]
    fn test_is_not_excluded() {
        let excluded = vec!["other_tool".to_string()];
        assert!(!is_excluded("safe_tool", "server_safe_tool", &excluded));
    }

    #[test]
    fn test_lookup_tool_exact_prefixed() {
        let github_tools = vec!["search_repos".to_string(), "list_issues".to_string()];
        let fs_tools = vec!["read_file".to_string(), "write_file".to_string()];
        let servers: Vec<(&str, &[String])> = vec![
            ("github", &github_tools),
            ("filesystem", &fs_tools),
        ];
        let mode = ToolPrefixMode::Server;

        let result = lookup_tool("github_search_repos", &servers, &mode);
        assert!(result.is_some());
        let (server, tool, prefixed) = result.unwrap();
        assert_eq!(server, "github");
        assert_eq!(tool, "search_repos");
        assert_eq!(prefixed, "github_search_repos");
    }

    #[test]
    fn test_lookup_tool_exact_raw() {
        let tools = vec!["search_repos".to_string()];
        let servers: Vec<(&str, &[String])> = vec![("github", &tools)];
        let mode = ToolPrefixMode::Server;

        let result = lookup_tool("search_repos", &servers, &mode);
        assert!(result.is_some());
        let (server, tool, _prefixed) = result.unwrap();
        assert_eq!(server, "github");
        assert_eq!(tool, "search_repos");
    }

    #[test]
    fn test_lookup_tool_fuzzy() {
        let tools = vec!["search_repos".to_string()];
        let servers: Vec<(&str, &[String])> = vec![("github", &tools)];
        let mode = ToolPrefixMode::Server;

        let result = lookup_tool("search", &servers, &mode);
        assert!(result.is_some());
        let (server, tool, _prefixed) = result.unwrap();
        assert_eq!(server, "github");
        assert_eq!(tool, "search_repos");
    }

    #[test]
    fn test_lookup_tool_not_found() {
        let tools = vec!["search_repos".to_string()];
        let servers: Vec<(&str, &[String])> = vec![("github", &tools)];
        let mode = ToolPrefixMode::Server;

        let result = lookup_tool("nonexistent", &servers, &mode);
        assert!(result.is_none());
    }

    #[test]
    fn test_lookup_tool_ambiguous_returns_first() {
        let tools_a = vec!["read_data".to_string()];
        let tools_b = vec!["read_files".to_string()];
        let servers: Vec<(&str, &[String])> = vec![("srv_a", &tools_a), ("srv_b", &tools_b)];
        let mode = ToolPrefixMode::Server;

        let result = lookup_tool("read", &servers, &mode);
        assert!(result.is_some());
        let (server, _tool, _prefixed) = result.unwrap();
        assert_eq!(server, "srv_a");
    }
}
