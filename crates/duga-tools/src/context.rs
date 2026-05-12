//! Tool execution context — workspace, cancellation, event sink.
//!
//! `ToolContext` is passed to every tool execution call. It carries
//! the sandboxed workspace, cancellation signal, and event sink.
//! No mutable memory, no orchestration control, no prompt injection access.

use crate::event_sink::EventSink;
use duga_sandbox::CancellationToken;
use duga_sandbox::Workspace;

/// Context passed to tool execution.
pub struct ToolContext<'a> {
    /// The sandboxed workspace directory.
    pub workspace: &'a Workspace,
    /// Cancellation token for aborting execution.
    pub cancellation: CancellationToken,
    /// Where to emit events.
    pub event_sink: &'a dyn EventSink,
}

impl<'a> ToolContext<'a> {
    pub fn new(
        workspace: &'a Workspace,
        cancellation: CancellationToken,
        event_sink: &'a dyn EventSink,
    ) -> Self {
        Self {
            workspace,
            cancellation,
            event_sink,
        }
    }

    /// Check if cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }
}

// Safe because Workspace is Send, CancellationToken is Send,
// and &dyn EventSink: Send + Sync is required by EventSink.
unsafe impl Send for ToolContext<'_> {}
unsafe impl Sync for ToolContext<'_> {}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummySink;
    impl EventSink for DummySink {}

    #[test]
    fn test_tool_context_new_and_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = Workspace::open(dir.path()).unwrap();
        let cancel = CancellationToken::new();
        let sink = DummySink;

        let ctx = ToolContext::new(&workspace, cancel.clone(), &sink);
        assert!(!ctx.is_cancelled());
    }
}