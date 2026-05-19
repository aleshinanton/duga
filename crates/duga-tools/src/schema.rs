//! Schema validation helpers.

use jsonschema::Validator;
use schemars::JsonSchema;
use serde_json::{Map, Value};

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

pub fn generate_args_schema<T: JsonSchema>() -> Value {
    let schema = schemars::schema_for!(T);
    let mut value = serde_json::to_value(schema).expect("schema is always serializable");
    sanitize_schema(&mut value);
    value
}

pub fn sanitize_schema(schema: &mut Value) {
    match schema {
        Value::Object(map) => {
            map.remove("$schema");
            map.remove("title");
            ensure_object_schema_defaults(map);

            for value in map.values_mut() {
                sanitize_schema(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                sanitize_schema(value);
            }
        }
        _ => {}
    }
}

pub fn validate_tool_args(schema: &Value, raw_args: &Value) -> Result<(), String> {
    if !raw_args.is_object() {
        return Err(format!(
            "Tool arguments must be a JSON object, got {}",
            json_type_name(raw_args)
        ));
    }

    validate_schema(schema, raw_args).map_err(|errors| {
        let formatted = errors
            .into_iter()
            .map(|error| format!("- {error}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!("Schema validation failed:\n{formatted}")
    })
}

pub fn deserialize_args<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|e| e.to_string())
}

fn ensure_object_schema_defaults(map: &mut Map<String, Value>) {
    let is_object = map.get("type").and_then(Value::as_str) == Some("object");
    let has_properties = map.get("properties").is_some();
    if is_object || has_properties {
        map.entry("type")
            .or_insert_with(|| Value::String("object".into()));
        map.entry("additionalProperties")
            .or_insert_with(|| Value::Bool(false));
        // Add 'label' to properties if not present, so all tools accept it.
        // Built-in tools declare it in their args struct; custom/plugin tools
        // don't need to — the field is silently accepted and ignored.
        if let Some(props) = map.get_mut("properties") {
            if let Some(obj) = props.as_object_mut() {
                obj.entry("label".to_string()).or_insert_with(|| {
                    serde_json::json!({"type": "string", "description": "Brief human-readable description of what this step does (shown to user)"})
                });
            }
        }
    }
}

fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use schemars::JsonSchema;
    use serde::Deserialize;

    #[allow(dead_code)]
    #[derive(Debug, Deserialize, JsonSchema)]
    struct Args {
        msg: String,
    }

    #[test]
    fn generated_schema_is_sanitized_and_strict() {
        let schema = generate_args_schema::<Args>();
        assert!(schema.get("$schema").is_none());
        assert!(schema.get("title").is_none());
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
    }

    #[test]
    fn validate_tool_args_requires_object() {
        let schema = generate_args_schema::<Args>();
        let err = validate_tool_args(&schema, &Value::String("bad".into())).unwrap_err();
        assert!(err.contains("must be a JSON object"));
    }

    #[test]
    fn validate_tool_args_formats_schema_errors() {
        let schema = generate_args_schema::<Args>();
        let err = validate_tool_args(&schema, &serde_json::json!({})).unwrap_err();
        assert!(err.contains("Schema validation failed"));
        assert!(err.contains("required"));

        let err = validate_tool_args(
            &schema,
            &serde_json::json!({"msg": "ok", "unexpected": true}),
        )
        .unwrap_err();
        assert!(err.contains("Schema validation failed"));
        assert!(err.contains("unexpected"));
    }
}
