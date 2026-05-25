//! Verification loop — generate N independent answers, then vote.
//!
//! For factual questions and correctness-critical work. Flow:
//! 1. Run N independent answer generations (mini ReAct cycles)
//! 2. Voting: present all answers to LLM, select best or synthesize consensus
//! 3. Return selected/consensus answer
//!
//! Generation count = `ctx.max_refinement_iterations`.

use crate::loop_context::LoopContext;
use crate::loop_result::LoopResult;
use crate::loop_trait::{Loop, LoopRunFuture};
use crate::loops::simple_react::dispatch_tool_with_events;
use crate::steering::{check_steer, SteerAction};
use duga_events::Event;
use duga_types::error::AgentError;
use duga_types::llm::LlmCallOptions;
use std::time::Instant;

pub struct VerificationLoop;

impl Loop for VerificationLoop {
    fn id(&self) -> &'static str {
        "verification"
    }

    fn name(&self) -> &'static str {
        "Verification"
    }

    fn description(&self) -> &'static str {
        "Generate multiple answers then vote. For questions where correctness \
         matters and competing answers are possible: math, facts, logic puzzles, \
         code correctness verification, and ambiguity resolution."
    }

    fn run<'a>(&'a self, task: String, ctx: &'a mut LoopContext<'a>) -> LoopRunFuture<'a> {
        Box::pin(async move {
            run_verification(task, ctx).await
        })
    }
}

#[tracing::instrument(skip(ctx), fields(loop_id = "verification", depth = ctx.delegation_depth))]
async fn run_verification(
    task: String,
    ctx: &mut LoopContext<'_>,
) -> Result<LoopResult, AgentError> {
    let started = Instant::now();
    let answer_count = ctx.max_refinement_iterations.max(1).min(5);
    let mut answers: Vec<String> = Vec::new();
    let mut total_tool_calls = 0u32;

    tracing::info!(task = %task, answer_count, "Verification loop started");

    // Phase 1: Generate N independent answers
    for i in 0..answer_count {
        // Steering checkpoint (no-op when ctx.steer is None)
        match check_steer(ctx, true).await? {
            SteerAction::Cancel(_) => return Err(AgentError::Cancelled),
            SteerAction::Complete(_) | SteerAction::Reprompt | SteerAction::Continue => {}
        }

        tracing::info!(attempt = i + 1, total = answer_count, "Generating answer");
        let (answer, tool_calls) = generate_answer(&task, i + 1, answer_count, ctx).await?;
        total_tool_calls += tool_calls;
        answers.push(answer);
    }

    // Phase 2: Vote / synthesize consensus
    let final_answer = if answer_count == 1 {
        tracing::info!("Single answer — no voting needed");
        answers.into_iter().next().unwrap_or_default()
    } else {
        vote(&answers, &task, ctx).await?
    };

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
        "Verification finished"
    );
    Ok(LoopResult {
        message: msg,
        steps: answer_count + 1,
        tool_calls: total_tool_calls,
        loop_id: "verification".into(),
    })
}

async fn generate_answer(
    task: &str,
    attempt: u32,
    total: u32,
    ctx: &mut LoopContext<'_>,
) -> Result<(String, u32), AgentError> {
    if ctx.cancellation.is_cancelled() {
        return Err(AgentError::Cancelled);
    }

    let prompt = format!(
        "You are attempting to answer a question. This is attempt {attempt} of {total}. \
         Focus on accuracy and correctness.\n\nQuestion: {task}\n\nProvide your answer:"
    );
    ctx.memory.push_user(prompt);

    let mut output = String::new();
    let mut tool_calls = 0u32;

    for _step in 0..30 {
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

        // Execute tool calls
        for call in &assistant.tool_calls {
            tool_calls += 1;
            let result = match dispatch_tool_with_events(ctx, call).await {
                Ok(r) => r,
                Err(_) => continue,
            };
            ctx.memory.push_tool_result(result);
        }

        output.push_str(&text);

        if assistant.is_termination() || !text.is_empty() && assistant.tool_calls.is_empty() {
            break;
        }
    }

    Ok((output, tool_calls))
}

async fn vote(
    answers: &[String],
    task: &str,
    ctx: &LoopContext<'_>,
) -> Result<String, AgentError> {
    let answers_text = answers
        .iter()
        .enumerate()
        .map(|(i, a)| format!("Answer {}:\n{}\n", i + 1, a))
        .collect::<Vec<_>>()
        .join("\n");

    let vote_prompt = format!(
        "You are a judge. Given a question and multiple independent answers, \
         select the best answer or synthesize a consensus. \
         Reply with only the final answer — no explanation.\n\n\
         Question: {task}\n\n{answers_text}\nFinal answer:"
    );

    let messages = crate::loops::problem_solving::minimal_context(ctx, &vote_prompt);

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
    use duga_types::message::{AssistantMessage, Message};
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
    async fn single_answer_no_voting() {
        let llm: Arc<dyn LlmClient> = Arc::new(MockLlm::new(vec![Ok(text("The answer is 42."))]));
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
            config: &config, memory: &mut memory, llm: &llm, tools: &tools,
            workspace: &workspace, event_sink: &sink, summarizer: &summarizer,
            cancellation: &cancel, registry: &registry,
            max_refinement_iterations: 1, max_delegation_depth: 2, delegation_depth: 0,
            steer: None, steer_limits: None,
        };

        let loop_impl = VerificationLoop;
        let result = loop_impl.run("What is the answer?".into(), &mut ctx).await.unwrap();
        assert_eq!(result.loop_id, "verification");
        assert!(result.message.text.unwrap().contains("42"));
    }

    #[tokio::test]
    async fn multi_answer_voting() {
        let llm: Arc<dyn LlmClient> = Arc::new(MockLlm::new(vec![
            Ok(text("Answer A: 42")),
            Ok(text("Answer B: 42")),
            Ok(text("Answer C: 42")),
            Ok(text("Consensus: 42")),
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
            config: &config, memory: &mut memory, llm: &llm, tools: &tools,
            workspace: &workspace, event_sink: &sink, summarizer: &summarizer,
            cancellation: &cancel, registry: &registry,
            max_refinement_iterations: 3, max_delegation_depth: 2, delegation_depth: 0,
            steer: None, steer_limits: None,
        };

        let loop_impl = VerificationLoop;
        let result = loop_impl.run("What is the answer?".into(), &mut ctx).await.unwrap();
        assert_eq!(result.loop_id, "verification");
        assert!(result.message.text.unwrap().contains("42"));
    }

    #[tokio::test]
    async fn max_refinement_zero_still_generates_one() {
        let llm: Arc<dyn LlmClient> = Arc::new(MockLlm::new(vec![Ok(text("Only answer"))]));
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
            config: &config, memory: &mut memory, llm: &llm, tools: &tools,
            workspace: &workspace, event_sink: &sink, summarizer: &summarizer,
            cancellation: &cancel, registry: &registry,
            max_refinement_iterations: 0, max_delegation_depth: 2, delegation_depth: 0,
            steer: None, steer_limits: None,
        };

        let loop_impl = VerificationLoop;
        let result = loop_impl.run("question".into(), &mut ctx).await.unwrap();
        assert_eq!(result.loop_id, "verification");
    }
}
