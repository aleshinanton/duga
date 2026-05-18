//! Core agent loop orchestration.

use crate::memory::Memory;
use crate::summarizer::Summarizer;
use duga_events::{Event, EventSink};
use duga_llm::LlmClient;
use duga_sandbox::{CancellationToken, Workspace};
use duga_tools::ToolDispatcher;
use duga_types::config::AgentConfig;
use duga_types::error::{AgentError, ToolError};
use duga_types::llm::LlmCallOptions;
use duga_types::message::AssistantMessage;
use duga_types::tool_call::ToolCall;
use duga_types::tool_result::ToolResult;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub struct AgentRunResult {
    pub message: AssistantMessage,
    pub steps: u32,
    pub tool_calls: u32,
}

pub struct AgentLoop {
    config: AgentConfig,
    memory: Memory,
    summarizer: Arc<dyn Summarizer>,
    llm: Arc<dyn LlmClient>,
    tools: Arc<ToolDispatcher>,
    workspace: Workspace,
    event_sink: Arc<dyn EventSink>,
}

impl AgentLoop {
    pub fn new(
        config: AgentConfig,
        memory: Memory,
        summarizer: Arc<dyn Summarizer>,
        llm: Arc<dyn LlmClient>,
        tools: Arc<ToolDispatcher>,
        workspace: Workspace,
        event_sink: Arc<dyn EventSink>,
    ) -> Self {
        Self {
            config,
            memory,
            summarizer,
            llm,
            tools,
            workspace,
            event_sink,
        }
    }

    pub fn memory(&self) -> &Memory {
        &self.memory
    }

    pub async fn run(
        &mut self,
        task: impl Into<String>,
        cancellation: CancellationToken,
    ) -> Result<AgentRunResult, AgentError> {
        let task = task.into();
        let started = Instant::now();
        let mut tool_calls = 0;

        tracing::info!(task = %task, "Agent run started");
        self.tools.reset_limits();
        self.emit(Event::AgentStarted { task: task.clone() })
            .await?;
        self.memory.push_user(task);

        for step in 1..=self.config.limits.max_steps {
            if let Err(error) = self.check_limits(started, &cancellation) {
                return self.fatal(error).await;
            }
            if let Err(error) = self.compress_if_needed().await {
                return self.fatal(error).await;
            }

            let memory_tokens = self.memory.token_count(self.llm.as_ref());
            tracing::info!(
                step,
                tool_calls_count = tool_calls,
                tokens_used = memory_tokens,
                duration_ms = started.elapsed().as_millis(),
                "Loop iteration"
            );
            self.emit(Event::StepStarted { step }).await?;

            let messages = self.memory.messages();
            let schemas = self.tools.schemas();
            self.emit(Event::LlmRequest {
                model: self.llm.model().into(),
                messages: messages.clone(),
                tools: schemas.clone(),
            })
            .await?;

            let llm_started = Instant::now();
            let response = match self
                .call_llm(&messages, &schemas, started, &cancellation)
                .await
            {
                Ok(response) => response,
                Err(error) => return self.fatal(error).await,
            };
            tracing::info!(
                llm_latency_ms = llm_started.elapsed().as_millis(),
                prompt_tokens = response.usage.prompt,
                completion_tokens = response.usage.completion,
                "LLM response"
            );
            let assistant = response.message;
            self.emit(Event::LlmResponse {
                model: self.llm.model().into(),
                text: assistant.text.clone(),
                tool_calls: assistant.tool_calls.clone(),
            })
            .await?;
            self.memory.push_assistant(assistant.clone());

            if assistant.is_termination() {
                self.emit(Event::StepFinished { step }).await?;
                self.emit(Event::AgentFinished {
                    text: assistant.text.clone(),
                })
                .await?;
                tracing::info!(
                    result = %assistant.text.clone().unwrap_or_default(),
                    duration_ms = started.elapsed().as_millis(),
                    "Agent finished"
                );
                return Ok(AgentRunResult {
                    message: assistant,
                    steps: step,
                    tool_calls,
                });
            }

            for call in &assistant.tool_calls {
                if tool_calls >= self.config.limits.max_tool_calls {
                    return self
                        .fatal(AgentError::MaxToolCallsReached {
                            limit: self.config.limits.max_tool_calls,
                        })
                        .await;
                }
                tool_calls += 1;

                let result = match self.execute_tool(call, started, &cancellation).await {
                    Ok(result) => result,
                    Err(error) => return self.fatal(error).await,
                };
                self.memory.push_tool_result(result);
                if let Err(error) = self.check_limits(started, &cancellation) {
                    return self.fatal(error).await;
                }
            }

            if let Err(error) = self.compress_if_needed().await {
                return self.fatal(error).await;
            }
            self.emit(Event::StepFinished { step }).await?;
        }

        self.fatal(AgentError::MaxStepsReached {
            limit: self.config.limits.max_steps,
        })
        .await
    }

