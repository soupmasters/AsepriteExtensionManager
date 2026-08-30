use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};

use aem_helper::protocol::{RpcError, RpcResult};

pub const MANAGER_NAME: &str = "aseprite-extension-manager";
const FALLBACK_ASEPRITE_VERSION: &str = "1.3.15";
const FALLBACK_API_VERSION: u32 = 35;

#[derive(Clone, Debug)]
pub struct ProfilePaths {
    pub user_config: PathBuf,
    pub manager_root: PathBuf,
    pub aseprite_version: String,
    pub api_version: u32,
}

impl ProfilePaths {
    pub fn resolve(
        user_config: Option<PathBuf>,
        manager_root: Option<PathBuf>,
        aseprite_version: Option<String>,
        api_version: Option<u32>,
    ) -> RpcResult<Self> {
        let user_config = choose_user_config(
            user_config,
            nonempty_env_path("ASEPRITE_USER_FOLDER"),
            default_user_config,
        )?;
        let user_config = canonical_real_directory(&absolute(user_config)?, "Aseprite profile")?;
        let manager_root = manager_root
            .or_else(|| nonempty_env_path("AEM_EXTENSION_ROOT"))
            .unwrap_or_else(|| user_config.join("extensions").join(MANAGER_NAME));
        let manager_root = absolute(manager_root)?;
        let aseprite_version = aseprite_version
            .or_else(|| read_aseprite_version(&user_config))
            .unwrap_or_else(|| FALLBACK_ASEPRITE_VERSION.to_owned());
        if !valid_aseprite_version(&aseprite_version) {
            return Err(RpcError::invalid(
                "INVALID_ASEPRITE_VERSION",
                "Aseprite version must contain dot-separated numbers",
            ));
        }
        Ok(Self {
            user_config,
            manager_root,
            aseprite_version,
            api_version: api_version.unwrap_or(FALLBACK_API_VERSION),
        })
    }
}

fn choose_user_config(
    explicit: Option<PathBuf>,
    environment: Option<PathBuf>,
    default: impl FnOnce() -> RpcResult<PathBuf>,
) -> RpcResult<PathBuf> {
    explicit.or(environment).map_or_else(default, Ok)
}

#[cfg(target_os = "windows")]
fn default_user_config() -> RpcResult<PathBuf> {
    nonempty_env_path("APPDATA")
        .map(|path| path.join("Aseprite"))
        .ok_or_else(|| {
            RpcError::invalid(
                "PROFILE_NOT_FOUND",
                "APPDATA is not set; pass --user-config or set ASEPRITE_USER_FOLDER",
            )
        })
}

#[cfg(target_os = "macos")]
fn default_user_config() -> RpcResult<PathBuf> {
    nonempty_env_path("HOME")
        .map(|path| path.join("Library/Application Support/Aseprite"))
        .ok_or_else(|| {
            RpcError::invalid(
                "PROFILE_NOT_FOUND",
                "HOME is not set; pass --user-config or set ASEPRITE_USER_FOLDER",
            )
        })
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn default_user_config() -> RpcResult<PathBuf> {
    if let Some(path) = nonempty_env_path("XDG_CONFIG_HOME") {
        return Ok(path.join("aseprite"));
    }
    nonempty_env_path("HOME")
        .map(|path| path.join(".config/aseprite"))
        .ok_or_else(|| {
            RpcError::invalid(
                "PROFILE_NOT_FOUND",
                "HOME is not set; pass --user-config or set ASEPRITE_USER_FOLDER",
            )
        })
}

fn nonempty_env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn absolute(path: PathBuf) -> RpcResult<PathBuf> {
    let path = if path.is_absolute() {
        path
    } else {
        env::current_dir().map_err(RpcError::io)?.join(path)
    };
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(RpcError::invalid(
            "UNSAFE_PROFILE_PATH",
            "profile paths must not contain parent-directory components",
        ));
    }
    Ok(path)
}

fn canonical_real_directory(path: &Path, label: &str) -> RpcResult<PathBuf> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            RpcError::invalid(
                "PROFILE_NOT_FOUND",
                format!("{label} does not exist: {}", path.display()),
            )
        } else {
            RpcError::io(error)
        }
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(RpcError::invalid(
            "UNSAFE_PROFILE_PATH",
            format!("{label} must be a real directory: {}", path.display()),
        ));
    }
    fs::canonicalize(path).map_err(RpcError::io)
}

fn read_aseprite_version(user_config: &Path) -> Option<String> {
    let data = fs::read_to_string(user_config.join("aseprite.ini")).ok()?;
    aseprite_version_from_ini(&data)
}

fn aseprite_version_from_ini(data: &str) -> Option<String> {
    let mut section = "";
    for raw in data.lines() {
        let line = raw.trim();
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim();
            continue;
        }
        if !matches!(section, "updater" | "updates") {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "current_version" {
            continue;
        }
        let version: String = value
            .trim()
            .chars()
            .take_while(|character| character.is_ascii_digit() || *character == '.')
            .collect();
        return valid_aseprite_version(&version).then_some(version);
    }
    None
}

fn valid_aseprite_version(value: &str) -> bool {
    !value.is_empty()
        && value.split('.').all(|part| {
            !part.is_empty() && part.chars().all(|character| character.is_ascii_digit())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_current_version_without_platform_suffix() {
        let ini = "[general]\nfoo = bar\n[updater]\ncurrent_version = 1.3.18.3-arm64\n";
        assert_eq!(aseprite_version_from_ini(ini).as_deref(), Some("1.3.18.3"));
    }

    #[test]
    fn rejects_incomplete_versions() {
        assert!(!valid_aseprite_version("1.3."));
        assert!(!valid_aseprite_version("latest"));
    }

    #[test]
    fn explicit_profile_does_not_evaluate_the_platform_default() {
        let expected = PathBuf::from("explicit-profile");
        let selected = choose_user_config(Some(expected.clone()), None, || {
            panic!("platform default must stay lazy")
        })
        .expect("explicit profile");

        assert_eq!(selected, expected);
    }
}
