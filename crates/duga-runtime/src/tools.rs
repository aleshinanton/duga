//! Tool dispatcher construction shared by all frontends.
//!
//! Registers built-in tools (bash, read, write, search, think) and loads
//! WASM plugins into a single `ToolDispatcher`.

use anyhow::{Context, Result};
use duga_config::Config;
use duga_plugin_host::load_plugins;
use duga_sandbox::binary_registry::BinaryRegistry;
use duga_sandbox::Workspace;
use duga_tools::{ErasedTool, ToolDispatcher};
use std::sync::Arc;

/// Build a fully-populated `ToolDispatcher` with built-in tools and plugins.
pub fn build_dispatcher(
    config: &Config,
    workspace: Arc<Workspace>,
) -> Result<Arc<ToolDispatcher>> {
    let registry = Arc::new(
        BinaryRegistry::from_paths(&config.sandbox.allowed_binaries)
            .context("building binary registry")?,
    );
    let dispatcher = Arc::new(ToolDispatcher::new());

    duga_tools_builtin::register_builtin_tools(
        &dispatcher,
        registry,
        workspace.clone(),
        config.agent.output.clone(),
        config.sandbox.timeout,
        config.agent.think.clone(),
    )
    .context("registering built-in tools")?;

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
