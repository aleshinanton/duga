//! Transcript items and conversation history for the TUI.
//!
//! Each item in the transcript represents a user message, assistant
//! response, tool call block, system notice, or delegation/memory event.

use std::time::Instant;

/// A single item in the conversation transcript.
#[derive(Clone, Debug)]
pub enum TranscriptItem {
    /// A system message (startup info, warnings, errors).
    SystemMessage {
        text: String,
        level: SystemLevel,
        timestamp: Instant,
    },
    /// A user-submitted prompt.
    UserMessage {
        text: String,
        timestamp: Instant,
    },
    /// An assistant response (may be streaming).
    AssistantMessage {
        text: String,
        timestamp: Instant,
        /// True while the LLM is still streaming tokens.
        is_streaming: bool,
    },
    /// A tool call block (collapsible).
    ToolCallBlock {
        tool_call_id: String,
        tool_name: String,
        description: String,
        is_running: bool,
        is_success: Option<bool>,
        is_expanded: bool,
        timestamp: Instant,
    },
    /// A delegation notice (loop handoff).
    DelegationNotice {
        from: String,
        to: String,
        reason: String,
        depth: u32,
        timestamp: Instant,
    },
    /// A memory compression notice.
    MemoryNotice {
        before_tokens: usize,
        after_tokens: usize,
        timestamp: Instant,
    },
}

/// Severity level for system messages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SystemLevel {
    Info,
    Warn,
    Error,
}

/// Scroll state for the transcript pane.
#[derive(Clone, Debug)]
pub struct ScrollState {
    /// Number of items scrolled above the visible area.
    pub offset: usize,
    /// Whether the user has manually scrolled (auto-follow disabled).
    pub manual_scroll: bool,
}

impl ScrollState {
    pub fn new() -> Self {
        Self {
            offset: 0,
            manual_scroll: false,
        }
    }

    /// Scroll up by n items.
    pub fn scroll_up(&mut self, n: usize) {
        self.offset = self.offset.saturating_add(n);
        self.manual_scroll = true;
    }

    /// Scroll down by n items.
    pub fn scroll_down(&mut self, n: usize) {
        self.offset = self.offset.saturating_sub(n);
        if self.offset == 0 {
            self.manual_scroll = false;
        }
    }

    /// Scroll to the bottom (auto-follow).
    pub fn scroll_to_bottom(&mut self) {
        self.offset = 0;
        self.manual_scroll = false;
    }

    /// Scroll to the top.
    pub fn scroll_to_top(&mut self, item_count: usize) {
        self.offset = item_count.saturating_sub(1);
        self.manual_scroll = true;
    }
}

impl Default for ScrollState {
    fn default() -> Self {
        Self::new()
    }
}

/// Conversation transcript with scroll state.
#[derive(Clone, Debug)]
pub struct Transcript {
    /// All items in chronological order.
    items: Vec<TranscriptItem>,
    /// Scroll state.
    scroll: ScrollState,
    /// Maps tool_call_id → index in items (for updating tool blocks).
    tool_call_index: std::collections::HashMap<String, usize>,
    /// Index of the currently-streaming assistant message (if any).
    streaming_index: Option<usize>,
}

