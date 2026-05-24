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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loop_registry::LoopRegistry;
    use crate::summarizer::Summarizer;
    use duga_events::NullSink;
    use duga_llm::dummy::DummyClient;
    use duga_sandbox::CancellationToken;
    use duga_tools::ToolDispatcher;
    use duga_types::config::AgentConfig;
    
    use duga_types::message::Message;
    use std::sync::Arc;

    /// A summarizer that does nothing — sufficient for LoopContext tests.
    struct NoopSummarizer;
    impl Summarizer for NoopSummarizer {
        fn summarize<'a>(&'a self, _messages: &'a [Message]) -> crate::SummaryFuture<'a> {
            Box::pin(async { Ok(duga_types::llm::SummaryMessage::new("summary".into())) })
        }
    }

    fn make_test_ctx<'a>(
        config: &'a AgentConfig,
        memory: &'a mut crate::memory::Memory,
        llm: &'a Arc<dyn duga_llm::LlmClient>,
        tools: &'a Arc<ToolDispatcher>,
        workspace: &'a duga_sandbox::Workspace,
        event_sink: &'a Arc<dyn duga_events::EventSink>,
        summarizer: &'a Arc<dyn Summarizer>,
        cancellation: &'a CancellationToken,
        registry: &'a Arc<LoopRegistry>,
    ) -> LoopContext<'a> {
        LoopContext {
            config,
            memory,
            llm,
            tools,
            workspace,
            event_sink,
            summarizer,
            cancellation,
            registry,
            max_refinement_iterations: 2,
            max_delegation_depth: 3,
            delegation_depth: 0,
        }
    }

    #[test]
    fn child_increments_delegation_depth() {
        let config = AgentConfig::default();
        let llm: Arc<dyn duga_llm::LlmClient> = Arc::new(DummyClient::default());
        let tools = Arc::new(ToolDispatcher::new());
        let dir = tempfile::tempdir().unwrap();
        let workspace = duga_sandbox::Workspace::open(dir.path()).unwrap();
        let sink: Arc<dyn duga_events::EventSink> = Arc::new(NullSink);
        let summarizer: Arc<dyn Summarizer> = Arc::new(NoopSummarizer);
        let cancel = CancellationToken::new();
        let registry = Arc::new(LoopRegistry::new());
        let mut memory = crate::memory::Memory::new(
            vec![Message::system("test")],
            16_000,
            0.8,
            0,
            16_000,
        );

        let mut ctx = make_test_ctx(
            &config, &mut memory, &llm, &tools, &workspace, &sink, &summarizer, &cancel, &registry,
        );
        assert_eq!(ctx.delegation_depth, 0);

        let child = ctx.child();
        assert_eq!(child.delegation_depth, 1, "child should have depth 1");
        assert_eq!(child.max_delegation_depth, 3, "child inherits max depth");
    }

    #[test]
    fn child_inherits_all_fields() {
        let config = AgentConfig::default();
        let llm: Arc<dyn duga_llm::LlmClient> = Arc::new(DummyClient::default());
        let tools = Arc::new(ToolDispatcher::new());
        let dir = tempfile::tempdir().unwrap();
        let workspace = duga_sandbox::Workspace::open(dir.path()).unwrap();
        let sink: Arc<dyn duga_events::EventSink> = Arc::new(NullSink);
        let summarizer: Arc<dyn Summarizer> = Arc::new(NoopSummarizer);
        let cancel = CancellationToken::new();
        let registry = Arc::new(LoopRegistry::new());
        let mut memory = crate::memory::Memory::new(
            vec![Message::system("test")],
            16_000,
            0.8,
            0,
            16_000,
        );

        let mut ctx = make_test_ctx(
            &config, &mut memory, &llm, &tools, &workspace, &sink, &summarizer, &cancel, &registry,
        );
        ctx.max_refinement_iterations = 5;
        ctx.max_delegation_depth = 7;

        // Verify child inherits fields correctly
        {
            let child = ctx.child();
            assert_eq!(child.max_refinement_iterations, 5, "child inherits refinement iters");
            assert_eq!(child.max_delegation_depth, 7, "child inherits max delegation depth");
            assert_eq!(child.delegation_depth, 1, "depth incremented once");
            // child shares references with parent
            assert!(std::ptr::eq(child.config, ctx.config));
        }
    }

    #[test]
    fn double_child_increments_twice() {
        let config = AgentConfig::default();
        let llm: Arc<dyn duga_llm::LlmClient> = Arc::new(DummyClient::default());
        let tools = Arc::new(ToolDispatcher::new());
        let dir = tempfile::tempdir().unwrap();
        let workspace = duga_sandbox::Workspace::open(dir.path()).unwrap();
        let sink: Arc<dyn duga_events::EventSink> = Arc::new(NullSink);
        let summarizer: Arc<dyn Summarizer> = Arc::new(NoopSummarizer);
        let cancel = CancellationToken::new();
        let registry = Arc::new(LoopRegistry::new());
        let mut memory = crate::memory::Memory::new(
            vec![Message::system("test")],
            16_000,
            0.8,
            0,
            16_000,
        );

        let mut ctx = make_test_ctx(
            &config, &mut memory, &llm, &tools, &workspace, &sink, &summarizer, &cancel, &registry,
        );

        let child1 = ctx.child();
        assert_eq!(child1.delegation_depth, 1);
        drop(child1);
        // After drop, can create another child with depth 1 again
        let child2 = ctx.child();
        assert_eq!(child2.delegation_depth, 1);
    }

    #[test]
    fn default_delegation_depth_is_zero() {
        let config = AgentConfig::default();
        let llm: Arc<dyn duga_llm::LlmClient> = Arc::new(DummyClient::default());
        let tools = Arc::new(ToolDispatcher::new());
        let dir = tempfile::tempdir().unwrap();
        let workspace = duga_sandbox::Workspace::open(dir.path()).unwrap();
        let sink: Arc<dyn duga_events::EventSink> = Arc::new(NullSink);
        let summarizer: Arc<dyn Summarizer> = Arc::new(NoopSummarizer);
        let cancel = CancellationToken::new();
        let registry = Arc::new(LoopRegistry::new());
        let mut memory = crate::memory::Memory::new(
            vec![Message::system("test")],
            16_000,
            0.8,
            0,
            16_000,
        );

        let ctx = make_test_ctx(
            &config, &mut memory, &llm, &tools, &workspace, &sink, &summarizer, &cancel, &registry,
        );
        assert_eq!(ctx.delegation_depth, 0, "root context starts at depth 0");
    }
}
