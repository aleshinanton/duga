# duga-replay

## Description

Replay log reader and CLI for `duga`. This crate reads and validates JSONL event logs, supports replay-oriented summaries, and provides tooling for checking observable runtime traces.

## Dependencies

- **Internal:** `duga-events`, `duga-llm`, `duga-types`
- **Runtime:** `clap`, `serde_json`, `thiserror`
- **Development:** `tempfile`, `tokio`

## License

Declared as `MIT OR Apache-2.0` in `Cargo.toml`. The repository currently includes the MIT license text at [`../../LICENSE`](../../LICENSE).

