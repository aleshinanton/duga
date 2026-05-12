//! Schema validation helpers.
//!
//! Provides JSON Schema validation using the `jsonschema` crate.

use jsonschema::JSONSchema;
use serde_json::Value;

/// Validate `instance` against `schema`.
///
/// Returns `Ok(())` on success, or `Err` with all validation error messages.
pub fn validate_schema(schema: &Value, instance: &Value) -> Result<(), Vec<String>> {
    let compiled = JSONSchema::compile(schema, Some(schema.clone().into()))
        .map_err(|e| vec![e.to_string()])?;

    let result = compiled.validate(instance);
    match result {
        Ok(_) => Ok(()),
        Err(errors) => {
            let messages: Vec<String> = errors
                .map(|e| format!("{}: {}", e.instance_path, e))
                .collect();
            Err(messages)
        }
    }
}

/// Deserialize a JSON `Value` into type `T`.
///
/// Returns the deserialized value or a descriptive error string.
pub fn deserialize_args<T: serde::de::DeserializeOwned>(
    value: Value,
) -> Result<T, String> {
    serde_json::from_value(value).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Deserialize, Serialize, JsonSchema)]
    struct Args {
        name: String,
        content: Option<String>,
    }

    #[test]
    fn test_validate_schema_valid() {
        let schema = schemars::schema_for!(Args);
        let schema_value = serde_json::to_value(schema).unwrap();
        let instance = serde_json::json!({"name": "test.txt", "content": "hello"});

        let result = validate_schema(&schema_value, &instance);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_schema_missing_required() {
        let schema = schemars::schema_for!(Args);
        let schema_value = serde_json::to_value(schema).unwrap();
        let instance = serde_json::json!({"content": "hello"}); // missing "name"

        let result = validate_schema(&schema_value, &instance);
        assert!(result.is_err());
        assert!(!result.unwrap_err().is_empty());
    }

    #[test]
    fn test_deserialize_args_valid() {
        let value = serde_json::json!({"name": "test.txt", "content": "hello"});
        let args: Args = deserialize_args(value).unwrap();
        assert_eq!(args.name, "test.txt");
        assert_eq!(args.content, Some("hello".into()));
    }

    #[test]
    fn test_deserialize_args_invalid() {
        let value = serde_json::json!({"name": 123, "content": true});
        let result: Result<Args, String> = deserialize_args(value);
        assert!(result.is_err());
    }
}