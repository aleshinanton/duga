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
| [docs/telegram-bot.md](docs/telegram-bot.md) | Telegram bot frontend setup and usage |
| [docs/](docs/) | All documentation |

## Quick Start

```bash
# Build the harness
cargo build --release

# Run with a config
./target/release/duga-harness --config config.yaml "Your task here"

# Build a plugin
cargo build --target wasm32-wasip2 --release
```

## Configuration

The harness reads YAML config with a provider and model:

```yaml
provider: "openai"
model: "gpt-4o-mini"
```

Hosted OpenAI and Anthropic require their API keys:

```bash
export OPENAI_API_KEY="..."
export ANTHROPIC_API_KEY="..."
```

Use one optional `BASE_URL` for OpenAI-compatible APIs. For local run:

```yaml
provider: "openai"
model: "qwen3.6:27b-coding-nvfp4"
```

```bash
BASE_URL=http://localhost:11434/v1 ./target/release/duga-harness --config duga-config.yaml "Your task"
```

### Telegram Bot

```bash
# Build and run the Telegram bot
cargo build --release -p duga-telegram-bot
export TELEGRAM_BOT_TOKEN="your-bot-token"
./target/release/duga-telegram-bot --config duga.yaml
```

See [docs/telegram-bot.md](docs/telegram-bot.md) for full configuration, triggers, commands, safety model, and scheduled events.

### Docker + Allow-All Mode

For environments where the agent runs inside a Docker container with full
OS-level isolation, you can skip the binary allowlist entirely:

```yaml
sandbox:
  mode: "docker"
  container: "duga-sandbox"
  allow_all_binaries: true
  timeout: 120s
```

In allow-all mode, commands are executed inside the container via `docker exec`,
and the container's own `PATH` resolves binaries. This avoids listing every
system tool individually. **Only use allow-all with container/VM isolation.**

A middle ground is glob patterns, which expand directories at startup:

```yaml
sandbox:
  allowed_binaries:
    - /usr/bin/*          # all executables in /usr/bin
    - /usr/local/bin/g*   # git, gcc, go, etc.
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
