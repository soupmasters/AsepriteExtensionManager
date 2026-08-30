use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Read, Seek, Write};
use std::path::{Component, Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use unicode_normalization::UnicodeNormalization;
use walkdir::WalkDir;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter};

use crate::protocol::{RpcError, RpcResult};
use crate::state::{package_id_is_safe, sha256_file, State};

pub const MAX_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_UNCOMPRESSED_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_FILE_COUNT: usize = 4_096;
pub const MAX_COMPRESSION_RATIO: u64 = 200;

const MANAGER_NAME: &str = "aseprite-extension-manager";
const MANAGER_DISPLAY_NAME: &str = "Aseprite Extension Manager";
const MANAGER_MACOS_HELPER: &str = "bin/macos/aem-helper";
const MANAGER_WINDOWS_HELPER: &str = "bin/windows/aem-helper.exe";
const MANAGER_LINUX_HELPER: &str = "bin/linux/aem-helper";
const MANAGER_REQUIRED_FILES: &[&str] = &[
    "package.json",
    "main.lua",
    MANAGER_MACOS_HELPER,
    MANAGER_WINDOWS_HELPER,
    MANAGER_LINUX_HELPER,
    "registry/root.json",
    "registry/metadata/timestamp.json",
    "registry/metadata/snapshot.json",
    "registry/metadata/targets.json",
    "registry/targets/catalog-v1.json",
];

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub main: Option<String>,
    #[serde(default)]
    pub contributes: Value,
}

#[derive(Clone, Debug, Default)]
pub struct ExpectedManifest<'a> {
    pub name: Option<&'a str>,
    pub version: Option<&'a str>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedPackage {
    pub artifact_path: PathBuf,
    pub name: String,
    pub display_name: Option<String>,
    pub version: String,
    pub sha256: String,
    pub byte_length: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}

#[derive(Debug)]
struct ArchiveInspection {
    manifest: Manifest,
    files: BTreeSet<String>,
}

#[derive(Clone, Copy)]
enum ManagerValidationKind {
    Release,
    Recovery,
}

pub fn validate_and_stage(
    state: &State,
    path: &Path,
    expected: ExpectedManifest<'_>,
) -> RpcResult<PreparedPackage> {
    let (artifact_path, sha256, byte_length) = state.stage_file(path)?;
    let manifest = validate_extension(&artifact_path, expected)?;
    verify_staged_integrity(&artifact_path, &sha256, byte_length)?;
    Ok(PreparedPackage {
        artifact_path,
        name: manifest.name,
        display_name: manifest.display_name,
        version: manifest.version,
        sha256,
        byte_length,
        content_hash: None,
    })
}

pub fn validate_extension(path: &Path, expected: ExpectedManifest<'_>) -> RpcResult<Manifest> {
    let metadata = fs::metadata(path).map_err(RpcError::io)?;
    if !metadata.is_file() {
        return Err(RpcError::invalid(
            "INVALID_ARCHIVE",
            "package path is not a regular file",
        ));
    }
    if metadata.len() > MAX_ARCHIVE_BYTES {
        return Err(RpcError::invalid(
            "ARCHIVE_TOO_LARGE",
            "compressed package exceeds the 64 MiB limit",
        ));
    }

    let input = fs::File::open(path).map_err(RpcError::io)?;
    let inspection = inspect_extension_archive(input)?;
    validate_manifest(&inspection.manifest)?;
    validate_contributions(&inspection.manifest, &inspection.files)?;
    if let Some(name) = expected.name {
        if !inspection.manifest.name.eq_ignore_ascii_case(name) {
            return Err(RpcError::invalid(
                "MANIFEST_MISMATCH",
                "package name does not match the resolved source",
            )
            .with_details(serde_json::json!({
                "expectedName": name,
                "actualName": inspection.manifest.name
            })));
        }
    }
    if let Some(version) = expected.version {
        if inspection.manifest.version != version {
            return Err(RpcError::invalid(
                "MANIFEST_MISMATCH",
                "package version does not match the resolved source",
            )
            .with_details(serde_json::json!({
                "expectedVersion": version,
                "actualVersion": inspection.manifest.version
            })));
        }
    }
    Ok(inspection.manifest)
}

pub fn verify_installed_artifact(
    artifact_path: &Path,
    installed_root: &Path,
    expected_name: &str,
    expected_version: &str,
) -> RpcResult<()> {
    validate_extension(
        artifact_path,
        ExpectedManifest {
            name: Some(expected_name),
            version: Some(expected_version),
        },
    )?;
    let root_metadata = fs::symlink_metadata(installed_root).map_err(RpcError::io)?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(RpcError::invalid(
            "INSTALL_CONTENT_MISMATCH",
            "the installed extension is not a real directory",
        ));
    }

    let input = fs::File::open(artifact_path).map_err(RpcError::io)?;
    let mut archive = ZipArchive::new(input).map_err(zip_error)?;
    let mut total = 0_u64;
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).map_err(zip_error)?;
        validate_zip_entry_type(&file)?;
        let normalized = normalize_archive_path(file.name())?;
        if file.is_dir()
            || normalized
                .rsplit('/')
                .next()
                .is_some_and(|name| name.eq_ignore_ascii_case("__info.json"))
        {
            continue;
        }
        enforce_entry_size(&file, &mut total)?;
        let destination = installed_root.join(&normalized);
        let metadata = fs::symlink_metadata(&destination).map_err(|_| {
            RpcError::invalid(
                "INSTALL_CONTENT_MISMATCH",
                format!("the installed extension is missing {normalized}"),
            )
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(RpcError::invalid(
                "INSTALL_CONTENT_MISMATCH",
                format!("the installed extension contains an unsafe {normalized}"),
            ));
        }
        ensure_real_parent_chain(installed_root, &destination)?;
        let mut expected = Vec::with_capacity(file.size().min(MAX_FILE_BYTES) as usize);
        file.read_to_end(&mut expected)
            .map_err(map_zip_read_error)?;
        let actual = fs::read(&destination).map_err(RpcError::io)?;
        if actual != expected {
            return Err(RpcError::invalid(
                "INSTALL_CONTENT_MISMATCH",
                format!("the installed file differs from the prepared package: {normalized}"),
            ));
        }
    }
    Ok(())
}

fn ensure_real_parent_chain(root: &Path, path: &Path) -> RpcResult<()> {
    let relative = path.strip_prefix(root).map_err(|_| {
        RpcError::invalid(
            "INSTALL_CONTENT_MISMATCH",
            "the installed extension path escaped its package directory",
        )
    })?;
    let mut current = root.to_path_buf();
    let parent_count = relative.components().count().saturating_sub(1);
    for component in relative.components().take(parent_count) {
        current.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&current).map_err(RpcError::io)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(RpcError::invalid(
                "INSTALL_CONTENT_MISMATCH",
                format!(
                    "the installed extension contains an unsafe path: {}",
                    current.display()
                ),
            ));
        }
    }
    Ok(())
}

pub fn validate_extension_directory(root: &Path) -> RpcResult<Manifest> {
    let files = collect_extension_directory_files(root)?;
    let names: BTreeSet<_> = files.iter().map(|(path, _)| path.clone()).collect();
    let manifest_count = names
        .iter()
        .filter(|path| path.rsplit('/').next() == Some("package.json"))
        .count();
    if manifest_count != 1 {
        return Err(RpcError::invalid(
            "INVALID_MANIFEST_COUNT",
            "package must contain exactly one package.json",
        ));
    }

    let manifest_path = files
        .iter()
        .find_map(|(path, absolute)| (path == "package.json").then_some(absolute))
        .ok_or_else(|| {
            RpcError::invalid(
                "MANIFEST_NOT_AT_ROOT",
                "package.json must be at the extension root",
            )
        })?;

    for (path, absolute) in &files {
        reject_native_name(path)?;
        let data = fs::read(absolute).map_err(RpcError::io)?;
        reject_native_magic(path, &data)?;
    }

    let manifest: Manifest =
        serde_json::from_slice(&fs::read(manifest_path).map_err(RpcError::io)?)
            .map_err(|error| RpcError::invalid("INVALID_MANIFEST", error.to_string()))?;
    validate_manifest(&manifest)?;
    validate_contributions(&manifest, &names)?;
    Ok(manifest)
}

pub fn collect_extension_directory_files(root: &Path) -> RpcResult<Vec<(String, PathBuf)>> {
    let metadata = fs::symlink_metadata(root).map_err(RpcError::io)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(RpcError::invalid(
            "INVALID_LOCAL_SOURCE",
            "extension root must be a real directory",
        ));
    }

    let ignore = build_ignore_matcher(root)?;
    collect_local_files(root, &ignore)
}

pub fn validate_manager_and_stage(
    state: &State,
    path: &Path,
    expected_version: &str,
) -> RpcResult<PreparedPackage> {
    validate_manager_and_stage_with_kind(
        state,
        path,
        expected_version,
        ManagerValidationKind::Release,
    )
}

pub(crate) fn validate_manager_recovery_and_stage(
    state: &State,
    path: &Path,
    expected_version: &str,
) -> RpcResult<PreparedPackage> {
    validate_manager_and_stage_with_kind(
        state,
        path,
        expected_version,
        ManagerValidationKind::Recovery,
    )
}

pub(crate) fn validate_manager_release_directory(
    root: &Path,
    expected_version: &str,
) -> RpcResult<Manifest> {
    validate_manager_directory_with_kind(root, expected_version, ManagerValidationKind::Release)
}

fn validate_manager_and_stage_with_kind(
    state: &State,
    path: &Path,
    expected_version: &str,
    kind: ManagerValidationKind,
) -> RpcResult<PreparedPackage> {
    let (artifact_path, sha256, byte_length) = state.stage_file(path)?;
    let manifest = validate_manager_archive_with_kind(&artifact_path, expected_version, kind)?;
    verify_staged_integrity(&artifact_path, &sha256, byte_length)?;
    Ok(PreparedPackage {
        artifact_path,
        name: manifest.name,
        display_name: manifest.display_name,
        version: manifest.version,
        sha256,
        byte_length,
        content_hash: None,
    })
}

