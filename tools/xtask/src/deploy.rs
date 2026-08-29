use std::{
    env, fs,
    path::{Component, Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::{bail, ensure, Context, Result};
use serde_json::Value;
use walkdir::WalkDir;

const PACKAGE_NAME: &str = "aseprite-extension-manager";
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Build and deploy the current macOS helper and extension sources to Aseprite.
///
/// `workspace_root` must be the absolute Cargo workspace root. When
/// `user_config` is omitted, the Aseprite profile is resolved from
/// `ASEPRITE_USER_FOLDER`, then from the standard macOS location below `HOME`.
pub fn run(workspace_root: &Path, user_config: Option<&Path>) -> Result<()> {
    ensure!(
        cfg!(target_os = "macos"),
        "local deployment is currently supported only on macOS"
    );

    let workspace_root = canonical_directory(workspace_root, "workspace root")?;
    let user_config = resolve_user_config(user_config)?;
    let destination = validate_install_target(&user_config)?;
    validate_workspace_sources(&workspace_root)?;
    ensure_disjoint(&workspace_root, &destination)?;

    ensure_aseprite_is_not_running()?;

    // Finish the fallible build before changing the installed extension.
    let helper = build_helper(&workspace_root)?;
    deploy_payload(
        &workspace_root,
        &destination,
        &helper,
        ensure_aseprite_is_not_running,
    )?;

    println!("Deployed to {}.", destination.display());
    println!("Restart Aseprite to load the changes.");
    Ok(())
}

fn resolve_user_config(override_path: Option<&Path>) -> Result<PathBuf> {
    let path = if let Some(path) = override_path {
        path.to_path_buf()
    } else if let Some(path) = nonempty_env_path("ASEPRITE_USER_FOLDER") {
        path
    } else {
        let home = nonempty_env_path("HOME")
            .context("HOME is not set; pass --user-config or set ASEPRITE_USER_FOLDER")?;
        home.join("Library/Application Support/Aseprite")
    };

    ensure_safe_absolute(&path, "Aseprite user configuration path")?;
    Ok(path)
}

fn nonempty_env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn ensure_safe_absolute(path: &Path, label: &str) -> Result<()> {
    ensure!(path.is_absolute(), "{label} must be an absolute path");
    ensure!(
        !path
            .components()
            .any(|component| matches!(component, Component::ParentDir)),
        "{label} must not contain parent-directory components"
    );
    Ok(())
}

fn canonical_directory(path: &Path, label: &str) -> Result<PathBuf> {
    ensure_safe_absolute(path, label)?;
    let canonical =
        fs::canonicalize(path).with_context(|| format!("resolve {label} {}", path.display()))?;
    ensure!(canonical.is_dir(), "{label} is not a directory");
    Ok(canonical)
}

fn validate_workspace_sources(workspace_root: &Path) -> Result<()> {
    let extension = workspace_root.join("extension");
    let registry = workspace_root.join("registry/bundled");
    ensure_real_directory(&extension, "extension source")?;
    ensure_real_directory(&registry, "bundled registry")?;
    validate_tree(&extension, "extension source")?;
    validate_tree(&registry, "bundled registry")?;
    validate_extension_source_root(&extension)?;
    validate_manager_manifest(&extension.join("package.json"), "source")
}

fn validate_extension_source_root(extension: &Path) -> Result<()> {
    for entry in fs::read_dir(extension)
        .with_context(|| format!("read extension source {}", extension.display()))?
    {
        let entry = entry.context("read extension source entry")?;
        ensure!(
            !is_protected_top_level(&entry.file_name()),
            "extension source contains reserved top-level path: {}",
            entry.file_name().to_string_lossy()
        );
    }
    Ok(())
}

fn validate_install_target(user_config: &Path) -> Result<PathBuf> {
    let canonical_config = canonical_directory(user_config, "Aseprite user configuration path")?;
    let destination = user_config.join("extensions").join(PACKAGE_NAME);
    let metadata = fs::symlink_metadata(&destination)
        .with_context(|| format!("inspect installed extension {}", destination.display()))?;
    ensure!(
        metadata.file_type().is_dir() && !metadata.file_type().is_symlink(),
        "installed extension must be a real directory: {}",
        destination.display()
    );

    let destination = fs::canonicalize(&destination)
        .with_context(|| format!("resolve installed extension {}", destination.display()))?;
    ensure!(
        destination.starts_with(&canonical_config),
        "installed extension resolves outside the Aseprite user configuration directory"
    );

    ensure_real_file(
        &destination.join("__info.json"),
        "Aseprite extension metadata",
    )?;
    validate_manager_manifest(&destination.join("package.json"), "installed")?;
    Ok(destination)
}

fn validate_manager_manifest(path: &Path, label: &str) -> Result<()> {
    ensure_real_file(path, &format!("{label} package manifest"))?;
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let manifest: Value =
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))?;
    ensure!(
        manifest.get("name").and_then(Value::as_str) == Some(PACKAGE_NAME),
        "{label} package.json name must be {PACKAGE_NAME}"
    );
    Ok(())
}

