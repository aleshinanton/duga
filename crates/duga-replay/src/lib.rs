//! Replay log parsing and test doubles.

pub mod mock_llm;
pub mod reader;

pub use mock_llm::ReplayMockLlm;
pub use reader::{JsonlReader, ReplayError};

use duga_events::{Event, StoredEvent};

pub fn final_text(events: &[StoredEvent]) -> Option<String> {
    events.iter().rev().find_map(|event| match &event.event {
        Event::AgentFinished { text } => text.clone(),
        _ => None,
    })
}
