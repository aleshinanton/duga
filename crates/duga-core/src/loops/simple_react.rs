//! Simple ReAct loop — the default entry-point execution strategy.
//!
//! This is the loop that bots always call first.  It handles direct tool
//! calls, lookups, and single-step tasks.  For complex tasks the LLM can
//! delegate to a specialized loop via the `delegate` tool.

use crate::loop_context::LoopContext;
use crate::loop_result::LoopResult;
use crate::loop_trait::{Loop, LoopRunFuture};
use duga_events::Event;
use duga_llm::LlmClient;
use duga_sandbox::CancellationToken;
use duga_types::config::AgentConfig;
use duga_types::error::{AgentError, ToolError};
use duga_types::llm::LlmCallOptions;
use duga_types::tool_call::ToolCall;
use duga_types::tool_result::ToolResult;
use std::time::{Duration, Instant};

/// The default entry-point loop.  Stateless — all state lives in `LoopContext`.
pub struct SimpleReActLoop;

impl Loop for SimpleReActLoop {
    fn id(&self) -> &'static str {
        "simple_react"
    }

    fn name(&self) -> &'static str {
        "Simple ReAct"
    }

    fn description(&self) -> &'static str {
        "Direct tool calls for single-step and straightforward tasks. Uses think, \
         shell, read, edit, write, and search tools. When a task requires structured \
         planning, multi-step reasoning, verification, decomposition, or information \
         retrieval, delegate to the appropriate specialized loop."
    }

    fn run<'a>(&'a self, task: String, ctx: &'a mut LoopContext<'a>) -> LoopRunFuture<'a> {
        Box::pin(async move { run_simple_react(task, ctx).await })
    }
}

#[tracing::instrument(skip(ctx), fields(loop_id = "simple_react", depth = ctx.delegation_depth))]
async fn run_simple_react(
    task: String,
    ctx: &mut LoopContext<'_>,
) -> Result<LoopResult, AgentError> {
    let started = Instant::now();
    let mut tool_calls = 0;

    tracing::info!(task = %task, loop_id = "simple_react", "Agent run started");
    ctx.tools.reset_limits();
    ctx.event_sink
        .emit(Event::AgentStarted {
            task: task.clone(),
        })
        .await
        .map_err(|e| AgentError::EventSinkFailed(e.to_string()))?;

    // Set the task anchoring prefix so the LLM stays focused on the
    // current task even when the context contains older topics.
    ctx.memory.set_task_anchor(Some(task.clone()));

    // Append a reminder suffix to the user message as a
    // belt-and-suspenders measure for providers that may ignore
    // system messages.
    let anchored_task = format!("{task}\n\nReminder: Focus exclusively on the current task: {task}");
    ctx.memory.push_user(anchored_task);

    for step in 1..=ctx.config.limits.max_steps {
        if let Err(error) = check_limits(ctx.config, ctx.cancellation, started) {
            return fatal(ctx, error).await;
        }
        if let Err(error) = compress_if_needed(ctx).await {
            return fatal(ctx, error).await;
        }

        let memory_tokens = ctx.memory.token_count(ctx.llm.as_ref());
        tracing::info!(
            step,
            tool_calls_count = tool_calls,
            tokens_used = memory_tokens,
            duration_ms = started.elapsed().as_millis(),
            "Loop iteration"
        );
        ctx.event_sink
            .emit(Event::StepStarted { step })
            .await
            .map_err(|e| AgentError::EventSinkFailed(e.to_string()))?;

        let messages = ctx.memory.messages();
        let schemas = ctx.tools.schemas();
        ctx.event_sink
            .emit(Event::LlmRequest {
                model: ctx.llm.model().into(),
                messages: messages.clone(),
                tools: schemas.clone(),
            })
            .await
            .map_err(|e| AgentError::EventSinkFailed(e.to_string()))?;

        let llm_started = Instant::now();
        let response = match call_llm(
            ctx.llm.as_ref(),
            &messages,
            &schemas,
            ctx.event_sink.as_ref(),
            ctx.config,
            started,
            ctx.cancellation,
        )
        .await
        {
            Ok(response) => response,
            Err(error) => return fatal(ctx, error).await,
        };
        tracing::info!(
            llm_latency_ms = llm_started.elapsed().as_millis(),
            prompt_tokens = response.usage.prompt,
            completion_tokens = response.usage.completion,
            "LLM response"
        );
        let assistant = response.message;
        ctx.event_sink
            .emit(Event::LlmResponse {
                model: ctx.llm.model().into(),
                text: assistant.text.clone(),
                tool_calls: assistant.tool_calls.clone(),
            })
            .await
            .map_err(|e| AgentError::EventSinkFailed(e.to_string()))?;
        ctx.memory.push_assistant(assistant.clone());

        if assistant.is_termination() {
            ctx.event_sink
                .emit(Event::StepFinished { step })
                .await
                .map_err(|e| AgentError::EventSinkFailed(e.to_string()))?;
            ctx.event_sink
                .emit(Event::AgentFinished {
                    text: assistant.text.clone(),
                })
                .await
                .map_err(|e| AgentError::EventSinkFailed(e.to_string()))?;
            tracing::info!(
                result = %assistant.text.clone().unwrap_or_default(),
                duration_ms = started.elapsed().as_millis(),
                "Agent finished"
            );
            return Ok(LoopResult {
                message: assistant,
                steps: step,
                tool_calls,
                loop_id: "simple_react".into(),
            });
        }

        // ── Delegation intercept (full logic added in T26.5) ──
        // For now, just scan for delegate calls and handle them.
        // If a delegate call is found and succeeds, return the delegated result.
        if let Some(delegated) =
            try_handle_delegate(&assistant.tool_calls, &task, ctx).await?
        {
            return Ok(delegated);
        }

        for call in &assistant.tool_calls {
            if tool_calls >= ctx.config.limits.max_tool_calls {
                return fatal(
                    ctx,
                    AgentError::MaxToolCallsReached {
                        limit: ctx.config.limits.max_tool_calls,
                    },
                )
                .await;
            }
            tool_calls += 1;

            let result = match execute_tool(ctx, call, started, ctx.cancellation).await {
                Ok(result) => result,
                Err(error) => return fatal(ctx, error).await,
            };
            ctx.memory.push_tool_result(result);
            if let Err(error) = check_limits(ctx.config, ctx.cancellation, started) {
                return fatal(ctx, error).await;
            }
        }

        if let Err(error) = compress_if_needed(ctx).await {
            return fatal(ctx, error).await;
        }
        ctx.event_sink
            .emit(Event::StepFinished { step })
            .await
            .map_err(|e| AgentError::EventSinkFailed(e.to_string()))?;
    }

    fatal(
        ctx,
        AgentError::MaxStepsReached {
            limit: ctx.config.limits.max_steps,
        },
    )
    .await
}

