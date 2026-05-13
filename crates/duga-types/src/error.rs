//! Error types for the duga runtime.
//!
//! Two distinct error hierarchies:
//! - `ToolError` — per-tool invocation failures (become feedback to the LLM)
//! - `AgentError` — fatal loop-terminating conditions

use serde::{Deserialize, Serialize, Serializer};
use std::fmt;
use std::io;

/// Errors from a single tool invocation.
#[derive(Debug, PartialEq)]
pub enum ToolError {
    /// Tool execution timed out.
    Timeout,
    /// Tool execution was cancelled by the user or system.
    Cancelled,
    /// Tool call was denied by policy.
    Denied(String),
    /// Tool arguments failed validation.
    InvalidArgs(String),
    /// I/O error during execution.
    Io(String),
    /// Plugin-specific error.
    Plugin(String),
    /// Output exceeded configured limits.
    OutputLimitExceeded,
}

impl Clone for ToolError {
    fn clone(&self) -> Self {
        match self {
            ToolError::Timeout => ToolError::Timeout,
            ToolError::Cancelled => ToolError::Cancelled,
            ToolError::Denied(s) => ToolError::Denied(s.clone()),
            ToolError::InvalidArgs(s) => ToolError::InvalidArgs(s.clone()),
            ToolError::Io(s) => ToolError::Io(s.clone()),
            ToolError::Plugin(s) => ToolError::Plugin(s.clone()),
            ToolError::OutputLimitExceeded => ToolError::OutputLimitExceeded,
        }
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToolError::Timeout => write!(f, "tool execution timed out"),
            ToolError::Cancelled => write!(f, "tool execution was cancelled"),
            ToolError::Denied(msg) => write!(f, "denied: {}", msg),
            ToolError::InvalidArgs(msg) => write!(f, "invalid arguments: {}", msg),
            ToolError::Io(s) => write!(f, "i/o error: {}", s),
            ToolError::Plugin(msg) => write!(f, "plugin error: {}", msg),
            ToolError::OutputLimitExceeded => write!(f, "output limit exceeded"),
        }
    }
}

impl std::error::Error for ToolError {}

impl From<io::Error> for ToolError {
    fn from(e: io::Error) -> Self {
        ToolError::Io(e.to_string())
    }
}

impl ToolError {
    /// Returns `true` for errors that may be transient (retryable).
    pub fn is_transient(&self) -> bool {
        matches!(self, ToolError::Io(_) | ToolError::Plugin(_))
    }
}

// Custom Serialize for ToolError (because io::Error is not serializable)
impl Serialize for ToolError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;

        match self {
            ToolError::Timeout => {
                let mut s = serializer.serialize_struct("ToolError", 1)?;
                s.serialize_field("type", "Timeout")?;
                s.end()
            }
            ToolError::Cancelled => {
                let mut s = serializer.serialize_struct("ToolError", 1)?;
                s.serialize_field("type", "Cancelled")?;
                s.end()
            }
            ToolError::Denied(msg) => {
                let mut s = serializer.serialize_struct("ToolError", 2)?;
                s.serialize_field("type", "Denied")?;
                s.serialize_field("msg", msg)?;
                s.end()
            }
            ToolError::InvalidArgs(msg) => {
                let mut s = serializer.serialize_struct("ToolError", 2)?;
                s.serialize_field("type", "InvalidArgs")?;
                s.serialize_field("msg", msg)?;
                s.end()
            }
            ToolError::Io(s) => {
                let mut st = serializer.serialize_struct("ToolError", 2)?;
                st.serialize_field("type", "Io")?;
                st.serialize_field("msg", s)?;
                st.end()
            }
            ToolError::Plugin(msg) => {
                let mut s = serializer.serialize_struct("ToolError", 2)?;
                s.serialize_field("type", "Plugin")?;
                s.serialize_field("msg", msg)?;
                s.end()
            }
            ToolError::OutputLimitExceeded => {
                let mut s = serializer.serialize_struct("ToolError", 1)?;
                s.serialize_field("type", "OutputLimitExceeded")?;
                s.end()
            }
        }
    }
}

