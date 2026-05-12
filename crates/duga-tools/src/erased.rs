//! Type-erased tool wrapper for registration and dispatch.
//!
//! `ErasedTool` wraps any `impl Tool` behind a boxed function pointer.
//! This avoids the `type Args` object-safety limitation of the `Tool` trait
//! while preserving schema validation at dispatch time.

use crate::context::ToolContext;
use crate::event_sink::EventSink;
use crate::result::ToolCallResult;
use crate::schema::{deserialize_args, validate_schema};
use duga_sandbox::CancellationToken;
use duga_sandbox::Workspace;
use duga_types::error::ToolError;
use duga_types::tool_call::CallId;
use duga_types::tool_schema::ToolSchema;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::RwLock;

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

impl ErasedTool {
    /// Erase a concrete tool into a type-erased `ErasedTool`.
    ///
    /// The tool is stored in an Arc inside the execute closure.
    /// Each invocation clones the Arc and calls `execute` on the clone.
    pub fn erase<T: crate::tool::Tool + 'static>(tool: T) -> Self {
        let name = tool.name().to_string();
        let description = tool.description().to_string();
        let args_schema = tool.json_schema();
        let retryable = tool.retryable();

        // Store tool in Arc, clone on each invocation
        let tool_arc = Arc::new(tool);

        let execute = Arc::new(
            move |call_id: CallId,
                  raw_args: Value,
                  workspace: &'static Workspace,
                  cancellation: CancellationToken,
                  event_sink: &'static dyn EventSink| {
                let tool = tool_arc.clone();
                Box::pin(async move {
                    // Validate schema before deserialization
                    if let Err(errors) = validate_schema(&args_schema, &raw_args) {
                        return Err(ToolError::InvalidArgs(format!(
                            "Invalid arguments: {}",
                            errors.join("; ")
                        )));
                    }

                    // Deserialize into concrete args type
                    let args: T::Args = match deserialize_args(raw_args) {
                        Ok(a) => a,
                        Err(e) => {
                            return Err(ToolError::InvalidArgs(format!(
                                "Deserialization failed: {}",
                                e
                            )));
                        }
                    };

                    // Execute with cloned tool
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

    /// Return the tool's schema.
    pub fn schema(&self) -> ToolSchema {
        ToolSchema::new(&self.name, &self.description, self.args_schema.clone())
    }

    /// Execute this tool with the given arguments.
    async fn execute(
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
    use crate::event_sink::EventSink;
    use crate::tool::Tool;
    use duga_types::tool_call::CallId;
    use duga_types::tool_result::ToolResult;
    use serde::{Deserialize, Serialize};

    struct MockTool;

    #[derive(Debug, Default, Deserialize, Serialize)]
    struct MockArgs {
        msg: String,
    }

    impl Tool for MockTool {
        type Args = MockArgs;

        fn name(&self) -> &str {
            "mock"
        }

        fn description(&self) -> &str {
            "A mock tool for testing"
        }

        fn retryable(&self) -> bool {
            true
        }

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
        assert_eq!(schema.description, "A mock tool for testing");
    }

    #[test]
    fn test_dispatcher_register_and_lookup() {
        let tool = MockTool;
        let erased = ErasedTool::erase(tool);
        let dispatcher = ToolDispatcher::new();
        dispatcher.register_erased(erased).unwrap();

        assert!(dispatcher.get("mock").is_some());
        assert!(dispatcher.get("nonexistent").is_none());
        assert_eq!(dispatcher.names(), vec!["mock"]);
    }

    #[test]
    fn test_dispatcher_schemas() {
        let tool = MockTool;
        let erased = ErasedTool::erase(tool);
        let dispatcher = ToolDispatcher::new();
        dispatcher.register_erased(erased).unwrap();

        let schemas = dispatcher.schemas();
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].name, "mock");
    }

    #[test]
    fn test_dispatcher_duplicate_name() {
        let tool = MockTool;
        let erased = ErasedTool::erase(tool);
        let dispatcher = ToolDispatcher::new();
        dispatcher.register_erased(erased.clone()).unwrap();
        let result = dispatcher.register_erased(erased);
        assert!(result.is_err());
    }
}