/// Placeholder — delegates are intercepted but no other loops are registered yet.
///
/// Full delegation logic is added in T26.5. For now we just check if any tool
/// call is named "delegate" and return an error tool result to memory so the
/// LLM learns it can't use delegation yet.
async fn try_handle_delegate(
    tool_calls: &[ToolCall],
    task: &str,
    ctx: &mut LoopContext<'_>,
) -> Result<Option<LoopResult>, AgentError> {
    for call in tool_calls {
        if call.tool != "delegate" {
            continue;
        }

        // If no loops beyond simple_react are enabled, push an error.
        if ctx.registry.is_empty() {
            let error_result = ToolResult::from_outcome(
                call.id.clone(),
                "delegate",
                Err(ToolError::InvalidArgs(
                    "no specialized loops are enabled. Proceed with your standard tools."
                        .into(),
                )),
                Instant::now(),
            );
            ctx.memory.push_tool_result(error_result);
            return Ok(None);
        }

        // Check depth.
        if ctx.delegation_depth >= ctx.max_delegation_depth {
            let error_result = ToolResult::from_outcome(
                call.id.clone(),
                "delegate",
                Err(ToolError::Denied(format!(
                    "max delegation depth ({}) reached",
                    ctx.max_delegation_depth
                ))),
                Instant::now(),
            );
            ctx.memory.push_tool_result(error_result);
            return Ok(None);
        }

        // Look up target loop.
        let loop_id = call.raw_args.get("loop").and_then(|v| v.as_str());
        let Some(target_id) = loop_id else {
            let error_result = ToolResult::from_outcome(
                call.id.clone(),
                "delegate",
                Err(ToolError::InvalidArgs(
                    "missing 'loop' field in delegate args".into(),
                )),
                Instant::now(),
            );
            ctx.memory.push_tool_result(error_result);
            return Ok(None);
        };

        let target = match ctx.registry.get(target_id) {
            Some(l) => l,
            None => {
                let error_result = ToolResult::from_outcome(
                    call.id.clone(),
                    "delegate",
                    Err(ToolError::InvalidArgs(format!(
                        "unknown loop '{}'. Use one of the available strategy ids from the prompt.",
                        target_id
                    ))),
                    Instant::now(),
                );
                ctx.memory.push_tool_result(error_result);
                return Ok(None);
            }
        };

        let reason = call
            .raw_args
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("no reason given")
            .to_string();

        // Build child context with incremented depth.
        let new_depth = ctx.delegation_depth + 1;
        let mut child_ctx = LoopContext {
            config: ctx.config,
            memory: ctx.memory,
            llm: ctx.llm,
            tools: ctx.tools,
            workspace: ctx.workspace,
            event_sink: ctx.event_sink,
            summarizer: ctx.summarizer,
            cancellation: ctx.cancellation,
            registry: ctx.registry,
            max_refinement_iterations: ctx.max_refinement_iterations,
            max_delegation_depth: ctx.max_delegation_depth,
            delegation_depth: new_depth,
        };

        // Emit delegation event.
        ctx.event_sink
            .emit(Event::LoopDelegated {
                from: "simple_react".into(),
                to: target_id.to_string(),
                reason: reason.clone(),
                depth: new_depth,
            })
            .await
            .map_err(|e| AgentError::EventSinkFailed(e.to_string()))?;

        // Run the target loop.
        let delegated_result = target
            .run(task.to_string(), &mut child_ctx)
            .await?;

        // Push the result as a tool result so the conversation records it.
        let tool_result = duga_types::tool_result::ToolResultBuilder::new()
            .tool_call_id(call.id.clone())
            .success(true)
            .output(delegated_result.message.text.clone().unwrap_or_default())
            .build()
            .unwrap();
        ctx.memory.push_tool_result(tool_result);

        return Ok(Some(delegated_result));
    }

    Ok(None)
}

