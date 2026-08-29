use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::protocol::{RpcError, RpcResult};
use crate::state::{atomic_write, State};

const MAX_ROOT_ROTATIONS: u64 = 32;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryView {
    pub status: String,
    pub packages: Vec<CatalogPackage>,
    pub expired: bool,
    pub from_cache: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Catalog {
    pub schema_version: u32,
    pub generated_at: DateTime<Utc>,
    pub packages: Vec<CatalogPackage>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogPackage {
    pub id: String,
    pub manifest_name: String,
    pub display_name: String,
    #[serde(default)]
    pub summary: String,
    pub author: Value,
    pub license: String,
    pub homepage: String,
    pub repository: String,
    pub releases: Vec<CatalogRelease>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogRelease {
    pub version: String,
    pub aseprite: AsepriteCompatibility,
    pub asset: CatalogAsset,
    pub published_at: DateTime<Utc>,
    #[serde(default)]
    pub release_notes: String,
    #[serde(default)]
    pub yanked: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AsepriteCompatibility {
    pub minimum_version: String,
    #[serde(default)]
    pub maximum_version: Option<String>,
    pub minimum_api: u32,
    #[serde(default)]
    pub maximum_api: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogAsset {
    pub url: String,
    pub sha256: String,
    pub byte_length: u64,
    #[serde(default)]
    pub release_tag: Option<String>,
    #[serde(default)]
    pub asset_id: Option<u64>,
    #[serde(default)]
    pub commit: Option<String>,
}

#[derive(Clone)]
pub struct RegistryClient {
    state: State,
    repository: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
struct Envelope {
    signed: Value,
    signatures: Vec<EnvelopeSignature>,
}

#[derive(Clone, Debug, Deserialize)]
struct EnvelopeSignature {
    keyid: String,
    sig: String,
}

#[derive(Clone, Debug, Deserialize)]
struct RootSigned {
    #[serde(rename = "_type")]
    type_name: String,
    spec_version: String,
    version: u64,
    expires: DateTime<Utc>,
    consistent_snapshot: bool,
    keys: BTreeMap<String, RootKey>,
    roles: BTreeMap<String, Role>,
}

#[derive(Clone, Debug, Deserialize)]
struct RootKey {
    keytype: String,
    scheme: String,
    keyval: RootKeyValue,
}

#[derive(Clone, Debug, Deserialize)]
struct RootKeyValue {
    public: String,
}

#[derive(Clone, Debug, Deserialize)]
struct Role {
    keyids: Vec<String>,
    threshold: u32,
}

#[derive(Clone, Debug, Deserialize)]
struct TimestampSigned {
    #[serde(rename = "_type")]
    type_name: String,
    spec_version: String,
    version: u64,
    expires: DateTime<Utc>,
    meta: BTreeMap<String, MetadataDescription>,
}

#[derive(Clone, Debug, Deserialize)]
struct SnapshotSigned {
    #[serde(rename = "_type")]
    type_name: String,
    spec_version: String,
    version: u64,
    expires: DateTime<Utc>,
    meta: BTreeMap<String, MetadataDescription>,
}

#[derive(Clone, Debug, Deserialize)]
struct TargetsSigned {
    #[serde(rename = "_type")]
    type_name: String,
    spec_version: String,
    version: u64,
    expires: DateTime<Utc>,
    targets: BTreeMap<String, TargetDescription>,
}

#[derive(Clone, Debug, Deserialize)]
struct MetadataDescription {
    version: u64,
    #[serde(default)]
    length: Option<u64>,
    #[serde(default)]
    hashes: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize)]
struct TargetDescription {
    length: u64,
    hashes: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct TrustedVersions {
    root: u64,
    timestamp: u64,
    snapshot: u64,
    targets: u64,
}

#[derive(Debug)]
struct VerifiedRepository {
    root_bytes: Vec<u8>,
    timestamp_bytes: Vec<u8>,
    snapshot_bytes: Vec<u8>,
    targets_bytes: Vec<u8>,
    catalog_bytes: Vec<u8>,
    versions: TrustedVersions,
    catalog: Catalog,
}

impl RegistryClient {
    pub fn new(state: State, extension_root: &Path) -> Self {
        Self {
            state,
            repository: extension_root.join("registry"),
        }
    }

    pub fn refresh(&self, now: DateTime<Utc>) -> RpcResult<RegistryView> {
        match self.verify_repository(&self.repository, now) {
            Ok(repository) => {
                self.persist(&repository)?;
                Ok(RegistryView {
                    status: if repository.catalog.packages.is_empty() {
                        "empty".to_owned()
                    } else {
                        "ready".to_owned()
                    },
                    packages: repository.catalog.packages,
                    expired: false,
                    from_cache: false,
                })
            }
            Err(error) => {
                match self.verify_repository(&self.state.tuf_path("last-known-good"), now) {
                    Ok(repository) => Ok(RegistryView {
                        status: if repository.catalog.packages.is_empty() {
                            "empty".to_owned()
                        } else {
                            "cached".to_owned()
                        },
                        packages: repository.catalog.packages,
                        expired: false,
                        from_cache: true,
                    }),
                    Err(cache_error)
                        if error.code == "TUF_EXPIRED" || cache_error.code == "TUF_EXPIRED" =>
                    {
                        let catalog = self.read_cached_catalog()?;
                        Ok(RegistryView {
                            status: "expired".to_owned(),
                            packages: catalog.map(|value| value.packages).unwrap_or_default(),
                            expired: true,
                            from_cache: true,
                        })
                    }
                    Err(_) => Err(error),
                }
            }
        }
    }

    fn verify_repository(
        &self,
        repository: &Path,
        now: DateTime<Utc>,
    ) -> RpcResult<VerifiedRepository> {
        let pinned_path = repository.join("root.json");
        let mut root_bytes = fs::read(&pinned_path).map_err(RpcError::io)?;
        let mut root_envelope = parse_envelope(&root_bytes, "root")?;
        let mut root: RootSigned = parse_signed(&root_envelope)?;
        validate_root_shape(&root)?;
        verify_role(&root_envelope, &root, "root")?;

        for next_version in (root.version + 1)..=(root.version + MAX_ROOT_ROTATIONS) {
            let candidate_path = repository
                .join("metadata")
                .join(format!("{next_version}.root.json"));
            if !candidate_path.exists() {
                break;
            }
            let candidate_bytes = fs::read(&candidate_path).map_err(RpcError::io)?;
            let candidate_envelope = parse_envelope(&candidate_bytes, "root")?;
            let candidate: RootSigned = parse_signed(&candidate_envelope)?;
            if candidate.version != next_version {
                return Err(tuf_error(
                    "TUF_ROOT_ROTATION",
                    "root metadata versions must advance one at a time",
                ));
            }
            verify_role(&candidate_envelope, &root, "root")?;
            verify_role(&candidate_envelope, &candidate, "root")?;
            validate_root_shape(&candidate)?;
            root_bytes = candidate_bytes;
            root_envelope = candidate_envelope;
            root = candidate;
        }
        let _ = root_envelope;
        if root.expires <= now {
            return Err(tuf_error(
                "TUF_EXPIRED",
                "trusted root metadata has expired",
            ));
        }

        let trusted = self.read_trusted_versions()?;
        if root.version < trusted.root {
            return Err(tuf_error(
                "TUF_ROLLBACK",
                "root metadata version rolled back",
            ));
        }

        let timestamp_bytes =
            fs::read(repository.join("metadata").join("timestamp.json")).map_err(RpcError::io)?;
        let timestamp_envelope = parse_envelope(&timestamp_bytes, "timestamp")?;
        verify_role(&timestamp_envelope, &root, "timestamp")?;
        let timestamp: TimestampSigned = parse_signed(&timestamp_envelope)?;
        validate_metadata_header(
            &timestamp.type_name,
            &timestamp.spec_version,
            "timestamp",
            timestamp.version,
            timestamp.expires,
            trusted.timestamp,
            now,
        )?;
        let snapshot_description = timestamp.meta.get("snapshot.json").ok_or_else(|| {
            tuf_error(
                "TUF_INVALID_METADATA",
                "timestamp metadata does not describe snapshot.json",
            )
        })?;
        if snapshot_description.version < trusted.snapshot {
            return Err(tuf_error(
                "TUF_ROLLBACK",
                "timestamp references an older snapshot",
            ));
        }
        let snapshot_path = metadata_path(
            repository,
            "snapshot",
            snapshot_description.version,
            root.consistent_snapshot,
        );
        let snapshot_bytes = fs::read(snapshot_path).map_err(RpcError::io)?;
        verify_description(&snapshot_bytes, snapshot_description, "snapshot.json")?;
        let snapshot_envelope = parse_envelope(&snapshot_bytes, "snapshot")?;
        verify_role(&snapshot_envelope, &root, "snapshot")?;
        let snapshot: SnapshotSigned = parse_signed(&snapshot_envelope)?;
        validate_metadata_header(
            &snapshot.type_name,
            &snapshot.spec_version,
            "snapshot",
            snapshot.version,
            snapshot.expires,
            trusted.snapshot,
            now,
        )?;
        if snapshot.version != snapshot_description.version {
            return Err(tuf_error(
                "TUF_VERSION_MISMATCH",
                "snapshot version differs from timestamp metadata",
            ));
        }

        let targets_description = snapshot.meta.get("targets.json").ok_or_else(|| {
            tuf_error(
                "TUF_INVALID_METADATA",
                "snapshot metadata does not describe targets.json",
            )
        })?;
        if targets_description.version < trusted.targets {
            return Err(tuf_error(
                "TUF_ROLLBACK",
                "snapshot references older targets metadata",
            ));
        }
        let targets_path = metadata_path(
            repository,
            "targets",
            targets_description.version,
            root.consistent_snapshot,
        );
        let targets_bytes = fs::read(targets_path).map_err(RpcError::io)?;
        verify_description(&targets_bytes, targets_description, "targets.json")?;
        let targets_envelope = parse_envelope(&targets_bytes, "targets")?;
        verify_role(&targets_envelope, &root, "targets")?;
        let targets: TargetsSigned = parse_signed(&targets_envelope)?;
        validate_metadata_header(
            &targets.type_name,
            &targets.spec_version,
            "targets",
            targets.version,
            targets.expires,
            trusted.targets,
            now,
        )?;
        if targets.version != targets_description.version {
            return Err(tuf_error(
                "TUF_VERSION_MISMATCH",
                "targets version differs from snapshot metadata",
            ));
        }

        let catalog_description = targets.targets.get("catalog-v1.json").ok_or_else(|| {
            tuf_error(
                "TUF_TARGET_MISSING",
                "targets metadata does not contain catalog-v1.json",
            )
        })?;
        let catalog_path = target_path(
            repository,
            "catalog-v1.json",
            catalog_description,
            root.consistent_snapshot,
        )?;
        let catalog_bytes = fs::read(catalog_path).map_err(RpcError::io)?;
        verify_target(&catalog_bytes, catalog_description, "catalog-v1.json")?;
        let catalog: Catalog = serde_json::from_slice(&catalog_bytes)
            .map_err(|error| tuf_error("CATALOG_INVALID", error.to_string()))?;
        validate_catalog(&catalog)?;

        Ok(VerifiedRepository {
            root_bytes,
            timestamp_bytes,
            snapshot_bytes,
            targets_bytes,
            catalog_bytes,
            versions: TrustedVersions {
                root: root.version,
                timestamp: timestamp.version,
                snapshot: snapshot.version,
                targets: targets.version,
            },
            catalog,
        })
    }

    fn persist(&self, repository: &VerifiedRepository) -> RpcResult<()> {
        let destination = self.state.tuf_path("last-known-good");
        for directory in [
            destination.clone(),
            destination.join("metadata"),
            destination.join("targets"),
        ] {
            fs::create_dir_all(directory).map_err(RpcError::io)?;
        }
        atomic_write(&destination.join("root.json"), &repository.root_bytes)
            .map_err(RpcError::io)?;
        atomic_write(
            &destination.join("metadata").join("timestamp.json"),
            &repository.timestamp_bytes,
        )
        .map_err(RpcError::io)?;
        atomic_write(
            &destination
                .join("metadata")
                .join(format!("{}.snapshot.json", repository.versions.snapshot)),
            &repository.snapshot_bytes,
        )
        .map_err(RpcError::io)?;
        atomic_write(
            &destination
                .join("metadata")
                .join(format!("{}.targets.json", repository.versions.targets)),
            &repository.targets_bytes,
        )
        .map_err(RpcError::io)?;
        let catalog_hash = hex::encode(Sha256::digest(&repository.catalog_bytes));
        atomic_write(
            &destination
                .join("targets")
                .join(format!("{catalog_hash}.catalog-v1.json")),
            &repository.catalog_bytes,
        )
        .map_err(RpcError::io)?;
        atomic_write(
            &self.state.tuf_path("catalog-v1.json"),
            &repository.catalog_bytes,
        )
        .map_err(RpcError::io)?;
        let versions = serde_json::to_vec_pretty(&repository.versions)
            .map_err(|error| RpcError::internal(error.to_string()))?;
        atomic_write(&self.state.tuf_path("trusted-versions.json"), &versions)
            .map_err(RpcError::io)?;
        Ok(())
    }

    fn read_trusted_versions(&self) -> RpcResult<TrustedVersions> {
        let path = self.state.tuf_path("trusted-versions.json");
        if !path.exists() {
            return Ok(TrustedVersions::default());
        }
        serde_json::from_slice(&fs::read(path).map_err(RpcError::io)?)
            .map_err(|error| RpcError::state(format!("invalid trusted versions: {error}")))
    }

    fn read_cached_catalog(&self) -> RpcResult<Option<Catalog>> {
        let path = self.state.tuf_path("catalog-v1.json");
        if !path.exists() {
            return Ok(None);
        }
        serde_json::from_slice(&fs::read(path).map_err(RpcError::io)?)
            .map(Some)
            .map_err(|error| RpcError::state(format!("invalid cached catalog: {error}")))
    }
}

fn parse_envelope(bytes: &[u8], role: &str) -> RpcResult<Envelope> {
    serde_json::from_slice(bytes)
        .map_err(|error| tuf_error("TUF_INVALID_METADATA", format!("{role}: {error}")))
}

fn parse_signed<T>(envelope: &Envelope) -> RpcResult<T>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_value(envelope.signed.clone())
        .map_err(|error| tuf_error("TUF_INVALID_METADATA", error.to_string()))
}

fn verify_role(envelope: &Envelope, root: &RootSigned, role_name: &str) -> RpcResult<()> {
    let role = root.roles.get(role_name).ok_or_else(|| {
        tuf_error(
            "TUF_INVALID_ROOT",
            format!("root does not define the {role_name} role"),
        )
    })?;
    if role.threshold == 0 || role.threshold as usize > role.keyids.len() {
        return Err(tuf_error(
            "TUF_INVALID_ROOT",
            format!("{role_name} has an invalid signature threshold"),
        ));
    }
    let signed_bytes = canonical_json(&envelope.signed)?;
    let mut valid = BTreeSet::new();
    for signature in &envelope.signatures {
        if !role.keyids.contains(&signature.keyid) || valid.contains(&signature.keyid) {
            continue;
        }
        let key = root.keys.get(&signature.keyid).ok_or_else(|| {
            tuf_error(
                "TUF_INVALID_ROOT",
                format!("role references missing key {}", signature.keyid),
            )
        })?;
        if key.keytype != "ed25519" || key.scheme != "ed25519" {
            continue;
        }
        let public = decode_key(&key.keyval.public)?;
        let public: [u8; 32] = public
            .try_into()
            .map_err(|_| tuf_error("TUF_INVALID_ROOT", "Ed25519 public key must be 32 bytes"))?;
        let verifying_key = VerifyingKey::from_bytes(&public)
            .map_err(|error| tuf_error("TUF_INVALID_ROOT", error.to_string()))?;
        let key_id = signature.keyid.clone();
        let signature_bytes = decode_key(&signature.sig)?;
        let verified_signature = Signature::from_slice(&signature_bytes)
            .map_err(|error| tuf_error("TUF_BAD_SIGNATURE", error.to_string()))?;
        if verifying_key
            .verify(&signed_bytes, &verified_signature)
            .is_ok()
        {
            valid.insert(key_id);
        }
    }
    if valid.len() < role.threshold as usize {
        return Err(tuf_error(
            "TUF_BAD_SIGNATURE",
            format!("{role_name} metadata did not meet its signature threshold"),
        ));
    }
    Ok(())
}

fn validate_root_shape(root: &RootSigned) -> RpcResult<()> {
    if root.type_name != "root"
        || root.version == 0
        || !root.spec_version.starts_with("1.")
        || root
            .roles
            .keys()
            .any(|role| !matches!(role.as_str(), "root" | "targets" | "snapshot" | "timestamp"))
    {
        return Err(tuf_error(
            "TUF_INVALID_ROOT",
            "root metadata is not compatible with TUF 1.0",
        ));
    }
    for role in ["root", "targets", "snapshot", "timestamp"] {
        if !root.roles.contains_key(role) {
            return Err(tuf_error(
                "TUF_INVALID_ROOT",
                format!("root metadata is missing the {role} role"),
            ));
        }
    }
    Ok(())
}

fn validate_metadata_header(
    actual_type: &str,
    spec_version: &str,
    expected_type: &str,
    version: u64,
    expires: DateTime<Utc>,
    minimum_version: u64,
    now: DateTime<Utc>,
) -> RpcResult<()> {
    if actual_type != expected_type || !spec_version.starts_with("1.") || version == 0 {
        return Err(tuf_error(
            "TUF_INVALID_METADATA",
            format!("invalid {expected_type} metadata header"),
        ));
    }
    if version < minimum_version {
        return Err(tuf_error(
            "TUF_ROLLBACK",
            format!("{expected_type} metadata version rolled back"),
        ));
    }
    if expires <= now {
        return Err(tuf_error(
            "TUF_EXPIRED",
            format!("{expected_type} metadata has expired"),
        ));
    }
    Ok(())
}

fn verify_description(
    bytes: &[u8],
    description: &MetadataDescription,
    name: &str,
) -> RpcResult<()> {
    if let Some(length) = description.length {
        if bytes.len() as u64 != length {
            return Err(tuf_error(
                "TUF_LENGTH_MISMATCH",
                format!("{name} length does not match authenticated metadata"),
            ));
        }
    }
    verify_hashes(bytes, &description.hashes, name)
}

fn verify_target(bytes: &[u8], description: &TargetDescription, name: &str) -> RpcResult<()> {
    if bytes.len() as u64 != description.length {
        return Err(tuf_error(
            "TUF_LENGTH_MISMATCH",
            format!("{name} length does not match authenticated metadata"),
        ));
    }
    verify_hashes(bytes, &description.hashes, name)
}

fn verify_hashes(bytes: &[u8], hashes: &BTreeMap<String, String>, name: &str) -> RpcResult<()> {
    let expected = hashes.get("sha256").ok_or_else(|| {
        tuf_error(
            "TUF_HASH_MISSING",
            format!("{name} does not have an authenticated SHA-256"),
        )
    })?;
    let actual = hex::encode(Sha256::digest(bytes));
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(tuf_error(
            "TUF_HASH_MISMATCH",
            format!("{name} failed authenticated hash verification"),
        ));
    }
    Ok(())
}

fn metadata_path(repository: &Path, role: &str, version: u64, consistent: bool) -> PathBuf {
    if consistent {
        repository
            .join("metadata")
            .join(format!("{version}.{role}.json"))
    } else {
        repository.join("metadata").join(format!("{role}.json"))
    }
}

fn target_path(
    repository: &Path,
    name: &str,
    description: &TargetDescription,
    consistent: bool,
) -> RpcResult<PathBuf> {
    if consistent {
        let hash = description.hashes.get("sha256").ok_or_else(|| {
            tuf_error(
                "TUF_HASH_MISSING",
                "consistent target does not have a SHA-256 hash",
            )
        })?;
        Ok(repository.join("targets").join(format!("{hash}.{name}")))
    } else {
        Ok(repository.join("targets").join(name))
    }
}

fn validate_catalog(catalog: &Catalog) -> RpcResult<()> {
    if catalog.schema_version != 1 {
        return Err(tuf_error(
            "CATALOG_SCHEMA_UNSUPPORTED",
            "catalog schema version is unsupported",
        ));
    }
    let mut ids = BTreeSet::new();
    for package in &catalog.packages {
        if package.id != package.manifest_name.to_lowercase()
            || package.id.is_empty()
            || !package
                .id
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "-._".contains(character))
            || !ids.insert(package.id.clone())
        {
            return Err(tuf_error(
                "CATALOG_INVALID",
                "catalog package IDs must be unique case-folded manifest names",
            ));
        }
        if package.releases.is_empty() {
            return Err(tuf_error(
                "CATALOG_INVALID",
                format!("catalog package {} has no releases", package.id),
            ));
        }
        let mut versions = BTreeSet::new();
        for release in &package.releases {
            Version::parse(&release.version).map_err(|_| {
                tuf_error(
                    "CATALOG_INVALID",
                    format!(
                        "catalog release {} {} is not MAJOR.MINOR.PATCH",
                        package.id, release.version
                    ),
                )
            })?;
            if release.version.contains('-') || release.version.contains('+') {
                return Err(tuf_error(
                    "CATALOG_INVALID",
                    "registry releases must use plain MAJOR.MINOR.PATCH versions",
                ));
            }
            if release.asset.sha256.len() != 64
                || !release
                    .asset
                    .sha256
                    .chars()
                    .all(|character| character.is_ascii_hexdigit())
                || release.asset.byte_length == 0
                || !versions.insert(release.version.clone())
            {
                return Err(tuf_error(
                    "CATALOG_INVALID",
                    format!("catalog release metadata is invalid for {}", package.id),
                ));
            }
            if parse_aseprite_version(&release.aseprite.minimum_version).is_none()
                || release
                    .aseprite
                    .maximum_version
                    .as_deref()
                    .is_some_and(|value| parse_aseprite_version(value).is_none())
                || release.aseprite.minimum_api == 0
                || release
                    .aseprite
                    .maximum_api
                    .is_some_and(|maximum| maximum < release.aseprite.minimum_api)
            {
                return Err(tuf_error(
                    "CATALOG_INVALID",
                    "catalog Aseprite compatibility range is invalid",
                ));
            }
            let url = url::Url::parse(&release.asset.url)
                .map_err(|_| tuf_error("CATALOG_INVALID", "catalog asset URL is invalid"))?;
            if url.scheme() != "https" {
                return Err(tuf_error(
                    "CATALOG_INVALID",
                    "catalog asset URLs must use HTTPS",
                ));
            }
        }
    }
    Ok(())
}

fn decode_key(value: &str) -> RpcResult<Vec<u8>> {
    hex::decode(value)
        .or_else(|_| {
            base64::engine::general_purpose::STANDARD
                .decode(value)
                .map_err(|_| hex::FromHexError::InvalidStringLength)
        })
        .map_err(|_| tuf_error("TUF_INVALID_KEY", "key material is not valid hex or base64"))
}

fn canonical_json(value: &Value) -> RpcResult<Vec<u8>> {
    fn write_value(output: &mut String, value: &Value) -> RpcResult<()> {
        match value {
            Value::Null => output.push_str("null"),
            Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
            Value::Number(value) => output.push_str(&value.to_string()),
            Value::String(value) => output.push_str(
                &serde_json::to_string(value)
                    .map_err(|error| tuf_error("TUF_CANONICAL_JSON", error.to_string()))?,
            ),
            Value::Array(values) => {
                output.push('[');
                for (index, value) in values.iter().enumerate() {
                    if index != 0 {
                        output.push(',');
                    }
                    write_value(output, value)?;
                }
                output.push(']');
            }
            Value::Object(values) => {
                output.push('{');
                let mut keys: Vec<_> = values.keys().collect();
                keys.sort();
                for (index, key) in keys.into_iter().enumerate() {
                    if index != 0 {
                        output.push(',');
                    }
                    output.push_str(
                        &serde_json::to_string(key)
                            .map_err(|error| tuf_error("TUF_CANONICAL_JSON", error.to_string()))?,
                    );
                    output.push(':');
                    write_value(output, &values[key])?;
                }
                output.push('}');
            }
        }
        Ok(())
    }
    let mut output = String::new();
    write_value(&mut output, value)?;
    Ok(output.into_bytes())
}

fn tuf_error(code: impl Into<String>, message: impl Into<String>) -> RpcError {
    RpcError::new(code, message, false)
}

pub fn available_updates(
    catalog: &[CatalogPackage],
    installed: &[(String, String)],
    aseprite_version: &str,
    api_version: u32,
) -> BTreeMap<String, Value> {
    let mut updates = BTreeMap::new();
    for (name, current) in installed {
        let Some(package) = catalog
            .iter()
            .find(|package| package.id == name.to_lowercase())
        else {
            continue;
        };
        let Ok(current_version) = Version::parse(current) else {
            continue;
        };
        let newest = package
            .releases
            .iter()
            .filter(|release| {
                !release.yanked && release_supports(release, aseprite_version, api_version)
            })
            .filter_map(|release| {
                Version::parse(&release.version)
                    .ok()
                    .map(|version| (version, release))
            })
            .filter(|(version, _)| version > &current_version)
            .max_by(|left, right| left.0.cmp(&right.0));
        if let Some((_, release)) = newest {
            if let Ok(value) = serde_json::to_value(release) {
                updates.insert(name.to_lowercase(), value);
            }
        }
    }
    updates
}

pub fn release_supports(
    release: &CatalogRelease,
    aseprite_version: &str,
    api_version: u32,
) -> bool {
    let Some(current) = parse_host_aseprite_version(aseprite_version) else {
        return false;
    };
    let Some(minimum) = parse_aseprite_version(&release.aseprite.minimum_version) else {
        return false;
    };
    if compare_version_parts(&current, &minimum).is_lt()
        || api_version < release.aseprite.minimum_api
    {
        return false;
    }
    if let Some(maximum) = release
        .aseprite
        .maximum_version
        .as_deref()
        .and_then(parse_aseprite_version)
    {
        if compare_version_parts(&current, &maximum).is_gt() {
            return false;
        }
    }
    if release
        .aseprite
        .maximum_api
        .is_some_and(|maximum| api_version > maximum)
    {
        return false;
    }
    true
}

fn parse_aseprite_version(value: &str) -> Option<Vec<u32>> {
    let parts: Option<Vec<_>> = value
        .split('.')
        .map(|part| {
            if part.is_empty()
                || (part.len() > 1 && part.starts_with('0'))
                || !part.chars().all(|character| character.is_ascii_digit())
            {
                None
            } else {
                part.parse().ok()
            }
        })
        .collect();
    let parts = parts?;
    (parts.len() == 3 || parts.len() == 4).then_some(parts)
}

fn parse_host_aseprite_version(value: &str) -> Option<Vec<u32>> {
    let numeric = value.split_once('-').map_or(value, |(prefix, suffix)| {
        if !suffix.is_empty()
            && suffix
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "-._".contains(character))
        {
            prefix
        } else {
            value
        }
    });
    parse_aseprite_version(numeric)
}

fn compare_version_parts(left: &[u32], right: &[u32]) -> std::cmp::Ordering {
    for index in 0..left.len().max(right.len()) {
        match left
            .get(index)
            .copied()
            .unwrap_or_default()
            .cmp(&right.get(index).copied().unwrap_or_default())
        {
            std::cmp::Ordering::Equal => {}
            ordering => return ordering,
        }
    }
    std::cmp::Ordering::Equal
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn bundled_repository() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("registry")
            .join("bundled")
    }

    fn copy_directory(source: &Path, destination: &Path) {
        for entry in walkdir::WalkDir::new(source) {
            let entry = entry.unwrap();
            let relative = entry.path().strip_prefix(source).unwrap();
            let target = destination.join(relative);
            if entry.file_type().is_dir() {
                fs::create_dir_all(target).unwrap();
            } else {
                fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    fn fixture_client(repository: PathBuf, state_root: &Path) -> RegistryClient {
        RegistryClient {
            state: State::new(state_root).unwrap(),
            repository,
        }
    }

    #[test]
    fn canonical_json_sorts_object_keys() {
        let value = serde_json::json!({"z": 1, "a": {"b": true, "a": false}});
        assert_eq!(
            String::from_utf8(canonical_json(&value).unwrap()).unwrap(),
            r#"{"a":{"a":false,"b":true},"z":1}"#
        );
    }

    #[test]
    fn catalog_rejects_non_semantic_versions() {
        let catalog = Catalog {
            schema_version: 1,
            generated_at: Utc::now(),
            packages: vec![CatalogPackage {
                id: "sample".to_owned(),
                manifest_name: "sample".to_owned(),
                display_name: "Sample".to_owned(),
                summary: String::new(),
                author: serde_json::json!({"name":"Author"}),
                license: "MIT".to_owned(),
                homepage: "https://example.com".to_owned(),
                repository: "https://github.com/example/sample".to_owned(),
                releases: vec![CatalogRelease {
                    version: "latest".to_owned(),
                    aseprite: AsepriteCompatibility {
                        minimum_version: "1.3.15".to_owned(),
                        maximum_version: None,
                        minimum_api: 35,
                        maximum_api: None,
                    },
                    asset: CatalogAsset {
                        url: "https://github.com/example/sample/file".to_owned(),
                        sha256: "0".repeat(64),
                        byte_length: 1,
                        release_tag: None,
                        asset_id: None,
                        commit: None,
                    },
                    published_at: Utc::now(),
                    release_notes: String::new(),
                    yanked: false,
                }],
            }],
        };
        assert!(validate_catalog(&catalog).is_err());
    }

    #[test]
    fn parses_committed_catalog_schema_and_enforces_compatibility() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("registry")
            .join("fixtures")
            .join("catalog-v1-valid.json");
        let catalog: Catalog = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        validate_catalog(&catalog).unwrap();
        let release = &catalog.packages[0].releases[0];
        assert!(release_supports(release, "1.3.18.1", 35));
        assert!(release_supports(release, "1.3.18.1-arm64", 35));
        assert!(!release_supports(release, "1.3.14", 35));
        assert!(!release_supports(release, "1.3.18.1", 34));
        assert_eq!(release.asset.asset_id, Some(12345));
    }

    #[test]
    fn update_listing_ignores_opaque_installed_versions() {
        let releases = vec![CatalogRelease {
            version: "2.0.0".to_owned(),
            aseprite: AsepriteCompatibility {
                minimum_version: "1.3.15".to_owned(),
                maximum_version: None,
                minimum_api: 35,
                maximum_api: None,
            },
            asset: CatalogAsset {
                url: "https://github.com/example/sample/file".to_owned(),
                sha256: "0".repeat(64),
                byte_length: 1,
                release_tag: None,
                asset_id: None,
                commit: None,
            },
            published_at: Utc::now(),
            release_notes: String::new(),
            yanked: false,
        }];
        let package = CatalogPackage {
            id: "sample".to_owned(),
            manifest_name: "sample".to_owned(),
            display_name: "Sample".to_owned(),
            summary: String::new(),
            author: serde_json::json!({"name":"Author"}),
            license: "MIT".to_owned(),
            homepage: "https://example.com".to_owned(),
            repository: "https://github.com/example/sample".to_owned(),
            releases,
        };
        assert!(available_updates(
            std::slice::from_ref(&package),
            &[("sample".to_owned(), "dev".to_owned())],
            "1.3.18.1",
            35
        )
        .is_empty());
        assert!(available_updates(
            &[package],
            &[("sample".to_owned(), "1.0.0".to_owned())],
            "1.3.18.1",
            35
        )
        .contains_key("sample"));
    }

    #[test]
    fn verifies_bundled_tuf_repository_and_rejects_corruption() {
        let temporary = tempfile::tempdir().unwrap();
        let repository = temporary.path().join("repository");
        copy_directory(&bundled_repository(), &repository);
        let client = fixture_client(repository.clone(), &temporary.path().join("state"));
        let now = DateTime::parse_from_rfc3339("2026-07-31T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let verified = client.verify_repository(&repository, now).unwrap();
        assert_eq!(verified.versions.targets, 1);
        assert!(verified.catalog.packages.is_empty());

        let target = fs::read_dir(repository.join("targets"))
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.len() > "catalog-v1.json".len() + 1)
            })
            .unwrap();
        fs::write(target, b"corrupted").unwrap();
        let error = client
            .verify_repository(&repository, now)
            .expect_err("corruption rejected");
        assert!(matches!(
            error.code.as_str(),
            "TUF_LENGTH_MISMATCH" | "TUF_HASH_MISMATCH"
        ));
    }

    #[test]
    fn enforces_expiry_and_version_rollback() {
        let temporary = tempfile::tempdir().unwrap();
        let repository = temporary.path().join("repository");
        copy_directory(&bundled_repository(), &repository);
        let client = fixture_client(repository.clone(), &temporary.path().join("state"));
        let expired = DateTime::parse_from_rfc3339("2031-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            client
                .verify_repository(&repository, expired)
                .expect_err("expired")
                .code,
            "TUF_EXPIRED"
        );

        let versions = TrustedVersions {
            root: 1,
            timestamp: 2,
            snapshot: 2,
            targets: 2,
        };
        atomic_write(
            &client.state.tuf_path("trusted-versions.json"),
            &serde_json::to_vec(&versions).unwrap(),
        )
        .unwrap();
        let now = DateTime::parse_from_rfc3339("2026-07-31T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            client
                .verify_repository(&repository, now)
                .expect_err("rollback")
                .code,
            "TUF_ROLLBACK"
        );
    }

    #[test]
    fn accepts_sequential_dual_verified_root_rotation() {
        let temporary = tempfile::tempdir().unwrap();
        let repository = temporary.path().join("repository");
        copy_directory(&bundled_repository(), &repository);
        let root_bytes = fs::read(repository.join("root.json")).unwrap();
        let mut envelope: Value = serde_json::from_slice(&root_bytes).unwrap();
        envelope["signed"]["version"] = Value::Number(2_u64.into());
        let signed = canonical_json(&envelope["signed"]).unwrap();
        let signing = SigningKey::from_bytes(&[1_u8; 32]);
        let signature = signing.sign(&signed);
        envelope["signatures"][0]["sig"] = Value::String(hex::encode(signature.to_bytes()));
        fs::write(
            repository.join("metadata/2.root.json"),
            serde_json::to_vec(&envelope).unwrap(),
        )
        .unwrap();
        let client = fixture_client(repository.clone(), &temporary.path().join("state"));
        let now = DateTime::parse_from_rfc3339("2026-07-31T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let verified = client.verify_repository(&repository, now).unwrap();
        assert_eq!(verified.versions.root, 2);
    }

    #[test]
    fn falls_back_to_unexpired_last_known_good_metadata() {
        let temporary = tempfile::tempdir().unwrap();
        let repository = temporary.path().join("repository");
        copy_directory(&bundled_repository(), &repository);
        let state_root = temporary.path().join("state");
        let client = fixture_client(repository, &state_root);
        let now = DateTime::parse_from_rfc3339("2026-07-31T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let first = client.refresh(now).unwrap();
        assert!(!first.from_cache);

        let offline = fixture_client(temporary.path().join("offline"), &state_root);
        let cached = offline.refresh(now).unwrap();
        assert!(cached.from_cache);
        assert!(!cached.expired);
    }
}
