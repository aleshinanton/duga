# duga

A hardened, agent-first LLM runtime.

```text
LLM + tools + memory + sandbox + loop
```

## Overview

**duga** is a minimal runtime for LLM-based coding agents. It provides tools, memory, sandboxing, and a core execution loop — and leaves all reasoning decisions to the LLM.

No hardcoded workflows. No orchestration DAGs. Just a bounded, observable, replayable loop.

## Documentation

| Document | Description |
|----------|-------------|
| [docs/architecture.md](docs/architecture.md) | Full architecture specification (core loop, tools, memory, sandbox, plugins, threat model) |
| [docs/implementation-plan.md](docs/implementation-plan.md) | Implementation plan (crate tree, phases, testing strategy, GAP tracking) |
| [docs/](docs/) | All documentation |

## Quick Start

```bash
# Build the harness
cargo build --release

# Run with a config
./harness --config config.yaml "Your task here"

# Build a plugin
cargo build --target wasm32-wasip2 --release
```

## Philosophy

The runtime is **not**:
- an orchestration platform
- an IDE replacement
- a workflow engine
- an enterprise automation framework

It **is**:
- small
- deterministic at the runtime layer
- observable (full replay-format event stream)
- interruptible
- capability-bounded
- extensible via WASM plugins

## License

See [LICENSE](LICENSE).
