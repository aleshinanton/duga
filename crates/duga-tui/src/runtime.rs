//! Agent runtime integration for the TUI.
//!
//! Spawns agent runs from the TUI event loop, wiring up the
//! `FrontendEventSink` and cancellation token.

use anyhow::Result;
use duga_config::Config;
use duga_core::loop_context::LoopContext;
use duga_core::loops::SimpleReActLoop;
use duga_core::LoopResult;
use duga_events::EventSink;
use duga_llm::dummy::DummyClient;
use duga_llm::LlmClient;
use duga_runtime::{
    ConfirmationMiddleware, ConfirmationPolicy, FrontendEventSink, build_agent, BuiltRuntime,
};
use duga_core::steering::SteeringReceiver;
use duga_sandbox::{CancellationToken, Workspace};
use duga_types::error::AgentError;
use std::sync::Arc;

use crate::confirmation::{PendingConfirmation, TuiConfirmationProvider};

/// Build and return a `BuiltRuntime` from config.
pub async fn build_runtime(
    config: &Config,
    fe_sink: Arc<FrontendEventSink>,
    confirmation_tx: Option<tokio::sync::mpsc::UnboundedSender<PendingConfirmation>>,
) -> Result<BuiltRuntime> {
    let llm: Arc<dyn LlmClient> = match duga_runtime::providers::build_llm(
        config.provider.as_deref().unwrap_or("dummy"),
        &config.model,
        config,
    ) {
        Ok(client) => client,
        Err(e) => {
            tracing::warn!("LLM build failed ({e}), using dummy client for testing");
            Arc::new(DummyClient::default())
        }
    };

    let workspace = Arc::new(Workspace::open(&config.workspace.root)?);
    let dispatcher = duga_runtime::tools::build_dispatcher(
        config,
        workspace.clone(),
        vec![],
    )?;

    // Wire up confirmation middleware if tools are configured for it.
    if let Some(confirm_tx) = confirmation_tx {
        let tui_cfg = config.tui.as_ref();
        let require: Vec<String> = tui_cfg
            .map(|c| c.require_confirmation_for.clone())
            .unwrap_or_default();
        if !require.is_empty() {
            let policy = ConfirmationPolicy::new(
                require,
                config.frontend.confirmation_timeout,
            );
            let provider = Arc::new(TuiConfirmationProvider::new(confirm_tx));
            dispatcher.set_confirmation(ConfirmationMiddleware::new(policy, provider));
        }
    }

    let sinks: Vec<Arc<dyn EventSink>> = vec![fe_sink];
    let runtime = build_agent(config, llm, dispatcher, workspace, sinks, None)?;

    Ok(runtime)
}

/// Run the agent loop for a given task, sending events through the sink.
pub async fn run_agent(
    config: &Config,
    task: String,
    fe_sink: Arc<FrontendEventSink>,
    cancellation: CancellationToken,
    steer_rx: Option<SteeringReceiver>,
    confirmation_tx: Option<tokio::sync::mpsc::UnboundedSender<PendingConfirmation>>,
) -> Result<LoopResult, AgentError> {
    let mut runtime = build_runtime(config, fe_sink, confirmation_tx)
        .await
        .map_err(|e| AgentError::EventSinkFailed(format!("failed to build runtime: {e}")))?;

    let mut ctx = LoopContext {
        config: &runtime.config.agent,
        memory: &mut runtime.memory,
        llm: &runtime.llm,
        tools: &runtime.dispatcher,
        workspace: &runtime.workspace,
        event_sink: &runtime.event_sink,
        summarizer: &runtime.summarizer,
        cancellation: &cancellation,
        registry: &runtime.registry,
        max_refinement_iterations: runtime.config.agent.loop_config.max_refinement_iterations,
        max_delegation_depth: runtime.config.agent.loop_config.max_delegation_depth,
        delegation_depth: 0,
        steer: steer_rx,
        steer_limits: None,
    };

    // Select the loop from the registry based on config.
    let tui_cfg = config.tui.as_ref();
    let loop_id = tui_cfg.map(|c| c.default_loop.as_str()).unwrap_or("simple_react");
    let loop_impl = runtime
        .registry
        .get(loop_id)
        .unwrap_or_else(|| {
            tracing::warn!(
                "configured loop '{}' not found in registry, falling back to simple_react",
                loop_id
            );
            &SimpleReActLoop
        });
    loop_impl.run(task, &mut ctx).await
}
