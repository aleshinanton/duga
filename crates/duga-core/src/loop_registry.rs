//! Loop registry — maps `loop_type_id → Box<dyn Loop>`.
//!
//! Registered at startup, never modified at runtime. The registry is
//! purely a lookup structure — no classifier logic lives here.

use crate::loop_trait::Loop;
use std::collections::HashMap;

/// Error returned when registration fails (e.g. duplicate id).
#[derive(Clone, Debug, PartialEq)]
pub enum LoopRegistryError {
    DuplicateId { id: String },
}

impl std::fmt::Display for LoopRegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoopRegistryError::DuplicateId { id } => {
                write!(f, "duplicate loop id in registry: {}", id)
            }
        }
    }
}

impl std::error::Error for LoopRegistryError {}

/// A registry mapping loop id → loop implementation.
///
/// Populated at startup and never modified thereafter.  The delegate tool
/// and `SimpleReActLoop` both consult the registry to look up loop
/// implementations during delegation.
pub struct LoopRegistry {
    loops: HashMap<String, Box<dyn Loop>>,
}

impl LoopRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            loops: HashMap::new(),
        }
    }

    /// Register a loop implementation.
    ///
    /// Returns an error if a loop with the same `id()` is already registered.
    pub fn register(&mut self, loop_impl: Box<dyn Loop>) -> Result<(), LoopRegistryError> {
        let id = loop_impl.id().to_string();
        if self.loops.contains_key(&id) {
            return Err(LoopRegistryError::DuplicateId { id });
        }
        self.loops.insert(id, loop_impl);
        Ok(())
    }

    /// Look up a loop by its id.
    pub fn get(&self, id: &str) -> Option<&dyn Loop> {
        self.loops.get(id).map(|b| b.as_ref())
    }

    /// Check whether a given loop id is both registered AND listed as enabled.
    pub fn is_enabled(&self, id: &str, enabled_ids: &[String]) -> bool {
        self.loops.contains_key(id) && enabled_ids.iter().any(|e| e == id)
    }

    /// Build the "Available Strategies" section of the system prompt.
    ///
    /// Only includes loops whose ids appear in `enabled_ids`.  Each
    /// enabled loop contributes a single line of the form
    /// `- id — description`.
    pub fn build_strategies_prompt(&self, enabled_ids: &[String]) -> String {
        if enabled_ids.is_empty() {
            return "No specialized loops available. Proceed with your standard \
                    tools (think, shell, read, edit, write, search)."
                .to_string();
        }

        let mut lines = vec!["## Available Strategies".to_string()];
        lines.push(String::new());
        lines.push(
            "You have access to a `delegate` tool that hands control to a \
             specialized loop. Only use it for genuinely complex tasks that \
             benefit from a structured approach. For simple questions, \
             calculations, lookups, or single-step tasks, use your normal \
             tools directly — delegation adds overhead."
                .to_string(),
        );
        lines.push(String::new());
        lines.push(
            "IMPORTANT: The `reason` field must describe the actual current task \
             in 3-5 words. Never reference previous conversations or unrelated \
             tasks. It is used for logging only."
                .to_string(),
        );
        lines.push(String::new());
        lines.push("Available loop types:".to_string());

        for enabled_id in enabled_ids {
            if let Some(loop_impl) = self.loops.get(enabled_id) {
                lines.push(format!(
                    "- {} — {}",
                    loop_impl.id(),
                    loop_impl.description()
                ));
            }
        }

        lines.push(String::new());
        lines.push(
            "For simple tasks (arithmetic, file reads, single edits, quick lookups), \
             do NOT delegate — just answer directly with your tools. Delegation is \
             only for tasks that genuinely need a multi-phase structured approach."
                .to_string(),
        );

        lines.join("\n")
    }

    /// Return the number of registered loops.
    pub fn len(&self) -> usize {
        self.loops.len()
    }

    /// Return true if the registry has no loops.
    pub fn is_empty(&self) -> bool {
        self.loops.is_empty()
    }
}

