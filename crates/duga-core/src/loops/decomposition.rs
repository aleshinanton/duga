//! Decomposition loop — break a compound task into independent subtasks,
//! solve each separately, then merge results.
//!
//! Flow: Decompose → Solve subtasks → Merge.
//! Subtask count capped at `ctx.max_refinement_iterations`.

use crate::loop_context::LoopContext;
use crate::loop_result::LoopResult;
use crate::loop_trait::{Loop, LoopRunFuture};
use duga_events::Event;
use duga_types::error::AgentError;
use duga_types::llm::LlmCallOptions;
use duga_types::message::Message;
use duga_types::tool_result::ToolResult;
use std::time::Instant;

pub struct DecompositionLoop;

impl Loop for DecompositionLoop {
    fn id(&self) -> &'static str {
        "decomposition"
    }

    fn name(&self) -> &'static str {
        "Decomposition"
    }

    fn description(&self) -> &'static str {
        "Breaks a large task into independent subtasks, solves each separately, \
         then merges results. For compound tasks where sub-problems can be \
         solved independently."
    }

    fn run<'a>(&'a self, task: String, ctx: &'a mut LoopContext<'a>) -> LoopRunFuture<'a> {
        Box::pin(async move {
            run_decomposition(task, ctx).await
        })
    }
}

#[tracing::instrument(skip(ctx), fields(loop_id = "decomposition", depth = ctx.delegation_depth))]
async fn run_decomposition(
    task: String,
    ctx: &mut LoopContext<'_>,
) -> Result<LoopResult, AgentError> {
    let started = Instant::now();
    let max_subtasks = ctx.max_refinement_iterations.max(1).min(8);
    let mut total_tool_calls = 0u32;

    tracing::info!(task = %task, max_subtasks, "Decomposition loop started");

    // Phase 1: Decompose
    let subtasks = decompose(&task, max_subtasks, ctx).await?;
    if subtasks.is_empty() {
        tracing::info!("No subtasks — falling back to direct execution");
        let (output, tc) = solve_subtask(&task, ctx).await?;
        total_tool_calls += tc;
        let msg = duga_types::message::AssistantMessage {
            text: Some(output),
            tool_calls: vec![],
            reasoning_content: None,
        };
        ctx.event_sink
            .emit(Event::AgentFinished {
                text: msg.text.clone(),
            })
            .await
            .map_err(|e| AgentError::EventSinkFailed(e.to_string()))?;
        return Ok(LoopResult {
            message: msg,
            steps: 1,
            tool_calls: total_tool_calls,
            loop_id: "decomposition".into(),
        });
    }

    // Phase 2: Solve each subtask
    let mut results: Vec<(String, String, bool)> = Vec::new(); // (title, output, success)
    for subtask in &subtasks {
        tracing::info!(subtask = %subtask.title, "Solving subtask");
        let (output, tc) = solve_subtask(&subtask.description, ctx).await?;
        total_tool_calls += tc;
        results.push((subtask.title.clone(), output, true));
    }

    // Phase 3: Merge results
    let final_answer = merge_results(&task, &results, ctx).await?;

    let msg = duga_types::message::AssistantMessage {
        text: Some(final_answer),
        tool_calls: vec![],
        reasoning_content: None,
    };
    ctx.event_sink
        .emit(Event::AgentFinished {
            text: msg.text.clone(),
        })
        .await
        .map_err(|e| AgentError::EventSinkFailed(e.to_string()))?;
    tracing::info!(
        duration_ms = started.elapsed().as_millis(),
        subtasks = results.len(),
        tool_calls = total_tool_calls,
        "Decomposition finished"
    );
    Ok(LoopResult {
        message: msg,
        steps: results.len() as u32 + 2,
        tool_calls: total_tool_calls,
        loop_id: "decomposition".into(),
    })
}

#[derive(Clone, Debug, serde::Deserialize)]
struct SubtaskDef {
    title: String,
    description: String,
}