fn ensure_aseprite_is_not_running() -> Result<()> {
    let output = Command::new("pgrep")
        .args(["-x", "aseprite"])
        .output()
        .context("check whether Aseprite is running with pgrep")?;
    match output.status.code() {
        Some(0) => bail!(
            "Aseprite is running; save your work and quit Aseprite before deploying local changes"
        ),
        Some(1) => Ok(()),
        _ => bail!(
            "pgrep could not determine whether Aseprite is running: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    }
}

fn ensure_disjoint(workspace_root: &Path, destination: &Path) -> Result<()> {
    ensure!(
        !workspace_root.starts_with(destination) && !destination.starts_with(workspace_root),
        "workspace and installed extension paths must not overlap"
    );
    Ok(())
}

fn build_helper(workspace_root: &Path) -> Result<PathBuf> {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args([
            "build",
            "--locked",
            "-p",
            "aem-helper",
            "--message-format=json-render-diagnostics",
        ])
        .current_dir(workspace_root)
        .output()
        .context("run cargo build for aem-helper")?;
    if !output.stderr.is_empty() {
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
    }
    if !output.status.success() {
        print_cargo_diagnostics(&output.stdout);
        bail!("cargo build for aem-helper failed");
    }

    let helper = helper_artifact_from_cargo(&output.stdout)?;
    ensure_real_file(&helper, "built macOS helper")?;
    Ok(helper)
}

fn helper_artifact_from_cargo(stdout: &[u8]) -> Result<PathBuf> {
    let stdout = std::str::from_utf8(stdout).context("cargo emitted non-UTF-8 JSON output")?;
    for line in stdout.lines() {
        let message: Value =
            serde_json::from_str(line).context("parse cargo JSON build message")?;
        let is_helper = message.get("reason").and_then(Value::as_str) == Some("compiler-artifact")
            && message.pointer("/target/name").and_then(Value::as_str) == Some("aem-helper");
        if is_helper {
            if let Some(executable) = message.get("executable").and_then(Value::as_str) {
                return Ok(PathBuf::from(executable));
            }
        }
    }
    bail!("cargo did not report the aem-helper executable path")
}

fn print_cargo_diagnostics(stdout: &[u8]) {
    let Ok(stdout) = std::str::from_utf8(stdout) else {
        return;
    };
    for line in stdout.lines() {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(rendered) = message.pointer("/message/rendered").and_then(Value::as_str) {
            eprint!("{rendered}");
        }
    }
}

fn deploy_payload<F>(
    workspace_root: &Path,
    destination: &Path,
    helper: &Path,
    before_swap: F,
) -> Result<()>
where
    F: FnOnce() -> Result<()>,
{
    ensure_real_file(helper, "built macOS helper")?;
    let parent = destination
        .parent()
        .context("installed extension has no parent directory")?;
    ensure_real_directory(parent, "Aseprite extensions directory")?;

    let stage = unique_child(parent, ".extension-deploy")?;
    copy_tree(&workspace_root.join("extension"), &stage)?;

    let result = (|| {
        preserve_aseprite_state(destination, &stage)?;
        copy_tree(
            &workspace_root.join("registry/bundled"),
            &stage.join("registry"),
        )?;

        let helper_directory = stage.join("bin/macos");
        fs::create_dir_all(&helper_directory)
            .with_context(|| format!("create {}", helper_directory.display()))?;
        let staged_helper = helper_directory.join("aem-helper");
        fs::copy(helper, &staged_helper).with_context(|| {
            format!(
                "copy helper {} to {}",
                helper.display(),
                staged_helper.display()
            )
        })?;
        set_mode(&staged_helper, 0o755)?;

        refresh_installed_file_inventory(&stage)?;
        validate_manager_manifest(&stage.join("package.json"), "staged")?;
        before_swap()?;
        replace_installed_directory(&stage, destination)
    })();

    if result.is_err() && fs::symlink_metadata(&stage).is_ok() {
        let _ = remove_entry(&stage);
    }
    result
}

fn is_protected_top_level(name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy();
    name.starts_with("__") || name == "bin" || name == "registry"
}

fn preserve_aseprite_state(installed: &Path, stage: &Path) -> Result<()> {
    for entry in fs::read_dir(installed)
        .with_context(|| format!("read installed extension {}", installed.display()))?
    {
        let entry = entry.context("read installed extension entry")?;
        if entry.file_name().to_string_lossy().starts_with("__") {
            copy_entry(&entry.path(), &stage.join(entry.file_name()))?;
        }
    }

    let installed_bin = installed.join("bin");
    ensure_real_directory(&installed_bin, "installed helper directory")?;
    let staged_bin = stage.join("bin");
    fs::create_dir(&staged_bin).with_context(|| format!("create {}", staged_bin.display()))?;
    for entry in fs::read_dir(&installed_bin)
        .with_context(|| format!("read installed helpers {}", installed_bin.display()))?
    {
        let entry = entry.context("read installed helper entry")?;
        if entry.file_name() != "macos" {
            copy_entry(&entry.path(), &staged_bin.join(entry.file_name()))?;
        }
    }
    Ok(())
}

fn refresh_installed_file_inventory(stage: &Path) -> Result<()> {
    let info_path = stage.join("__info.json");
    ensure_real_file(&info_path, "Aseprite extension metadata")?;
    let bytes = fs::read(&info_path).with_context(|| format!("read {}", info_path.display()))?;
    let mut info: Value =
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", info_path.display()))?;
    let info = info
        .as_object_mut()
        .context("Aseprite extension metadata must be a JSON object")?;

    let mut installed_files = Vec::new();
    for entry in WalkDir::new(stage).min_depth(1).follow_links(false) {
        let entry = entry.with_context(|| format!("walk staged extension {}", stage.display()))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(stage)
            .context("strip staged extension prefix")?;
        let mut components = Vec::new();
        for component in relative.components() {
            let Component::Normal(component) = component else {
                bail!(
                    "staged extension contains an invalid path: {}",
                    relative.display()
                );
            };
            components.push(
                component
                    .to_str()
                    .with_context(|| {
                        format!("staged extension path is not UTF-8: {}", relative.display())
                    })?
                    .to_owned(),
            );
        }
        if components
            .first()
            .is_some_and(|component| component.starts_with("__"))
        {
            continue;
        }
        installed_files.push(components.join("/"));
    }
    installed_files.sort();
    info.insert(
        "installedFiles".to_owned(),
        Value::Array(installed_files.into_iter().map(Value::String).collect()),
    );

    let bytes = serde_json::to_vec(&info).context("serialize Aseprite extension metadata")?;
    fs::write(&info_path, bytes).with_context(|| format!("write {}", info_path.display()))
}

fn copy_entry(source: &Path, destination: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)
        .with_context(|| format!("inspect preserved path {}", source.display()))?;
    if metadata.file_type().is_file() {
        fs::copy(source, destination).with_context(|| {
            format!(
                "copy preserved file {} to {}",
                source.display(),
                destination.display()
            )
        })?;
        Ok(())
    } else if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
        copy_tree(source, destination)
    } else {
        bail!(
            "refusing to preserve a symlink or special file: {}",
            source.display()
        )
    }
}

