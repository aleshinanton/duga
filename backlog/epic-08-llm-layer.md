# EPIC-8: LLM Layer

**§SPEC:** §6a, §32  
**Labels:** `epic/llm`  
**Crates:** `duga-llm`, `duga-llm-openai`, `duga-llm-anthropic`, `duga-llm-ollama`

## Goal
Define the `LlmClient` trait, `ProviderRegistry` for `provider/model` routing, and concrete implementations for OpenAI, Anthropic, and Ollama backends.

---

### TASK-8.1: LlmClient trait + LlmError

- **§SPEC:** §6a (LlmClient Interface)
- **Labels:** `layer/llm`, `priority/critical`
- **Description:** Define `trait LlmClient: Send + Sync` in `duga-llm` with `async fn chat(&self, messages: &[Message], tools: &[ToolSchema], opts: LlmCallOptions, events: &dyn EventSink) -> Result<LlmResponse, LlmError>` and `fn count_tokens(&self, messages: &[Message]) -> usize`. Define `LlmError` enum with variants: `ApiError(u16, String)`, `NetworkError(String)`, `RateLimited(String)`, `SerializationError(String)`, `ProviderError(String)`. Derive `thiserror`, `Clone`, `Debug`.
- **Files affected:**
  - `crates/duga-llm/Cargo.toml` (new)
  - `crates/duga-llm/src/lib.rs` (new)
  - `crates/duga-llm/src/error.rs` (new)
- **Types involved:** `LlmClient` (trait), `LlmError`, `Message`, `ToolSchema`, `LlmCallOptions`, `LlmResponse`, `EventSink`
- **Dependencies:** TASK-1.2, TASK-1.3, TASK-1.8, TASK-7.2
- **Implementation steps:**
  1. Create `duga-llm` crate (trait-only, no concrete impls)
  2. Define `LlmClient` trait
  3. Define `LlmError` enum with Display
  4. Add note about GAP G6: `count_tokens` is sync in trait, but may be async for Anthropic
- **Definition of Done:** Trait compiles, `cargo clippy` clean
- **Estimated effort:** 3 hours

---

### TASK-8.2: ProviderRegistry + ProviderFactory trait

- **§SPEC:** §6a (provider routing), §32 (model string format)
- **Labels:** `layer/llm`, `priority/critical`
- **Description:** Implement `ProviderRegistry` that maps `provider` prefix strings to `Box<dyn ProviderFactory>`. The `ProviderFactory` trait has `fn build(&self, model: &str, api_key: Option<String>, base_url: Option<String>) -> Result<Arc<dyn LlmClient>, LlmError>`. `ProviderRegistry::register(name, factory)` adds a provider. `ProviderRegistry::build(model_str: &str)` parses `"provider/model"` format and delegates to the matching factory.
- **Files affected:**
  - `crates/duga-llm/src/registry.rs` (new)
  - `crates/duga-llm/src/lib.rs` (add module)
- **Types involved:** `ProviderRegistry`, `ProviderFactory` (trait), `LlmClient`
- **Functions to implement:**
  - `ProviderRegistry::new() -> Self`
  - `ProviderRegistry::register(&mut self, prefix: &str, factory: Box<dyn ProviderFactory>)`
  - `ProviderRegistry::build(&self, model_str: &str, api_key: Option<String>, base_url: Option<String>) -> Result<Arc<dyn LlmClient>, LlmError>`
  - `trait ProviderFactory: Send + Sync { fn build(...) -> ... }`
- **Dependencies:** TASK-8.1
- **Implementation steps:**
  1. Define `ProviderFactory` trait
  2. Implement `ProviderRegistry` with `HashMap<String, Box<dyn ProviderFactory>>`
  3. `build()`: split `model_str` on first `/` → `(provider, model)`, lookup factory, call build
  4. Invalid format (no `/`) → `LlmError::ProviderError("invalid model string")`
  5. Unknown prefix → `LlmError::ProviderError("unknown provider")`
- **Edge cases:**
  - Model string with multiple `/` → split on first only: `openai/gpt-4o/extra` → provider=`openai`, model=`gpt-4o/extra`
  - Empty string → error
- **Definition of Done:** Registry works, `cargo test` passes
- **Acceptance criteria:**
  - `registry.build("openai/gpt-4o")` → calls OpenAI factory
  - `registry.build("unknown/model")` → Err
  - `registry.build("invalid")` → Err
- **Test plan:** unit: Mock ProviderFactory; test routing
- **Estimated effort:** 4 hours

---

### TASK-8.3: OpenAI LlmClient implementation

