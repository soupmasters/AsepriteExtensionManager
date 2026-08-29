use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::package::Manifest;
use crate::protocol::{RpcError, RpcResult};
use crate::state::{Receipt, State};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledPackage {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub version: String,
    pub path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    pub managed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_error: Option<Value>,
    pub rollback_available: bool,
}

pub fn scan(user_config_path: &Path, state: &State) -> RpcResult<Vec<InstalledPackage>> {
    let extensions = user_config_path.join("extensions");
    if !extensions.exists() {
        return Ok(Vec::new());
    }
    let receipts = state.receipts()?;
    let mut packages = Vec::new();
    for entry in fs::read_dir(&extensions).map_err(RpcError::io)? {
        let entry = entry.map_err(RpcError::io)?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(RpcError::io)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            continue;
        }
        let manifest_path = entry.path().join("package.json");
        if !manifest_path.is_file() {
            continue;
        }
        let manifest = match read_manifest(&manifest_path) {
            Ok(manifest) => manifest,
            Err(_) => continue,
        };
        let receipt = receipts.iter().find(|receipt| {
            receipt.package_name.eq_ignore_ascii_case(&manifest.name)
                && receipt.installed_version == manifest.version
        });
        let rollback_available =
            receipt.is_some() && state.cached_artifact(&manifest.name, true)?.is_some();
        packages.push(InstalledPackage {
            name: manifest.name,
            display_name: manifest.display_name,
            version: manifest.version,
            path: entry.path(),
            enabled: None,
            managed: receipt.is_some(),
            source: receipt.map(|receipt| receipt.source.clone()),
            update: None,
            update_error: None,
            rollback_available,
        });
    }
    packages.sort_by(|left, right| {
        left.display_name
            .as_deref()
            .unwrap_or(&left.name)
            .to_lowercase()
            .cmp(
                &right
                    .display_name
                    .as_deref()
                    .unwrap_or(&right.name)
                    .to_lowercase(),
            )
    });
    Ok(packages)
}

pub fn find(
    user_config_path: &Path,
    state: &State,
    name: &str,
) -> RpcResult<Option<InstalledPackage>> {
    Ok(scan(user_config_path, state)?
        .into_iter()
        .find(|package| package.name.eq_ignore_ascii_case(name)))
}

pub fn verify(
    user_config_path: &Path,
    state: &State,
    expected_name: &str,
    expected_version: &str,
) -> RpcResult<InstalledPackage> {
    let package = find(user_config_path, state, expected_name)?.ok_or_else(|| {
        RpcError::invalid(
            "INSTALL_VERIFICATION_FAILED",
            "Aseprite did not install the expected package",
        )
    })?;
    if package.version != expected_version {
        return Err(RpcError::invalid(
            "INSTALL_VERIFICATION_FAILED",
            "the installed manifest version does not match the prepared package",
        )
        .with_details(serde_json::json!({
            "expectedVersion": expected_version,
            "actualVersion": package.version
        })));
    }
    Ok(package)
}

pub fn attach_updates(
    packages: &mut [InstalledPackage],
    releases: &std::collections::BTreeMap<String, Value>,
) {
    for package in packages {
        if let Some(release) = releases.get(&package.name.to_lowercase()) {
            package.update = Some(release.clone());
        }
    }
}

pub fn attach_update_errors(
    packages: &mut [InstalledPackage],
    errors: &std::collections::BTreeMap<String, Value>,
) {
    for package in packages {
        if let Some(error) = errors.get(&package.name.to_lowercase()) {
            package.update_error = Some(error.clone());
        }
    }
}

pub fn receipt_for<'a>(receipts: &'a [Receipt], name: &str) -> Option<&'a Receipt> {
    receipts
        .iter()
        .find(|receipt| receipt.package_name.eq_ignore_ascii_case(name))
}

