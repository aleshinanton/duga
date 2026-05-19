//! Plugin enumeration and validation.

use crate::{StaticPluginExecutor, WasiCapabilities, WasmPluginAdapter};
use duga_plugin_abi::{Outcome, PluginModuleConfig, ToolInfo};
use duga_sandbox::Workspace;
use duga_types::config::OutputLimits;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub const BUILTIN_NAMES: &[&str] = &["read", "write", "edit", "shell", "search", "think"];

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("plugin directory not found: {0}")]
    DirectoryNotFound(PathBuf),
    #[error("not a component model wasm module: {0}")]
    NotAComponent(PathBuf),
    #[error("plugin info failed for {path}: {message}")]
    InfoFailed { path: PathBuf, message: String },
    #[error("plugin name collision: {0}")]
    NameCollision(String),
    #[error("plugin load failed for {path}: {message}")]
    LoadFailed { path: PathBuf, message: String },
}

#[derive(Debug, Default)]
pub struct PluginRegistry {
    plugins: Vec<WasmPluginAdapter>,
}

impl PluginRegistry {
    pub fn new(plugins: Vec<WasmPluginAdapter>) -> Self {
        Self { plugins }
    }

    pub fn plugins(&self) -> &[WasmPluginAdapter] {
        &self.plugins
    }

    pub fn into_plugins(self) -> Vec<WasmPluginAdapter> {
        self.plugins
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }
}

pub fn load_plugins(
    dir: &Path,
    modules: &[PluginModuleConfig],
    _workspace: &Workspace,
    output_limits: OutputLimits,
    timeout: Duration,
) -> Result<PluginRegistry, PluginError> {
    if !dir.exists() {
        return Ok(PluginRegistry::default());
    }
    if !dir.is_dir() {
        return Err(PluginError::DirectoryNotFound(dir.to_path_buf()));
    }

    let mut module_config = modules
        .iter()
        .map(|module| (module.name.clone(), module.capabilities.clone()))
        .collect::<std::collections::HashMap<_, _>>();
    let mut seen = HashSet::new();
    let mut plugins = Vec::new();

    for entry in std::fs::read_dir(dir).map_err(|e| PluginError::LoadFailed {
        path: dir.to_path_buf(),
        message: e.to_string(),
    })? {
        let entry = entry.map_err(|e| PluginError::LoadFailed {
            path: dir.to_path_buf(),
            message: e.to_string(),
        })?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("wasm") {
            continue;
        }

        validate_component_header(&path)?;
        let info = read_sidecar_info(&path)?;
        let name = info.name.trim();
        if name.is_empty() || name != info.name {
            tracing::warn!(path = %path.display(), "Skipping plugin with invalid name");
            continue;
        }
        if BUILTIN_NAMES.contains(&name) || !seen.insert(info.name.clone()) {
            tracing::warn!(plugin = %info.name, "Skipping plugin with colliding name");
            continue;
        }

        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();
        let capabilities = module_config
            .remove(&file_name)
            .or_else(|| module_config.remove(&info.name))
            .unwrap_or_else(WasiCapabilities::default);
        let adapter = WasmPluginAdapter::new(
            &path,
            info,
            capabilities,
            output_limits.clone(),
            timeout,
            Arc::new(StaticPluginExecutor::new(Outcome {
                success: true,
                output: "plugin executed".into(),
                metadata_json: "{}".into(),
            })),
        )
        .map_err(|e| PluginError::InfoFailed {
            path: path.clone(),
            message: e.to_string(),
        })?;
        plugins.push(adapter);
    }

    Ok(PluginRegistry::new(plugins))
}

fn validate_component_header(path: &Path) -> Result<(), PluginError> {
    let bytes = std::fs::read(path).map_err(|e| PluginError::LoadFailed {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    if bytes.len() < 8 || &bytes[..4] != b"\0asm" {
        return Err(PluginError::NotAComponent(path.to_path_buf()));
    }
    let version = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    if version == 1 {
        return Err(PluginError::NotAComponent(path.to_path_buf()));
    }
    Ok(())
}

fn read_sidecar_info(path: &Path) -> Result<ToolInfo, PluginError> {
    let sidecar = path.with_extension("json");
    if sidecar.exists() {
        let raw = std::fs::read_to_string(&sidecar).map_err(|e| PluginError::InfoFailed {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        return serde_json::from_str(&raw).map_err(|e| PluginError::InfoFailed {
            path: path.to_path_buf(),
            message: e.to_string(),
        });
    }

    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| PluginError::InfoFailed {
            path: path.to_path_buf(),
            message: "plugin file has no valid stem".into(),
        })?;
    Ok(ToolInfo {
        name: stem.into(),
        description: format!("WASM plugin {stem}"),
        args_schema: serde_json::json!({
            "type": "object",
            "additionalProperties": true
        })
        .to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use duga_tools::Tool;

    fn component_bytes() -> Vec<u8> {
        b"\0asm\r\0\0\0".to_vec()
    }

    #[test]
    fn empty_missing_dir_is_empty_registry() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("plugins");
        let workspace = Workspace::open(dir.path()).unwrap();

        let registry = load_plugins(
            &missing,
            &[],
            &workspace,
            OutputLimits::default(),
            Duration::from_secs(1),
        )
        .unwrap();

        assert!(registry.is_empty());
    }

    #[test]
    fn invalid_wasm_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.wasm");
        std::fs::write(&path, b"not wasm").unwrap();
        let workspace = Workspace::open(dir.path()).unwrap();

        let err = load_plugins(
            dir.path(),
            &[],
            &workspace,
            OutputLimits::default(),
            Duration::from_secs(1),
        )
        .unwrap_err();

        assert!(matches!(err, PluginError::NotAComponent(_)));
    }

    #[test]
    fn loads_component_with_sidecar_info() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("format-code.wasm");
        std::fs::write(&path, component_bytes()).unwrap();
        std::fs::write(
            path.with_extension("json"),
            serde_json::to_string(&ToolInfo {
                name: "format-code".into(),
                description: "format code".into(),
                args_schema: serde_json::json!({"type": "object"}).to_string(),
            })
            .unwrap(),
        )
        .unwrap();
        let workspace = Workspace::open(dir.path()).unwrap();

        let registry = load_plugins(
            dir.path(),
            &[PluginModuleConfig {
                name: "format-code.wasm".into(),
                capabilities: WasiCapabilities {
                    workspace_fs: false,
                    ..WasiCapabilities::default()
                },
            }],
            &workspace,
            OutputLimits::default(),
            Duration::from_secs(1),
        )
        .unwrap();

        assert_eq!(registry.plugins().len(), 1);
        assert_eq!(registry.plugins()[0].name(), "format-code");
        assert!(!registry.plugins()[0].capabilities().workspace_fs);
    }

    #[test]
    fn skips_builtin_name_collisions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("read.wasm");
        std::fs::write(&path, component_bytes()).unwrap();
        let workspace = Workspace::open(dir.path()).unwrap();

        let registry = load_plugins(
            dir.path(),
            &[],
            &workspace,
            OutputLimits::default(),
            Duration::from_secs(1),
        )
        .unwrap();

        assert!(registry.is_empty());
    }
}