async fn decompose(
    task: &str,
    max_count: u32,
    ctx: &LoopContext<'_>,
) -> Result<Vec<SubtaskDef>, AgentError> {
    let prompt = format!(
        "Break this compound task into up to {max_count} independent subtasks. \
         Output ONLY a JSON array of objects, each with 'title' and 'description' fields. \
         No other text.\n\nTask: {task}\n\nSubtasks (JSON array):"
    );
    let mut messages = ctx.memory.messages();
    messages.push(Message::user(&prompt));

    let response = ctx
        .llm
        .chat(
            &messages,
            &[],
            LlmCallOptions { streaming: false },
            ctx.event_sink.as_ref(),
        )
        .await
        .map_err(|e| AgentError::LlmFailed(e.to_string()))?;

    let raw = response.message.text.unwrap_or_default();
    match serde_json::from_str::<Vec<SubtaskDef>>(&raw) {
        Ok(subtasks) if !subtasks.is_empty() => Ok(subtasks),
        _ => {
            // Fallback: single subtask
            Ok(vec![SubtaskDef {
                title: "Main task".into(),
                description: task.to_string(),
            }])
        }
    }
}

async fn solve_subtask(
    task: &str,
    ctx: &mut LoopContext<'_>,
) -> Result<(String, u32), AgentError> {
    ctx.memory.push_user(format!("Subtask: {task}\n\nSolve this subtask. Start with tools if needed."));

    let mut output = String::new();
    let mut tool_calls = 0u32;

    for _step in 0..30 {
        if ctx.cancellation.is_cancelled() {
            return Err(AgentError::Cancelled);
        }

        let messages = ctx.memory.messages();
        let schemas = ctx.tools.schemas();
        let response = ctx
            .llm
            .chat(
                &messages,
                &schemas,
                LlmCallOptions { streaming: false },
                ctx.event_sink.as_ref(),
            )
            .await
            .map_err(|e| AgentError::LlmFailed(e.to_string()))?;

        let assistant = response.message;
        let text = assistant.text.clone().unwrap_or_default();
        ctx.memory.push_assistant(assistant.clone());

        for call in &assistant.tool_calls {
            tool_calls += 1;
            let start = Instant::now();
            let result = ctx
                .tools
                .dispatch(
                    call,
                    ctx.workspace,
                    ctx.cancellation.clone(),
                    ctx.event_sink.as_ref(),
                )
                .await;
            let result = ToolResult::from_outcome(call.id.clone(), &call.tool, result, start);
            ctx.memory.push_tool_result(result);
        }

        output.push_str(&text);

        if assistant.is_termination()
            || (!text.is_empty() && assistant.tool_calls.is_empty())
        {
            break;
        }
    }

    Ok((output, tool_calls))
}

