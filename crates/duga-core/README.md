# duga-core

## Description

Core loop and memory primitives for `duga`. This crate contains the **loop-agnostic agent execution system** (Loop trait, LoopRegistry, SimpleReActLoop + specialized loops), memory handling, summarization hooks, cancellation checks, tool dispatch integration, and runtime test utilities.

## Loop System

See [`docs/loop-system.md`](../../docs/loop-system.md) for the full architecture.

### Quick Start — Add a new loop

1. Implement the `Loop` trait (see `src/loops/simple_react.rs`)
2. Register in `src/loops/mod.rs`:
   ```rust
   pub mod my_loop;
   pub use my_loop::MyLoop;
   // add to register_default_loops()
   ```
3. Add the loop id to `agent.loop.enabled_loops` in your config YAML

Nothing else changes — the loop appears in the system prompt and `delegate` tool schema automatically.

## Dependencies

- **Internal:** `duga-events`, `duga-llm`, `duga-sandbox`, `duga-tools`, `duga-types`
- **Runtime:** `tokio`, `tracing`, `schemars`, `serde_json`
- **Development:** `duga-replay`, `serde`, `tempfile`, `tokio`

## License

Declared as `MIT` in `Cargo.toml`. The repository currently includes the MIT license text at [`../../LICENSE`](../../LICENSE).