pub fn validate_manager_directory(root: &Path, expected_version: &str) -> RpcResult<Manifest> {
    validate_manager_directory_with_kind(root, expected_version, ManagerValidationKind::Recovery)
}

fn validate_manager_directory_with_kind(
    root: &Path,
    expected_version: &str,
    kind: ManagerValidationKind,
) -> RpcResult<Manifest> {
    let metadata = fs::symlink_metadata(root).map_err(RpcError::io)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(RpcError::invalid(
            "INVALID_MANAGER_DIRECTORY",
            "manager package root must be a real directory",
        ));
    }

    let files = collect_manager_files(root)?;
    let mut names = BTreeSet::new();
    let mut manifest_bytes = None;
    for (relative, absolute) in &files {
        let data = fs::read(absolute).map_err(RpcError::io)?;
        validate_manager_file(relative, &data, kind)?;
        if relative == "package.json" {
            manifest_bytes = Some(data);
        }
        names.insert(relative.clone());
    }
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes.ok_or_else(|| {
        RpcError::invalid(
            "INVALID_MANAGER_PACKAGE",
            "manager package is missing package.json at its root",
        )
    })?)
    .map_err(|error| RpcError::invalid("INVALID_MANIFEST", error.to_string()))?;
    validate_manager_layout(&manifest, &names, expected_version)?;
    validate_manager_registry(&root.join("registry"), kind)?;
    Ok(manifest)
}

pub fn package_manager_directory(state: &State, root: &Path) -> RpcResult<PreparedPackage> {
    let manifest_path = root.join("package.json");
    let manifest: Manifest =
        serde_json::from_slice(&fs::read(&manifest_path).map_err(RpcError::io)?)
            .map_err(|error| RpcError::invalid("INVALID_MANIFEST", error.to_string()))?;
    let expected_version = manifest.version.clone();
    validate_manager_directory(root, &expected_version)?;
    let files = collect_manager_files(root)?;

    let mut archive = NamedTempFile::new_in(state.root().join("staging")).map_err(RpcError::io)?;
    {
        let mut writer = ZipWriter::new(archive.as_file_mut());
        for (relative, absolute) in &files {
            let options = SimpleFileOptions::default()
                .compression_method(CompressionMethod::Deflated)
                .last_modified_time(DateTime::default())
                .unix_permissions(manager_file_mode(relative));
            writer.start_file(relative, options).map_err(zip_error)?;
            let mut input = fs::File::open(absolute).map_err(RpcError::io)?;
            io::copy(&mut input, &mut writer).map_err(RpcError::io)?;
        }
        writer.finish().map_err(zip_error)?;
    }
    archive.as_file_mut().sync_all().map_err(RpcError::io)?;
    if archive.as_file().metadata().map_err(RpcError::io)?.len() > MAX_ARCHIVE_BYTES {
        return Err(RpcError::invalid(
            "ARCHIVE_TOO_LARGE",
            "manager package exceeds the 64 MiB compressed limit",
        ));
    }
    validate_manager_recovery_and_stage(state, archive.path(), &expected_version)
}

pub fn package_local_folder(state: &State, package_json_path: &Path) -> RpcResult<PreparedPackage> {
    if package_json_path.file_name().and_then(|name| name.to_str()) != Some("package.json") {
        return Err(RpcError::invalid(
            "INVALID_LOCAL_SOURCE",
            "select the local folder's package.json",
        ));
    }
    let source_root = package_json_path
        .parent()
        .ok_or_else(|| RpcError::invalid("INVALID_LOCAL_SOURCE", "package path has no folder"))?;
    let source_root = fs::canonicalize(source_root).map_err(RpcError::io)?;
    let package_json_path = source_root.join("package.json");
    let manifest: Manifest =
        serde_json::from_slice(&fs::read(&package_json_path).map_err(RpcError::io)?)
            .map_err(|error| RpcError::invalid("INVALID_MANIFEST", error.to_string()))?;
    validate_manifest(&manifest)?;

    let ignore = build_ignore_matcher(&source_root)?;
    let files = collect_local_files(&source_root, &ignore)?;
    if !files.iter().any(|(path, _)| path == "package.json") {
        return Err(RpcError::invalid(
            "INVALID_LOCAL_SOURCE",
            "package.json is excluded from the snapshot",
        ));
    }

    let mut content_hasher = Sha256::new();
    let mut archive = NamedTempFile::new_in(state.root().join("staging")).map_err(RpcError::io)?;
    {
        let mut writer = ZipWriter::new(archive.as_file_mut());
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .last_modified_time(DateTime::default())
            .unix_permissions(0o644);
        for (relative, absolute) in &files {
            let metadata = fs::symlink_metadata(absolute).map_err(RpcError::io)?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(RpcError::invalid(
                    "UNSUPPORTED_FILE_TYPE",
                    format!("{} is not a regular file", absolute.display()),
                ));
            }
            if metadata.len() > MAX_FILE_BYTES {
                return Err(RpcError::invalid(
                    "FILE_TOO_LARGE",
                    format!("{relative} exceeds the 64 MiB per-file limit"),
                ));
            }
            reject_native_name(relative)?;
            let data = fs::read(absolute).map_err(RpcError::io)?;
            reject_native_magic(relative, &data)?;
            content_hasher.update((relative.len() as u64).to_le_bytes());
            content_hasher.update(relative.as_bytes());
            content_hasher.update((data.len() as u64).to_le_bytes());
            content_hasher.update(&data);
            writer.start_file(relative, options).map_err(zip_error)?;
            writer.write_all(&data).map_err(RpcError::io)?;
        }
        writer.finish().map_err(zip_error)?;
    }
    archive.as_file_mut().sync_all().map_err(RpcError::io)?;
    if archive.as_file().metadata().map_err(RpcError::io)?.len() > MAX_ARCHIVE_BYTES {
        return Err(RpcError::invalid(
            "ARCHIVE_TOO_LARGE",
            "snapshot exceeds the 64 MiB compressed limit",
        ));
    }
    let content_hash = hex::encode(content_hasher.finalize());
    let inspection = validate_extension(
        archive.path(),
        ExpectedManifest {
            name: Some(&manifest.name),
            version: Some(&manifest.version),
        },
    )?;
    let (artifact_path, sha256, byte_length) = state.stage_file(archive.path())?;
    Ok(PreparedPackage {
        artifact_path,
        name: inspection.name,
        display_name: inspection.display_name,
        version: inspection.version,
        sha256,
        byte_length,
        content_hash: Some(content_hash),
    })
}

pub fn package_repository_archive(
    state: &State,
    archive_path: &Path,
) -> RpcResult<PreparedPackage> {
    if fs::metadata(archive_path).map_err(RpcError::io)?.len() > MAX_ARCHIVE_BYTES {
        return Err(RpcError::invalid(
            "ARCHIVE_TOO_LARGE",
            "repository archive exceeds the compressed size limit",
        ));
    }
    let input = fs::File::open(archive_path).map_err(RpcError::io)?;
    let mut archive = ZipArchive::new(input).map_err(zip_error)?;
    if archive.len() > MAX_FILE_COUNT {
        return Err(RpcError::invalid(
            "TOO_MANY_FILES",
            "repository archive contains too many entries",
        ));
    }
    let prefix = repository_prefix(&mut archive)?;
    let temporary = tempfile::tempdir().map_err(RpcError::io)?;
    let mut seen = BTreeSet::new();
    let mut manifest_count = 0_usize;
    let mut total = 0_u64;
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).map_err(zip_error)?;
        validate_zip_entry_type(&file)?;
        let raw = file.name().to_owned();
        let normalized = normalize_archive_path(&raw)?;
        let relative = normalized
            .strip_prefix(&format!("{prefix}/"))
            .unwrap_or("")
            .to_owned();
        if relative.is_empty() || file.is_dir() {
            continue;
        }
        let collision_key = collision_key(&relative);
        if !seen.insert(collision_key) {
            return Err(RpcError::invalid(
                "PATH_COLLISION",
                format!("repository contains a duplicate or case-colliding path: {relative}"),
            ));
        }
        if relative == "package.json" || relative.ends_with("/package.json") {
            manifest_count += 1;
        }
        enforce_entry_size(&file, &mut total)?;
        reject_native_name(&relative)?;
        let destination = temporary.path().join(&relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(RpcError::io)?;
        }
        let mut bytes = Vec::with_capacity(file.size().min(MAX_FILE_BYTES) as usize);
        file.read_to_end(&mut bytes).map_err(map_zip_read_error)?;
        reject_native_magic(&relative, &bytes)?;
        fs::write(destination, bytes).map_err(RpcError::io)?;
    }
    if manifest_count != 1 || !temporary.path().join("package.json").is_file() {
        return Err(RpcError::invalid(
            "AMBIGUOUS_REPOSITORY",
            "repository snapshot must contain exactly one package.json at its root",
        ));
    }
    package_local_folder(state, &temporary.path().join("package.json"))
}

