//! Built-in tools for duga: read, write, bash, search, think.
//!
//! This crate implements all five built-in tools specified in §11 of the architecture.
//! Each tool implements the `Tool` trait and uses the security primitives from
//! `duga-sandbox` (Workspace, BinaryRegistry, ShellSession, run_captured).

pub mod read;
pub mod write;
pub mod bash;
pub mod search;
pub mod think;

#[cfg(test)]
mod tests;
