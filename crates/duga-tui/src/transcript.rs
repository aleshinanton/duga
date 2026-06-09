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
        /// Human-readable label from the LLM.
        description: String,
        /// Raw JSON arguments (the actual command/parameters).
        raw_args: Option<String>,
        is_running: bool,
        is_success: Option<bool>,
        is_expanded: bool,
        timestamp: Instant,
        /// Tool result output (shown when expanded).
        output: Option<String>,
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
    /// LLM thinking/reasoning content (collapsible, distinct from final response).
    ThinkingBlock {
        text: String,
        is_streaming: bool,
        is_expanded: bool,
        /// Line offset for paginated view (0 = first page).
        scroll_offset: usize,
        timestamp: Instant,
        /// When the first thinking delta arrived (for duration tracking).
        started_at: Option<Instant>,
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
    /// Number of lines scrolled above the bottom of the viewport.
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

    /// Scroll up by n lines.
    pub fn scroll_up(&mut self, n: usize) {
        self.offset = self.offset.saturating_add(n);
        self.manual_scroll = true;
    }

    /// Scroll down by n lines.
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
    /// Index of the currently-streaming thinking block (if any).
    streaming_thinking_index: Option<usize>,
    /// Default expand state for new collapsible blocks.
    /// When true, new thinking/tool blocks start expanded.
    pub default_expanded: bool,
}

