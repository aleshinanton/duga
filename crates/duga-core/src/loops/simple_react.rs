//! Simple ReAct loop — the default entry-point execution strategy.
//!
//! This is the loop that bots always call first.  It handles direct tool
//! calls, lookups, and single-step tasks.  For complex tasks the LLM can
//! delegate to a specialized loop via the `delegate` tool.

use crate::loop_context::LoopContext;
use crate::loop_result::LoopResult;
use crate::loop_trait::{Loop, LoopRunFuture};
use crate::steering::{apply_context_event, check_steer, SteerAction, SteeringContextEvent};
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
        "Default strategy — direct tool use. For calculations, lookups, file \
         edits, and straightforward queries. Use your normal tools directly. \
         Only delegate when the task genuinely needs a specialized workflow."
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
    let mut consecutive_errors: u32 = 0;

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

    'outer: for step in 1..=ctx.config.limits.max_steps {
        if let Err(error) = check_limits(ctx.config, ctx.cancellation, started) {
            return fatal(ctx, error).await;
        }
        if let Err(error) = compress_if_needed(ctx).await {
            return fatal(ctx, error).await;
        }

        // ── POINT 0: Steering before LLM call ──
        match check_steer(ctx, true).await? {
            SteerAction::Cancel(_reason) => {
                return fatal(ctx, AgentError::Cancelled).await;
            }
            SteerAction::Complete(answer) => {
                return complete_from_steer(ctx, answer).await;
            }
            SteerAction::Reprompt => {
                continue 'outer;
            }
            SteerAction::Continue => { /* normal flow */ }
        }

        // ── Policy steer: config-driven rules ──
        for rule in &ctx.config.steering.rules {
            if rule.repeat || step == 1 {
                apply_context_event(
                    ctx,
                    SteeringContextEvent::InjectGuidance {
                        text: rule.guidance.clone(),
                        as_system: rule.as_system,
                        source: "policy".into(),
                    },
                )
                .await?;
            }
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

        if assistant.is_termination() {
            ctx.memory.push_assistant(assistant.clone());
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

        // ── Delegation intercept ──
        // Handle delegate BEFORE pushing assistant to memory.
        // This is critical: the target loop calls the LLM with memory.messages(),
        // and OpenAI rejects requests where an assistant has unresolved tool_calls.
        let mut delegate_skip_id: Option<duga_types::tool_call::CallId> = None;
        match try_handle_delegate(&assistant.tool_calls, &task, ctx).await? {
            DelegateOutcome::Success(delegated_result) => {
                // Push assistant + delegate tool result so conversation is valid.
                ctx.memory.push_assistant(assistant.clone());
                // Push skipped results for any remaining tool calls.
                for call in &assistant.tool_calls {
                    if call.tool == "delegate" {
                        continue;
                    }
                    let skipped = ToolResult::from_outcome(
                        call.id.clone(),
                        &call.tool,
                        Err(ToolError::Cancelled),
                        Instant::now(),
                    );
                    ctx.memory.push_tool_result(skipped);
                }
                return Ok(delegated_result);
            }
            DelegateOutcome::Error(error_result) => {
                // Push assistant first, then the error tool result.
                ctx.memory.push_assistant(assistant.clone());
                ctx.memory.push_tool_result(error_result);
                // Track which delegate call was handled so we skip it below.
                delegate_skip_id = assistant
                    .tool_calls
                    .iter()
                    .find(|c| c.tool == "delegate")
                    .map(|c| c.id.clone());
                // Fall through to normal tool loop (skipping the handled delegate).
            }
            DelegateOutcome::NotFound => {
                // No delegate call — push assistant and proceed normally.
                ctx.memory.push_assistant(assistant.clone());
            }
        }

        for call in &assistant.tool_calls {
            // Skip the delegate call already handled by the intercept
            if let Some(ref skip_id) = delegate_skip_id {
                if call.id == *skip_id {
                    continue;
                }
            }
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

            let mut result = match execute_tool(ctx, call, started, ctx.cancellation).await {
                Ok(result) => result,
                Err(error) => return fatal(ctx, error).await,
            };
            let tool_success = result.success;
            let steering_hint = result.steering_hint.take();
            ctx.memory.push_tool_result(result);

            // ── Tool steer: process steering hints from tools ──
            if let Some(hint) = steering_hint {
                apply_context_event(
                    ctx,
                    SteeringContextEvent::InjectGuidance {
                        text: hint,
                        as_system: false,
                        source: "tool".into(),
                    },
                )
                .await?;
            }

            // ── Self-steering: track consecutive errors ──
            if tool_success {
                consecutive_errors = 0;
            } else {
                consecutive_errors += 1;
                if consecutive_errors >= 3 {
                    apply_context_event(
                        ctx,
                        SteeringContextEvent::InjectGuidance {
                            text: "You have failed 3 times in a row. Pivot to a different \
                                   approach. Consider: (1) using a different tool, \
                                   (2) breaking the problem down further, \
                                   (3) explaining what's blocking you."
                                .into(),
                            as_system: true,
                            source: "self-diagnosis".into(),
                        },
                    )
                    .await?;
                    consecutive_errors = 0; // reset after injecting
                }
            }

            if let Err(error) = check_limits(ctx.config, ctx.cancellation, started) {
                return fatal(ctx, error).await;
            }
            // ── POINT 2: Steering after tool result ──
            match check_steer(ctx, false).await? {
                SteerAction::Cancel(_reason) => return fatal(ctx, AgentError::Cancelled).await,
                SteerAction::Complete(answer) => return complete_from_steer(ctx, answer).await,
                SteerAction::Reprompt => unreachable!("Reprompt buffered at POINT 2"),
                SteerAction::Continue => { /* next tool */ }
            }
        }

        if let Err(error) = compress_if_needed(ctx).await {
            return fatal(ctx, error).await;
        }
        // ── POINT 3: Steering before next iteration ──
        match check_steer(ctx, true).await? {
            SteerAction::Cancel(_reason) => return fatal(ctx, AgentError::Cancelled).await,
            SteerAction::Complete(answer) => return complete_from_steer(ctx, answer).await,
            SteerAction::Reprompt => continue 'outer,
            SteerAction::Continue => { /* fall through to next step */ }
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

/// Outcome of delegation intercept.
enum DelegateOutcome {
    /// No delegate call found — proceed normally.
    NotFound,
    /// Delegation succeeded — this is the final answer.
    Success(LoopResult),
    /// Delegation failed — push this error tool result after push_assistant.
    Error(ToolResult),
}

/// Handle delegate tool call BEFORE the assistant is pushed to memory.
///
/// The target loop runs with the current memory state (no unresolved delegate),
/// so its LLM calls won't trigger OpenAI's "insufficient tool messages" error.
/// The caller pushes assistant + results AFTER this function returns.
async fn try_handle_delegate(
    tool_calls: &[ToolCall],
    task: &str,
    ctx: &mut LoopContext<'_>,
) -> Result<DelegateOutcome, AgentError> {
    let delegate_call = match tool_calls.iter().find(|c| c.tool == "delegate") {
        Some(c) => c,
        None => return Ok(DelegateOutcome::NotFound),
    };

    // Check if any specialized loops are registered.
    let has_specialized = ctx
        .registry
        .get("problem_solving")
        .or_else(|| ctx.registry.get("verification"))
        .or_else(|| ctx.registry.get("decomposition"))
        .or_else(|| ctx.registry.get("search"))
        .is_some();

    if !has_specialized {
        return Ok(DelegateOutcome::Error(ToolResult::from_outcome(
            delegate_call.id.clone(),
            "delegate",
            Err(ToolError::InvalidArgs(
                "no specialized loops are enabled".into(),
            )),
            Instant::now(),
        )));
    }

    // Check depth.
    if ctx.delegation_depth >= ctx.max_delegation_depth {
        return Ok(DelegateOutcome::Error(ToolResult::from_outcome(
            delegate_call.id.clone(),
            "delegate",
            Err(ToolError::Denied(format!(
                "max delegation depth ({}) reached",
                ctx.max_delegation_depth
            ))),
            Instant::now(),
        )));
    }

    // Look up target loop.
    let target_id = match delegate_call.raw_args.get("loop").and_then(|v| v.as_str()) {
        Some(id) => id,
        None => {
            return Ok(DelegateOutcome::Error(ToolResult::from_outcome(
                delegate_call.id.clone(),
                "delegate",
                Err(ToolError::InvalidArgs("missing 'loop' field".into())),
                Instant::now(),
            )));
        }
    };

    let target = match ctx.registry.get(target_id) {
        Some(l) => l,
        None => {
            return Ok(DelegateOutcome::Error(ToolResult::from_outcome(
                delegate_call.id.clone(),
                "delegate",
                Err(ToolError::InvalidArgs(format!(
                    "unknown loop '{}'",
                    target_id
                ))),
                Instant::now(),
            )));
        }
    };

    // Use the actual task as the reason — never trust the LLM's reason
    // field since it can hallucinate content from previous conversations.
    let reason = format!(
        "{}…",
        task.chars().take(80).collect::<String>().trim()
    );

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
        steer: None,
        steer_limits: None,
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

    // Run the target loop (memory does NOT have the delegate assistant yet).
    let delegated_result = target
        .run(task.to_string(), &mut child_ctx)
        .await?;

    // Build the tool result for the delegate call (caller pushes after assistant).
    let tool_result = duga_types::tool_result::ToolResultBuilder::new()
        .tool_call_id(delegate_call.id.clone())
        .success(true)
        .output(delegated_result.message.text.clone().unwrap_or_default())
        .build()
        .unwrap();
    ctx.memory.push_tool_result(tool_result);

    Ok(DelegateOutcome::Success(delegated_result))
}

// ── Public helpers for specialized loops ─────────────────────────────────

/// Dispatch a tool call with proper event emissions, retry, and timeout.
///
/// Specialized loops should use this instead of calling `ctx.tools.dispatch()`
/// directly so that frontends see tool progress during delegation.
///
/// Retries up to `config.limits.retry_on_error` additional times on transient
/// errors for tools that opt in via `Tool::retryable`.  Each dispatch is
/// bounded by the remaining runtime.
pub async fn dispatch_tool_with_events(
    ctx: &LoopContext<'_>,
    call: &ToolCall,
) -> Result<ToolResult, AgentError> {
    // Delegate calls are handled by SimpleReActLoop's try_handle_delegate —
    // they must never reach the tool dispatcher (the DelegateTool always errors).
    if call.tool == "delegate" {
        return Err(AgentError::EventSinkFailed(
            "delegate tool called outside SimpleReActLoop".into(),
        ));
    }

    let retryable = ctx
        .tools
        .get(&call.tool)
        .map(|tool| tool.retryable)
        .unwrap_or(false);
    let max_attempts = ctx.config.limits.retry_on_error + 1;
    let mut last_error: Option<duga_types::error::ToolError> = None;

    for attempt in 1..=max_attempts {
        if ctx.cancellation.is_cancelled() {
            return Err(AgentError::Cancelled);
        }

        ctx.event_sink
            .emit(Event::ToolCallStarted {
                tool_call: call.clone(),
                attempt,
            })
            .await
            .map_err(|e| AgentError::EventSinkFailed(e.to_string()))?;

        let start = Instant::now();
        let dispatch = ctx.tools.dispatch(
            call,
            ctx.workspace,
            ctx.cancellation.clone(),
            ctx.event_sink.as_ref(),
        );

        // Timeout bounded by config.max_runtime — but we don't have a
        // loop_started Instant here.  Use a generous 60s timeout that the
        // cancellation token can still interrupt.
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            dispatch,
        )
        .await;

        match outcome {
            Ok(Ok(result)) => {
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
            Ok(Err(error)) if attempt < max_attempts && retryable && error.is_transient() => {
                let result = ToolResult::from_outcome(
                    call.id.clone(),
                    &call.tool,
                    Err(error.clone()),
                    start,
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
            Ok(Err(error)) => {
                let result = ToolResult::from_outcome(
                    call.id.clone(),
                    &call.tool,
                    Err(error),
                    start,
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
            Err(_timeout) => {
                // Timeout — return the last error if any, or a timeout error
                let error = last_error
                    .unwrap_or_else(|| duga_types::error::ToolError::Timeout);
                let result = ToolResult::from_outcome(
                    call.id.clone(),
                    &call.tool,
                    Err(error),
                    start,
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

    // All attempts exhausted
    let result = ToolResult::from_outcome(
        call.id.clone(),
        &call.tool,
        Err(last_error.unwrap_or_else(|| {
            duga_types::error::ToolError::Plugin("retry attempts exhausted".into())
        })),
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

/// Complete the loop with a forced answer from steering.
async fn complete_from_steer(
    ctx: &mut LoopContext<'_>,
    answer: String,
) -> Result<LoopResult, AgentError> {
    let msg = duga_types::message::AssistantMessage {
        text: Some(answer.clone()),
        tool_calls: vec![],
        reasoning_content: None,
    };
    ctx.memory.push_assistant(msg.clone());
    ctx.event_sink
        .emit(Event::AgentFinished {
            text: Some(answer),
        })
        .await
        .map_err(|e| AgentError::EventSinkFailed(e.to_string()))?;
    Ok(LoopResult {
        message: msg,
        steps: 0,
        tool_calls: 0,
        loop_id: "simple_react".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loop_registry::LoopRegistry;
    use crate::steering::{SteeringControlEvent, SteeringEvent, SteeringReceiver, SteeringSender};
    use crate::summarizer::Summarizer;
    use crate::testing::{CapturingEventSink, MockTool};
    use duga_events::{EventSink, NullSink};
    use duga_llm::dummy::DummyClient;
    use duga_llm::LlmClient;
    use duga_tools::{ErasedTool, ToolDispatcher};
    use duga_types::llm::{LlmResponse, TokenUsage};
    use duga_types::message::{AssistantMessage, ContentBlock, Message};
    use std::sync::Arc;
    use tokio::sync::mpsc;

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
            steer: None,
            steer_limits: None,
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
            steer: None,
            steer_limits: None,
        };

        let loop_impl = SimpleReActLoop;
        let err = loop_impl.run("task".into(), &mut ctx).await.unwrap_err();

        assert_eq!(err, AgentError::Cancelled);
    }

    // ── Steering integration tests ────────────────────────────────────

    fn make_steer_channel() -> (SteeringSender, SteeringReceiver) {
        let (tx, rx) = mpsc::unbounded_channel();
        (SteeringSender::new(tx), SteeringReceiver::new(rx))
    }

    #[tokio::test]
    async fn steer_none_has_zero_overhead_identical_to_baseline() {
        // Verify that when steer is None, the loop behaves identically to before.
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
            10000, 0.8, 0, 0,
        );
        let dir = tempfile::tempdir().unwrap();
        let workspace = duga_sandbox::Workspace::open(dir.path()).unwrap();
        let summarizer: Arc<dyn crate::Summarizer> = Arc::new(StaticSummarizer);
        let registry = Arc::new(LoopRegistry::new());
        let cancellation = duga_sandbox::CancellationToken::new();

        let mut ctx = make_ctx(
            &config, &mut memory, &llm, &tools, &workspace,
            &sink, &summarizer, &registry, &cancellation,
        );
        // steer is None by default in make_ctx
        assert!(ctx.steer.is_none());

        let loop_impl = SimpleReActLoop;
        let result = loop_impl.run("task".into(), &mut ctx).await.unwrap();
        assert_eq!(result.message.text, Some("done".into()));
    }

    #[tokio::test]
    async fn cancel_at_point_0_terminates_loop() {
        let llm: Arc<dyn LlmClient> = Arc::new(DummyClient::default());
        let sink: Arc<dyn EventSink> = Arc::new(NullSink);
        let tools = Arc::new(ToolDispatcher::new());

        let config = AgentConfig::default();
        let mut memory = crate::Memory::new(
            vec![Message::system("system")],
            10000, 0.8, 0, 0,
        );
        let dir = tempfile::tempdir().unwrap();
        let workspace = duga_sandbox::Workspace::open(dir.path()).unwrap();
        let summarizer: Arc<dyn crate::Summarizer> = Arc::new(StaticSummarizer);
        let registry = Arc::new(LoopRegistry::new());
        let cancellation = duga_sandbox::CancellationToken::new();

        let (sender, receiver) = make_steer_channel();
        // Inject Cancel before the loop starts
        sender
            .inject(SteeringEvent::Control(SteeringControlEvent::Cancel {
                reason: "test cancel".into(),
            }))
            .unwrap();

        let mut ctx = make_ctx(
            &config, &mut memory, &llm, &tools, &workspace,
            &sink, &summarizer, &registry, &cancellation,
        );
        ctx.steer = Some(receiver);

        let loop_impl = SimpleReActLoop;
        let err = loop_impl.run("task".into(), &mut ctx).await.unwrap_err();
        assert_eq!(err, AgentError::Cancelled);
    }

    #[tokio::test]
    async fn complete_at_point_0_returns_forced_answer() {
        let llm: Arc<dyn LlmClient> = Arc::new(DummyClient::default());
        let sink: Arc<dyn EventSink> = Arc::new(NullSink);
        let tools = Arc::new(ToolDispatcher::new());

        let config = AgentConfig::default();
        let mut memory = crate::Memory::new(
            vec![Message::system("system")],
            10000, 0.8, 0, 0,
        );
        let dir = tempfile::tempdir().unwrap();
        let workspace = duga_sandbox::Workspace::open(dir.path()).unwrap();
        let summarizer: Arc<dyn crate::Summarizer> = Arc::new(StaticSummarizer);
        let registry = Arc::new(LoopRegistry::new());
        let cancellation = duga_sandbox::CancellationToken::new();

        let (sender, receiver) = make_steer_channel();
        sender
            .inject(SteeringEvent::Control(SteeringControlEvent::ForceComplete {
                answer: "forced answer".into(),
            }))
            .unwrap();

        let mut ctx = make_ctx(
            &config, &mut memory, &llm, &tools, &workspace,
            &sink, &summarizer, &registry, &cancellation,
        );
        ctx.steer = Some(receiver);

        let loop_impl = SimpleReActLoop;
        let result = loop_impl.run("task".into(), &mut ctx).await.unwrap();
        assert_eq!(result.message.text, Some("forced answer".into()));
    }

    #[tokio::test]
    async fn reprompt_at_point_0_re_calls_llm() {
        // The LLM is called first time, returns a tool call. Then Reprompt
        // arrives at POINT 0 before step 2, causing continue 'outer.
        let call = ToolCall::new("echo", serde_json::json!({"msg": "hello"}));

        let llm = DummyClient::default();
        llm.push_response(llm_response(AssistantMessage {
            text: None,
            tool_calls: vec![call.clone()],
            reasoning_content: None,
        }));
        llm.push_response(llm_response(AssistantMessage {
            text: Some("done after reprompt".into()),
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
            10000, 0.8, 0, 0,
        );
        let dir = tempfile::tempdir().unwrap();
        let workspace = duga_sandbox::Workspace::open(dir.path()).unwrap();
        let sink: Arc<dyn EventSink> = Arc::new(NullSink);
        let summarizer: Arc<dyn crate::Summarizer> = Arc::new(StaticSummarizer);
        let registry = Arc::new(LoopRegistry::new());
        let cancellation = duga_sandbox::CancellationToken::new();

        let (sender, receiver) = make_steer_channel();
        // Inject Reprompt — will be picked up at POINT 0 on step 2
        sender
            .inject(SteeringEvent::Control(SteeringControlEvent::Reprompt {
                guidance: "try differently".into(),
            }))
            .unwrap();

        let mut ctx = make_ctx(
            &config, &mut memory, &llm, &tools, &workspace,
            &sink, &summarizer, &registry, &cancellation,
        );
        ctx.steer = Some(receiver);

        let loop_impl = SimpleReActLoop;
        let result = loop_impl.run("task".into(), &mut ctx).await.unwrap();
        assert_eq!(
            result.message.text,
            Some("done after reprompt".into()),
            "Should get the second LLM response after reprompt"
        );
    }

    #[tokio::test]
    async fn cancel_during_tool_execution_at_point_2() {
        let call = ToolCall::new("echo", serde_json::json!({"msg": "hello"}));

        let llm: Arc<dyn LlmClient> = Arc::new(DummyClient::with_response(
            "dummy",
            llm_response(AssistantMessage {
                text: None,
                tool_calls: vec![call.clone()],
                reasoning_content: None,
            }),
        ));
        let tools = Arc::new(ToolDispatcher::new());
        tools
            .register_erased(ErasedTool::erase(
                MockTool::new("echo", "echo").always(Ok(MockTool::success("hello"))),
            ))
            .unwrap();

        let config = AgentConfig::default();
        let mut memory = crate::Memory::new(
            vec![Message::system("system")],
            10000, 0.8, 0, 0,
        );
        let dir = tempfile::tempdir().unwrap();
        let workspace = duga_sandbox::Workspace::open(dir.path()).unwrap();
        let sink: Arc<dyn EventSink> = Arc::new(NullSink);
        let summarizer: Arc<dyn crate::Summarizer> = Arc::new(StaticSummarizer);
        let registry = Arc::new(LoopRegistry::new());
        let cancellation = duga_sandbox::CancellationToken::new();

        let (sender, receiver) = make_steer_channel();
        // Inject Cancel — will be picked up at POINT 2 after tool result
        sender
            .inject(SteeringEvent::Control(SteeringControlEvent::Cancel {
                reason: "user stopped".into(),
            }))
            .unwrap();

        let mut ctx = make_ctx(
            &config, &mut memory, &llm, &tools, &workspace,
            &sink, &summarizer, &registry, &cancellation,
        );
        ctx.steer = Some(receiver);

        let loop_impl = SimpleReActLoop;
        let err = loop_impl.run("task".into(), &mut ctx).await.unwrap_err();
        assert_eq!(err, AgentError::Cancelled);
    }
}
