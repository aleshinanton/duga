//! duga-sandbox: capability-based filesystem isolation and secure process execution.
//!
//! This crate provides:
//! - `Workspace` — capability-based directory wrapper using `cap_std`
//! - `BinaryRegistry` — resolved binary path allowlist
//! - `SanitizedEnv` — filtered subprocess environment builder
//! - `ShellSession` — persistent shell state (cwd, env) without a running shell process
//! - `run_captured()` — the single process-spawning function with output limits and cancellation

pub mod workspace;
pub mod binary_registry;
pub mod env;
pub mod shell_session;
pub mod exec;
pub mod error;

pub use error::{BinaryError, ShellSessionError, WorkspaceError};
pub use exec::{run_captured, CancellationToken};
pub use shell_session::{SessionCommand, ShellSession};
pub use workspace::Workspace;