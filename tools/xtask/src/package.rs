use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::Read,
    path::{Component, Path, PathBuf},
};

use anyhow::{bail, ensure, Context, Result};
use serde_json::Value;
use walkdir::WalkDir;
use zip::{write::SimpleFileOptions, CompressionMethod, DateTime, ZipArchive, ZipWriter};

const PACKAGE_NAME: &str = "aseprite-extension-manager";
const MAX_ENTRY_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_UNCOMPRESSED_BYTES: u64 = 256 * 1024 * 1024;
const MAX_FILE_COUNT: usize = 4_096;
const MAX_COMPRESSION_RATIO: u64 = 200;
const REQUIRED_FILES: &[&str] = &[
    "package.json",
    "main.lua",
    "bin/macos/aem-helper",
    "bin/windows/aem-helper.exe",
    "bin/linux/aem-helper",
    "registry/root.json",
    "registry/metadata/timestamp.json",
    "registry/metadata/snapshot.json",
    "registry/metadata/targets.json",
    "registry/targets/catalog-v1.json",
];

pub fn stage(
    extension: &Path,
    registry: &Path,
    macos_helper: &Path,
    windows_helper: &Path,
    linux_helper: &Path,
    output: &Path,
) -> Result<()> {
    ensure_directory(extension, "extension source")?;
    ensure_directory(registry, "bundled registry")?;
    ensure_file(macos_helper, "macOS helper")?;
    ensure_file(windows_helper, "Windows helper")?;
    ensure_file(linux_helper, "Linux helper")?;

    if output.exists() {
        bail!("stage output already exists: {}", output.display());
    }

    fs::create_dir_all(output).with_context(|| format!("create stage {}", output.display()))?;
    copy_tree(extension, output)?;
    copy_tree(registry, &output.join("registry"))?;
    copy_file(macos_helper, &output.join("bin/macos/aem-helper"), 0o755)?;
    copy_file(
        windows_helper,
        &output.join("bin/windows/aem-helper.exe"),
        0o644,
    )?;
    copy_file(linux_helper, &output.join("bin/linux/aem-helper"), 0o755)?;

    validate_directory(output)?;
    Ok(())
}

pub fn create(input: &Path, output: &Path) -> Result<()> {
    validate_directory(input)?;

    let manifest = read_manifest(&fs::read(input.join("package.json"))?)?;
    let version = manifest
        .get("version")
        .and_then(Value::as_str)
        .context("package.json version must be a string")?;
    let output_path =
        if output.extension().and_then(|value| value.to_str()) == Some("aseprite-extension") {
            output.to_path_buf()
        } else {
            fs::create_dir_all(output).with_context(|| format!("create {}", output.display()))?;
            output.join(format!("{PACKAGE_NAME}-{version}.aseprite-extension"))
        };

    if output_path.exists() {
        bail!("package output already exists: {}", output_path.display());
    }
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }

    let files = collect_files(input)?;
    let file =
        File::create(&output_path).with_context(|| format!("create {}", output_path.display()))?;
    let mut writer = ZipWriter::new(file);

    for (relative, absolute) in files {
        let mode = packaged_mode(&relative);
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .compression_level(Some(9))
            .last_modified_time(DateTime::default())
            .unix_permissions(mode);
        writer
            .start_file(&relative, options)
            .with_context(|| format!("add {relative}"))?;
        let mut source =
            File::open(&absolute).with_context(|| format!("open {}", absolute.display()))?;
        std::io::copy(&mut source, &mut writer).with_context(|| format!("write {relative}"))?;
    }

    writer.finish().context("finish extension archive")?;
    validate_archive(&output_path)
}

pub fn validate(path: &Path) -> Result<()> {
    if path.is_dir() {
        validate_directory(path)
    } else {
        validate_archive(path)
    }
}

fn validate_directory(root: &Path) -> Result<()> {
    ensure_directory(root, "extension")?;
    let files = collect_files(root)?;
    let names: BTreeSet<&str> = files.iter().map(|(name, _)| name.as_str()).collect();
    validate_names(&names)?;

    for (name, path) in &files {
        let metadata =
            fs::symlink_metadata(path).with_context(|| format!("inspect {}", path.display()))?;
        ensure!(
            metadata.file_type().is_file(),
            "{name} is not a regular file"
        );
        ensure!(
            metadata.len() <= MAX_ENTRY_BYTES,
            "{name} exceeds the per-file size limit"
        );
        if is_helper(name) {
            validate_helper_magic(name, &fs::read(path)?)?;
        }
    }

    let manifest_bytes = fs::read(root.join("package.json")).context("read package.json")?;
    let version = validate_manifest(&manifest_bytes, &names)?;
    aem_helper::package::validate_manager_directory(root, &version)
        .map_err(|error| anyhow::anyhow!("runtime manager validation failed: {error}"))?;
    aem_helper::registry::validate_bundled_repository(&root.join("registry"))
        .map_err(|error| anyhow::anyhow!("bundled registry validation failed: {error}"))?;
    Ok(())
}

