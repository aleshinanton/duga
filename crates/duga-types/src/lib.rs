//! Core wire and on-the-loop datatypes for the duga agent runtime.
//!
//! This crate defines every type that flows across system boundaries:
//! messages between memory ↔ event system ↔ tools,
//! configuration, errors, and the fundamental building blocks
//! that the rest of the runtime depends on.
//!
//! **Design principles:**
//! - Zero logic, zero I/O, zero side-effects.
//! - Pure data types with serialization roundtrip guarantees.
//! - Every other crate depends on this one — keep it stable.

pub mod message;
pub mod tool_call;
pub mod tool_schema;
pub mod error;
pub mod config;
pub mod llm;
pub mod summary;
pub mod seq;