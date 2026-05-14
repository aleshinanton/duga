# duga-llm

## Description

LLM client abstractions and provider implementations for `duga`. This crate provides the async chat interface, provider registry, OpenAI-compatible client support, Anthropic integration, response parsing, and token counting hooks used by the agent loop.

## Dependencies

- **Internal:** `duga-events`, `duga-types`
- **Runtime:** `reqwest`, `serde`, `serde_json`, `thiserror`
- **Development:** `tokio`

## License

The crate follows the repository license. See the MIT license text at [`../../LICENSE`](../../LICENSE).

