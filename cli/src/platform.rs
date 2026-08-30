#[cfg(target_os = "macos")]
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(not(target_os = "macos"))]
use std::process::Stdio;

use aem_helper::protocol::{RpcError, RpcResult};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AsepriteApplication {
    #[cfg(target_os = "macos")]
    AppBundle(PathBuf),
    #[cfg(not(target_os = "macos"))]
    Executable(PathBuf),
}

pub(crate) fn resolve_application(path: Option<PathBuf>) -> RpcResult<Option<AsepriteApplication>> {
    let Some(path) = path else {
        return Ok(None);
    };

    #[cfg(target_os = "macos")]
    if path.extension().and_then(|extension| extension.to_str()) == Some("app") {
        return resolve_macos_bundle(&path).map(Some);
    }

    let executable = canonical_executable(&path)?;
    #[cfg(target_os = "macos")]
    {
        if let Some(bundle) = macos_bundle_for_executable(&executable) {
            return Ok(Some(AsepriteApplication::AppBundle(bundle)));
        }
        Err(RpcError::invalid(
            "INVALID_ASEPRITE_EXECUTABLE",
            "on macOS, --aseprite must name Aseprite.app or its Contents/MacOS/aseprite executable",
        ))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(Some(AsepriteApplication::Executable(executable)))
    }
}

pub(crate) fn open_extension(
    application: Option<&AsepriteApplication>,
    user_config: &Path,
    _isolated_profile: bool,
    artifact: &Path,
) -> RpcResult<()> {
    #[cfg(target_os = "macos")]
    {
        open_extension_macos(application, user_config, _isolated_profile, artifact)
    }

    #[cfg(target_os = "windows")]
    {
        match application {
            Some(AsepriteApplication::Executable(executable)) => {
                spawn_executable(executable, user_config, artifact)
            }
            None => opener::open(artifact).map_err(|error| {
                RpcError::invalid(
                    "ASEPRITE_OPEN_FAILED",
                    format!(
                        "could not open the prepared package with Aseprite: {error}; pass --aseprite PATH if Aseprite is not the registered handler"
                    ),
                )
            }),
        }
    }

    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        match application {
            Some(AsepriteApplication::Executable(executable)) => {
                spawn_executable(executable, user_config, artifact)
            }
            None => std::process::Command::new("xdg-open")
                .arg(artifact)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map(|_| ())
                .map_err(|error| {
                    RpcError::invalid(
                        "ASEPRITE_OPEN_FAILED",
                        format!(
                            "could not start xdg-open for the prepared package: {error}; pass --aseprite PATH"
                        ),
                    )
                }),
        }
    }
}

fn canonical_executable(path: &Path) -> RpcResult<PathBuf> {
    let path = fs::canonicalize(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            RpcError::invalid(
                "ASEPRITE_EXECUTABLE_NOT_FOUND",
                format!("Aseprite executable does not exist: {}", path.display()),
            )
        } else {
            RpcError::io(error)
        }
    })?;
    let metadata = fs::metadata(&path).map_err(RpcError::io)?;
    if !metadata.is_file() {
        return Err(RpcError::invalid(
            "INVALID_ASEPRITE_EXECUTABLE",
            format!("Aseprite executable must be a file: {}", path.display()),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(RpcError::invalid(
                "INVALID_ASEPRITE_EXECUTABLE",
                format!("Aseprite executable is not executable: {}", path.display()),
            ));
        }
    }
    Ok(path)
}

#[cfg(not(target_os = "macos"))]
fn spawn_executable(executable: &Path, user_config: &Path, artifact: &Path) -> RpcResult<()> {
    std::process::Command::new(executable)
        .env("ASEPRITE_USER_FOLDER", user_config)
        .arg(artifact)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| {
            RpcError::invalid(
                "ASEPRITE_OPEN_FAILED",
                format!("could not start the selected Aseprite executable: {error}"),
            )
        })
}

#[cfg(target_os = "macos")]
fn resolve_macos_bundle(path: &Path) -> RpcResult<AsepriteApplication> {
    let bundle = fs::canonicalize(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            RpcError::invalid(
                "ASEPRITE_EXECUTABLE_NOT_FOUND",
                format!("Aseprite application does not exist: {}", path.display()),
            )
        } else {
            RpcError::io(error)
        }
    })?;
    if !bundle.is_dir() {
        return Err(RpcError::invalid(
            "INVALID_ASEPRITE_EXECUTABLE",
            format!(
                "Aseprite application must be a bundle: {}",
                bundle.display()
            ),
        ));
    }
    canonical_executable(&bundle.join("Contents/MacOS/aseprite"))?;
    Ok(AsepriteApplication::AppBundle(bundle))
}

#[cfg(target_os = "macos")]
fn macos_bundle_for_executable(executable: &Path) -> Option<PathBuf> {
    if executable.file_name()?.to_str()? != "aseprite" {
        return None;
    }
    let macos = executable.parent()?;
    if macos.file_name()?.to_str()? != "MacOS" {
        return None;
    }
    let contents = macos.parent()?;
    if contents.file_name()?.to_str()? != "Contents" {
        return None;
    }
    let bundle = contents.parent()?;
    (bundle.extension()?.to_str()? == "app").then(|| bundle.to_path_buf())
}

#[cfg(target_os = "macos")]
fn open_extension_macos(
    application: Option<&AsepriteApplication>,
    user_config: &Path,
    isolated_profile: bool,
    artifact: &Path,
) -> RpcResult<()> {
    match application {
        Some(AsepriteApplication::AppBundle(bundle)) => run_open(macos_bundle_command(
            bundle,
            user_config,
            isolated_profile,
            artifact,
        )),
        None => run_open(macos_registered_command(artifact)),
    }
}

