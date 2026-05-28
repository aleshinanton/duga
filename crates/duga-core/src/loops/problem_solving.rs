//! Problem Solving loop — Plan → Execute → Audit pattern.
//!
//! For code generation, multi-step reasoning, and tasks requiring
//! structured thinking. Uses a three-phase cycle:
//!
//! 1. **Planning**: LLM creates a step-by-step plan
//! 2. **Execution**: Each step is executed with tool calls
//! 3. **Audit**: LLM reviews output vs plan, re-enters planning if needed
//!
//! Refinement caps at `ctx.max_refinement_iterations`.

use crate::loop_context::LoopContext;
use crate::loop_result::LoopResult;
use crate::loop_trait::{Loop, LoopRunFuture};
use crate::loops::simple_react::{dispatch_tool_with_events, schemas_without_delegate};
use crate::steering::{check_steer, SteerAction};
use duga_events::Event;
use duga_types::error::AgentError;
use duga_types::llm::LlmCallOptions;
use duga_types::message::Message;
use std::time::Instant;

pub struct ProblemSolvingLoop;

impl Loop for ProblemSolvingLoop {
    fn id(&self) -> &'static str {
        "problem_solving"
    }

    fn name(&self) -> &'static str {
        "Problem Solving"
    }

    fn description(&self) -> &'static str {
        "Plan→execute→audit for code generation and multi-file refactors. \
         NOT for: calculations, facts, lookups, single-file edits, \
         or tasks answerable in one step. Only for genuinely multi-stage work."
    }

    fn run<'a>(&'a self, task: String, ctx: &'a mut LoopContext<'a>) -> LoopRunFuture<'a> {
        Box::pin(async move {
            run_problem_solving(task, ctx).await
        })
    }
}

#[tracing::instrument(skip(ctx), fields(loop_id = "problem_solving", depth = ctx.delegation_depth))]
async fn run_problem_solving(
    task: String,
    ctx: &mut LoopContext<'_>,
) -> Result<LoopResult, AgentError> {
    let started = Instant::now();
    let mut total_tool_calls = 0u32;
    let max_iterations = ctx.max_refinement_iterations.max(1);

    tracing::info!(task = %task, "ProblemSolving loop started");

    let mut current_task = task.clone();
    let mut accumulated_output = String::new();

    for iteration in 1..=max_iterations {
        // Steering checkpoint (no-op when ctx.steer is None)
        match check_steer(ctx, true).await? {
            SteerAction::Cancel(_) => return Err(AgentError::Cancelled),
            SteerAction::Complete(_) | SteerAction::Reprompt | SteerAction::Continue => {}
        }

        tracing::info!(iteration, "Plan phase");
        let plan = plan_phase(&current_task, ctx).await?;
        tracing::info!(plan_steps = plan.steps.len(), "Plan created");

        tracing::info!(iteration, "Execute phase");
        let (output, tool_calls) = execute_plan(&plan, ctx).await?;
        total_tool_calls += tool_calls;
        accumulated_output.push_str(&output);
        accumulated_output.push('\n');

        tracing::info!(iteration, "Audit phase");
        let audit = audit_phase(&plan, &output, &current_task, ctx).await?;

        if audit.is_complete {
            // Prefer the audit's clean final_answer over the noisy
            // accumulated_output (which contains LLM meta-commentary
            // and tool execution logs mixed with substantive output).
            let answer = audit
                .final_answer
                .unwrap_or_else(|| accumulated_output.trim().to_string());
            let msg = duga_types::message::AssistantMessage {
                text: Some(answer),
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
                "ProblemSolving finished"
            );
            return Ok(LoopResult {
                message: msg,
                steps: iteration,
                tool_calls: total_tool_calls,
                loop_id: "problem_solving".into(),
            });
        }

        // Refine: use the audit's clean final_answer (or accumulated output
        // as fallback) for the next iteration's context — not the raw
        // execute_plan output which contains LLM meta-commentary.
        let previous = audit
            .final_answer
            .as_deref()
            .unwrap_or(&accumulated_output);
        current_task = format!(
            "{task}\n\nPrevious attempt result:\n{previous}\n\nGaps found:\n{gaps}",
            gaps = audit.gaps.unwrap_or_default()
        );
    }

    let msg = duga_types::message::AssistantMessage {
        text: Some(format!(
            "Reached {max_iterations} refinement iterations. Best result:\n\n{accumulated_output}"
        )),
        tool_calls: vec![],
        reasoning_content: None,
    };
    ctx.event_sink
        .emit(Event::AgentFinished {
            text: msg.text.clone(),
        })
        .await
        .map_err(|e| AgentError::EventSinkFailed(e.to_string()))?;
    Ok(LoopResult {
        message: msg,
        steps: max_iterations,
        tool_calls: total_tool_calls,
        loop_id: "problem_solving".into(),
    })
}