struct ToolErrorVisitor;

impl<'de> serde::de::Visitor<'de> for ToolErrorVisitor {
    type Value = ToolError;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a ToolError struct with a `type` discriminant")
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<ToolError, A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        let tag: String = seq
            .next_element()?
            .ok_or_else(|| serde::de::Error::invalid_length(0, &self))?;
        match tag.as_str() {
            "Timeout" => Ok(ToolError::Timeout),
            "Cancelled" => Ok(ToolError::Cancelled),
            "Denied" => {
                let msg = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(1, &self))?;
                Ok(ToolError::Denied(msg))
            }
            "InvalidArgs" => {
                let msg = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(1, &self))?;
                Ok(ToolError::InvalidArgs(msg))
            }
            "Io" => {
                let msg: String = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(1, &self))?;
                Ok(ToolError::Io(msg))
            }
            "Plugin" => {
                let msg = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(1, &self))?;
                Ok(ToolError::Plugin(msg))
            }
            "OutputLimitExceeded" => Ok(ToolError::OutputLimitExceeded),
            _ => Err(serde::de::Error::unknown_variant(
                &tag,
                &[
                    "Timeout",
                    "Cancelled",
                    "Denied",
                    "InvalidArgs",
                    "Io",
                    "Plugin",
                    "OutputLimitExceeded",
                ],
            )),
        }
    }

    fn visit_map<A>(self, mut map: A) -> Result<ToolError, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        let tag: String = map
            .next_key()?
            .ok_or_else(|| serde::de::Error::invalid_length(0, &self))?;
        if tag != "type" {
            return Err(serde::de::Error::unknown_field(&tag, &["type"]));
        }
        let variant: String = map.next_value()?;
        match variant.as_str() {
            "Timeout" => Ok(ToolError::Timeout),
            "Cancelled" => Ok(ToolError::Cancelled),
            "Denied" => {
                let _key: String = map
                    .next_key()?
                    .ok_or_else(|| serde::de::Error::invalid_length(1, &self))?;
                let msg = map.next_value::<String>()?;
                Ok(ToolError::Denied(msg))
            }
            "InvalidArgs" => {
                let _key: String = map
                    .next_key()?
                    .ok_or_else(|| serde::de::Error::invalid_length(1, &self))?;
                let msg = map.next_value::<String>()?;
                Ok(ToolError::InvalidArgs(msg))
            }
            "Io" => {
                let _key: String = map
                    .next_key()?
                    .ok_or_else(|| serde::de::Error::invalid_length(1, &self))?;
                let msg: String = map.next_value()?;
                Ok(ToolError::Io(msg))
            }
            "Plugin" => {
                let _key: String = map
                    .next_key()?
                    .ok_or_else(|| serde::de::Error::invalid_length(1, &self))?;
                let msg = map.next_value::<String>()?;
                Ok(ToolError::Plugin(msg))
            }
            "OutputLimitExceeded" => Ok(ToolError::OutputLimitExceeded),
            _ => Err(serde::de::Error::unknown_variant(
                &variant,
                &[
                    "Timeout",
                    "Cancelled",
                    "Denied",
                    "InvalidArgs",
                    "Io",
                    "Plugin",
                    "OutputLimitExceeded",
                ],
            )),
        }
    }
}

impl<'de> Deserialize<'de> for ToolError {
    fn deserialize<D>(deserializer: D) -> Result<ToolError, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(ToolErrorVisitor)
    }
}

// ── AgentError ─────────────────────────────────────────────────────────────

