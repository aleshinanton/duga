# EPIC-19: Provider Compatibility Follow-up

**SPEC:** §6a, §32
**Labels:** `epic/provider-compatibility`
**Crates:** `duga-llm`, `duga-runtime`, `duga-harness`

## Goal

Preserve the current provider implementation history and define future cleanup around OpenAI-compatible endpoints without changing the original EPIC-8 backlog.

## Current implementation baseline

- `duga-llm` currently exposes `DummyClient`, `OpenAiClient`, and `AnthropicClient`.
- `duga-harness` supports both legacy `model: "provider/model"` and separate `provider` + `model` config.
- OpenAI-compatible local endpoints, including Ollama, currently run through `OpenAiClient`:
  ```yaml
  provider: "openai"
  model: "qwen3.6:27b-coding-nvfp4"
  ```
  with:
  ```sh
  BASE_URL=http://localhost:11434/v1
  ```
- Hosted OpenAI requires `OPENAI_API_KEY` unless `BASE_URL` is set for a compatible local endpoint.
- Anthropic requires `ANTHROPIC_API_KEY`; `BASE_URL` may override the API root.
- A separate runtime `ollama` provider should only be added later if native Ollama `/api/chat` features are needed beyond the OpenAI-compatible endpoint.

---

### TASK-19.1: Provider implementation history and examples

- **SPEC:** §32
- **Labels:** `layer/provider`, `layer/docs`, `priority/normal`
- **Description:** Document current provider behavior, including hosted OpenAI, Anthropic, dummy, and OpenAI-compatible local endpoints.
- **Files affected:**
  - `README.md`
  - `docs/providers.md` (new)
- **Dependencies:** TASK-18.2
- **Implementation steps:**
  1. Document `provider` + `model` config.
  2. Document `BASE_URL` for OpenAI-compatible endpoints.
  3. Include Ollama example using `provider: "openai"`.
  4. Explain when a native provider would be justified.
- **Definition of Done:** Provider behavior is documented without changing EPIC-8.
- **Acceptance criteria:**
  - Docs include hosted and local examples.
  - Ollama example uses OpenAI-compatible configuration.
- **Test plan:** documentation review.
- **Estimated effort:** 3 hours

---

### TASK-19.2: OpenAI-compatible endpoint conformance tests

- **SPEC:** §6a
- **Labels:** `layer/llm`, `layer/provider`, `priority/high`
- **Description:** Add offline fixtures and optional live tests for OpenAI-compatible endpoints, including Ollama-style responses.
- **Files affected:**
  - `crates/duga-llm/tests/openai_compat.rs` (new)
  - `crates/duga-llm/src/openai.rs`
- **Dependencies:** TASK-8.3, TASK-8.6
- **Implementation steps:**
  1. Add mock HTTP tests for chat completions.
  2. Add mock streaming fixtures.
  3. Verify empty API key is accepted only when `BASE_URL` is set.
  4. Add optional live test gated by env vars.
- **Definition of Done:** OpenAI-compatible behavior is covered without requiring a live Ollama instance.
- **Acceptance criteria:**
  - Offline tests cover success, error, and streaming responses.
  - Auth behavior matches the documented provider contract.
- **Test plan:** `cargo test -p duga-llm openai_compat`.
- **Estimated effort:** 4 hours

---

### TASK-19.3: Provider config migration notes

- **SPEC:** §32
- **Labels:** `layer/config`, `layer/provider`, `priority/normal`
- **Description:** Add migration guidance from legacy `model: "provider/model"` configs to separate `provider` + `model`, with explicit examples for OpenAI-compatible endpoints.
- **Files affected:**
  - `README.md`
  - `docs/providers.md`
- **Dependencies:** TASK-19.1
- **Implementation steps:**
  1. Document backward-compatible legacy format.
  2. Recommend separate `provider` and `model` for new configs.
  3. Add examples for OpenAI, Anthropic, dummy, and local OpenAI-compatible endpoints.
- **Definition of Done:** Users can migrate configs without ambiguity.
- **Acceptance criteria:**
  - Legacy format is documented as supported.
  - New examples prefer separate fields.
- **Test plan:** documentation review.
- **Estimated effort:** 3 hours

---

### TASK-19.4: Native Ollama provider decision gate

- **SPEC:** §6a
- **Labels:** `layer/llm`, `layer/provider`, `priority/normal`
- **Description:** Decide whether to add a native Ollama `/api/chat` provider after OpenAI-compatible support is tested. This task is a decision gate, not an automatic implementation.
- **Files affected:**
  - `crates/duga-llm/src/openai.rs`
  - `docs/providers.md`
- **Dependencies:** TASK-19.2
- **Implementation steps:**
  1. Compare OpenAI-compatible Ollama behavior with native `/api/chat`.
  2. Identify missing capabilities, if any.
  3. Document decision and tradeoffs.
  4. If native support is needed, create a new implementation task rather than changing EPIC-8 retroactively.
- **Definition of Done:** Native Ollama is either explicitly deferred or scoped as a new future task.
- **Acceptance criteria:**
  - Decision is documented.
  - EPIC-8 remains unchanged unless intentionally reopened later.
- **Test plan:** design review.
- **Estimated effort:** 4 hours

---

## Summary Table

| Task | Name | Est. Hours |
|------|------|------------|
| TASK-19.1 | Provider implementation history and examples | 3 |
| TASK-19.2 | OpenAI-compatible endpoint conformance tests | 4 |
| TASK-19.3 | Provider config migration notes | 3 |
| TASK-19.4 | Native Ollama provider decision gate | 4 |
| **Total** | | **14 hours** |
