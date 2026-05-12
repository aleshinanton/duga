//! Tool registry and dispatch engine.
//!
//! `ToolDispatcher` owns the tool registry and provides dispatch.
//! Tools are registered as `ErasedTool` instances (type-erased).

use crate::erased::ErasedTool;
use crate::event_sink::EventSink;
use crate::result::ToolCallResult;
use crate::schema::validate_schema;
use duga_sandbox::CancellationToken;
use duga_sandbox::Workspace;
use duga_types::error::ToolError;
use duga_types::tool_call::ToolCall;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::RwLock;

/// Errors from tool registration and dispatch.
#[derive(Clone, Debug, thiserror::Error, PartialEq)]
pub enum ToolDispatcherError {
    #[error("duplicate tool name: {0}")]
    DuplicateName(String),
    #[error("tool not found: {0}")]
    NotFound(String),
}

/// Manages tool registration and dispatch.
#[derive(Debug, Default)]
pub struct ToolDispatcher {
    tools: RwLock<HashMap<String, ErasedTool>>,
}

impl ToolDispatcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a tool. Returns error on duplicate name.
    pub fn register_erased(&self, tool: ErasedTool) -> Result<(), ToolDispatcherError> {
        let name = tool.name.clone();
        let mut tools = self.tools.write().unwrap();
        if tools.contains_key(&name) {
            return Err(ToolDispatcherError::DuplicateName(name));
        }
        tools.insert(name, tool);
        Ok(())
    }

    /// Get a tool by name.
    pub fn get(&self, name: &str) -> Option<ErasedTool> {
        self.tools.read().unwrap().get(name).cloned()
    }

    /// Return all tool names.
    pub fn names(&self) -> Vec<&str> {
        self.tools.read().unwrap().keys().map(|s| s.as_str()).collect()
    }

    /// Return all tool schemas.
    pub fn schemas(&self) -> Vec<duga_types::tool_schema::ToolSchema> {
        self.tools
            .read()
            .unwrap()
            .values()
            .map(|t| t.schema())
            .collect()
    }

    /// Dispatch a tool call.
    ///
    /// 1. Resolve tool by name
    /// 2. Validate JSON args against schema
    /// 3. Call the erased execute function
    pub async fn dispatch(
        &self,
        call: &ToolCall,
        workspace: &Workspace,
        cancellation: CancellationToken,
        event_sink: &dyn EventSink,
    ) -> ToolCallResult {
        // Resolve tool
        let tool = {
            let tools = self.tools.read().unwrap();
            tools.get(&call.tool).cloned()
        };

        let tool = match tool {
            Some(t) => t,
            None => {
                let available = self.names().join(", ");
                let msg = if available.is_empty() {
                    format!("Unknown tool '{}'. No tools registered.", call.tool)
                } else {
                    format!("Unknown tool '{}'. Available: {}", call.tool, available)
                };
                return Err(ToolError::InvalidArgs(msg));
            }
        };

        // Validate schema (before execution)
        if let Err(errors) = validate_schema(&tool.args_schema, &call.raw_args) {
            return Err(ToolError::InvalidArgs(format!(
                "Invalid arguments: {}",
                errors.join("; ")
            )));
        }

        // Clone for the async block
        let call_id = call.id;
        let raw_args = call.raw_args.clone();

        // SAFETY: Cast Workspace and EventSink to 'static for the erased closure.
        // The dispatcher lives as long as the agent loop, which also owns
        // the workspace and event sink. This is only safe because the closure
        // executes within the same request scope.
        let workspace_static: &'static Workspace = unsafe { std::mem::transmute(workspace) };
        let sink_static: &'static dyn EventSink = unsafe { std::mem::transmute(event_sink) };

        tool.execute(call_id, raw_args, workspace_static, cancellation, sink_static)
            .await
    }
}