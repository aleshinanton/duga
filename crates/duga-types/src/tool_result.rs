//! Tool result types — the outcome of a tool invocation.
//!
//! `ToolResult` captures everything a tool produces: success/failure,
//! output, timing, and byte counts. The builder pattern allows
//! incremental construction from process output or error paths.

use crate::error::ToolError;
use crate::tool_call::CallId;
use serde::{Deserialize, Serialize};
use std::time::Instant;

/// The result of a single tool invocation.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ToolResult {
    pub tool_call_id: CallId,
    pub success: bool,
    pub output: String,
    /// Optional steering hint from the tool (e.g., "search returned 500 results — narrow your query").
    /// Processed by the loop as a context event (source: "tool").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steering_hint: Option<String>,
    #[serde(flatten)]
    pub metadata: serde_json::Value,
    pub duration_ms: u64,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub truncated: bool,
}

/// Builder for `ToolResult`.
#[derive(Debug, Default)]
pub struct ToolResultBuilder {
    tool_call_id: Option<CallId>,
    success: bool,
    output: String,
    steering_hint: Option<String>,
    metadata: serde_json::Value,
    duration_ms: u64,
    stdout_bytes: u64,
    stderr_bytes: u64,
    truncated: bool,
}

impl ToolResultBuilder {
    pub fn new() -> Self {
        Self {
            tool_call_id: None,
            success: false,
            output: String::new(),
            steering_hint: None,
            metadata: serde_json::Value::Null,
            duration_ms: 0,
            stdout_bytes: 0,
            stderr_bytes: 0,
            truncated: false,
        }
    }

    pub fn tool_call_id(&mut self, id: CallId) -> &mut Self {
        self.tool_call_id = Some(id);
        self
    }

    pub fn success(&mut self, success: bool) -> &mut Self {
        self.success = success;
        self
    }

    pub fn output(&mut self, output: impl Into<String>) -> &mut Self {
        self.output = output.into();
        self
    }

    pub fn metadata(&mut self, metadata: serde_json::Value) -> &mut Self {
        self.metadata = metadata;
        self
    }

    pub fn duration_ms(&mut self, duration_ms: u128) -> &mut Self {
        self.duration_ms = duration_ms.min(u64::MAX as u128) as u64;
        self
    }

    pub fn stdout_bytes(&mut self, bytes: u64) -> &mut Self {
        self.stdout_bytes = bytes;
        self
    }

    pub fn stderr_bytes(&mut self, bytes: u64) -> &mut Self {
        self.stderr_bytes = bytes;
        self
    }

    pub fn truncated(&mut self, truncated: bool) -> &mut Self {
        self.truncated = truncated;
        self
    }

    pub fn steering_hint(&mut self, hint: impl Into<String>) -> &mut Self {
        self.steering_hint = Some(hint.into());
        self
    }

    pub fn build(&self) -> Option<ToolResult> {
        Some(ToolResult {
            tool_call_id: self.tool_call_id.clone()?,
            success: self.success,
            output: self.output.clone(),
            steering_hint: self.steering_hint.clone(),
            metadata: self.metadata.clone(),
            duration_ms: self.duration_ms,
            stdout_bytes: self.stdout_bytes,
            stderr_bytes: self.stderr_bytes,
            truncated: self.truncated,
        })
    }
}

impl ToolResult {
    /// Build a `ToolResult` from an outcome.
    ///
    /// On `Ok`: returns the inner `ToolResult` with duration measured from `started`.
    /// On `Err`: builds a result with `success: false`, `output` set to the error display,
    ///           and duration measured from `started`.
    pub fn from_outcome(
        tool_call_id: CallId,
        tool_name: &str,
        outcome: Result<ToolResult, ToolError>,
        started: Instant,
    ) -> ToolResult {
        match outcome {
            Ok(result) => result,
            Err(e) => ToolResultBuilder::new()
                .tool_call_id(tool_call_id)
                .success(false)
                .output(format!("{}: {}", tool_name, e))
                .duration_ms(started.elapsed().as_millis())
                .build()
                .expect("all fields set"),
        }
    }
}

