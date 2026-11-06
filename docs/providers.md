# Provider Compatibility Guide

This document describes the current LLM provider behavior in `duga`, including
hosted OpenAI, Anthropic, dummy, and OpenAI-compatible local endpoints (e.g. Ollama).

## Provider Overview

| Provider | Client Type | API Key Required | Default Base URL |
|----------|------------|-----------------|-----------------|
| `openai` (API key) | `OpenAiClient` (Chat Completions) | `OPENAI_API_KEY` (unless BASE_URL set) | `https://api.openai.com/v1` |
| `openai` + `provider_auth: oauth`, or `openai-codex` | `OpenAiCodexClient` (Responses) | ChatGPT browser login (`--login openai`) | `https://chatgpt.com/backend-api/codex/responses` |
| `anthropic` | `AnthropicClient` | `ANTHROPIC_API_KEY` or OAuth (`--login anthropic`) | `https://api.anthropic.com` |
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

OpenAI supports **two distinct authentication paths**. An API key uses the
standard OpenAI API and API billing; browser login uses the account's Codex
access and ChatGPT usage limits. ChatGPT login does not grant access to every
OpenAI API model or replace an API key for arbitrary compatible endpoints.

### Browser login (ChatGPT / Codex)

Run this **before starting the interactive TUI**, from a terminal on the same
computer as your browser:

```sh
cargo run --release -p duga-tui -- --login openai
# Installed binary: duga-tui --login openai
# Also supported: duga-harness --login openai
```

Duga opens the OpenAI sign-in page in your browser (and prints the URL if you
need to open it manually). Approve access there; the browser redirects to the
short-lived loopback listener on `127.0.0.1:1455` (or `1457` if the first port is
busy). The listener checks OAuth state and exchanges the code using PKCE S256.
No password or authorization code needs to be pasted into duga. Login times out
after five minutes; Ctrl+C cancels and closes the listener. This is a local
browser flow, not a headless/device-code flow; remote shells need a suitable
loopback tunnel or an API key instead.

Then configure the model:

```yaml
provider: "openai"
provider_auth: "oauth"
model: "gpt-5.2-codex"  # choose a Codex model your account can access
thinking_level: "medium"
```

A complete, safe-default example is provided in [`duga-openai.yaml`](../duga-openai.yaml):

```sh
mkdir -p data/openai-workspace
cargo run --release -p duga-tui -- --config duga-openai.yaml
```

In PowerShell, create the directory with
`New-Item -ItemType Directory -Force data/openai-workspace` instead of `mkdir -p`,
or set `workspace.root` to an existing directory.

`provider: "openai-codex"` is an explicit alias for the OAuth path; the legacy
`model: "openai-codex/gpt-5.2-codex"` form is also supported.

Credentials are stored separately from Anthropic in `~/.duga/openai-auth.json`
(`%USERPROFILE%\.duga\openai-auth.json` on Windows when `HOME` is unset).
Override this with `DUGA_OPENAI_AUTH_FILE`. The file is atomically replaced and
created with owner-only `0600` permissions on Unix; Windows uses the containing
directory's ACLs. Keep it in a private user directory, outside the agent's
workspace; do not commit or share it. Tokens refresh automatically before
expiry, and an HTTP 401 triggers one refresh/retry. Concurrent clients within
one duga process share refresh state. Separate processes should use separate
credential files/logins to avoid concurrent refresh-token rotation. Re-run
`--login openai` if authorization is revoked. To remove locally stored login,
stop duga and delete the credential file; revoke account access through OpenAI
when needed.

OAuth inference uses streaming Codex Responses with `store: false`, including
text, reasoning summaries, tool calls and usage accounting. `thinking_level:
max` maps to `xhigh`; available reasoning levels depend on the selected model.
`off` omits the explicit effort override, leaving the model's default behavior.

Credential resolution for `provider: "openai"`:

1. `provider_auth: "oauth"` always uses ChatGPT credentials, never an API key.
2. Otherwise, an API key or a configured compatible base URL uses the existing
   Chat Completions client.
3. When neither is configured, stored OpenAI OAuth credentials are used unless
   `provider_auth: "api_key"` explicitly disables the fallback.

For OAuth, remove `provider_base_url` / `provider_base_url_env` from the config;
these overrides are rejected. Ambient `BASE_URL` is ignored in explicit OAuth
mode, and bearer tokens are sent only to the fixed ChatGPT endpoint (redirects
are disabled). Existing API-key and Ollama configurations remain unchanged.
The TUI reports provider/authentication errors instead of silently substituting
a dummy model.

### API key

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

Supports two authentication modes: API key and OAuth.

### API key

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

### OAuth (Claude Pro/Max subscription)

Instead of an API key, you can log in with your Claude account via a
PKCE OAuth flow (the same flow Claude Code uses):

```sh
duga-harness --login anthropic
# or
duga-tui --login anthropic
```

This prints an authorize URL; open it in a browser, approve access, and
paste the displayed `code#state` string back into the terminal. Tokens are
stored in `~/.duga/auth.json` (override with `DUGA_AUTH_FILE`) with `0600`
permissions and are refreshed automatically before they expire.

Credential resolution order for `provider: "anthropic"`:

1. `provider_auth: "oauth"` in config → always use stored OAuth tokens.
2. API key (`provider_api_key`, `provider_api_key_env`, or `ANTHROPIC_API_KEY`).
3. Stored OAuth credentials, when no API key is configured.

```yaml
provider: "anthropic"
model: "claude-sonnet-4-5"
provider_auth: "oauth"
```

OAuth requests are sent with `Authorization: Bearer` and the
`anthropic-beta: oauth-2025-04-20` header, and the system prompt is
prefixed with the Claude Code identity line required by Anthropic for
OAuth-authenticated inference.

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
| `OPENAI_API_KEY` | `openai` | Unless OAuth or BASE_URL is used | OpenAI API key |
| `DUGA_OPENAI_AUTH_FILE` | `openai` OAuth / `openai-codex` | No | Override `~/.duga/openai-auth.json` |
| `ANTHROPIC_API_KEY` | `anthropic` | Unless OAuth is used | Anthropic API key |
| `DUGA_AUTH_FILE` | `anthropic` OAuth | No | Override `~/.duga/auth.json` |
| `BASE_URL` | `openai`, `anthropic` (API-key modes) | No | Override API base URL; ignored by OpenAI OAuth |

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