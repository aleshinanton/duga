//! duga-tools: Tool trait system and dispatcher.
//!
//! This crate defines:
//! - `Tool` trait — the interface for all tool implementations
//! - `ToolContext` — runtime context provided to tool execution
//! - `ErasedTool` — type-erased tool wrapper for registration
//! - `ToolDispatcher` — registry and dispatch engine
//! - Schema validation helpers

pub mod context;
pub mod dispatcher;
pub mod erased;
pub mod event_sink;
pub mod result;
pub mod schema;
pub mod tool;

pub use context::ToolContext;
pub use dispatcher::{ToolDispatcher, ToolDispatcherError};
pub use erased::ErasedTool;
pub use tool::Tool;