fn validate_archive(path: &Path) -> Result<()> {
    ensure_file(path, "extension archive")?;
    let metadata = fs::metadata(path).with_context(|| format!("inspect {}", path.display()))?;
    ensure!(
        metadata.len() <= MAX_ARCHIVE_BYTES,
        "archive exceeds the size limit"
    );

    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut archive = ZipArchive::new(file).context("read extension ZIP")?;
    ensure!(
        archive.len() <= MAX_FILE_COUNT,
        "archive contains more than 4096 entries"
    );
    let mut names = BTreeSet::new();
    let mut folded = BTreeSet::new();
    let mut manifest = None;
    let mut total = 0_u64;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).context("read ZIP entry")?;
        let name = normalized_relative(Path::new(entry.name()))?;
        ensure!(
            !name.ends_with('/'),
            "directory ZIP entries are not allowed"
        );
        ensure!(names.insert(name.clone()), "duplicate ZIP path: {name}");
        ensure!(
            folded.insert(name.to_lowercase()),
            "case-colliding ZIP path: {name}"
        );
        ensure!(
            entry.size() <= MAX_ENTRY_BYTES,
            "{name} exceeds the per-file size limit"
        );
        let compressed = entry.compressed_size();
        ensure!(
            entry.size() <= 1024 * 1024
                || (compressed > 0 && entry.size() / compressed.max(1) <= MAX_COMPRESSION_RATIO),
            "{name} has an unsafe compression ratio"
        );
        total = total
            .checked_add(entry.size())
            .context("uncompressed archive size overflow")?;
        ensure!(
            total <= MAX_UNCOMPRESSED_BYTES,
            "uncompressed archive exceeds the size limit"
        );

        if let Some(mode) = entry.unix_mode() {
            let file_type = mode & 0o170000;
            ensure!(
                file_type == 0 || file_type == 0o100000,
                "{name} is not a regular file"
            );
            let expected = packaged_mode(&name);
            ensure!(
                mode & 0o777 == expected,
                "{name} has mode {:o}; expected {:o}",
                mode & 0o777,
                expected
            );
        }

        if name == "package.json" {
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .context("read package.json from archive")?;
            manifest = Some(bytes);
        } else if is_helper(&name) {
            let mut bytes = [0_u8; 4];
            entry
                .read_exact(&mut bytes)
                .with_context(|| format!("read executable header for {name}"))?;
            validate_helper_magic(&name, &bytes)?;
        }
    }

    validate_names(&names.iter().map(String::as_str).collect())?;
    let version = validate_manifest(
        manifest
            .as_deref()
            .context("archive is missing package.json")?,
        &names.iter().map(String::as_str).collect(),
    )?;
    aem_helper::package::validate_manager_archive(path, &version)
        .map_err(|error| anyhow::anyhow!("runtime manager validation failed: {error}"))?;
    Ok(())
}

fn validate_names(names: &BTreeSet<&str>) -> Result<()> {
    for required in REQUIRED_FILES {
        ensure!(
            names.contains(required),
            "missing required file: {required}"
        );
    }
    for name in names {
        ensure!(
            !contains_private_registry_material(name),
            "private registry material must not be packaged: {name}"
        );
        let folded = name.to_ascii_lowercase();
        let is_expected_windows_helper = *name == "bin/windows/aem-helper.exe";
        ensure!(
            is_expected_windows_helper
                || !(folded.ends_with(".exe")
                    || folded.ends_with(".dll")
                    || folded.ends_with(".dylib")
                    || folded.ends_with(".so")),
            "unexpected native file in manager package: {name}"
        );
    }
    ensure!(
        names
            .iter()
            .filter(|name| name.rsplit('/').next() == Some("package.json"))
            .count()
            == 1,
        "manager package must contain exactly one package.json"
    );
    ensure!(
        names.iter().any(|name| is_consistent_catalog_target(name)),
        "missing hash-prefixed catalog target"
    );
    ensure!(
        names
            .iter()
            .any(|name| is_versioned_metadata(name, "snapshot")),
        "missing versioned snapshot metadata"
    );
    ensure!(
        names
            .iter()
            .any(|name| is_versioned_metadata(name, "targets")),
        "missing versioned targets metadata"
    );
    Ok(())
}

