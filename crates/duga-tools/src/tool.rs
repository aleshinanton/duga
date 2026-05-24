//! Tool trait — the interface for all tool implementations.
//!
//! Every tool (built-in or WASM plugin) implements `Tool`.
//!
//! **Trait object safety note:** `type Args` makes `dyn Tool` NOT object-safe
//! for `execute()` dispatch. Use `ErasedTool` (from `erased` module) for
//! type-erased registration and dispatch. The concrete `Tool` impls are
//! still used directly in tests and for schema generation.

use crate::context::ToolContext;
use crate::result::ToolCallResult;
use duga_types::tool_schema::ToolSchema;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// The `Tool` trait — all tools implement this.
pub trait Tool: Send + Sync {
    /// Arguments type for this tool.
    type Args: DeserializeOwned + JsonSchema + Send + 'static;

    /// Tool name (e.g. "read", "write", "shell").
    fn name(&self) -> &str;

    /// Human-readable description for the LLM.
    fn description(&self) -> &str;

    /// Whether this tool is safe to retry on transient errors.
    fn retryable(&self) -> bool {
        false
    }

    /// Execute the tool with the given arguments.
    fn execute(
        &self,
        ctx: ToolContext<'_>,
        args: Self::Args,
    ) -> impl std::future::Future<Output = ToolCallResult> + Send;

    /// Generate the JSON Schema for the arguments.
    fn json_schema(&self) -> Value {
        crate::schema::generate_args_schema::<Self::Args>()
    }

    /// Build a `ToolSchema` from the tool's metadata.
    fn tool_schema(&self) -> ToolSchema {
        ToolSchema::new(self.name(), self.description(), self.json_schema())
    }

    /// Reset per-run limits/state. Called by the agent loop before each run.
    /// Tools with usage counters (e.g. `think`) should reset them here.
    fn reset_limits(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ToolContext;
    use duga_sandbox::CancellationToken;
    use duga_types::tool_call::CallId;
    use schemars::JsonSchema;
    use serde::Deserialize;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Minimal tool to verify trait methods compile and behave correctly.
    struct TestTool {
        name: &'static str,
        desc: &'static str,
        retryable: bool,
        reset_count: Arc<AtomicUsize>,
    }

    #[derive(Deserialize, JsonSchema)]
    struct TestArgs {
        input: String,
    }

    impl Tool for TestTool {
        type Args = TestArgs;

        fn name(&self) -> &str {
            self.name
        }

        fn description(&self) -> &str {
            self.desc
        }

        fn retryable(&self) -> bool {
            self.retryable
        }

        fn reset_limits(&self) {
            self.reset_count.fetch_add(1, Ordering::SeqCst);
        }

        async fn execute(
            &self,
            _ctx: ToolContext<'_>,
            args: Self::Args,
        ) -> ToolCallResult {
            Ok(duga_types::tool_result::ToolResultBuilder::new()
                .tool_call_id(CallId::new())
                .success(true)
                .output(args.input)
                .build()
                .unwrap())
        }
    }

    #[test]
    fn tool_name_and_description() {
        let tool = TestTool { name: "test_tool", desc: "A test tool.", retryable: false, reset_count: Arc::new(AtomicUsize::new(0)) };
        assert_eq!(tool.name(), "test_tool");
        assert_eq!(tool.description(), "A test tool.");
    }

    #[test]
    fn tool_retryable_defaults_to_false() {
        let tool = TestTool { name: "t", desc: "d", retryable: false, reset_count: Arc::new(AtomicUsize::new(0)) };
        assert!(!tool.retryable());

        let tool = TestTool { name: "t", desc: "d", retryable: true, reset_count: Arc::new(AtomicUsize::new(0)) };
        assert!(tool.retryable());
    }

    #[test]
    fn tool_schema_generates_valid_json_schema() {
        let tool = TestTool { name: "test_tool", desc: "A test tool.", retryable: false, reset_count: Arc::new(AtomicUsize::new(0)) };
        let schema = tool.tool_schema();
        assert_eq!(schema.name, "test_tool");
        assert_eq!(schema.description, "A test tool.");
        // JSON Schema should be a valid object
        assert!(schema.args_schema.is_object());
        assert_eq!(schema.args_schema["type"], "object");
    }

    #[test]
    fn tool_json_schema_is_object() {
        let tool = TestTool { name: "test_tool", desc: "A test tool.", retryable: false, reset_count: Arc::new(AtomicUsize::new(0)) };
        let args_schema = tool.json_schema();
        assert!(args_schema.is_object());
        assert!(args_schema.get("properties").is_some(), "should have properties");
    }

    #[test]
    fn tool_reset_limits_called() {
        let counter = Arc::new(AtomicUsize::new(0));
        let tool = TestTool { name: "t", desc: "d", retryable: false, reset_count: counter.clone() };
        assert_eq!(counter.load(Ordering::SeqCst), 0);
        tool.reset_limits();
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        tool.reset_limits();
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    struct TestEventSink;
    impl crate::event_sink::EventSink for TestEventSink {}

    #[tokio::test]
    async fn tool_execute_ok() {
        let tool = TestTool { name: "t", desc: "d", retryable: false, reset_count: Arc::new(AtomicUsize::new(0)) };
        let dir = tempfile::tempdir().unwrap();
        let ws = duga_sandbox::Workspace::open(dir.path()).unwrap();
        let cancel = CancellationToken::new();
        let sink = TestEventSink;
        let ctx = ToolContext::new(&ws, cancel, &sink);
        let result = tool.execute(ctx, TestArgs { input: "hello".into() }).await.unwrap();
        assert!(result.success);
        assert_eq!(result.output, "hello");
    }
}
