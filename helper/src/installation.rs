use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::Value;

use crate::installed;
use crate::package::{self, PreparedPackage};
use crate::protocol::{RpcError, RpcResult};
use crate::state::{Receipt, State};

const MANAGER_NAME: &str = "aseprite-extension-manager";

pub fn verify_and_record(
    user_config: &Path,
    state: &State,
    prepared: PreparedPackage,
    source: Value,
) -> RpcResult<Value> {
    if prepared.name.eq_ignore_ascii_case(MANAGER_NAME) {
        return Err(RpcError::invalid(
            "SELF_UPDATE_RESTRICTED",
            "use the manager's dedicated update action for Aseprite Extension Manager releases",
        ));
    }

    let installed = match installed::verify(user_config, state, &prepared.name, &prepared.version) {
        Ok(installed) => installed,
        Err(error) if error.code == "INSTALL_VERIFICATION_FAILED" => {
            return verification_failure(user_config, state, &prepared, error.message);
        }
        Err(error) => return Err(error),
    };

    if let Err(error) = package::verify_installed_artifact(
        &prepared.artifact_path,
        &installed.path,
        &prepared.name,
        &prepared.version,
    ) {
        if error.code == "INSTALL_CONTENT_MISMATCH" {
            return verification_failure(user_config, state, &prepared, error.message);
        }
        return Err(error);
    }

    let prior_receipt = state.read_receipt(&prepared.name)?;
    let (current, previous) = state.cache_artifact(&prepared.name, &prepared.artifact_path)?;
    let (
        previous_source,
        previous_version,
        previous_artifact_sha256,
        previous_artifact_byte_length,
    ) = previous_identity(prior_receipt.as_ref(), &prepared.sha256, previous.is_some());
    let source_kind = source
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    let receipt = Receipt {
        schema_version: 1,
        package_name: prepared.name,
        source_kind,
        commit: string_field(&source, "commit"),
        release: string_field(&source, "release"),
        asset: string_field(&source, "assetName"),
        installed_version: prepared.version,
        artifact_sha256: prepared.sha256,
        artifact_byte_length: prepared.byte_length,
        installed_at: Utc::now(),
        local_folder: source
            .get("packageJsonPath")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .and_then(|path| path.parent().map(Path::to_owned)),
        content_hash: source
            .get("contentHash")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        previous_artifact: previous,
        previous_source,
        previous_version,
        previous_artifact_sha256,
        previous_artifact_byte_length,
        source,
    };
    let receipt_path = state.write_receipt(&receipt)?;
    Ok(serde_json::json!({
        "verified": true,
        "receipt": receipt,
        "receiptPath": receipt_path,
        "cachedArtifact": current
    }))
}

fn verification_failure(
    user_config: &Path,
    state: &State,
    prepared: &PreparedPackage,
    message: String,
) -> RpcResult<Value> {
    let previous_receipt = state.read_receipt(&prepared.name)?;
    let currently_installed = installed::find(user_config, state, &prepared.name)?;
    let current_intact = previous_receipt
        .as_ref()
        .zip(currently_installed.as_ref())
        .is_some_and(|(receipt, installed)| receipt.installed_version == installed.version);
    Ok(serde_json::json!({
        "verified": false,
        "message": message,
        "currentIntact": current_intact,
        "rollbackAvailable": !current_intact
            && state.cached_artifact(&prepared.name, false)?.is_some()
    }))
}

fn string_field(value: &Value, name: &str) -> Option<String> {
    value
        .get(name)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

pub(crate) fn previous_identity(
    prior: Option<&Receipt>,
    prepared_sha256: &str,
    has_previous_artifact: bool,
) -> (Option<Value>, Option<String>, Option<String>, Option<u64>) {
    if !has_previous_artifact {
        return (None, None, None, None);
    }
    let Some(prior) = prior else {
        return (None, None, None, None);
    };
    if prior.artifact_sha256.eq_ignore_ascii_case(prepared_sha256) {
        (
            prior.previous_source.clone(),
            prior.previous_version.clone(),
            prior.previous_artifact_sha256.clone(),
            prior.previous_artifact_byte_length,
        )
    } else {
        (
            Some(prior.source.clone()),
            Some(prior.installed_version.clone()),
            Some(prior.artifact_sha256.clone()),
            Some(prior.artifact_byte_length),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn duplicate_installs_cannot_create_a_receipt() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let extensions = temporary.path().join("extensions");
        for folder in ["sample-one", "sample-two"] {
            let extension = extensions.join(folder);
            fs::create_dir_all(&extension).expect("mkdir");
            fs::write(
                extension.join("package.json"),
                br#"{"name":"sample","version":"1.0.0"}"#,
            )
            .expect("manifest");
        }
        let state = State::new(temporary.path()).expect("state");
        let prepared = PreparedPackage {
            artifact_path: temporary.path().join("sample.aseprite-extension"),
            name: "sample".to_owned(),
            display_name: Some("Sample".to_owned()),
            version: "1.0.0".to_owned(),
            sha256: "0".repeat(64),
            byte_length: 1,
            content_hash: None,
        };

        let result = verify_and_record(
            temporary.path(),
            &state,
            prepared,
            serde_json::json!({"kind":"local-folder"}),
        )
        .expect("verification result");

        assert_eq!(result["verified"], false);
        assert_eq!(
            result["message"],
            "more than one installed extension uses the expected package name"
        );
        assert!(state
            .read_receipt("sample")
            .expect("read receipt")
            .is_none());
    }
}
