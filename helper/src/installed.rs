use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::package::Manifest;
use crate::protocol::{RpcError, RpcResult};
use crate::state::{package_id_is_safe, Receipt, State};

const MANAGER_NAME: &str = "aseprite-extension-manager";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledPackage {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub version: String,
    pub path: PathBuf,
    pub is_self: bool,
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

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallResult {
    pub name: String,
    pub version: String,
    pub recovery_path: PathBuf,
    pub restart_required: bool,
    pub receipt_cleanup_pending: bool,
}

pub fn scan(user_config_path: &Path, state: &State) -> RpcResult<Vec<InstalledPackage>> {
    scan_inner(user_config_path, state, None)
}

pub fn scan_with_manager_root(
    user_config_path: &Path,
    state: &State,
    manager_root: &Path,
) -> RpcResult<Vec<InstalledPackage>> {
    scan_inner(user_config_path, state, Some(manager_root))
}

pub fn uninstall(
    user_config_path: &Path,
    state: &State,
    manager_root: &Path,
    expected_name: &str,
    expected_version: &str,
    requested_path: &Path,
) -> RpcResult<UninstallResult> {
    if expected_name.eq_ignore_ascii_case(MANAGER_NAME) {
        return Err(RpcError::invalid(
            "SELF_UNINSTALL_RESTRICTED",
            "Aseprite Extension Manager cannot uninstall itself",
        ));
    }

    let requested_metadata = fs::symlink_metadata(requested_path).map_err(RpcError::io)?;
    if requested_metadata.file_type().is_symlink() || !requested_metadata.is_dir() {
        return Err(RpcError::invalid(
            "INVALID_EXTENSION_PATH",
            "the installed extension must be a real directory",
        ));
    }

    let extensions_root =
        fs::canonicalize(user_config_path.join("extensions")).map_err(RpcError::io)?;
    let extension_path = fs::canonicalize(requested_path).map_err(RpcError::io)?;
    if extension_path.parent() != Some(extensions_root.as_path()) {
        return Err(RpcError::invalid(
            "UNTRUSTED_EXTENSION_PATH",
            "the extension must be a direct child of the Aseprite extensions directory",
        ));
    }
    let canonical_manager_root = fs::canonicalize(manager_root).map_err(RpcError::io)?;
    if extension_path == canonical_manager_root {
        return Err(RpcError::invalid(
            "SELF_UNINSTALL_RESTRICTED",
            "Aseprite Extension Manager cannot uninstall itself",
        ));
    }

    let manifest_path = extension_path.join("package.json");
    let manifest_metadata = fs::symlink_metadata(&manifest_path).map_err(RpcError::io)?;
    if manifest_metadata.file_type().is_symlink() || !manifest_metadata.is_file() {
        return Err(RpcError::invalid(
            "INVALID_INSTALLED_MANIFEST",
            "the installed package manifest must be a real file",
        ));
    }
    let manifest = read_manifest(&manifest_path)?;
    if !manifest.name.eq_ignore_ascii_case(expected_name) || manifest.version != expected_version {
        return Err(RpcError::invalid(
            "INSTALLED_PACKAGE_CHANGED",
            "the installed extension changed after it was scanned; refresh and try again",
        )
        .with_details(serde_json::json!({
            "expectedName": expected_name,
            "actualName": manifest.name,
            "expectedVersion": expected_version,
            "actualVersion": manifest.version,
        })));
    }

    let same_name_count = scan(user_config_path, state)?
        .iter()
        .filter(|package| package.name.eq_ignore_ascii_case(&manifest.name))
        .count();
    let archive_receipt = same_name_count == 1
        && package_id_is_safe(&manifest.name)
        && state.read_receipt(&manifest.name)?.is_some_and(|receipt| {
            receipt.package_name.eq_ignore_ascii_case(&manifest.name)
                && receipt.installed_version == manifest.version
        });
    let quarantine = state.quarantine_extension(
        &manifest.name,
        &manifest.version,
        &extension_path,
        archive_receipt,
    )?;
    Ok(UninstallResult {
        name: manifest.name,
        version: manifest.version,
        recovery_path: quarantine.recovery_path,
        restart_required: true,
        receipt_cleanup_pending: quarantine.receipt_cleanup_pending,
    })
}

fn scan_inner(
    user_config_path: &Path,
    state: &State,
    manager_root: Option<&Path>,
) -> RpcResult<Vec<InstalledPackage>> {
    let extensions = user_config_path.join("extensions");
    if !extensions.exists() {
        return Ok(Vec::new());
    }
    let canonical_manager_root = manager_root.and_then(|root| fs::canonicalize(root).ok());
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
        let canonical_package_path = canonical_manager_root
            .as_ref()
            .and_then(|_| fs::canonicalize(entry.path()).ok());
        let is_self = manifest.name.eq_ignore_ascii_case(MANAGER_NAME)
            && canonical_manager_root
                .as_ref()
                .zip(canonical_package_path.as_ref())
                .is_some_and(|(manager, package)| manager == package);
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
            is_self,
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
    let mut matches = scan(user_config_path, state)?
        .into_iter()
        .filter(|package| package.name.eq_ignore_ascii_case(expected_name))
        .collect::<Vec<_>>();
    if matches.len() > 1 {
        let candidates = matches
            .iter()
            .map(|package| {
                serde_json::json!({
                    "name": package.name,
                    "version": package.version,
                    "path": package.path,
                })
            })
            .collect::<Vec<_>>();
        return Err(RpcError::invalid(
            "INSTALL_VERIFICATION_FAILED",
            "more than one installed extension uses the expected package name",
        )
        .with_details(serde_json::json!({
            "expectedName": expected_name,
            "matchCount": candidates.len(),
            "candidates": candidates,
        })));
    }
    let package = matches.pop().ok_or_else(|| {
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
            if release.get("kind").and_then(Value::as_str) == Some("self") && !package.is_self {
                continue;
            }
            package.update = Some(release.clone());
        }
    }
}

pub fn attach_update_errors(
    packages: &mut [InstalledPackage],
    errors: &std::collections::BTreeMap<String, Value>,
) {
    for package in packages {
        if package.name.eq_ignore_ascii_case(MANAGER_NAME) && !package.is_self {
            continue;
        }
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

    fn receipt(name: &str, version: &str) -> Receipt {
        Receipt {
            schema_version: 1,
            package_name: name.to_owned(),
            source_kind: "github-release".to_owned(),
            source: serde_json::json!({"kind":"github-release"}),
            installed_version: version.to_owned(),
            commit: None,
            release: Some(format!("v{version}")),
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
        }
    }

    #[test]
    fn uninstall_quarantines_the_exact_extension_and_its_receipt() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let extensions = temporary.path().join("extensions");
        let manager = extensions.join("manager");
        let extension = extensions.join("Manual Folder");
        fs::create_dir_all(&manager).expect("manager mkdir");
        fs::create_dir_all(&extension).expect("extension mkdir");
        fs::write(
            extension.join("package.json"),
            br#"{"name":"sample","version":"1.0.0"}"#,
        )
        .expect("manifest");
        fs::write(extension.join("custom.lua"), b"return true").expect("custom file");
        let state = State::new(temporary.path()).expect("state");
        state
            .write_receipt(&receipt("sample", "1.0.0"))
            .expect("receipt");

        let result = uninstall(
            temporary.path(),
            &state,
            &manager,
            "sample",
            "1.0.0",
            &extension,
        )
        .expect("uninstall");

        assert_eq!(result.name, "sample");
        assert_eq!(result.version, "1.0.0");
        assert!(result.restart_required);
        assert!(!result.receipt_cleanup_pending);
        assert!(!extension.exists());
        assert!(result.recovery_path.join("package.json").is_file());
        assert!(result.recovery_path.join("custom.lua").is_file());
        assert!(result
            .recovery_path
            .parent()
            .expect("recovery directory")
            .join("receipt.json")
            .is_file());
        assert!(state
            .read_receipt("sample")
            .expect("read receipt")
            .is_none());
        assert!(scan(temporary.path(), &state).expect("rescan").is_empty());
    }

    #[test]
    fn uninstall_quarantines_two_distinct_extensions_sequentially() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let extensions = temporary.path().join("extensions");
        let manager = extensions.join("manager");
        let first = extensions.join("first");
        let second = extensions.join("second");
        for directory in [&manager, &first, &second] {
            fs::create_dir_all(directory).expect("mkdir");
        }
        fs::write(
            first.join("package.json"),
            br#"{"name":"first","version":"1.0.0"}"#,
        )
        .expect("first manifest");
        fs::write(
            second.join("package.json"),
            br#"{"name":"second","version":"2.0.0"}"#,
        )
        .expect("second manifest");
        let state = State::new(temporary.path()).expect("state");
        state
            .write_receipt(&receipt("first", "1.0.0"))
            .expect("first receipt");
        state
            .write_receipt(&receipt("second", "2.0.0"))
            .expect("second receipt");

        let first_result = uninstall(temporary.path(), &state, &manager, "first", "1.0.0", &first)
            .expect("uninstall first");
        let second_result = uninstall(
            temporary.path(),
            &state,
            &manager,
            "second",
            "2.0.0",
            &second,
        )
        .expect("uninstall second");

        assert!(!first.exists());
        assert!(!second.exists());
        assert_ne!(first_result.recovery_path, second_result.recovery_path);
        assert!(first_result.recovery_path.join("package.json").is_file());
        assert!(second_result.recovery_path.join("package.json").is_file());
        assert!(state
            .read_receipt("first")
            .expect("read first receipt")
            .is_none());
        assert!(state
            .read_receipt("second")
            .expect("read second receipt")
            .is_none());
        assert!(scan(temporary.path(), &state).expect("rescan").is_empty());
    }

    #[test]
    fn uninstall_preserves_a_stale_receipt() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let extensions = temporary.path().join("extensions");
        let manager = extensions.join("manager");
        let extension = extensions.join("sample");
        fs::create_dir_all(&manager).expect("manager mkdir");
        fs::create_dir_all(&extension).expect("extension mkdir");
        fs::write(
            extension.join("package.json"),
            br#"{"name":"sample","version":"2.0.0"}"#,
        )
        .expect("manifest");
        let state = State::new(temporary.path()).expect("state");
        state
            .write_receipt(&receipt("sample", "1.0.0"))
            .expect("stale receipt");

        let result = uninstall(
            temporary.path(),
            &state,
            &manager,
            "sample",
            "2.0.0",
            &extension,
        )
        .expect("uninstall");

        assert!(!result.receipt_cleanup_pending);
        assert_eq!(
            state
                .read_receipt("sample")
                .expect("read receipt")
                .expect("preserved receipt")
                .installed_version,
            "1.0.0"
        );
        assert!(!result
            .recovery_path
            .parent()
            .expect("recovery directory")
            .join("receipt.json")
            .exists());
    }

    #[test]
    fn uninstall_preserves_a_receipt_with_a_mismatched_identity() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let extensions = temporary.path().join("extensions");
        let manager = extensions.join("manager");
        let extension = extensions.join("sample");
        fs::create_dir_all(&manager).expect("manager mkdir");
        fs::create_dir_all(&extension).expect("extension mkdir");
        fs::write(
            extension.join("package.json"),
            br#"{"name":"sample","version":"1.0.0"}"#,
        )
        .expect("manifest");
        let state = State::new(temporary.path()).expect("state");
        let mismatched = receipt("different-package", "1.0.0");
        fs::write(
            state.root().join("receipts/sample.json"),
            serde_json::to_vec_pretty(&mismatched).expect("receipt JSON"),
        )
        .expect("mismatched receipt");

        let result = uninstall(
            temporary.path(),
            &state,
            &manager,
            "sample",
            "1.0.0",
            &extension,
        )
        .expect("uninstall");

        assert!(!result.receipt_cleanup_pending);
        assert_eq!(
            state
                .read_receipt("sample")
                .expect("read receipt")
                .expect("preserved receipt")
                .package_name,
            "different-package"
        );
        assert!(!result
            .recovery_path
            .parent()
            .expect("recovery directory")
            .join("receipt.json")
            .exists());
    }

    #[test]
    fn uninstall_preserves_a_receipt_shared_by_duplicate_installs() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let extensions = temporary.path().join("extensions");
        let manager = extensions.join("manager");
        let first = extensions.join("sample-one");
        let second = extensions.join("sample-two");
        fs::create_dir_all(&manager).expect("manager mkdir");
        for extension in [&first, &second] {
            fs::create_dir_all(extension).expect("extension mkdir");
            fs::write(
                extension.join("package.json"),
                br#"{"name":"sample","version":"1.0.0"}"#,
            )
            .expect("manifest");
        }
        let state = State::new(temporary.path()).expect("state");
        state
            .write_receipt(&receipt("sample", "1.0.0"))
            .expect("receipt");

        let result = uninstall(
            temporary.path(),
            &state,
            &manager,
            "sample",
            "1.0.0",
            &first,
        )
        .expect("uninstall one duplicate");

        assert!(!first.exists());
        assert!(second.is_dir());
        assert!(!result.receipt_cleanup_pending);
        assert!(state
            .read_receipt("sample")
            .expect("read receipt")
            .is_some());
        assert!(!result
            .recovery_path
            .parent()
            .expect("recovery directory")
            .join("receipt.json")
            .exists());
    }

    #[test]
    fn uninstall_rejects_changed_or_untrusted_targets_without_moving_them() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let extensions = temporary.path().join("extensions");
        let manager = extensions.join("manager");
        let extension = extensions.join("sample");
        let outside = temporary.path().join("outside");
        for directory in [&manager, &extension, &outside] {
            fs::create_dir_all(directory).expect("mkdir");
        }
        for directory in [&extension, &outside] {
            fs::write(
                directory.join("package.json"),
                br#"{"name":"sample","version":"2.0.0"}"#,
            )
            .expect("manifest");
        }
        let state = State::new(temporary.path()).expect("state");

        let changed = uninstall(
            temporary.path(),
            &state,
            &manager,
            "sample",
            "1.0.0",
            &extension,
        )
        .expect_err("version mismatch");
        assert_eq!(changed.code, "INSTALLED_PACKAGE_CHANGED");
        assert!(extension.is_dir());

        let untrusted = uninstall(
            temporary.path(),
            &state,
            &manager,
            "sample",
            "2.0.0",
            &outside,
        )
        .expect_err("outside path");
        assert_eq!(untrusted.code, "UNTRUSTED_EXTENSION_PATH");
        assert!(outside.is_dir());
    }

    #[test]
    fn uninstall_never_moves_the_manager() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let manager = temporary.path().join("extensions").join("manager");
        fs::create_dir_all(&manager).expect("manager mkdir");
        fs::write(
            manager.join("package.json"),
            br#"{"name":"aseprite-extension-manager","version":"0.1.0"}"#,
        )
        .expect("manifest");
        let state = State::new(temporary.path()).expect("state");

        let error = uninstall(
            temporary.path(),
            &state,
            &manager,
            "aseprite-extension-manager",
            "0.1.0",
            &manager,
        )
        .expect_err("manager uninstall");

        assert_eq!(error.code, "SELF_UNINSTALL_RESTRICTED");
        assert!(manager.is_dir());
    }

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
    fn verify_accepts_one_matching_installed_package() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let extension = temporary.path().join("extensions").join("sample");
        fs::create_dir_all(&extension).expect("mkdir");
        fs::write(
            extension.join("package.json"),
            br#"{"name":"sample","version":"1.0.0"}"#,
        )
        .expect("manifest");
        let state = State::new(temporary.path()).expect("state");

        let installed =
            verify(temporary.path(), &state, "SAMPLE", "1.0.0").expect("verified package");

        assert_eq!(installed.name, "sample");
        assert_eq!(installed.version, "1.0.0");
        assert_eq!(installed.path, extension);
    }

    #[test]
    fn verify_rejects_duplicate_package_names_case_insensitively() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let extensions = temporary.path().join("extensions");
        for (folder, name) in [("sample-one", "sample"), ("sample-two", "SAMPLE")] {
            let extension = extensions.join(folder);
            fs::create_dir_all(&extension).expect("mkdir");
            fs::write(
                extension.join("package.json"),
                format!(r#"{{"name":"{name}","version":"1.0.0"}}"#),
            )
            .expect("manifest");
        }
        let state = State::new(temporary.path()).expect("state");

        let error = verify(temporary.path(), &state, "sample", "1.0.0")
            .expect_err("duplicate package names must be ambiguous");

        assert_eq!(error.code, "INSTALL_VERIFICATION_FAILED");
        assert_eq!(
            error.message,
            "more than one installed extension uses the expected package name"
        );
        let details = error.details.expect("duplicate details");
        assert_eq!(details["expectedName"], "sample");
        assert_eq!(details["matchCount"], 2);
        assert_eq!(
            details["candidates"]
                .as_array()
                .expect("candidate array")
                .len(),
            2
        );
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

    #[test]
    fn only_the_canonical_running_manager_is_marked_as_self() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let extensions = temporary.path().join("extensions");
        let manager = extensions.join("Current Manager");
        let duplicate = extensions.join("Old Manager Copy");
        for directory in [&manager, &duplicate] {
            fs::create_dir_all(directory).expect("mkdir");
            fs::write(
                directory.join("package.json"),
                br#"{"name":"aseprite-extension-manager","version":"0.1.0"}"#,
            )
            .expect("manifest");
        }
        let state = State::new(temporary.path()).expect("state");

        let mut packages =
            scan_with_manager_root(temporary.path(), &state, &manager).expect("manager-aware scan");

        assert_eq!(packages.len(), 2);
        let current = packages
            .iter()
            .find(|package| package.path == manager)
            .expect("running manager");
        let old = packages
            .iter()
            .find(|package| package.path == duplicate)
            .expect("duplicate manager");
        assert!(current.is_self);
        assert!(!old.is_self);

        let updates = std::collections::BTreeMap::from([(
            MANAGER_NAME.to_owned(),
            serde_json::json!({"kind":"self","version":"0.2.0"}),
        )]);
        attach_updates(&mut packages, &updates);
        let current = packages
            .iter()
            .find(|package| package.is_self)
            .expect("running manager after update attachment");
        let old = packages
            .iter()
            .find(|package| package.path == duplicate)
            .expect("duplicate manager after update attachment");
        assert_eq!(
            current
                .update
                .as_ref()
                .and_then(|update| update.get("kind")),
            Some(&Value::String("self".to_owned()))
        );
        assert!(old.update.is_none());
    }
}
