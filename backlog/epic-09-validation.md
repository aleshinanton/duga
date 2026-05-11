# EPIC-9: Validation Pipeline

**§SPEC:** §8  
**Labels:** `epic/validation`  
**Crate:** `duga-tools` (extends `ToolDispatcher`)

## Goal
Complete the end-to-end validation pipeline in `ToolDispatcher::dispatch()`: JSON parse validation → jsonschema validation → typed deserialization → tool execution. All errors become `ToolError::InvalidArgs` with LLM-friendly messages.

---

### TASK-9.1: json_schema generation from Tool trait

- **§SPEC:** §7 (JsonSchema derive from schemars), §8 (schema available to dispatcher)
- **Labels:** `layer/validation`, `priority/critical`
- **Description:** Ensure the `Tool::json_schema()` default implementation (from TASK-3.1) correctly generates Draft 2020-12 JSON Schema via `schemars`. Handle edge cases: `Option<T>` fields become non-required, `Vec<T>` become arrays, nested structs work. Emitted schema must be OpenAI/Anthropic-compatible (no `$ref` cycles, no `oneOf` at top level).
- **Files affected:**
  - `crates/duga-tools/src/tool.rs` (verify json_schema impl)
  - `crates/duga-tools/src/schema.rs` (schema generation helper)
- **Types involved:** `Tool`, `ToolSchema`, `schemars`
- **Functions to implement:**
  - `fn generate_schema<T: JsonSchema>() -> Value`
  - `fn sanitize_for_openai(schema: &mut Value)` — remove `$schema` keyword, ensure type is object
- **Dependencies:** TASK-3.1
- **Implementation steps:**
  1. Implement `generate_schema()` using `schemars::schema_for!`
  2. Serialize to `Value` via `serde_json::to_value`
  3. Sanitize: remove `$schema`, `title`, `description` from schema root (duplicate of tool description), ensure `additionalProperties: false`
  4. Test with a sample tool's Args
- **Edge cases:**
  - `Option<String>` → `{ "type": ["string", "null"] }` — some providers (Ollama) reject `null` type. For MVP, accept the standard output.
  - `Vec<String>` → `{ "type": "array", "items": { "type": "string" } }`
  - Enum with `#[serde(rename_all = "...")]` → schema uses renamed values
- **Definition of Done:** Schema generation produces valid, compatible JSON Schema
- **Acceptance criteria:**
  - `ReadArgs` schema includes `path: { type: "string" }`, `offset: { type: ["integer", "null"] }`
  - Schema has no `$schema` keyword at root
  - `required` array includes only non-Option fields
- **Test plan:** unit: Generate schemas for all 5 built-in tools, verify structure
- **Estimated effort:** 4 hours

---

### TASK-9.2: JSON parse step in dispatch — already valid Value, add type checks

- **§SPEC:** §8 (Parse JSON step)
- **Labels:** `layer/validation`, `priority/critical`
- **Description:** The `call.raw_args` is already `serde_json::Value` (parsed). Add structural validation: ensure the value is a JSON Object (`{...}`), not an array, string, or number. Tools expect key-value arguments. If not an object → `ToolError::InvalidArgs("arguments must be a JSON object")`.
- **Files affected:**
  - `crates/duga-tools/src/dispatcher.rs` (add check in dispatch)
- **Types involved:** `ToolCall`, `ToolError::InvalidArgs`
- **Functions to implement:** (inline check in dispatch)
- **Dependencies:** TASK-3.4
- **Implementation steps:**
  1. In `dispatch()`, before schema validation: `if !call.raw_args.is_object() { return Err(ToolError::InvalidArgs("arguments must be a JSON object, got ...".into())); }`
  2. Test
- **Edge cases:**
  - `null` → not an object → error
  - Empty object `{}` → valid (all fields use defaults)
- **Definition of Done:** Non-object args rejected
- **Acceptance criteria:**
  - `raw_args: json!("string")` → `Err(InvalidArgs)`
  - `raw_args: json!({})` → proceeds to schema validation
- **Test plan:** unit: Test array, string, number, null, empty object
- **Estimated effort:** 2 hours

---

### TASK-9.3: jsonschema validate step — error formatting

