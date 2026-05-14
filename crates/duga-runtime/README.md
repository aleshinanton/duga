# duga-runtime

## Description

Shared runtime composition for `duga` frontends. This crate centralizes provider resolution, LLM construction, dispatcher construction, built-in/plugin tool wiring, persistent memory context, skill loading, frontend event bridges, and confirmation middleware re-exports.

## Dependencies

- **Internal:** `duga-config`, `duga-core`, `duga-events`, `duga-llm`, `duga-plugin-host`, `duga-sandbox`, `duga-tools`, `duga-tools-builtin`, `duga-types`
- **Runtime:** `anyhow`, `serde`, `serde_yaml`, `thiserror`, `tokio`, `tracing`
- **Development:** `serde_json`, `tempfile`

## License

Declared as `MIT OR Apache-2.0` in `Cargo.toml`. The repository currently includes the MIT license text at [`../../LICENSE`](../../LICENSE).

