//! The `Loop` trait — every agent strategy must implement this.
//!
//! Loops are registered in the `LoopRegistry` at startup.  The LLM selects
//! a loop via the `delegate` tool; `SimpleReActLoop` is always the entry point.

use crate::loop_context::LoopContext;
use crate::loop_result::LoopResult;
use duga_types::error::AgentError;
use std::future::Future;
use std::pin::Pin;

/// Type-erased future returned by `Loop::run()`.
pub type LoopRunFuture<'a> =
    Pin<Box<dyn Future<Output = Result<LoopResult, AgentError>> + Send + 'a>>;

/// A self-contained execution strategy for an agent task.
///
/// Implementations are stateless — all mutable state lives in `LoopContext`.
/// The trait is object-safe so loops can be stored as `Box<dyn Loop>`.
pub trait Loop: Send + Sync {
    /// Unique identifier, e.g. `"simple_react"`, `"problem_solving"`.
    fn id(&self) -> &'static str;

    /// Human-readable name for logs and observability.
    fn name(&self) -> &'static str;

    /// When to select this loop — injected into the system prompt so the
    /// LLM can make informed delegation decisions.
    fn description(&self) -> &'static str;

    /// Execute this loop for the given task.
    ///
    /// `ctx` carries all runtime dependencies (memory, LLM, tools, etc.).
    /// The loop is responsible for checking cancellation and limits.
    fn run<'a>(
        &'a self,
        task: String,
        ctx: &'a mut LoopContext<'a>,
    ) -> LoopRunFuture<'a>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// A trivial mock loop used to verify the trait is object-safe.
    struct MockLoop {
        id: &'static str,
        name: &'static str,
        desc: &'static str,
        result_text: String,
    }

    impl Loop for MockLoop {
        fn id(&self) -> &'static str {
            self.id
        }

        fn name(&self) -> &'static str {
            self.name
        }

        fn description(&self) -> &'static str {
            self.desc
        }

        fn run<'a>(
            &'a self,
            _task: String,
            _ctx: &'a mut LoopContext<'a>,
        ) -> LoopRunFuture<'a> {
            Box::pin(async move {
                Ok(LoopResult {
                    message: duga_types::message::AssistantMessage {
                        text: Some(self.result_text.clone()),
                        tool_calls: vec![],
                        reasoning_content: None,
                    },
                    steps: 1,
                    tool_calls: 0,
                    loop_id: self.id.to_string(),
                })
            })
        }
    }

    /// A mock LLM client so the test can build a `LoopContext`.
    struct DummyLlm;
    impl duga_llm::LlmClient for DummyLlm {
        fn model(&self) -> &str {
            "dummy"
        }
        fn chat<'a>(
            &'a self,
            _messages: &'a [duga_types::message::Message],
            _tools: &'a [duga_types::tool_schema::ToolSchema],
            _options: duga_types::llm::LlmCallOptions,
            _event_sink: &'a dyn duga_events::EventSink,
        ) -> duga_llm::ChatFuture<'a> {
            Box::pin(async { Err(duga_llm::LlmError::Provider("not implemented".into())) })
        }
        fn count_tokens(&self, _messages: &[duga_types::message::Message]) -> usize {
            0
        }
    }

    struct DummySummarizer;
    impl crate::summarizer::Summarizer for DummySummarizer {
        fn summarize<'a>(
            &'a self,
            _messages: &'a [duga_types::message::Message],
        ) -> crate::summarizer::SummaryFuture<'a> {
            Box::pin(async {
                Ok(duga_types::llm::SummaryMessage::new("summary".into()))
            })
        }
    }

    #[tokio::test]
    async fn mock_loop_compiles_and_runs() {
        let mock = MockLoop {
            id: "mock",
            name: "Mock Loop",
            desc: "For testing only",
            result_text: "mock result".into(),
        };

        // Build enough infrastructure for a LoopContext.
        let config = duga_types::config::AgentConfig::default();
        let mut memory = crate::Memory::new(
            vec![duga_types::message::Message::system("test")],
            10000,
            0.8,
            0,
            0,
        );
        let llm: Arc<dyn duga_llm::LlmClient> = Arc::new(DummyLlm);
        let tools = Arc::new(duga_tools::ToolDispatcher::new());
        let dir = tempfile::tempdir().unwrap();
        let workspace = duga_sandbox::Workspace::open(dir.path()).unwrap();
        let event_sink: Arc<dyn duga_events::EventSink> = Arc::new(duga_events::NullSink);
        let summarizer: Arc<dyn crate::Summarizer> = Arc::new(DummySummarizer);
        let cancellation = duga_sandbox::CancellationToken::new();
        let registry = Arc::new(crate::loop_registry::LoopRegistry::new());

        let mut ctx = LoopContext {
            config: &config,
            memory: &mut memory,
            llm: &llm,
            tools: &tools,
            workspace: &workspace,
            event_sink: &event_sink,
            summarizer: &summarizer,
            cancellation: &cancellation,
            registry: &registry,
            max_refinement_iterations: 3,
            max_delegation_depth: 2,
            delegation_depth: 0,
        };

        // Verify trait basics.
        assert_eq!(mock.id(), "mock");
        assert_eq!(mock.name(), "Mock Loop");
        assert_eq!(mock.description(), "For testing only");

        let result = mock.run("dummy task".into(), &mut ctx).await.unwrap();
        assert_eq!(result.message.text, Some("mock result".into()));
        assert_eq!(result.loop_id, "mock");
        assert_eq!(result.steps, 1);
    }

    #[test]
    fn loop_trait_is_object_safe() {
        // Compile-time check: if the trait were not object-safe this
        // would fail with "the trait cannot be made into an object".
        fn _accept(_: Box<dyn Loop>) {}
        fn _accept_ref(_: &dyn Loop) {}
    }
}
