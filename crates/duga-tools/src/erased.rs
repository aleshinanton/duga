//! Type-erased tool wrapper for registration and dispatch.

use crate::context::ToolContext;
use crate::event_sink::EventSink;
use crate::result::ToolCallResult;
use crate::schema::{deserialize_args, sanitize_schema, validate_tool_args};
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
type ErasedFuture<'a> = Pin<Box<dyn Future<Output = ToolCallResult> + Send + 'a>>;

trait ErasedExecute: Send + Sync {
    fn execute<'a>(
        &self,
        call_id: CallId,
        raw_args: Value,
        workspace: &'a Workspace,
        cancellation: CancellationToken,
        event_sink: &'a dyn EventSink,
    ) -> ErasedFuture<'a>;

    /// Reset per-run tool state (e.g., think counters).
    fn reset_limits(&self) {}
}

struct ErasedExecutor<T> {
    tool: Arc<T>,
}

impl<T: crate::tool::Tool + 'static> ErasedExecute for ErasedExecutor<T> {
    fn reset_limits(&self) {
        self.tool.reset_limits();
    }

    fn execute<'a>(
        &self,
        call_id: CallId,
        raw_args: Value,
        workspace: &'a Workspace,
        cancellation: CancellationToken,
        event_sink: &'a dyn EventSink,
    ) -> ErasedFuture<'a> {
        let tool = self.tool.clone();
        Box::pin(async move {
            let args: T::Args = match deserialize_args(raw_args) {
                Ok(a) => a,
                Err(e) => {
                    return Err(ToolError::InvalidArgs(format!(
                        "Argument deserialization failed: {}",
                        e
                    )));
                }
            };

            let ctx = ToolContext::new(workspace, cancellation, event_sink);
            let mut result = tool.execute(ctx, args).await?;
            result.tool_call_id = call_id;
            Ok(result)
        })
    }
}

/// A type-erased tool for registration in `ToolDispatcher`.
#[derive(Clone)]
pub struct ErasedTool {
    pub name: String,
    pub description: String,
    pub args_schema: Value,
    pub retryable: bool,
    execute: Arc<dyn ErasedExecute>,
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
        let mut args_schema = tool.json_schema();
        sanitize_schema(&mut args_schema);
        let retryable = tool.retryable();

        let execute = Arc::new(ErasedExecutor {
            tool: Arc::new(tool),
        });

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

    pub fn validate_args(&self, raw_args: &Value) -> Result<(), ToolError> {
        validate_tool_args(&self.args_schema, raw_args).map_err(ToolError::InvalidArgs)
    }

    pub async fn execute(
        &self,
        call_id: CallId,
        raw_args: Value,
        workspace: &Workspace,
        cancellation: CancellationToken,
        event_sink: &dyn EventSink,
    ) -> ToolCallResult {
        self.validate_args(&raw_args)?;

        self.execute
            .execute(call_id, raw_args, workspace, cancellation, event_sink)
            .await
    }

    /// Reset per-run limits for this tool (e.g., think call counters).
    pub fn reset_limits(&self) {
        self.execute.reset_limits();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::confirmation::{
        ConfirmationDecision, ConfirmationMiddleware, ConfirmationPolicy, ConfirmationProvider,
        ConfirmationRequest,
    };
    use crate::context::ToolContext;
    use crate::dispatcher::ToolDispatcher;
    use crate::event_sink::NullSink;
    use crate::tool::Tool;
    use duga_sandbox::{CancellationToken, Workspace};
    use duga_types::tool_call::CallId;
    use duga_types::tool_call::ToolCall;
    use duga_types::tool_result::ToolResult;
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};
    use std::sync::Arc;
    use std::time::Duration;

    struct MockTool;

