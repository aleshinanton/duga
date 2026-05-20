# duga-tools-builtin

## Description

Built-in tools for `duga`. This crate implements the default `read`, `write`, `edit`, `shell`, `search`, and `think` tools used by the harness and frontends.

## Dependencies

- **Internal:** `duga-sandbox`, `duga-tools`, `duga-types`
- **Runtime:** `regex`, `schemars`, `serde`, `serde_json`, `tempfile`, `thiserror`, `tokio`, `tracing`, `uuid`, `walkdir`
- **Development:** `tokio-test`

## License

Declared as `MIT` in `Cargo.toml`. The repository currently includes the MIT license text at [`../../LICENSE`](../../LICENSE).
