use std::fs;
use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use crate::protocol::{RpcError, RpcResult};

const STAGING_CACHE_LIMIT: u64 = 256 * 1024 * 1024;
const HTTP_CACHE_LIMIT: u64 = 16 * 1024 * 1024;
const LOG_FILE_LIMIT: u64 = 512 * 1024;

#[derive(Clone, Debug)]
pub struct State {
    root: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Receipt {
    pub schema_version: u32,
    pub package_name: String,
    pub source_kind: String,
    pub source: Value,
    pub installed_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset: Option<String>,
    pub artifact_sha256: String,
    pub artifact_byte_length: u64,
    pub installed_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_folder: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_artifact: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_source: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_artifact_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_artifact_byte_length: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingSelfUpdate {
    pub schema_version: u32,
    pub target_version: String,
    pub previous_version: String,
    #[serde(default)]
    pub target_is_recovery: bool,
    pub source: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_source: Option<Value>,
    pub artifact_sha256: String,
    pub artifact_byte_length: u64,
    pub recovery_sha256: String,
    pub recovery_byte_length: u64,
    pub prepared_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheEntry {
    pub package_name: String,
    pub current: Option<PathBuf>,
    pub previous: Option<PathBuf>,
    pub bytes: u64,
}

#[derive(Clone, Debug)]
pub struct QuarantinedExtension {
    pub recovery_path: PathBuf,
    pub receipt_cleanup_pending: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UninstallRecord {
    schema_version: u32,
    name: String,
    version: String,
    original_path: PathBuf,
    phase: String,
    uninstalled_at: DateTime<Utc>,
}

impl State {
    pub fn new(user_config_path: impl AsRef<Path>) -> RpcResult<Self> {
        let root = user_config_path.as_ref().join("extension-manager");
        let state = Self { root };
        state.ensure()?;
        Ok(state)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn ensure(&self) -> RpcResult<()> {
        ensure_real_state_directory(&self.root)?;
        for name in [
            "cache",
            "receipts",
            "staging",
            "http",
            "tuf",
            "logs",
            "self-update",
            "uninstalled",
        ] {
            ensure_real_state_directory(&self.root.join(name))?;
        }
        self.reconcile_uninstalls()
    }

    pub fn begin_self_update(
        &self,
        candidate: &Path,
        recovery: &Path,
        pending: &PendingSelfUpdate,
    ) -> RpcResult<(PathBuf, PathBuf)> {
        let directory = self.root.join("self-update");
        let pending_path = directory.join("pending.json");
        if pending_path.exists() {
            return Err(RpcError::invalid(
                "SELF_UPDATE_PENDING",
                "a manager update is already waiting to be completed or recovered",
            ));
        }

        let candidate_path = directory.join("candidate.aseprite-extension");
        let recovery_path = directory.join("recovery.aseprite-extension");
        atomic_copy(candidate, &candidate_path).map_err(RpcError::io)?;
        if let Err(error) = atomic_copy(recovery, &recovery_path) {
            let _ = fs::remove_file(&candidate_path);
            return Err(RpcError::io(error));
        }
        let (candidate_copy, recovery_copy) =
            match (sha256_file(&candidate_path), sha256_file(&recovery_path)) {
                (Ok(candidate_copy), Ok(recovery_copy)) => (candidate_copy, recovery_copy),
                (Err(error), _) | (_, Err(error)) => {
                    let _ = fs::remove_file(&candidate_path);
                    let _ = fs::remove_file(&recovery_path);
                    return Err(RpcError::io(error));
                }
            };
        if candidate_copy
            != (
                pending.artifact_sha256.clone(),
                pending.artifact_byte_length,
            )
            || recovery_copy
                != (
                    pending.recovery_sha256.clone(),
                    pending.recovery_byte_length,
                )
        {
            let _ = fs::remove_file(&candidate_path);
            let _ = fs::remove_file(&recovery_path);
            return Err(RpcError::invalid(
                "SELF_UPDATE_ARTIFACT_CHANGED",
                "a manager update or recovery package changed while it was being prepared",
            ));
        }
        let data = serde_json::to_vec_pretty(pending)
            .map_err(|error| RpcError::internal(error.to_string()))?;
        if let Err(error) = atomic_write(&pending_path, &data) {
            let _ = fs::remove_file(&candidate_path);
            let _ = fs::remove_file(&recovery_path);
            return Err(RpcError::io(error));
        }
        Ok((candidate_path, recovery_path))
    }

    pub fn pending_self_update(&self) -> RpcResult<Option<PendingSelfUpdate>> {
        let path = self.root.join("self-update").join("pending.json");
        if !path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&path).map_err(RpcError::io)?;
        serde_json::from_slice(&bytes).map(Some).map_err(|error| {
            RpcError::state(format!(
                "invalid pending manager update {}: {error}",
                path.display()
            ))
        })
    }

    pub fn self_update_artifact(&self, recovery: bool) -> Option<PathBuf> {
        let filename = if recovery {
            "recovery.aseprite-extension"
        } else {
            "candidate.aseprite-extension"
        };
        let path = self.root.join("self-update").join(filename);
        path.is_file().then_some(path)
    }

    pub fn clear_self_update(&self) -> RpcResult<()> {
        let directory = self.root.join("self-update");
        for filename in [
            "pending.json",
            "candidate.aseprite-extension",
            "recovery.aseprite-extension",
        ] {
            let path = directory.join(filename);
            if path.exists() {
                fs::remove_file(path).map_err(RpcError::io)?;
            }
        }
        Ok(())
    }

    pub fn stage_bytes(&self, bytes: &[u8]) -> RpcResult<(PathBuf, String)> {
        let hash = sha256_bytes(bytes);
        let path = self
            .root
            .join("staging")
            .join(format!("{hash}.aseprite-extension"));
        let existing_matches = fs::symlink_metadata(&path)
            .map(|metadata| metadata.file_type().is_file())
            .unwrap_or(false)
            && sha256_file(&path)
                .map(|(existing_hash, existing_length)| {
                    existing_hash == hash && existing_length == bytes.len() as u64
                })
                .unwrap_or(false);
        if !existing_matches {
            atomic_write(&path, bytes).map_err(RpcError::io)?;
        }
        evict_directory(&self.root.join("staging"), STAGING_CACHE_LIMIT, Some(&path))
            .map_err(RpcError::io)?;
        Ok((path, hash))
    }

    pub fn stage_file(&self, source: &Path) -> RpcResult<(PathBuf, String, u64)> {
        let mut input = fs::File::open(source).map_err(RpcError::io)?;
        let mut tmp = NamedTempFile::new_in(self.root.join("staging")).map_err(RpcError::io)?;
        let mut hasher = Sha256::new();
        let mut bytes = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let count = input.read(&mut buffer).map_err(RpcError::io)?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
            tmp.write_all(&buffer[..count]).map_err(RpcError::io)?;
            bytes += count as u64;
        }
        drop(input);
        tmp.as_file_mut().sync_all().map_err(RpcError::io)?;
        let hash = hex::encode(hasher.finalize());
        let destination = self
            .root
            .join("staging")
            .join(format!("{hash}.aseprite-extension"));
        let existing_matches = fs::symlink_metadata(&destination)
            .map(|metadata| metadata.file_type().is_file())
            .unwrap_or(false)
            && sha256_file(&destination)
                .map(|(existing_hash, existing_length)| {
                    existing_hash == hash && existing_length == bytes
                })
                .unwrap_or(false);
        if existing_matches {
            drop(tmp);
        } else {
            if fs::symlink_metadata(&destination).is_ok() {
                fs::remove_file(&destination).map_err(RpcError::io)?;
            }
            tmp.persist(&destination)
                .map_err(|error| RpcError::io(error.error))?;
        }
        evict_directory(
            &self.root.join("staging"),
            STAGING_CACHE_LIMIT,
            Some(&destination),
        )
        .map_err(RpcError::io)?;
        Ok((destination, hash, bytes))
    }

    pub fn cache_artifact(
        &self,
        package_name: &str,
        artifact: &Path,
    ) -> RpcResult<(PathBuf, Option<PathBuf>)> {
        let package_id = safe_package_id(package_name)?;
        let package_dir = self.root.join("cache").join(package_id);
        fs::create_dir_all(&package_dir).map_err(RpcError::io)?;
        let current = package_dir.join("current.aseprite-extension");
        let previous = package_dir.join("previous.aseprite-extension");

        if current.exists() && !files_equal(&current, artifact).map_err(RpcError::io)? {
            atomic_copy(&current, &previous).map_err(RpcError::io)?;
        }
        if !current.exists() || !files_equal(&current, artifact).map_err(RpcError::io)? {
            atomic_copy(artifact, &current).map_err(RpcError::io)?;
        }
        if !files_equal(&current, artifact).map_err(RpcError::io)? {
            return Err(RpcError::state(
                "cached package differs from the verified staged artifact",
            ));
        }
        Ok((current, previous.exists().then_some(previous)))
    }

    pub fn cached_artifact(
        &self,
        package_name: &str,
        previous: bool,
    ) -> RpcResult<Option<PathBuf>> {
        let filename = if previous {
            "previous.aseprite-extension"
        } else {
            "current.aseprite-extension"
        };
        let path = self
            .root
            .join("cache")
            .join(safe_package_id(package_name)?)
            .join(filename);
        Ok(path.exists().then_some(path))
    }

    pub fn write_receipt(&self, receipt: &Receipt) -> RpcResult<PathBuf> {
        let path = self.receipt_path(&receipt.package_name)?;
        let data = serde_json::to_vec_pretty(receipt)
            .map_err(|error| RpcError::internal(error.to_string()))?;
        atomic_write(&path, &data).map_err(RpcError::io)?;
        Ok(path)
    }

    pub fn read_receipt(&self, package_name: &str) -> RpcResult<Option<Receipt>> {
        let path = self.receipt_path(package_name)?;
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(RpcError::io(error)),
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(RpcError::state(format!(
                "receipt must be a real file: {}",
                path.display()
            )));
        }
        let data = fs::read(&path).map_err(RpcError::io)?;
        serde_json::from_slice(&data).map(Some).map_err(|error| {
            RpcError::state(format!("invalid receipt {}: {error}", path.display()))
        })
    }

    pub fn quarantine_extension(
        &self,
        package_name: &str,
        version: &str,
        extension_path: &Path,
        archive_receipt: bool,
    ) -> RpcResult<QuarantinedExtension> {
        let uninstalled = self.root.join("uninstalled");
        ensure_real_state_directory(&uninstalled)?;
        let quarantine = tempfile::Builder::new()
            .prefix("extension-")
            .tempdir_in(&uninstalled)
            .map_err(RpcError::io)?;
        let mut record = UninstallRecord {
            schema_version: 1,
            name: package_name.to_owned(),
            version: version.to_owned(),
            original_path: extension_path.to_owned(),
            phase: "prepared".to_owned(),
            uninstalled_at: Utc::now(),
        };
        let metadata = serde_json::to_vec_pretty(&record)
            .map_err(|error| RpcError::internal(error.to_string()))?;
        atomic_write(&quarantine.path().join("uninstall.json"), &metadata).map_err(RpcError::io)?;

        if archive_receipt {
            let receipt = self.receipt_path(package_name)?;
            let metadata = fs::symlink_metadata(&receipt).map_err(RpcError::io)?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(RpcError::state(format!(
                    "receipt must be a real file: {}",
                    receipt.display()
                )));
            }
            let archived_path = quarantine.path().join("receipt.json");
            atomic_copy(&receipt, &archived_path).map_err(RpcError::io)?;
            let archived: Receipt = serde_json::from_slice(
                &fs::read(&archived_path).map_err(RpcError::io)?,
            )
            .map_err(|error| RpcError::state(format!("invalid archived receipt: {error}")))?;
            if !archived.package_name.eq_ignore_ascii_case(package_name)
                || archived.installed_version != version
                || !files_equal(&receipt, &archived_path).map_err(RpcError::io)?
            {
                return Err(RpcError::state(
                    "active receipt changed while the uninstall was being prepared",
                ));
            }
        }

        let quarantined_extension = quarantine.path().join("extension");
        fs::rename(extension_path, &quarantined_extension).map_err(RpcError::io)?;
        let quarantine = quarantine.keep();
        let recovery_path = quarantine.join("extension");
        let source_parent_synced = extension_path
            .parent()
            .is_some_and(|parent| sync_parent(parent).is_ok());
        let quarantine_synced = sync_parent(&quarantine).is_ok();
        let uninstalled_synced = sync_parent(&uninstalled).is_ok();
        let move_is_durable = source_parent_synced && quarantine_synced && uninstalled_synced;
        record.phase = "committed".to_owned();
        if let Ok(metadata) = serde_json::to_vec_pretty(&record) {
            let _ = atomic_write(&quarantine.join("uninstall.json"), &metadata);
        }
        let receipt_cleanup_pending = archive_receipt
            && (!move_is_durable
                || self
                    .remove_matching_receipt(&quarantine, package_name, version)
                    .is_err());
        Ok(QuarantinedExtension {
            recovery_path,
            receipt_cleanup_pending,
        })
    }

    pub fn receipts(&self) -> RpcResult<Vec<Receipt>> {
        let directory = self.root.join("receipts");
        ensure_real_state_directory(&directory)?;
        let mut receipts: Vec<Receipt> = Vec::new();
        for entry in fs::read_dir(directory).map_err(RpcError::io)? {
            let entry = entry.map_err(RpcError::io)?;
            if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let file_type = entry.file_type().map_err(RpcError::io)?;
            if file_type.is_symlink() || !file_type.is_file() {
                continue;
            }
            let data = fs::read(entry.path()).map_err(RpcError::io)?;
            if let Ok(receipt) = serde_json::from_slice(&data) {
                receipts.push(receipt);
            }
        }
        receipts.sort_by(|left, right| left.package_name.cmp(&right.package_name));
        Ok(receipts)
    }

    fn receipt_path(&self, package_name: &str) -> RpcResult<PathBuf> {
        let receipts = self.root.join("receipts");
        ensure_real_state_directory(&receipts)?;
        Ok(receipts.join(format!("{}.json", safe_package_id(package_name)?)))
    }

    fn remove_matching_receipt(
        &self,
        quarantine: &Path,
        package_name: &str,
        version: &str,
    ) -> RpcResult<()> {
        let archived_path = quarantine.join("receipt.json");
        let archived_metadata = fs::symlink_metadata(&archived_path).map_err(RpcError::io)?;
        if archived_metadata.file_type().is_symlink() || !archived_metadata.is_file() {
            return Err(RpcError::state(
                "archived uninstall receipt is not a real file",
            ));
        }
        let archived: Receipt =
            serde_json::from_slice(&fs::read(&archived_path).map_err(RpcError::io)?)
                .map_err(|error| RpcError::state(format!("invalid archived receipt: {error}")))?;
        if !archived.package_name.eq_ignore_ascii_case(package_name)
            || archived.installed_version != version
        {
            return Err(RpcError::state(
                "archived uninstall receipt does not match the extension",
            ));
        }

        let active_path = self.receipt_path(package_name)?;
        let active = match self.read_receipt(package_name)? {
            Some(receipt) => receipt,
            None => return Ok(()),
        };
        if !active.package_name.eq_ignore_ascii_case(package_name)
            || active.installed_version != version
            || !files_equal(&active_path, &archived_path).map_err(RpcError::io)?
        {
            return Err(RpcError::state(
                "active receipt changed during the uninstall transaction",
            ));
        }
        fs::remove_file(&active_path).map_err(RpcError::io)?;
        sync_parent(
            active_path
                .parent()
                .ok_or_else(|| RpcError::state("receipt path has no parent"))?,
        )
        .map_err(RpcError::io)
    }

    fn reconcile_uninstalls(&self) -> RpcResult<()> {
        let uninstalled = self.root.join("uninstalled");
        for entry in fs::read_dir(&uninstalled).map_err(RpcError::io)? {
            let entry = entry.map_err(RpcError::io)?;
            let file_type = entry.file_type().map_err(RpcError::io)?;
            if file_type.is_symlink() || !file_type.is_dir() {
                continue;
            }
            let record_path = entry.path().join("uninstall.json");
            let record_metadata = match fs::symlink_metadata(&record_path) {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            if record_metadata.file_type().is_symlink() || !record_metadata.is_file() {
                continue;
            }
            let record: UninstallRecord = match fs::read(&record_path)
                .ok()
                .and_then(|data| serde_json::from_slice::<UninstallRecord>(&data).ok())
            {
                Some(record)
                    if record.schema_version == 1
                        && matches!(record.phase.as_str(), "prepared" | "committed") =>
                {
                    record
                }
                _ => continue,
            };
            let recovery = entry.path().join("extension");
            let recovery_metadata = match fs::symlink_metadata(&recovery) {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            if recovery_metadata.file_type().is_symlink() || !recovery_metadata.is_dir() {
                continue;
            }
            if fs::symlink_metadata(&record.original_path).is_ok() {
                continue;
            }
            let _ = self.remove_matching_receipt(&entry.path(), &record.name, &record.version);
        }
        Ok(())
    }

    pub fn cache_status(&self) -> RpcResult<Vec<CacheEntry>> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(self.root.join("cache")).map_err(RpcError::io)? {
            let entry = entry.map_err(RpcError::io)?;
            if !entry.file_type().map_err(RpcError::io)?.is_dir() {
                continue;
            }
            let current = entry.path().join("current.aseprite-extension");
            let previous = entry.path().join("previous.aseprite-extension");
            let paths = [&current, &previous];
            let bytes = paths
                .iter()
                .filter_map(|path| fs::metadata(path).ok())
                .map(|metadata| metadata.len())
                .sum();
            entries.push(CacheEntry {
                package_name: entry.file_name().to_string_lossy().into_owned(),
                current: current.exists().then_some(current),
                previous: previous.exists().then_some(previous),
                bytes,
            });
        }
        entries.sort_by(|left, right| left.package_name.cmp(&right.package_name));
        Ok(entries)
    }

    pub fn clear_cache(&self, package_name: Option<&str>) -> RpcResult<u64> {
        let cache = self.root.join("cache");
        let mut removed = 0_u64;
        if let Some(name) = package_name {
            let path = cache.join(safe_package_id(name)?);
            removed = directory_bytes(&path).map_err(RpcError::io)?;
            if path.exists() {
                fs::remove_dir_all(path).map_err(RpcError::io)?;
            }
            return Ok(removed);
        }
        for entry in fs::read_dir(&cache).map_err(RpcError::io)? {
            let entry = entry.map_err(RpcError::io)?;
            if entry.file_type().map_err(RpcError::io)?.is_dir() {
                removed += directory_bytes(&entry.path()).map_err(RpcError::io)?;
                fs::remove_dir_all(entry.path()).map_err(RpcError::io)?;
            }
        }
        Ok(removed)
    }

    pub fn http_cache_path(&self, key: &str) -> PathBuf {
        self.root.join("http").join(key)
    }

    pub fn enforce_http_cache_limit(&self, protected: Option<&Path>) -> RpcResult<()> {
        evict_directory(&self.root.join("http"), HTTP_CACHE_LIMIT, protected)
            .map(|_| ())
            .map_err(RpcError::io)
    }

    pub fn clear_transient_caches(&self) -> RpcResult<u64> {
        let mut removed = 0_u64;
        for directory in [self.root.join("staging"), self.root.join("http")] {
            for entry in fs::read_dir(&directory).map_err(RpcError::io)? {
                let entry = entry.map_err(RpcError::io)?;
                if entry.file_type().map_err(RpcError::io)?.is_file() {
                    removed += entry.metadata().map_err(RpcError::io)?.len();
                    fs::remove_file(entry.path()).map_err(RpcError::io)?;
                }
            }
        }
        Ok(removed)
    }

    pub fn tuf_path(&self, name: &str) -> PathBuf {
        self.root.join("tuf").join(name)
    }

    pub fn open_rotated_log(&self) -> RpcResult<fs::File> {
        let logs = self.root.join("logs");
        fs::create_dir_all(&logs).map_err(RpcError::io)?;
        let current = logs.join("helper.log");
        let previous = logs.join("helper.log.1");
        if fs::metadata(&current)
            .map(|metadata| metadata.len() >= LOG_FILE_LIMIT)
            .unwrap_or(false)
        {
            if previous.exists() {
                fs::remove_file(&previous).map_err(RpcError::io)?;
            }
            fs::rename(&current, &previous).map_err(RpcError::io)?;
        }
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(current)
            .map_err(RpcError::io)
    }
}

fn ensure_real_state_directory(path: &Path) -> RpcResult<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(RpcError::state(format!(
                    "manager state path must be a real directory: {}",
                    path.display()
                )));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(path).map_err(RpcError::io)?;
            let metadata = fs::symlink_metadata(path).map_err(RpcError::io)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(RpcError::state(format!(
                    "manager state path must be a real directory: {}",
                    path.display()
                )));
            }
        }
        Err(error) => return Err(RpcError::io(error)),
    }
    Ok(())
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
    fs::create_dir_all(parent)?;
    let mut temp = NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    temp.as_file_mut().sync_all()?;
    temp.persist(path).map_err(|error| error.error)?;
    sync_parent(parent)?;
    Ok(())
}

