//! Integration tests: verify loop_id is included in responses when
//! a specialized loop handles the request.

use duga_core::loop_context::LoopContext;
use duga_core::loop_registry::LoopRegistry;
use duga_core::loops::{
    register_default_loops, ProblemSolvingLoop, SimpleReActLoop,
};
use duga_core::testing::{CapturingEventSink, MockLlm};
use duga_core::{Loop, Summarizer};
use duga_events::{Event, EventSink};
use duga_sandbox::{CancellationToken, Workspace};
use duga_tools::ToolDispatcher;
use duga_types::config::{AgentConfig, LoopConfig};
use duga_types::llm::{LlmResponse, SummaryMessage, TokenUsage};
use duga_types::message::{AssistantMessage, Message};
use duga_types::tool_call::ToolCall;
use std::sync::Arc;

fn text_response(text: &str) -> LlmResponse {
    LlmResponse {
        message: AssistantMessage {
            text: Some(text.into()),
            tool_calls: vec![],
            reasoning_content: None,
        },
        usage: TokenUsage { prompt: 1, completion: 1 },
    }
}

fn tool_call_response(calls: Vec<ToolCall>) -> LlmResponse {
    LlmResponse {
        message: AssistantMessage {
            text: None,
            tool_calls: calls,
            reasoning_content: None,
        },
        usage: TokenUsage { prompt: 1, completion: 1 },
    }
}

fn delegate_call(target: &str, reason: &str) -> ToolCall {
    ToolCall::new("delegate", serde_json::json!({"loop": target, "reason": reason}))
}

struct TestSummarizer;
impl Summarizer for TestSummarizer {
    fn summarize<'a>(&'a self, _: &'a [Message]) -> duga_core::SummaryFuture<'a> {
        Box::pin(async { Ok(SummaryMessage::new("summary".into())) })
    }
}