// ── Helpers (free functions adapted from AgentLoop) ──────────────────────

async fn call_llm(
    llm: &dyn LlmClient,
    messages: &[duga_types::message::Message],
    tools: &[duga_types::tool_schema::ToolSchema],
    event_sink: &dyn duga_events::EventSink,
    config: &AgentConfig,
    started: Instant,
    cancellation: &CancellationToken,
) -> Result<duga_types::llm::LlmResponse, AgentError> {
    if cancellation.is_cancelled() {
        return Err(AgentError::Cancelled);
    }

    let remaining = remaining_runtime(config, started)?;
    let mut cancelled = cancellation.subscribe();
    let call = llm.chat(
        messages,
        tools,
        LlmCallOptions {
            streaming: config.features.streaming,
        },
        event_sink,
    );

    tokio::select! {
        result = tokio::time::timeout(remaining, call) => {
            match result {
                Ok(Ok(response)) => Ok(response),
                Ok(Err(error)) => Err(AgentError::LlmFailed(error.to_string())),
                Err(_) => Err(AgentError::Timeout),
            }
        }
        _ = cancelled.changed() => Err(AgentError::Cancelled),
    }
}

async fn execute_tool(
    ctx: &LoopContext<'_>,
    call: &ToolCall,
    loop_started: Instant,
    cancellation: &CancellationToken,
) -> Result<ToolResult, AgentError> {
    let retryable = ctx
        .tools
        .get(&call.tool)
        .map(|tool| tool.retryable)
        .unwrap_or(false);
    let span = tracing::info_span!("tool_call", tool = %call.tool, id = %call.id);
    let _entered = span.enter();
    let max_attempts = ctx.config.limits.retry_on_error + 1;
    let mut last_error = None;

    for attempt in 1..=max_attempts {
        check_limits(ctx.config, cancellation, loop_started)?;
        ctx.event_sink
            .emit(Event::ToolCallStarted {
                tool_call: call.clone(),
                attempt,
            })
            .await
            .map_err(|e| AgentError::EventSinkFailed(e.to_string()))?;

        let started = Instant::now();
        let remaining = remaining_runtime(ctx.config, loop_started)?;
        let dispatch = ctx.tools.dispatch(
            call,
            ctx.workspace,
            cancellation.clone(),
            ctx.event_sink.as_ref(),
        );
        let outcome = tokio::time::timeout(remaining, dispatch).await;
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(_) => {
                cancellation.cancel();
                return Err(AgentError::Timeout);
            }
        };

        match outcome {
            Ok(result) => {
                tracing::info!(
                    tool_duration_ms = result.duration_ms,
                    tool_success = result.success,
                    "Tool finished"
                );
                ctx.event_sink
                    .emit(Event::ToolCallFinished {
                        result: result.clone(),
                        attempt,
                        tool_name: call.tool.clone(),
                    })
                    .await
                    .map_err(|e| AgentError::EventSinkFailed(e.to_string()))?;
                return Ok(result);
            }
            Err(error) if attempt < max_attempts && retryable && error.is_transient() => {
                let result = tool_error_result(call, error.clone(), started);
                tracing::info!(
                    tool_duration_ms = result.duration_ms,
                    tool_success = false,
                    "Tool retry scheduled"
                );
                ctx.event_sink
                    .emit(Event::ToolCallFinished {
                        result,
                        attempt,
                        tool_name: call.tool.clone(),
                    })
                    .await
                    .map_err(|e| AgentError::EventSinkFailed(e.to_string()))?;
                last_error = Some(error);
            }
            Err(error) => {
                let result = tool_error_result(call, error, started);
                tracing::info!(
                    tool_duration_ms = result.duration_ms,
                    tool_success = false,
                    "Tool finished"
                );
                ctx.event_sink
                    .emit(Event::ToolCallFinished {
                        result: result.clone(),
                        attempt,
                        tool_name: call.tool.clone(),
                    })
                    .await
                    .map_err(|e| AgentError::EventSinkFailed(e.to_string()))?;
                return Ok(result);
            }
        }
    }

    let result = tool_error_result(
        call,
        last_error.unwrap_or_else(|| ToolError::Plugin("retry attempts exhausted".into())),
        Instant::now(),
    );
    ctx.event_sink
        .emit(Event::ToolCallFinished {
            result: result.clone(),
            attempt: max_attempts,
            tool_name: call.tool.clone(),
        })
        .await
        .map_err(|e| AgentError::EventSinkFailed(e.to_string()))?;
    Ok(result)
}