/// Fatal errors that terminate the agent loop.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum AgentError {
    /// Reached the maximum number of reasoning steps.
    #[serde(rename = "max_steps_reached")]
    MaxStepsReached { limit: u32 },
    /// Reached the maximum number of tool calls.
    #[serde(rename = "max_tool_calls_reached")]
    MaxToolCallsReached { limit: u32 },
    /// Overall timeout exceeded.
    #[serde(rename = "timeout")]
    Timeout,
    /// Execution was cancelled externally.
    #[serde(rename = "cancelled")]
    Cancelled,
    /// Context window overflowed.
    #[serde(rename = "context_overflow")]
    ContextOverflow,
    /// The summarizer failed.
    #[serde(rename = "summarizer_failed")]
    SummarizerFailed(String),
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentError::MaxStepsReached { limit } => {
                write!(f, "max steps ({}) reached", limit)
            }
            AgentError::MaxToolCallsReached { limit } => {
                write!(f, "max tool calls ({}) reached", limit)
            }
            AgentError::Timeout => write!(f, "timeout"),
            AgentError::Cancelled => write!(f, "cancelled"),
            AgentError::ContextOverflow => write!(f, "context overflow"),
            AgentError::SummarizerFailed(msg) => write!(f, "summarizer failed: {}", msg),
        }
    }
}

impl std::error::Error for AgentError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_error_transient() {
        assert!(!ToolError::Timeout.is_transient());
        assert!(!ToolError::Cancelled.is_transient());
        assert!(!ToolError::Denied("x".into()).is_transient());
        assert!(!ToolError::InvalidArgs("x".into()).is_transient());
        assert!(ToolError::Io("x".into()).is_transient());
        assert!(ToolError::Plugin("x".into()).is_transient());
        assert!(!ToolError::OutputLimitExceeded.is_transient());
    }

    #[test]
    fn test_tool_error_display() {
        assert_eq!(ToolError::Timeout.to_string(), "tool execution timed out");
        assert_eq!(ToolError::Denied("foo".into()).to_string(), "denied: foo");
        assert_eq!(
            ToolError::Cancelled.to_string(),
            "tool execution was cancelled"
        );
        assert_eq!(
            ToolError::Plugin("boom".into()).to_string(),
            "plugin error: boom"
        );
        assert_eq!(
            ToolError::OutputLimitExceeded.to_string(),
            "output limit exceeded"
        );
        let io_err = ToolError::Io("disk full".into());
        assert!(io_err.to_string().contains("disk full"));
    }

    #[test]
    fn test_tool_error_serde_roundtrip() {
        let variants = vec![
            ToolError::Timeout,
            ToolError::Cancelled,
            ToolError::Denied("test denied".into()),
            ToolError::InvalidArgs("bad arg".into()),
            ToolError::Io("disk error".into()),
            ToolError::Plugin("plugin boom".into()),
            ToolError::OutputLimitExceeded,
        ];
        for err in variants {
            let json = serde_json::to_string(&err).unwrap();
            let decoded: ToolError = serde_json::from_str(&json).unwrap();
            assert_eq!(
                decoded.is_transient(),
                err.is_transient(),
                "transient mismatch for {}",
                json
            );
            assert_eq!(
                decoded.to_string(),
                err.to_string(),
                "display mismatch for {}",
                json
            );
        }
    }

    #[test]
    fn test_agent_error_display() {
        assert_eq!(
            AgentError::MaxStepsReached { limit: 50 }.to_string(),
            "max steps (50) reached"
        );
        assert_eq!(
            AgentError::MaxToolCallsReached { limit: 100 }.to_string(),
            "max tool calls (100) reached"
        );
        assert_eq!(AgentError::Timeout.to_string(), "timeout");
        assert_eq!(AgentError::Cancelled.to_string(), "cancelled");
        assert_eq!(AgentError::ContextOverflow.to_string(), "context overflow");
        assert!(AgentError::SummarizerFailed("boom".into())
            .to_string()
            .contains("summarizer failed: boom"));
    }

    #[test]
    fn test_agent_error_serde_roundtrip() {
        let err = AgentError::MaxStepsReached { limit: 10 };
        let json = serde_json::to_string(&err).unwrap();
        let decoded: AgentError = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, err);

        let err = AgentError::SummarizerFailed("oops".into());
        let json = serde_json::to_string(&err).unwrap();
        let decoded: AgentError = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, err);
    }
}
