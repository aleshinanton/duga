//! Shared plugin ABI types plus the canonical WIT contract.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const WIT: &str = include_str!("../wit/plugin.wit");
pub const PACKAGE: &str = "duga:plugin@0.1.0";

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub args_schema: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct Invocation {
    pub args_json: String,
    pub workspace_root: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct Outcome {
    pub success: bool,
    pub output: String,
    pub metadata_json: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct PluginModuleConfig {
    pub name: String,
    #[serde(default)]
    pub capabilities: WasiCapabilities,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct WasiCapabilities {
    #[serde(default = "default_true")]
    pub workspace_fs: bool,
    #[serde(default)]
    pub network: bool,
    #[serde(default)]
    pub process_spawn: bool,
    #[serde(default = "default_true")]
    pub randomness: bool,
    #[serde(default = "default_true")]
    pub clocks: bool,
}

impl Default for WasiCapabilities {
    fn default() -> Self {
        Self {
            workspace_fs: true,
            network: false,
            process_spawn: false,
            randomness: true,
            clocks: true,
        }
    }
}

fn default_true() -> bool {
    true
}

#[cfg(feature = "host")]
pub mod host {
    pub use crate::{Invocation, Outcome, ToolInfo, WasiCapabilities, WIT};
}

#[cfg(feature = "guest")]
pub mod guest {
    pub use crate::{Invocation, Outcome, ToolInfo, WasiCapabilities, WIT};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wit_contains_expected_world() {
        assert!(WIT.contains("package duga:plugin@0.1.0"));
        assert!(WIT.contains("world plugin"));
        assert!(WIT.contains("execute: func"));
    }

    #[test]
    fn capabilities_default_to_safe_matrix() {
        let caps = WasiCapabilities::default();
        assert!(caps.workspace_fs);
        assert!(!caps.network);
        assert!(!caps.process_spawn);
        assert!(caps.randomness);
        assert!(caps.clocks);
    }

    #[test]
    fn partial_capabilities_deserialize_with_defaults() {
        let caps: WasiCapabilities = serde_json::from_value(serde_json::json!({
            "workspace_fs": false
        }))
        .unwrap();

        assert!(!caps.workspace_fs);
        assert!(!caps.network);
        assert!(!caps.process_spawn);
        assert!(caps.randomness);
        assert!(caps.clocks);
    }
}
