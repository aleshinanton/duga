//! Core runtime primitives for duga.

pub mod agent_loop;
pub mod loop_context;
pub mod loop_registry;
pub mod loop_result;
pub mod loop_trait;
pub mod loops;
pub mod memory;
pub mod summarizer;
pub mod testing;

#[deprecated(note = "use loop_trait::Loop + loop_result::LoopResult")]
pub use agent_loop::{AgentLoop, AgentRunResult};
pub use loop_context::LoopContext;
pub use loop_registry::LoopRegistry;
pub use loop_result::LoopResult;
pub use loop_trait::Loop;
pub use memory::Memory;
pub use summarizer::{Summarizer, SummaryFuture};
