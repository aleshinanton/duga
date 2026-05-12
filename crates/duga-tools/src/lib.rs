//! duga-tools: Tool trait system and dispatcher.
//!
//! This crate defines:
//! - `Tool` trait — the interface for all tool implementations
//! - `ToolContext` — runtime context provided to tool execution
//! - `ErasedTool` — type-erased tool wrapper for registration
//! - `ToolDispatcher` — registry and dispatch engine
//! - Schema validation helpers

pub mod tool;
pub mod context;
pub mod event_sink;
pub mod erased;
pub mod dispatcher;
pub mod schema;
pub mod result;

pub use context::ToolContext;
pub use dispatcher::{ToolDispatcher, ToolDispatcherError};
pub use tool::Tool;