impl std::fmt::Display for ToolResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "tool_call_id={}\nsuccess={}\noutput={}\nduration={}ms\nstdout={}B\nstderr={}B\ntruncated={}",
            self.tool_call_id,
            self.success,
            self.output.lines().next().unwrap_or(""),
            self.duration_ms,
            self.stdout_bytes,
            self.stderr_bytes,
            self.truncated
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ToolError;

    #[test]
    fn test_builder_all_fields() {
        let id = CallId::new();
        let result = ToolResultBuilder::new()
            .tool_call_id(id.clone())
            .success(true)
            .output("hello world")
            .metadata(serde_json::json!({"key": "value"}))
            .duration_ms(42)
            .stdout_bytes(100)
            .stderr_bytes(0)
            .truncated(false)
            .build()
            .expect("all fields set");

        assert_eq!(result.tool_call_id, id);
        assert!(result.success);
        assert_eq!(result.output, "hello world");
        assert_eq!(result.duration_ms, 42);
        assert_eq!(result.stdout_bytes, 100);
        assert_eq!(result.stderr_bytes, 0);
        assert!(!result.truncated);
    }

    #[test]
    fn test_builder_defaults() {
        let id = CallId::new();
        let result = ToolResultBuilder::new()
            .tool_call_id(id.clone())
            .build()
            .expect("all fields set");

        assert!(!result.success);
        assert!(result.output.is_empty());
        assert_eq!(result.metadata, serde_json::Value::Null);
        assert_eq!(result.duration_ms, 0);
        assert_eq!(result.stdout_bytes, 0);
        assert_eq!(result.stderr_bytes, 0);
        assert!(!result.truncated);
    }

    #[test]
    fn test_builder_missing_id_returns_none() {
        let result = ToolResultBuilder::new().build();
        assert!(result.is_none());
    }

    #[test]
    fn test_from_outcome_success() {
        let id = CallId::new();
        let inner = ToolResultBuilder::new()
            .tool_call_id(id.clone())
            .success(true)
            .output("done")
            .duration_ms(10)
            .build()
            .unwrap();
        let outcome: Result<ToolResult, ToolError> = Ok(inner.clone());
        let started = Instant::now();
        std::thread::sleep(std::time::Duration::from_millis(1));
        let result = ToolResult::from_outcome(id, "test", outcome, started);
        assert_eq!(result.success, inner.success);
        assert_eq!(result.output, inner.output);
    }

    #[test]
    fn test_from_outcome_error() {
        let id = CallId::new();
        let outcome: Result<ToolResult, ToolError> = Err(ToolError::Timeout);
        let started = Instant::now();
        std::thread::sleep(std::time::Duration::from_millis(1));
        let result = ToolResult::from_outcome(id, "shell", outcome, started);
        assert!(!result.success);
        assert!(result.output.contains("shell"));
        assert!(result.output.contains("timed out"));
        assert!(result.duration_ms >= 1);
    }

    #[test]
    fn test_from_outcome_io_error() {
        let id = CallId::new();
        let outcome: Result<ToolResult, ToolError> = Err(ToolError::Io("file missing".into()));
        let started = Instant::now();
        let result = ToolResult::from_outcome(id, "read", outcome, started);
        assert!(!result.success);
        assert!(result.output.contains("read"));
        assert!(result.output.contains("i/o error"));
    }

    #[test]
    fn test_display() {
        let id = CallId::new();
        let result = ToolResultBuilder::new()
            .tool_call_id(id.clone())
            .success(true)
            .output("hello")
            .duration_ms(50)
            .stdout_bytes(5)
            .stderr_bytes(0)
            .truncated(false)
            .build()
            .unwrap();
        let display = format!("{}", result);
        assert!(display.contains("tool_call_id="));
        assert!(display.contains("success=true"));
        assert!(display.contains("duration=50ms"));
    }

    #[test]
    fn test_roundtrip() {
        let id = CallId::new();
        let original = ToolResultBuilder::new()
            .tool_call_id(id.clone())
            .success(true)
            .output("test output")
            .metadata(serde_json::json!({"foo": "bar"}))
            .duration_ms(100)
            .stdout_bytes(50)
            .stderr_bytes(10)
            .truncated(true)
            .build()
            .unwrap();

        let json = serde_json::to_string(&original).unwrap();
        let decoded: ToolResult = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.tool_call_id, original.tool_call_id);
        assert_eq!(decoded.success, original.success);
        assert_eq!(decoded.output, original.output);
        assert_eq!(decoded.duration_ms, original.duration_ms);
        assert_eq!(decoded.truncated, original.truncated);
    }
}
