# duga-telegram-bot

## Description

Telegram bot frontend for the `duga` agent runtime. This crate handles Telegram startup, authentication, per-chat sessions, cancellation, tool confirmation UI, progress/final rendering, attachments, scheduled events, and bot logging.

## Dependencies

- **Internal:** `duga-config`, `duga-core`, `duga-events`, `duga-llm`, `duga-runtime`, `duga-sandbox`, `duga-tools`, `duga-types`
- **Runtime:** `anyhow`, `async-trait`, `chrono`, `clap`, `cron`, `dashmap`, `notify`, `serde`, `serde_json`, `teloxide`, `thiserror`, `tokio`, `tracing`, `tracing-subscriber`
- **Development:** `duga-core`, `duga-llm`, `tempfile`

## License

Declared as `MIT OR Apache-2.0` in `Cargo.toml`. The repository currently includes the MIT license text at [`../../LICENSE`](../../LICENSE).