    async fn call_llm(
        &self,
        messages: &[duga_types::message::Message],
        tools: &[duga_types::tool_schema::ToolSchema],
        started: Instant,
        cancellation: &CancellationToken,
    ) -> Result<duga_types::llm::LlmResponse, AgentError> {
        if cancellation.is_cancelled() {
            return Err(AgentError::Cancelled);
        }

        let remaining = self.remaining_runtime(started)?;
        let mut cancelled = cancellation.subscribe();
        let call = self.llm.chat(
            messages,
            tools,
            LlmCallOptions {
                streaming: self.config.features.streaming,
            },
            self.event_sink.as_ref(),
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
        &self,
        call: &ToolCall,
        loop_started: Instant,
        cancellation: &CancellationToken,
    ) -> Result<ToolResult, AgentError> {
        let retryable = self
            .tools
            .get(&call.tool)
            .map(|tool| tool.retryable)
            .unwrap_or(false);
        let span = tracing::info_span!("tool_call", tool = %call.tool, id = %call.id);
        let _entered = span.enter();
        let max_attempts = self.config.limits.retry_on_error + 1;
        let mut last_error = None;

        for attempt in 1..=max_attempts {
            self.check_limits(loop_started, cancellation)?;
            self.emit(Event::ToolCallStarted {
                tool_call: call.clone(),
                attempt,
            })
            .await?;

            let started = Instant::now();
            let remaining = self.remaining_runtime(loop_started)?;
            let dispatch = self.tools.dispatch(
                call,
                &self.workspace,
                cancellation.clone(),
                self.event_sink.as_ref(),
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
                    self.emit(Event::ToolCallFinished {
                        result: result.clone(),
                        attempt,
                        tool_name: call.tool.clone(),
                    })
                    .await?;
                    return Ok(result);
                }
                Err(error) if attempt < max_attempts && retryable && error.is_transient() => {
                    let result = tool_error_result(call, error.clone(), started);
                    tracing::info!(
                        tool_duration_ms = result.duration_ms,
                        tool_success = false,
                        "Tool retry scheduled"
                    );
                    self.emit(Event::ToolCallFinished {
                        result,
                        attempt,
                        tool_name: call.tool.clone(),
                    })
                        .await?;
                    last_error = Some(error);
                }
                Err(error) => {
                    let result = tool_error_result(call, error, started);
                    tracing::info!(
                        tool_duration_ms = result.duration_ms,
                        tool_success = false,
                        "Tool finished"
                    );
                    self.emit(Event::ToolCallFinished {
                        result: result.clone(),
                        attempt,
                        tool_name: call.tool.clone(),
                    })
                    .await?;
                    return Ok(result);
                }
            }
        }

        let result = tool_error_result(
            call,
            last_error.unwrap_or_else(|| ToolError::Plugin("retry attempts exhausted".into())),
            Instant::now(),
        );
        self.emit(Event::ToolCallFinished {
            result: result.clone(),
            attempt: max_attempts,
            tool_name: call.tool.clone(),
        })
        .await?;
        Ok(result)
    }

    async fn compress_if_needed(&mut self) -> Result<(), AgentError> {
        if !self.memory.over_budget(self.llm.as_ref()) {
            return Ok(());
        }

        let before = self.memory.token_count(self.llm.as_ref());
        tracing::info!(
            memory_tokens = before,
            max_tokens = self.memory.max_tokens(),
            "Memory budget check"
        );
        self.memory
            .compress(self.summarizer.as_ref(), self.llm.as_ref())
            .await?;
        let after = self.memory.token_count(self.llm.as_ref());
        tracing::info!(
            before_tokens = before,
            after_tokens = after,
            "Memory compressed"
        );
        self.emit(Event::MemoryCompressed {
            before_tokens: before,
            after_tokens: after,
        })
        .await
    }

    fn check_limits(
        &self,
        started: Instant,
        cancellation: &CancellationToken,
    ) -> Result<(), AgentError> {
        if cancellation.is_cancelled() {
            return Err(AgentError::Cancelled);
        }
        self.remaining_runtime(started).map(|_| ())
    }

    fn remaining_runtime(&self, started: Instant) -> Result<Duration, AgentError> {
        self.config
            .limits
            .max_runtime
            .checked_sub(started.elapsed())
            .filter(|remaining| !remaining.is_zero())
            .ok_or(AgentError::Timeout)
    }

    async fn fatal<T>(&self, error: AgentError) -> Result<T, AgentError> {
        self.emit(Event::Error {
            message: error.to_string(),
        })
        .await?;
        Err(error)
    }

    async fn emit(&self, event: Event) -> Result<(), AgentError> {
        self.event_sink
            .emit(event)
            .await
            .map_err(|error| AgentError::EventSinkFailed(error.to_string()))
    }
}

