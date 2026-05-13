//! Event types and sinks shared by runtime crates.

use duga_types::message::Message;
use duga_types::tool_call::ToolCall;
use duga_types::tool_result::ToolResult;
use duga_types::tool_schema::ToolSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{File, OpenOptions};
use std::future::Future;
use std::io::Write;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

pub type EventFuture<'a> = Pin<Box<dyn Future<Output = Result<(), EventError>> + Send + 'a>>;

#[derive(Debug, thiserror::Error)]
pub enum EventError {
    #[error("event io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("event serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("event sink '{sink}' failed: {message}")]
    Sink { sink: String, message: String },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    AgentStarted {
        task: String,
    },
    AgentFinished {
        text: Option<String>,
    },
    StepStarted {
        step: u32,
    },
    StepFinished {
        step: u32,
    },
    LlmRequest {
        model: String,
        messages: Vec<Message>,
        tools: Vec<ToolSchema>,
    },
    LlmResponse {
        model: String,
        text: Option<String>,
        tool_calls: Vec<ToolCall>,
    },
    ToolCallStarted {
        tool_call: ToolCall,
        attempt: u32,
    },
    ToolCallFinished {
        result: ToolResult,
        attempt: u32,
    },
    MemoryCompressed {
        before_tokens: usize,
        after_tokens: usize,
    },
    Error {
        message: String,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct StoredEvent {
    pub seq: u64,
    pub timestamp_ms: u128,
    #[serde(flatten)]
    pub event: Event,
}

impl StoredEvent {
    pub fn new(seq: u64, event: Event) -> Self {
        Self {
            seq,
            timestamp_ms: now_ms(),
            event,
        }
    }
}

pub trait EventSink: Send + Sync {
    fn name(&self) -> &str {
        "anonymous"
    }

    fn emit<'a>(&'a self, _event: Event) -> EventFuture<'a> {
        Box::pin(async { Ok(()) })
    }
}

#[derive(Debug, Default)]
pub struct NullSink;

impl EventSink for NullSink {
    fn name(&self) -> &str {
        "null"
    }
}

#[derive(Debug, Default)]
pub struct SeqAllocator {
    next: AtomicU64,
}

impl SeqAllocator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn next(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed) + 1
    }
}

pub struct JsonlSink {
    name: String,
    seq: SeqAllocator,
    file: Mutex<File>,
}

impl JsonlSink {
    pub fn new(path: impl AsRef<Path>) -> Result<Self, EventError> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            name: "jsonl".into(),
            seq: SeqAllocator::new(),
            file: Mutex::new(file),
        })
    }

    pub fn with_name(path: impl AsRef<Path>, name: impl Into<String>) -> Result<Self, EventError> {
        let mut sink = Self::new(path)?;
        sink.name = name.into();
        Ok(sink)
    }
}

impl EventSink for JsonlSink {
    fn name(&self) -> &str {
        &self.name
    }

    fn emit<'a>(&'a self, event: Event) -> EventFuture<'a> {
        Box::pin(async move {
            let stored = StoredEvent::new(self.seq.next(), event);
            let line = serde_json::to_string(&stored)?;
            let mut file = self.file.lock().map_err(|_| EventError::Sink {
                sink: self.name.clone(),
                message: "event file lock poisoned".into(),
            })?;
            writeln!(file, "{line}")?;
            file.flush()?;
            Ok(())
        })
    }
}

#[derive(Default)]
pub struct MultiSink {
    sinks: Vec<Arc<dyn EventSink>>,
}

impl MultiSink {
    pub fn new(sinks: Vec<Arc<dyn EventSink>>) -> Self {
        Self { sinks }
    }

    pub fn push(&mut self, sink: Arc<dyn EventSink>) {
        self.sinks.push(sink);
    }

    pub fn is_empty(&self) -> bool {
        self.sinks.is_empty()
    }
}

impl EventSink for MultiSink {
    fn name(&self) -> &str {
        "multi"
    }

    fn emit<'a>(&'a self, event: Event) -> EventFuture<'a> {
        Box::pin(async move {
            for sink in &self.sinks {
                sink.emit(event.clone())
                    .await
                    .map_err(|error| EventError::Sink {
                        sink: sink.name().into(),
                        message: error.to_string(),
                    })?;
            }
            Ok(())
        })
    }
}

#[derive(Clone, Debug)]
pub struct Redactor {
    sensitive_keys: Vec<String>,
    replacement: String,
}

