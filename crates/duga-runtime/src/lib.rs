//! Shared runtime composition for duga frontends.
//!
//! This crate extracts neutral infrastructure used by CLI, Telegram, and TUI
//! frontends: provider resolution, tool/plugin registration, memory/summarizer
//! wiring, event bridges, and confirmation middleware.

pub mod agent;
pub mod confirmation;
pub mod events;
pub mod memory_context;
pub mod providers;
pub mod skills;
pub mod summarizer;
pub mod tools;

pub use agent::{build_agent, sandbox_environment_context, tool_guidance, BuiltRuntime};
pub use confirmation::{
    ConfirmationDecision, ConfirmationMiddleware, ConfirmationPolicy, ConfirmationProvider,
    ConfirmationRequest, MiddlewareDecision,
};
pub use events::{FrontendEvent, FrontendEventBridge, FrontendEventSink};
pub use memory_context::{load_persistent_memory, format_memory_for_prompt, PersistentMemory};
pub use providers::{build_llm, resolve_provider, ProviderSelection};
pub use skills::{load_skills, format_skills_for_prompt, Skill, SkillSource};
pub use tools::build_dispatcher;
