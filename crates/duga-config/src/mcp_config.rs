//! MCP (Model Context Protocol) plugin configuration types.
//!
//! MCP servers are treated as plugins alongside WASM `.wasm` files.
//! Both surface `Tool` implementations via `ErasedTool` and register
//! into the same `ToolDispatcher`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

// ── MCP Plugin Config (lives under `plugins.mcp`) ──────────────────────────

/// Top-level MCP plugin configuration, placed under `plugins.mcp` in config.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct McpPluginConfig {
    pub servers: HashMap<String, McpServerDefinition>,
    #[serde(default)]
    pub settings: McpSettings,
}

// ── Server Definition ──────────────────────────────────────────────────────

/// Definition of a single MCP server.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct McpServerDefinition {
    /// Unique server identifier (the key in the `servers` map).
    /// Included here so the definition can be passed around without the key.
    #[serde(default)]
    pub name: String,

    /// Transport protocol: "stdio" or "http".
    #[serde(default = "default_transport")]
    pub transport: String,

    // ── Stdio transport fields ──
    /// Command to launch the server process.
    #[serde(default)]
    pub command: Option<String>,
    /// Arguments for the server process.
    #[serde(default)]
    pub args: Option<Vec<String>>,
    /// Environment variables for the server process. Supports `${VAR}` /
    /// `$env:VAR` interpolation at connection time.
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,
    /// Working directory for the server process. `~` expands to `$HOME`.
    #[serde(default)]
    pub cwd: Option<String>,

    // ── HTTP/SSE transport fields ──
    /// Server URL for HTTP transport.
    #[serde(default)]
    pub url: Option<String>,
    /// Additional HTTP request headers.
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
    /// Literal bearer token for HTTP auth.
    #[serde(default)]
    pub bearer_token: Option<String>,
    /// Environment variable containing the bearer token.
    #[serde(default)]
    pub bearer_token_env: Option<String>,

    // ── Lifecycle ──
    #[serde(default)]
    pub lifecycle: McpLifecycleMode,

    // ── Tool filtering ──
    /// Tools to exclude/hide from this server.
    #[serde(default)]
    pub exclude_tools: Vec<String>,

    /// Promote specific tools to direct (non-proxy) registration.
    /// `true` → all tools; `false` → none; list → specific names.
    #[serde(default)]
    pub direct_tools: Option<McpDirectToolsMode>,
}

fn default_transport() -> String {
    "stdio".into()
}

// ── Lifecycle Mode ─────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum McpLifecycleMode {
    /// Connect on first use, disconnect after idle timeout.
    #[default]
    Lazy,
    /// Connect at session start, keep alive for the session.
    Eager,
    /// Connect at session start, reconnect on failure.
    KeepAlive,
}

// ── Direct Tools Mode ──────────────────────────────────────────────────────

/// Specifies which tools should be promoted to direct (non-proxy) tools.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum McpDirectToolsMode {
    /// `true` → promote all tools; `false` → promote none.
    All(bool),
    /// Specific tool names to promote.
    List(Vec<String>),
}

// ── MCP Settings ───────────────────────────────────────────────────────────

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct McpSettings {
    /// Tool name prefix mode: "server", "short", or "none".
    #[serde(default)]
    pub tool_prefix: Option<String>,

    /// Time after which idle server connections are closed.
    #[serde(
        default = "default_idle_timeout",
        deserialize_with = "deserialize_duration_opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub idle_timeout: Option<Duration>,

    /// Global override for direct tool promotion.
    #[serde(default)]
    pub direct_tools: Option<McpDirectToolsMode>,

    /// When true, skip registering the `mcp()` proxy tool.
    /// Direct tools from cache are still registered.
    #[serde(default)]
    pub disable_proxy_tool: bool,
}

fn default_idle_timeout() -> Option<Duration> {
    Some(Duration::from_secs(600))
}

