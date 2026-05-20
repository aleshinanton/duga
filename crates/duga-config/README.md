# duga-config

## Description

YAML configuration loading and validation for `duga`. This crate owns the typed configuration model used by the harness, shared runtime, sandbox, plugins, providers, and Telegram frontend.

## Dependencies

- **Internal:** `duga-plugin-abi`, `duga-sandbox`, `duga-types`
- **Runtime:** `serde`, `serde_yaml`, `thiserror`, `tracing`
- **Development:** `tempfile`

## License

Declared as `MIT` in `Cargo.toml`. The repository currently includes the MIT license text at [`../../LICENSE`](../../LICENSE).

