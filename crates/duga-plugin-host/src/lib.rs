//! Host-side WASM plugin loading and tool adaptation.

pub mod adapter;
pub mod capabilities;
pub mod loader;

pub use adapter::{PluginExecutor, StaticPluginExecutor, WasmPluginAdapter};
pub use capabilities::WasiCapabilities;
pub use loader::{load_plugins, PluginError, PluginRegistry, BUILTIN_NAMES};
