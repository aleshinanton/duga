//! Reference plugin scaffold for the `duga:plugin` WIT contract.

use duga_plugin_abi::{Invocation, Outcome, ToolInfo};

pub fn info() -> ToolInfo {
    ToolInfo {
        name: "format-code".into(),
        description: "Validate and format a source file in the workspace".into(),
        args_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "file": { "type": "string" }
            },
            "required": ["file"],
            "additionalProperties": false
        })
        .to_string(),
    }
}

pub fn execute(invocation: Invocation) -> Result<Outcome, String> {
    let args: serde_json::Value =
        serde_json::from_str(&invocation.args_json).map_err(|e| e.to_string())?;
    let file = args
        .get("file")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "missing string field 'file'".to_string())?;

    Ok(Outcome {
        success: true,
        output: format!("validated formatting request for {file}"),
        metadata_json: serde_json::json!({
            "workspace_root": invocation.workspace_root,
            "file": file
        })
        .to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_has_expected_name() {
        assert_eq!(info().name, "format-code");
    }

    #[test]
    fn execute_validates_json_args() {
        let outcome = execute(Invocation {
            args_json: serde_json::json!({"file": "src/lib.rs"}).to_string(),
            workspace_root: ".".into(),
        })
        .unwrap();

        assert!(outcome.success);
        assert!(outcome.output.contains("src/lib.rs"));
    }
}

