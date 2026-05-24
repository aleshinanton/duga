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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_caps(workspace_fs: bool, network: bool, process_spawn: bool) -> WasiCapabilities {
        WasiCapabilities {
            workspace_fs,
            network,
            process_spawn,
            randomness: true,
            clocks: true,
        }
    }

    #[test]
    fn allowed_when_fs_enabled() {
        let caps = make_caps(true, false, false);
        assert!(caps.allows_workspace_fs());
    }

    #[test]
    fn denied_when_fs_disabled() {
        let caps = make_caps(false, false, false);
        assert!(!caps.allows_workspace_fs());
    }

    #[test]
    fn denies_network_when_false() {
        let caps = make_caps(false, false, false);
        assert!(caps.denies_network());
    }

    #[test]
    fn allows_network_when_true() {
        let caps = make_caps(false, true, false);
        assert!(!caps.denies_network());
    }

    #[test]
    fn denies_process_spawn_when_false() {
        let caps = make_caps(false, false, false);
        assert!(caps.denies_process_spawn());
    }

    #[test]
    fn allows_process_spawn_when_true() {
        let caps = make_caps(false, false, true);
        assert!(!caps.denies_process_spawn());
    }

    #[test]
    fn full_sandbox_caps() {
        // Typical safe default: FS allowed, no network, no spawn
        let caps = make_caps(true, false, false);
        assert!(caps.allows_workspace_fs());
        assert!(caps.denies_network());
        assert!(caps.denies_process_spawn());
    }

    #[test]
    fn default_caps_are_safe() {
        let caps = WasiCapabilities::default();
        // Default should be safe: no network, no process spawn
        assert!(caps.denies_network(), "default should deny network");
        assert!(caps.denies_process_spawn(), "default should deny process spawn");
    }
}
