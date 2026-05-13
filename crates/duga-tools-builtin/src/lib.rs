//! Built-in tools for duga: read, write, bash, search, think.
//!
//! This crate implements all five built-in tools specified in §11 of the architecture.
//! Each tool implements the `Tool` trait and uses the security primitives from
//! `duga-sandbox` (Workspace, BinaryRegistry, ShellSession, run_captured).

pub mod bash;
pub mod read;
pub mod search;
pub mod think;
pub mod write;

use duga_sandbox::{binary_registry::BinaryRegistry, Workspace};
use duga_tools::{ErasedTool, ToolDispatcher, ToolDispatcherError};
use duga_types::config::{OutputLimits, ThinkLimits as AgentThinkLimits};
use std::sync::Arc;
use std::time::Duration;

pub fn register_builtin_tools(
    dispatcher: &ToolDispatcher,
    registry: Arc<BinaryRegistry>,
    workspace: Arc<Workspace>,
    output_limits: OutputLimits,
    sandbox_timeout: Duration,
    think_limits: AgentThinkLimits,
) -> Result<(), ToolDispatcherError> {
    dispatcher.register_erased(ErasedTool::erase(read::ReadTool::new()))?;
    dispatcher.register_erased(ErasedTool::erase(write::WriteTool::new()))?;
    dispatcher.register_erased(ErasedTool::erase(search::SearchTool::new()))?;
    dispatcher.register_erased(ErasedTool::erase(think::ThinkTool::new(
        think::ThinkLimits {
            max_calls: think_limits.max_calls as usize,
            max_tokens: think_limits.max_tokens as usize,
        },
    )))?;
    dispatcher.register_erased(ErasedTool::erase(bash::BashTool::with_sandbox(
        registry,
        workspace,
        output_limits,
        sandbox_timeout,
    )))?;
    Ok(())
}

#[cfg(test)]
mod tests;