fn contains_private_registry_material(name: &str) -> bool {
    if !name.starts_with("registry/") {
        return false;
    }
    name.split('/').any(|component| {
        let folded = component.to_ascii_lowercase();
        folded == "keys"
            || folded == "fixtures"
            || folded.contains("private")
            || folded.contains("seed")
    })
}

fn is_consistent_catalog_target(name: &str) -> bool {
    let Some(filename) = name.strip_prefix("registry/targets/") else {
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

fn is_versioned_metadata(name: &str, role: &str) -> bool {
    let Some(filename) = name.strip_prefix("registry/metadata/") else {
        return false;
    };
    let Some(version) = filename.strip_suffix(&format!(".{role}.json")) else {
        return false;
    };
    version.parse::<u64>().is_ok_and(|value| value > 0)
}

fn is_helper(name: &str) -> bool {
    matches!(
        name,
        "bin/macos/aem-helper" | "bin/windows/aem-helper.exe" | "bin/linux/aem-helper"
    )
}

fn validate_helper_magic(name: &str, bytes: &[u8]) -> Result<()> {
    ensure!(bytes.len() >= 4, "{name} is too short to be an executable");
    let valid = match name {
        "bin/macos/aem-helper" => {
            matches!(
                &bytes[..4],
                [0xca, 0xfe, 0xba, 0xbe] | [0xca, 0xfe, 0xba, 0xbf]
            )
        }
        "bin/windows/aem-helper.exe" => bytes.starts_with(b"MZ"),
        "bin/linux/aem-helper" => bytes.starts_with(b"\x7fELF"),
        _ => true,
    };
    ensure!(valid, "{name} has an unexpected executable format");
    Ok(())
}

fn validate_manifest(bytes: &[u8], names: &BTreeSet<&str>) -> Result<String> {
    let manifest = read_manifest(bytes)?;
    ensure!(
        manifest.get("name").and_then(Value::as_str) == Some(PACKAGE_NAME),
        "package.json name must be {PACKAGE_NAME}"
    );
    ensure!(
        manifest.get("displayName").and_then(Value::as_str) == Some("Aseprite Extension Manager"),
        "package.json displayName is invalid"
    );
    ensure!(
        manifest.get("publisher").and_then(Value::as_str) == Some("martincalander"),
        "package.json publisher is invalid"
    );
    ensure!(
        manifest.get("license").and_then(Value::as_str) == Some("MIT"),
        "package.json license is invalid"
    );

    let author_is_valid = match manifest.get("author") {
        Some(Value::String(author)) => author == "Martin Calander",
        Some(Value::Object(author)) => {
            author.get("name").and_then(Value::as_str) == Some("Martin Calander")
        }
        _ => false,
    };
    ensure!(author_is_valid, "package.json author is invalid");
    ensure!(
        manifest
            .pointer("/engines/aseprite")
            .and_then(Value::as_str)
            == Some(">=1.3.15"),
        "package.json minimum Aseprite version must be 1.3.15"
    );
    ensure!(
        manifest
            .pointer("/engines/asepriteApi")
            .and_then(Value::as_str)
            == Some(">=35"),
        "package.json minimum Aseprite API must be 35"
    );

    let version = manifest
        .get("version")
        .and_then(Value::as_str)
        .context("package.json version must be a string")?;
    ensure!(
        is_semver(version),
        "package.json version must be MAJOR.MINOR.PATCH"
    );

    validate_contribution_paths(&manifest, names)?;
    Ok(version.to_owned())
}

fn validate_contribution_paths(manifest: &Value, names: &BTreeSet<&str>) -> Result<()> {
    let Some(contributes) = manifest.get("contributes").and_then(Value::as_object) else {
        bail!("package.json contributes must be an object");
    };

    let mut paths = Vec::new();
    for entries in contributes.values().filter_map(Value::as_array) {
        for entry in entries {
            if let Some(path) = entry.get("path").and_then(Value::as_str) {
                paths.push(path);
            } else if let Some(path) = entry.as_str() {
                paths.push(path);
            }
        }
    }
    ensure!(!paths.is_empty(), "package.json has no contribution paths");

    for path in paths {
        let path = path.strip_prefix("./").unwrap_or(path);
        let normalized = normalized_relative(Path::new(path))?;
        ensure!(
            names.contains(normalized.as_str()),
            "missing contribution file: {normalized}"
        );
    }
    Ok(())
}

fn read_manifest(bytes: &[u8]) -> Result<Value> {
    serde_json::from_slice(bytes).context("parse package.json")
}

fn is_semver(value: &str) -> bool {
    let parts: Vec<_> = value.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty() && part.chars().all(|character| character.is_ascii_digit())
        })
}