async fn compress_if_needed(ctx: &mut LoopContext<'_>) -> Result<(), AgentError> {
    if !ctx.memory.over_budget(ctx.llm.as_ref()) {
        return Ok(());
    }

    let before = ctx.memory.token_count(ctx.llm.as_ref());
    tracing::info!(
        memory_tokens = before,
        max_tokens = ctx.memory.max_tokens(),
        "Memory budget check"
    );
    ctx.memory
        .compress(ctx.summarizer.as_ref(), ctx.llm.as_ref())
        .await?;
    let after = ctx.memory.token_count(ctx.llm.as_ref());
    tracing::info!(
        before_tokens = before,
        after_tokens = after,
        "Memory compressed"
    );
    ctx.event_sink
        .emit(Event::MemoryCompressed {
            before_tokens: before,
            after_tokens: after,
        })
        .await
        .map_err(|e| AgentError::EventSinkFailed(e.to_string()))
}

fn check_limits(
    config: &AgentConfig,
    cancellation: &CancellationToken,
    started: Instant,
) -> Result<(), AgentError> {
    if cancellation.is_cancelled() {
        return Err(AgentError::Cancelled);
    }
    remaining_runtime(config, started).map(|_| ())
}

fn remaining_runtime(config: &AgentConfig, started: Instant) -> Result<Duration, AgentError> {
    config
        .limits
        .max_runtime
        .checked_sub(started.elapsed())
        .filter(|remaining| !remaining.is_zero())
        .ok_or(AgentError::Timeout)
}

async fn fatal<T>(ctx: &LoopContext<'_>, error: AgentError) -> Result<T, AgentError> {
    ctx.event_sink
        .emit(Event::Error {
            message: error.to_string(),
        })
        .await
        .map_err(|e| AgentError::EventSinkFailed(e.to_string()))?;
    Err(error)
}