impl Default for Redactor {
    fn default() -> Self {
        Self::new([
            "api_key",
            "authorization",
            "auth",
            "credential",
            "password",
            "secret",
            "token",
        ])
    }
}

impl Redactor {
    pub fn new<I, S>(keys: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            sensitive_keys: keys
                .into_iter()
                .map(|key| key.into().to_lowercase())
                .collect(),
            replacement: "[REDACTED]".into(),
        }
    }

    pub fn redact_value(&self, value: &mut Value) {
        match value {
            Value::Object(map) => {
                for (key, value) in map {
                    if self.is_sensitive_key(key) {
                        *value = Value::String(self.replacement.clone());
                    } else {
                        self.redact_value(value);
                    }
                }
            }
            Value::Array(values) => {
                for value in values {
                    self.redact_value(value);
                }
            }
            _ => {}
        }
    }

    pub fn redact_event(&self, event: Event) -> Result<Event, EventError> {
        let mut value = serde_json::to_value(event)?;
        self.redact_value(&mut value);
        Ok(serde_json::from_value(value)?)
    }

    fn is_sensitive_key(&self, key: &str) -> bool {
        let key = key.to_lowercase();
        self.sensitive_keys
            .iter()
            .any(|sensitive| key.contains(sensitive))
    }
}

pub struct RedactingSink {
    inner: Arc<dyn EventSink>,
    redactor: Redactor,
}

impl RedactingSink {
    pub fn new(inner: Arc<dyn EventSink>) -> Self {
        Self {
            inner,
            redactor: Redactor::default(),
        }
    }

    pub fn with_redactor(inner: Arc<dyn EventSink>, redactor: Redactor) -> Self {
        Self { inner, redactor }
    }
}

impl EventSink for RedactingSink {
    fn name(&self) -> &str {
        "redacting"
    }

    fn emit<'a>(&'a self, event: Event) -> EventFuture<'a> {
        Box::pin(async move {
            let event = self.redactor.redact_event(event)?;
            self.inner.emit(event).await
        })
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use duga_types::message::AssistantMessage;
    use duga_types::tool_call::ToolCall;
    use std::sync::Mutex;

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

    #[test]
    fn seq_allocator_is_monotonic() {
        let seq = SeqAllocator::new();
        assert_eq!(seq.next(), 1);
        assert_eq!(seq.next(), 2);
    }

    #[tokio::test]
    async fn multi_sink_fans_out() {
        let left = Arc::new(RecordingSink::default());
        let right = Arc::new(RecordingSink::default());
        let sink = MultiSink::new(vec![left.clone(), right.clone()]);

        sink.emit(Event::StepStarted { step: 1 }).await.unwrap();

        assert_eq!(left.events(), vec![Event::StepStarted { step: 1 }]);
        assert_eq!(right.events(), vec![Event::StepStarted { step: 1 }]);
    }

    #[tokio::test]
    async fn redacting_sink_redacts_sensitive_raw_args() {
        let inner = Arc::new(RecordingSink::default());
        let sink = RedactingSink::new(inner.clone());
        let call = ToolCall::new(
            "send",
            serde_json::json!({"api_key": "secret-value", "body": {"token": "abc"}}),
        );

        sink.emit(Event::LlmResponse {
            model: "test".into(),
            text: None,
            tool_calls: vec![call],
        })
        .await
        .unwrap();

        let events = inner.events();
        match &events[0] {
            Event::LlmResponse { tool_calls, .. } => {
                assert_eq!(tool_calls[0].raw_args["api_key"], "[REDACTED]");
                assert_eq!(tool_calls[0].raw_args["body"]["token"], "[REDACTED]");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn jsonl_sink_writes_stored_events() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let sink = JsonlSink::new(&path).unwrap();

        sink.emit(Event::AgentFinished {
            text: Some("done".into()),
        })
        .await
        .unwrap();

        let contents = std::fs::read_to_string(path).unwrap();
        let stored: StoredEvent = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(stored.seq, 1);
        assert_eq!(
            stored.event,
            Event::AgentFinished {
                text: Some("done".into())
            }
        );
    }

    #[test]
    fn event_roundtrip_preserves_tool_calls() {
        let msg = AssistantMessage {
            text: Some("hello".into()),
            tool_calls: vec![ToolCall::new("read", serde_json::json!({"path": "a"}))],
        };
        let event = Event::LlmResponse {
            model: "test".into(),
            text: msg.text,
            tool_calls: msg.tool_calls,
        };

        let json = serde_json::to_string(&event).unwrap();
        let decoded: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, event);
    }
}
