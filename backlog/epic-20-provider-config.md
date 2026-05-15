# EPIC-20: Provider API Key and Base URL Config

**SPEC:** §6a, §32
**Labels:** `epic/provider-config`
**Crates:** `duga-config`, `duga-runtime`, `duga-llm`, `duga-harness`

## Goal

Allow users to specify API keys and base URLs directly in the YAML config file,
with explicit control over which environment variable names each provider reads.

Currently keys and URLs are hardcoded (`OPENAI_API_KEY`, `BASE_URL`, `ANTHROPIC_API_KEY`),
which means all providers share `BASE_URL` and users can't rename their env vars.

---

### TASK-20.1: Add provider credential fields to Config

- **SPEC:** §32
- **Labels:** `layer/config`, `priority/critical`
- **Description:** Add optional `provider_api_key`, `provider_api_key_env`, `provider_base_url`, and `provider_base_url_env` fields to `Config`.
- **Files affected:**
  - `crates/duga-config/src/config.rs`
- **Types involved:** `Config` (add 4 `Option<String>` fields)
- **Proposed YAML:**
  ```yaml
  provider: "openai"
  model: "gpt-4.1"
  provider_api_key: "sk-..."           # literal, highest priority
  provider_api_key_env: "MY_OPENAI_KEY" # env var name override
  provider_base_url: "http://..."       # literal
  provider_base_url_env: "MY_BASE_URL"  # env var name override
  ```
- **Resolution order:** `provider_api_key` > `provider_api_key_env` env > provider default env. Same for base URL.
- **Dependencies:** none
- **Implementation steps:**
  1. Add `#[serde(default)] provider_api_key: Option<String>` to Config
  2. Same for `provider_api_key_env`, `provider_base_url`, `provider_base_url_env`
  3. Update config validation (no special rules needed — all optional)
  4. Preserve backward compatibility: unset fields → existing env-var behavior.
- **Acceptance criteria:**
  - Existing configs parse unchanged.
  - New fields are optional and default to `None`.
- **Test plan:** config parse tests for each field.
- **Estimated effort:** 1 hour

---

### TASK-20.2: Thread credential overrides through build_llm

- **SPEC:** §6a
- **Labels:** `layer/runtime`, `priority/critical`
- **Description:** Update `build_llm` in `duga-runtime/src/providers.rs` to resolve API key and base URL from config fields, falling back to env vars and then to provider defaults.
- **Files affected:**
  - `crates/duga-runtime/src/providers.rs`
- **Functions to implement/update:**
  - `resolve_api_key(config: &Config, default_env: &str) -> Option<String>`
  - `resolve_base_url(config: &Config) -> Option<String>`
  - `build_llm` passes resolved values to `OpenAiClient::new` / `AnthropicClient::new` instead of calling `from_env`.
- **Resolution logic:**
  1. If `provider_api_key` is `Some` → use it literally
  2. Else if `provider_api_key_env` is `Some` → read that env var
  3. Else → read provider's default env var (`OPENAI_API_KEY` / `ANTHROPIC_API_KEY`)
  4. Same cascade for base URL: `provider_base_url` → `provider_base_url_env` → `BASE_URL`
- **Dependencies:** TASK-20.1
- **Acceptance criteria:**
  - Empty `provider_api_key` with `provider_base_url` set allows local endpoints (same as `BASE_URL` fallback).
  - Literal values in config take priority over env vars.
  - Custom env var names work.
- **Test plan:** unit tests for each resolution path.
- **Estimated effort:** 3 hours

---

### TASK-20.3: Remove from_env, accept explicit params in clients

- **SPEC:** §6a
- **Labels:** `layer/llm`, `priority/high`
- **Description:** Replace `OpenAiClient::from_env` and `AnthropicClient::from_env` with explicit parameter passing. The client constructors no longer read environment variables — all env resolution happens in `build_llm`.
- **Files affected:**
  - `crates/duga-llm/src/openai.rs`
  - `crates/duga-llm/src/anthropic.rs`
- **Types involved:** `OpenAiClient`, `AnthropicClient`
- **Dependencies:** TASK-20.2
- **Implementation steps:**
  1. Delete `from_env` from both clients.
  2. `build_llm` calls `OpenAiClient::new(model, api_key, base_url)` with resolved values.
  3. `AnthropicClient` same pattern.
  4. Update all tests to construct clients explicitly instead of through `from_env`.
- **Acceptance criteria:**
  - `from_env` is removed.
  - All existing tests pass with explicit construction.
  - No `std::env::var` calls remain in LLM crates.
- **Test plan:** existing tests updated; env isolation is handled in runtime tests.
- **Estimated effort:** 2 hours

---

### TASK-20.4: Update harness to use new config fields

- **SPEC:** §5
- **Labels:** `layer/frontend`, `priority/normal`
- **Description:** Ensure `duga-harness` passes the new config fields through to `build_llm` (replacing its own duplicated `build_provider` if still present).
- **Files affected:**
  - `crates/duga-harness/src/main.rs`
- **Dependencies:** TASK-20.2
- **Implementation steps:**
  1. If harness still has its own `build_provider`, switch to `duga_runtime::providers::build_llm`.
  2. Pass full `Config` to `build_llm` so credential resolution works.
  3. Update CLI to print which credential source is used (env var name or config literal).
- **Acceptance criteria:**
  - CLI still runs with existing YAML config.
  - New `provider_api_key` / `provider_base_url` fields work from CLI.
- **Test plan:** CLI smoke test with dummy provider and new fields.
- **Estimated effort:** 2 hours

---

### TASK-20.5: Document new config fields

- **SPEC:** §32
- **Labels:** `layer/docs`, `priority/normal`
- **Description:** Update `docs/providers.md` and `README.md` with examples of the new fields and resolution order.
- **Files affected:**
  - `docs/providers.md`
  - `README.md`
- **Dependencies:** TASK-20.1
- **Acceptance criteria:**
  - All four new fields documented with YAML examples.
  - Resolution order clearly described.
  - Migration note: existing env var users don't need to change.
- **Test plan:** documentation review.
- **Estimated effort:** 1 hour

---

## Summary Table

| Task | Name | Est. Hours |
|------|------|------------|
| TASK-20.1 | Add provider credential fields to Config | 1 |
| TASK-20.2 | Thread credential overrides through build_llm | 3 |
| TASK-20.3 | Remove from_env, accept explicit params in clients | 2 |
| TASK-20.4 | Update harness to use new config fields | 2 |
| TASK-20.5 | Document new config fields | 1 |
| **Total** | | **9 hours** |