#[cfg(target_os = "macos")]
fn macos_registered_command(artifact: &Path) -> std::process::Command {
    let mut command = std::process::Command::new("/usr/bin/open");
    command.arg("-b").arg("org.aseprite.Aseprite").arg(artifact);
    command
}

#[cfg(target_os = "macos")]
fn macos_bundle_command(
    bundle: &Path,
    user_config: &Path,
    isolated_profile: bool,
    artifact: &Path,
) -> std::process::Command {
    let mut command = std::process::Command::new("/usr/bin/open");
    if isolated_profile {
        command.arg("-n");
    }
    command.arg("-a").arg(bundle);
    if isolated_profile {
        let mut profile = OsString::from("ASEPRITE_USER_FOLDER=");
        profile.push(user_config.as_os_str());
        command.arg("--env").arg(profile);
    }
    command.arg(artifact);
    command
}

#[cfg(target_os = "macos")]
fn run_open(mut command: std::process::Command) -> RpcResult<()> {
    let output = command.output().map_err(|error| {
        RpcError::invalid(
            "ASEPRITE_OPEN_FAILED",
            format!("could not ask macOS to open the prepared package in Aseprite: {error}"),
        )
    })?;
    if !output.status.success() {
        return Err(RpcError::invalid(
            "ASEPRITE_OPEN_FAILED",
            format!(
                "macOS could not open the prepared package in Aseprite (status {}); pass --aseprite /path/to/Aseprite.app",
                output.status
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    fn make_bundle(root: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let bundle = root.join("Aseprite With Spaces.app");
        let executable = bundle.join("Contents/MacOS/aseprite");
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::write(&executable, b"#!/bin/sh\n").unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).unwrap();
        bundle
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_bundle_input_preserves_bundle_identity() {
        let temporary = tempfile::tempdir().unwrap();
        let bundle = make_bundle(temporary.path());
        let expected = fs::canonicalize(&bundle).unwrap();

        assert_eq!(
            resolve_application(Some(bundle)).unwrap(),
            Some(AsepriteApplication::AppBundle(expected))
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_bundle_executable_resolves_back_to_bundle() {
        let temporary = tempfile::tempdir().unwrap();
        let bundle = make_bundle(temporary.path());
        let executable = bundle.join("Contents/MacOS/aseprite");
        let expected = fs::canonicalize(&bundle).unwrap();

        assert_eq!(
            resolve_application(Some(executable)).unwrap(),
            Some(AsepriteApplication::AppBundle(expected))
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_rejects_a_raw_executable_that_cannot_be_activated_as_an_app() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().unwrap();
        let executable = temporary.path().join("aseprite");
        fs::write(&executable, b"#!/bin/sh\n").unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).unwrap();

        let error = resolve_application(Some(executable)).unwrap_err();

        assert_eq!(error.code, "INVALID_ASEPRITE_EXECUTABLE");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_default_handoff_targets_the_registered_bundle() {
        use std::ffi::OsString;

        let command =
            macos_registered_command(Path::new("/tmp/Package With Spaces.aseprite-extension"));
        let arguments: Vec<_> = command.get_args().map(OsString::from).collect();

        assert_eq!(
            arguments,
            [
                OsString::from("-b"),
                OsString::from("org.aseprite.Aseprite"),
                OsString::from("/tmp/Package With Spaces.aseprite-extension"),
            ]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_custom_profile_gets_an_activated_isolated_instance() {
        use std::ffi::OsString;

        let command = macos_bundle_command(
            Path::new("/Applications/Aseprite Preview.app"),
            Path::new("/tmp/Profile With Spaces"),
            true,
            Path::new("/tmp/Package With Spaces.aseprite-extension"),
        );
        let arguments: Vec<_> = command.get_args().map(OsString::from).collect();

        assert_eq!(
            arguments,
            [
                OsString::from("-n"),
                OsString::from("-a"),
                OsString::from("/Applications/Aseprite Preview.app"),
                OsString::from("--env"),
                OsString::from("ASEPRITE_USER_FOLDER=/tmp/Profile With Spaces"),
                OsString::from("/tmp/Package With Spaces.aseprite-extension"),
            ]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_explicit_bundle_reuses_the_regular_profile_instance() {
        use std::ffi::OsString;

        let command = macos_bundle_command(
            Path::new("/Applications/Aseprite Preview.app"),
            Path::new("/tmp/Profile"),
            false,
            Path::new("/tmp/Package.aseprite-extension"),
        );
        let arguments: Vec<_> = command.get_args().map(OsString::from).collect();

        assert_eq!(
            arguments,
            [
                OsString::from("-a"),
                OsString::from("/Applications/Aseprite Preview.app"),
                OsString::from("/tmp/Package.aseprite-extension"),
            ]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_custom_profile_argument_preserves_non_unicode_paths() {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};

        let profile = PathBuf::from(OsString::from_vec(b"/tmp/Profile\xff".to_vec()));
        let command = macos_bundle_command(
            Path::new("/Applications/Aseprite.app"),
            &profile,
            true,
            Path::new("/tmp/Package.aseprite-extension"),
        );
        let environment = command.get_args().nth(4).unwrap();

        assert_eq!(
            environment.as_bytes(),
            b"ASEPRITE_USER_FOLDER=/tmp/Profile\xff"
        );
    }
}