fn tool_error_result(call: &ToolCall, error: ToolError, started: Instant) -> ToolResult {
    ToolResult::from_outcome(call.id.clone(), &call.tool, Err(error), started)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loop_registry::LoopRegistry;
    use crate::summarizer::Summarizer;
    use crate::testing::{CapturingEventSink, MockTool};
    use duga_events::{EventSink, NullSink};
    use duga_llm::dummy::DummyClient;
    use duga_llm::LlmClient;
    use duga_tools::{ErasedTool, ToolDispatcher};
    use duga_types::llm::{LlmResponse, TokenUsage};
    use duga_types::message::{AssistantMessage, ContentBlock, Message};
    use std::sync::Arc;

    struct StaticSummarizer;

    impl Summarizer for StaticSummarizer {
        fn summarize<'a>(
            &'a self,
            _messages: &'a [Message],
        ) -> crate::summarizer::SummaryFuture<'a> {
            Box::pin(async {
                Ok(duga_types::llm::SummaryMessage::new("summary".into()))
            })
        }
    }

    fn llm_response(message: AssistantMessage) -> LlmResponse {
        LlmResponse {
            message,
            usage: TokenUsage {
                prompt: 0,
                completion: 0,
            },
        }
    }

    fn make_ctx<'a>(
        config: &'a AgentConfig,
        memory: &'a mut crate::Memory,
        llm: &'a Arc<dyn LlmClient>,
        tools: &'a Arc<ToolDispatcher>,
        workspace: &'a duga_sandbox::Workspace,
        event_sink: &'a Arc<dyn EventSink>,
        summarizer: &'a Arc<dyn crate::Summarizer>,
        registry: &'a Arc<LoopRegistry>,
        cancellation: &'a duga_sandbox::CancellationToken,
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
            max_refinement_iterations: config.loop_config.max_refinement_iterations,
            max_delegation_depth: config.loop_config.max_delegation_depth,
            delegation_depth: 0,
        }
    }

    #[tokio::test]
    async fn run_returns_terminal_assistant_message() {
        let llm: Arc<dyn LlmClient> = Arc::new(DummyClient::with_response(
            "dummy",
            llm_response(AssistantMessage {
                text: Some("done".into()),
                tool_calls: vec![],
                reasoning_content: None,
            }),
        ));
        let sink: Arc<dyn EventSink> = Arc::new(CapturingEventSink::new());
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
        let summarizer: Arc<dyn crate::Summarizer> = Arc::new(StaticSummarizer);
        let registry = Arc::new(LoopRegistry::new());

        let cancellation = duga_sandbox::CancellationToken::new();
        let mut ctx = make_ctx(
            &config,
            &mut memory,
            &llm,
            &tools,
            &workspace,
            &sink,
            &summarizer,
            &registry,
            &cancellation,
        );

        let loop_impl = SimpleReActLoop;
        let result = loop_impl.run("task".into(), &mut ctx).await.unwrap();

        assert_eq!(result.message.text, Some("done".into()));
        assert_eq!(result.steps, 1);
        assert_eq!(result.loop_id, "simple_react");
        assert_eq!(result.tool_calls, 0);
    }

    #[tokio::test]
    async fn run_executes_tools_and_feeds_results_to_next_step() {
        let call = ToolCall::new("echo", serde_json::json!({"msg": "hello"}));

        let llm = DummyClient::default();
        llm.push_response(llm_response(AssistantMessage {
            text: None,
            tool_calls: vec![call.clone()],
            reasoning_content: None,
        }));
        llm.push_response(llm_response(AssistantMessage {
            text: Some("done".into()),
            tool_calls: vec![],
            reasoning_content: None,
        }));

        let llm: Arc<dyn LlmClient> = Arc::new(llm);
        let tools = Arc::new(ToolDispatcher::new());
        tools
            .register_erased(ErasedTool::erase(
                MockTool::new("echo", "echo").always(Ok(MockTool::success("hello"))),
            ))
            .unwrap();

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
        let sink: Arc<dyn EventSink> = Arc::new(NullSink);
        let summarizer: Arc<dyn crate::Summarizer> = Arc::new(StaticSummarizer);
        let registry = Arc::new(LoopRegistry::new());
        let cancellation = duga_sandbox::CancellationToken::new();

        let mut ctx = make_ctx(
            &config,
            &mut memory,
            &llm,
            &tools,
            &workspace,
            &sink,
            &summarizer,
            &registry,
            &cancellation,
        );

        let loop_impl = SimpleReActLoop;
        let result = loop_impl.run("task".into(), &mut ctx).await.unwrap();

        assert_eq!(result.steps, 2);
        assert_eq!(result.tool_calls, 1);
        assert!(memory
            .messages()
            .iter()
            .any(|message| matches!(message.content.first(), Some(ContentBlock::Text { text }) if text == "hello")));
    }

    #[tokio::test]
    async fn cancellation_before_run_is_fatal() {
        let tools = Arc::new(ToolDispatcher::new());
        let llm: Arc<dyn LlmClient> = Arc::new(DummyClient::default());

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
        let sink: Arc<dyn EventSink> = Arc::new(NullSink);
        let summarizer: Arc<dyn crate::Summarizer> = Arc::new(StaticSummarizer);
        let registry = Arc::new(LoopRegistry::new());
        let cancellation = duga_sandbox::CancellationToken::new();
        cancellation.cancel();

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
            max_refinement_iterations: config.loop_config.max_refinement_iterations,
            max_delegation_depth: config.loop_config.max_delegation_depth,
            delegation_depth: 0,
        };

        let loop_impl = SimpleReActLoop;
        let err = loop_impl.run("task".into(), &mut ctx).await.unwrap_err();

        assert_eq!(err, AgentError::Cancelled);
    }
}
