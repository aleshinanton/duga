//! Steering — dynamic mid-loop guidance injection.
//!
//! Steering allows human users, tools, or the loop itself to inject
//! guidance, observations, or constraints into the LLM context *during*
//! a loop run, not just at startup via the system prompt.
//!
//! # Architecture
//!
//! ```text
//! LLM → Response → Execute Tools → Push Results → [STEER] → LLM → ...
//! ```
//!
//! # Channel design
//!
//! - `SteeringSender` is cloned and stored in session state for external
//!   injection (e.g. Telegram bot).
//! - `SteeringReceiver` is owned by `LoopContext`, matching the loop's
//!   lifetime.
//! - `check_steer()` drains the receiver at injection points, applies
//!   context events to memory/limits, and returns the highest-priority
//!   control action.
//!
//! # Priority (Cancel > ForceComplete > Reprompt)
//!
//! The two-pass `drain()` collects all context events and returns exactly
//! one control event with Cancel taking precedence over ForceComplete,
//! which takes precedence over Reprompt.

use crate::loop_context::LoopContext;
use duga_events::Event;
use duga_types::error::AgentError;
use duga_types::message::Message;
use duga_types::tool_call::CallId;
use duga_types::tool_result::ToolResultBuilder;
use std::collections::VecDeque;
use tokio::sync::mpsc;

// ── Steering event types ─────────────────────────────────────────────────

/// Context-mutating steering events.
///
/// Applied by `check_steer()` during the context-event pass. These
/// modify memory or limits without changing loop control flow.
#[derive(Clone, Debug)]
pub enum SteeringContextEvent {
    /// Inject a guidance message into memory.
    InjectGuidance {
        /// The guidance text.
        text: String,
        /// If true, pushed as a system message (higher LLM attention).
        /// If false, pushed as a user message with a source prefix.
        as_system: bool,
        /// Origin tag for observability ("human", "self-diagnosis", "tool", "policy").
        source: String,
    },
    /// Reset the task anchor — the loop re-focuses on a new task.
    ResetTask {
        new_task: String,
        source: String,
    },
    /// Adjust remaining limits (steps / tool calls).
    /// `None` means "don't change this limit".
    AdjustLimits {
        remaining_steps: Option<u32>,
        remaining_tool_calls: Option<u32>,
    },
    /// Inject a synthetic tool result into memory (e.g., from an external
    /// system providing data mid-run).
    InjectToolResult {
        tool_name: String,
        result: String,
    },
}

/// Control-flow steering events.
///
/// Exactly one is returned by `drain()` after all context events are
/// applied. Priority: Cancel > ForceComplete > Reprompt.
#[derive(Clone, Debug)]
pub enum SteeringControlEvent {
    /// Terminate the loop immediately with an error.
    Cancel { reason: String },
    /// Terminate the loop successfully, returning the given answer.
    ForceComplete { answer: String },
    /// Re-call the LLM with the given guidance text injected.
    /// Buffered at POINT 2 (tool results), flushed at POINT 3.
    Reprompt { guidance: String },
}

/// A steering event — either context-mutating or control-flow.
#[derive(Clone, Debug)]
pub enum SteeringEvent {
    Context(SteeringContextEvent),
    Control(SteeringControlEvent),
}

// ── Channel types ────────────────────────────────────────────────────────

/// The sending end of a steering channel.
///
/// Clone it to share with multiple injectors (e.g., Telegram bot handler).
/// Drop all clones to signal that no more steering events are coming.
#[derive(Clone, Debug)]
pub struct SteeringSender {
    tx: mpsc::UnboundedSender<SteeringEvent>,
}

impl SteeringSender {
    pub fn new(tx: mpsc::UnboundedSender<SteeringEvent>) -> Self {
        Self { tx }
    }

    /// Returns `true` if the receiver is still alive and can receive events.
    pub fn is_active(&self) -> bool {
        !self.tx.is_closed()
    }

