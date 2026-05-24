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
use std::path::PathBuf;
use std::sync::Arc;

/// Build a fully-populated `ToolDispatcher` with built-in tools and plugins.
pub fn build_dispatcher(
    config: &Config,
    workspace: Arc<Workspace>,
    aux_roots: Vec<PathBuf>,
) -> Result<Arc<ToolDispatcher>> {
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
        aux_roots,
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

#[cfg(test)]
mod tests {
    use super::*;
    use duga_config::SandboxMode as ConfigSandboxMode;
    use duga_sandbox::executor::SandboxMode;

    #[test]
    fn sandbox_mode_host() {
        assert_eq!(
            sandbox_mode_from_config(&ConfigSandboxMode::Host),
            SandboxMode::Host
        );
    }

    #[test]
    fn sandbox_mode_capability() {
        assert_eq!(
            sandbox_mode_from_config(&ConfigSandboxMode::Capability),
            SandboxMode::Capability
        );
    }

    #[test]
    fn sandbox_mode_docker() {
        assert_eq!(
            sandbox_mode_from_config(&ConfigSandboxMode::Docker),
            SandboxMode::Docker
        );
    }

    #[test]
    fn build_dispatcher_allow_all_binaries() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = format!(
            r#"
model: "dummy/test"
agent:
  limits:
    max_steps: 5
    max_tool_calls: 10
    max_runtime: 60s
    retry_on_error: 1
  features:
    streaming: false
  output:
    max_stdout_bytes: 4096
    max_stderr_bytes: 4096
    max_combined_bytes: 8192
  think:
    max_calls: 2
    max_tokens: 128
sandbox:
  mode: host
  allow_all_binaries: true
  timeout: 30s
workspace:
  root: {root}
environment:
  allowed:
    - HOME
memory:
  max_tokens: 4096
  compress_at_ratio: 0.8
  context_window_size: 50
  max_context_tokens: 12000
  summarizer: simple
plugins:
  dir: {plugin_dir}
  modules: []
"#,
            root = dir.path().display(),
            plugin_dir = dir.path().display(),
        );
        let config_path = dir.path().join("config.yaml");
        std::fs::write(&config_path, yaml).unwrap();
        let config = duga_config::Config::load(&config_path).expect("test config must parse");

        let ws_dir = tempfile::tempdir().unwrap();
        let workspace = std::sync::Arc::new(
            duga_sandbox::Workspace::open(ws_dir.path()).unwrap(),
        );

        let result = build_dispatcher(&config, workspace, vec![]);
        assert!(result.is_ok(), "build_dispatcher should succeed: {:?}", result.err());
        let dispatcher = result.unwrap();
        let names = dispatcher.names();
        assert!(names.contains(&"shell".to_string()), "should have shell tool");
        assert!(names.contains(&"read".to_string()), "should have read tool");
        assert!(names.contains(&"write".to_string()), "should have write tool");
        assert!(names.contains(&"think".to_string()), "should have think tool");
        assert!(names.contains(&"search".to_string()), "should have search tool");
    }
}