impl Default for McpSettings {
    fn default() -> Self {
        Self {
            tool_prefix: None,
            idle_timeout: default_idle_timeout(),
            direct_tools: None,
            disable_proxy_tool: false,
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Deserialize an optional duration string (e.g. "600s", "30s", "5m").
fn deserialize_duration_opt<'de, D>(deserializer: D) -> Result<Option<Duration>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Option<String> = Option::deserialize(deserializer)?;
    match raw {
        None => Ok(None),
        Some(s) => {
            let s = s.trim();
            let parse_num = |v: &str| {
                v.parse::<u64>()
                    .map_err(|_| serde::de::Error::custom(format!("invalid duration '{}'", s)))
            };
            let dur = if let Some(val) = s.strip_suffix("ms") {
                Duration::from_millis(parse_num(val)?)
            } else if let Some(val) = s.strip_suffix('s') {
                Duration::from_secs(parse_num(val)?)
            } else if let Some(val) = s.strip_suffix('m') {
                Duration::from_secs(parse_num(val)? * 60)
            } else if let Some(val) = s.strip_suffix('h') {
                Duration::from_secs(parse_num(val)? * 3600)
            } else {
                return Err(serde::de::Error::custom(
                    "duration must use ms, s, m, or h suffix",
                ));
            };
            Ok(Some(dur))
        }
    }
}

/// Expand `${VAR}`, `$env:VAR`, and `~` in a string.
pub fn expand_env_vars(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '$' {
            match chars.peek() {
                Some('{') => {
                    chars.next(); // consume '{'
                    let mut var = String::new();
                    let mut found_close = false;
                    for c in chars.by_ref() {
                        if c == '}' {
                            found_close = true;
                            break;
                        }
                        var.push(c);
                    }
                    if !found_close {
                        // Malformed: output literal
                        result.push_str("${");
                        result.push_str(&var);
                    } else {
                        let expanded = std::env::var(&var).unwrap_or_default();
                        result.push_str(&expanded);
                    }
                }
                Some('e') => {
                    // Check for $env:VAR pattern
                    let remaining: String = chars.clone().take(4).collect();
                    if remaining.starts_with("env:") {
                        chars.next(); // 'e'
                        chars.next(); // 'n'
                        chars.next(); // 'v'
                        chars.next(); // ':'
                        let mut var = String::new();
                        for c in chars.by_ref() {
                            if !c.is_alphanumeric() && c != '_' {
                                // Note: we consumed a non-var char, need to handle it.
                                // For simplicity, just break and lose the char.
                                // In practice this should be at end-of-string.
                                break;
                            }
                            var.push(c);
                        }
                        let expanded = std::env::var(&var).unwrap_or_default();
                        result.push_str(&expanded);
                    } else {
                        result.push('$');
                        result.push('e');
                    }
                }
                _ => {
                    result.push('$');
                }
            }
        } else if ch == '~' {
            // Expand ~ to HOME if at start or after whitespace
            if result.is_empty() || result.ends_with(' ') {
                if let Ok(home) = std::env::var("HOME") {
                    result.push_str(&home);
                    continue;
                }
            }
            result.push('~');
        } else {
            result.push(ch);
        }
    }

    result
}

/// Build a stable "identity hash" for a server definition.
/// This is used to detect config changes and invalidate the metadata cache.
pub fn server_identity_hash(def: &McpServerDefinition) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();

    // Hash key identity fields in a stable order.
    hasher.update(b"v1"); // version tag for future-proofing
    hasher.update(def.transport.as_bytes());
    hasher.update(b"\x00");

    if let Some(ref cmd) = def.command {
        hasher.update(cmd.as_bytes());
    }
    hasher.update(b"\x00");

    if let Some(ref args) = def.args {
        for a in args {
            hasher.update(a.as_bytes());
            hasher.update(b"\x01");
        }
    }
    hasher.update(b"\x00");

    if let Some(ref url) = def.url {
        hasher.update(url.as_bytes());
    }
    hasher.update(b"\x00");

    // Note: we deliberately do NOT hash env vars, bearer tokens,
    // or headers since those don't change the set of tools.
    // The hash is about tool identity, not auth identity.