fn inspect_extension_archive<R: Read + Seek>(reader: R) -> RpcResult<ArchiveInspection> {
    let mut archive = ZipArchive::new(reader).map_err(zip_error)?;
    if archive.len() > MAX_FILE_COUNT {
        return Err(RpcError::invalid(
            "TOO_MANY_FILES",
            "package contains more than 4096 entries",
        ));
    }
    let mut seen = BTreeSet::new();
    let mut files = BTreeSet::new();
    let mut manifest_bytes = None;
    let mut manifest_count = 0_usize;
    let mut total = 0_u64;
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).map_err(zip_error)?;
        validate_zip_entry_type(&file)?;
        let normalized = normalize_archive_path(file.name())?;
        let key = collision_key(&normalized);
        if !seen.insert(key) {
            return Err(RpcError::invalid(
                "PATH_COLLISION",
                format!("duplicate or case-colliding archive path: {normalized}"),
            ));
        }
        if file.is_dir() {
            continue;
        }
        enforce_entry_size(&file, &mut total)?;
        reject_native_name(&normalized)?;
        let mut data = Vec::with_capacity(file.size().min(MAX_FILE_BYTES) as usize);
        file.read_to_end(&mut data).map_err(map_zip_read_error)?;
        reject_native_magic(&normalized, &data)?;
        if normalized == "package.json" {
            manifest_count += 1;
            manifest_bytes = Some(data);
        } else if normalized.ends_with("/package.json") {
            manifest_count += 1;
        }
        files.insert(normalized);
    }
    if manifest_count != 1 {
        return Err(RpcError::invalid(
            "INVALID_MANIFEST_COUNT",
            "package must contain exactly one package.json",
        ));
    }
    let bytes = manifest_bytes.ok_or_else(|| {
        RpcError::invalid(
            "MANIFEST_NOT_AT_ROOT",
            "package.json must be at the archive root",
        )
    })?;
    let manifest = serde_json::from_slice(&bytes)
        .map_err(|error| RpcError::invalid("INVALID_MANIFEST", error.to_string()))?;
    Ok(ArchiveInspection { manifest, files })
}

fn repository_prefix<R: Read + Seek>(archive: &mut ZipArchive<R>) -> RpcResult<String> {
    let mut prefixes = BTreeSet::new();
    for index in 0..archive.len() {
        let file = archive.by_index(index).map_err(zip_error)?;
        let normalized = normalize_archive_path(file.name())?;
        if let Some(first) = normalized.split('/').next() {
            if !first.is_empty() {
                prefixes.insert(first.to_owned());
            }
        }
    }
    if prefixes.len() != 1 {
        return Err(RpcError::invalid(
            "INVALID_REPOSITORY_ARCHIVE",
            "repository archive must have one top-level directory",
        ));
    }
    Ok(prefixes.into_iter().next().expect("one prefix"))
}

fn validate_zip_entry_type(file: &zip::read::ZipFile<'_>) -> RpcResult<()> {
    if file.encrypted() {
        return Err(RpcError::invalid(
            "ENCRYPTED_ARCHIVE",
            format!("archive entry is encrypted: {}", file.name()),
        ));
    }
    if let Some(mode) = file.unix_mode() {
        match mode & 0o170000 {
            0 | 0o040000 | 0o100000 => {}
            0o120000 => {
                return Err(RpcError::invalid(
                    "UNSUPPORTED_FILE_TYPE",
                    format!("archive contains a symbolic link: {}", file.name()),
                ));
            }
            _ => {
                return Err(RpcError::invalid(
                    "UNSUPPORTED_FILE_TYPE",
                    format!("archive contains a device or special file: {}", file.name()),
                ));
            }
        }
    }
    Ok(())
}

fn enforce_entry_size(file: &zip::read::ZipFile<'_>, total: &mut u64) -> RpcResult<()> {
    if file.size() > MAX_FILE_BYTES {
        return Err(RpcError::invalid(
            "FILE_TOO_LARGE",
            format!("{} exceeds the per-file size limit", file.name()),
        ));
    }
    *total = total.saturating_add(file.size());
    if *total > MAX_UNCOMPRESSED_BYTES {
        return Err(RpcError::invalid(
            "ARCHIVE_BOMB",
            "package exceeds the total uncompressed size limit",
        ));
    }
    let compressed = file.compressed_size();
    if file.size() > 1024 * 1024
        && (compressed == 0 || file.size() / compressed.max(1) > MAX_COMPRESSION_RATIO)
    {
        return Err(RpcError::invalid(
            "ARCHIVE_BOMB",
            format!("{} has an unsafe compression ratio", file.name()),
        ));
    }
    Ok(())
}

fn normalize_archive_path(raw: &str) -> RpcResult<String> {
    if raw.is_empty()
        || raw.len() > 512
        || raw.contains('\0')
        || raw.contains('\\')
        || raw.starts_with('/')
        || raw.starts_with("//")
        || raw.as_bytes().get(1) == Some(&b':')
    {
        return Err(RpcError::invalid(
            "UNSAFE_ARCHIVE_PATH",
            format!("unsafe archive path: {raw:?}"),
        ));
    }
    let mut trimmed = raw.strip_suffix('/').unwrap_or(raw);
    while let Some(without_prefix) = trimmed.strip_prefix("./") {
        trimmed = without_prefix;
    }
    if trimmed.is_empty() {
        return Err(RpcError::invalid(
            "UNSAFE_ARCHIVE_PATH",
            "archive contains an empty path",
        ));
    }
    for component in Path::new(trimmed).components() {
        match component {
            Component::Normal(value) if !value.is_empty() => {}
            _ => {
                return Err(RpcError::invalid(
                    "UNSAFE_ARCHIVE_PATH",
                    format!("unsafe archive path: {raw:?}"),
                ));
            }
        }
    }
    if trimmed
        .split('/')
        .any(|part| part == "." || part == ".." || part.is_empty())
    {
        return Err(RpcError::invalid(
            "UNSAFE_ARCHIVE_PATH",
            format!("unsafe archive path: {raw:?}"),
        ));
    }
    for component in trimmed.split('/') {
        validate_portable_component(component, raw)?;
    }
    Ok(trimmed.nfc().collect())
}

fn validate_portable_component(component: &str, raw: &str) -> RpcResult<()> {
    if component.ends_with(['.', ' '])
        || component
            .chars()
            .any(|character| character < ' ' || ":<>\"|?*".contains(character))
    {
        return Err(RpcError::invalid(
            "UNSAFE_ARCHIVE_PATH",
            format!("archive path is not portable across supported platforms: {raw:?}"),
        ));
    }

    let stem = component
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let reserved = matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$"
    ) || stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"))
        .is_some_and(|suffix| suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9'));
    if reserved {
        return Err(RpcError::invalid(
            "UNSAFE_ARCHIVE_PATH",
            format!("archive path uses a reserved Windows device name: {raw:?}"),
        ));
    }
    Ok(())
}

fn collision_key(path: &str) -> String {
    path.nfkc().flat_map(char::to_lowercase).collect()
}

fn validate_manifest(manifest: &Manifest) -> RpcResult<()> {
    if !package_id_is_safe(&manifest.name) {
        return Err(RpcError::invalid(
            "INVALID_MANIFEST",
            "manifest name is missing or invalid",
        ));
    }
    if manifest.version.trim().is_empty() || manifest.version.len() > 128 {
        return Err(RpcError::invalid(
            "INVALID_MANIFEST",
            "manifest version is missing or invalid",
        ));
    }
    Ok(())
}

fn validate_contributions(manifest: &Manifest, files: &BTreeSet<String>) -> RpcResult<()> {
    let mut paths = Vec::new();
    if let Some(main) = &manifest.main {
        paths.push(main.clone());
    }
    collect_contribution_paths(&manifest.contributes, &mut paths);
    for path in paths {
        if matches!(path.as_str(), "." | "./") {
            continue;
        }
        let normalized = normalize_archive_path(&path)?;
        let present = files.contains(&normalized)
            || files
                .iter()
                .any(|candidate| candidate.starts_with(&format!("{normalized}/")));
        if !present {
            return Err(RpcError::invalid(
                "MISSING_CONTRIBUTION",
                format!("manifest contribution is missing from the package: {normalized}"),
            ));
        }
    }
    Ok(())
}

fn collect_contribution_paths(value: &Value, paths: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if (key == "path" || key == "script") && value.is_string() {
                    paths.push(value.as_str().unwrap_or_default().to_owned());
                } else {
                    collect_contribution_paths(value, paths);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_contribution_paths(value, paths);
            }
        }
        _ => {}
    }
}

pub fn validate_manager_archive(path: &Path, expected_version: &str) -> RpcResult<Manifest> {
    validate_manager_archive_with_kind(path, expected_version, ManagerValidationKind::Release)
}

fn validate_manager_archive_with_kind(
    path: &Path,
    expected_version: &str,
    kind: ManagerValidationKind,
) -> RpcResult<Manifest> {
    let metadata = fs::symlink_metadata(path).map_err(RpcError::io)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(RpcError::invalid(
            "INVALID_ARCHIVE",
            "manager package path is not a regular file",
        ));
    }
    if metadata.len() > MAX_ARCHIVE_BYTES {
        return Err(RpcError::invalid(
            "ARCHIVE_TOO_LARGE",
            "manager package exceeds the 64 MiB compressed limit",
        ));
    }

    let input = fs::File::open(path).map_err(RpcError::io)?;
    let mut archive = ZipArchive::new(input).map_err(zip_error)?;
    if archive.len() > MAX_FILE_COUNT {
        return Err(RpcError::invalid(
            "TOO_MANY_FILES",
            "manager package contains more than 4096 entries",
        ));
    }

    let mut seen = BTreeSet::new();
    let mut files = BTreeSet::new();
    let mut manifest_bytes = None;
    let mut manifest_count = 0_usize;
    let mut total = 0_u64;
    let registry_directory = tempfile::tempdir().map_err(RpcError::io)?;
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).map_err(zip_error)?;
        validate_zip_entry_type(&file)?;
        let normalized = normalize_archive_path(file.name())?;
        let key = collision_key(&normalized);
        if !seen.insert(key) {
            return Err(RpcError::invalid(
                "PATH_COLLISION",
                format!("duplicate or case-colliding manager path: {normalized}"),
            ));
        }
        if file.is_dir() {
            continue;
        }
        enforce_entry_size(&file, &mut total)?;
        if let Some(mode) = file.unix_mode() {
            let expected_mode = manager_file_mode(&normalized);
            if mode & 0o777 != expected_mode {
                return Err(RpcError::invalid(
                    "INVALID_MANAGER_MODE",
                    format!(
                        "{normalized} has mode {:o}; expected {expected_mode:o}",
                        mode & 0o777
                    ),
                ));
            }
        }
        let mut data = Vec::with_capacity(file.size().min(MAX_FILE_BYTES) as usize);
        file.read_to_end(&mut data).map_err(map_zip_read_error)?;
        validate_manager_file(&normalized, &data, kind)?;
        if normalized.starts_with("registry/") {
            let destination = registry_directory.path().join(&normalized);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).map_err(RpcError::io)?;
            }
            fs::write(destination, &data).map_err(RpcError::io)?;
        }
        if normalized == "package.json" {
            manifest_count += 1;
            manifest_bytes = Some(data);
        } else if normalized.ends_with("/package.json") {
            manifest_count += 1;
        }
        files.insert(normalized);
    }
    if manifest_count != 1 {
        return Err(RpcError::invalid(
            "INVALID_MANIFEST_COUNT",
            "manager package must contain exactly one package.json",
        ));
    }
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes.ok_or_else(|| {
        RpcError::invalid(
            "MANIFEST_NOT_AT_ROOT",
            "manager package.json must be at the archive root",
        )
    })?)
    .map_err(|error| RpcError::invalid("INVALID_MANIFEST", error.to_string()))?;
    validate_manager_layout(&manifest, &files, expected_version)?;
    validate_manager_registry(&registry_directory.path().join("registry"), kind)?;
    Ok(manifest)
}

