use std::{collections::BTreeMap, fs, path::Path};

use anyhow::{bail, ensure, Context, Result};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

const PURPOSE: &str = "test-fixture-only";
const SPEC_VERSION: &str = "1.0.31";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FixtureKey {
    purpose: String,
    scheme: String,
    seed_hex: String,
}

struct RoleKey {
    key_id: String,
    descriptor: Value,
    signing: SigningKey,
}

pub fn generate(
    keys: &Path,
    catalog: &Path,
    output: &Path,
    version: u64,
    expired: bool,
) -> Result<()> {
    ensure!(version > 0, "fixture metadata version must be positive");
    let role_keys = load_keys(keys)?;
    let catalog_bytes = fs::read(catalog).with_context(|| format!("read {}", catalog.display()))?;
    let catalog_value: Value =
        serde_json::from_slice(&catalog_bytes).context("parse catalog fixture")?;
    ensure!(
        catalog_value.get("schemaVersion").and_then(Value::as_u64) == Some(1),
        "catalog fixture schemaVersion must be 1"
    );
    ensure!(
        catalog_value
            .get("packages")
            .and_then(Value::as_array)
            .is_some(),
        "catalog fixture packages must be an array"
    );

    let metadata_expiry = if expired {
        "2020-01-01T00:00:00Z"
    } else {
        "2030-01-01T00:00:00Z"
    };
    let root_signed = root_signed(&role_keys, version);
    let root = signed_envelope(
        root_signed,
        role_keys.get("root").context("missing root fixture key")?,
    )?;
    let root_bytes = encoded(&root)?;

    let catalog_hash = sha256_hex(&catalog_bytes);
    let targets_signed = json!({
        "_type": "targets",
        "spec_version": SPEC_VERSION,
        "version": version,
        "expires": metadata_expiry,
        "targets": {
            "catalog-v1.json": {
                "length": catalog_bytes.len(),
                "hashes": { "sha256": catalog_hash },
                "custom": {
                    "schemaVersion": 1,
                    "channel": "private-alpha",
                    "purpose": PURPOSE
                }
            }
        }
    });
    let targets = signed_envelope(
        targets_signed,
        role_keys
            .get("targets")
            .context("missing targets fixture key")?,
    )?;
    let targets_bytes = encoded(&targets)?;

    let snapshot_signed = json!({
        "_type": "snapshot",
        "spec_version": SPEC_VERSION,
        "version": version,
        "expires": metadata_expiry,
        "meta": {
            "targets.json": {
                "version": version,
                "length": targets_bytes.len(),
                "hashes": { "sha256": sha256_hex(&targets_bytes) }
            }
        }
    });
    let snapshot = signed_envelope(
        snapshot_signed,
        role_keys
            .get("snapshot")
            .context("missing snapshot fixture key")?,
    )?;
    let snapshot_bytes = encoded(&snapshot)?;

    let timestamp_signed = json!({
        "_type": "timestamp",
        "spec_version": SPEC_VERSION,
        "version": version,
        "expires": metadata_expiry,
        "meta": {
            "snapshot.json": {
                "version": version,
                "length": snapshot_bytes.len(),
                "hashes": { "sha256": sha256_hex(&snapshot_bytes) }
            }
        }
    });
    let timestamp = signed_envelope(
        timestamp_signed,
        role_keys
            .get("timestamp")
            .context("missing timestamp fixture key")?,
    )?;
    let timestamp_bytes = encoded(&timestamp)?;

    let metadata = output.join("metadata");
    let targets_dir = output.join("targets");
    fs::create_dir_all(&metadata).with_context(|| format!("create {}", metadata.display()))?;
    fs::create_dir_all(&targets_dir)
        .with_context(|| format!("create {}", targets_dir.display()))?;

    write(&output.join("root.json"), &root_bytes)?;
    write(&metadata.join(format!("{version}.root.json")), &root_bytes)?;
    write(
        &metadata.join(format!("{version}.targets.json")),
        &targets_bytes,
    )?;
    write(&metadata.join("targets.json"), &targets_bytes)?;
    write(
        &metadata.join(format!("{version}.snapshot.json")),
        &snapshot_bytes,
    )?;
    write(&metadata.join("snapshot.json"), &snapshot_bytes)?;
    write(&metadata.join("timestamp.json"), &timestamp_bytes)?;
    write(&targets_dir.join("catalog-v1.json"), &catalog_bytes)?;
    write(
        &targets_dir.join(format!("{catalog_hash}.catalog-v1.json")),
        &catalog_bytes,
    )?;

    verify(&output.join("root.json"), &metadata, &targets_dir)
}