#[cfg(target_os = "macos")]
fn replace_installed_directory(stage: &Path, destination: &Path) -> Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    ensure_real_directory(stage, "staged extension")?;
    ensure_real_directory(destination, "installed extension")?;
    let staged_path = CString::new(stage.as_os_str().as_bytes())
        .context("staged extension path contains a null byte")?;
    let installed_path = CString::new(destination.as_os_str().as_bytes())
        .context("installed extension path contains a null byte")?;

    // Both directories are siblings, so RENAME_SWAP provides an atomic install:
    // the installed path is never missing, even if the process is interrupted.
    let result = unsafe {
        libc::renameatx_np(
            libc::AT_FDCWD,
            staged_path.as_ptr(),
            libc::AT_FDCWD,
            installed_path.as_ptr(),
            libc::RENAME_SWAP,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error()).with_context(|| {
            format!(
                "atomically replace installed extension {} with {}",
                destination.display(),
                stage.display()
            )
        });
    }

    remove_entry(stage).with_context(|| {
        format!(
            "deployment succeeded but the previous install could not be removed from {}",
            stage.display()
        )
    })
}

#[cfg(not(target_os = "macos"))]
fn replace_installed_directory(stage: &Path, destination: &Path) -> Result<()> {
    ensure_real_directory(destination, "installed extension")?;
    let parent = destination
        .parent()
        .context("installed extension has no parent directory")?;
    let backup = unique_child(parent, ".extension-backup")?;

    fs::rename(destination, &backup).with_context(|| {
        format!(
            "move installed extension {} to temporary backup {}",
            destination.display(),
            backup.display()
        )
    })?;

    if let Err(install_error) = fs::rename(stage, destination) {
        fs::rename(&backup, destination).with_context(|| {
            format!(
                "restore installed extension {} after deployment failed",
                destination.display()
            )
        })?;
        return Err(install_error).with_context(|| {
            format!(
                "replace installed extension {} with staged deployment {}",
                destination.display(),
                stage.display()
            )
        });
    }

    remove_entry(&backup)
}