- **§SPEC:** §8 (Validate schema step)
- **Labels:** `layer/validation`, `priority/critical`
- **Description:** After TASK-3.5's basic jsonschema validation, improve error message formatting. Instead of raw jsonschema error objects, format them as: `"Invalid args: missing required field 'content' at /"` or `"Invalid args: expected string at /path, got integer"`. Each validation error becomes a semicolon-separated list in the error message.
- **Files affected:**
  - `crates/duga-tools/src/schema.rs` (enhance error formatting)
- **Types involved:** `ToolError::InvalidArgs`, jsonschema `ValidationError`
- **Functions to implement:**
  - `fn format_validation_errors(errors: &[ValidationError]) -> String`
  - `fn format_single_error(err: &ValidationError) -> String`
- **Dependencies:** TASK-3.5
- **Implementation steps:**
  1. Map jsonschema error kinds to human-readable strings:
      - `Required` → "missing required field '{field}'"
      - `Type` → "expected {expected}, got {actual}"
      - `Enum` → "value must be one of: {options}"
      - etc.
  2. Join multiple errors with "; "
  3. Prefix with "Invalid args: "
  4. Test with known bad inputs
- **Edge cases:**
  - 0 errors → (shouldn't happen, but return "unknown validation error")
  - Instance path `/` → "at root"
  - Instance path `/0/name` → "at [0].name"
  - Nested object field missing → "at /config.name"
- **Definition of Done:** Error messages are LLM-friendly
- **Acceptance criteria:**
  - Missing required field → "Invalid args: missing required field 'content' at /"
  - Wrong type → "Invalid args: expected string at /path, got integer"
- **Test plan:** unit: Test each error kind with known schema violations
- **Estimated effort:** 4 hours

---

### TASK-9.4: Typed deserialize from validated JSON — error wrapping

- **§SPEC:** §8 (Deserialize typed args step)
- **Labels:** `layer/validation`, `priority/critical`
- **Description:** After successful jsonschema validation, deserialize the `Value` into `T::Args`. Wrap serde deserialization errors in `ToolError::InvalidArgs` with clear messages. Since validation already passed, deserialization errors should be rare (e.g., custom deserializer logic). Format: `"Deserialization failed: {error}"`.
- **Files affected:**
  - `crates/duga-tools/src/dispatcher.rs` (deserialize step in ErasedTool)
- **Types involved:** `ToolError::InvalidArgs`
- **Functions to implement:** (inline in dispatch)
- **Dependencies:** TASK-3.5, TASK-9.3
- **Implementation steps:**
  1. In the erased execute closure: `serde_json::from_value::<T::Args>(validated_args)`
  2. Map error: `ToolError::InvalidArgs(format!("Deserialization failed: {}", e))`
  3. Test with a tool that has custom deserialization
- **Edge cases:**
  - Schema validated but custom deserialize rejects (e.g., `#[serde(deserialize_with = "validate_url")]`) → this error is surfaced
  - Stack overflow in deeply nested deserialization → serde limit guards this
- **Definition of Done:** Deserialization errors wrapped correctly
- **Estimated effort:** 2 hours

---

### TASK-9.5: End-to-end validation pipeline tests

- **§SPEC:** §8 (complete pipeline verification)
- **Labels:** `layer/validation`, `priority/critical`
- **Description:** Create comprehensive tests for the complete validation pipeline with a mock tool that has a non-trivial schema (nested objects, enums, arrays, optional fields). Test all failure modes: wrong type, missing required, extra fields (if additionalProperties: false), invalid enum value, array with wrong item type. Test successful path with all field combinations.
- **Files affected:**
  - `crates/duga-tools/tests/validation.rs` (new)
- **Types involved:** `ToolDispatcher`, mock tool
- **Dependencies:** TASK-9.1 through TASK-9.4
- **Implementation steps:**
  1. Define mock tool with complex Args struct
  2. Register in ToolDispatcher
  3. Test 10+ failure cases
  4. Test 5+ success cases
  5. Verify error messages are human-readable
- **Acceptance criteria:** All failure modes produce correct `ToolError::InvalidArgs`
- **Test plan:** unit tests as described
- **Estimated effort:** 4 hours
