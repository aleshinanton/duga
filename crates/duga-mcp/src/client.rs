//! Thin wrapper over `rust-mcp-sdk` client runtime.
//!
//! Provides:
//! - `McpClientConnection` — a connected MCP client with tool listing/calling.
//! - `connect_stdio()` and `connect_http()` factory functions.
//! - Bearer token auth via custom headers on HTTP transport.
//! - Cursor-paginated `list_tools()`.

use duga_config::McpServerDefinition;
use rust_mcp_sdk::error::SdkResult;
use rust_mcp_sdk::mcp_client::client_runtime;
use rust_mcp_sdk::mcp_client::{ClientHandler, McpClientOptions, ToMcpClientHandler};
use rust_mcp_sdk::schema::{
    CallToolRequestParams, ClientCapabilities, Implementation, InitializeRequestParams,
    ListToolsResult, PaginatedRequestParams, ProtocolVersion, Tool,
};
use rust_mcp_sdk::McpClient;
use rust_mcp_sdk::{
    ClientStreamableTransport, RequestOptions, StdioTransport, StreamableTransportOptions,
    TransportOptions,
};
use std::collections::HashMap;
use std::sync::Arc;

/// A connected MCP client wrapping the SDK's `McpClient` runtime.
pub struct McpClientConnection {
    /// The underlying MCP client.
    inner: Arc<dyn McpClient>,
    /// Server name for logging.
    pub server_name: String,
}

impl McpClientConnection {
    /// List all tools exposed by this server.
    /// Uses cursor-based pagination to collect all tools.
    pub async fn list_tools(&self) -> SdkResult<Vec<Tool>> {
        let mut all_tools = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let params = PaginatedRequestParams {
                cursor,
                meta: None,
            };
            let result: ListToolsResult = self.inner.request_tool_list(Some(params)).await?;
            all_tools.extend(result.tools);

            match result.next_cursor {
                Some(ref next) if !next.is_empty() => cursor = Some(next.clone()),
                _ => break,
            }
        }

        Ok(all_tools)
    }

    /// Call a specific tool by name with JSON arguments.
    pub async fn call_tool(
        &self,
        name: &str,
        args: serde_json::Map<String, serde_json::Value>,
    ) -> SdkResult<rust_mcp_sdk::schema::CallToolResult> {
        let params = CallToolRequestParams {
            name: name.to_string(),
            arguments: Some(args),
            meta: None,
            task: None,
        };
        self.inner.request_tool_call(params).await
    }

    /// Shut down the client connection gracefully.
    pub async fn shutdown(&self) -> SdkResult<()> {
        self.inner.shut_down().await
    }
}

// A default no-op client handler (all methods have default impls).
struct NoopClientHandler;

#[async_trait::async_trait]
impl ClientHandler for NoopClientHandler {}

/// Client info sent during MCP initialization.
fn client_info() -> Implementation {
    Implementation {
        name: "duga".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        title: Some("duga MCP client".into()),
        description: Some("duga AI agent connecting to MCP servers".into()),
        icons: vec![],
        website_url: None,
    }
}

/// Connect to an MCP server via stdio transport.
pub async fn connect_stdio(def: &McpServerDefinition) -> SdkResult<McpClientConnection> {
    let transport = StdioTransport::create_with_server_launch(
        def.command
            .as_deref()
            .unwrap_or("npx"),
        def.args.clone().unwrap_or_default(),
        def.env.clone(),
        TransportOptions::default(),
    )?;

    let client_details = InitializeRequestParams {
        capabilities: ClientCapabilities::default(),
        client_info: client_info(),
        protocol_version: ProtocolVersion::V2025_11_25.into(),
        meta: None,
    };

    let handler = NoopClientHandler;
    let options = McpClientOptions {
        client_details,
        transport,
        handler: handler.to_mcp_client_handler(),
        task_store: None,
        server_task_store: None,
        message_observer: None,
    };
    let client = client_runtime::create_client(options);
    client.clone().start().await?;

    Ok(McpClientConnection {
        inner: client,
        server_name: def.name.clone(),
    })
}

/// Connect to an MCP server via HTTP/Streamable HTTP transport.
pub async fn connect_http(def: &McpServerDefinition) -> SdkResult<McpClientConnection> {
    let url = def.url.as_deref().unwrap_or("http://localhost:8080");

    // Build custom headers.
    let mut custom_headers = HashMap::new();

    // Add bearer token if configured.
    let token = def
        .bearer_token
        .as_ref()
        .filter(|t| !t.is_empty() && !t.starts_with('$'))
        .cloned();

    let token = token.or_else(|| {
        def.bearer_token_env
            .as_ref()
            .and_then(|env_var| std::env::var(env_var).ok())
    });

    if let Some(t) = token {
        custom_headers.insert("Authorization".to_string(), format!("Bearer {}", t));
    }

    // Add custom headers from config.
    if let Some(ref headers) = def.headers {
        for (key, value) in headers {
            custom_headers.insert(key.clone(), value.clone());
        }
    }

    let transport_options = StreamableTransportOptions {
        mcp_url: url.to_string(),
        request_options: RequestOptions {
            custom_headers: if custom_headers.is_empty() {
                None
            } else {
                Some(custom_headers)
            },
            ..Default::default()
        },
    };

    let transport = ClientStreamableTransport::new(&transport_options, None, true)?;

    let client_details = InitializeRequestParams {
        capabilities: ClientCapabilities::default(),
        client_info: client_info(),
        protocol_version: ProtocolVersion::V2025_11_25.into(),
        meta: None,
    };

    let handler = NoopClientHandler;
    let options = McpClientOptions {
        client_details,
        transport,
        handler: handler.to_mcp_client_handler(),
        task_store: None,
        server_task_store: None,
        message_observer: None,
    };
    let client = client_runtime::create_client(options);
    client.clone().start().await?;

    Ok(McpClientConnection {
        inner: client,
        server_name: def.name.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_info_has_name() {
        let info = client_info();
        assert_eq!(info.name, "duga");
        assert!(!info.version.is_empty());
    }
}
