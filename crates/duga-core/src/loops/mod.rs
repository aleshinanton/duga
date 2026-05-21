pub mod decomposition;
pub mod problem_solving;
pub mod search_loop;
pub mod simple_react;
pub mod verification;

pub use decomposition::DecompositionLoop;
pub use problem_solving::ProblemSolvingLoop;
pub use search_loop::SearchLoop;
pub use simple_react::SimpleReActLoop;
pub use verification::VerificationLoop;

/// Register all default loop implementations in the given registry.
///
/// This registers the four specialized loops. `SimpleReActLoop` is always
/// registered separately as the entry point.
pub fn register_default_loops(registry: &mut crate::LoopRegistry) {
    let _ = registry.register(Box::new(ProblemSolvingLoop));
    let _ = registry.register(Box::new(VerificationLoop));
    let _ = registry.register(Box::new(DecompositionLoop));
    let _ = registry.register(Box::new(SearchLoop));
}
