//! Schema validation helpers.

use jsonschema::Validator;
use serde_json::Value;

pub fn validate_schema(schema: &Value, instance: &Value) -> Result<(), Vec<String>> {
    let compiled = Validator::new(schema).map_err(|e| vec![e.to_string()])?;

    let errors: Vec<String> = compiled
        .iter_errors(instance)
        .map(|e| format!("{}: {}", e.instance_path, e))
        .collect();

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

pub fn deserialize_args<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|e| e.to_string())
}
