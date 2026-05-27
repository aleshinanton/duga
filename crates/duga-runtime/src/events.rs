//! Non-blocking frontend event bridge.
//!
//! Translates internal `Event` objects into lightweight `FrontendEvent`
//! values sent over a bounded mpsc channel. The bridge never blocks the
//! agent loop: if the channel is full, events are dropped and logged.

use duga_events::{Event, EventSink, EventError};
use std::collections::HashMap;
use std::sync::Mutex;
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
    /// Execution was handed to a specialized loop.
    LoopDelegated {
        from: String,
        to: String,
        reason: String,
        depth: u32,
    },
    /// The agent's memory was compressed.
    MemoryCompressed {
        before_tokens: usize,
        after_tokens: usize,
    },
}

/// An `EventSink` that translates internal events to `FrontendEvent` and
/// pushes them to a bounded channel. It never fails the agent run.
///
/// Maintains an internal cache of tool_call_id → description so that
/// `ToolCallFinished` events can carry the human-readable label from the
/// corresponding `ToolCallStarted`.
pub struct FrontendEventSink {
    tx: mpsc::Sender<FrontendEvent>,
    name: String,
    /// Cache: tool_call_id → description (populated by ToolCallStarted, consumed by ToolCallFinished).
    descriptions: Mutex<HashMap<String, String>>,
}

impl FrontendEventSink {
    pub fn new(tx: mpsc::Sender<FrontendEvent>) -> Self {
        Self {
            tx,
            name: "frontend".into(),
            descriptions: Mutex::new(HashMap::new()),
        }
    }

    pub fn with_name(tx: mpsc::Sender<FrontendEvent>, name: impl Into<String>) -> Self {
        Self {
            tx,
            name: name.into(),
            descriptions: Mutex::new(HashMap::new()),
        }
    }
}

impl EventSink for FrontendEventSink {
    fn name(&self) -> &str {
        &self.name
    }