    #[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
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
                steering_hint: None,
            })
        }
    }

    struct StaticProvider(ConfirmationDecision);

    #[async_trait::async_trait]
    impl ConfirmationProvider for StaticProvider {
        async fn confirm(
            &self,
            _request: &ConfirmationRequest,
            _timeout: Duration,
        ) -> ConfirmationDecision {
            self.0.clone()
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

    #[tokio::test]
    async fn dispatch_rejects_non_object_args() {
        let dispatcher = ToolDispatcher::new();
        dispatcher
            .register_erased(ErasedTool::erase(MockTool))
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let workspace = Workspace::open(dir.path()).unwrap();
        let call = ToolCall::new("mock", serde_json::json!("bad"));

        let err = dispatcher
            .dispatch(&call, &workspace, CancellationToken::new(), &NullSink)
            .await
            .unwrap_err();

        assert!(matches!(err, ToolError::InvalidArgs(message) if message.contains("JSON object")));
    }

    #[tokio::test]
    async fn dispatch_rejects_schema_violations_before_tool_runs() {
        let dispatcher = ToolDispatcher::new();
        dispatcher
            .register_erased(ErasedTool::erase(MockTool))
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let workspace = Workspace::open(dir.path()).unwrap();
        let call = ToolCall::new("mock", serde_json::json!({"unexpected": "field"}));

        let err = dispatcher
            .dispatch(&call, &workspace, CancellationToken::new(), &NullSink)
            .await
            .unwrap_err();

        assert!(
            matches!(err, ToolError::InvalidArgs(message) if message.contains("Schema validation failed"))
        );
    }

    #[tokio::test]
    async fn dispatch_accepts_valid_args() {
        let dispatcher = ToolDispatcher::new();
        dispatcher
            .register_erased(ErasedTool::erase(MockTool))
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let workspace = Workspace::open(dir.path()).unwrap();
        let call = ToolCall::new("mock", serde_json::json!({"msg": "hello"}));

        let result = dispatcher
            .dispatch(&call, &workspace, CancellationToken::new(), &NullSink)
            .await
            .unwrap();

        assert_eq!(result.tool_call_id, call.id);
        assert_eq!(result.output, "Mock received: hello");
    }

    #[tokio::test]
    async fn dispatch_denies_when_confirmation_denied() {
        let dispatcher = ToolDispatcher::new();
        dispatcher
            .register_erased(ErasedTool::erase(MockTool))
            .unwrap();
        dispatcher.set_confirmation(ConfirmationMiddleware::new(
            ConfirmationPolicy::new(["mock"], Duration::from_secs(1)),
            Arc::new(StaticProvider(ConfirmationDecision::Denied)),
        ));

        let dir = tempfile::tempdir().unwrap();
        let workspace = Workspace::open(dir.path()).unwrap();
        let call = ToolCall::new("mock", serde_json::json!({"msg": "hello"}));

        let err = dispatcher
            .dispatch(&call, &workspace, CancellationToken::new(), &NullSink)
            .await
            .unwrap_err();

        assert!(matches!(err, ToolError::Denied(message) if message.contains("mock")));
    }

    /// A tool that counts how many times reset_limits was called.
    struct ResetCountTool {
        name: String,
        reset_count: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl ResetCountTool {
        fn new(name: &str) -> Self {
            Self {
                name: name.into(),
                reset_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            }
        }
    }

    #[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
    struct ResetCountArgs {
        msg: String,
    }

    impl Tool for ResetCountTool {
        type Args = ResetCountArgs;
        fn name(&self) -> &str {
            &self.name
        }
        fn description(&self) -> &str {
            "Counts resets"
        }
        fn reset_limits(&self) {
            self.reset_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        async fn execute(&self, _ctx: ToolContext<'_>, args: Self::Args) -> ToolCallResult {
            Ok(ToolResult {
                tool_call_id: CallId::new(),
                success: true,
                output: format!("reset_count={} msg={}", self.reset_count.load(std::sync::atomic::Ordering::SeqCst), args.msg),
                metadata: serde_json::Value::Null,
                duration_ms: 0,
                stdout_bytes: 0,
                stderr_bytes: 0,
                truncated: false,
                steering_hint: None,
            })
        }
    }

    #[test]
    fn dispatcher_reset_limits_calls_all_tools() {
        let dispatcher = ToolDispatcher::new();
        let tool1 = ResetCountTool::new("reset_a");
        let tool2 = ResetCountTool::new("reset_b");
        let counter1 = Arc::clone(&tool1.reset_count);
        let counter2 = Arc::clone(&tool2.reset_count);
        dispatcher
            .register_erased(ErasedTool::erase(tool1))
            .unwrap();
        dispatcher
            .register_erased(ErasedTool::erase(tool2))
            .unwrap();

        // reset_limits on dispatcher should call each tool's reset_limits.
        dispatcher.reset_limits();
        assert_eq!(counter1.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(counter2.load(std::sync::atomic::Ordering::SeqCst), 1);

        // Second call increments again.
        dispatcher.reset_limits();
        assert_eq!(counter1.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(counter2.load(std::sync::atomic::Ordering::SeqCst), 2);
    }
}
