# duga-core

## Description

Core loop and memory primitives for `duga`. This crate contains the bounded agent loop, memory handling, summarization hooks, cancellation checks, tool dispatch integration, and runtime test utilities.

## Dependencies

- **Internal:** `duga-events`, `duga-llm`, `duga-sandbox`, `duga-tools`, `duga-types`
- **Runtime:** `tokio`, `tracing`, `schemars`, `serde_json`
- **Development:** `duga-replay`, `serde`, `tempfile`, `tokio`

## License

Declared as `MIT` in `Cargo.toml`. The repository currently includes the MIT license text at [`../../LICENSE`](../../LICENSE).

