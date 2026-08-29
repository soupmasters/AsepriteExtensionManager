use std::{fs, path::Path};

use anyhow::{bail, Context, Result};
use serde_json::Value;

pub fn validate_files(schema_path: &Path, catalog_path: &Path) -> Result<()> {
    let schema = read_json(schema_path)?;
    let catalog = read_json(catalog_path)?;
    validate(&schema, &catalog)
        .with_context(|| format!("catalog {} is invalid", catalog_path.display()))
}

pub fn validate(schema: &Value, catalog: &Value) -> Result<()> {
    let validator = jsonschema::options()
        .with_draft(jsonschema::Draft::Draft7)
        .build(schema)
        .context("compile catalog schema")?;

    let errors: Vec<String> = validator
        .iter_errors(catalog)
        .map(|error| format!("{}: {}", error.instance_path, error))
        .collect();

    if !errors.is_empty() {
        bail!("{}", errors.join("\n"));
    }

    Ok(())
}

fn read_json(path: &Path) -> Result<Value> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::validate;

    #[test]
    fn reports_invalid_catalog() {
        let schema = json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "required": ["schemaVersion"],
            "properties": {
                "schemaVersion": { "const": 1 }
            }
        });

        let error = validate(&schema, &json!({ "schemaVersion": 2 })).unwrap_err();
        assert!(error.to_string().contains("schemaVersion"));
    }
}