fn collect_files(root: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut files = BTreeMap::new();
    let mut folded = BTreeSet::new();

    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry.with_context(|| format!("walk {}", root.display()))?;
        if entry.path() == root {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(root)
            .context("strip package root")?;
        let name = normalized_relative(relative)?;
        let file_type = entry.file_type();
        if file_type.is_dir() {
            continue;
        }
        ensure!(file_type.is_file(), "{name} is not a regular file");
        ensure!(
            folded.insert(name.to_lowercase()),
            "case-colliding package path: {name}"
        );
        ensure!(
            files
                .insert(name.clone(), entry.path().to_path_buf())
                .is_none(),
            "duplicate package path: {name}"
        );
    }

    Ok(files.into_iter().collect())
}

fn normalized_relative(path: &Path) -> Result<String> {
    ensure!(!path.as_os_str().is_empty(), "empty package path");
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => {
                let value = value
                    .to_str()
                    .context("package paths must be valid UTF-8")?;
                ensure!(!value.is_empty(), "empty package path component");
                parts.push(value);
            }
            _ => bail!("unsafe package path: {}", path.display()),
        }
    }
    ensure!(!parts.is_empty(), "empty package path");
    Ok(parts.join("/"))
}

fn packaged_mode(relative: &str) -> u32 {
    match relative {
        "bin/macos/aem-helper" | "bin/linux/aem-helper" => 0o755,
        _ => 0o644,
    }
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    let files = collect_files(source)?;
    for (relative, path) in files {
        copy_file(&path, &destination.join(relative), 0o644)?;
    }
    Ok(())
}

fn copy_file(source: &Path, destination: &Path, mode: u32) -> Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::copy(source, destination)
        .with_context(|| format!("copy {} to {}", source.display(), destination.display()))?;
    set_mode(destination, mode)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .with_context(|| format!("set mode on {}", path.display()))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

fn ensure_directory(path: &Path, label: &str) -> Result<()> {
    ensure!(
        path.is_dir(),
        "{label} is not a directory: {}",
        path.display()
    );
    Ok(())
}

