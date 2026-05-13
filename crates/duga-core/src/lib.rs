//! Core runtime primitives for duga.

pub mod memory;
pub mod summarizer;

pub use memory::Memory;
pub use summarizer::{Summarizer, SummaryFuture};