fn validate_manager_registry(repository: &Path, kind: ManagerValidationKind) -> RpcResult<()> {
    match kind {
        ManagerValidationKind::Release => crate::registry::validate_bundled_repository(repository),
        ManagerValidationKind::Recovery => {
            crate::registry::validate_bundled_repository_allow_expired(repository)
        }
    }
}

fn validate_manager_layout(
    manifest: &Manifest,
    files: &BTreeSet<String>,
    expected_version: &str,
) -> RpcResult<()> {
    if manifest.name != MANAGER_NAME
        || manifest.display_name.as_deref() != Some(MANAGER_DISPLAY_NAME)
    {
        return Err(RpcError::invalid(
            "INVALID_MANAGER_MANIFEST",
            "manager manifest identity is invalid",
        ));
    }
    let version = semver::Version::parse(&manifest.version).map_err(|_| {
        RpcError::invalid(
            "INVALID_MANAGER_MANIFEST",
            "manager version must be stable semantic versioning",
        )
    })?;
    if !version.pre.is_empty()
        || !version.build.is_empty()
        || version.to_string() != manifest.version
    {
        return Err(RpcError::invalid(
            "INVALID_MANAGER_MANIFEST",
            "manager version must be stable semantic versioning",
        ));
    }
    if manifest.version != expected_version {
        return Err(RpcError::invalid(
            "MANIFEST_MISMATCH",
            "manager package version does not match the expected release",
        )
        .with_details(serde_json::json!({
            "expectedVersion": expected_version,
            "actualVersion": manifest.version
        })));
    }

    for required in MANAGER_REQUIRED_FILES {
        if !files.contains(*required) {
            return Err(RpcError::invalid(
                "INVALID_MANAGER_PACKAGE",
                format!("manager package is missing required file: {required}"),
            ));
        }
    }
    if files
        .iter()
        .filter(|path| path.rsplit('/').next() == Some("package.json"))
        .count()
        != 1
    {
        return Err(RpcError::invalid(
            "INVALID_MANIFEST_COUNT",
            "manager package must contain exactly one package.json",
        ));
    }
    if !files.iter().any(|path| is_hash_prefixed_catalog(path)) {
        return Err(RpcError::invalid(
            "INVALID_MANAGER_PACKAGE",
            "manager package is missing its hash-prefixed catalog target",
        ));
    }
    if !files
        .iter()
        .any(|path| is_versioned_metadata(path, "snapshot"))
    {
        return Err(RpcError::invalid(
            "INVALID_MANAGER_PACKAGE",
            "manager package is missing versioned snapshot metadata",
        ));
    }
    if !files
        .iter()
        .any(|path| is_versioned_metadata(path, "targets"))
    {
        return Err(RpcError::invalid(
            "INVALID_MANAGER_PACKAGE",
            "manager package is missing versioned targets metadata",
        ));
    }

    let mut contributed = Vec::new();
    collect_contribution_paths(&manifest.contributes, &mut contributed);
    if contributed.is_empty() {
        return Err(RpcError::invalid(
            "MISSING_CONTRIBUTION",
            "manager manifest must contribute main.lua",
        ));
    }
    let contributes_main = contributed.iter().try_fold(false, |found, path| {
        normalize_archive_path(path).map(|normalized| found || normalized == "main.lua")
    })?;
    if !contributes_main {
        return Err(RpcError::invalid(
            "MISSING_CONTRIBUTION",
            "manager manifest must contribute main.lua",
        ));
    }
    validate_contributions(manifest, files)
}

fn validate_manager_file(path: &str, data: &[u8], kind: ManagerValidationKind) -> RpcResult<()> {
    if contains_private_registry_material(path) {
        return Err(RpcError::invalid(
            "PRIVATE_REGISTRY_MATERIAL",
            format!("manager package contains private registry material: {path}"),
        ));
    }
    match path {
        MANAGER_MACOS_HELPER => {
            validate_manager_helper_magic(path, data, ManagerHelper::Macos, kind)
        }
        MANAGER_WINDOWS_HELPER => {
            validate_manager_helper_magic(path, data, ManagerHelper::Windows, kind)
        }
        MANAGER_LINUX_HELPER => {
            validate_manager_helper_magic(path, data, ManagerHelper::Linux, kind)
        }
        _ => {
            reject_native_name(path)?;
            reject_native_magic(path, data)
        }
    }
}

#[derive(Clone, Copy)]
enum ManagerHelper {
    Macos,
    Windows,
    Linux,
}

fn validate_manager_helper_magic(
    path: &str,
    data: &[u8],
    helper: ManagerHelper,
    kind: ManagerValidationKind,
) -> RpcResult<()> {
    let valid = match helper {
        ManagerHelper::Macos => {
            let universal = is_universal_macos_helper(data);
            universal
                || matches!(kind, ManagerValidationKind::Recovery)
                    && is_supported_thin_macos_helper(data)
        }
        ManagerHelper::Windows => data.starts_with(b"MZ"),
        ManagerHelper::Linux => data.starts_with(b"\x7fELF"),
    };
    if !valid {
        return Err(RpcError::invalid(
            "INVALID_MANAGER_HELPER",
            format!("manager helper has an unexpected executable format: {path}"),
        ));
    }
    Ok(())
}

fn is_supported_thin_macos_helper(data: &[u8]) -> bool {
    thin_macos_cpu(data).is_some()
}

fn thin_macos_cpu(data: &[u8]) -> Option<u32> {
    const CPU_TYPE_X86_64: u32 = 0x0100_0007;
    const CPU_TYPE_ARM64: u32 = 0x0100_000c;
    if data.get(..4)? != [0xcf, 0xfa, 0xed, 0xfe] || data.len() < 32 {
        return None;
    }
    let cpu = u32::from_le_bytes(data.get(4..8)?.try_into().ok()?);
    matches!(cpu, CPU_TYPE_X86_64 | CPU_TYPE_ARM64).then_some(cpu)
}

fn is_universal_macos_helper(data: &[u8]) -> bool {
    const CPU_TYPE_X86_64: u32 = 0x0100_0007;
    const CPU_TYPE_ARM64: u32 = 0x0100_000c;

    let entry_size = match data.get(..4) {
        Some([0xca, 0xfe, 0xba, 0xbe]) => 20_usize,
        Some([0xca, 0xfe, 0xba, 0xbf]) => 32_usize,
        _ => return false,
    };
    let Some(architecture_count) = read_be_u32(data, 4).map(|value| value as usize) else {
        return false;
    };
    if architecture_count != 2 {
        return false;
    }
    let Some(header_length) = entry_size
        .checked_mul(architecture_count)
        .and_then(|entries| 8_usize.checked_add(entries))
    else {
        return false;
    };
    if header_length > data.len() {
        return false;
    }

    let mut architectures = BTreeSet::new();
    let mut ranges = Vec::with_capacity(architecture_count);
    for index in 0..architecture_count {
        let entry = 8 + index * entry_size;
        let Some(cpu) = read_be_u32(data, entry) else {
            return false;
        };
        if !matches!(cpu, CPU_TYPE_X86_64 | CPU_TYPE_ARM64) || !architectures.insert(cpu) {
            return false;
        }
        let (offset, size) = if entry_size == 20 {
            (
                read_be_u32(data, entry + 8).map(u64::from),
                read_be_u32(data, entry + 12).map(u64::from),
            )
        } else {
            (read_be_u64(data, entry + 8), read_be_u64(data, entry + 16))
        };
        let (Some(offset), Some(size)) = (offset, size) else {
            return false;
        };
        let Some(end) = offset.checked_add(size) else {
            return false;
        };
        let (Ok(start), Ok(end)) = (usize::try_from(offset), usize::try_from(end)) else {
            return false;
        };
        if size == 0 || start < header_length || end > data.len() {
            return false;
        }
        if thin_macos_cpu(&data[start..end]) != Some(cpu) {
            return false;
        }
        ranges.push((start, end));
    }
    ranges.sort_unstable();
    ranges.windows(2).all(|pair| pair[0].1 <= pair[1].0)
        && architectures.contains(&CPU_TYPE_X86_64)
        && architectures.contains(&CPU_TYPE_ARM64)
}

fn read_be_u32(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        data.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn read_be_u64(data: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_be_bytes(
        data.get(offset..offset.checked_add(8)?)?.try_into().ok()?,
    ))
}

fn manager_file_mode(path: &str) -> u32 {
    if matches!(path, MANAGER_MACOS_HELPER | MANAGER_LINUX_HELPER) {
        0o755
    } else {
        0o644
    }
}

