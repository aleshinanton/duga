//! TUI confirmation provider — channels tool execution approval requests
//! from the async agent runtime into the synchronous TUI event loop.
//!
//! Uses a tokio oneshot channel so the agent loop can await the user's
//! decision while the TUI renders the confirmation dialog.

use duga_runtime::confirmation::{ConfirmationDecision, ConfirmationProvider, ConfirmationRequest};
use tokio::sync::{mpsc, oneshot};

/// A pending confirmation request sent to the TUI event loop.
pub struct PendingConfirmation {
    pub request: ConfirmationRequest,
    pub timeout: std::time::Duration,
    pub response_tx: oneshot::Sender<ConfirmationDecision>,
}

/// TUI-side confirmation provider.
///
/// Implements `ConfirmationProvider` by sending the request over an mpsc
/// channel to the TUI event loop and awaiting the user's response via a
/// oneshot channel.
#[derive(Clone)]
pub struct TuiConfirmationProvider {
    tx: mpsc::UnboundedSender<PendingConfirmation>,
}

impl TuiConfirmationProvider {
    pub fn new(tx: mpsc::UnboundedSender<PendingConfirmation>) -> Self {
        Self { tx }
    }
}

#[async_trait::async_trait]
impl ConfirmationProvider for TuiConfirmationProvider {
    async fn confirm(
        &self,
        request: &ConfirmationRequest,
        timeout: std::time::Duration,
    ) -> ConfirmationDecision {
        let (response_tx, response_rx) = oneshot::channel();

        let pending = PendingConfirmation {
            request: request.clone(),
            timeout,
            response_tx,
        };

        // Send to TUI event loop. If the channel is closed (app exited),
        // default to denying.
        if self.tx.send(pending).is_err() {
            return ConfirmationDecision::Denied;
        }

        // Await the user's response with a timeout.
        match tokio::time::timeout(timeout, response_rx).await {
            Ok(Ok(decision)) => decision,
            Ok(Err(_)) => ConfirmationDecision::Timeout,
            Err(_) => ConfirmationDecision::Timeout,
        }
    }
}