    fn emit<'a>(&'a self, event: Event) -> duga_events::EventFuture<'a> {
        Box::pin(async move {
            let frontend = self.map_event(event);
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

impl FrontendEventSink {
    /// Map internal event to frontend event, maintaining a description cache
    /// so `ToolCallFinished` carries the label from the original `ToolCallStarted`.
    fn map_event(&self, event: Event) -> Option<FrontendEvent> {
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
                let call_id = tool_call.id.to_string();
                // Cache description for the ToolCallFinished event.
                if let Ok(mut map) = self.descriptions.lock() {
                    map.insert(call_id.clone(), description.clone());
                }
                Some(FrontendEvent::ToolCallStarted {
                    tool_name: tool_call.tool,
                    tool_call_id: call_id,
                    attempt,
                    description,
                })
            }
            Event::ToolCallFinished {
                result,
                attempt,
                tool_name,
            } => {
                let call_id = result.tool_call_id.to_string();
                // Look up the cached description; fall back to tool_name.
                let description = self
                    .descriptions
                    .lock()
                    .ok()
                    .and_then(|mut map| map.remove(&call_id))
                    .unwrap_or_else(|| tool_name.clone());
                Some(FrontendEvent::ToolCallFinished {
                    tool_name: tool_name.clone(),
                    tool_call_id: call_id,
                    success: result.success,
                    attempt,
                    description,
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
            Event::LoopDelegated {
                from,
                to,
                reason,
                depth,
            } => Some(FrontendEvent::LoopDelegated {
                from,
                to,
                reason,
                depth,
            }),
            // Suppress high-volume internal events.
            Event::StepStarted { .. }
            | Event::StepFinished { .. }
            | Event::LlmRequest { .. }
            | Event::LlmResponse { .. }
            | Event::SteeringApplied { .. } => None,
        }
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

    /// Helper: create a fresh sink for testing map_event.
    fn test_sink() -> FrontendEventSink {
        let (tx, _rx) = mpsc::channel(16);
        FrontendEventSink::new(tx)
    }

    #[test]
    fn map_event_agent_started() {
        let sink = test_sink();
        let event = Event::AgentStarted {
            task: "do thing".into(),
        };
        match sink.map_event(event) {
            Some(FrontendEvent::RunStarted { task }) => assert_eq!(task, "do thing"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn map_event_tool_call() {
        let sink = test_sink();
        let call = ToolCall::new("shell", serde_json::json!({"cmd": "ls", "label": "ls -la"}));
        let event = Event::ToolCallStarted {
            tool_call: call,
            attempt: 1,
        };
        match sink.map_event(event) {
            Some(FrontendEvent::ToolCallStarted {
                tool_name,
                attempt,
                description,
                ..
            }) => {
                assert_eq!(tool_name, "shell");
                assert_eq!(attempt, 1);
                assert_eq!(description, "ls -la");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn map_event_tool_call_no_label() {
        let sink = test_sink();
        // When the LLM doesn't supply a label, description falls back to tool_name.
        let call = ToolCall::new("shell", serde_json::json!({"cmd": "ls"}));
        let event = Event::ToolCallStarted {
            tool_call: call,
            attempt: 1,
        };
        match sink.map_event(event) {
            Some(FrontendEvent::ToolCallStarted {
                tool_name,
                description,
                ..
            }) => {
                assert_eq!(tool_name, "shell");
                assert_eq!(description, "shell");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn map_event_step_suppressed() {
        let sink = test_sink();
        assert!(sink.map_event(Event::StepStarted { step: 1 }).is_none());
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
    fn map_event_tool_call_finished_no_cached_description() {
        // When no ToolCallStarted was processed (cold sink), description falls back to tool_name.
        let sink = test_sink();
        let result = duga_types::tool_result::ToolResult {
            tool_call_id: duga_types::tool_call::CallId::new(),
            success: true,
            output: "ok".into(),
            metadata: serde_json::json!({}),
            duration_ms: 0,
            stdout_bytes: 0,
            stderr_bytes: 0,
            truncated: false,
            steering_hint: None,
        };
        let event = Event::ToolCallFinished {
            result,
            attempt: 1,
            tool_name: "shell".into(),
        };
        match sink.map_event(event) {
            Some(FrontendEvent::ToolCallFinished {
                tool_name,
                success,
                attempt,
                description,
                ..
            }) => {
                assert_eq!(tool_name, "shell");
                assert!(success);
                assert_eq!(attempt, 1);
                // No cached description, should fall back to tool_name
                assert_eq!(description, "shell");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn map_event_tool_call_finished_uses_cached_description() {
        // ToolCallStarted caches the description; ToolCallFinished retrieves it.
        let sink = test_sink();
        let call_id = duga_types::tool_call::CallId::new();

        // First, a ToolCallStarted with a label
        let call = ToolCall {
            id: call_id.clone(),
            tool: "shell".into(),
            raw_args: serde_json::json!({"cmd": "ls", "label": "list files"}),
        };
        let started = sink.map_event(Event::ToolCallStarted {
            tool_call: call,
            attempt: 1,
        });
        assert!(started.is_some());

        // Then a ToolCallFinished with the same call_id
        let result = duga_types::tool_result::ToolResult {
            tool_call_id: call_id.clone(),
            success: true,
            output: "ok".into(),
            metadata: serde_json::json!({}),
            duration_ms: 0,
            stdout_bytes: 0,
            stderr_bytes: 0,
            truncated: false,
            steering_hint: None,
        };
        let finished = sink.map_event(Event::ToolCallFinished {
            result,
            attempt: 1,
            tool_name: "shell".into(),
        });
        match finished {
            Some(FrontendEvent::ToolCallFinished { description, .. }) => {
                assert_eq!(description, "list files",
                    "description should come from ToolCallStarted cache");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn map_event_tool_call_started_label_is_empty_string() {
        let sink = test_sink();
        // When label is present but empty, description should be empty string.
        let call = ToolCall::new("shell", serde_json::json!({"label": ""}));
        let event = Event::ToolCallStarted {
            tool_call: call,
            attempt: 1,
        };
        match sink.map_event(event) {
            Some(FrontendEvent::ToolCallStarted {
                tool_name,
                description,
                ..
            }) => {
                assert_eq!(tool_name, "shell");
                assert_eq!(description, "");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn map_event_tool_call_started_label_is_non_string() {
        let sink = test_sink();
        // When label is not a string (e.g., JSON number), fall back to tool_name.
        let call = ToolCall::new("shell", serde_json::json!({"label": 42}));
        let event = Event::ToolCallStarted {
            tool_call: call,
            attempt: 1,
        };
        match sink.map_event(event) {
            Some(FrontendEvent::ToolCallStarted {
                tool_name,
                description,
                ..
            }) => {
                assert_eq!(tool_name, "shell");
                assert_eq!(description, "shell");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn map_event_tool_call_finished_preserves_tool_name_from_event() {
        let sink = test_sink();
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
            steering_hint: None,
        };
        let event = Event::ToolCallFinished {
            result,
            attempt: 3,
            tool_name: "write".into(),
        };
        match sink.map_event(event) {
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
            steering_hint: None,
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