pub fn verify(root_path: &Path, metadata: &Path, targets: &Path) -> Result<()> {
    let root = read_json(root_path)?;
    ensure_signed_type(&root, "root")?;
    let root_signed = root
        .get("signed")
        .and_then(Value::as_object)
        .context("root signed object is missing")?;
    ensure!(
        root_signed
            .get("consistent_snapshot")
            .and_then(Value::as_bool)
            == Some(true),
        "root must enable consistent snapshots"
    );

    let keys = root_signed
        .get("keys")
        .and_then(Value::as_object)
        .context("root keys are missing")?;
    let roles = root_signed
        .get("roles")
        .and_then(Value::as_object)
        .context("root roles are missing")?;
    verify_role(&root, "root", keys, roles)?;

    let timestamp_path = metadata.join("timestamp.json");
    let timestamp_bytes =
        fs::read(&timestamp_path).with_context(|| format!("read {}", timestamp_path.display()))?;
    let timestamp: Value =
        serde_json::from_slice(&timestamp_bytes).context("parse timestamp metadata")?;
    ensure_signed_type(&timestamp, "timestamp")?;
    verify_role(&timestamp, "timestamp", keys, roles)?;

    let snapshot_description = metadata_description(&timestamp, "snapshot.json")?;
    let snapshot_version = required_u64(snapshot_description, "version")?;
    let snapshot_path = metadata.join(format!("{snapshot_version}.snapshot.json"));
    let snapshot_bytes =
        fs::read(&snapshot_path).with_context(|| format!("read {}", snapshot_path.display()))?;
    verify_description(snapshot_description, &snapshot_bytes)?;
    let snapshot: Value =
        serde_json::from_slice(&snapshot_bytes).context("parse snapshot metadata")?;
    ensure_signed_type(&snapshot, "snapshot")?;
    verify_role(&snapshot, "snapshot", keys, roles)?;

    let targets_description = metadata_description(&snapshot, "targets.json")?;
    let targets_version = required_u64(targets_description, "version")?;
    let targets_path = metadata.join(format!("{targets_version}.targets.json"));
    let targets_bytes =
        fs::read(&targets_path).with_context(|| format!("read {}", targets_path.display()))?;
    verify_description(targets_description, &targets_bytes)?;
    let targets_metadata: Value =
        serde_json::from_slice(&targets_bytes).context("parse targets metadata")?;
    ensure_signed_type(&targets_metadata, "targets")?;
    verify_role(&targets_metadata, "targets", keys, roles)?;

    let target_description = targets_metadata
        .pointer("/signed/targets/catalog-v1.json")
        .and_then(Value::as_object)
        .context("catalog target description is missing")?;
    let target_hash = target_description
        .get("hashes")
        .and_then(Value::as_object)
        .and_then(|hashes| hashes.get("sha256"))
        .and_then(Value::as_str)
        .context("catalog target SHA-256 is missing")?;
    let target_path = targets.join(format!("{target_hash}.catalog-v1.json"));
    let target_bytes =
        fs::read(&target_path).with_context(|| format!("read {}", target_path.display()))?;
    verify_description(target_description, &target_bytes)?;

    let catalog: Value =
        serde_json::from_slice(&target_bytes).context("parse authenticated catalog")?;
    ensure!(
        catalog.get("schemaVersion").and_then(Value::as_u64) == Some(1),
        "authenticated catalog schemaVersion must be 1"
    );
    ensure!(
        catalog.get("packages").and_then(Value::as_array).is_some(),
        "authenticated catalog packages must be an array"
    );
    Ok(())
}