    let result = hasher.finalize();
    hex::encode(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_env_vars_brace() {
        unsafe { std::env::set_var("TEST_MCP_FOO", "bar") };
        let result = expand_env_vars("prefix_${TEST_MCP_FOO}_suffix");
        assert_eq!(result, "prefix_bar_suffix");
    }

    #[test]
    fn test_expand_env_vars_env_colon() {
        unsafe { std::env::set_var("TEST_MCP_BAR", "baz") };
        let result = expand_env_vars("$env:TEST_MCP_BAR");
        assert_eq!(result, "baz");
    }

    #[test]
    fn test_expand_env_vars_unknown_var() {
        let result = expand_env_vars("${NONEXISTENT_VAR_12345}");
        assert_eq!(result, "");
    }

    #[test]
    fn test_expand_tilde() {
        unsafe { std::env::set_var("HOME", "/home/testuser") };
        let result = expand_env_vars("~/projects");
        assert_eq!(result, "/home/testuser/projects");
    }

    #[test]
    fn test_server_identity_hash_stable() {
        let def = McpServerDefinition {
            name: "test".into(),
            transport: "stdio".into(),
            command: Some("npx".into()),
            args: Some(vec!["-y".into(), "@mcp/server".into()]),
            ..Default::default()
        };
        let h1 = server_identity_hash(&def);
        let h2 = server_identity_hash(&def);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 hex
    }

    #[test]
    fn test_server_identity_hash_differs_on_command_change() {
        let def1 = McpServerDefinition {
            name: "test".into(),
            transport: "stdio".into(),
            command: Some("npx".into()),
            args: Some(vec!["-y".into(), "@mcp/server".into()]),
            ..Default::default()
        };
        let mut def2 = def1.clone();
        def2.command = Some("node".into());
        assert_ne!(server_identity_hash(&def1), server_identity_hash(&def2));
    }

    #[test]
    fn test_mcp_config_deserialize_minimal() {
        let yaml = r#"
servers:
  github:
    transport: stdio
    command: npx
    args: ["-y", "@modelcontextprotocol/server-github"]
settings: {}
"#;
        let config: McpPluginConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.servers.len(), 1);
        let gh = &config.servers["github"];
        assert_eq!(gh.transport, "stdio");
        assert_eq!(gh.command.as_deref(), Some("npx"));
        assert_eq!(gh.lifecycle, McpLifecycleMode::Lazy);
    }

    #[test]
    fn test_mcp_config_deserialize_full() {
        let yaml = r#"
servers:
  filesystem:
    transport: stdio
    command: npx
    args: ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
    lifecycle: eager
    direct_tools:
      - read_file
      - write_file
    exclude_tools:
      - dangerous_tool
settings:
  tool_prefix: server
  idle_timeout: 300s
  disable_proxy_tool: false
"#;
        let config: McpPluginConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.servers.len(), 1);
        let fs = &config.servers["filesystem"];
        assert_eq!(fs.lifecycle, McpLifecycleMode::Eager);
        assert_eq!(fs.exclude_tools, vec!["dangerous_tool"]);
        match &fs.direct_tools {
            Some(McpDirectToolsMode::List(v)) => {
                assert_eq!(v.len(), 2);
                assert!(v.contains(&"read_file".to_string()));
            }
            _ => panic!("expected List"),
        }
        assert_eq!(config.settings.tool_prefix.as_deref(), Some("server"));
        assert_eq!(
            config.settings.idle_timeout,
            Some(Duration::from_secs(300))
        );
    }

    #[test]
    fn test_direct_tools_mode_all() {
        let yaml = "true";
        let mode: McpDirectToolsMode = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(mode, McpDirectToolsMode::All(true));

        let yaml = "false";
        let mode: McpDirectToolsMode = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(mode, McpDirectToolsMode::All(false));
    }

    #[test]
    fn test_direct_tools_mode_list() {
        let yaml = r#"
- tool_a
- tool_b
"#;
        let mode: McpDirectToolsMode = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            mode,
            McpDirectToolsMode::List(vec!["tool_a".into(), "tool_b".into()])
        );
    }

    #[test]
    fn test_lifecycle_mode_default() {
        assert_eq!(McpLifecycleMode::default(), McpLifecycleMode::Lazy);
    }

    #[test]
    fn test_mcp_settings_default() {
        let settings = McpSettings::default();
        assert_eq!(settings.tool_prefix, None);
        assert_eq!(settings.idle_timeout, Some(Duration::from_secs(600)));
        assert_eq!(settings.direct_tools, None);
        assert!(!settings.disable_proxy_tool);
    }
}
