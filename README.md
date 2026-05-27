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

### Terminal UI

```bash
# Build and run the interactive terminal UI
cargo build --release -p duga-tui
./target/release/duga-tui --config duga.yaml
```

**Keybindings:**

| Key | Action |
|-----|--------|
| `Enter` | Submit input |
| `Ctrl+C` | Cancel current run |
| `q` / `Ctrl+Q` | Quit (when idle) |
| `F1` | Toggle help overlay |
| `Ctrl+F` | Search transcript |
| `Ctrl+L` | Clear transcript |
| `Escape` | Close overlay / cancel run |
| `Shift+Enter` | New line (multi-line input) |
| `Up` / `Down` | Navigate input history |
| `PageUp` / `PageDown` | Scroll transcript |
| `Tab` | Toggle tool call expansion |

**TUI-specific config** (optional, in `duga.yaml`):

```yaml
tui:
  ime_support: true
  protocol_detection: true
  tool_event_format: "collapsed"  # full, collapsed, final_only
  theme:
    name: "default"
  keybindings:
    submit: "enter"
    cancel: "ctrl-c"
    quit: "q"
    help: "f1"
    search: "ctrl-f"
```

- **`tool_event_format`**: `full` always shows expanded tool blocks; `collapsed` (default) shows them collapsed; `final_only` only reveals tool names after completion.
- **Keybindings** accept `ctrl-`, `alt-`, and `shift-` modifiers (e.g. `ctrl-shift-c`).

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
