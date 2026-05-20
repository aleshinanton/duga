# duga-harness

## Description

Composition root and CLI for `duga`. This crate wires configuration, providers, workspace sandboxing, tools, plugins, memory, events, tracing, and signal cancellation into a runnable command-line harness.

## Dependencies

- **Internal:** `duga-config`, `duga-core`, `duga-events`, `duga-llm`, `duga-plugin-host`, `duga-sandbox`, `duga-tools`, `duga-tools-builtin`, `duga-types`
- **Runtime:** `anyhow`, `clap`, `tokio`, `tracing`, `tracing-subscriber`
- **Development:** `tempfile`

## License

Declared as `MIT` in `Cargo.toml`. The repository currently includes the MIT license text at [`../../LICENSE`](../../LICENSE).