fn read_manifest(path: &Path) -> RpcResult<Manifest> {
    let bytes = fs::read(path).map_err(RpcError::io)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| RpcError::invalid("INVALID_INSTALLED_MANIFEST", error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn discovers_unmanaged_extensions_without_mutating_them() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let extension = temporary.path().join("extensions").join("Folder Name");
        fs::create_dir_all(&extension).expect("mkdir");
        fs::write(
            extension.join("package.json"),
            br#"{"name":"sample","displayName":"Sample","version":"1.0"}"#,
        )
        .expect("write");
        let before = fs::read(extension.join("package.json")).expect("before");
        let state = State::new(temporary.path()).expect("state");

        let packages = scan(temporary.path(), &state).expect("scan");

        assert_eq!(packages.len(), 1);
        assert!(!packages[0].managed);
        assert_eq!(
            fs::read(extension.join("package.json")).expect("after"),
            before
        );
    }

    #[test]
    fn stale_receipt_does_not_manage_a_different_installed_version() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let extension = temporary.path().join("extensions").join("sample");
        fs::create_dir_all(&extension).expect("mkdir");
        fs::write(
            extension.join("package.json"),
            br#"{"name":"sample","version":"2.0.0"}"#,
        )
        .expect("manifest");
        let state = State::new(temporary.path()).expect("state");
        state
            .write_receipt(&Receipt {
                schema_version: 1,
                package_name: "sample".to_owned(),
                source_kind: "github-release".to_owned(),
                source: serde_json::json!({"kind":"github-release"}),
                installed_version: "1.0.0".to_owned(),
                commit: None,
                release: Some("v1.0.0".to_owned()),
                asset: None,
                artifact_sha256: "0".repeat(64),
                artifact_byte_length: 1,
                installed_at: Utc::now(),
                local_folder: None,
                content_hash: None,
                previous_artifact: None,
                previous_source: None,
                previous_version: None,
                previous_artifact_sha256: None,
                previous_artifact_byte_length: None,
            })
            .expect("receipt");

        let packages = scan(temporary.path(), &state).expect("scan");

        assert_eq!(packages.len(), 1);
        assert!(!packages[0].managed);
        assert!(packages[0].source.is_none());
        assert!(!packages[0].rollback_available);
    }

    #[test]
    fn malformed_receipt_does_not_block_valid_and_unmanaged_extensions() {
        let temporary = tempfile::tempdir().expect("tempdir");
        for (folder, name) in [("managed", "managed"), ("unmanaged", "unmanaged")] {
            let extension = temporary.path().join("extensions").join(folder);
            fs::create_dir_all(&extension).expect("mkdir");
            fs::write(
                extension.join("package.json"),
                format!(r#"{{"name":"{name}","version":"1.0.0"}}"#),
            )
            .expect("manifest");
        }
        let state = State::new(temporary.path()).expect("state");
        state
            .write_receipt(&Receipt {
                schema_version: 1,
                package_name: "managed".to_owned(),
                source_kind: "github-release".to_owned(),
                source: serde_json::json!({"kind":"github-release"}),
                installed_version: "1.0.0".to_owned(),
                commit: None,
                release: Some("v1.0.0".to_owned()),
                asset: None,
                artifact_sha256: "0".repeat(64),
                artifact_byte_length: 1,
                installed_at: Utc::now(),
                local_folder: None,
                content_hash: None,
                previous_artifact: None,
                previous_source: None,
                previous_version: None,
                previous_artifact_sha256: None,
                previous_artifact_byte_length: None,
            })
            .expect("receipt");
        fs::write(
            state.root().join("receipts").join("broken.json"),
            b"{not valid json",
        )
        .expect("malformed receipt");

        let packages = scan(temporary.path(), &state).expect("scan");

        assert_eq!(packages.len(), 2);
        assert!(
            packages
                .iter()
                .find(|package| package.name == "managed")
                .expect("managed package")
                .managed
        );
        assert!(
            !packages
                .iter()
                .find(|package| package.name == "unmanaged")
                .expect("unmanaged package")
                .managed
        );
    }
}