fn validate_tree(root: &Path, label: &str) -> Result<()> {
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry.with_context(|| format!("walk {label} {}", root.display()))?;
        if entry.path() == root {
            continue;
        }
        let file_type = entry.file_type();
        ensure!(
            file_type.is_dir() || file_type.is_file(),
            "{label} contains a symlink or special file: {}",
            entry.path().display()
        );
    }
    Ok(())
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    ensure!(
        fs::symlink_metadata(destination).is_err(),
        "copy destination already exists: {}",
        destination.display()
    );
    validate_tree(source, "copy source")?;
    fs::create_dir(destination)
        .with_context(|| format!("create copy destination {}", destination.display()))?;

    for entry in WalkDir::new(source).min_depth(1).follow_links(false) {
        let entry = entry.with_context(|| format!("walk {}", source.display()))?;
        let relative = entry
            .path()
            .strip_prefix(source)
            .context("strip copy source prefix")?;
        let target = destination.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir(&target).with_context(|| format!("create {}", target.display()))?;
        } else {
            fs::copy(entry.path(), &target).with_context(|| {
                format!("copy {} to {}", entry.path().display(), target.display())
            })?;
        }
    }
    Ok(())
}

fn remove_entry(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect deployment path {}", path.display()))?;
    if metadata.file_type().is_symlink() || metadata.file_type().is_file() {
        fs::remove_file(path).with_context(|| format!("remove file {}", path.display()))
    } else if metadata.file_type().is_dir() {
        fs::remove_dir_all(path).with_context(|| format!("remove directory {}", path.display()))
    } else {
        bail!("refusing to remove special file: {}", path.display())
    }
}