fn is_hash_prefixed_catalog(path: &str) -> bool {
    let Some(filename) = path.strip_prefix("registry/targets/") else {
        return false;
    };
    let Some(hash) = filename.strip_suffix(".catalog-v1.json") else {
        return false;
    };
    hash.len() == 64
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_versioned_metadata(path: &str, role: &str) -> bool {
    let Some(filename) = path.strip_prefix("registry/metadata/") else {
        return false;
    };
    let Some(version) = filename.strip_suffix(&format!(".{role}.json")) else {
        return false;
    };
    version.parse::<u64>().is_ok_and(|value| value > 0)
}

fn contains_private_registry_material(path: &str) -> bool {
    if !path
        .split('/')
        .next()
        .is_some_and(|component| component.eq_ignore_ascii_case("registry"))
    {
        return false;
    }
    path.split('/').any(|component| {
        let folded = component.to_ascii_lowercase();
        folded == "keys"
            || folded == "fixtures"
            || folded.contains("private")
            || folded.contains("seed")
    })
}

fn collect_manager_files(root: &Path) -> RpcResult<Vec<(String, PathBuf)>> {
    let mut files = BTreeMap::new();
    let mut total = 0_u64;
    let mut walker = WalkDir::new(root).follow_links(false).into_iter();
    while let Some(entry) = walker.next() {
        let entry = entry.map_err(|error| RpcError::io(io::Error::other(error)))?;
        if entry.path() == root {
            continue;
        }
        let relative_path = entry.path().strip_prefix(root).map_err(|error| {
            RpcError::internal(format!("could not make manager path relative: {error}"))
        })?;
        let relative = path_to_archive_name(relative_path)?;
        let file_type = entry.file_type();
        if is_aseprite_metadata(&relative) {
            if file_type.is_dir() {
                walker.skip_current_dir();
            }
            continue;
        }
        if file_type.is_symlink() {
            return Err(RpcError::invalid(
                "UNSUPPORTED_FILE_TYPE",
                format!("manager directory contains a symbolic link: {relative}"),
            ));
        }
        if file_type.is_dir() {
            continue;
        }
        if !file_type.is_file() {
            return Err(RpcError::invalid(
                "UNSUPPORTED_FILE_TYPE",
                format!("manager directory contains a special file: {relative}"),
            ));
        }
        if files.len() >= MAX_FILE_COUNT {
            return Err(RpcError::invalid(
                "TOO_MANY_FILES",
                "manager directory contains more than 4096 files",
            ));
        }
        let size = entry
            .metadata()
            .map_err(|error| RpcError::io(io::Error::other(error)))?
            .len();
        total = total.saturating_add(size);
        if size > MAX_FILE_BYTES || total > MAX_UNCOMPRESSED_BYTES {
            return Err(RpcError::invalid(
                "LOCAL_SOURCE_TOO_LARGE",
                "manager directory exceeds package size limits",
            ));
        }
        let key = collision_key(&relative);
        if files
            .insert(key, (relative.clone(), entry.path().to_owned()))
            .is_some()
        {
            return Err(RpcError::invalid(
                "PATH_COLLISION",
                format!("manager directory has a case-colliding path: {relative}"),
            ));
        }
    }
    let mut files: Vec<_> = files.into_values().collect();
    files.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(files)
}

fn is_aseprite_metadata(path: &str) -> bool {
    path.split('/')
        .any(|component| matches!(component, "__info.json" | "__pref.lua"))
}

fn collect_local_files(root: &Path, ignore: &Gitignore) -> RpcResult<Vec<(String, PathBuf)>> {
    let mut files = BTreeMap::new();
    let mut total = 0_u64;
    let mut walker = WalkDir::new(root).follow_links(false).into_iter();
    while let Some(entry) = walker.next() {
        let entry = entry.map_err(|error| RpcError::io(io::Error::other(error)))?;
        if entry.path() == root {
            continue;
        }
        let relative_path = entry.path().strip_prefix(root).map_err(|error| {
            RpcError::internal(format!("could not make local path relative: {error}"))
        })?;
        let relative = path_to_archive_name(relative_path)?;
        let file_type = entry.file_type();
        let excluded = default_excluded(&relative)
            || ignore
                .matched_path_or_any_parents(relative_path, file_type.is_dir())
                .is_ignore();
        if excluded {
            if file_type.is_dir() {
                walker.skip_current_dir();
            }
            continue;
        }
        if file_type.is_symlink() {
            return Err(RpcError::invalid(
                "UNSUPPORTED_FILE_TYPE",
                format!("local folder contains a symbolic link: {relative}"),
            ));
        }
        if file_type.is_dir() {
            continue;
        }
        if !file_type.is_file() {
            return Err(RpcError::invalid(
                "UNSUPPORTED_FILE_TYPE",
                format!("local folder contains a special file: {relative}"),
            ));
        }
        if files.len() >= MAX_FILE_COUNT {
            return Err(RpcError::invalid(
                "TOO_MANY_FILES",
                "local folder contains more than 4096 files",
            ));
        }
        let size = entry
            .metadata()
            .map_err(|error| RpcError::io(io::Error::other(error)))?
            .len();
        total = total.saturating_add(size);
        if size > MAX_FILE_BYTES || total > MAX_UNCOMPRESSED_BYTES {
            return Err(RpcError::invalid(
                "LOCAL_SOURCE_TOO_LARGE",
                "local folder exceeds package size limits",
            ));
        }
        let key = collision_key(&relative);
        if files
            .insert(key, (relative.clone(), entry.path().to_owned()))
            .is_some()
        {
            return Err(RpcError::invalid(
                "PATH_COLLISION",
                format!("local folder has a case-colliding path: {relative}"),
            ));
        }
    }
    let mut files: Vec<_> = files.into_values().collect();
    files.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(files)
}

fn build_ignore_matcher(root: &Path) -> RpcResult<Gitignore> {
    let mut builder = GitignoreBuilder::new(root);
    let path = root.join(".aemignore");
    if path.exists() {
        for (line_number, line) in fs::read_to_string(&path)
            .map_err(RpcError::io)?
            .lines()
            .enumerate()
        {
            builder
                .add_line(Some(path.clone()), line)
                .map_err(|error| {
                    RpcError::invalid(
                        "INVALID_AEMIGNORE",
                        format!("line {}: {error}", line_number + 1),
                    )
                })?;
        }
    }
    builder
        .build()
        .map_err(|error| RpcError::invalid("INVALID_AEMIGNORE", error.to_string()))
}

fn default_excluded(relative: &str) -> bool {
    relative.split('/').any(|component| {
        component == ".git"
            || component == ".DS_Store"
            || component.eq_ignore_ascii_case("Thumbs.db")
            || component.eq_ignore_ascii_case("desktop.ini")
            || component == "__info.json"
            || component == "__pref.lua"
            || component == ".aemignore"
            || component.starts_with(".aem-")
    })
}

fn path_to_archive_name(path: &Path) -> RpcResult<String> {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => {
                let value = value.to_str().ok_or_else(|| {
                    RpcError::invalid("UNSUPPORTED_PATH", "local path is not valid Unicode")
                })?;
                components.push(value.nfc().collect::<String>());
            }
            _ => {
                return Err(RpcError::invalid(
                    "UNSUPPORTED_PATH",
                    "local source contains an unsafe path",
                ));
            }
        }
    }
    normalize_archive_path(&components.join("/"))
}

fn reject_native_name(path: &str) -> RpcResult<()> {
    let lower = path.to_ascii_lowercase();
    let extension = Path::new(&lower)
        .extension()
        .and_then(|value| value.to_str());
    if matches!(
        extension,
        Some("exe" | "dll" | "so" | "dylib" | "node" | "com" | "a" | "lib" | "o" | "obj")
    ) {
        return Err(RpcError::invalid(
            "NATIVE_CODE_UNSUPPORTED",
            format!("third-party native executable content is unsupported: {path}"),
        ));
    }
    Ok(())
}

fn reject_native_magic(path: &str, data: &[u8]) -> RpcResult<()> {
    let is_elf = data.starts_with(b"\x7fELF");
    let is_pe = data.starts_with(b"MZ");
    let is_static_archive = data.starts_with(b"!<arch>\n");
    let is_coff_object = data.get(..20).is_some_and(|header| {
        let machine = u16::from_le_bytes([header[0], header[1]]);
        let section_count = u16::from_le_bytes([header[2], header[3]]);
        let optional_header_size = u16::from_le_bytes([header[16], header[17]]);
        matches!(
            machine,
            0x014c // x86
                | 0x01c0 // ARM
                | 0x01c4 // ARM Thumb-2
                | 0x0200 // Itanium
                | 0x8664 // x86-64
                | 0xaa64 // ARM64
        ) && section_count > 0
            && optional_header_size == 0
    });
    let is_mach = matches!(
        data.get(..4),
        Some([0xfe, 0xed, 0xfa, 0xce])
            | Some([0xce, 0xfa, 0xed, 0xfe])
            | Some([0xfe, 0xed, 0xfa, 0xcf])
            | Some([0xcf, 0xfa, 0xed, 0xfe])
            | Some([0xca, 0xfe, 0xba, 0xbe])
            | Some([0xca, 0xfe, 0xba, 0xbf])
            | Some([0xbe, 0xba, 0xfe, 0xca])
            | Some([0xbf, 0xba, 0xfe, 0xca])
    );
    if is_elf || is_pe || is_static_archive || is_coff_object || is_mach {
        return Err(RpcError::invalid(
            "NATIVE_CODE_UNSUPPORTED",
            format!("third-party native executable content is unsupported: {path}"),
        ));
    }
    Ok(())
}

fn zip_error(error: zip::result::ZipError) -> RpcError {
    let message = error.to_string();
    let code = if message.to_ascii_lowercase().contains("password")
        || message.to_ascii_lowercase().contains("encrypt")
    {
        "ENCRYPTED_ARCHIVE"
    } else {
        "INVALID_ARCHIVE"
    };
    RpcError::invalid(code, message)
}