fn load_keys(directory: &Path) -> Result<BTreeMap<String, RoleKey>> {
    let mut keys = BTreeMap::new();
    for role in ["root", "targets", "snapshot", "timestamp"] {
        let path = directory.join(format!("{role}.json"));
        let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let fixture: FixtureKey =
            serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))?;
        ensure!(
            fixture.purpose == PURPOSE,
            "{} is not marked {PURPOSE}",
            path.display()
        );
        ensure!(
            fixture.scheme == "ed25519",
            "{} must use ed25519",
            path.display()
        );
        let seed: [u8; 32] = hex::decode(&fixture.seed_hex)
            .context("decode fixture seed")?
            .try_into()
            .map_err(|_| anyhow::anyhow!("{} seed must contain 32 bytes", path.display()))?;
        let signing = SigningKey::from_bytes(&seed);
        let descriptor = key_descriptor(&signing.verifying_key());
        let key_id = sha256_hex(&canonical_bytes(&descriptor)?);
        keys.insert(
            role.to_owned(),
            RoleKey {
                key_id,
                descriptor,
                signing,
            },
        );
    }
    Ok(keys)
}

fn root_signed(keys: &BTreeMap<String, RoleKey>, version: u64) -> Value {
    let mut descriptors = Map::new();
    let mut roles = Map::new();
    for role in ["root", "targets", "snapshot", "timestamp"] {
        let key = &keys[role];
        descriptors.insert(key.key_id.clone(), key.descriptor.clone());
        roles.insert(
            role.to_owned(),
            json!({
                "keyids": [key.key_id],
                "threshold": 1
            }),
        );
    }

    json!({
        "_type": "root",
        "spec_version": SPEC_VERSION,
        "version": version,
        "expires": "2035-01-01T00:00:00Z",
        "consistent_snapshot": true,
        "keys": descriptors,
        "roles": roles
    })
}

fn signed_envelope(signed: Value, key: &RoleKey) -> Result<Value> {
    let signature = key.signing.sign(&canonical_bytes(&signed)?);
    Ok(json!({
        "signatures": [{
            "keyid": key.key_id,
            "sig": hex::encode(signature.to_bytes())
        }],
        "signed": signed
    }))
}

fn verify_role(
    envelope: &Value,
    role: &str,
    keys: &Map<String, Value>,
    roles: &Map<String, Value>,
) -> Result<()> {
    let role_value = roles
        .get(role)
        .and_then(Value::as_object)
        .with_context(|| format!("root role {role} is missing"))?;
    ensure!(
        role_value.get("threshold").and_then(Value::as_u64) == Some(1),
        "fixture role {role} must have threshold 1"
    );
    let key_ids = role_value
        .get("keyids")
        .and_then(Value::as_array)
        .with_context(|| format!("fixture role {role} keyids are missing"))?;
    let signed = envelope.get("signed").context("signed object is missing")?;
    let signed_bytes = canonical_bytes(signed)?;
    let signatures = envelope
        .get("signatures")
        .and_then(Value::as_array)
        .context("signatures are missing")?;

    for key_id in key_ids.iter().filter_map(Value::as_str) {
        let Some(signature_value) = signatures
            .iter()
            .find(|signature| signature.get("keyid").and_then(Value::as_str) == Some(key_id))
        else {
            continue;
        };
        let descriptor = keys
            .get(key_id)
            .with_context(|| format!("key {key_id} is missing"))?;
        ensure!(
            sha256_hex(&canonical_bytes(descriptor)?) == key_id,
            "key identifier does not match key descriptor"
        );
        let public_hex = descriptor
            .pointer("/keyval/public")
            .and_then(Value::as_str)
            .context("ed25519 public key is missing")?;
        let public: [u8; 32] = hex::decode(public_hex)
            .context("decode ed25519 public key")?
            .try_into()
            .map_err(|_| anyhow::anyhow!("ed25519 public key must contain 32 bytes"))?;
        let signature_hex = signature_value
            .get("sig")
            .and_then(Value::as_str)
            .context("signature bytes are missing")?;
        let signature: [u8; 64] = hex::decode(signature_hex)
            .context("decode signature")?
            .try_into()
            .map_err(|_| anyhow::anyhow!("ed25519 signature must contain 64 bytes"))?;
        VerifyingKey::from_bytes(&public)
            .context("parse ed25519 public key")?
            .verify(&signed_bytes, &Signature::from_bytes(&signature))
            .with_context(|| format!("verify {role} signature"))?;
        return Ok(());
    }

    bail!("no authorized signature for {role}")
}

