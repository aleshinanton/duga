//! Integration tests for all five built-in tools (TASK-4.10).

use duga_sandbox::binary_registry::BinaryRegistry;
use duga_sandbox::exec::CancellationToken;
use duga_sandbox::{SandboxExecutor, Workspace};
use duga_tools::dispatcher::ToolDispatcher;
use duga_tools::erased::ErasedTool;
use duga_tools::event_sink::NullSink;
use duga_types::config::OutputLimits;
use duga_types::tool_call::ToolCall;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

use crate::bash::BashTool;
use crate::read::ReadTool;
use crate::search::SearchTool;
use crate::think::{ThinkLimits, ThinkTool};
use crate::write::WriteTool;

fn setup() -> (TempDir, Arc<Workspace>, ToolDispatcher) {
    let d = TempDir::new().unwrap();
    let ws = Arc::new(Workspace::open(d.path()).unwrap());
    let r = Arc::new(
        BinaryRegistry::new(&["echo".into(), "cat".into(), "bash".into(), "sh".into()]).unwrap(),
    );
    let l = OutputLimits::default();
    let dp = ToolDispatcher::new();
    dp.register_erased(ErasedTool::erase(ReadTool::new()))
        .unwrap();
    dp.register_erased(ErasedTool::erase(WriteTool::new()))
        .unwrap();
    dp.register_erased(ErasedTool::erase(BashTool::with_sandbox(
        r.clone(),
        ws.clone(),
        l.clone(),
        Duration::from_secs(30),
        Arc::new(SandboxExecutor::capability()),
    )))
    .unwrap();
    dp.register_erased(ErasedTool::erase(SearchTool::new()))
        .unwrap();
    dp.register_erased(ErasedTool::erase(ThinkTool::new(ThinkLimits {
        max_calls: 100,
        max_tokens: 10000,
    })))
    .unwrap();
    (d, ws, dp)
}

#[tokio::test]
async fn test_write_read_roundtrip() {
    let (_d, ws, dp) = setup();
    let c = CancellationToken::new();
    let s = NullSink;

    dp.dispatch(
        &ToolCall::new("write", json!({"path":"t.txt","content":"hello\nworld"})),
        &ws,
        c.clone(),
        &s,
    )
    .await
    .unwrap();
    let r = dp
        .dispatch(
            &ToolCall::new("read", json!({"path":"t.txt"})),
            &ws,
            c.clone(),
            &s,
        )
        .await
        .unwrap();
    assert_eq!(r.output, "hello\nworld");
}

#[tokio::test]
async fn test_dispatch_preserves_tool_call_id() {
    let (_d, ws, dp) = setup();
    let c = CancellationToken::new();
    let s = NullSink;
    let call = ToolCall::new("write", json!({"path":"id.txt","content":"hello"}));
    let expected = call.id.clone();

    let r = dp.dispatch(&call, &ws, c, &s).await.unwrap();

    assert_eq!(r.tool_call_id, expected);
}

#[tokio::test]
async fn test_search_finds() {
    let (_d, ws, dp) = setup();
    let c = CancellationToken::new();
    let s = NullSink;
    dp.dispatch(
        &ToolCall::new("write", json!({"path":"d.txt","content":"foo\nbar"})),
        &ws,
        c.clone(),
        &s,
    )
    .await
    .unwrap();
    let r = dp
        .dispatch(
            &ToolCall::new("search", json!({"query":"foo"})),
            &ws,
            c.clone(),
            &s,
        )
        .await
        .unwrap();
    assert!(r.output.contains("foo"));
}

#[tokio::test]
async fn test_think_works() {
    let (_d, ws, dp) = setup();
    let c = CancellationToken::new();
    let s = NullSink;
    let r = dp
        .dispatch(
            &ToolCall::new("think", json!({"thought":"hello"})),
            &ws,
            c.clone(),
            &s,
        )
        .await
        .unwrap();
    assert_eq!(r.output, "hello");
}

#[tokio::test]
async fn test_bash_cat() {
    let (_d, ws, dp) = setup();
    let c = CancellationToken::new();
    let s = NullSink;
    dp.dispatch(
        &ToolCall::new("write", json!({"path":"x.txt","content":"data"})),
        &ws,
        c.clone(),
        &s,
    )
    .await
    .unwrap();
    let r = dp
        .dispatch(
            &ToolCall::new("bash", json!({"command":["cat","x.txt"]})),
            &ws,
            c.clone(),
            &s,
        )
        .await
        .unwrap();
    assert!(r.output.contains("data"));
}

#[tokio::test]
async fn test_errors() {
    let (_d, ws, dp) = setup();
    let c = CancellationToken::new();
    let s = NullSink;
    assert!(dp
        .dispatch(
            &ToolCall::new("write", json!({"path":"../x","content":"x"})),
            &ws,
            c.clone(),
            &s
        )
        .await
        .is_err());
    assert!(dp
        .dispatch(
            &ToolCall::new("read", json!({"path":"no"})),
            &ws,
            c.clone(),
            &s
        )
        .await
        .is_err());
    assert!(dp
        .dispatch(
            &ToolCall::new("search", json!({"query":"["})),
            &ws,
            c.clone(),
            &s
        )
        .await
        .is_err());
}
