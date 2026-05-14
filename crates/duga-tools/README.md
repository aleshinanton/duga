# duga-tools

## Description

Tool trait system and dispatcher for `duga`. This crate defines `Tool`, `ToolContext`, type-erased registration, schema validation, confirmation middleware, event-sink adapters, and dispatch behavior for built-in and plugin tools.

## Dependencies

- **Internal:** `duga-events`, `duga-sandbox`, `duga-types`
- **Runtime:** `async-trait`, `jsonschema`, `schemars`, `serde`, `serde_json`, `thiserror`, `tokio`, `tracing`
- **Development:** `tempfile`

## License

Declared as `MIT OR Apache-2.0` in `Cargo.toml`. The repository currently includes the MIT license text at [`../../LICENSE`](../../LICENSE).