pub fn atomic_copy(source: &Path, destination: &Path) -> io::Result<()> {
    let parent = destination
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
    fs::create_dir_all(parent)?;
    let mut input = fs::File::open(source)?;
    let mut temp = NamedTempFile::new_in(parent)?;
    io::copy(&mut input, &mut temp)?;
    temp.as_file_mut().sync_all()?;
    temp.persist(destination).map_err(|error| error.error)?;
    sync_parent(parent)?;
    Ok(())
}

#[cfg(unix)]
fn sync_parent(parent: &Path) -> io::Result<()> {
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_parent(_: &Path) -> io::Result<()> {
    Ok(())
}

fn safe_package_id(name: &str) -> RpcResult<String> {
    if !package_id_is_safe(name) {
        return Err(RpcError::invalid(
            "INVALID_PACKAGE_NAME",
            "package name contains unsupported characters",
        ));
    }
    Ok(name.to_lowercase())
}

pub(crate) fn package_id_is_safe(name: &str) -> bool {
    if name.is_empty()
        || name.len() > 128
        || matches!(name, "." | "..")
        || name.ends_with('.')
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-._".contains(character))
    {
        return false;
    }
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|suffix| suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9'))
}

pub fn sha256_file(path: &Path) -> io::Result<(String, u64)> {
    let mut input = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        total += count as u64;
    }
    Ok((hex::encode(hasher.finalize()), total))
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn files_equal(left: &Path, right: &Path) -> io::Result<bool> {
    if fs::metadata(left)?.len() != fs::metadata(right)?.len() {
        return Ok(false);
    }
    Ok(sha256_file(left)?.0 == sha256_file(right)?.0)
}

fn directory_bytes(path: &Path) -> io::Result<u64> {
    if !path.exists() {
        return Ok(0);
    }
    let mut bytes = 0_u64;
    for entry in walkdir::WalkDir::new(path) {
        let entry = entry?;
        if entry.file_type().is_file() {
            bytes += entry.metadata()?.len();
        }
    }
    Ok(bytes)
}

fn evict_directory(directory: &Path, limit: u64, protected: Option<&Path>) -> io::Result<u64> {
    let protected = protected.and_then(|path| fs::canonicalize(path).ok());
    let mut files = Vec::new();
    let mut total = 0_u64;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if !metadata.is_file() {
            continue;
        }
        total = total.saturating_add(metadata.len());
        files.push((
            metadata.modified().unwrap_or(std::time::UNIX_EPOCH),
            metadata.len(),
            entry.path(),
        ));
    }
    files.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.2.cmp(&right.2)));
    let mut removed = 0_u64;
    for (_, bytes, path) in files {
        if total <= limit {
            break;
        }
        if protected
            .as_ref()
            .is_some_and(|protected| fs::canonicalize(&path).ok().as_ref() == Some(protected))
        {
            continue;
        }
        fs::remove_file(path)?;
        total = total.saturating_sub(bytes);
        removed = removed.saturating_add(bytes);
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[cfg(unix)]
    #[test]
    fn state_rejects_symlinked_receipt_and_uninstall_directories() {
        use std::os::unix::fs::symlink;

        for directory_name in ["receipts", "uninstalled"] {
            let temporary = tempfile::tempdir().expect("tempdir");
            let state_root = temporary.path().join("extension-manager");
            let outside = temporary.path().join("outside");
            fs::create_dir_all(&state_root).expect("state root");
            fs::create_dir_all(&outside).expect("outside directory");
            symlink(&outside, state_root.join(directory_name)).expect("state directory symlink");

            let error = State::new(temporary.path()).expect_err("symlinked state directory");

            assert_eq!(error.code, "INVALID_STATE");
            assert!(error.message.contains("must be a real directory"));
            assert!(fs::read_dir(&outside)
                .expect("outside contents")
                .next()
                .is_none());
        }
    }

    #[test]
    fn startup_reconciles_receipt_cleanup_after_a_committed_move() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let state = State::new(temporary.path()).expect("state");
        let active_receipt = state
            .write_receipt(&receipt("sample", "1.0.0"))
            .expect("active receipt");
        let quarantine = state.root().join("uninstalled/extension-crash");
        fs::create_dir_all(quarantine.join("extension")).expect("recovery extension");
        atomic_copy(&active_receipt, &quarantine.join("receipt.json")).expect("archived receipt");
        let record = UninstallRecord {
            schema_version: 1,
            name: "sample".to_owned(),
            version: "1.0.0".to_owned(),
            original_path: temporary.path().join("extensions/sample"),
            phase: "prepared".to_owned(),
            uninstalled_at: Utc::now(),
        };
        atomic_write(
            &quarantine.join("uninstall.json"),
            &serde_json::to_vec_pretty(&record).expect("record JSON"),
        )
        .expect("uninstall record");

        let restarted = State::new(temporary.path()).expect("restarted state");

        assert!(restarted
            .read_receipt("sample")
            .expect("read receipt")
            .is_none());
        assert!(quarantine.join("receipt.json").is_file());
        assert!(quarantine.join("extension").is_dir());
    }

    #[test]
    fn startup_does_not_remove_a_replaced_same_version_receipt() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let state = State::new(temporary.path()).expect("state");
        let archived_receipt = state
            .write_receipt(&receipt("sample", "1.0.0"))
            .expect("active receipt");
        let quarantine = state.root().join("uninstalled/extension-crash");
        fs::create_dir_all(quarantine.join("extension")).expect("recovery extension");
        atomic_copy(&archived_receipt, &quarantine.join("receipt.json")).expect("archived receipt");
        let record = UninstallRecord {
            schema_version: 1,
            name: "sample".to_owned(),
            version: "1.0.0".to_owned(),
            original_path: temporary.path().join("extensions/sample"),
            phase: "committed".to_owned(),
            uninstalled_at: Utc::now(),
        };
        atomic_write(
            &quarantine.join("uninstall.json"),
            &serde_json::to_vec_pretty(&record).expect("record JSON"),
        )
        .expect("uninstall record");
        let mut replacement = receipt("sample", "1.0.0");
        replacement.source = serde_json::json!({"kind":"local-folder"});
        state
            .write_receipt(&replacement)
            .expect("replacement receipt");

        let restarted = State::new(temporary.path()).expect("restarted state");

        assert_eq!(
            restarted
                .read_receipt("sample")
                .expect("read receipt")
                .expect("preserved replacement")
                .source,
            serde_json::json!({"kind":"local-folder"})
        );
    }

    #[test]
    fn staging_repairs_corrupted_content_addressed_destinations() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let state = State::new(temporary.path()).expect("state");
        let source = temporary.path().join("source.zip");
        let expected = b"verified package";
        fs::write(&source, expected).expect("source");
        let hash = sha256_bytes(expected);
        let destination = state
            .root()
            .join("staging")
            .join(format!("{hash}.aseprite-extension"));
        fs::write(&destination, b"tampered").expect("corrupt staged file");

        let (staged, staged_hash, staged_length) = state.stage_file(&source).expect("stage file");
        assert_eq!(staged, destination);
        assert_eq!(staged_hash, hash);
        assert_eq!(staged_length, expected.len() as u64);
        assert_eq!(fs::read(&staged).expect("staged bytes"), expected);

        fs::write(&destination, b"tampered again").expect("corrupt staged bytes");
        let (staged, staged_hash) = state.stage_bytes(expected).expect("stage bytes");
        assert_eq!(staged, destination);
        assert_eq!(staged_hash, hash);
        assert_eq!(fs::read(staged).expect("restaged bytes"), expected);
    }

    #[test]
    fn cache_rotates_current_to_previous() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let state = State::new(temporary.path()).expect("state");
        let first = temporary.path().join("first.zip");
        let second = temporary.path().join("second.zip");
        fs::write(&first, b"first").expect("first");
        fs::write(&second, b"second").expect("second");

        state.cache_artifact("sample", &first).expect("cache first");
        state
            .cache_artifact("sample", &second)
            .expect("cache second");

        assert_eq!(
            fs::read(
                state
                    .cached_artifact("sample", false)
                    .expect("path")
                    .expect("current")
            )
            .expect("read"),
            b"second"
        );
        assert_eq!(
            fs::read(
                state
                    .cached_artifact("sample", true)
                    .expect("path")
                    .expect("previous")
            )
            .expect("read"),
            b"first"
        );
    }

    #[test]
    fn self_update_transaction_is_durable_exclusive_and_clearable() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let state = State::new(temporary.path()).expect("state");
        let candidate = temporary.path().join("candidate.zip");
        let recovery = temporary.path().join("recovery.zip");
        fs::write(&candidate, b"candidate").expect("candidate");
        fs::write(&recovery, b"recovery").expect("recovery");
        let pending = PendingSelfUpdate {
            schema_version: 1,
            target_version: "0.2.0".to_owned(),
            previous_version: "0.1.0".to_owned(),
            target_is_recovery: false,
            source: serde_json::json!({ "kind": "github-release", "release": "v0.2.0" }),
            previous_source: Some(serde_json::json!({ "kind": "self-recovery" })),
            artifact_sha256: sha256_bytes(b"candidate"),
            artifact_byte_length: 9,
            recovery_sha256: sha256_bytes(b"recovery"),
            recovery_byte_length: 8,
            prepared_at: Utc::now(),
        };

        let (candidate_path, recovery_path) = state
            .begin_self_update(&candidate, &recovery, &pending)
            .expect("begin transaction");
        assert_eq!(
            fs::read(candidate_path).expect("candidate copy"),
            b"candidate"
        );
        assert_eq!(fs::read(recovery_path).expect("recovery copy"), b"recovery");
        let stored = state
            .pending_self_update()
            .expect("pending state")
            .expect("pending transaction");
        assert_eq!(stored.target_version, "0.2.0");
        assert_eq!(stored.previous_version, "0.1.0");
        assert!(!stored.target_is_recovery);

        let mut legacy = serde_json::to_value(&pending).expect("legacy pending JSON");
        legacy
            .as_object_mut()
            .expect("pending object")
            .remove("targetIsRecovery");
        let legacy: PendingSelfUpdate =
            serde_json::from_value(legacy).expect("schema-1 pending journal");
        assert!(!legacy.target_is_recovery);
        assert_eq!(
            state
                .begin_self_update(&candidate, &recovery, &pending)
                .expect_err("second transaction")
                .code,
            "SELF_UPDATE_PENDING"
        );

        state.clear_self_update().expect("clear transaction");
        assert!(state
            .pending_self_update()
            .expect("cleared pending state")
            .is_none());
        assert!(state.self_update_artifact(false).is_none());
        assert!(state.self_update_artifact(true).is_none());

        let mut mismatched = pending;
        mismatched.artifact_sha256 = sha256_bytes(b"different");
        assert_eq!(
            state
                .begin_self_update(&candidate, &recovery, &mismatched)
                .expect_err("copied artifact hash")
                .code,
            "SELF_UPDATE_ARTIFACT_CHANGED"
        );
        assert!(state
            .pending_self_update()
            .expect("no pending state")
            .is_none());
        assert!(state.self_update_artifact(false).is_none());
        assert!(state.self_update_artifact(true).is_none());
    }

    #[test]
    fn rejects_unsafe_package_ids() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let state = State::new(temporary.path()).expect("state");
        for name in [
            "../escape",
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
            "LPT9.x",
            "name.",
        ] {
            assert!(
                state.cached_artifact(name, false).is_err(),
                "unsafe package name should fail: {name}"
            );
        }
        for name in [
            "valid.package-name",
            "console",
            "COM0",
            "COM10",
            "LPT0",
            "LPT10",
        ] {
            assert!(
                state.cached_artifact(name, false).is_ok(),
                "non-device package name should remain valid: {name}"
            );
        }
    }

    #[test]
    fn transient_cache_evicts_oldest_files_and_keeps_protected_file() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let directory = temporary.path();
        let old = directory.join("old");
        let protected = directory.join("protected");
        let newest = directory.join("newest");
        fs::write(&old, vec![0; 8]).expect("old");
        fs::write(&protected, vec![1; 8]).expect("protected");
        fs::write(&newest, vec![2; 8]).expect("newest");
        filetime::set_file_mtime(&old, filetime::FileTime::from_unix_time(1, 0)).expect("old time");
        filetime::set_file_mtime(&protected, filetime::FileTime::from_unix_time(2, 0))
            .expect("protected time");
        filetime::set_file_mtime(&newest, filetime::FileTime::from_unix_time(3, 0))
            .expect("new time");

        evict_directory(directory, 10, Some(&protected)).expect("evict");

        assert!(!old.exists());
        assert!(protected.exists());
        assert!(!newest.exists());
    }

    #[test]
    fn rotates_local_log_without_recording_remote_state() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let state = State::new(temporary.path()).expect("state");
        let path = state.root().join("logs/helper.log");
        fs::write(&path, vec![b'x'; LOG_FILE_LIMIT as usize]).expect("large log");
        drop(state.open_rotated_log().expect("open"));
        assert!(state.root().join("logs/helper.log.1").exists());
        assert!(path.exists());
        assert_eq!(fs::metadata(path).expect("metadata").len(), 0);
    }
}