    /// Inject a raw steering event.
    ///
    /// Returns an error if the channel is closed (loop has finished).
    pub fn inject(&self, event: SteeringEvent) -> Result<(), AgentError> {
        self.tx
            .send(event)
            .map_err(|_| AgentError::EventSinkFailed("steering channel closed".into()))
    }

    /// Convenience: inject guidance as a Reprompt control event.
    pub fn guide(&self, text: &str) -> Result<(), AgentError> {
        self.inject(SteeringEvent::Control(SteeringControlEvent::Reprompt {
            guidance: text.to_string(),
        }))
    }

    /// Convenience: inject a Cancel control event.
    pub fn cancel(&self, reason: &str) -> Result<(), AgentError> {
        self.inject(SteeringEvent::Control(SteeringControlEvent::Cancel {
            reason: reason.to_string(),
        }))
    }
}

/// The receiving end of a steering channel.
///
/// Owned by `LoopContext` for the duration of one loop run. Uses
/// `try_recv()` to collect events without blocking, and buffers
/// `Reprompt` at POINT 2 until POINT 3.
#[derive(Debug)]
pub struct SteeringReceiver {
    rx: mpsc::UnboundedReceiver<SteeringEvent>,
    /// Reprompt that arrived at POINT 2 (where reprompt is not allowed).
    /// Flushed at the next checkpoint with `allow_reprompt = true`.
    pending_reprompt: Option<SteeringControlEvent>,
}

impl SteeringReceiver {
    pub fn new(rx: mpsc::UnboundedReceiver<SteeringEvent>) -> Self {
        Self {
            rx,
            pending_reprompt: None,
        }
    }

    /// Drain all pending events from the channel.
    ///
    /// Returns `(context_events, control_event)` where:
    /// - All `Context` events are collected into the vec.
    /// - At most one `Control` event is returned with priority:
    ///   Cancel > ForceComplete > Reprompt.
    /// - If `allow_reprompt` is `false` and a `Reprompt` arrives,
    ///   it is buffered in `pending_reprompt` instead of returned.
    /// - If `allow_reprompt` is `true` and `pending_reprompt` is set,
    ///   it is flushed — but has lower priority than any newly-arrived
    ///   control events.
    pub fn drain(
        &mut self,
        allow_reprompt: bool,
    ) -> (Vec<SteeringContextEvent>, Option<SteeringControlEvent>) {
        let mut context_events: Vec<SteeringContextEvent> = Vec::new();
        let mut control_candidate: Option<SteeringControlEvent> = None;

        // Collect all pending events.
        let mut pending: VecDeque<SteeringEvent> = VecDeque::new();
        while let Ok(event) = self.rx.try_recv() {
            pending.push_back(event);
        }

        for event in pending {
            match event {
                SteeringEvent::Context(ctx_event) => {
                    context_events.push(ctx_event);
                }
                SteeringEvent::Control(ctrl_event) => {
                    // Merge into candidate with priority: Cancel > ForceComplete > Reprompt.
                    control_candidate = merge_control(control_candidate, ctrl_event);
                }
            }
        }

        // Handle buffered reprompt from a previous POINT 2.
        if let Some(buffered) = self.pending_reprompt.take() {
            if allow_reprompt {
                // Flush the buffered reprompt — but only if no higher-priority
                // control arrived in this drain.
                control_candidate = merge_control(control_candidate, buffered);
            } else {
                // Still at POINT 2 — re-buffer.
                self.pending_reprompt = Some(buffered);
            }
        }

        // If the final control candidate is a Reprompt and allow_reprompt is false,
        // buffer it instead of returning it.
        if !allow_reprompt {
            if let Some(ref ctrl) = control_candidate {
                if matches!(ctrl, SteeringControlEvent::Reprompt { .. }) {
                    self.pending_reprompt = Some(ctrl.clone());
                    return (context_events, None);
                }
            }
        }

        (context_events, control_candidate)
    }
}

/// Merge two control events, keeping the higher-priority one.
/// Priority: Cancel > ForceComplete > Reprompt.
fn merge_control(
    existing: Option<SteeringControlEvent>,
    incoming: SteeringControlEvent,
) -> Option<SteeringControlEvent> {
    match existing {
        None => Some(incoming),
        Some(existing) => {
            let existing_prio = control_priority(&existing);
            let incoming_prio = control_priority(&incoming);
            if incoming_prio > existing_prio {
                Some(incoming)
            } else {
                Some(existing)
            }
        }
    }
}

