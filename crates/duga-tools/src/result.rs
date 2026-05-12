//! Result type for tool execution.

use duga_types::error::ToolError;
use duga_types::tool_result::ToolResult;

pub type ToolCallResult = Result<ToolResult, ToolError>;
