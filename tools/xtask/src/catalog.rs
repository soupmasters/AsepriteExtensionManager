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

    let catalog: aem_helper::registry::Catalog = serde_json::from_value(catalog.clone())
        .context("deserialize catalog for semantic validation")?;
    aem_helper::registry::validate_catalog(&catalog).context("validate catalog semantics")?;

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

    fn catalog_schema() -> serde_json::Value {
        serde_json::from_str(include_str!(
            "../../../registry/schema/catalog-v1.schema.json"
        ))
        .unwrap()
    }

    fn snapshot_catalog() -> serde_json::Value {
        let commit = "1111111111111111111111111111111111111111";
        json!({
            "schemaVersion": 1,
            "generatedAt": "2026-08-30T00:00:00Z",
            "packages": [{
                "id": "sample",
                "manifestName": "sample",
                "displayName": "Sample",
                "summary": "Sample extension",
                "author": { "name": "Author" },
                "license": "MIT",
                "homepage": "https://example.com/sample",
                "repository": "https://github.com/example/sample",
                "releases": [{
                    "version": "1.2.3",
                    "aseprite": {
                        "minimumVersion": "1.3.15",
                        "minimumApi": 35
                    },
                    "asset": {
                        "url": format!(
                            "https://codeload.github.com/example/sample/zip/{commit}"
                        ),
                        "sha256": "0".repeat(64),
                        "byteLength": 1,
                        "commit": commit
                    },
                    "publishedAt": "2026-08-30T00:00:00Z",
                    "releaseNotes": "Sample release",
                    "yanked": false
                }]
            }]
        })
    }

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

    #[test]
    fn rejects_codeload_snapshot_without_commit_after_schema_validation() {
        let mut catalog = snapshot_catalog();
        catalog["packages"][0]["releases"][0]["asset"]
            .as_object_mut()
            .unwrap()
            .remove("commit");

        let error = validate(&catalog_schema(), &catalog).unwrap_err();
        assert!(
            format!("{error:#}").contains("codeload snapshots must declare their immutable commit")
        );
    }

    #[test]
    fn rejects_snapshot_repository_and_codeload_mismatch_after_schema_validation() {
        let mut catalog = snapshot_catalog();
        catalog["packages"][0]["repository"] =
            serde_json::Value::String("https://github.com/other/sample".to_owned());

        let error = validate(&catalog_schema(), &catalog).unwrap_err();
        assert!(
            format!("{error:#}").contains("snapshot URL does not match its repository and commit")
        );
    }
}
