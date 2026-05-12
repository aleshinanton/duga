//! Result type for tool execution.
//!
//! Wrapper around `Result<ToolResult, ToolError>` with helper constructors.

use duga_types::error::ToolError;
use duga_types::tool_result::ToolResult;

/// Result returned by tool `execute()` methods.
pub type ToolCallResult = Result<ToolResult, ToolError>;

impl ToolCallResult {
    pub fn is_ok(&self) -> bool {
        matches!(self, Ok(_))
    }

    pub fn is_err(&self) -> bool {
        matches!(self, Err(_))
    }

    pub fn is_transient_error(&self) -> bool {
        matches!(self, Err(e)) && e.is_transient()
    }
}