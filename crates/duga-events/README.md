# duga-events

## Description

Event types and sinks for `duga`. This crate defines the observable event stream, JSONL sink support, sink fan-out, event redaction, and related error types used by runtime and replay components.

## Dependencies

- **Internal:** `duga-types`
- **Runtime:** `chrono`, `serde`, `serde_json`, `thiserror`
- **Development:** `tempfile`, `tokio`

## License

Declared as `MIT` in `Cargo.toml`. The repository currently includes the MIT license text at [`../../LICENSE`](../../LICENSE).