impl Default for LoopRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loop_trait::LoopRunFuture;
    use crate::LoopContext;
    use duga_types::message::AssistantMessage;

    /// A mock loop used only in registry tests.
    struct MockLoop {
        id: &'static str,
        desc: &'static str,
    }

    impl Loop for MockLoop {
        fn id(&self) -> &'static str {
            self.id
        }

        fn name(&self) -> &'static str {
            self.id // reuse id as name for simplicity
        }

        fn description(&self) -> &'static str {
            self.desc
        }

        fn run<'a>(
            &'a self,
            _task: String,
            _ctx: &'a mut LoopContext<'a>,
        ) -> LoopRunFuture<'a> {
            Box::pin(async {
                Ok(crate::LoopResult {
                    message: AssistantMessage {
                        text: Some("mock done".into()),
                        tool_calls: vec![],
                        reasoning_content: None,
                    },
                    steps: 0,
                    tool_calls: 0,
                    loop_id: self.id.to_string(),
                })
            })
        }
    }

    #[test]
    fn empty_registry_is_empty() {
        let reg = LoopRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        assert!(reg.get("anything").is_none());
    }

    #[test]
    fn register_and_lookup() {
        let mut reg = LoopRegistry::new();

        let mock = MockLoop {
            id: "alpha",
            desc: "Alpha strategy",
        };
        reg.register(Box::new(mock)).unwrap();

        assert_eq!(reg.len(), 1);
        let found = reg.get("alpha").unwrap();
        assert_eq!(found.id(), "alpha");
        assert_eq!(found.description(), "Alpha strategy");
    }

    #[test]
    fn register_multiple() {
        let mut reg = LoopRegistry::new();

        reg.register(Box::new(MockLoop {
            id: "alpha",
            desc: "Alpha",
        }))
        .unwrap();
        reg.register(Box::new(MockLoop {
            id: "beta",
            desc: "Beta",
        }))
        .unwrap();

        assert_eq!(reg.len(), 2);
        assert!(reg.get("alpha").is_some());
        assert!(reg.get("beta").is_some());
    }

    #[test]
    fn register_duplicate_is_error() {
        let mut reg = LoopRegistry::new();
        reg.register(Box::new(MockLoop {
            id: "dup",
            desc: "first",
        }))
        .unwrap();

        let err = reg
            .register(Box::new(MockLoop {
                id: "dup",
                desc: "second",
            }))
            .unwrap_err();

        assert_eq!(
            err,
            LoopRegistryError::DuplicateId {
                id: "dup".into()
            }
        );
    }

    #[test]
    fn get_missing_returns_none() {
        let mut reg = LoopRegistry::new();
        reg.register(Box::new(MockLoop {
            id: "x",
            desc: "desc",
        }))
        .unwrap();

        assert!(reg.get("y").is_none());
    }

    #[test]
    fn is_enabled_checks_both_registration_and_config() {
        let mut reg = LoopRegistry::new();
        reg.register(Box::new(MockLoop {
            id: "a",
            desc: "a",
        }))
        .unwrap();
        reg.register(Box::new(MockLoop {
            id: "b",
            desc: "b",
        }))
        .unwrap();

        let enabled: Vec<String> = vec!["a".into(), "c".into()]; // c is not registered

        assert!(reg.is_enabled("a", &enabled));
        // b is registered but not enabled
        assert!(!reg.is_enabled("b", &enabled));
        // c is enabled but not registered
        assert!(!reg.is_enabled("c", &enabled));
        // d is neither
        assert!(!reg.is_enabled("d", &enabled));
    }

    #[test]
    fn build_strategies_prompt_empty_enabled() {
        let reg = LoopRegistry::new();
        let prompt = reg.build_strategies_prompt(&[]);
        assert!(prompt.contains("No specialized loops available"));
        assert!(!prompt.contains("Available Strategies"));
    }

    #[test]
    fn build_strategies_prompt_includes_enabled_only() {
        let mut reg = LoopRegistry::new();
        reg.register(Box::new(MockLoop {
            id: "id1",
            desc: "Description 1",
        }))
        .unwrap();
        reg.register(Box::new(MockLoop {
            id: "id2",
            desc: "Description 2",
        }))
        .unwrap();
        reg.register(Box::new(MockLoop {
            id: "id3",
            desc: "Description 3",
        }))
        .unwrap();

        // Only enable id1 and id3.
        let enabled: Vec<String> = vec!["id1".into(), "id3".into()];
        let prompt = reg.build_strategies_prompt(&enabled);

        assert!(prompt.contains("id1"));
        assert!(prompt.contains("Description 1"));
        assert!(prompt.contains("id3"));
        assert!(prompt.contains("Description 3"));
        assert!(!prompt.contains("id2"));
        assert!(prompt.contains("delegate"));
    }

    #[test]
    fn build_strategies_prompt_ignores_unknown_enabled_ids() {
        let mut reg = LoopRegistry::new();
        reg.register(Box::new(MockLoop {
            id: "known",
            desc: "Known",
        }))
        .unwrap();

        let enabled: Vec<String> = vec!["known".into(), "unknown".into()];
        let prompt = reg.build_strategies_prompt(&enabled);

        assert!(prompt.contains("known"));
        assert!(!prompt.contains("unknown"));
    }
}
