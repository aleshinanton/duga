//! Borrowed runtime context passed to every `Loop::run()` invocation.
//!
//! All fields are borrowed — the loop does not own any infrastructure.
//! `&mut Memory` is intentional: only one loop runs at a time, so
//! exclusive access is safe and delegation runs inline.

use crate::loop_registry::LoopRegistry;
use crate::memory::Memory;
use crate::summarizer::Summarizer;
use duga_events::EventSink;
use duga_llm::LlmClient;
use duga_sandbox::{CancellationToken, Workspace};
use duga_tools::ToolDispatcher;
use duga_types::config::AgentConfig;
use std::sync::Arc;

// LoopRegistry is defined in `loop_registry.rs` and re-exported here
// so that code consuming `LoopContext` doesn't need a separate import.

/// All runtime dependencies a loop needs to operate.
///
/// Every field is borrowed from the owning `BuiltRuntime` or equivalent —
/// the loop never owns infrastructure.  `delegation_depth` is the only
/// field that changes across delegation boundaries; the parent loop
/// constructs a new `LoopContext` with an incremented depth before
/// passing control to a child loop.
pub struct LoopContext<'a> {
    /// Agent configuration (limits, features, etc.).
    pub config: &'a AgentConfig,
    /// Conversation memory — exclusive &mut, one loop at a time.
    pub memory: &'a mut Memory,
    /// LLM client for chat calls.
    pub llm: &'a Arc<dyn LlmClient>,
    /// Tool dispatcher (shell, read, write, edit, search, think, delegate).
    pub tools: &'a Arc<ToolDispatcher>,
    /// Sandbox workspace for file I/O.
    pub workspace: &'a Workspace,
    /// Event sink for observability.
    pub event_sink: &'a Arc<dyn EventSink>,
    /// Summarizer for context compression.
    pub summarizer: &'a Arc<dyn Summarizer>,
    /// Cancellation token — checked before every operation.
    pub cancellation: &'a CancellationToken,
    /// The global loop registry (for the `delegate` tool).
    pub registry: &'a Arc<LoopRegistry>,
    /// Max refinement iterations (used by advanced loops).
    pub max_refinement_iterations: u32,
    /// Hard cap on delegation chain depth (from config).
    pub max_delegation_depth: u32,
    /// Current nesting depth in the delegation chain.
    /// Starts at 0 and increments on each delegation.
    pub delegation_depth: u32,
}

impl<'a> LoopContext<'a> {
    /// Build a child context with incremented `delegation_depth`.
    ///
    /// Re-borrows `memory` from the parent context — safe because the
    /// parent loop yields execution while the child runs.
    pub fn child<'b>(&'b mut self) -> LoopContext<'b>
    where
        'a: 'b,
    {
        LoopContext {
            config: self.config,
            memory: self.memory,
            llm: self.llm,
            tools: self.tools,
            workspace: self.workspace,
            event_sink: self.event_sink,
            summarizer: self.summarizer,
            cancellation: self.cancellation,
            registry: self.registry,
            max_refinement_iterations: self.max_refinement_iterations,
            max_delegation_depth: self.max_delegation_depth,
            delegation_depth: self.delegation_depth + 1,
        }
    }
}
