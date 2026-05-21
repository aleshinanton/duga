//! Tool dispatcher construction shared by all frontends.
//!
//! Registers built-in tools (shell, read, write, search, think, delegate) and
//! loads WASM plugins into a single `ToolDispatcher`.

use anyhow::{Context, Result};
use duga_config::{Config, SandboxMode as ConfigSandboxMode};
use duga_plugin_host::load_plugins;
use duga_sandbox::binary_registry::{BinaryPattern, BinaryRegistry};
use duga_sandbox::executor::SandboxMode;
use duga_sandbox::{SandboxExecutor, Workspace};
use duga_tools::{ErasedTool, ToolDispatcher};
use duga_tools_builtin::delegate::DelegateTool;
use std::sync::Arc;

/// Build a fully-populated `ToolDispatcher` with built-in tools and plugins.
pub fn build_dispatcher(config: &Config, workspace: Arc<Workspace>) -> Result<Arc<ToolDispatcher>> {
    let registry = if config.sandbox.allow_all_binaries {
        Arc::new(BinaryRegistry::allow_all())
    } else {
        let patterns: Vec<BinaryPattern> = config
            .sandbox
            .allowed_binaries
            .iter()
            .map(|s| {
                BinaryPattern::parse(s)
                    .with_context(|| format!("invalid binary entry: {}", s))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Arc::new(
            BinaryRegistry::from_patterns(&patterns)
                .context("building binary registry")?,
        )
    };
    let dispatcher = Arc::new(ToolDispatcher::new());
    let executor = Arc::new(
        SandboxExecutor::from_config(
            &sandbox_mode_from_config(&config.sandbox.mode),
            config.sandbox.container.as_deref(),
            config.sandbox.workspace_mount.as_deref(),
        )
        .context("building sandbox executor")?,
    );

    duga_tools_builtin::register_builtin_tools(
        &dispatcher,
        registry,
        workspace.clone(),
        config.agent.output.clone(),
        config.sandbox.timeout,
        config.agent.think.clone(),
        executor,
    )
    .context("registering built-in tools")?;

    // Register the delegate tool (schema-only placeholder for LLM visibility).
    // Actual delegation is handled by SimpleReActLoop's intercept.
    let delegate_tool = DelegateTool::new(config.agent.loop_config.enabled_loops.clone());
    dispatcher
        .register_erased(ErasedTool::erase(delegate_tool))
        .context("registering delegate tool")?;

    let plugin_registry = load_plugins(
        &config.plugins.dir,
        &config.plugins.modules,
        &workspace,
        config.agent.output.clone(),
        config.sandbox.timeout,
    )
    .context("loading plugins")?;

    for plugin in plugin_registry.into_plugins() {
        dispatcher
            .register_erased(ErasedTool::erase(plugin))
            .context("registering plugin")?;
    }

    Ok(dispatcher)
}

fn sandbox_mode_from_config(mode: &ConfigSandboxMode) -> SandboxMode {
    match mode {
        ConfigSandboxMode::Host => SandboxMode::Host,
        ConfigSandboxMode::Capability => SandboxMode::Capability,
        ConfigSandboxMode::Docker => SandboxMode::Docker,
    }
}
