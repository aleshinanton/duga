//! Search loop (RAG workflow) — Query → Search → Evaluate → Refine → Answer.
//!
//! For information retrieval, codebase exploration, and document search.
//! Uses `search` and `read` tools via the dispatcher.
//! Refinement capped at `ctx.max_refinement_iterations`.

use crate::loop_context::LoopContext;
use crate::loop_result::LoopResult;
use crate::loop_trait::{Loop, LoopRunFuture};
use crate::loops::simple_react::dispatch_tool_with_events;
use crate::steering::{check_steer, SteerAction};
use duga_events::Event;
use duga_types::error::AgentError;
use duga_types::llm::LlmCallOptions;
use duga_types::message::Message;
use std::time::Instant;

pub struct SearchLoop;

impl Loop for SearchLoop {
    fn id(&self) -> &'static str {
        "search"
    }

    fn name(&self) -> &'static str {
        "Search"
    }

    fn description(&self) -> &'static str {
        "Query → search → evaluate → refine cycle for information retrieval. \
         For finding information in the workspace, codebase exploration, \
         document search, or RAG workflows."
    }

    fn run<'a>(&'a self, task: String, ctx: &'a mut LoopContext<'a>) -> LoopRunFuture<'a> {
        Box::pin(async move {
            run_search(task, ctx).await
        })
    }
}

#[tracing::instrument(skip(ctx), fields(loop_id = "search", depth = ctx.delegation_depth))]
async fn run_search(
    task: String,
    ctx: &mut LoopContext<'_>,
) -> Result<LoopResult, AgentError> {
    let started = Instant::now();
    let max_cycles = ctx.max_refinement_iterations.max(1).min(5);
    let mut total_tool_calls = 0u32;
    let mut findings = String::new();

    tracing::info!(task = %task, max_cycles, "Search loop started");

    // Phase 1: Formulate initial query
    let mut query = formulate_query(&task, ctx).await?;

    for cycle in 1..=max_cycles {
        tracing::info!(cycle, query = %query, "Search cycle");

        // Steering checkpoint (no-op when ctx.steer is None)
        match check_steer(ctx, true).await? {
            SteerAction::Cancel(_) => return Err(AgentError::Cancelled),
            SteerAction::Complete(_) | SteerAction::Reprompt | SteerAction::Continue => {}
        }

        // Phase 2: Execute search + read
        let (new_findings, tc) = execute_search(&query, ctx).await?;
        total_tool_calls += tc;
        findings.push_str(&new_findings);
        findings.push('\n');

        // Phase 3: Evaluate
        let evaluation = evaluate_results(&task, &query, &findings, ctx).await?;

        if evaluation.is_sufficient {
            tracing::info!(cycle, "Results sufficient — synthesizing answer");
            break;
        }

        if cycle < max_cycles && !evaluation.next_query.is_empty() {
            query = evaluation.next_query;
            tracing::info!(cycle, new_query = %query, "Refining search");
        } else {
            tracing::info!(cycle, "Max cycles reached or no refinement");
            break;
        }
    }

    // Phase 5: Synthesize findings into final answer
    let final_answer = synthesize_findings(&task, &findings, ctx).await?;

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
        tool_calls = total_tool_calls,
        "Search loop finished"
    );
    Ok(LoopResult {
        message: msg,
        steps: max_cycles + 2,
        tool_calls: total_tool_calls,
        loop_id: "search".into(),
    })
}

struct EvalResult {
    is_sufficient: bool,
    next_query: String,
}