fn ensure_real_directory(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect {label} {}", path.display()))?;
    ensure!(
        metadata.file_type().is_dir() && !metadata.file_type().is_symlink(),
        "{label} is not a real directory: {}",
        path.display()
    );
    Ok(())
}

fn ensure_real_file(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect {label} {}", path.display()))?;
    ensure!(
        metadata.file_type().is_file() && !metadata.file_type().is_symlink(),
        "{label} is not a regular file: {}",
        path.display()
    );
    Ok(())
}

fn unique_child(parent: &Path, stem: &str) -> Result<PathBuf> {
    for _ in 0..100 {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!("{stem}-{}-{sequence}", std::process::id()));
        if fs::symlink_metadata(&candidate).is_err() {
            return Ok(candidate);
        }
    }
    bail!("could not allocate a temporary deployment path")
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

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use tempfile::tempdir;

    use super::{
        deploy_payload, ensure_disjoint, ensure_safe_absolute, helper_artifact_from_cargo,
        validate_install_target, validate_workspace_sources, PACKAGE_NAME,
    };

    fn manifest(name: &str) -> String {
        format!(r#"{{"name":"{name}"}}"#)
    }

    fn write(path: &Path, contents: impl AsRef<[u8]>) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn create_workspace(root: &Path) {
        write(&root.join("extension/package.json"), manifest(PACKAGE_NAME));
        write(&root.join("extension/main.lua"), b"return 'new main'");
        write(
            &root.join("extension/aem/controller.lua"),
            b"return 'new controller'",
        );
        write(&root.join("registry/bundled/root.json"), b"new root");
        write(
            &root.join("registry/bundled/metadata/timestamp.json"),
            b"new timestamp",
        );
    }

    fn create_install(config: &Path, name: &str) -> std::path::PathBuf {
        let extension = config.join("extensions").join(PACKAGE_NAME);
        write(
            &extension.join("__info.json"),
            br#"{"installedFiles":["stale.txt"],"custom":"preserved"}"#,
        );
        write(&extension.join("__pref.lua"), b"original preferences");
        write(&extension.join("package.json"), manifest(name));
        extension
    }

    #[test]
    fn deploy_preserves_aseprite_state_and_other_helpers_while_removing_stale_files() {
        let temporary = tempdir().unwrap();
        let workspace = temporary.path().join("workspace");
        let config = temporary.path().join("profile");
        create_workspace(&workspace);
        let extension = create_install(&config, PACKAGE_NAME);
        write(&extension.join("stale.txt"), b"stale root");
        write(&extension.join("aem/stale.lua"), b"stale module");
        write(&extension.join("registry/stale.json"), b"stale registry");
        write(&extension.join("bin/windows/aem-helper.exe"), b"windows");
        write(&extension.join("bin/linux/aem-helper"), b"linux");
        write(&extension.join("bin/macos/aem-helper"), b"old mac");
        let helper = workspace.join("target/debug/aem-helper");
        write(&helper, b"new mac");

        validate_workspace_sources(&workspace).unwrap();
        let destination = validate_install_target(&config).unwrap();
        deploy_payload(&workspace, &destination, &helper, || Ok(())).unwrap();

        let info: serde_json::Value =
            serde_json::from_slice(&fs::read(extension.join("__info.json")).unwrap()).unwrap();
        assert_eq!(info["custom"], "preserved");
        assert_eq!(
            info["installedFiles"],
            serde_json::json!([
                "aem/controller.lua",
                "bin/linux/aem-helper",
                "bin/macos/aem-helper",
                "bin/windows/aem-helper.exe",
                "main.lua",
                "package.json",
                "registry/metadata/timestamp.json",
                "registry/root.json"
            ])
        );
        assert_eq!(
            fs::read(extension.join("__pref.lua")).unwrap(),
            b"original preferences"
        );
        assert_eq!(
            fs::read(extension.join("bin/windows/aem-helper.exe")).unwrap(),
            b"windows"
        );
        assert_eq!(
            fs::read(extension.join("bin/linux/aem-helper")).unwrap(),
            b"linux"
        );
        assert_eq!(
            fs::read(extension.join("bin/macos/aem-helper")).unwrap(),
            b"new mac"
        );
        assert!(!extension.join("stale.txt").exists());
        assert!(!extension.join("aem/stale.lua").exists());
        assert!(!extension.join("registry/stale.json").exists());
        assert_eq!(
            fs::read(extension.join("aem/controller.lua")).unwrap(),
            b"return 'new controller'"
        );
        assert_eq!(
            fs::read(extension.join("registry/root.json")).unwrap(),
            b"new root"
        );
        assert_eq!(
            fs::read(extension.join("registry/metadata/timestamp.json")).unwrap(),
            b"new timestamp"
        );
        assert!(!extension.join("registry/bundled").exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                fs::metadata(extension.join("bin/macos/aem-helper"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o755
            );
        }
    }

    #[test]
    fn install_guard_rejects_an_unrelated_package() {
        let temporary = tempdir().unwrap();
        let config = temporary.path().join("profile");
        create_install(&config, "different-extension");

        let error = validate_install_target(&config).unwrap_err();
        assert!(error
            .to_string()
            .contains("installed package.json name must be"));
    }

    #[test]
    fn path_guard_rejects_relative_and_parent_directory_paths() {
        assert!(ensure_safe_absolute(Path::new("relative/profile"), "profile").is_err());

        let temporary = tempdir().unwrap();
        let unsafe_path = temporary.path().join("profile/../profile");
        let error = ensure_safe_absolute(&unsafe_path, "profile").unwrap_err();
        assert!(error.to_string().contains("parent-directory"));
    }

    #[test]
    fn path_guard_rejects_overlapping_workspace_and_install_paths() {
        let temporary = tempdir().unwrap();
        let workspace = temporary.path().join("workspace");
        let installed_inside_workspace = workspace.join("profile/extensions").join(PACKAGE_NAME);
        let sibling_install = temporary
            .path()
            .join("profile/extensions")
            .join(PACKAGE_NAME);

        assert!(ensure_disjoint(&workspace, &installed_inside_workspace).is_err());
        assert!(ensure_disjoint(&installed_inside_workspace, &workspace).is_err());
        ensure_disjoint(&workspace, &sibling_install).unwrap();
    }

    #[test]
    fn cargo_artifact_message_selects_the_reported_helper_executable() {
        let messages = concat!(
            r#"{"reason":"compiler-artifact","target":{"name":"aem-helper"},"executable":null}"#,
            "\n",
            r#"{"reason":"compiler-artifact","target":{"name":"aem-helper"},"executable":"/tmp/custom-target/debug/aem-helper"}"#,
            "\n"
        );

        assert_eq!(
            helper_artifact_from_cargo(messages.as_bytes()).unwrap(),
            Path::new("/tmp/custom-target/debug/aem-helper")
        );
    }

    #[cfg(unix)]
    #[test]
    fn install_guard_rejects_a_symlinked_extension_directory() {
        use std::os::unix::fs::symlink;

        let temporary = tempdir().unwrap();
        let config = temporary.path().join("profile");
        let outside = temporary.path().join("outside");
        write(&outside.join("__info.json"), b"{}");
        write(&outside.join("package.json"), manifest(PACKAGE_NAME));
        fs::create_dir_all(config.join("extensions")).unwrap();
        symlink(&outside, config.join("extensions").join(PACKAGE_NAME)).unwrap();

        let error = validate_install_target(&config).unwrap_err();
        assert!(error.to_string().contains("must be a real directory"));
    }
}