impl Transcript {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            scroll: ScrollState::new(),
            tool_call_index: std::collections::HashMap::new(),
            streaming_index: None,
        }
    }

    /// Push an item onto the transcript.
    pub fn push(&mut self, item: TranscriptItem) {
        // Track tool call indices and streaming state
        match &item {
            TranscriptItem::ToolCallBlock {
                tool_call_id,
                is_running,
                ..
            } => {
                let idx = self.items.len();
                self.tool_call_index.insert(tool_call_id.clone(), idx);
                if !is_running {
                    // If it's already finished, collapse based on tool_event_format
                }
            }
            TranscriptItem::AssistantMessage { is_streaming, .. } => {
                if *is_streaming {
                    self.streaming_index = Some(self.items.len());
                }
            }
            _ => {}
        }
        self.items.push(item);
        // Auto-scroll to bottom if not manually scrolled
        if !self.scroll.manual_scroll {
            self.scroll.scroll_to_bottom();
        }
    }

    /// Get a reference to the items.
    pub fn items(&self) -> &[TranscriptItem] {
        &self.items
    }

    /// Get a mutable reference to an item by index.
    pub fn item_mut(&mut self, idx: usize) -> Option<&mut TranscriptItem> {
        self.items.get_mut(idx)
    }

    /// Get the streaming assistant message index.
    pub fn streaming_index(&self) -> Option<usize> {
        self.streaming_index
    }

    /// Find a tool call block by tool_call_id.
    pub fn find_tool_call(&self, tool_call_id: &str) -> Option<usize> {
        self.tool_call_index.get(tool_call_id).copied()
    }

    /// Update the last assistant message (append streaming tokens).
    pub fn append_to_streaming(&mut self, delta: &str) {
        if let Some(idx) = self.streaming_index {
            if let Some(TranscriptItem::AssistantMessage {
                text, ..
            }) = self.items.get_mut(idx)
            {
                text.push_str(delta);
            }
        } else {
            // No streaming message exists yet; create one.
            let item = TranscriptItem::AssistantMessage {
                text: delta.to_string(),
                timestamp: Instant::now(),
                is_streaming: true,
            };
            self.push(item);
        }
    }

    /// Mark the streaming assistant message as complete.
    pub fn finish_streaming(&mut self) {
        if let Some(idx) = self.streaming_index.take() {
            if let Some(TranscriptItem::AssistantMessage {
                is_streaming, ..
            }) = self.items.get_mut(idx)
            {
                *is_streaming = false;
            }
        }
    }

    /// Update a tool call block (mark as finished).
    pub fn update_tool_call(
        &mut self,
        tool_call_id: &str,
        is_success: bool,
    ) {
        if let Some(idx) = self.tool_call_index.get(tool_call_id) {
            if let Some(TranscriptItem::ToolCallBlock {
                is_running,
                is_success: ref mut succ,
                is_expanded,
                ..
            }) = self.items.get_mut(*idx)
            {
                *is_running = false;
                *succ = Some(is_success);
                // Expand on failure, collapse on success
                if !is_success {
                    *is_expanded = true;
                }
            }
        }
    }

    /// Toggle a tool call block's expanded state.
    pub fn toggle_tool_expand(&mut self, idx: usize) {
        if let Some(TranscriptItem::ToolCallBlock {
            is_expanded, ..
        }) = self.items.get_mut(idx)
        {
            *is_expanded = !*is_expanded;
        }
    }

    /// Number of items.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the transcript is empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Get scroll state.
    pub fn scroll(&self) -> &ScrollState {
        &self.scroll
    }

    /// Get mutable scroll state.
    pub fn scroll_mut(&mut self) -> &mut ScrollState {
        &mut self.scroll
    }

    /// Clear all items.
    pub fn clear(&mut self) {
        self.items.clear();
        self.tool_call_index.clear();
        self.streaming_index = None;
        self.scroll = ScrollState::new();
    }
}

impl Default for Transcript {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_retrieve_items() {
        let mut t = Transcript::new();
        t.push(TranscriptItem::UserMessage {
            text: "hello".into(),
            timestamp: Instant::now(),
        });
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn append_to_streaming() {
        let mut t = Transcript::new();
        t.append_to_streaming("Hello");
        t.append_to_streaming(" world");
        assert_eq!(t.len(), 1);
        if let TranscriptItem::AssistantMessage { text, is_streaming, .. } = &t.items[0] {
            assert_eq!(text, "Hello world");
            assert!(is_streaming);
        } else {
            panic!("expected AssistantMessage");
        }
    }

    #[test]
    fn finish_streaming() {
        let mut t = Transcript::new();
        t.append_to_streaming("Done");
        t.finish_streaming();
        if let TranscriptItem::AssistantMessage { is_streaming, .. } = &t.items[0] {
            assert!(!is_streaming);
        } else {
            panic!("expected AssistantMessage");
        }
    }

    #[test]
    fn update_tool_call() {
        let mut t = Transcript::new();
        t.push(TranscriptItem::ToolCallBlock {
            tool_call_id: "tc1".into(),
            tool_name: "shell".into(),
            description: "ls".into(),
            is_running: true,
            is_success: None,
            is_expanded: false,
            timestamp: Instant::now(),
        });
        t.update_tool_call("tc1", true);
        if let TranscriptItem::ToolCallBlock { is_running, is_success, .. } = &t.items[0] {
            assert!(!is_running);
            assert_eq!(*is_success, Some(true));
        } else {
            panic!("expected ToolCallBlock");
        }
    }

    #[test]
    fn scroll_behavior() {
        let mut t = Transcript::new();
        for i in 0..5 {
            t.push(TranscriptItem::SystemMessage {
                text: format!("msg {i}"),
                level: SystemLevel::Info,
                timestamp: Instant::now(),
            });
        }
        // Should auto-scroll to bottom
        assert_eq!(t.scroll().offset, 0);
        assert!(!t.scroll().manual_scroll);

        t.scroll_mut().scroll_up(2);
        assert_eq!(t.scroll().offset, 2);
        assert!(t.scroll().manual_scroll);
    }
}