/// Build a minimal message list for LLM calls that should not be influenced
/// by previous conversation context. Includes only system messages + task.
pub fn minimal_context(ctx: &LoopContext<'_>, task_prompt: &str) -> Vec<Message> {
    let mut msgs: Vec<Message> = ctx
        .memory
        .messages()
        .into_iter()
        .filter(|m| matches!(m.role, duga_types::message::Role::System))
        .collect();
    msgs.push(Message::user(task_prompt));
    msgs
}

struct Plan {
    steps: Vec<String>,
}

struct AuditResult {
    is_complete: bool,
    final_answer: Option<String>,
    gaps: Option<String>,
}

async fn plan_phase(task: &str, ctx: &LoopContext<'_>) -> Result<Plan, AgentError> {
    let plan_prompt = format!(
        "Create a step-by-step plan to accomplish the following task. \
         Focus ONLY on this task — ignore any previous conversation. \
         Output ONLY a JSON array of strings, each being one step. \
         No other text.\n\nTask: {task}\n\nPlan (JSON array):"
    );
    // Use minimal context: only system messages + task, not full history.
    let messages = minimal_context(ctx, &plan_prompt);

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
    let steps: Vec<String> = match serde_json::from_str(&raw) {
        Ok(arr) => arr,
        Err(_) => {
            // Fallback: split by numbered lines
            raw.lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| l.trim().to_string())
                .collect()
        }
    };

    if steps.is_empty() {
        return Ok(Plan {
            steps: vec!["Execute the task directly".into()],
        });
    }

    Ok(Plan { steps })
}

async fn execute_plan(
    plan: &Plan,
    ctx: &mut LoopContext<'_>,
) -> Result<(String, u32), AgentError> {
    let mut all_output = String::new();
    let mut tool_calls = 0u32;
    // Push the plan as context so the LLM sees it
    let plan_text = plan
        .steps
        .iter()
        .enumerate()
        .map(|(i, s)| format!("{}. {}", i + 1, s))
        .collect::<Vec<_>>()
        .join("\n");
    ctx.memory.push_user(format!(
        "Follow this plan step by step:\n\n{plan_text}\n\nExecute each step using tools as needed. \
         After completing all steps, state 'ALL STEPS COMPLETE'."
    ));

    for step_idx in 1..=50u32 {
        // Check limits
        if ctx.cancellation.is_cancelled() {
            return Err(AgentError::Cancelled);
        }

        let messages = ctx.memory.messages();
        let schemas = schemas_without_delegate(&ctx.tools.schemas());

        let response = ctx
            .llm
            .chat(
                &messages,
                &schemas,
                LlmCallOptions {
                    streaming: false,
                },
                ctx.event_sink.as_ref(),
            )
            .await
            .map_err(|e| AgentError::LlmFailed(e.to_string()))?;

        let assistant = response.message;
        let text = assistant.text.clone().unwrap_or_default();
        ctx.memory.push_assistant(assistant.clone());

        if text.contains("ALL STEPS COMPLETE") || assistant.is_termination() {
            all_output.push_str(&text);
            break;
        }

        // Execute tool calls
        for call in &assistant.tool_calls {
            tool_calls += 1;
            let result = match dispatch_tool_with_events(ctx, call).await {
                Ok(r) => r,
                Err(_) => continue,
            };
            ctx.memory.push_tool_result(result);
        }

        all_output.push_str(&text);
        all_output.push('\n');

        if step_idx >= 50 {
            all_output.push_str("\n[Max execution steps reached]");
            break;
        }
    }

    Ok((all_output, tool_calls))
}