fn map_zip_read_error(error: io::Error) -> RpcError {
    let message = error.to_string();
    if message.to_ascii_lowercase().contains("password")
        || message.to_ascii_lowercase().contains("encrypt")
    {
        RpcError::invalid("ENCRYPTED_ARCHIVE", message)
    } else {
        RpcError::invalid("INVALID_ARCHIVE", message)
    }
}

pub fn artifact_hash(path: &Path) -> RpcResult<(String, u64)> {
    sha256_file(path).map_err(RpcError::io)
}

fn verify_staged_integrity(
    path: &Path,
    expected_hash: &str,
    expected_length: u64,
) -> RpcResult<()> {
    let (actual_hash, actual_length) = artifact_hash(path)?;
    if actual_length != expected_length || !actual_hash.eq_ignore_ascii_case(expected_hash) {
        return Err(RpcError::invalid(
            "STAGED_ARTIFACT_CHANGED",
            "the staged package changed while it was being validated",
        )
        .with_details(serde_json::json!({
            "expectedSha256": expected_hash,
            "actualSha256": actual_hash,
            "expectedByteLength": expected_length,
            "actualByteLength": actual_length
        })));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::unstable::write::FileOptionsExt;

    fn write_package(path: &Path, entries: &[(&str, &[u8])]) {
        let file = fs::File::create(path).expect("create");
        let mut writer = ZipWriter::new(file);
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .unix_permissions(0o644);
        for (name, bytes) in entries {
            writer.start_file(*name, options).expect("start");
            writer.write_all(bytes).expect("write");
        }
        writer.finish().expect("finish");
    }

    fn write_encrypted_package(path: &Path) {
        let file = fs::File::create(path).expect("create");
        let mut writer = ZipWriter::new(file);
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .unix_permissions(0o644)
            .with_deprecated_encryption(b"fixture-password");
        writer.start_file("package.json", options).expect("start");
        writer
            .write_all(br#"{"name":"sample","version":"1.0.0"}"#)
            .expect("write");
        writer.finish().expect("finish");
    }

    fn write_manager_manifest(root: &Path, version: &str, display_name: &str, contributes: &str) {
        fs::write(
            root.join("package.json"),
            format!(
                r#"{{"name":"aseprite-extension-manager","displayName":"{display_name}","version":"{version}","contributes":{contributes}}}"#
            ),
        )
        .expect("manager manifest");
    }

    fn thin_macos_fixture(cpu: u32) -> Vec<u8> {
        let mut binary = vec![0_u8; 32];
        binary[..4].copy_from_slice(&[0xcf, 0xfa, 0xed, 0xfe]);
        binary[4..8].copy_from_slice(&cpu.to_le_bytes());
        binary
    }

    fn universal_macos_fixture() -> Vec<u8> {
        const CPU_TYPE_X86_64: u32 = 0x0100_0007;
        const CPU_TYPE_ARM64: u32 = 0x0100_000c;
        const HEADER_LENGTH: u32 = 48;
        const SLICE_LENGTH: u32 = 32;

        let mut binary = Vec::with_capacity((HEADER_LENGTH + 2 * SLICE_LENGTH) as usize);
        binary.extend_from_slice(&[0xca, 0xfe, 0xba, 0xbe]);
        binary.extend_from_slice(&2_u32.to_be_bytes());
        for (cpu, offset) in [
            (CPU_TYPE_X86_64, HEADER_LENGTH),
            (CPU_TYPE_ARM64, HEADER_LENGTH + SLICE_LENGTH),
        ] {
            binary.extend_from_slice(&cpu.to_be_bytes());
            binary.extend_from_slice(&0_u32.to_be_bytes());
            binary.extend_from_slice(&offset.to_be_bytes());
            binary.extend_from_slice(&SLICE_LENGTH.to_be_bytes());
            binary.extend_from_slice(&0_u32.to_be_bytes());
        }
        binary.extend_from_slice(&thin_macos_fixture(CPU_TYPE_X86_64));
        binary.extend_from_slice(&thin_macos_fixture(CPU_TYPE_ARM64));
        binary
    }

    fn universal_macos64_fixture() -> Vec<u8> {
        const CPU_TYPE_X86_64: u32 = 0x0100_0007;
        const CPU_TYPE_ARM64: u32 = 0x0100_000c;
        const HEADER_LENGTH: u64 = 72;
        const SLICE_LENGTH: u64 = 32;

        let mut binary = Vec::with_capacity((HEADER_LENGTH + 2 * SLICE_LENGTH) as usize);
        binary.extend_from_slice(&[0xca, 0xfe, 0xba, 0xbf]);
        binary.extend_from_slice(&2_u32.to_be_bytes());
        for (cpu, offset) in [
            (CPU_TYPE_X86_64, HEADER_LENGTH),
            (CPU_TYPE_ARM64, HEADER_LENGTH + SLICE_LENGTH),
        ] {
            binary.extend_from_slice(&cpu.to_be_bytes());
            binary.extend_from_slice(&0_u32.to_be_bytes());
            binary.extend_from_slice(&offset.to_be_bytes());
            binary.extend_from_slice(&SLICE_LENGTH.to_be_bytes());
            binary.extend_from_slice(&0_u32.to_be_bytes());
            binary.extend_from_slice(&0_u32.to_be_bytes());
        }
        binary.extend_from_slice(&thin_macos_fixture(CPU_TYPE_X86_64));
        binary.extend_from_slice(&thin_macos_fixture(CPU_TYPE_ARM64));
        binary
    }

    fn copy_test_registry(destination: &Path) {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("registry")
            .join("bundled");
        for entry in WalkDir::new(&source) {
            let entry = entry.expect("registry entry");
            let relative = entry.path().strip_prefix(&source).expect("relative path");
            let target = destination.join(relative);
            if entry.file_type().is_dir() {
                fs::create_dir_all(&target).expect("registry directory");
            } else {
                fs::copy(entry.path(), target).expect("registry file");
            }
        }
    }

    fn write_manager_fixture(root: &Path, version: &str) {
        for directory in ["bin/macos", "bin/windows", "bin/linux"] {
            fs::create_dir_all(root.join(directory)).expect("manager directory");
        }
        write_manager_manifest(
            root,
            version,
            MANAGER_DISPLAY_NAME,
            r#"{"scripts":[{"path":"./main.lua"}]}"#,
        );
        fs::write(root.join("main.lua"), b"return true").expect("main");
        fs::write(root.join(MANAGER_MACOS_HELPER), universal_macos_fixture())
            .expect("macOS helper");
        fs::write(root.join(MANAGER_WINDOWS_HELPER), b"MZwindows").expect("Windows helper");
        fs::write(root.join(MANAGER_LINUX_HELPER), b"\x7fELFlinux").expect("Linux helper");
        copy_test_registry(&root.join("registry"));
    }

    #[test]
    fn manifest_names_must_be_safe_portable_cache_ids() {
        for name in [
            ".",
            "..",
            "CON",
            "con.json",
            "PRN",
            "aux.extension",
            "NUL",
            "COM1",
            "com9.package",
            "LPT1",
            "LPT9.extension",
            "name.",
        ] {
            let manifest = Manifest {
                name: name.to_owned(),
                version: "1.0.0".to_owned(),
                display_name: None,
                main: None,
                contributes: Value::Null,
            };
            let error = validate_manifest(&manifest)
                .expect_err("unsafe manifest name must be rejected before cache use");
            assert_eq!(error.code, "INVALID_MANIFEST", "name: {name}");
        }

        for name in ["console", "COM0", "COM10", "LPT0", "LPT10"] {
            let manifest = Manifest {
                name: name.to_owned(),
                version: "1.0.0".to_owned(),
                display_name: None,
                main: None,
                contributes: Value::Null,
            };
            validate_manifest(&manifest).expect("non-device manifest name should remain valid");
        }
    }

    #[test]
    fn manager_directory_packaging_is_deterministic_and_validated_separately() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let root = temporary.path().join("manager");
        write_manager_fixture(&root, "1.2.3");
        fs::write(root.join("__info.json"), b"runtime metadata").expect("info");
        fs::write(root.join("__pref.lua"), b"runtime preferences").expect("preferences");
        let state = State::new(temporary.path().join("config")).expect("state");

        let manifest = validate_manager_directory(&root, "1.2.3").expect("directory");
        assert_eq!(manifest.name, MANAGER_NAME);
        let first = package_manager_directory(&state, &root).expect("first package");
        let second = package_manager_directory(&state, &root).expect("second package");
        assert_eq!(first.sha256, second.sha256);
        assert_eq!(first.artifact_path, second.artifact_path);
        validate_manager_and_stage(&state, &first.artifact_path, "1.2.3").expect("manager archive");

        let file = fs::File::open(&first.artifact_path).expect("open package");
        let mut archive = ZipArchive::new(file).expect("zip");
        let mut names = BTreeSet::new();
        for index in 0..archive.len() {
            let entry = archive.by_index(index).expect("entry");
            let name = entry.name().to_owned();
            assert_eq!(
                entry.unix_mode().expect("mode") & 0o777,
                manager_file_mode(&name)
            );
            names.insert(name);
        }
        assert!(!names.contains("__info.json"));
        assert!(!names.contains("__pref.lua"));
        assert!(names.contains(MANAGER_MACOS_HELPER));
        assert!(names.contains(MANAGER_WINDOWS_HELPER));
        assert!(names.contains(MANAGER_LINUX_HELPER));

        assert_eq!(
            validate_extension(&first.artifact_path, ExpectedManifest::default())
                .expect_err("generic validator still rejects native manager helpers")
                .code,
            "NATIVE_CODE_UNSUPPORTED"
        );
    }

    #[test]
    fn manager_release_accepts_bounded_fat64_macos_helper() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let root = temporary.path().join("manager");
        write_manager_fixture(&root, "1.2.3");
        fs::write(root.join(MANAGER_MACOS_HELPER), universal_macos64_fixture())
            .expect("fat64 macOS helper");

        validate_manager_release_directory(&root, "1.2.3")
            .expect("fat64 manager release directory");
    }

    #[test]
    fn manager_recovery_accepts_supported_thin_macos_helper_but_release_rejects_it() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let state = State::new(temporary.path().join("config")).expect("state");
        for (architecture, cpu) in [("x86_64", 0x0100_0007), ("arm64", 0x0100_000c)] {
            let root = temporary.path().join(architecture);
            write_manager_fixture(&root, "1.2.3");
            fs::write(root.join(MANAGER_MACOS_HELPER), thin_macos_fixture(cpu))
                .expect("thin macOS helper");

            validate_manager_directory(&root, "1.2.3").expect("installed manager directory");
            assert_eq!(
                validate_manager_release_directory(&root, "1.2.3")
                    .expect_err("public release directory must be universal")
                    .code,
                "INVALID_MANAGER_HELPER"
            );
            let recovery = package_manager_directory(&state, &root).expect("recovery package");
            assert_eq!(
                validate_manager_archive(&recovery.artifact_path, "1.2.3")
                    .expect_err("public release must be universal")
                    .code,
                "INVALID_MANAGER_HELPER"
            );
            validate_manager_recovery_and_stage(&state, &recovery.artifact_path, "1.2.3")
                .expect("verified recovery package");
        }
    }

    #[test]
    fn manager_validation_requires_exact_identity_stable_version_and_layout() {
        let temporary = tempfile::tempdir().expect("tempdir");

        let wrong_name = temporary.path().join("wrong-name");
        write_manager_fixture(&wrong_name, "1.2.3");
        write_manager_manifest(
            &wrong_name,
            "1.2.3",
            "Extension Manager",
            r#"{"scripts":[{"path":"./main.lua"}]}"#,
        );
        assert_eq!(
            validate_manager_directory(&wrong_name, "1.2.3")
                .expect_err("exact identity")
                .code,
            "INVALID_MANAGER_MANIFEST"
        );

        let prerelease = temporary.path().join("prerelease");
        write_manager_fixture(&prerelease, "1.2.3-beta.1");
        assert_eq!(
            validate_manager_directory(&prerelease, "1.2.3-beta.1")
                .expect_err("stable version")
                .code,
            "INVALID_MANAGER_MANIFEST"
        );

        let mismatch = temporary.path().join("mismatch");
        write_manager_fixture(&mismatch, "1.2.3");
        assert_eq!(
            validate_manager_directory(&mismatch, "1.2.4")
                .expect_err("expected version")
                .code,
            "MANIFEST_MISMATCH"
        );

        let missing_helper = temporary.path().join("missing-helper");
        write_manager_fixture(&missing_helper, "1.2.3");
        fs::remove_file(missing_helper.join(MANAGER_LINUX_HELPER)).expect("remove helper");
        assert_eq!(
            validate_manager_directory(&missing_helper, "1.2.3")
                .expect_err("required helper")
                .code,
            "INVALID_MANAGER_PACKAGE"
        );

        let missing_consistent_catalog = temporary.path().join("missing-consistent-catalog");
        write_manager_fixture(&missing_consistent_catalog, "1.2.3");
        let consistent_catalog = fs::read_dir(missing_consistent_catalog.join("registry/targets"))
            .expect("catalog targets")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name != "catalog-v1.json")
            })
            .expect("consistent catalog target");
        fs::remove_file(consistent_catalog).expect("remove consistent catalog");
        assert_eq!(
            validate_manager_directory(&missing_consistent_catalog, "1.2.3")
                .expect_err("consistent catalog")
                .code,
            "INVALID_MANAGER_PACKAGE"
        );

        let nested_manifest = temporary.path().join("nested-manifest");
        write_manager_fixture(&nested_manifest, "1.2.3");
        fs::create_dir_all(nested_manifest.join("nested")).expect("nested");
        fs::write(nested_manifest.join("nested/package.json"), b"{}").expect("nested manifest");
        assert_eq!(
            validate_manager_directory(&nested_manifest, "1.2.3")
                .expect_err("single manifest")
                .code,
            "INVALID_MANIFEST_COUNT"
        );

        let missing_contribution = temporary.path().join("missing-contribution");
        write_manager_fixture(&missing_contribution, "1.2.3");
        write_manager_manifest(
            &missing_contribution,
            "1.2.3",
            MANAGER_DISPLAY_NAME,
            r#"{"scripts":[{"path":"./missing.lua"}]}"#,
        );
        assert_eq!(
            validate_manager_directory(&missing_contribution, "1.2.3")
                .expect_err("main contribution")
                .code,
            "MISSING_CONTRIBUTION"
        );
    }

    #[test]
    fn manager_validation_allows_only_exact_helpers_and_public_registry_material() {
        let temporary = tempfile::tempdir().expect("tempdir");

        let bad_magic = temporary.path().join("bad-magic");
        write_manager_fixture(&bad_magic, "1.2.3");
        fs::write(
            bad_magic.join(MANAGER_MACOS_HELPER),
            b"\xcf\xfa\xed\xfesingle-arch",
        )
        .expect("replace helper");
        assert_eq!(
            validate_manager_directory(&bad_magic, "1.2.3")
                .expect_err("universal helper")
                .code,
            "INVALID_MANAGER_HELPER"
        );

        let truncated_thin = temporary.path().join("truncated-thin");
        write_manager_fixture(&truncated_thin, "1.2.3");
        fs::write(
            truncated_thin.join(MANAGER_MACOS_HELPER),
            b"\xcf\xfa\xed\xfe\x0c\x00\x00\x01",
        )
        .expect("replace helper");
        assert_eq!(
            validate_manager_directory(&truncated_thin, "1.2.3")
                .expect_err("complete thin helper header")
                .code,
            "INVALID_MANAGER_HELPER"
        );

        let unsupported_thin = temporary.path().join("unsupported-thin");
        write_manager_fixture(&unsupported_thin, "1.2.3");
        fs::write(
            unsupported_thin.join(MANAGER_MACOS_HELPER),
            thin_macos_fixture(0x0100_0012),
        )
        .expect("replace helper");
        assert_eq!(
            validate_manager_directory(&unsupported_thin, "1.2.3")
                .expect_err("supported thin helper architecture")
                .code,
            "INVALID_MANAGER_HELPER"
        );

        let fat_stub = temporary.path().join("fat-stub");
        write_manager_fixture(&fat_stub, "1.2.3");
        fs::write(fat_stub.join(MANAGER_MACOS_HELPER), b"\xca\xfe\xba\xbe")
            .expect("replace helper");
        assert_eq!(
            validate_manager_directory(&fat_stub, "1.2.3")
                .expect_err("complete universal helper")
                .code,
            "INVALID_MANAGER_HELPER"
        );

        let extra_native = temporary.path().join("extra-native");
        write_manager_fixture(&extra_native, "1.2.3");
        fs::write(extra_native.join("payload.dat"), b"\x7fELFunexpected").expect("native payload");
        assert_eq!(
            validate_manager_directory(&extra_native, "1.2.3")
                .expect_err("extra native")
                .code,
            "NATIVE_CODE_UNSUPPORTED"
        );

        let native_name = temporary.path().join("native-name");
        write_manager_fixture(&native_name, "1.2.3");
        fs::write(native_name.join("unexpected.dll"), b"not executable").expect("native name");
        assert_eq!(
            validate_manager_directory(&native_name, "1.2.3")
                .expect_err("extra native name")
                .code,
            "NATIVE_CODE_UNSUPPORTED"
        );

        let private_registry = temporary.path().join("private-registry");
        write_manager_fixture(&private_registry, "1.2.3");
        fs::create_dir_all(private_registry.join("registry/keys")).expect("keys");
        fs::write(private_registry.join("registry/keys/root.json"), b"secret")
            .expect("private key");
        assert_eq!(
            validate_manager_directory(&private_registry, "1.2.3")
                .expect_err("private registry")
                .code,
            "PRIVATE_REGISTRY_MATERIAL"
        );
    }

    #[test]
    fn manager_archive_rejects_unsafe_paths_and_types() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let state = State::new(temporary.path().join("config")).expect("state");

        let traversal = temporary.path().join("manager-traversal.zip");
        write_package(
            &traversal,
            &[
                (
                    "package.json",
                    br#"{"name":"aseprite-extension-manager","displayName":"Aseprite Extension Manager","version":"1.2.3","contributes":{"scripts":[{"path":"./main.lua"}]}}"#,
                ),
                ("../outside.lua", b"bad"),
            ],
        );
        assert_eq!(
            validate_manager_and_stage(&state, &traversal, "1.2.3")
                .expect_err("traversal")
                .code,
            "UNSAFE_ARCHIVE_PATH"
        );

        let linked = temporary.path().join("manager-link.zip");
        let file = fs::File::create(&linked).expect("create");
        let mut writer = ZipWriter::new(file);
        let options = SimpleFileOptions::default().unix_permissions(0o644);
        writer
            .start_file("package.json", options)
            .expect("manifest");
        writer
            .write_all(
                br#"{"name":"aseprite-extension-manager","displayName":"Aseprite Extension Manager","version":"1.2.3","contributes":{"scripts":[{"path":"./main.lua"}]}}"#,
            )
            .expect("write manifest");
        writer
            .add_symlink("main.lua", "../outside.lua", options)
            .expect("symlink");
        writer.finish().expect("finish");
        assert_eq!(
            validate_manager_and_stage(&state, &linked, "1.2.3")
                .expect_err("symlink")
                .code,
            "UNSUPPORTED_FILE_TYPE"
        );
    }

    #[test]
    fn accepts_minimal_extension() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let path = temporary.path().join("sample.zip");
        write_package(
            &path,
            &[
                (
                    "package.json",
                    br#"{"name":"sample","version":"1.2.3","main":"./main.lua"}"#,
                ),
                ("main.lua", b"return true"),
            ],
        );
        let manifest = validate_extension(&path, ExpectedManifest::default()).expect("valid");
        assert_eq!(manifest.name, "sample");
    }

    #[test]
    fn rejects_traversal_and_case_collisions() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let traversal = temporary.path().join("traversal.zip");
        write_package(
            &traversal,
            &[
                ("package.json", br#"{"name":"sample","version":"1"}"#),
                ("../outside.lua", b"bad"),
            ],
        );
        assert_eq!(
            validate_extension(&traversal, ExpectedManifest::default())
                .expect_err("reject")
                .code,
            "UNSAFE_ARCHIVE_PATH"
        );

        let absolute = temporary.path().join("absolute.zip");
        write_package(
            &absolute,
            &[
                ("package.json", br#"{"name":"sample","version":"1"}"#),
                ("/absolute.lua", b"bad"),
            ],
        );
        assert_eq!(
            validate_extension(&absolute, ExpectedManifest::default())
                .expect_err("reject")
                .code,
            "UNSAFE_ARCHIVE_PATH"
        );

        let collision = temporary.path().join("collision.zip");
        write_package(
            &collision,
            &[
                ("package.json", br#"{"name":"sample","version":"1"}"#),
                ("Code.lua", b"one"),
                ("code.lua", b"two"),
            ],
        );
        assert_eq!(
            validate_extension(&collision, ExpectedManifest::default())
                .expect_err("reject")
                .code,
            "PATH_COLLISION"
        );
    }

    #[test]
    fn rejects_encryption_links_multiple_manifests_and_missing_contributions() {
        let temporary = tempfile::tempdir().expect("tempdir");

        let encrypted = temporary.path().join("encrypted.zip");
        write_encrypted_package(&encrypted);
        assert_eq!(
            validate_extension(&encrypted, ExpectedManifest::default())
                .expect_err("encrypted")
                .code,
            "ENCRYPTED_ARCHIVE"
        );

        let linked = temporary.path().join("linked.zip");
        let file = fs::File::create(&linked).expect("create");
        let mut writer = ZipWriter::new(file);
        let options = SimpleFileOptions::default().unix_permissions(0o644);
        writer
            .start_file("package.json", options)
            .expect("manifest");
        writer
            .write_all(br#"{"name":"sample","version":"1.0.0","main":"link.lua"}"#)
            .expect("write manifest");
        writer
            .add_symlink("link.lua", "../outside.lua", options)
            .expect("symlink");
        writer.finish().expect("finish");
        assert_eq!(
            validate_extension(&linked, ExpectedManifest::default())
                .expect_err("link")
                .code,
            "UNSUPPORTED_FILE_TYPE"
        );

        let multiple = temporary.path().join("multiple.zip");
        write_package(
            &multiple,
            &[
                ("package.json", br#"{"name":"sample","version":"1.0.0"}"#),
                (
                    "nested/package.json",
                    br#"{"name":"nested","version":"1.0.0"}"#,
                ),
            ],
        );
        assert_eq!(
            validate_extension(&multiple, ExpectedManifest::default())
                .expect_err("multiple manifests")
                .code,
            "INVALID_MANIFEST_COUNT"
        );

        let missing = temporary.path().join("missing.zip");
        write_package(
            &missing,
            &[(
                "package.json",
                br#"{"name":"sample","version":"1.0.0","main":"missing.lua"}"#,
            )],
        );
        assert_eq!(
            validate_extension(&missing, ExpectedManifest::default())
                .expect_err("missing contribution")
                .code,
            "MISSING_CONTRIBUTION"
        );
    }

    #[test]
    fn accepts_unicode_and_space_heavy_paths() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let path = temporary.path().join("unicode.zip");
        write_package(
            &path,
            &[
                (
                    "package.json",
                    br#"{"name":"sample","version":"1.0.0","main":"./scripts/\u00e5 space.lua"}"#,
                ),
                ("scripts/å space.lua", b"return true"),
            ],
        );
        validate_extension(&path, ExpectedManifest::default()).expect("portable Unicode path");
    }

    #[test]
    fn rejects_suspicious_compression_ratios() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let path = temporary.path().join("bomb.zip");
        let file = fs::File::create(&path).expect("create");
        let mut writer = ZipWriter::new(file);
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .unix_permissions(0o644);
        writer
            .start_file("package.json", options)
            .expect("manifest");
        writer
            .write_all(br#"{"name":"sample","version":"1.0.0"}"#)
            .expect("write manifest");
        writer.start_file("payload.txt", options).expect("payload");
        writer
            .write_all(&vec![0_u8; 2 * 1024 * 1024])
            .expect("write payload");
        writer.finish().expect("finish");
        assert_eq!(
            validate_extension(&path, ExpectedManifest::default())
                .expect_err("compression ratio")
                .code,
            "ARCHIVE_BOMB"
        );
    }

    #[test]
    fn local_snapshot_is_deterministic_and_honors_excludes() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let source = temporary.path().join("source");
        fs::create_dir_all(source.join(".git")).expect("mkdir");
        fs::write(
            source.join("package.json"),
            br#"{"name":"sample","version":"1.0.0","main":"main.lua"}"#,
        )
        .expect("manifest");
        fs::write(source.join("main.lua"), b"return true").expect("lua");
        fs::write(source.join(".DS_Store"), b"ignored").expect("metadata");
        fs::write(source.join(".git/config"), b"ignored").expect("git");
        fs::write(source.join(".aemignore"), b"ignored.lua\n").expect("ignore");
        fs::write(source.join("ignored.lua"), b"ignored").expect("ignored file");
        let state = State::new(temporary.path().join("config")).expect("state");

        let first =
            package_local_folder(&state, &source.join("package.json")).expect("first snapshot");
        let second =
            package_local_folder(&state, &source.join("package.json")).expect("second snapshot");

        assert_eq!(first.sha256, second.sha256);
        assert_eq!(first.content_hash, second.content_hash);
        let file = fs::File::open(&first.artifact_path).expect("artifact");
        let mut archive = ZipArchive::new(file).expect("zip");
        let names: Vec<_> = (0..archive.len())
            .map(|index| archive.by_index(index).expect("entry").name().to_owned())
            .collect();
        assert!(!names.iter().any(|name| name == ".aemignore"));
        assert!(!names.iter().any(|name| name == "ignored.lua"));
    }

    #[test]
    fn repository_snapshot_removes_archive_prefix_and_normalizes_exclusions() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let repository = temporary.path().join("repository.zip");
        write_package(
            &repository,
            &[
                (
                    "sample-commit/package.json",
                    br#"{"name":"sample","version":"1.2.3","main":"main.lua"}"#,
                ),
                ("sample-commit/main.lua", b"return true"),
                ("sample-commit/.git/config", b"ignored"),
                ("sample-commit/.aemignore", b"scratch.txt\n"),
                ("sample-commit/scratch.txt", b"ignored"),
            ],
        );
        let state = State::new(temporary.path().join("config")).expect("state");

        let package = package_repository_archive(&state, &repository).expect("repository snapshot");

        assert_eq!(package.name, "sample");
        assert_eq!(package.version, "1.2.3");
        let file = fs::File::open(package.artifact_path).expect("artifact");
        let mut archive = ZipArchive::new(file).expect("zip");
        let names: Vec<_> = (0..archive.len())
            .map(|index| archive.by_index(index).expect("entry").name().to_owned())
            .collect();
        assert!(names.iter().any(|name| name == "package.json"));
        assert!(names.iter().any(|name| name == "main.lua"));
        assert!(!names.iter().any(|name| name.starts_with("sample-commit/")));
        assert!(!names.iter().any(|name| name.starts_with(".git/")));
        assert!(!names.iter().any(|name| name == "scratch.txt"));
    }

    #[test]
    fn rejects_native_content_by_name_and_magic() {
        for path in [
            "bin/helper.dll",
            "lib/helper.a",
            "lib/helper.LIB",
            "objects/helper.o",
            "objects/helper.obj",
        ] {
            assert_eq!(
                reject_native_name(path).expect_err(path).code,
                "NATIVE_CODE_UNSUPPORTED"
            );
        }
        assert!(reject_native_magic("payload.dat", b"\x7fELFmore").is_err());
        assert_eq!(
            reject_native_magic("payload.dat", b"!<arch>\nmember")
                .expect_err("static archive")
                .code,
            "NATIVE_CODE_UNSUPPORTED"
        );
        assert_eq!(
            reject_native_magic(
                "payload.dat",
                &[
                    0x64, 0x86, // x86-64
                    0x01, 0x00, // one section
                    0x00, 0x00, 0x00, 0x00, // timestamp
                    0x00, 0x00, 0x00, 0x00, // symbol-table pointer
                    0x00, 0x00, 0x00, 0x00, // symbol count
                    0x00, 0x00, // no optional header
                    0x00, 0x00, // characteristics
                ],
            )
            .expect_err("COFF object")
            .code,
            "NATIVE_CODE_UNSUPPORTED"
        );
        assert!(reject_native_magic("payload.dat", b"\xca\xfe\xba\xbfmore").is_err());
        assert!(reject_native_magic("payload.dat", b"\xbf\xba\xfe\xcamore").is_err());
    }

    #[test]
    fn rejects_windows_aliases_and_nonportable_paths() {
        for path in [
            "folder/file.lua:stream",
            "folder/trailing. ",
            "folder/aux.txt",
            "folder/COM1.lua",
            "folder/conout$.txt",
            "folder/question?.lua",
        ] {
            assert_eq!(
                normalize_archive_path(path).expect_err(path).code,
                "UNSAFE_ARCHIVE_PATH"
            );
        }
        assert_eq!(
            normalize_archive_path("folder with spaces/main.lua").unwrap(),
            "folder with spaces/main.lua"
        );
    }

    #[test]
    fn accepts_root_theme_contribution_path() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let path = temporary.path().join("theme.zip");
        write_package(
            &path,
            &[(
                "package.json",
                br#"{"name":"theme","version":"1.0.0","contributes":{"themes":[{"id":"theme","path":"."}]}}"#,
            )],
        );
        validate_extension(&path, ExpectedManifest::default()).expect("root contribution");
    }
}
