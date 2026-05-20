# duga-plugin-host

## Description

Host-side plugin loading and `Tool` adapter for `duga`. This crate validates plugin configuration, loads plugin modules, applies host-side capability settings, and exposes plugins through the shared tool system.

## Dependencies

- **Internal:** `duga-plugin-abi`, `duga-sandbox`, `duga-tools`, `duga-types`
- **Runtime:** `serde`, `serde_json`, `schemars`, `thiserror`, `tokio`, `tracing`
- **Development:** `tempfile`

## License

Declared as `MIT` in `Cargo.toml`. The repository currently includes the MIT license text at [`../../LICENSE`](../../LICENSE).