fn tool_error_result(call: &ToolCall, error: ToolError, started: Instant) -> ToolResult {
    ToolResult::from_outcome(call.id.clone(), &call.tool, Err(error), started)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Summarizer, SummaryFuture};
    use duga_events::{EventFuture, NullSink};
    use duga_llm::dummy::DummyClient;
    use duga_tools::{ErasedTool, Tool, ToolContext};
    use duga_types::llm::{LlmResponse, SummaryMessage, TokenUsage};
    use duga_types::message::{ContentBlock, Message};
    use duga_types::tool_result::ToolResultBuilder;
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};
    use std::sync::Mutex;

    struct StaticSummarizer;

    impl Summarizer for StaticSummarizer {
        fn summarize<'a>(&'a self, _messages: &'a [Message]) -> SummaryFuture<'a> {
            Box::pin(async { Ok(SummaryMessage::new("summary".into())) })
        }
    }

    #[derive(Default)]
    struct RecordingSink {
        events: Mutex<Vec<Event>>,
    }

    impl RecordingSink {
        fn events(&self) -> Vec<Event> {
            self.events.lock().unwrap().clone()
        }
    }

    impl EventSink for RecordingSink {
        fn name(&self) -> &str {
            "recording"
        }

        fn emit<'a>(&'a self, event: Event) -> EventFuture<'a> {
            Box::pin(async move {
                self.events.lock().unwrap().push(event);
                Ok(())
            })
        }
    }

    #[derive(Debug, Deserialize, Serialize, JsonSchema)]
    struct EchoArgs {
        msg: String,
    }

    struct EchoTool;

    impl Tool for EchoTool {
        type Args = EchoArgs;

        fn name(&self) -> &str {
            "echo"
        }

        fn description(&self) -> &str {
            "echo a message"
        }

        async fn execute(
            &self,
            _ctx: ToolContext<'_>,
            args: Self::Args,
        ) -> duga_tools::result::ToolCallResult {
            Ok(ToolResultBuilder::new()
                .tool_call_id(duga_types::tool_call::CallId::new())
                .success(true)
                .output(args.msg)
                .build()
                .unwrap())
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

    fn loop_with(
        llm: Arc<dyn LlmClient>,
        sink: Arc<dyn EventSink>,
        tools: Arc<ToolDispatcher>,
    ) -> (AgentLoop, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let workspace = Workspace::open(dir.path()).unwrap();
        let memory = Memory::new(vec![Message::system("system")], 10_000, 0.8);
        (
            AgentLoop::new(
                AgentConfig::default(),
                memory,
                Arc::new(StaticSummarizer),
                llm,
                tools,
                workspace,
                sink,
            ),
            dir,
        )
    }

    #[tokio::test]
    async fn run_returns_terminal_assistant_message() {
        let llm = Arc::new(DummyClient::with_response(
            "dummy",
            llm_response(AssistantMessage {
                text: Some("done".into()),
                tool_calls: vec![],
            }),
        ));
        let sink = Arc::new(RecordingSink::default());
        let tools = Arc::new(ToolDispatcher::new());
        let (mut agent, _dir) = loop_with(llm, sink.clone(), tools);

        let result = agent.run("task", CancellationToken::new()).await.unwrap();

        assert_eq!(result.message.text, Some("done".into()));
        assert_eq!(result.steps, 1);
        assert!(matches!(sink.events()[0], Event::AgentStarted { .. }));
    }

    #[tokio::test]
    async fn run_executes_tools_and_feeds_results_to_next_step() {
        let call = ToolCall::new("echo", serde_json::json!({"msg": "hello"}));
        let llm = Arc::new(DummyClient::default());
        llm.push_response(llm_response(AssistantMessage {
            text: None,
            tool_calls: vec![call.clone()],
        }));
        llm.push_response(llm_response(AssistantMessage {
            text: Some("done".into()),
            tool_calls: vec![],
        }));
        let tools = Arc::new(ToolDispatcher::new());
        tools.register_erased(ErasedTool::erase(EchoTool)).unwrap();
        let (mut agent, _dir) = loop_with(llm, Arc::new(NullSink), tools);

        let result = agent.run("task", CancellationToken::new()).await.unwrap();

        assert_eq!(result.steps, 2);
        assert_eq!(result.tool_calls, 1);
        assert!(agent
            .memory()
            .messages()
            .iter()
            .any(|message| matches!(message.content.first(), Some(ContentBlock::Text { text }) if text == "hello")));
    }

    #[tokio::test]
    async fn cancellation_before_run_is_fatal() {
        let llm = Arc::new(DummyClient::default());
        let tools = Arc::new(ToolDispatcher::new());
        let (mut agent, _dir) = loop_with(llm, Arc::new(NullSink), tools);
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let err = agent.run("task", cancellation).await.unwrap_err();

        assert_eq!(err, AgentError::Cancelled);
    }
}