async fn audit_phase(
    plan: &Plan,
    output: &str,
    task: &str,
    ctx: &LoopContext<'_>,
) -> Result<AuditResult, AgentError> {
    let plan_text = plan
        .steps
        .iter()
        .enumerate()
        .map(|(i, s)| format!("{}. {}", i + 1, s))
        .collect::<Vec<_>>()
        .join("\n");

    let audit_prompt = format!(
        "Review the execution output against the original plan and task. \
         Focus ONLY on whether THIS task is complete — ignore previous conversation. \
         Reply with a JSON object with these fields:\n\
         - complete: boolean (true if task is fully done)\n\
         - final_answer: string (the final result if complete, null if not)\n\
         - gaps: string (what's missing if not complete, null if complete)\n\n\
         Original task: {task}\n\nPlan:\n{plan_text}\n\nExecution output:\n{output}\n\n\
         Audit JSON:"
    );

    let messages = minimal_context(ctx, &audit_prompt);

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
    struct RawAudit {
        complete: bool,
        #[serde(default)]
        final_answer: Option<String>,
        #[serde(default)]
        gaps: Option<String>,
    }

    match serde_json::from_str::<RawAudit>(&raw) {
        Ok(audit) => Ok(AuditResult {
            is_complete: audit.complete,
            final_answer: audit.final_answer,
            gaps: audit.gaps,
        }),
        Err(_) => {
            // Fallback: if output looks substantial, treat as complete
            let is_complete = output.len() > 50 && !output.contains("ALL STEPS COMPLETE");
            Ok(AuditResult {
                is_complete,
                final_answer: Some(output.to_string()),
                gaps: None,
            })
        }
    }
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

    fn llm_text(text: &str) -> LlmResponse {
        LlmResponse {
            message: AssistantMessage {
                text: Some(text.into()),
                tool_calls: vec![],
                reasoning_content: None,
            },
            usage: TokenUsage {
                prompt: 0,
                completion: 0,
            },
        }
    }

    #[tokio::test]
    async fn problem_solving_completes_with_plan_and_audit() {
        let llm = MockLlm::new(vec![
            // Plan phase: return a JSON plan
            Ok(llm_text(r#"["Step 1: Create file", "Step 2: Test it", "Step 3: Verify"]"#)),
            // Execute phase: LLM completes the plan
            Ok(llm_text("Executed step 1. ALL STEPS COMPLETE")),
            // Audit phase: complete=true
            Ok(llm_text(r#"{"complete": true, "final_answer": "Task done", "gaps": null}"#)),
        ]);
        let llm: Arc<dyn LlmClient> = Arc::new(llm);

        let tools = Arc::new(ToolDispatcher::new());
        let config = AgentConfig::default();
        let mut memory = crate::Memory::new(
            vec![Message::system("system")],
            10000,
            0.8,
            0,
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        let workspace = duga_sandbox::Workspace::open(dir.path()).unwrap();
        let sink: Arc<dyn duga_events::EventSink> = Arc::new(NullSink);

        struct DummySummarizer;
        impl crate::Summarizer for DummySummarizer {
            fn summarize<'a>(
                &'a self,
                _msgs: &'a [Message],
            ) -> crate::SummaryFuture<'a> {
                Box::pin(async {
                    Ok(duga_types::llm::SummaryMessage::new("summary".into()))
                })
            }
        }
        let summarizer: Arc<dyn crate::Summarizer> = Arc::new(DummySummarizer);
        let registry = Arc::new(LoopRegistry::new());
        let cancellation = duga_sandbox::CancellationToken::new();

        let mut ctx = LoopContext {
            config: &config,
            memory: &mut memory,
            llm: &llm,
            tools: &tools,
            workspace: &workspace,
            event_sink: &sink,
            summarizer: &summarizer,
            cancellation: &cancellation,
            registry: &registry,
            max_refinement_iterations: 3,
            max_delegation_depth: 2,
            delegation_depth: 0,
            steer: None,
            steer_limits: None,
        };

        let loop_impl = ProblemSolvingLoop;
        let result = loop_impl
            .run("Write hello world in Rust".into(), &mut ctx)
            .await
            .unwrap();

        assert_eq!(result.loop_id, "problem_solving");
        assert!(result.message.text.unwrap().contains("Task done"));
    }

    #[tokio::test]
    async fn refinement_respects_iteration_cap() {
        let mut responses = Vec::new();
        for _ in 0..3 {
            // Plan
            responses.push(Ok(llm_text(r#"["Step 1"]"#)));
            // Execute
            responses.push(Ok(llm_text("partial output. ALL STEPS COMPLETE")));
            // Audit: not complete
            responses.push(Ok(llm_text(
                r#"{"complete": false, "final_answer": null, "gaps": "missing test"}"#,
            )));
        }

        let llm = MockLlm::new(responses);
        let llm: Arc<dyn LlmClient> = Arc::new(llm);
        let tools = Arc::new(ToolDispatcher::new());
        let config = AgentConfig::default();
        let mut memory = crate::Memory::new(
            vec![Message::system("system")],
            10000,
            0.8,
            0,
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        let workspace = duga_sandbox::Workspace::open(dir.path()).unwrap();
        let sink: Arc<dyn duga_events::EventSink> = Arc::new(NullSink);
        struct DS;
        impl crate::Summarizer for DS {
            fn summarize<'a>(&'a self, _m: &'a [Message]) -> crate::SummaryFuture<'a> {
                Box::pin(async {
                    Ok(duga_types::llm::SummaryMessage::new("s".into()))
                })
            }
        }
        let summarizer: Arc<dyn crate::Summarizer> = Arc::new(DS);
        let registry = Arc::new(LoopRegistry::new());
        let cancellation = duga_sandbox::CancellationToken::new();

        let mut ctx = LoopContext {
            config: &config,
            memory: &mut memory,
            llm: &llm,
            tools: &tools,
            workspace: &workspace,
            event_sink: &sink,
            summarizer: &summarizer,
            cancellation: &cancellation,
            registry: &registry,
            max_refinement_iterations: 2,
            max_delegation_depth: 2,
            delegation_depth: 0,
            steer: None,
            steer_limits: None,
        };

        let loop_impl = ProblemSolvingLoop;
        let result = loop_impl
            .run("task".into(), &mut ctx)
            .await
            .unwrap();

        assert_eq!(result.loop_id, "problem_solving");
        assert!(result.message.text.unwrap().contains("refinement iterations"));
    }
}
