//! `delegate` built-in tool — schema placeholder for LLM visibility.
//!
//! The LLM sees this tool in the tool list and can emit `delegate` tool
//! calls, but the actual delegation logic lives in `SimpleReActLoop`
//! (which intercepts the call before dispatching).
//!
//! The tool's `execute()` is a no-op that returns an error — this path
//! should never be reached because `SimpleReActLoop` intercepts first.

use duga_tools::context::ToolContext;
use duga_tools::result::ToolCallResult;
use duga_tools::Tool;
use duga_types::error::ToolError;
use schemars::JsonSchema;
use serde::Deserialize;

/// Arguments for the `delegate` tool call.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct DelegateArgs {
    /// Which loop to hand control to.
    /// The schema description is dynamically updated to list available loops.
    #[schemars(description = "Loop identifier. Check Available Strategies in system prompt.")]
    pub r#loop: String,

    /// Why this loop was chosen (for logs and observability).
    #[schemars(description = "Brief reason for choosing this strategy.")]
    pub reason: String,
}

/// A tool placeholder that makes the `delegate` command visible to the LLM.
///
/// The tool's schema description lists which loops are currently enabled.
/// Actual delegation is handled by `SimpleReActLoop::try_handle_delegate`.
pub struct DelegateTool {
    /// Loops currently enabled (from config), used to build the description.
    enabled_ids: Vec<String>,
    /// Cached description string built at construction time.
    cached_description: String,
}

impl DelegateTool {
    /// Create a new delegate tool with a description that lists enabled loops.
    ///
    /// When `enabled_ids` is empty the description tells the LLM
    /// that no specialized loops are available.
    pub fn new(enabled_ids: Vec<String>) -> Self {
        // Filter out "simple_react" — it's the entry point, not a delegation target.
        let filtered: Vec<String> = enabled_ids
            .into_iter()
            .filter(|id| id != "simple_react")
            .collect();

        let description = if filtered.is_empty() {
            "No specialized loops available. Use your standard tools \
             (think, shell, read, edit, write, search)."
                .to_string()
        } else {
            let list = filtered.join(", ");
            format!(
                "Hand control to a specialized loop for complex tasks. \
                 Available: {list}. Use when your current approach isn't optimal. \
                 The `loop` argument must be one of: {list}."
            )
        };

        Self {
            enabled_ids: filtered,
            cached_description: description,
        }
    }

    /// Return the list of enabled loop ids (for diagnostics).
    pub fn enabled_ids(&self) -> &[String] {
        &self.enabled_ids
    }
}

impl Tool for DelegateTool {
    type Args = DelegateArgs;

    fn name(&self) -> &str {
        "delegate"
    }

    fn description(&self) -> &str {
        &self.cached_description
    }

    fn retryable(&self) -> bool {
        false
    }

    async fn execute(&self, _ctx: ToolContext<'_>, _args: Self::Args) -> ToolCallResult {
        Err(ToolError::InvalidArgs(
            "delegate is handled by the agent loop — \
             this tool cannot be called directly"
                .into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delegate_tool_description(enabled: &[&str]) -> String {
        let ids: Vec<String> = enabled.iter().map(|s| s.to_string()).collect();
        let tool = DelegateTool::new(ids);
        tool.description().to_string()
    }

    #[test]
    fn no_loops_enabled() {
        let desc = delegate_tool_description(&[]);
        assert!(desc.contains("No specialized loops"));
        assert!(!desc.contains("Available:"));
    }

    #[test]
    fn two_loops_enabled() {
        let desc = delegate_tool_description(&["problem_solving", "verification"]);
        assert!(desc.contains("problem_solving"));
        assert!(desc.contains("verification"));
        assert!(desc.contains("Available:"));
    }

    #[test]
    fn simple_react_filtered() {
        let desc = delegate_tool_description(&["simple_react", "problem_solving"]);
        // simple_react should NOT appear
        assert!(!desc.contains("simple_react"));
        // problem_solving should appear
        assert!(desc.contains("problem_solving"));
    }

    #[test]
    fn single_loop_enabled() {
        let desc = delegate_tool_description(&["search"]);
        assert!(desc.contains("search"));
        assert!(desc.contains("Available: search"));
    }

    #[test]
    fn execute_returns_error() {
        let tool = DelegateTool::new(vec![]);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let workspace = duga_sandbox::Workspace::open(dir.path()).unwrap();
        let ctx = ToolContext::new(
            &workspace,
            duga_sandbox::CancellationToken::new(),
            &duga_tools::event_sink::NullSink,
        );

        let result = rt.block_on(tool.execute(ctx, DelegateArgs {
            r#loop: "test".into(),
            reason: "test".into(),
        }));

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgs(_)));
        assert!(err.to_string().contains("cannot be called directly"));
    }
}
