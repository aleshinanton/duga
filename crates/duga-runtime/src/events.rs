//! Non-blocking frontend event bridge.
//!
//! Translates internal `Event` objects into lightweight `FrontendEvent`
//! values sent over a bounded mpsc channel. The bridge never blocks the
//! agent loop: if the channel is full, events are dropped and logged.

use duga_events::{Event, EventSink, EventError};
use tokio::sync::mpsc;

/// Lightweight frontend event with only the fields needed for UI rendering.
/// This avoids passing full `Event` payloads into frontend workers.
#[derive(Clone, Debug)]
pub enum FrontendEvent {
    /// A new agent run has started.
    RunStarted {
        task: String,
    },
    /// The agent has produced a final answer.
    RunFinished {
        text: Option<String>,
    },
    /// A tool call is about to be executed.
    ToolCallStarted {
        tool_name: String,
        tool_call_id: String,
        attempt: u32,
        /// Human-readable description from the LLM's `label` arg,
        /// or falls back to tool_name if absent.
        description: String,
    },
    /// A tool call has completed.
    ToolCallFinished {
        tool_name: String,
        tool_call_id: String,
        success: bool,
        attempt: u32,
        /// Human-readable description from the LLM's `label` arg,
        /// or falls back to tool_name if absent.
        description: String,
    },
    /// A partial token delta from the LLM.
    LlmTokenDelta {
        model: String,
        delta: String,
    },
    /// The agent encountered an error.
    Error {
        message: String,
    },
    /// The agent's memory was compressed.
    MemoryCompressed {
        before_tokens: usize,
        after_tokens: usize,
    },
}

/// An `EventSink` that translates internal events to `FrontendEvent` and
/// pushes them to a bounded channel. It never fails the agent run.
pub struct FrontendEventSink {
    tx: mpsc::Sender<FrontendEvent>,
    name: String,
}

impl FrontendEventSink {
    pub fn new(tx: mpsc::Sender<FrontendEvent>) -> Self {
        Self {
            tx,
            name: "frontend".into(),
        }
    }

    pub fn with_name(tx: mpsc::Sender<FrontendEvent>, name: impl Into<String>) -> Self {
        Self {
            tx,
            name: name.into(),
        }
    }
}

impl EventSink for FrontendEventSink {
    fn name(&self) -> &str {
        &self.name
    }

    fn emit<'a>(&'a self, event: Event) -> duga_events::EventFuture<'a> {
        Box::pin(async move {
            let frontend = map_event(event);
            if let Some(fe) = frontend {
                match self.tx.try_send(fe.clone()) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        tracing::warn!("frontend event channel full, dropping event");
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        return Err(EventError::Sink {
                            sink: self.name.clone(),
                            message: "frontend event channel closed".into(),
                        });
                    }
                }
            }
            Ok(())
        })
    }
}

/// Map internal event to frontend event (best-effort).
fn map_event(event: Event) -> Option<FrontendEvent> {
    match event {
        Event::AgentStarted { task } => Some(FrontendEvent::RunStarted { task }),
        Event::AgentFinished { text } => Some(FrontendEvent::RunFinished { text }),
        Event::ToolCallStarted { tool_call, attempt } => {
            let description = tool_call
                .raw_args
                .get("label")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| tool_call.tool.clone());
            Some(FrontendEvent::ToolCallStarted {
                tool_name: tool_call.tool,
                tool_call_id: tool_call.id.to_string(),
                attempt,
                description,
            })
        }
        Event::ToolCallFinished {
            result,
            attempt,
            tool_name,
        } => {
            Some(FrontendEvent::ToolCallFinished {
                tool_name: tool_name.clone(),
                tool_call_id: result.tool_call_id.to_string(),
                success: result.success,
                attempt,
                description: String::new(),
            })
        }
        Event::LlmTokenDelta { model, delta } => Some(FrontendEvent::LlmTokenDelta { model, delta }),
        Event::Error { message } => Some(FrontendEvent::Error { message }),
        Event::MemoryCompressed {
            before_tokens,
            after_tokens,
        } => Some(FrontendEvent::MemoryCompressed {
            before_tokens,
            after_tokens,
        }),
        // Suppress high-volume internal events.
        Event::StepStarted { .. }
        | Event::StepFinished { .. }
        | Event::LlmRequest { .. }
        | Event::LlmResponse { .. } => None,
    }
}

/// Bridge between runtime event stream and frontend renderers.
pub struct FrontendEventBridge {
    rx: mpsc::Receiver<FrontendEvent>,
}

impl FrontendEventBridge {
    /// Create a new bridge and return the sending half to be wired
    /// into the runtime event sink.
    pub fn new(buffer: usize) -> (mpsc::Sender<FrontendEvent>, Self) {
        let (tx, rx) = mpsc::channel(buffer);
        (tx, Self { rx })
    }

    /// Receive the next frontend event (or `None` if the sender was dropped).
    pub async fn recv(&mut self) -> Option<FrontendEvent> {
        self.rx.recv().await
    }