fn ensure_signed_type(envelope: &Value, expected: &str) -> Result<()> {
    ensure!(
        envelope.pointer("/signed/_type").and_then(Value::as_str) == Some(expected),
        "expected {expected} metadata"
    );
    ensure!(
        envelope
            .pointer("/signed/spec_version")
            .and_then(Value::as_str)
            == Some(SPEC_VERSION),
        "{expected} metadata has an unsupported specification version"
    );
    Ok(())
}

fn metadata_description<'a>(envelope: &'a Value, name: &str) -> Result<&'a Map<String, Value>> {
    envelope
        .pointer(&format!("/signed/meta/{name}"))
        .and_then(Value::as_object)
        .with_context(|| format!("{name} metadata description is missing"))
}

fn verify_description(description: &Map<String, Value>, bytes: &[u8]) -> Result<()> {
    let expected_length = required_u64(description, "length")?;
    ensure!(
        bytes.len() as u64 == expected_length,
        "authenticated length mismatch"
    );
    let expected_hash = description
        .get("hashes")
        .and_then(Value::as_object)
        .and_then(|hashes| hashes.get("sha256"))
        .and_then(Value::as_str)
        .context("authenticated SHA-256 is missing")?;
    ensure!(
        sha256_hex(bytes) == expected_hash,
        "authenticated SHA-256 mismatch"
    );
    Ok(())
}

fn required_u64(object: &Map<String, Value>, key: &str) -> Result<u64> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .with_context(|| format!("{key} must be an unsigned integer"))
}

fn key_descriptor(key: &VerifyingKey) -> Value {
    json!({
        "keytype": "ed25519",
        "scheme": "ed25519",
        "keyval": {
            "public": hex::encode(key.to_bytes())
        }
    })
}

fn canonical_bytes(value: &Value) -> Result<Vec<u8>> {
    fn write_value(value: &Value, output: &mut String) -> Result<()> {
        match value {
            Value::Null => output.push_str("null"),
            Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
            Value::Number(value) => output.push_str(&value.to_string()),
            Value::String(value) => {
                output.push_str(&serde_json::to_string(value).context("encode JSON string")?)
            }
            Value::Array(values) => {
                output.push('[');
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        output.push(',');
                    }
                    write_value(value, output)?;
                }
                output.push(']');
            }
            Value::Object(values) => {
                output.push('{');
                let mut keys: Vec<_> = values.keys().collect();
                keys.sort();
                for (index, key) in keys.into_iter().enumerate() {
                    if index > 0 {
                        output.push(',');
                    }
                    output.push_str(&serde_json::to_string(key).context("encode JSON key")?);
                    output.push(':');
                    write_value(&values[key], output)?;
                }
                output.push('}');
            }
        }
        Ok(())
    }

    let mut output = String::new();
    write_value(value, &mut output)?;
    Ok(output.into_bytes())
}

fn encoded(value: &Value) -> Result<Vec<u8>> {
    let mut bytes = canonical_bytes(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn read_json(path: &Path) -> Result<Value> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(path, bytes).with_context(|| format!("write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::canonical_bytes;

    #[test]
    fn canonical_json_sorts_object_keys() {
        let value = json!({"z": [3, 2, 1], "a": {"b": true, "a": null}});
        assert_eq!(
            canonical_bytes(&value).unwrap(),
            br#"{"a":{"a":null,"b":true},"z":[3,2,1]}"#
        );
    }
}
