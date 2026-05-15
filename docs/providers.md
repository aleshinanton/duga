# Provider Compatibility Guide

This document describes the current LLM provider behavior in `duga`, including
hosted OpenAI, Anthropic, dummy, and OpenAI-compatible local endpoints (e.g. Ollama).

## Provider Overview

| Provider | Client Type | API Key Required | Default Base URL |
|----------|------------|-----------------|-----------------|
| `openai` | `OpenAiClient` | `OPENAI_API_KEY` (unless BASE_URL set) | `https://api.openai.com/v1` |
| `anthropic` | `AnthropicClient` | `ANTHROPIC_API_KEY` | `https://api.anthropic.com` |
| `dummy` | `DummyClient` | No | N/A (offline) |

## Configuration

### Separate `provider` + `model` fields (recommended)

```yaml
provider: "openai"
model: "gpt-4.1"
```

```yaml
provider: "anthropic"
model: "claude-sonnet-4-5"
```

### Legacy `provider/model` format (still supported)

```yaml
model: "openai/gpt-4.1"
```

```yaml
model: "anthropic/claude-sonnet-4-5"
```

The legacy format is parsed by splitting on the first `/`. The `provider` field
takes precedence if both are specified and must not conflict with the embedded
provider in the model string.

### Credential overrides (optional)

API keys and base URLs default to environment variables, but can be overridden
in config with four optional fields:

```yaml
provider: "openai"
model: "gpt-4.1"
provider_api_key: "sk-..."             # literal value (highest priority)
provider_api_key_env: "MY_OPENAI_KEY"   # custom env var name
provider_base_url: "http://..."         # literal value
provider_base_url_env: "MY_BASE_URL"    # custom env var name
```

**Resolution order** for API key:
1. `provider_api_key` (literal)
2. `provider_api_key_env` (env var)
3. Provider default (`OPENAI_API_KEY` / `ANTHROPIC_API_KEY`)

Same cascade for base URL: `provider_base_url` → `provider_base_url_env` → `BASE_URL`.

Examples:
```yaml
# Literal key for hosted OpenAI
provider_api_key: "sk-proj-abc123"

# Custom env var name (e.g. CI pipeline injects MY_KEY)
provider_api_key_env: "MY_KEY"

# Local Ollama with custom endpoint
provider_base_url: "http://192.168.1.50:11434/v1"

# Different base URL per provider via custom env vars
provider_base_url_env: "OLLAMA_BASE_URL"
```

## Hosted OpenAI

Requires `OPENAI_API_KEY` environment variable:

```sh
export OPENAI_API_KEY="sk-..."
```

Config:

```yaml
model: "gpt-4.1"
# or
provider: "openai"
model: "gpt-4.1"
```

### Overriding the base URL

Set `BASE_URL` to use an OpenAI-compatible local endpoint instead of the
hosted API:

```sh
export BASE_URL=http://localhost:11434/v1
```

When `BASE_URL` is set, the API key is optional (empty string is accepted).
This allows using Ollama or other OpenAI-compatible servers without an API key.

## Anthropic

Requires `ANTHROPIC_API_KEY` environment variable:

```sh
export ANTHROPIC_API_KEY="sk-ant-..."
```

Config:

```yaml
provider: "anthropic"
model: "claude-sonnet-4-5"
```

To use a custom-compatible Anthropic endpoint, set `BASE_URL`:

```sh
export BASE_URL=https://custom.anthropic.endpoint/v1
```

## Dummy Provider (Offline Development)

The `dummy` provider returns queued responses without making any API calls.
It is useful for testing and development.

```yaml
provider: "dummy"
model: "test"
```

The dummy client returns a static message: `"Dummy provider for model 'test' is wired correctly."`

## Ollama / OpenAI-Compatible Local Endpoints

Ollama exposes an OpenAI-compatible API at `http://localhost:11434/v1` by default.
Use it with the `openai` provider:

```yaml
provider: "openai"
model: "qwen3.6:27b-coding-nvfp4"
```

With environment:

```sh
export BASE_URL=http://localhost:11434/v1
```

No API key is needed when `BASE_URL` is set to a local endpoint.

### Why Use OpenAI-Compatible Mode Instead of a Native Ollama Provider?

The OpenAI-compatible endpoint covers the standard chat completions flow:
- System/user/assistant messages
- Tool calls via `tools` parameter
- Streaming via SSE

A native `/api/chat` client is not planned unless Ollama deprecates the compatible API.

## Environment Variable Summary

| Variable | Provider | Required | Description |
|----------|----------|----------|-------------|
| `OPENAI_API_KEY` | `openai` | Yes (unless BASE_URL set) | OpenAI API key |
| `ANTHROPIC_API_KEY` | `anthropic` | Yes | Anthropic API key |
| `BASE_URL` | `openai`, `anthropic` | No | Override API base URL |

## Migration Guide

### From Legacy to New Format

Old config:

```yaml
model: "openai/gpt-4.1"
```

New config (equivalent):

```yaml
provider: "openai"
model: "gpt-4.1"
```

Old config with Anthropic:

```yaml
model: "anthropic/claude-sonnet-4-5"
```

New config:

```yaml
provider: "anthropic"
model: "claude-sonnet-4-5"
```

### Adding Ollama Support

Old (using separate fields):

```yaml
provider: "openai"
model: "qwen3.6:27b-coding-nvfp4"
```

Ensure `BASE_URL` is set:

```sh
export BASE_URL=http://localhost:11434/v1
```

### Backward Compatibility

The legacy `model: "provider/model"` format continues to work. If both `provider`
and the embedded provider in `model` are set, they must match, or an error is
raised at startup.