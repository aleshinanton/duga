//! Shared tool confirmation middleware for duga frontends.
//!
//! Risky tool calls (bash, write) can be gated behind confirmation.
//! Frontends (CLI, Telegram, TUI) provide their own `ConfirmationProvider`
//! implementation; this module handles the shared policy and decision logic.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::Duration;
use thiserror::Error;

/// Frontend-neutral request for tool execution confirmation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConfirmationRequest {
    /// Unique ID matching a specific tool call instance.
    pub confirmation_id: String,
    /// Tool name being invoked (e.g. "bash", "write").
    pub tool_name: String,
    /// Human-readable label describing the pending action.
    pub label: String,
    /// Optional raw arguments for display in the confirmation prompt.
    pub arguments: Option<String>,
}

/// Outcome of a confirmation request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfirmationDecision {
    /// Execution is allowed.
    Approved,
    /// Execution is denied.
    Denied,
    /// The confirmation timed out.
    Timeout,
}

/// Policy for which tools require confirmation.
#[derive(Clone, Debug)]
pub struct ConfirmationPolicy {
    pub require_for: HashSet<String>,
    pub timeout: Duration,
}

impl Default for ConfirmationPolicy {
    fn default() -> Self {
        Self {
            require_for: HashSet::from(["bash".into(), "write".into()]),
            timeout: Duration::from_secs(60),
        }
    }
}

impl ConfirmationPolicy {
    /// Create a policy requiring confirmation for the given tool names.
    pub fn new(require_for: impl IntoIterator<Item = impl Into<String>>, timeout: Duration) -> Self {
        Self {
            require_for: require_for.into_iter().map(|s| s.into()).collect(),
            timeout,
        }
    }

    /// Check whether a tool call requires confirmation.
    pub fn requires_confirmation(&self, tool_name: &str) -> bool {
        self.require_for.contains(tool_name)
    }
}

/// Decision from the confirmation middleware after policy evaluation.
#[derive(Clone, Debug)]
pub enum MiddlewareDecision {
    /// Tool call is allowed to proceed immediately.
    Allow,
    /// Tool call requires confirmation via the frontend provider.
    Confirm(ConfirmationRequest),
}

/// Evaluate whether a tool call needs confirmation and produce a middleware decision.
pub fn evaluate_confirmation(
    tool_name: &str,
    policy: &ConfirmationPolicy,
    confirmation_id: impl Into<String>,
    label: impl Into<String>,
    arguments: Option<String>,
) -> MiddlewareDecision {
    if policy.requires_confirmation(tool_name) {
        MiddlewareDecision::Confirm(ConfirmationRequest {
            confirmation_id: confirmation_id.into(),
            tool_name: tool_name.into(),
            label: label.into(),
            arguments,
        })
    } else {
        MiddlewareDecision::Allow
    }
}

/// Object-safe confirmation provider trait.
///
/// Frontends implement this trait to prompt users for tool execution
/// approval via their native UI (CLI stdin, Telegram inline buttons, TUI modal).
#[async_trait::async_trait]
pub trait ConfirmationProvider: Send + Sync {
    /// Prompt the user to approve or deny a pending tool call.
    ///
    /// Returns the decision. Implementations should respect the timeout.
    async fn confirm(&self, request: &ConfirmationRequest, timeout: Duration) -> ConfirmationDecision;
}

/// Error returned when a tool call is denied by the user.
#[derive(Clone, Debug, Error)]
#[error("tool call denied by confirmation middleware")]
pub struct ToolDeniedError;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_requires_confirmation() {
        let policy = ConfirmationPolicy::default();
        assert!(policy.requires_confirmation("bash"));
        assert!(policy.requires_confirmation("write"));
        assert!(!policy.requires_confirmation("read"));
        assert!(!policy.requires_confirmation("search"));
    }

    #[test]
    fn evaluate_confirm_when_required() {
        let policy = ConfirmationPolicy::default();
        let decision = evaluate_confirmation("bash", &policy, "abc-123", "Run: ls", None);
        match decision {
            MiddlewareDecision::Confirm(req) => {
                assert_eq!(req.tool_name, "bash");
                assert_eq!(req.confirmation_id, "abc-123");
            }
            other => panic!("expected Confirm, got {other:?}"),
        }
    }

    #[test]
    fn evaluate_allow_when_not_required() {
        let policy = ConfirmationPolicy::default();
        let decision = evaluate_confirmation("read", &policy, "abc-123", "Read file", None);
        match decision {
            MiddlewareDecision::Allow => {}
            other => panic!("expected Allow, got {other:?}"),
        }
    }

    #[test]
    fn custom_policy() {
        let policy = ConfirmationPolicy::new(["delete", "rm"], Duration::from_secs(30));
        assert!(policy.requires_confirmation("delete"));
        assert!(!policy.requires_confirmation("bash"));
        assert_eq!(policy.timeout, Duration::from_secs(30));
    }
}