- **§SPEC:** §6a (OpenAI-compatible chat API)
- **Labels:** `layer/llm`, `priority/critical`
- **Description:** Implement `OpenAiClient` in `duga-llm-openai` that calls the OpenAI `/v1/chat/completions` endpoint via `reqwest`. Map `Message` and `ToolSchema` to OpenAI's API format. Handle API errors as `LlmError`. Support both streaming and non-streaming modes. Use `OPENAI_API_KEY` env var or passed key.
- **Files affected:**
  - `crates/duga-llm/openai/Cargo.toml` (new)
  - `crates/duga-llm/openai/src/lib.rs` (new)
  - `crates/duga-llm/openai/src/client.rs` (new)
  - `crates/duga-llm/openai/src/mapping.rs` (new — message/schema mapping)
- **Types involved:** `OpenAiClient`, `LlmClient`, `LlmResponse`, `LlmError`
- **Functions to implement:**
  - `OpenAiClient::new(model: String, api_key: String, base_url: Option<String>) -> Self`
  - `impl LlmClient for OpenAiClient`
  - `fn to_openai_messages(msgs: &[Message]) -> Vec<Value>`
  - `fn to_openai_tools(tools: &[ToolSchema]) -> Vec<Value>`
  - `fn from_openai_response(resp: Value) -> Result<LlmResponse, LlmError>`
- **Dependencies:** TASK-8.1, TASK-8.2
- **Implementation steps:**
  1. Create crate with deps: `duga-llm`, `reqwest`, `tokio`, `serde_json`, `tracing`
  2. Implement message mapping: system/user/assistant/tool roles, tool_calls in assistant
  3. Implement tool schema mapping: OpenAI's `tools` array with `type: function`
  4. Implement non-streaming HTTP POST
  5. Parse response: extract text, tool_calls, token usage
  6. Implement `ProviderFactory` for registration
  7. Add integration test with `OPENAI_API_KEY` (optional, skipped if not set)
- **Edge cases:**
  - Empty tool list → omit `tools` field
  - Assistant message with no text and no tool calls → should not happen, but handle
  - API returns 429 → `LlmError::RateLimited`
  - API returns 401 → `LlmError::ApiError(401, ...)`
- **Definition of Done:** Client works with real API key, `cargo test` passes (integration test optional)
- **Acceptance criteria:**
  - `chat("Hello")` returns text response
  - `chat("Write a function", tools=[...])` returns function call
  - Invalid API key → `LlmError::ApiError(401, ...)`
- **Test plan:** integration: Live API test with env var; unit: Mock HTTP server
- **Estimated effort:** 8 hours

---

### TASK-8.4: Anthropic LlmClient implementation

- **§SPEC:** §6a (Anthropic Messages API)
- **Labels:** `layer/llm`, `priority/high`
- **Description:** Implement `AnthropicClient` calling Anthropic's `/v1/messages` endpoint. Map messages/tools to Anthropic's format. Handle `stop_reason: "tool_use"` for tool calls.
- **Files affected:**
  - `crates/duga-llm/anthropic/Cargo.toml` (new)
  - `crates/duga-llm/anthropic/src/lib.rs` (new)
- **Types involved:** `AnthropicClient`, `LlmClient`
- **Dependencies:** TASK-8.3 (same pattern)
- **Implementation steps:** Same pattern as TASK-8.3 with Anthropic-specific mapping
- **Estimated effort:** 6 hours

---

### TASK-8.5: Ollama LlmClient implementation

- **§SPEC:** §6a (Ollama /api/chat)
- **Labels:** `layer/llm`, `priority/normal`
- **Description:** Implement `OllamaClient` calling Ollama's `/api/chat` endpoint (OpenAI-compatible mode). Handles local models with minimal configuration.
- **Files affected:**
  - `crates/duga-llm/ollama/Cargo.toml` (new)
  - `crates/duga-llm/ollama/src/lib.rs` (new)
- **Dependencies:** TASK-8.3
- **Implementation steps:** Same pattern; Ollama's API is OpenAI-compatible, reuse mapping
- **Estimated effort:** 4 hours

---

### TASK-8.6: Streaming SSE parser — per-provider

- **§SPEC:** §5.2 (Streaming Contract)
- **Labels:** `layer/llm`, `priority/critical`
- **Description:** Implement SSE streaming for OpenAI (and Ollama). When `opts.streaming == true`, parse the SSE stream line-by-line, extract `delta.content` tokens, and emit `Event::LlmTokenDelta` to the provided `EventSink`. On `[DONE]`, accumulate all deltas into the final `AssistantMessage` and return `LlmResponse`. GAP G3: ensure every TokenDelta for turn N has seq < LlmResponse for turn N.
- **Files affected:**
  - `crates/duga-llm/openai/src/streaming.rs` (new)