impl Transcript {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            scroll: ScrollState::new(),
            tool_call_index: std::collections::HashMap::new(),
            streaming_index: None,
            streaming_thinking_index: None,
            default_expanded: false,
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
            TranscriptItem::ThinkingBlock { is_streaming, .. } => {
                if *is_streaming {
                    self.streaming_thinking_index = Some(self.items.len());
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

    /// Append text to the current thinking block. Auto-starts a new block if none exists.
    pub fn append_to_thinking(&mut self, delta: &str) {
        if let Some(idx) = self.streaming_thinking_index {
            if let Some(TranscriptItem::ThinkingBlock { text, .. }) = self.items.get_mut(idx) {
                text.push_str(delta);
                return;
            }
        }
        let now = Instant::now();
        let item = TranscriptItem::ThinkingBlock {
            text: delta.to_string(),
            is_streaming: true,
            is_expanded: true,
            scroll_offset: 0,
            timestamp: now,
            started_at: Some(now),
        };
        self.streaming_thinking_index = Some(self.items.len());
        self.items.push(item);
        if !self.scroll.manual_scroll {
            self.scroll.scroll_to_bottom();
        }
    }

    /// Mark streaming thinking as complete. Respects default_expanded preference.
    pub fn finish_thinking(&mut self) {
        if let Some(idx) = self.streaming_thinking_index.take() {
            if let Some(TranscriptItem::ThinkingBlock {
                is_streaming,
                is_expanded,
                scroll_offset,
                ..
            }) = self.items.get_mut(idx)
            {
                *is_streaming = false;
                *is_expanded = self.default_expanded;
                *scroll_offset = 0;
            }
        }
    }

    /// Whether a thinking block is currently streaming.
    pub fn thinking_is_streaming(&self) -> bool {
        self.streaming_thinking_index.is_some()
    }

    /// Toggle expand/collapse of a thinking block.
    pub fn toggle_thinking_expand(&mut self, idx: usize) {
        if let Some(TranscriptItem::ThinkingBlock { is_expanded, scroll_offset, .. }) = self.items.get_mut(idx) {
            if *is_expanded {
                *is_expanded = false;
                *scroll_offset = 0;
            } else {
                *is_expanded = true;
                *scroll_offset = 0;
            }
        }
    }

    /// Advance scroll within a thinking block by one page. Returns true if it wrapped around
    /// (collapsed), false if it advanced or stayed.
    pub fn advance_thinking_scroll(&mut self, idx: usize, page_lines: usize) -> bool {
        if let Some(TranscriptItem::ThinkingBlock { is_expanded, scroll_offset, text, is_streaming, .. }) = self.items.get_mut(idx) {
            if !*is_expanded {
                *is_expanded = true;
                if *is_streaming {
                    let wrapped = crate::text::wrap_text(text, 60);
                    let total = wrapped.len().max(1);
                    *scroll_offset = total.saturating_sub(page_lines);
                } else {
                    *scroll_offset = 0;
                }
                return false;
            }
            let total_lines = crate::text::wrap_text(text, 60).len().max(1);
            let next = *scroll_offset + page_lines;
            if next >= total_lines {
                // Wrapped around: collapse
                *is_expanded = false;
                *scroll_offset = 0;
                return true;
            }
            *scroll_offset = next;
            return false;
        }
        false
    }

    /// Update a tool call block (mark as finished, optionally set output).
    pub fn update_tool_call(
        &mut self,
        tool_call_id: &str,
        is_success: bool,
        tool_output: Option<String>,
    ) {
        if let Some(idx) = self.tool_call_index.get(tool_call_id) {
            if let Some(TranscriptItem::ToolCallBlock {
                is_running,
                is_success: succ,
                is_expanded,
                output,
                ..
            }) = self.items.get_mut(*idx)
            {
                *is_running = false;
                *succ = Some(is_success);
                *output = tool_output;
                // Expand on failure, respect default_expanded on success
                if !is_success {
                    *is_expanded = true;
                } else {
                    *is_expanded = self.default_expanded;
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

    /// Estimate the visible range of item indices for a given viewport.
    /// Uses average lines per item type to skip off-screen items.
    /// Returns (start_index, end_index) for items that could be visible.
    pub fn estimate_visible_range(&self, viewport_height: u16) -> (usize, usize) {
        let total = self.items.len();
        if total == 0 {
            return (0, 0);
        }

        // Avg lines per item type (border + content + separator)
        let avg_lines: usize = 7;
        let visible_items = (viewport_height as usize / avg_lines).max(1) + 3;

        let max_offset = total.saturating_sub(1);
        let start = if self.scroll.manual_scroll {
            max_offset.saturating_sub(self.scroll.offset)
        } else {
            max_offset.saturating_sub(visible_items / 2)
        };

        let start = start.min(total.saturating_sub(1));
        let end = (start + visible_items).min(total);
        (start, end)
    }

    /// Return all thinking blocks (with their index).
    pub fn thinking_blocks(&self) -> Vec<(usize, &TranscriptItem)> {
        self.items
            .iter()
            .enumerate()
            .filter(|(_, item)| matches!(item, TranscriptItem::ThinkingBlock { .. }))
            .collect()
    }

    /// Clear all items.
    pub fn clear(&mut self) {
        self.items.clear();
        self.tool_call_index.clear();
        self.streaming_index = None;
        self.streaming_thinking_index = None;
        self.scroll = ScrollState::new();
        self.default_expanded = false;
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
            raw_args: None,
            is_running: true,
            is_success: None,
            is_expanded: false,
            timestamp: Instant::now(),
            output: None,
        });
        t.update_tool_call("tc1", true, None);
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

    #[test]
    fn append_to_thinking_creates_block() {
        let mut t = Transcript::new();
        t.append_to_thinking("Hello");
        t.append_to_thinking(" world");
        assert_eq!(t.len(), 1);
        if let TranscriptItem::ThinkingBlock { text, is_streaming, started_at, .. } = &t.items[0] {
            assert_eq!(text, "Hello world");
            assert!(is_streaming);
            assert!(started_at.is_some());
        } else { panic!("expected ThinkingBlock"); }
    }

    #[test]
    fn finish_thinking_marks_complete_and_collapsed() {
        let mut t = Transcript::new();
        t.append_to_thinking("reasoning");
        assert!(t.thinking_is_streaming());
        t.finish_thinking();
        assert!(!t.thinking_is_streaming());
        if let TranscriptItem::ThinkingBlock { is_streaming, is_expanded, .. } = &t.items[0] {
            assert!(!is_streaming);
            assert!(!is_expanded);
        } else { panic!("expected ThinkingBlock"); }
    }

    #[test]
    fn append_to_thinking_after_finish_creates_new_block() {
        let mut t = Transcript::new();
        t.append_to_thinking("first");
        t.finish_thinking();
        t.append_to_thinking("second");
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn clear_during_thinking_resets_state() {
        let mut t = Transcript::new();
        t.append_to_thinking("thinking");
        assert!(t.thinking_is_streaming());
        t.clear();
        assert!(!t.thinking_is_streaming());
    }

    #[test]
    fn finish_thinking_noop_when_no_streaming() {
        let mut t = Transcript::new();
        t.finish_thinking();
        assert!(!t.thinking_is_streaming());
    }

    #[test]
    fn toggle_thinking_expand() {
        let mut t = Transcript::new();
        t.append_to_thinking("reasoning");
        t.finish_thinking();
        // After finish, auto-collapsed
        if let TranscriptItem::ThinkingBlock { is_expanded, .. } = &t.items[0] {
            assert!(!is_expanded);
        } else { panic!("expected ThinkingBlock"); }
        t.toggle_thinking_expand(0);
        if let TranscriptItem::ThinkingBlock { is_expanded, .. } = &t.items[0] {
            assert!(is_expanded);
        } else { panic!("expected ThinkingBlock"); }
        t.toggle_thinking_expand(0);
        if let TranscriptItem::ThinkingBlock { is_expanded, .. } = &t.items[0] {
            assert!(!is_expanded);
        } else { panic!("expected ThinkingBlock"); }
    }
}