async fn formulate_query(task: &str, ctx: &LoopContext<'_>) -> Result<String, AgentError> {
    let prompt = format!(
        "You are an information retrieval specialist. Convert this task into a concise \
         search query for finding relevant files or information in a codebase. \
         Output ONLY the query string, nothing else.\n\nTask: {task}\n\nQuery:"
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
    let query = raw.trim().to_string();

    Ok(if query.is_empty() {
        task.to_string()
    } else {
        query
    })
}

async fn execute_search(
    query: &str,
    ctx: &mut LoopContext<'_>,
) -> Result<(String, u32), AgentError> {
    let mut output = String::new();
    let mut tool_calls = 0u32;

    // Use the search tool via memory + LLM
    // We push the query as a user message and let the LLM use search/read tools
    ctx.memory.push_user(format!(
        "Search the workspace for: {query}\n\nUse the `search` tool to find files matching this query, \
         then use `read` to inspect the most relevant results. Report what you find."
    ));

    for _step in 0..20 {
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
            let result = match dispatch_tool_with_events(ctx, call).await {
                Ok(r) => r,
                Err(_) => continue,
            };
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

async fn evaluate_results(
    task: &str,
    query: &str,
    findings: &str,
    ctx: &LoopContext<'_>,
) -> Result<EvalResult, AgentError> {
    let prompt = format!(
        "Evaluate whether the search results are sufficient to answer the task. \
         Reply with a JSON object with these fields:\n\
         - sufficient: boolean\n\
         - next_query: string (only if not sufficient, empty if sufficient)\n\n\
         Original task: {task}\nSearch query used: {query}\nFindings:\n{findings}\n\n\
         Evaluation JSON:"
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

    #[derive(serde::Deserialize)]
    struct RawEval {
        sufficient: bool,
        #[serde(default)]
        next_query: String,
    }

    match serde_json::from_str::<RawEval>(&raw) {
        Ok(eval) => Ok(EvalResult {
            is_sufficient: eval.sufficient,
            next_query: eval.next_query,
        }),
        Err(_) => {
            // Fallback: if we have findings, consider sufficient
            Ok(EvalResult {
                is_sufficient: !findings.trim().is_empty(),
                next_query: String::new(),
            })
        }
    }
}

async fn synthesize_findings(
    task: &str,
    findings: &str,
    ctx: &LoopContext<'_>,
) -> Result<String, AgentError> {
    let prompt = format!(
        "Synthesize the following search findings into a clear answer to the task. \
         Reply with only the answer.\n\nTask: {task}\n\nFindings:\n{findings}\n\nAnswer:"
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

    fn make_infra() -> (
        Arc<ToolDispatcher>,
        Arc<dyn duga_events::EventSink>,
        Arc<dyn crate::Summarizer>,
        Arc<LoopRegistry>,
    ) {
        struct S;
        impl crate::Summarizer for S {
            fn summarize<'a>(&'a self, _m: &'a [Message]) -> crate::SummaryFuture<'a> {
                Box::pin(async { Ok(duga_types::llm::SummaryMessage::new("s".into())) })
            }
        }
        (
            Arc::new(ToolDispatcher::new()),
            Arc::new(NullSink),
            Arc::new(S),
            Arc::new(LoopRegistry::new()),
        )
    }

    #[tokio::test]
    async fn search_loop_with_immediate_result() {
        let llm: Arc<dyn LlmClient> = Arc::new(MockLlm::new(vec![
            Ok(text("auth logic pattern")),
            Ok(text("Found auth in src/auth.rs: uses JWT tokens")),
            Ok(text(r#"{"sufficient": true, "next_query": ""}"#)),
            Ok(text("Auth is implemented in src/auth.rs using JWT.")),
        ]));
        let config = AgentConfig::default();
        let mut memory = crate::Memory::new(vec![Message::system("s")], 10000, 0.8, 0, 0);
        let (tools, sink, summarizer, registry) = make_infra();
        let dir = tempfile::tempdir().unwrap();
        let workspace = duga_sandbox::Workspace::open(dir.path()).unwrap();
        let cancel = duga_sandbox::CancellationToken::new();

        let mut ctx = LoopContext {
            config: &config, memory: &mut memory, llm: &llm, tools: &tools,
            workspace: &workspace, event_sink: &sink, summarizer: &summarizer,
            cancellation: &cancel, registry: &registry,
            max_refinement_iterations: 2, max_delegation_depth: 2, delegation_depth: 0,
            steer: None, steer_limits: None,
        };

        let loop_impl = SearchLoop;
        let result = loop_impl.run("find auth logic".into(), &mut ctx).await.unwrap();
        assert_eq!(result.loop_id, "search");
        assert!(result.message.text.unwrap().contains("src/auth.rs"));
    }

    #[tokio::test]
    async fn search_loop_refines_on_insufficient() {
        let llm: Arc<dyn LlmClient> = Arc::new(MockLlm::new(vec![
            Ok(text("auth")),
            Ok(text("No results found")),
            Ok(text(r#"{"sufficient": false, "next_query": "authentication"}"#)),
            Ok(text("Found auth module in src/")),
            Ok(text(r#"{"sufficient": true, "next_query": ""}"#)),
            Ok(text("Auth module in src/")),
        ]));
        let config = AgentConfig::default();
        let mut memory = crate::Memory::new(vec![Message::system("s")], 10000, 0.8, 0, 0);
        let (tools, sink, summarizer, registry) = make_infra();
        let dir = tempfile::tempdir().unwrap();
        let workspace = duga_sandbox::Workspace::open(dir.path()).unwrap();
        let cancel = duga_sandbox::CancellationToken::new();

        let mut ctx = LoopContext {
            config: &config, memory: &mut memory, llm: &llm, tools: &tools,
            workspace: &workspace, event_sink: &sink, summarizer: &summarizer,
            cancellation: &cancel, registry: &registry,
            max_refinement_iterations: 2, max_delegation_depth: 2, delegation_depth: 0,
            steer: None, steer_limits: None,
        };

        let loop_impl = SearchLoop;
        let result = loop_impl.run("find auth logic".into(), &mut ctx).await.unwrap();
        assert_eq!(result.loop_id, "search");
        assert!(result.message.text.unwrap().contains("src/"));
    }

    #[tokio::test]
    async fn search_loop_stops_at_max_cycles() {
        let llm: Arc<dyn LlmClient> = Arc::new(MockLlm::new(vec![
            Ok(text("auth")),
            Ok(text("nothing found")),
            Ok(text(r#"{"sufficient": true, "next_query": ""}"#)),
            Ok(text("best effort")),
        ]));
        let config = AgentConfig::default();
        let mut memory = crate::Memory::new(vec![Message::system("s")], 10000, 0.8, 0, 0);
        let (tools, sink, summarizer, registry) = make_infra();
        let dir = tempfile::tempdir().unwrap();
        let workspace = duga_sandbox::Workspace::open(dir.path()).unwrap();
        let cancel = duga_sandbox::CancellationToken::new();

        let mut ctx = LoopContext {
            config: &config, memory: &mut memory, llm: &llm, tools: &tools,
            workspace: &workspace, event_sink: &sink, summarizer: &summarizer,
            cancellation: &cancel, registry: &registry,
            max_refinement_iterations: 1, max_delegation_depth: 2, delegation_depth: 0,
            steer: None, steer_limits: None,
        };

        let loop_impl = SearchLoop;
        let result = loop_impl.run("find x".into(), &mut ctx).await.unwrap();
        assert_eq!(result.loop_id, "search");
        assert!(result.message.text.unwrap().contains("best effort"));
    }
}
