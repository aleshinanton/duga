//! Tool adapter for loaded plugins.

use crate::capabilities::WasiCapabilities;
use duga_plugin_abi::{Invocation, Outcome, ToolInfo};
use duga_tools::context::ToolContext;
use duga_tools::result::ToolCallResult;
use duga_tools::Tool;
use duga_types::config::OutputLimits;
use duga_types::error::ToolError;
use duga_types::tool_call::CallId;
use duga_types::tool_result::ToolResult;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub trait PluginExecutor: Send + Sync {
    fn execute(&self, invocation: Invocation) -> Result<Outcome, String>;
}

#[derive(Clone, Debug)]
pub struct StaticPluginExecutor {
    outcome: Outcome,
}

impl StaticPluginExecutor {
    pub fn new(outcome: Outcome) -> Self {
        Self { outcome }
    }
}

impl PluginExecutor for StaticPluginExecutor {
    fn execute(&self, _invocation: Invocation) -> Result<Outcome, String> {
        Ok(self.outcome.clone())
    }
}

#[derive(Clone)]
pub struct WasmPluginAdapter {
    module_path: PathBuf,
    info: ToolInfo,
    args_schema_json: Value,
    capabilities: WasiCapabilities,
    output_limits: OutputLimits,
    timeout: Duration,
    executor: Arc<dyn PluginExecutor>,
}

impl std::fmt::Debug for WasmPluginAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmPluginAdapter")
            .field("module_path", &self.module_path)
            .field("name", &self.info.name)
            .field("capabilities", &self.capabilities)
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl WasmPluginAdapter {
    pub fn new(
        module_path: impl Into<PathBuf>,
        info: ToolInfo,
        capabilities: WasiCapabilities,
        output_limits: OutputLimits,
        timeout: Duration,
        executor: Arc<dyn PluginExecutor>,
    ) -> Result<Self, ToolError> {
        let args_schema_json = serde_json::from_str(&info.args_schema)
            .map_err(|e| ToolError::Plugin(format!("invalid plugin args schema: {e}")))?;
        Ok(Self {
            module_path: module_path.into(),
            info,
            args_schema_json,
            capabilities,
            output_limits,
            timeout,
            executor,
        })
    }

    pub fn module_path(&self) -> &Path {
        &self.module_path
    }

    pub fn capabilities(&self) -> &WasiCapabilities {
        &self.capabilities
    }

    pub fn info(&self) -> &ToolInfo {
        &self.info
    }

    fn outcome_to_result(&self, outcome: Outcome, started: Instant) -> ToolResult {
        let mut output = outcome.output;
        let mut truncated = false;
        if output.len() as u64 > self.output_limits.max_combined_bytes {
            output.truncate(self.output_limits.max_combined_bytes as usize);
            truncated = true;
        }
        let metadata = serde_json::from_str(&outcome.metadata_json)
            .unwrap_or_else(|_| serde_json::json!({ "raw": outcome.metadata_json }));

        ToolResult {
            tool_call_id: CallId::new(),
            success: outcome.success,
            stdout_bytes: output.len() as u64,
            stderr_bytes: 0,
            output,
            metadata,
            duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
            truncated,
        }
    }
}

impl Tool for WasmPluginAdapter {
    type Args = Value;

    fn name(&self) -> &str {
        &self.info.name
    }

    fn description(&self) -> &str {
        &self.info.description
    }

    fn json_schema(&self) -> Value {
        self.args_schema_json.clone()
    }

    async fn execute(&self, ctx: ToolContext<'_>, args: Self::Args) -> ToolCallResult {
        let started = Instant::now();
        if ctx.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        if !self.capabilities.workspace_fs {
            return Err(ToolError::Plugin(
                "plugin denied workspace filesystem capability".into(),
            ));
        }

        let invocation = Invocation {
            args_json: serde_json::to_string(&args)
                .map_err(|e| ToolError::InvalidArgs(e.to_string()))?,
            workspace_root: ".".into(),
        };
        let executor = self.executor.clone();
        let timeout = self.timeout;

        let outcome = tokio::time::timeout(
            timeout,
            tokio::task::spawn_blocking(move || executor.execute(invocation)),
        )
        .await
        .map_err(|_| ToolError::Timeout)?
        .map_err(|e| ToolError::Plugin(format!("plugin task failed: {e}")))?
        .map_err(ToolError::Plugin)?;

        Ok(self.outcome_to_result(outcome, started))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use duga_sandbox::{CancellationToken, Workspace};
    use duga_tools::event_sink::NullSink;

    fn adapter() -> WasmPluginAdapter {
        WasmPluginAdapter::new(
            "format-code.wasm",
            ToolInfo {
                name: "format-code".into(),
                description: "format code".into(),
                args_schema: serde_json::json!({
                    "type": "object",
                    "properties": { "file": { "type": "string" } },
                    "required": ["file"]
                })
                .to_string(),
            },
            WasiCapabilities::default(),
            OutputLimits::default(),
            Duration::from_secs(1),
            Arc::new(StaticPluginExecutor::new(Outcome {
                success: true,
                output: "formatted".into(),
                metadata_json: "{}".into(),
            })),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn adapter_executes_and_maps_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = Workspace::open(dir.path()).unwrap();
        let sink = NullSink;
        let result = adapter()
            .execute(
                ToolContext::new(&workspace, CancellationToken::new(), &sink),
                serde_json::json!({ "file": "src/lib.rs" }),
            )
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.output, "formatted");
    }

    #[tokio::test]
    async fn adapter_denies_workspace_when_capability_disabled() {
        let mut adapter = adapter();
        adapter.capabilities.workspace_fs = false;
        let dir = tempfile::tempdir().unwrap();
        let workspace = Workspace::open(dir.path()).unwrap();
        let sink = NullSink;

        let err = adapter
            .execute(
                ToolContext::new(&workspace, CancellationToken::new(), &sink),
                serde_json::json!({ "file": "src/lib.rs" }),
            )
            .await
            .unwrap_err();

        assert!(matches!(err, ToolError::Plugin(message) if message.contains("denied")));
    }
}
