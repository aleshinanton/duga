//! WASI capability matrix for plugins.

pub use duga_plugin_abi::WasiCapabilities;

pub trait WasiCapabilityExt {
    fn allows_workspace_fs(&self) -> bool;
    fn denies_network(&self) -> bool;
    fn denies_process_spawn(&self) -> bool;
}

impl WasiCapabilityExt for WasiCapabilities {
    fn allows_workspace_fs(&self) -> bool {
        self.workspace_fs
    }

    fn denies_network(&self) -> bool {
        !self.network
    }

    fn denies_process_spawn(&self) -> bool {
        !self.process_spawn
    }
}