fn ensure_file(path: &Path, label: &str) -> Result<()> {
    ensure!(path.is_file(), "{label} is not a file: {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, File},
        io::Write,
        path::Path,
    };

    use tempfile::tempdir;
    use walkdir::WalkDir;
    use zip::{write::SimpleFileOptions, ZipArchive, ZipWriter};

    use super::{
        contains_private_registry_material, create, is_consistent_catalog_target, is_semver,
        normalized_relative, validate,
    };

    #[test]
    fn semver_requires_three_numeric_parts() {
        assert!(is_semver("1.2.3"));
        assert!(!is_semver("1.2"));
        assert!(!is_semver("1.2.3-alpha"));
        assert!(!is_semver("1.02.x"));
    }

    #[test]
    fn rejects_parent_path() {
        assert!(normalized_relative(Path::new("../escape")).is_err());
        assert!(normalized_relative(Path::new("/absolute")).is_err());
    }

    #[test]
    fn rejects_case_colliding_archive() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("collision.aseprite-extension");
        let file = File::create(&path).unwrap();
        let mut writer = ZipWriter::new(file);
        let options = SimpleFileOptions::default();
        writer.start_file("main.lua", options).unwrap();
        writer.write_all(b"").unwrap();
        writer.start_file("MAIN.LUA", options).unwrap();
        writer.write_all(b"").unwrap();
        writer.finish().unwrap();

        let error = validate(&path).unwrap_err();
        assert!(error.to_string().contains("case-colliding"));
    }

    #[test]
    fn recognizes_private_registry_material() {
        assert!(contains_private_registry_material(
            "registry/fixtures/keys/root.json"
        ));
        assert!(contains_private_registry_material(
            "registry/metadata/private-seed.json"
        ));
        assert!(!contains_private_registry_material(
            "registry/metadata/1.targets.json"
        ));
        assert!(is_consistent_catalog_target(
            "registry/targets/0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef.catalog-v1.json"
        ));
    }

    #[test]
    fn package_is_deterministic_and_sets_helper_modes() {
        let directory = tempdir().unwrap();
        let input = directory.path().join("input");
        create_minimal_tree(&input);
        let first = directory.path().join("first");
        let second = directory.path().join("second");

        create(&input, &first).unwrap();
        create(&input, &second).unwrap();
        let name = "aseprite-extension-manager-0.1.0.aseprite-extension";
        assert_eq!(
            fs::read(first.join(name)).unwrap(),
            fs::read(second.join(name)).unwrap()
        );

        let file = File::open(first.join(name)).unwrap();
        let mut archive = ZipArchive::new(file).unwrap();
        assert_eq!(
            archive
                .by_name("bin/macos/aem-helper")
                .unwrap()
                .unix_mode()
                .unwrap()
                & 0o777,
            0o755
        );
        assert_eq!(
            archive
                .by_name("bin/windows/aem-helper.exe")
                .unwrap()
                .unix_mode()
                .unwrap()
                & 0o777,
            0o644
        );
    }

    #[test]
    fn extension_rejects_fixture_keys() {
        let directory = tempdir().unwrap();
        let input = directory.path().join("input");
        create_minimal_tree(&input);
        fs::create_dir_all(input.join("registry/fixtures/keys")).unwrap();
        fs::write(input.join("registry/fixtures/keys/root.json"), b"{}").unwrap();

        let error = validate(&input).unwrap_err();
        assert!(error
            .to_string()
            .contains("private registry material must not be packaged"));
    }

    #[test]
    fn final_package_must_pass_the_runtime_manager_validator() {
        let directory = tempdir().unwrap();
        let input = directory.path().join("input");
        create_minimal_tree(&input);
        fs::write(input.join("disguised-data.bin"), b"\x7fELF-native").unwrap();

        let error = create(&input, &directory.path().join("output")).unwrap_err();
        assert!(error
            .to_string()
            .contains("runtime manager validation failed"));
    }

    fn copy_test_registry(destination: &Path) {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("registry")
            .join("bundled");
        for entry in WalkDir::new(&source) {
            let entry = entry.unwrap();
            let relative = entry.path().strip_prefix(&source).unwrap();
            let target = destination.join(relative);
            if entry.file_type().is_dir() {
                fs::create_dir_all(&target).unwrap();
            } else {
                fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    fn create_minimal_tree(root: &Path) {
        let files = [
            ("main.lua", b"return {}\n".as_slice()),
            ("bin/windows/aem-helper.exe", b"MZ-windows".as_slice()),
            ("bin/linux/aem-helper", b"\x7fELF-linux".as_slice()),
        ];
        for (relative, bytes) in files {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
        let macos_helper = root.join("bin/macos/aem-helper");
        fs::create_dir_all(macos_helper.parent().unwrap()).unwrap();
        fs::write(macos_helper, universal_macos_fixture()).unwrap();
        fs::write(
            root.join("package.json"),
            br#"{
                "name":"aseprite-extension-manager",
                "displayName":"Aseprite Extension Manager",
                "publisher":"martincalander",
                "author":{"name":"Martin Calander"},
                "license":"MIT",
                "version":"0.1.0",
                "engines":{"aseprite":">=1.3.15","asepriteApi":">=35"},
                "contributes":{"scripts":[{"path":"./main.lua"}]}
            }"#,
        )
        .unwrap();
        copy_test_registry(&root.join("registry"));
    }

    fn universal_macos_fixture() -> Vec<u8> {
        const CPU_TYPE_X86_64: u32 = 0x0100_0007;
        const CPU_TYPE_ARM64: u32 = 0x0100_000c;
        const HEADER_LENGTH: u32 = 48;
        const SLICE_LENGTH: u32 = 32;

        fn thin(cpu: u32) -> Vec<u8> {
            let mut binary = vec![0_u8; SLICE_LENGTH as usize];
            binary[..4].copy_from_slice(&[0xcf, 0xfa, 0xed, 0xfe]);
            binary[4..8].copy_from_slice(&cpu.to_le_bytes());
            binary
        }

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
        binary.extend_from_slice(&thin(CPU_TYPE_X86_64));
        binary.extend_from_slice(&thin(CPU_TYPE_ARM64));
        binary
    }
}