- **Types involved:** `Event::LlmTokenDelta`, `LlmResponse`
- **Functions to implement:**
  - `async fn parse_sse_stream(response: reqwest::Response, events: &dyn EventSink) -> Result<LlmResponse, LlmError>`
- **Dependencies:** TASK-8.3, TASK-7.1 (LlmTokenDelta event)
- **Implementation steps:**
  1. Stream response body as bytes via `response.bytes_stream()`
  2. Split on `\n\n` to get SSE events
  3. Parse each line starting with `data: `
  4. Handle `data: [DONE]` as end-of-stream
  5. Extract token deltas from `choices[0].delta.content`
  6. Emit `Event::LlmTokenDelta` for each
  7. Extract tool calls from final chunks (tool calls sent in one chunk)
  8. Build final `LlmResponse`
- **Edge cases:**
  - Empty delta (first chunk often has `delta: { role: "assistant" }` only)
  - Tool call deltas arrive as accumulated chunks → track tool_call index + function name + arguments
  - `[DONE]` never arrives → timeout from reqwest handles this
  - Multiple `data:` lines in one chunk → split correctly
- **Definition of Done:** Streaming works, tokens emitted
- **Acceptance criteria:**
  - Streaming call emits TokenDelta events during response
  - Final response contains complete message text
  - Tool calls in streaming mode parsed correctly
- **Test plan:** unit: Mock SSE server; integration: Live API streaming test
- **Estimated effort:** 6 hours

---

### TASK-8.7: count_tokens with tiktoken-rs

- **§SPEC:** §6a (count_tokens), §21 (token budget)
- **Labels:** `layer/llm`, `priority/critical`
- **Description:** Implement `count_tokens()` for OpenAI using `tiktoken-rs` with the appropriate encoding (cl100k_base for GPT-4, o200k_base for GPT-4o). For Anthropic and Ollama, use approximations based on character/word counts (Anthropic's tokenizer is HTTP-only — GAP G6). Provide a fallback `estimate_tokens` function: `chars * 0.25` for English, `chars * 0.5` for code.
- **Files affected:**
  - `crates/duga-llm/openai/src/tokenizer.rs` (new)
  - `crates/duga-llm/src/tokenizer.rs` (new — fallback estimator)
- **Types involved:** `LlmClient::count_tokens`
- **Dependencies:** TASK-8.3
- **Implementation steps:**
  1. Add `tiktoken-rs` dep to `duga-llm-openai`
  2. On client creation, load appropriate encoding based on model name
  3. `count_tokens()`: serialize messages to chat format, encode, return token count
  4. For Anthropic/Ollama: use fallback estimator
  5. Document GAP G6: Anthropic count may be approximate
- **Edge cases:**
  - tiktoken-rs encoding load fails → fallback to estimator
  - Tool definitions counted? → Yes, tool schemas contribute to token count. Count them in the loop (TASK-10.x), not here
- **Definition of Done:** Token counting works
- **Acceptance criteria:**
  - `count_tokens(&[Message::user("hello")])` returns > 0
  - Same message counted by different providers gives similar results (within 20%)
- **Test plan:** unit: Test with known message sets, verify non-zero
- **Estimated effort:** 4 hours

---

### TASK-8.8: Provider conformance tests

- **§SPEC:** §6a (cross-provider behavior)
- **Labels:** `layer/llm`, `priority/normal`
- **Description:** Create a test suite that verifies each LlmClient implementation conforms to the trait contract: (1) non-streaming returns complete LlmResponse, (2) streaming emits deltas then returns complete response, (3) tool calls roundtrip correctly, (4) error handling for invalid API keys. GAP G17: conformance fixtures for cross-provider testing.
- **Files affected:**
  - `crates/duga-llm/tests/conformance.rs` (new)
- **Types involved:** `LlmClient` implementations
- **Dependencies:** TASK-8.3, TASK-8.4, TASK-8.5, TASK-8.6
- **Implementation steps:**
  1. Create test module with `MockHttpServer` (using `wiremock` or `axum` test server)
  2. Test each client against known request/response patterns
  3. Test streaming endpoint
  4. Test error responses (401, 429, 500)
  5. Skip live API tests by default (behind `#[cfg(feature = "live-tests")]`)
- **Definition of Done:** All conformances pass with mock server
- **Estimated effort:** 5 hours
