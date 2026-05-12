//! Type-erased tool wrapper for registration and dispatch.

use crate::context::ToolContext;
use crate::event_sink::EventSink;
use crate::result::ToolCallResult;
use crate::schema::deserialize_args;
use duga_sandbox::CancellationToken;
use duga_sandbox::Workspace;
use duga_types::error::ToolError;
use duga_types::tool_call::CallId;
use duga_types::tool_schema::ToolSchema;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Type-erased execute function signature.
type ErasedFn = Arc<
    dyn Fn(
            CallId,
            Value,
            &'static Workspace,
            CancellationToken,
            &'static dyn EventSink,
        ) -> Pin<Box<dyn Future<Output = ToolCallResult> + Send>>
        + Send
        + Sync,
>;

/// A type-erased tool for registration in `ToolDispatcher`.
#[derive(Clone)]
pub struct ErasedTool {
    pub name: String,
    pub description: String,
    pub args_schema: Value,
    pub retryable: bool,
    execute: ErasedFn,
}

impl std::fmt::Debug for ErasedTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ErasedTool")
            .field("name", &self.name)
            .field("retryable", &self.retryable)
            .finish()
    }
}

impl ErasedTool {
    pub fn erase<T: crate::tool::Tool + 'static>(tool: T) -> Self {
        let name = tool.name().to_string();
        let description = tool.description().to_string();
        let args_schema = tool.json_schema();
        let retryable = tool.retryable();

        let tool_arc = Arc::new(tool);

        let execute = Arc::new(
            move |_call_id: CallId,
                  raw_args: Value,
                  workspace: &'static Workspace,
                  cancellation: CancellationToken,
                  event_sink: &'static dyn EventSink| {
                let tool = tool_arc.clone();
                Box::pin(async move {
                    let args: T::Args = match deserialize_args(raw_args) {
                        Ok(a) => a,
                        Err(e) => {
                            return Err(ToolError::InvalidArgs(format!(
                                "Deserialization failed: {}",
                                e
                            )));
                        }
                    };

                    let ctx = ToolContext::new(workspace, cancellation, event_sink);
                    tool.execute(ctx, args).await
                }) as Pin<Box<dyn Future<Output = ToolCallResult> + Send>>
            },
        ) as ErasedFn;

        Self {
            name,
            description,
            args_schema,
            retryable,
            execute,
        }
    }

    pub fn schema(&self) -> ToolSchema {
        ToolSchema::new(&self.name, &self.description, self.args_schema.clone())
    }

    pub async fn execute(
        &self,
        call_id: CallId,
        raw_args: Value,
        workspace: &'static Workspace,
        cancellation: CancellationToken,
        event_sink: &'static dyn EventSink,
    ) -> ToolCallResult {
        (self.execute)(call_id, raw_args, workspace, cancellation, event_sink).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ToolContext;
    use crate::dispatcher::ToolDispatcher;
    use crate::tool::Tool;
    use duga_types::tool_call::CallId;
    use duga_types::tool_result::ToolResult;
    use serde::{Deserialize, Serialize};
    use schemars::JsonSchema;

    struct MockTool;

    #[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
    struct MockArgs {
        msg: String,
    }

    impl Tool for MockTool {
        type Args = MockArgs;

        fn name(&self) -> &str { "mock" }
        fn description(&self) -> &str { "A mock tool for testing" }
        fn retryable(&self) -> bool { true }

        async fn execute(&self, _ctx: ToolContext<'_>, args: Self::Args) -> ToolCallResult {
            Ok(ToolResult {
                tool_call_id: CallId::new(),
                success: true,
                output: format!("Mock received: {}", args.msg),
                metadata: serde_json::Value::Null,
                duration_ms: 0,
                stdout_bytes: 0,
                stderr_bytes: 0,
                truncated: false,
            })
        }
    }

    #[test]
    fn test_erased_tool_schema() {
        let tool = MockTool;
        let erased = ErasedTool::erase(tool);
        let schema = erased.schema();
        assert_eq!(schema.name, "mock");
    }

    #[test]
    fn test_dispatcher_register_and_lookup() {
        let tool = MockTool;
        let erased = ErasedTool::erase(tool);
        let dispatcher = ToolDispatcher::new();
        dispatcher.register_erased(erased).unwrap();
        assert!(dispatcher.get("mock").is_some());
    }
}