    /// Extract the inner receiver for direct use with a renderer.
    pub fn into_inner(self) -> mpsc::Receiver<FrontendEvent> {
        self.rx
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use duga_types::tool_call::ToolCall;

    #[test]
    fn map_event_agent_started() {
        let event = Event::AgentStarted {
            task: "do thing".into(),
        };
        match map_event(event) {
            Some(FrontendEvent::RunStarted { task }) => assert_eq!(task, "do thing"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn map_event_tool_call() {
        let call = ToolCall::new("bash", serde_json::json!({"cmd": "ls", "label": "ls -la"}));
        let event = Event::ToolCallStarted {
            tool_call: call,
            attempt: 1,
        };
        match map_event(event) {
            Some(FrontendEvent::ToolCallStarted {
                tool_name,
                attempt,
                description,
                ..
            }) => {
                assert_eq!(tool_name, "bash");
                assert_eq!(attempt, 1);
                assert_eq!(description, "ls -la");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn map_event_tool_call_no_label() {
        // When the LLM doesn't supply a label, description falls back to tool_name.
        let call = ToolCall::new("bash", serde_json::json!({"cmd": "ls"}));
        let event = Event::ToolCallStarted {
            tool_call: call,
            attempt: 1,
        };
        match map_event(event) {
            Some(FrontendEvent::ToolCallStarted {
                tool_name,
                description,
                ..
            }) => {
                assert_eq!(tool_name, "bash");
                assert_eq!(description, "bash");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn map_event_step_suppressed() {
        assert!(map_event(Event::StepStarted { step: 1 }).is_none());
    }

    #[tokio::test]
    async fn bridge_send_recv() {
        let (tx, mut bridge) = FrontendEventBridge::new(32);
        tx.send(FrontendEvent::RunStarted {
            task: "test".into(),
        })
        .await
        .unwrap();
        let event = bridge.recv().await.unwrap();
        match event {
            FrontendEvent::RunStarted { task } => assert_eq!(task, "test"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn map_event_tool_call_finished() {
        let result = duga_types::tool_result::ToolResult {
            tool_call_id: duga_types::tool_call::CallId::new(),
            success: true,
            output: "ok".into(),
            metadata: serde_json::json!({}),
            duration_ms: 0,
            stdout_bytes: 0,
            stderr_bytes: 0,
            truncated: false,
        };
        let event = Event::ToolCallFinished {
            result,
            attempt: 1,
            tool_name: "bash".into(),
        };
        match map_event(event) {
            Some(FrontendEvent::ToolCallFinished {
                tool_name,
                success,
                attempt,
                ..
            }) => {
                assert_eq!(tool_name, "bash");
                assert!(success);
                assert_eq!(attempt, 1);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn map_event_tool_call_started_label_is_empty_string() {
        // When label is present but empty, description should be empty string.
        let call = ToolCall::new("bash", serde_json::json!({"label": ""}));
        let event = Event::ToolCallStarted {
            tool_call: call,
            attempt: 1,
        };
        match map_event(event) {
            Some(FrontendEvent::ToolCallStarted {
                tool_name,
                description,
                ..
            }) => {
                assert_eq!(tool_name, "bash");
                assert_eq!(description, "");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn map_event_tool_call_started_label_is_non_string() {
        // When label is not a string (e.g., JSON number), fall back to tool_name.
        let call = ToolCall::new("bash", serde_json::json!({"label": 42}));
        let event = Event::ToolCallStarted {
            tool_call: call,
            attempt: 1,
        };
        match map_event(event) {
            Some(FrontendEvent::ToolCallStarted {
                tool_name,
                description,
                ..
            }) => {
                assert_eq!(tool_name, "bash");
                assert_eq!(description, "bash");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn map_event_tool_call_finished_preserves_tool_name_from_event() {
        // Verify that the tool_name from Event::ToolCallFinished is carried through.
        let result = duga_types::tool_result::ToolResult {
            tool_call_id: duga_types::tool_call::CallId::new(),
            success: false,
            output: "failed".into(),
            metadata: serde_json::json!({}),
            duration_ms: 5,
            stdout_bytes: 0,
            stderr_bytes: 0,
            truncated: false,
        };
        let event = Event::ToolCallFinished {
            result,
            attempt: 3,
            tool_name: "write".into(),
        };
        match map_event(event) {
            Some(FrontendEvent::ToolCallFinished {
                tool_name,
                success,
                attempt,
                ..
            }) => {
                assert_eq!(tool_name, "write");
                assert!(!success);
                assert_eq!(attempt, 3);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn event_tool_call_finished_roundtrip_preserves_tool_name() {
        // Verify Event::ToolCallFinished serializes/deserializes with tool_name.
        let result = duga_types::tool_result::ToolResult {
            tool_call_id: duga_types::tool_call::CallId::new(),
            success: true,
            output: "ok".into(),
            metadata: serde_json::json!({}),
            duration_ms: 0,
            stdout_bytes: 0,
            stderr_bytes: 0,
            truncated: false,
        };
        let event = Event::ToolCallFinished {
            result,
            attempt: 2,
            tool_name: "search".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let decoded: Event = serde_json::from_str(&json).unwrap();
        match decoded {
            Event::ToolCallFinished { tool_name, attempt, .. } => {
                assert_eq!(tool_name, "search");
                assert_eq!(attempt, 2);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
}