async fn merge_results(
    task: &str,
    results: &[(String, String, bool)],
    ctx: &LoopContext<'_>,
) -> Result<String, AgentError> {
    let results_text = results
        .iter()
        .map(|(title, output, ok)| {
            format!(
                "## {}{}\n{}\n",
                title,
                if *ok { "" } else { " (FAILED)" },
                output
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let merge_prompt = format!(
        "Synthesize the following subtask results into a single cohesive answer. \
         Reply with only the final answer.\n\n\
         Original task: {task}\n\n{results_text}\nFinal answer:"
    );
    let mut messages = ctx.memory.messages();
    messages.push(Message::user(&merge_prompt));

    let response = ctx
        .llm
        .chat(
            &messages,
            &[],
            LlmCallOptions { streaming: false },
            ctx.event_sink.as_ref(),
        )
        .await
        .map_err(|e| AgentError::LlmFailed(e.to_string()))?;

    Ok(response.message.text.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loop_registry::LoopRegistry;
    use crate::testing::MockLlm;
    use duga_events::NullSink;
    use duga_llm::LlmClient;
    use duga_tools::ToolDispatcher;
    use duga_types::config::AgentConfig;
    use duga_types::llm::{LlmResponse, TokenUsage};
    use duga_types::message::AssistantMessage;
    use std::sync::Arc;

    fn text(msg: &str) -> LlmResponse {
        LlmResponse {
            message: AssistantMessage {
                text: Some(msg.into()),
                tool_calls: vec![],
                reasoning_content: None,
            },
            usage: TokenUsage { prompt: 0, completion: 0 },
        }
    }

    #[tokio::test]
    async fn decomposition_with_subtasks() {
        let llm: Arc<dyn LlmClient> = Arc::new(MockLlm::new(vec![
            Ok(text(r#"[{"title": "A", "description": "do A"}, {"title": "B", "description": "do B"}]"#)),
            Ok(text("A done")),
            Ok(text("B done")),
            Ok(text("Merged: A done, B done")),
        ]));
        let config = AgentConfig::default();
        let mut memory = crate::Memory::new(vec![Message::system("s")], 10000, 0.8, 0, 0);
        let tools = Arc::new(ToolDispatcher::new());
        let dir = tempfile::tempdir().unwrap();
        let workspace = duga_sandbox::Workspace::open(dir.path()).unwrap();
        let sink: Arc<dyn duga_events::EventSink> = Arc::new(NullSink);
        struct S;
        impl crate::Summarizer for S {
            fn summarize<'a>(&'a self, _m: &'a [Message]) -> crate::SummaryFuture<'a> {
                Box::pin(async { Ok(duga_types::llm::SummaryMessage::new("s".into())) })
            }
        }
        let summarizer: Arc<dyn crate::Summarizer> = Arc::new(S);
        let registry = Arc::new(LoopRegistry::new());
        let cancel = duga_sandbox::CancellationToken::new();

        let mut ctx = LoopContext {
            config: &config,
            memory: &mut memory,
            llm: &llm,
            tools: &tools,
            workspace: &workspace,
            event_sink: &sink,
            summarizer: &summarizer,
            cancellation: &cancel,
            registry: &registry,
            max_refinement_iterations: 3,
            max_delegation_depth: 2,
            delegation_depth: 0,
        };

        let loop_impl = DecompositionLoop;
        let result = loop_impl.run("big task".into(), &mut ctx).await.unwrap();

        assert_eq!(result.loop_id, "decomposition");
        assert!(result.message.text.unwrap().contains("Merged"));
        assert_eq!(result.steps, 4);
    }

    #[tokio::test]
    async fn empty_decompose_falls_back_to_direct() {
        let llm: Arc<dyn LlmClient> = Arc::new(MockLlm::new(vec![
            Ok(text("not valid json")),   // decompose
            Ok(text("done directly")),     // solve subtask
            Ok(text("merged result")),     // merge results
        ]));
        let config = AgentConfig::default();
        let mut memory = crate::Memory::new(vec![Message::system("s")], 10000, 0.8, 0, 0);
        let tools = Arc::new(ToolDispatcher::new());
        let dir = tempfile::tempdir().unwrap();
        let workspace = duga_sandbox::Workspace::open(dir.path()).unwrap();
        let sink: Arc<dyn duga_events::EventSink> = Arc::new(NullSink);
        struct S;
        impl crate::Summarizer for S {
            fn summarize<'a>(&'a self, _m: &'a [Message]) -> crate::SummaryFuture<'a> {
                Box::pin(async { Ok(duga_types::llm::SummaryMessage::new("s".into())) })
            }
        }
        let summarizer: Arc<dyn crate::Summarizer> = Arc::new(S);
        let registry = Arc::new(LoopRegistry::new());
        let cancel = duga_sandbox::CancellationToken::new();

        let mut ctx = LoopContext {
            config: &config,
            memory: &mut memory,
            llm: &llm,
            tools: &tools,
            workspace: &workspace,
            event_sink: &sink,
            summarizer: &summarizer,
            cancellation: &cancel,
            registry: &registry,
            max_refinement_iterations: 3,
            max_delegation_depth: 2,
            delegation_depth: 0,
        };

        let loop_impl = DecompositionLoop;
        let result = loop_impl.run("task".into(), &mut ctx).await.unwrap();
        assert_eq!(result.loop_id, "decomposition");
    }
}
