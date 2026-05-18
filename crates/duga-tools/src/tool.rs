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

    /// Tool name (e.g. "read", "write", "bash").
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
