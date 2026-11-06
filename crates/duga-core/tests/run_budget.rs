use duga_core::loops::{
    DecompositionLoop, ProblemSolvingLoop, SearchLoop, SimpleReActLoop, VerificationLoop,
};
use duga_core::{Loop, LoopContext, LoopRegistry, Memory, Summarizer, SummaryFuture};
use duga_events::{EventSink, NullSink};
use duga_llm::{ChatFuture, LlmClient};
use duga_sandbox::{CancellationToken, Workspace};
use duga_tools::ToolDispatcher;
use duga_types::{
    config::AgentConfig,
    error::AgentError,
    llm::{LlmCallOptions, LlmResponse, TokenUsage},
    message::{AssistantMessage, Message},
    tool_call::ToolCall,
    tool_schema::ToolSchema,
};
use std::{sync::Arc, time::Duration};

struct Summary;
impl Summarizer for Summary {
    fn summarize<'a>(&'a self, _: &'a [Message]) -> SummaryFuture<'a> {
        Box::pin(async { Ok(duga_types::llm::SummaryMessage::new("summary".into())) })
    }
}
struct PendingLlm;
impl LlmClient for PendingLlm {
    fn count_tokens(&self, _: &[Message]) -> usize {
        0
    }
    fn chat<'a>(
        &'a self,
        _: &'a [Message],
        _: &'a [ToolSchema],
        _: LlmCallOptions,
        _: &'a dyn EventSink,
    ) -> ChatFuture<'a> {
        Box::pin(std::future::pending())
    }
}
fn strategies() -> Vec<Box<dyn Loop>> {
    vec![
        Box::new(SimpleReActLoop),
        Box::new(ProblemSolvingLoop),
        Box::new(SearchLoop),
        Box::new(VerificationLoop),
        Box::new(DecompositionLoop),
    ]
}
async fn run(
    strategy: &dyn Loop,
    config: &AgentConfig,
    llm: Arc<dyn LlmClient>,
    cancel: &CancellationToken,
) -> Result<duga_core::LoopResult, AgentError> {
    let dir = tempfile::tempdir().unwrap();
    let workspace = Workspace::open(dir.path()).unwrap();
    let mut memory = Memory::new(vec![Message::system("test")], 10000, 0.8, 0, 0);
    let tools = Arc::new(ToolDispatcher::new());
    let sink: Arc<dyn EventSink> = Arc::new(NullSink);
    let summarizer: Arc<dyn Summarizer> = Arc::new(Summary);
    let mut registry = LoopRegistry::new();
    duga_core::loops::register_default_loops(&mut registry);
    let registry = Arc::new(registry);
    let mut ctx = LoopContext {
        config,
        memory: &mut memory,
        llm: &llm,
        tools: &tools,
        workspace: &workspace,
        event_sink: &sink,
        summarizer: &summarizer,
        cancellation: cancel,
        registry: &registry,
        max_refinement_iterations: 2,
        max_delegation_depth: 2,
        delegation_depth: 0,
        steer: None,
        steer_limits: None,
    };
    strategy.run("test task".into(), &mut ctx).await
}
fn response(text: &str, tool: Option<&str>) -> Result<LlmResponse, duga_llm::LlmError> {
    Ok(LlmResponse {
        message: AssistantMessage {
            text: Some(text.into()),
            reasoning_content: None,
            tool_calls: tool
                .map(|name| {
                    vec![ToolCall::new(
                        name,
                        serde_json::json!({"loop": "verification"}),
                    )]
                })
                .unwrap_or_default(),
        },
        usage: TokenUsage {
            prompt: 0,
            completion: 0,
        },
    })
}

#[tokio::test]
async fn every_strategy_obeys_deadline_and_interrupts_pending_llm() {
    for strategy in strategies() {
        let mut config = AgentConfig::default();
        config.limits.max_runtime = Duration::from_millis(20);
        let result = run(
            strategy.as_ref(),
            &config,
            Arc::new(PendingLlm),
            &CancellationToken::new(),
        )
        .await;
        assert!(
            matches!(result, Err(AgentError::Timeout)),
            "{}: {result:?}",
            strategy.id()
        );
    }
}
#[tokio::test]
async fn every_strategy_cancels_pending_llm() {
    for strategy in strategies() {
        let cancel = CancellationToken::new();
        let signal = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            signal.cancel();
        });
        let result = run(
            strategy.as_ref(),
            &AgentConfig::default(),
            Arc::new(PendingLlm),
            &cancel,
        )
        .await;
        assert!(
            matches!(result, Err(AgentError::Cancelled)),
            "{}: {result:?}",
            strategy.id()
        );
    }
}
#[tokio::test]
async fn all_phases_share_step_and_tool_limits() {
    for strategy in strategies() {
        let mut config = AgentConfig::default();
        config.limits.max_steps = 1;
        let initial = if strategy.id() == "simple_react" {
            response("", Some("missing"))
        } else {
            response("[]", None)
        };
        let llm = Arc::new(duga_core::testing::MockLlm::new(vec![initial]));
        let result = run(strategy.as_ref(), &config, llm, &CancellationToken::new()).await;
        assert!(
            matches!(result, Err(AgentError::MaxStepsReached { .. })),
            "{}: {result:?}",
            strategy.id()
        );

        config.limits.max_steps = 20;
        config.limits.max_tool_calls = 0;
        let responses = if matches!(strategy.id(), "simple_react" | "verification") {
            vec![response("", Some("missing"))]
        } else {
            vec![response("[]", None), response("", Some("missing"))]
        };
        let result = run(
            strategy.as_ref(),
            &config,
            Arc::new(duga_core::testing::MockLlm::new(responses)),
            &CancellationToken::new(),
        )
        .await;
        assert!(
            matches!(result, Err(AgentError::MaxToolCallsReached { .. })),
            "{}: {result:?}",
            strategy.id()
        );
    }
}
#[tokio::test]
async fn delegation_does_not_refresh_parent_budget() {
    let mut config = AgentConfig::default();
    config.limits.max_steps = 1;
    let llm = Arc::new(duga_core::testing::MockLlm::new(vec![response(
        "",
        Some("delegate"),
    )]));
    let result = run(&SimpleReActLoop, &config, llm, &CancellationToken::new()).await;
    assert!(
        matches!(result, Err(AgentError::MaxStepsReached { .. })),
        "{result:?}"
    );
}
