# duga-sandbox

## Description

Capability-based sandboxing for the `duga` agent runtime. This crate provides workspace path containment, binary allowlisting, sanitized subprocess environments, shell-session state, process execution, output limits, and cancellation tokens.

## Dependencies

- **Internal:** `duga-types`
- **Runtime:** `cap-std`, `serde`, `serde_json`, `tempfile`, `thiserror`, `tokio`, `tracing`, `uuid`, `which`
- **Development:** `tokio-test`

## License

Declared as `MIT OR Apache-2.0` in `Cargo.toml`. The repository currently includes the MIT license text at [`../../LICENSE`](../../LICENSE).