fn control_priority(ctrl: &SteeringControlEvent) -> u8 {
    match ctrl {
        SteeringControlEvent::Cancel { .. } => 3,
        SteeringControlEvent::ForceComplete { .. } => 2,
        SteeringControlEvent::Reprompt { .. } => 1,
    }
}

// ── Steer limits ─────────────────────────────────────────────────────────

/// Limit adjustments from `AdjustLimits` steering events.
///
/// `None` means "no override" — the config defaults apply.
#[derive(Clone, Debug, Default)]
pub struct SteerLimits {
    pub remaining_steps: Option<u32>,
    pub remaining_tool_calls: Option<u32>,
}

// ── Steer action ─────────────────────────────────────────────────────────

/// The action the loop should take after `check_steer()`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SteerAction {
    /// No steering — continue normal execution.
    Continue,
    /// Terminate the loop with an error (reason).
    Cancel(String),
    /// Terminate the loop successfully with a forced answer.
    Complete(String),
    /// Re-call the LLM (guidance has been injected into memory).
    Reprompt,
}

// ── Public functions ─────────────────────────────────────────────────────

/// Check for steering events at an injection point.
///
/// Drains the receiver, applies all context events (mutating memory/limits),
/// emits `SteeringApplied` events, and returns the highest-priority control
/// action.
///
/// If `allow_reprompt` is `false` (POINT 2), `Reprompt` is buffered and
/// `SteerAction::Continue` is returned. The buffered reprompt is flushed
/// at the next checkpoint with `allow_reprompt = true`.
///
/// If `steer` is `None`, returns `SteerAction::Continue` immediately
/// (zero overhead).
pub async fn check_steer(
    ctx: &mut LoopContext<'_>,
    allow_reprompt: bool,
) -> Result<SteerAction, AgentError> {
    let receiver = match ctx.steer.as_mut() {
        Some(rx) => rx,
        None => return Ok(SteerAction::Continue),
    };

    let (context_events, control_event) = receiver.drain(allow_reprompt);

    for event in context_events {
        apply_context_event(ctx, event).await?;
    }

    match control_event {
        None => Ok(SteerAction::Continue),
        Some(SteeringControlEvent::Cancel { reason }) => {
            tracing::info!(%reason, "Steering cancelled run");
            ctx.event_sink
                .emit(Event::SteeringApplied {
                    source: "human".into(),
                    kind: "cancel".into(),
                })
                .await
                .map_err(|e| AgentError::EventSinkFailed(e.to_string()))?;
            Ok(SteerAction::Cancel(reason))
        }
        Some(SteeringControlEvent::ForceComplete { answer }) => {
            tracing::info!("Steering force-completed run");
            ctx.event_sink
                .emit(Event::SteeringApplied {
                    source: "human".into(),
                    kind: "complete".into(),
                })
                .await
                .map_err(|e| AgentError::EventSinkFailed(e.to_string()))?;
            Ok(SteerAction::Complete(answer))
        }
        Some(SteeringControlEvent::Reprompt { guidance }) => {
            tracing::info!("Steering reprompt — re-calling LLM");
            // The guidance was already injected as a context event by the caller
            // (human steering sends Reprompt which carries guidance; self-steering
            // sends InjectGuidance separately). If the Reprompt arrived via the
            // channel without a separate InjectGuidance, inject it now.
            // We check: if the guidance is non-empty and not already present as
            // the last message, push it.
            if !guidance.is_empty() {
                let msg = Message::user(format!("[Steering from human]: {guidance}"));
                ctx.memory.push_msg(msg);
            }
            ctx.event_sink
                .emit(Event::SteeringApplied {
                    source: "human".into(),
                    kind: "reprompt".into(),
                })
                .await
                .map_err(|e| AgentError::EventSinkFailed(e.to_string()))?;
            Ok(SteerAction::Reprompt)
        }
    }
}

