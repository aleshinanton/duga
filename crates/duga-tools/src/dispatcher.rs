//! Tool registry and dispatch engine.

use crate::confirmation::ConfirmationMiddleware;
use crate::erased::ErasedTool;
use crate::event_sink::EventSink;
use crate::result::ToolCallResult;
use duga_sandbox::CancellationToken;
use duga_sandbox::Workspace;
use duga_types::error::ToolError;
use duga_types::tool_call::ToolCall;
use std::collections::HashMap;
use std::sync::RwLock;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ToolDispatcherError {
    #[error("duplicate tool name: {0}")]
    DuplicateName(String),
    #[error("tool not found: {0}")]
    NotFound(String),
}

pub struct ToolDispatcher {
    tools: RwLock<HashMap<String, ErasedTool>>,
    confirmation: RwLock<Option<ConfirmationMiddleware>>,
}

impl Default for ToolDispatcher {
    fn default() -> Self {
        Self {
            tools: RwLock::new(HashMap::new()),
            confirmation: RwLock::new(None),
        }
    }
}

impl std::fmt::Debug for ToolDispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolDispatcher")
            .field("tools", &self.names())
            .field("confirmation", &self.confirmation.read().unwrap().is_some())
            .finish()
    }
}

impl ToolDispatcher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_erased(&self, tool: ErasedTool) -> Result<(), ToolDispatcherError> {
        let name = tool.name.clone();
        let mut tools = self.tools.write().unwrap();
        if tools.contains_key(&name) {
            return Err(ToolDispatcherError::DuplicateName(name));
        }
        tools.insert(name, tool);
        Ok(())
    }

    pub fn set_confirmation(&self, confirmation: ConfirmationMiddleware) {
        *self.confirmation.write().unwrap() = Some(confirmation);
    }

    pub fn clear_confirmation(&self) {
        *self.confirmation.write().unwrap() = None;
    }

    pub fn get(&self, name: &str) -> Option<ErasedTool> {
        self.tools.read().unwrap().get(name).cloned()
    }

    pub fn reset_limits(&self) {
        for tool in self.tools.read().unwrap().values() {
            tool.reset_limits();
        }
    }

    pub fn names(&self) -> Vec<String> {
        self.tools.read().unwrap().keys().cloned().collect()
    }

    pub fn schemas(&self) -> Vec<duga_types::tool_schema::ToolSchema> {
        self.tools
            .read()
            .unwrap()
            .values()
            .map(|t| t.schema())
            .collect()
    }

    pub fn tool_schema(
        &self,
        name: &str,
    ) -> Result<duga_types::tool_schema::ToolSchema, ToolDispatcherError> {
        self.tools
            .read()
            .unwrap()
            .get(name)
            .map(|t| t.schema())
            .ok_or(ToolDispatcherError::NotFound(name.into()))
    }

    pub async fn dispatch(
        &self,
        call: &ToolCall,
        workspace: &Workspace,
        cancellation: CancellationToken,
        event_sink: &dyn EventSink,
    ) -> ToolCallResult {
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

        // Inject default 'label' if the LLM omitted it, so validation passes.
        let mut raw_args = call.raw_args.clone();
        if let Some(obj) = raw_args.as_object_mut() {
            if !obj.contains_key("label") {
                obj.insert("label".to_string(), serde_json::Value::String(String::new()));
            }
        }

        tool.validate_args(&raw_args)?;

        let confirmation = self.confirmation.read().unwrap().clone();
        if let Some(confirmation) = confirmation {
            confirmation
                .confirm_tool_call(call, &tool.description)
                .await?;
        }

        let call_id = call.id.clone();

        tool.execute(call_id, raw_args, workspace, cancellation, event_sink)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_sink::NullSink;
    use serde_json::json;

    #[test]
    fn new_dispatcher_is_empty() {
        let d = ToolDispatcher::new();
        assert!(d.names().is_empty());
        assert!(d.schemas().is_empty());
    }

    #[test]
    fn default_dispatcher_is_empty() {
        let d = ToolDispatcher::default();
        assert!(d.names().is_empty());
    }

    #[test]
    fn get_nonexistent_returns_none() {
        let d = ToolDispatcher::new();
        assert!(d.get("nonexistent").is_none());
    }

    #[test]
    fn tool_schema_not_found() {
        let d = ToolDispatcher::new();
        let err = d.tool_schema("missing").unwrap_err();
        assert_eq!(err, ToolDispatcherError::NotFound("missing".into()));
    }

    #[test]
    fn dispatcher_debug_format() {
        let d = ToolDispatcher::new();
        let dbg = format!("{:?}", d);
        assert!(dbg.contains("ToolDispatcher"));
    }

    #[tokio::test]
    async fn dispatch_unknown_tool_returns_error() {
        let d = ToolDispatcher::new();
        let dir = tempfile::tempdir().unwrap();
        let ws = duga_sandbox::Workspace::open(dir.path()).unwrap();
        let cancel = duga_sandbox::CancellationToken::new();
        let sink = NullSink;

        let call = duga_types::tool_call::ToolCall {
            id: duga_types::tool_call::CallId::new(),
            tool: "nonexistent".into(),
            raw_args: json!({}),
        };

        let result = d.dispatch(&call, &ws, cancel, &sink).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Unknown tool"), "got: {err_msg}");
    }

    #[tokio::test]
    async fn dispatch_to_empty_registry_produces_helpful_error() {
        let d = ToolDispatcher::new();
        let dir = tempfile::tempdir().unwrap();
        let ws = duga_sandbox::Workspace::open(dir.path()).unwrap();
        let cancel = duga_sandbox::CancellationToken::new();
        let sink = NullSink;

        let call = duga_types::tool_call::ToolCall {
            id: duga_types::tool_call::CallId::new(),
            tool: "empty".into(),
            raw_args: json!({}),
        };

        let err = d.dispatch(&call, &ws, cancel, &sink).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("No tools registered"), "got: {msg}");
    }
}