fn format_response(result: &duga_core::LoopResult) -> String {
    let mut text = result.message.text.clone().unwrap_or_default();
    if result.loop_id != "simple_react" {
        text.push_str(&format!("\n\n⟳ via *{}* loop", result.loop_id));
    }
    text
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn simple_react_response_has_no_loop_annotation() {
    let llm: Arc<dyn duga_llm::LlmClient> = Arc::new(MockLlm::new(vec![
        Ok(text_response("here is the answer")),
    ]));

    let result = run_with_llm(llm, 3, 2, "task").await;
    let formatted = format_response(&result);

    assert_eq!(result.loop_id, "simple_react");
    assert!(!formatted.contains("⟳ via"));
    assert!(formatted.contains("here is the answer"));
}

#[tokio::test]
async fn problem_solving_response_includes_loop_annotation() {
    let llm: Arc<dyn duga_llm::LlmClient> = Arc::new(MockLlm::new(vec![
        // SimpleReAct delegates to problem_solving
        Ok(tool_call_response(vec![delegate_call(
            "problem_solving",
            "multi-step task",
        )])),
        // problem_solving: plan
        Ok(text_response(r#"["Step 1"]"#)),
        // execute
        Ok(text_response("ALL STEPS COMPLETE")),
        // audit
        Ok(text_response(
            r#"{"complete": true, "final_answer": "solution found", "gaps": null}"#,
        )),
    ]));

    let result = run_with_llm(llm, 3, 2, "complex task").await;
    let formatted = format_response(&result);

    assert_eq!(result.loop_id, "problem_solving");
    assert!(formatted.contains("⟳ via *problem_solving* loop"));
    assert!(formatted.contains("solution found"));
}

#[tokio::test]
async fn verification_response_includes_loop_annotation() {
    let llm: Arc<dyn duga_llm::LlmClient> = Arc::new(MockLlm::new(vec![
        Ok(tool_call_response(vec![delegate_call(
            "verification",
            "factual check",
        )])),
        Ok(text_response("Answer 1: 42")),
        Ok(text_response("Answer 2: 42")),
        Ok(text_response("Consensus: 42")),
    ]));

    let result = run_with_llm(llm, 2, 2, "question").await;
    let formatted = format_response(&result);

    assert_eq!(result.loop_id, "verification");
    assert!(formatted.contains("⟳ via *verification* loop"));
}

#[tokio::test]
async fn delegation_event_emitted_with_correct_fields() {
    let llm: Arc<dyn duga_llm::LlmClient> = Arc::new(MockLlm::new(vec![
        Ok(tool_call_response(vec![delegate_call(
            "problem_solving",
            "for complex work",
        )])),
        Ok(text_response(r#"["Step 1"]"#)),
        Ok(text_response("ALL STEPS COMPLETE")),
        Ok(text_response(
            r#"{"complete": true, "final_answer": "done", "gaps": null}"#,
        )),
    ]));

    let (result, events) = run_with_events(llm, 3, 2, "task").await;

    assert_eq!(result.loop_id, "problem_solving");

    let delegations: Vec<&Event> = events
        .iter()
        .filter(|e| matches!(e, Event::LoopDelegated { .. }))
        .collect();

    assert_eq!(delegations.len(), 1);
    match delegations[0] {
        Event::LoopDelegated { from, to, reason, depth } => {
            assert_eq!(from, "simple_react");
            assert_eq!(to, "problem_solving");
            assert!(!reason.is_empty(), "reason should be task text");
            assert_eq!(*depth, 1);
        }
        _ => panic!("wrong event"),
    }
}

#[tokio::test]
async fn cancelled_delegation_falls_back_to_simple_react() {
    let llm: Arc<dyn duga_llm::LlmClient> = Arc::new(MockLlm::new(vec![
        // LLM tries to delegate to unknown loop
        Ok(tool_call_response(vec![delegate_call(
            "nonexistent",
            "try",
        )])),
        // LLM receives error, handles directly
        Ok(text_response("I'll do it myself")),
    ]));

    let result = run_with_llm(llm, 3, 2, "task").await;
    let formatted = format_response(&result);

    // Should be simple_react since delegation failed
    assert_eq!(result.loop_id, "simple_react");
    assert!(!formatted.contains("⟳ via"));
    assert!(formatted.contains("do it myself"));
}

// ── Helpers ─────────────────────────────────────────────────────────────────

async fn run_with_llm(
    llm: Arc<dyn duga_llm::LlmClient>,
    max_refinement: u32,
    max_depth: u32,
    task: &str,
) -> duga_core::LoopResult {
    let config = Box::leak(Box::new(AgentConfig {
        loop_config: LoopConfig {
            enabled_loops: vec![
                "problem_solving".into(),
                "verification".into(),
                "decomposition".into(),
                "search".into(),
            ],
            max_refinement_iterations: max_refinement,
            max_delegation_depth: max_depth,
        },
        ..AgentConfig::default()
    }));
    let memory = Box::leak(Box::new(duga_core::Memory::new(
        vec![Message::system("system")],
        100_000, 0.8, 0, 0,
    )));
    let llm: &'static Arc<dyn duga_llm::LlmClient> = Box::leak(Box::new(llm));
    let tools = Arc::new(ToolDispatcher::new());
    let tools: &'static Arc<ToolDispatcher> = Box::leak(Box::new(tools));
    let dir = tempfile::tempdir().unwrap();
    let workspace = Box::leak(Box::new(Workspace::open(dir.path()).unwrap()));
    let sink: Arc<dyn EventSink> = Arc::new(CapturingEventSink::new());
    let sink: &'static Arc<dyn EventSink> = Box::leak(Box::new(sink));
    let summarizer: Arc<dyn Summarizer> = Arc::new(TestSummarizer);
    let summarizer: &'static Arc<dyn Summarizer> = Box::leak(Box::new(summarizer));
    let mut registry = LoopRegistry::new();
    registry.register(Box::new(SimpleReActLoop)).unwrap();
    register_default_loops(&mut registry);
    let registry: &'static Arc<LoopRegistry> = Box::leak(Box::new(Arc::new(registry)));
    let cancel: &'static CancellationToken = Box::leak(Box::new(CancellationToken::new()));

    let mut ctx = LoopContext {
        config,
        memory,
        llm,
        tools,
        workspace,
        event_sink: sink,
        summarizer,
        cancellation: cancel,
        registry,
        max_refinement_iterations: max_refinement,
        max_delegation_depth: max_depth,
        delegation_depth: 0,
    };

    SimpleReActLoop.run(task.to_string(), &mut ctx).await.unwrap()
}

async fn run_with_events(
    llm: Arc<dyn duga_llm::LlmClient>,
    max_refinement: u32,
    max_depth: u32,
    task: &str,
) -> (duga_core::LoopResult, Vec<Event>) {
    let config = Box::leak(Box::new(AgentConfig {
        loop_config: LoopConfig {
            enabled_loops: vec!["problem_solving".into()],
            max_refinement_iterations: max_refinement,
            max_delegation_depth: max_depth,
        },
        ..AgentConfig::default()
    }));
    let memory = Box::leak(Box::new(duga_core::Memory::new(
        vec![Message::system("system")],
        100_000, 0.8, 0, 0,
    )));
    let llm: &'static Arc<dyn duga_llm::LlmClient> = Box::leak(Box::new(llm));
    let tools: &'static Arc<ToolDispatcher> = Box::leak(Box::new(Arc::new(ToolDispatcher::new())));
    let dir = tempfile::tempdir().unwrap();
    let workspace = Box::leak(Box::new(Workspace::open(dir.path()).unwrap()));
    let sink = Arc::new(CapturingEventSink::new());
    let events_sink = sink.clone();
    let sink: &'static Arc<dyn EventSink> = Box::leak(Box::new(sink as Arc<dyn EventSink>));
    let summarizer: &'static Arc<dyn Summarizer> =
        Box::leak(Box::new(Arc::new(TestSummarizer) as Arc<dyn Summarizer>));
    let mut registry = LoopRegistry::new();
    registry.register(Box::new(SimpleReActLoop)).unwrap();
    registry.register(Box::new(ProblemSolvingLoop)).unwrap();
    let registry: &'static Arc<LoopRegistry> = Box::leak(Box::new(Arc::new(registry)));
    let cancel: &'static CancellationToken = Box::leak(Box::new(CancellationToken::new()));

    let mut ctx = LoopContext {
        config,
        memory,
        llm,
        tools,
        workspace,
        event_sink: sink,
        summarizer,
        cancellation: cancel,
        registry,
        max_refinement_iterations: max_refinement,
        max_delegation_depth: max_depth,
        delegation_depth: 0,
    };

    let result = SimpleReActLoop.run(task.to_string(), &mut ctx).await.unwrap();
    (result, events_sink.events())
}
