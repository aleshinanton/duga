use duga_core::testing::{CapturingEventSink, MockLlm, MockTool};
use duga_core::{AgentLoop, Summarizer, SummaryFuture};
use duga_events::{Event, JsonlSink};
use duga_replay::JsonlReader;
use duga_sandbox::{CancellationToken, Workspace};
use duga_tools::{ErasedTool, ToolDispatcher};
use duga_types::config::AgentConfig;
use duga_types::llm::{LlmResponse, SummaryMessage, TokenUsage};
use duga_types::message::{AssistantMessage, Message};
use duga_types::tool_call::ToolCall;
use std::sync::Arc;

struct TestSummarizer;

impl Summarizer for TestSummarizer {
    fn summarize<'a>(&'a self, _messages: &'a [Message]) -> SummaryFuture<'a> {
        Box::pin(async { Ok(SummaryMessage::new("summary".into())) })
    }
}

fn response(message: AssistantMessage) -> LlmResponse {
    LlmResponse {
        message,
        usage: TokenUsage {
            prompt: 1,
            completion: 1,
        },
    }
}

async fn run_test_agent(
    config: AgentConfig,
    mock_llm: MockLlm,
    mock_tools: Vec<MockTool>,
    task: &str,
) -> (String, CapturingEventSink) {
    let dir = tempfile::tempdir().unwrap();
    let workspace = Workspace::open(dir.path()).unwrap();
    let dispatcher = Arc::new(ToolDispatcher::new());
    for tool in mock_tools {
        dispatcher.register_erased(ErasedTool::erase(tool)).unwrap();
    }
    let sink = CapturingEventSink::new();
    let mut agent = AgentLoop::new(
        config,
        duga_core::Memory::new(vec![Message::system("system")], 100_000, 0.8),
        Arc::new(TestSummarizer),
        Arc::new(mock_llm),
        dispatcher,
        workspace,
        Arc::new(sink.clone()),
    );

    let result = agent.run(task, CancellationToken::new()).await.unwrap();
    (result.message.text.unwrap_or_default(), sink)
}

fn fibonacci_script() -> (MockLlm, Vec<MockTool>) {
    let write_fib = ToolCall::new(
        "write",
        serde_json::json!({
            "path": "src/fib.rs",
            "content": "pub fn fibonacci(n: u64) -> u64 { match n { 0 => 0, 1 => 1, _ => fibonacci(n - 1) + fibonacci(n - 2) } }"
        }),
    );
    let read_cargo = ToolCall::new("read", serde_json::json!({"path": "Cargo.toml"}));
    let write_cargo = ToolCall::new(
        "write",
        serde_json::json!({
            "path": "Cargo.toml",
            "content": "[package]\nname = \"fib\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"
        }),
    );
    let test = ToolCall::new(
        "bash",
        serde_json::json!({"command": ["cargo", "test"], "session": null}),
    );

    let llm = MockLlm::new(vec![
        Ok(response(AssistantMessage {
            text: Some("writing fibonacci".into()),
            tool_calls: vec![
                write_fib.clone(),
                read_cargo.clone(),
                write_cargo.clone(),
                test.clone(),
            ],
        })),
        Ok(response(AssistantMessage {
            text: Some("test result: ok".into()),
            tool_calls: vec![],
        })),
    ]);

    let write = MockTool::new("write", "write")
        .with_response(
            write_fib.raw_args,
            Ok(MockTool::success("wrote src/fib.rs")),
        )
        .with_response(
            write_cargo.raw_args,
            Ok(MockTool::success("wrote Cargo.toml")),
        );
    let read = MockTool::new("read", "read").with_response(
        read_cargo.raw_args,
        Ok(MockTool::success("[package]\nname = \"fib\"\n")),
    );
    let bash = MockTool::new("bash", "bash").with_response(
        test.raw_args,
        Ok(MockTool::success("test result: ok. 1 passed")),
    );

    (llm, vec![write, read, bash])
}

#[tokio::test]
async fn fibonacci_smoke_test_completes_with_expected_events() {
    let (llm, tools) = fibonacci_script();
    let (answer, sink) = run_test_agent(
        AgentConfig::default(),
        llm,
        tools,
        "Write a Rust function fibonacci(n: u64) -> u64 with unit tests. Ensure cargo test passes.",
    )
    .await;

    assert!(answer.contains("test result: ok"));
    let events = sink.events();
    assert!(matches!(events.first(), Some(Event::AgentStarted { .. })));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, Event::ToolCallStarted { .. }))
            .count(),
        4
    );
    assert!(matches!(events.last(), Some(Event::AgentFinished { .. })));
}

#[tokio::test]
async fn replay_roundtrip_reads_written_events() {
    let (llm, tools) = fibonacci_script();
    let (answer, sink) = run_test_agent(AgentConfig::default(), llm, tools, "fibonacci").await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    let jsonl = JsonlSink::new(&path).unwrap();

    for event in sink.events() {
        duga_events::EventSink::emit(&jsonl, event).await.unwrap();
    }

    let events = JsonlReader::read(&path).unwrap();

    assert!(answer.contains("test result: ok"));
    assert_eq!(
        duga_replay::final_text(&events),
        Some("test result: ok".into())
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.event, Event::ToolCallFinished { .. }))
            .count(),
        4
    );
}