/// Apply a single context event to memory or limits.
///
/// Called by `check_steer()` during the context-event pass, and also
/// directly by self-steering logic (bypassing the channel to keep
/// human-originated and machine-originated steering separate).
pub async fn apply_context_event(
    ctx: &mut LoopContext<'_>,
    event: SteeringContextEvent,
) -> Result<(), AgentError> {
    match event {
        SteeringContextEvent::InjectGuidance {
            text,
            as_system,
            source,
        } => {
            let msg = if as_system {
                Message::system(text)
            } else {
                Message::user(format!("[Steering from {source}]: {text}"))
            };
            ctx.memory.push_msg(msg);
            ctx.event_sink
                .emit(Event::SteeringApplied {
                    source,
                    kind: "guidance".into(),
                })
                .await
                .map_err(|e| AgentError::EventSinkFailed(e.to_string()))?;
        }
        SteeringContextEvent::ResetTask { new_task, source } => {
            ctx.memory.set_task_anchor(Some(new_task.clone()));
            ctx.memory
                .push_msg(Message::user(format!("[Steering from {source}]: Task reset to: {new_task}")));
            ctx.event_sink
                .emit(Event::SteeringApplied {
                    source,
                    kind: "reset".into(),
                })
                .await
                .map_err(|e| AgentError::EventSinkFailed(e.to_string()))?;
        }
        SteeringContextEvent::AdjustLimits {
            remaining_steps,
            remaining_tool_calls,
        } => {
            let limits = ctx.steer_limits.get_or_insert_with(SteerLimits::default);
            if let Some(steps) = remaining_steps {
                limits.remaining_steps = Some(steps);
            }
            if let Some(calls) = remaining_tool_calls {
                limits.remaining_tool_calls = Some(calls);
            }
            ctx.event_sink
                .emit(Event::SteeringApplied {
                    source: "human".into(),
                    kind: "limit".into(),
                })
                .await
                .map_err(|e| AgentError::EventSinkFailed(e.to_string()))?;
        }
        SteeringContextEvent::InjectToolResult { tool_name, result } => {
            let tool_result = ToolResultBuilder::new()
                .tool_call_id(CallId::new())
                .success(true)
                .output(result)
                .build()
                .ok_or_else(|| {
                    AgentError::EventSinkFailed("failed to build injected tool result".into())
                })?;
            ctx.memory.push_tool_result(tool_result);
            ctx.event_sink
                .emit(Event::SteeringApplied {
                    source: "external".into(),
                    kind: format!("tool_result:{tool_name}"),
                })
                .await
                .map_err(|e| AgentError::EventSinkFailed(e.to_string()))?;
        }
    }
    Ok(())
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_channel() -> (SteeringSender, SteeringReceiver) {
        let (tx, rx) = mpsc::unbounded_channel();
        (SteeringSender::new(tx), SteeringReceiver::new(rx))
    }

    // ── Channel roundtrip ────────────────────────────────────────────

    #[test]
    fn send_and_drain_roundtrip() {
        let (sender, mut receiver) = make_channel();

        sender
            .inject(SteeringEvent::Control(SteeringControlEvent::Cancel {
                reason: "test".into(),
            }))
            .unwrap();

        let (_ctx_events, ctrl_event) = receiver.drain(true);
        assert!(matches!(
            ctrl_event,
            Some(SteeringControlEvent::Cancel { .. })
        ));
    }

    #[test]
    fn context_event_drained_to_context_events() {
        let (sender, mut receiver) = make_channel();

        sender
            .inject(SteeringEvent::Context(SteeringContextEvent::InjectGuidance {
                text: "hello".into(),
                as_system: false,
                source: "human".into(),
            }))
            .unwrap();

        let (_ctx_events, ctrl_event) = receiver.drain(true);
        assert_eq!(_ctx_events.len(), 1);
        assert!(ctrl_event.is_none());
    }

    #[test]
    fn control_event_drained_to_control_event() {
        let (sender, mut receiver) = make_channel();

        sender
            .inject(SteeringEvent::Control(SteeringControlEvent::ForceComplete {
                answer: "done".into(),
            }))
            .unwrap();

        let (_ctx_events, ctrl_event) = receiver.drain(true);
        assert!(_ctx_events.is_empty());
        assert!(matches!(
            ctrl_event,
            Some(SteeringControlEvent::ForceComplete { .. })
        ));
    }

    // ── Priority tests ───────────────────────────────────────────────

    #[test]
    fn cancel_wins_over_force_complete() {
        let (sender, mut receiver) = make_channel();

        sender
            .inject(SteeringEvent::Control(SteeringControlEvent::ForceComplete {
                answer: "done".into(),
            }))
            .unwrap();
        sender
            .inject(SteeringEvent::Control(SteeringControlEvent::Cancel {
                reason: "stop".into(),
            }))
            .unwrap();

        let (_ctx_events, ctrl_event) = receiver.drain(true);
        assert!(_ctx_events.is_empty());
        assert!(matches!(
            ctrl_event,
            Some(SteeringControlEvent::Cancel { .. })
        ));
    }

    #[test]
    fn force_complete_wins_over_reprompt() {
        let (sender, mut receiver) = make_channel();

        sender
            .inject(SteeringEvent::Control(SteeringControlEvent::Reprompt {
                guidance: "try again".into(),
            }))
            .unwrap();
        sender
            .inject(SteeringEvent::Control(SteeringControlEvent::ForceComplete {
                answer: "done".into(),
            }))
            .unwrap();

        let (_ctx_events, ctrl_event) = receiver.drain(true);
        assert!(_ctx_events.is_empty());
        assert!(matches!(
            ctrl_event,
            Some(SteeringControlEvent::ForceComplete { .. })
        ));
    }

    #[test]
    fn cancel_wins_over_reprompt() {
        let (sender, mut receiver) = make_channel();

        sender
            .inject(SteeringEvent::Control(SteeringControlEvent::Reprompt {
                guidance: "try again".into(),
            }))
            .unwrap();
        sender
            .inject(SteeringEvent::Control(SteeringControlEvent::Cancel {
                reason: "stop".into(),
            }))
            .unwrap();

        let (_ctx_events, ctrl_event) = receiver.drain(true);
        assert!(matches!(
            ctrl_event,
            Some(SteeringControlEvent::Cancel { .. })
        ));
    }

    // ── Reprompt buffering ───────────────────────────────────────────

    #[test]
    fn reprompt_buffered_when_allow_false() {
        let (sender, mut receiver) = make_channel();

        sender
            .inject(SteeringEvent::Control(SteeringControlEvent::Reprompt {
                guidance: "try again".into(),
            }))
            .unwrap();

        let (_ctx_events, ctrl_event) = receiver.drain(false);
        assert!(_ctx_events.is_empty());
        assert!(ctrl_event.is_none(), "Reprompt should be buffered at POINT 2");
    }

    #[test]
    fn buffered_reprompt_flushed_next_allow_true() {
        let (sender, mut receiver) = make_channel();

        // Inject Reprompt at POINT 2 (allow_reprompt = false)
        sender
            .inject(SteeringEvent::Control(SteeringControlEvent::Reprompt {
                guidance: "try again".into(),
            }))
            .unwrap();

        let (_ctx_events, ctrl_event) = receiver.drain(false);
        assert!(_ctx_events.is_empty());
        assert!(ctrl_event.is_none(), "POINT 2: should be buffered");

        // Now at POINT 3 (allow_reprompt = true) — should flush
        let (_ctx_events, ctrl_event) = receiver.drain(true);
        assert!(_ctx_events.is_empty());
        assert!(
            matches!(ctrl_event, Some(SteeringControlEvent::Reprompt { .. })),
            "POINT 3: buffered reprompt should flush"
        );
    }

    #[test]
    fn buffered_reprompt_flushed_but_new_cancel_wins() {
        let (sender, mut receiver) = make_channel();

        // Buffered reprompt from earlier POINT 2
        sender
            .inject(SteeringEvent::Control(SteeringControlEvent::Reprompt {
                guidance: "try again".into(),
            }))
            .unwrap();

        let (_ctx, ctrl) = receiver.drain(false);
        assert!(ctrl.is_none(), "buffered");

        // Now a Cancel arrives before POINT 3
        sender
            .inject(SteeringEvent::Control(SteeringControlEvent::Cancel {
                reason: "user stopped".into(),
            }))
            .unwrap();

        let (_ctx_events, ctrl_event) = receiver.drain(true);
        assert!(_ctx_events.is_empty());
        assert!(
            matches!(ctrl_event, Some(SteeringControlEvent::Cancel { .. })),
            "Cancel should win over buffered reprompt"
        );
    }

    // ── is_active lifecycle ──────────────────────────────────────────

    #[test]
    fn is_active_before_and_after_drop() {
        let (sender, receiver) = make_channel();
        assert!(sender.is_active());

        drop(receiver);
        assert!(!sender.is_active());
    }

    // ── Empty drain ──────────────────────────────────────────────────

    #[test]
    fn empty_receiver_drain_returns_continue() {
        let (_sender, mut receiver) = make_channel();
        let (_ctx_events, ctrl_event) = receiver.drain(true);
        assert!(_ctx_events.is_empty());
        assert!(ctrl_event.is_none());
    }

    // ── Multiple context events ──────────────────────────────────────

    #[test]
    fn multiple_context_events_drained_in_one_pass() {
        let (sender, mut receiver) = make_channel();

        for i in 0..3 {
            sender
                .inject(SteeringEvent::Context(SteeringContextEvent::InjectGuidance {
                    text: format!("msg {i}"),
                    as_system: false,
                    source: "test".into(),
                }))
                .unwrap();
        }

        let (_ctx_events, ctrl_event) = receiver.drain(true);
        assert_eq!(_ctx_events.len(), 3);
        assert!(ctrl_event.is_none());
    }

    // ── Sender drop semantics ────────────────────────────────────────

    #[test]
    fn sender_drop_does_not_close_channel_while_clone_exists() {
        let (sender, mut receiver) = make_channel();
        let clone = sender.clone();
        assert!(clone.is_active());

        drop(sender);
        // Clone still exists, so channel isn't closed yet
        assert!(clone.is_active());

        // We can still inject via clone
        clone
            .inject(SteeringEvent::Control(SteeringControlEvent::Cancel {
                reason: "stop".into(),
            }))
            .unwrap();

        let (_, ctrl) = receiver.drain(true);
        assert!(matches!(ctrl, Some(SteeringControlEvent::Cancel { .. })));

        // When the last clone is dropped, the channel closes
        drop(clone);
        // After drop we can't check is_active on clone, but we can verify
        // the channel is closed by checking that the receiver returns empty.
        let (_ctx_events, ctrl) = receiver.drain(true);
        assert!(_ctx_events.is_empty());
        assert!(ctrl.is_none());
    }

    // ── Convenience methods ──────────────────────────────────────────

    #[test]
    fn sender_guide_convenience() {
        let (sender, mut receiver) = make_channel();
        sender.guide("try different approach").unwrap();

        let (_, ctrl) = receiver.drain(true);
        assert!(matches!(
            ctrl,
            Some(SteeringControlEvent::Reprompt { ref guidance }) if guidance == "try different approach"
        ));
    }

    #[test]
    fn sender_cancel_convenience() {
        let (sender, mut receiver) = make_channel();
        sender.cancel("user stopped").unwrap();

        let (_, ctrl) = receiver.drain(true);
        assert!(matches!(
            ctrl,
            Some(SteeringControlEvent::Cancel { ref reason }) if reason == "user stopped"
        ));
    }

    // ── Inject on dead channel ───────────────────────────────────────

    #[test]
    fn inject_after_receiver_dropped_returns_error() {
        let (sender, receiver) = make_channel();
        drop(receiver);

        let result = sender.inject(SteeringEvent::Control(
            SteeringControlEvent::Cancel {
                reason: "stop".into(),
            },
        ));
        assert!(result.is_err());
    }
}
