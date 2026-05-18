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

        tool.validate_args(&call.raw_args)?;

        let confirmation = self.confirmation.read().unwrap().clone();
        if let Some(confirmation) = confirmation {
            confirmation
                .confirm_tool_call(call, &tool.description)
                .await?;
        }

        let call_id = call.id.clone();
        let raw_args = call.raw_args.clone();

        tool.execute(call_id, raw_args, workspace, cancellation, event_sink)
            .await
    }
}
