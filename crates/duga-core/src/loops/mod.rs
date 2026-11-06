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

/// Bound the entire strategy, including events and summarization, not just API calls.
pub(crate) async fn bounded_run<F>(
    remaining: std::time::Duration,
    cancellation: duga_sandbox::CancellationToken,
    future: F,
) -> Result<crate::LoopResult, duga_types::error::AgentError>
where
    F: std::future::Future<Output = Result<crate::LoopResult, duga_types::error::AgentError>>,
{
    use duga_types::error::AgentError;
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err(AgentError::Cancelled),
        result = tokio::time::timeout(remaining, future) => match result {
            Ok(result) => result,
            Err(_) => { cancellation.cancel(); Err(AgentError::Timeout) }
        }
    }
}

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
