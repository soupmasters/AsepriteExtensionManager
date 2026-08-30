mod profile;

use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

use aem_helper::github::{GitHubClient, ResolveOptions, ResolveResult};
use aem_helper::installation;
use aem_helper::installed;
use aem_helper::package::{self, ExpectedManifest, PreparedPackage};
use aem_helper::protocol::{RpcError, RpcResult};
use aem_helper::registry::{release_supports, CatalogPackage, CatalogRelease, RegistryClient};
use aem_helper::state::State;
use aem_helper::{tooling, VERSION};
use chrono::Utc;
use clap::{Parser, Subcommand};
use profile::{ProfilePaths, MANAGER_NAME};
use semver::Version;
use serde_json::Value;

const MANAGER_DISPLAY_NAME: &str = "Aseprite Extension Manager";
#[cfg(target_os = "macos")]
const MANAGER_HELPER_PATH: &str = "bin/macos/aem-helper";
#[cfg(target_os = "windows")]
const MANAGER_HELPER_PATH: &str = "bin/windows/aem-helper.exe";
#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
const MANAGER_HELPER_PATH: &str = "bin/linux/aem-helper";

#[derive(Debug, Parser)]
#[command(
    name = "aem",
    version = VERSION,
    about = "Install and inspect Aseprite extensions"
)]
struct Cli {
    /// Aseprite user configuration directory.
    #[arg(long, global = true, value_name = "PATH")]
    user_config: Option<PathBuf>,

    /// Installed Aseprite Extension Manager directory.
    #[arg(long, global = true, value_name = "PATH", hide = true)]
    extension_root: Option<PathBuf>,

    /// Override the detected Aseprite version for compatibility checks.
    #[arg(long, global = true, value_name = "VERSION")]
    aseprite_version: Option<String>,

    /// Override the Aseprite scripting API version for compatibility checks.
    #[arg(long, global = true, value_name = "NUMBER")]
    api_version: Option<u32>,

    /// Aseprite executable to use when installing into a custom or portable profile.
    #[arg(long, global = true, value_name = "PATH")]
    aseprite: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Install a catalog package, GitHub repository, or local extension.
    Install {
        /// Catalog ID, GitHub URL, folder, package.json, or .aseprite-extension file.
        source: String,

        /// Release asset name or ID when a GitHub release has several packages.
        #[arg(long, value_name = "NAME_OR_ID")]
        asset: Option<String>,

        /// Resolve and validate the package without opening Aseprite.
        #[arg(long)]
        prepare_only: bool,
    },

    /// List extensions installed in the selected Aseprite profile.
    List,

    /// Search the trusted extension catalog.
    Search { query: String },

    /// Check the profile, catalog, Git, and GitHub CLI.
    Doctor,
}

pub async fn run() -> RpcResult<()> {
    let cli = Cli::parse();
    let custom_profile = cli.user_config.is_some()
        || std::env::var_os("ASEPRITE_USER_FOLDER").is_some_and(|value| !value.is_empty());
    let aseprite_executable = resolve_aseprite_executable(cli.aseprite)?;
    let paths = ProfilePaths::resolve(
        cli.user_config,
        cli.extension_root,
        cli.aseprite_version,
        cli.api_version,
    )?;
    match cli.command {
        Command::Install {
            source,
            asset,
            prepare_only,
        } => {
            if !prepare_only {
                require_interactive_install()?;
                if custom_profile && aseprite_executable.is_none() {
                    return Err(RpcError::invalid(
                        "ASEPRITE_EXECUTABLE_REQUIRED",
                        "installing into a custom profile also requires --aseprite PATH so the package opens in the matching Aseprite installation",
                    ));
                }
            }
            install_with(
                &paths,
                &source,
                asset,
                prepare_only,
                &SystemInstaller {
                    executable: aseprite_executable,
                    user_config: paths.user_config.clone(),
                },
            )
            .await
        }
        Command::List => list(&paths),
        Command::Search { query } => search(&paths, &query),
        Command::Doctor => doctor(&paths).await,
    }
}

trait NativeInstaller {
    fn install(&self, artifact: &Path, package: &PreparedPackage) -> RpcResult<()>;
}

struct SystemInstaller {
    executable: Option<PathBuf>,
    user_config: PathBuf,
}

impl NativeInstaller for SystemInstaller {
    fn install(&self, artifact: &Path, package: &PreparedPackage) -> RpcResult<()> {
        require_interactive_install()?;
        if let Some(executable) = &self.executable {
            std::process::Command::new(executable)
                .env("ASEPRITE_USER_FOLDER", &self.user_config)
                .arg(artifact)
                .spawn()
                .map_err(|error| {
                    RpcError::invalid(
                        "ASEPRITE_OPEN_FAILED",
                        format!("could not start the selected Aseprite executable: {error}"),
                    )
                })?;
        } else {
            opener::open(artifact).map_err(|error| {
                RpcError::invalid(
                    "ASEPRITE_OPEN_FAILED",
                    format!("could not open the prepared package in Aseprite: {error}"),
                )
            })?;
        }
        let name = safe_terminal_text(display_name(package));
        let version = safe_terminal_text(&package.version);
        print!(
            "Confirm {} {} in Aseprite, then press Enter here to verify it... ",
            name, version
        );
        io::stdout().flush().map_err(RpcError::io)?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer).map_err(RpcError::io)?;
        Ok(())
    }
}

pub fn safe_terminal_text(value: &str) -> String {
    let mut safe = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_control()
            || matches!(character, '\u{061c}' | '\u{200e}' | '\u{200f}')
            || ('\u{202a}'..='\u{202e}').contains(&character)
            || ('\u{2066}'..='\u{2069}').contains(&character)
        {
            safe.extend(character.escape_default());
        } else {
            safe.push(character);
        }
    }
    safe
}

fn resolve_aseprite_executable(path: Option<PathBuf>) -> RpcResult<Option<PathBuf>> {
    let Some(mut path) = path else {
        return Ok(None);
    };
    #[cfg(target_os = "macos")]
    if path.extension().and_then(|extension| extension.to_str()) == Some("app") {
        path = path.join("Contents/MacOS/aseprite");
    }
    let path = fs::canonicalize(&path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
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
    Ok(Some(path))
}

fn require_interactive_install() -> RpcResult<()> {
    if io::stdin().is_terminal() {
        Ok(())
    } else {
        Err(RpcError::invalid(
            "INTERACTIVE_INSTALL_REQUIRED",
            "installation requires a terminal and Aseprite confirmation; use --prepare-only in automation",
        ))
    }
}

struct PreparedInstall {
    package: PreparedPackage,
    source: Value,
}

async fn install_with(
    paths: &ProfilePaths,
    requested: &str,
    asset: Option<String>,
    prepare_only: bool,
    installer: &dyn NativeInstaller,
) -> RpcResult<()> {
    let (state, state_lock) = State::new_locked(&paths.user_config)?;
    let prepared = prepare_source(paths, &state, requested, asset).await?;
    reject_manager_package(&prepared.package)?;
    let prepared_name = safe_terminal_text(display_name(&prepared.package));
    let prepared_version = safe_terminal_text(&prepared.package.version);
    println!("Prepared {} {}.", prepared_name, prepared_version);
    if prepare_only {
        println!(
            "{}",
            safe_terminal_text(&prepared.package.artifact_path.to_string_lossy())
        );
        return Ok(());
    }

    drop(state_lock);
    installer.install(&prepared.package.artifact_path, &prepared.package)?;
    let (state, _state_lock) = acquire_verification_lock(&paths.user_config)?;
    let name = prepared.package.name.clone();
    let version = prepared.package.version.clone();
    let result = installation::verify_and_record(
        &paths.user_config,
        &state,
        prepared.package,
        prepared.source,
    )?;
    if result.get("verified").and_then(Value::as_bool) != Some(true) {
        return Err(RpcError::invalid(
            "INSTALL_VERIFICATION_FAILED",
            result
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Aseprite did not install the prepared package"),
        ));
    }
    println!(
        "Installed {} {}. Restart Aseprite before using it.",
        safe_terminal_text(&name),
        safe_terminal_text(&version)
    );
    Ok(())
}

fn acquire_verification_lock(
    user_config: &Path,
) -> RpcResult<(State, aem_helper::state::StateLock)> {
    loop {
        match State::new_locked(user_config) {
            Ok(locked) => return Ok(locked),
            Err(error) if error.code == "PROFILE_BUSY" && io::stdin().is_terminal() => {
                print!(
                    "The manager is still using this profile. Finish its prompt or close its window, then press Enter to retry... "
                );
                io::stdout().flush().map_err(RpcError::io)?;
                let mut answer = String::new();
                io::stdin().read_line(&mut answer).map_err(RpcError::io)?;
            }
            Err(error) => return Err(error),
        }
    }
}

async fn prepare_source(
    paths: &ProfilePaths,
    state: &State,
    requested: &str,
    asset: Option<String>,
) -> RpcResult<PreparedInstall> {
    let path = PathBuf::from(requested);
    if path.exists() {
        return prepare_local_path(state, &path);
    }
    if requested.starts_with("https://") || requested.starts_with("http://") {
        return prepare_github(state, requested, asset).await;
    }
    if looks_like_path(requested) {
        return Err(RpcError::invalid(
            "LOCAL_SOURCE_NOT_FOUND",
            format!("local extension path does not exist: {requested}"),
        ));
    }
    prepare_catalog(paths, state, requested).await
}

fn prepare_local_path(state: &State, path: &Path) -> RpcResult<PreparedInstall> {
    let canonical = fs::canonicalize(path).map_err(RpcError::io)?;
    if canonical.is_dir() {
        let package_json = canonical.join("package.json");
        let package = package::package_local_folder(state, &package_json)?;
        let source = local_source(&package_json, &package);
        return Ok(PreparedInstall { package, source });
    }
    if canonical.file_name().and_then(|name| name.to_str()) == Some("package.json") {
        let package = package::package_local_folder(state, &canonical)?;
        let source = local_source(&canonical, &package);
        return Ok(PreparedInstall { package, source });
    }
    if canonical
        .extension()
        .and_then(|extension| extension.to_str())
        == Some("aseprite-extension")
    {
        let package = package::validate_and_stage(state, &canonical, ExpectedManifest::default())?;
        let source = serde_json::json!({
            "kind": "direct",
            "path": canonical,
        });
        return Ok(PreparedInstall { package, source });
    }
    Err(RpcError::invalid(
        "INVALID_LOCAL_SOURCE",
        "use an extension folder, package.json, or .aseprite-extension file",
    ))
}

fn local_source(package_json: &Path, package: &PreparedPackage) -> Value {
    serde_json::json!({
        "kind": "local",
        "packageJsonPath": package_json,
        "contentHash": package.content_hash,
    })
}

async fn prepare_github(
    state: &State,
    url: &str,
    selection: Option<String>,
) -> RpcResult<PreparedInstall> {
    let client = GitHubClient::new(state.clone())?;
    match client
        .resolve(ResolveOptions {
            url: url.to_owned(),
            selection,
        })
        .await?
    {
        ResolveResult::Ready { package, source } => Ok(PreparedInstall {
            package: *package,
            source: serde_json::to_value(*source)
                .map_err(|error| RpcError::internal(error.to_string()))?,
        }),
        ResolveResult::SelectionRequired {
            repository,
            release,
            choices,
        } => {
            let choices = choices
                .iter()
                .map(|choice| format!("{} ({})", choice.name, choice.id))
                .collect::<Vec<_>>()
                .join(", ");
            Err(RpcError::invalid(
                "ASSET_SELECTION_REQUIRED",
                format!(
                    "{repository} release {release} has several extension assets: {choices}. Retry with --asset NAME_OR_ID"
                ),
            ))
        }
    }
}

async fn prepare_catalog(
    paths: &ProfilePaths,
    state: &State,
    requested: &str,
) -> RpcResult<PreparedInstall> {
    ensure_registry(&paths.manager_root)?;
    let registry = RegistryClient::new(state.clone(), &paths.manager_root);
    let view = registry.refresh(Utc::now())?;
    if view.expired {
        return Err(RpcError::invalid(
            "REGISTRY_EXPIRED",
            "expired registry metadata cannot authorize an installation",
        ));
    }
    let catalog_package = find_catalog_package(&view.packages, requested)?;
    let release = newest_release(catalog_package, &paths.aseprite_version, paths.api_version)
        .ok_or_else(|| {
            RpcError::invalid(
                "NO_COMPATIBLE_RELEASE",
                format!(
                    "{} has no release compatible with Aseprite {} and API {}",
                    catalog_package.display_name, paths.aseprite_version, paths.api_version
                ),
            )
        })?;
    let client = GitHubClient::new(state.clone())?;
    let package = client
        .prepare_authenticated_asset(
            &release.asset.url,
            &release.asset.sha256,
            release.asset.byte_length,
            &catalog_package.manifest_name,
            &release.version,
            release.asset.commit.as_deref(),
        )
        .await?;
    let source = serde_json::json!({
        "kind": "registry",
        "packageId": catalog_package.id,
        "repository": catalog_package.repository,
        "immutableUrl": release.asset.url,
        "release": release.asset.release_tag,
        "assetId": release.asset.asset_id,
        "commit": release.asset.commit,
    });
    Ok(PreparedInstall { package, source })
}

fn list(paths: &ProfilePaths) -> RpcResult<()> {
    let state = State::open_existing(&paths.user_config)?;
    let packages =
        installed::scan_with_manager_root(&paths.user_config, &state, &paths.manager_root)?;
    let packages: Vec<_> = packages
        .into_iter()
        .filter(|package| !package.name.eq_ignore_ascii_case(MANAGER_NAME))
        .collect();
    if packages.is_empty() {
        println!("No extensions installed.");
        return Ok(());
    }
    for package in packages {
        let status = if package.managed {
            "managed"
        } else {
            "unmanaged"
        };
        let name = package.display_name.as_deref().unwrap_or(&package.name);
        println!(
            "{} {} [{status}]",
            safe_terminal_text(name),
            safe_terminal_text(&package.version)
        );
    }
    Ok(())
}

fn search(paths: &ProfilePaths, query: &str) -> RpcResult<()> {
    ensure_registry(&paths.manager_root)?;
    let temporary = tempfile::tempdir().map_err(RpcError::io)?;
    let state = State::new(temporary.path())?;
    let view = RegistryClient::new(state, &paths.manager_root).refresh(Utc::now())?;
    if view.expired {
        eprintln!("Warning: the trusted catalog metadata has expired.");
    }
    let folded = query.to_lowercase();
    let mut packages: Vec<_> = view
        .packages
        .iter()
        .filter(|package| package_matches(package, &folded))
        .collect();
    packages.sort_by(|left, right| {
        left.display_name
            .to_lowercase()
            .cmp(&right.display_name.to_lowercase())
    });
    if packages.is_empty() {
        println!("No catalog extensions match {query:?}.");
        return Ok(());
    }
    for package in packages {
        let version = newest_release(package, &paths.aseprite_version, paths.api_version)
            .map(|release| release.version.as_str())
            .unwrap_or("incompatible");
        println!(
            "{}  {}  {}  {}",
            safe_terminal_text(&package.id),
            safe_terminal_text(version),
            safe_terminal_text(&package.display_name),
            safe_terminal_text(&package.summary)
        );
    }
    Ok(())
}

async fn doctor(paths: &ProfilePaths) -> RpcResult<()> {
    println!("Aseprite Extension Manager {VERSION}");
    println!(
        "[ok] Aseprite profile: {}",
        safe_terminal_text(&paths.user_config.to_string_lossy())
    );
    let mut failed = false;

    match inspect_manager_install(&paths.manager_root) {
        Ok(version) => println!(
            "[ok] Manager extension: {} ({version})",
            safe_terminal_text(&paths.manager_root.to_string_lossy())
        ),
        Err(error) => {
            failed = true;
            println!(
                "[fail] Manager extension: {}",
                safe_terminal_text(&error.message)
            );
        }
    }

    match aem_helper::registry::validate_bundled_repository(&paths.manager_root.join("registry")) {
        Ok(()) => println!("[ok] Trusted catalog"),
        Err(error) => {
            failed = true;
            println!(
                "[fail] Trusted catalog: {}",
                safe_terminal_text(&error.message)
            );
        }
    }

    let state = State::open_existing(&paths.user_config)?;
    match installed::scan_with_manager_root(&paths.user_config, &state, &paths.manager_root) {
        Ok(packages) => println!(
            "[ok] Installed extensions: {}",
            packages
                .iter()
                .filter(|package| !package.name.eq_ignore_ascii_case(MANAGER_NAME))
                .count()
        ),
        Err(error) => {
            failed = true;
            println!(
                "[fail] Installed extensions: {}",
                safe_terminal_text(&error.message)
            );
        }
    }

    let tools = serde_json::to_value(tooling::diagnostics().await)
        .map_err(|error| RpcError::internal(error.to_string()))?;
    print_tool("Git", tools.get("git"));
    print_tool("GitHub CLI", tools.get("gh"));
    println!(
        "[ok] Compatibility target: Aseprite {}, API {}",
        paths.aseprite_version, paths.api_version
    );
    println!("[ok] Telemetry: disabled");
    if failed {
        Err(RpcError::invalid(
            "DOCTOR_FAILED",
            "one or more required checks failed",
        ))
    } else {
        Ok(())
    }
}

fn inspect_manager_install(manager_root: &Path) -> RpcResult<String> {
    let manifest_path = manager_root.join("package.json");
    require_regular_file(&manifest_path, "manager manifest")?;
    let manifest: package::Manifest =
        serde_json::from_slice(&fs::read(&manifest_path).map_err(RpcError::io)?)
            .map_err(|error| RpcError::invalid("INVALID_MANAGER_MANIFEST", error.to_string()))?;
    if manifest.name != MANAGER_NAME
        || manifest.display_name.as_deref() != Some(MANAGER_DISPLAY_NAME)
    {
        return Err(RpcError::invalid(
            "INVALID_MANAGER_MANIFEST",
            "manager manifest identity is invalid",
        ));
    }
    let version = Version::parse(&manifest.version).map_err(|_| {
        RpcError::invalid(
            "INVALID_MANAGER_MANIFEST",
            "manager version must use stable semantic versioning",
        )
    })?;
    if !version.pre.is_empty()
        || !version.build.is_empty()
        || version.to_string() != manifest.version
    {
        return Err(RpcError::invalid(
            "INVALID_MANAGER_MANIFEST",
            "manager version must use stable semantic versioning",
        ));
    }
    require_regular_file(&manager_root.join("main.lua"), "manager entry point")?;
    require_regular_file(
        &manager_root.join(MANAGER_HELPER_PATH),
        "manager helper for this platform",
    )?;
    Ok(manifest.version)
}

fn require_regular_file(path: &Path, label: &str) -> RpcResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            RpcError::invalid(
                "MANAGER_FILE_MISSING",
                format!("{label} is missing: {}", path.display()),
            )
        } else {
            RpcError::io(error)
        }
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(RpcError::invalid(
            "INVALID_MANAGER_FILE",
            format!("{label} must be a real file: {}", path.display()),
        ));
    }
    Ok(())
}

fn print_tool(name: &str, value: Option<&Value>) {
    let installed = value
        .and_then(|value| value.get("installed"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !installed {
        println!("[--] {name}: not installed");
        return;
    }
    let version = safe_terminal_text(
        value
            .and_then(|value| value.get("version"))
            .and_then(Value::as_str)
            .unwrap_or("installed"),
    );
    let auth = value
        .and_then(|value| value.get("authenticated"))
        .and_then(Value::as_bool);
    match auth {
        Some(true) => println!("[ok] {name}: {version}, signed in"),
        Some(false) => println!("[--] {name}: {version}, not signed in"),
        None => println!("[ok] {name}: {version}"),
    }
}

fn ensure_registry(manager_root: &Path) -> RpcResult<()> {
    if manager_root.join("registry/root.json").is_file() {
        Ok(())
    } else {
        Err(RpcError::invalid(
            "REGISTRY_NOT_FOUND",
            format!(
                "trusted catalog not found under {}; install Aseprite Extension Manager or pass --extension-root",
                manager_root.display()
            ),
        ))
    }
}

fn find_catalog_package<'a>(
    packages: &'a [CatalogPackage],
    requested: &str,
) -> RpcResult<&'a CatalogPackage> {
    let requested_key = alias_key(requested);
    let mut matches: Vec<_> = packages
        .iter()
        .filter(|package| {
            package.id.eq_ignore_ascii_case(requested)
                || package.manifest_name.eq_ignore_ascii_case(requested)
                || alias_key(&package.id) == requested_key
                || alias_key(&package.manifest_name) == requested_key
                || alias_key(&package.display_name) == requested_key
                || repository_alias(package).is_some_and(|alias| alias_key(alias) == requested_key)
        })
        .collect();
    matches.sort_by(|left, right| left.id.cmp(&right.id));
    matches.dedup_by(|left, right| left.id == right.id);
    match matches.as_slice() {
        [package] => Ok(*package),
        [] => Err(RpcError::invalid(
            "CATALOG_PACKAGE_NOT_FOUND",
            format!("{requested:?} is not in the trusted catalog; try `aem search {requested}`"),
        )),
        _ => Err(RpcError::invalid(
            "AMBIGUOUS_CATALOG_PACKAGE",
            format!(
                "{requested:?} matches several catalog packages: {}",
                matches
                    .iter()
                    .map(|package| package.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
    }
}

fn newest_release<'a>(
    package: &'a CatalogPackage,
    aseprite_version: &str,
    api_version: u32,
) -> Option<&'a CatalogRelease> {
    package
        .releases
        .iter()
        .filter(|release| !release.yanked)
        .filter(|release| release_supports(release, aseprite_version, api_version))
        .filter_map(|release| {
            Version::parse(&release.version)
                .ok()
                .map(|version| (version, release))
        })
        .max_by(|left, right| left.0.cmp(&right.0))
        .map(|(_, release)| release)
}

fn package_matches(package: &CatalogPackage, folded_query: &str) -> bool {
    let author = package
        .author
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    [
        package.id.as_str(),
        package.manifest_name.as_str(),
        package.display_name.as_str(),
        package.summary.as_str(),
        package.license.as_str(),
        package.homepage.as_str(),
        package.repository.as_str(),
        author,
    ]
    .iter()
    .any(|value| value.to_lowercase().contains(folded_query))
}

fn repository_alias(package: &CatalogPackage) -> Option<&str> {
    package
        .repository
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .map(|value| value.strip_suffix(".git").unwrap_or(value))
}

fn alias_key(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn looks_like_path(value: &str) -> bool {
    !value.contains("://")
        && (value.starts_with('.')
            || value.starts_with('/')
            || value.contains(std::path::MAIN_SEPARATOR)
            || value.ends_with("package.json")
            || value.ends_with(".aseprite-extension"))
}

fn reject_manager_package(package: &PreparedPackage) -> RpcResult<()> {
    if package.name.eq_ignore_ascii_case(MANAGER_NAME) {
        Err(RpcError::invalid(
            "SELF_UPDATE_RESTRICTED",
            "Aseprite Extension Manager updates itself from inside Aseprite",
        ))
    } else {
        Ok(())
    }
}

fn display_name(package: &PreparedPackage) -> &str {
    package.display_name.as_deref().unwrap_or(&package.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use zip::ZipArchive;

    struct FakeAsepriteInstaller {
        user_config: PathBuf,
    }

    impl NativeInstaller for FakeAsepriteInstaller {
        fn install(&self, artifact: &Path, package: &PreparedPackage) -> RpcResult<()> {
            let (_state, _lock) = State::new_locked(&self.user_config)?;
            let destination = self.user_config.join("extensions").join(&package.name);
            fs::create_dir_all(&destination).map_err(RpcError::io)?;
            let mut archive = ZipArchive::new(File::open(artifact).map_err(RpcError::io)?)
                .map_err(|error| RpcError::invalid("TEST_ARCHIVE_ERROR", error.to_string()))?;
            let mut installed_files = Vec::new();
            for index in 0..archive.len() {
                let mut entry = archive
                    .by_index(index)
                    .map_err(|error| RpcError::invalid("TEST_ARCHIVE_ERROR", error.to_string()))?;
                let enclosed = entry.enclosed_name().ok_or_else(|| {
                    RpcError::invalid("TEST_ARCHIVE_ERROR", "unsafe fixture path")
                })?;
                if entry.is_dir() {
                    fs::create_dir_all(destination.join(&enclosed)).map_err(RpcError::io)?;
                    continue;
                }
                let output = destination.join(&enclosed);
                if let Some(parent) = output.parent() {
                    fs::create_dir_all(parent).map_err(RpcError::io)?;
                }
                let mut file = File::create(&output).map_err(RpcError::io)?;
                io::copy(&mut entry, &mut file).map_err(RpcError::io)?;
                installed_files.push(enclosed.to_string_lossy().replace('\\', "/"));
            }
            fs::write(
                destination.join("__info.json"),
                serde_json::to_vec(&serde_json::json!({
                    "installedFiles": installed_files
                }))
                .map_err(|error| RpcError::internal(error.to_string()))?,
            )
            .map_err(RpcError::io)
        }
    }

    #[test]
    fn repository_slug_is_a_catalog_install_alias() {
        let package = CatalogPackage {
            id: "unity-animation-event".to_owned(),
            manifest_name: "unity-animation-event".to_owned(),
            display_name: "Unity Event for Aseprite".to_owned(),
            summary: String::new(),
            author: serde_json::json!({"name":"Martin Calander"}),
            license: "MIT".to_owned(),
            homepage: String::new(),
            repository: "https://github.com/soupmasters/UnityEventForAseprite".to_owned(),
            releases: Vec::new(),
        };
        let packages = [package];
        let found = find_catalog_package(&packages, "unity-event-for-aseprite").unwrap();
        assert_eq!(found.id, "unity-animation-event");
    }

    #[test]
    fn path_like_missing_sources_do_not_fall_through_to_the_catalog() {
        assert!(looks_like_path("./my-local-extension"));
        assert!(looks_like_path("thing.aseprite-extension"));
        assert!(!looks_like_path("unity-event-for-aseprite"));
        assert!(!looks_like_path(
            "https://github.com/soupmasters/UnityEventForAseprite"
        ));
    }

    #[test]
    fn terminal_text_escapes_control_and_directional_characters() {
        assert_eq!(
            safe_terminal_text("Lyutria\n\u{1b}[31m\u{202e}"),
            "Lyutria\\n\\u{1b}[31m\\u{202e}"
        );
    }

    #[test]
    fn manager_health_check_requires_identity_and_runtime_files() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        fs::write(
            root.join("package.json"),
            br#"{"name":"not-the-manager","displayName":"Other","version":"1.0.0"}"#,
        )
        .unwrap();
        assert!(inspect_manager_install(root).is_err());

        fs::write(
            root.join("package.json"),
            br#"{"name":"aseprite-extension-manager","displayName":"Aseprite Extension Manager","version":"1.0.0"}"#,
        )
        .unwrap();
        fs::write(root.join("main.lua"), b"return true\n").unwrap();
        let helper = root.join(MANAGER_HELPER_PATH);
        fs::create_dir_all(helper.parent().unwrap()).unwrap();
        fs::write(helper, b"helper").unwrap();

        assert_eq!(inspect_manager_install(root).unwrap(), "1.0.0");
    }

    #[tokio::test]
    async fn local_install_runs_the_complete_verified_receipt_flow() {
        let temporary = tempfile::tempdir().unwrap();
        let user_config = temporary.path().join("profile");
        fs::create_dir_all(&user_config).unwrap();
        let source = temporary.path().join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(
            source.join("package.json"),
            br#"{"name":"cli-local","displayName":"CLI Local","version":"1.0.0","main":"main.lua"}"#,
        )
        .unwrap();
        fs::write(source.join("main.lua"), b"return true\n").unwrap();
        let paths = ProfilePaths {
            user_config: user_config.clone(),
            manager_root: user_config.join("extensions").join(MANAGER_NAME),
            aseprite_version: "1.3.15".to_owned(),
            api_version: 35,
        };
        let installer = FakeAsepriteInstaller {
            user_config: user_config.clone(),
        };

        install_with(&paths, source.to_str().unwrap(), None, false, &installer)
            .await
            .unwrap();

        assert!(user_config.join("extensions/cli-local/main.lua").is_file());
        let state = State::open_existing(&user_config).unwrap();
        let receipt = state.read_receipt("cli-local").unwrap().unwrap();
        assert_eq!(receipt.source_kind, "local");
        assert_eq!(receipt.installed_version, "1.0.0");
        assert!(state.cached_artifact("cli-local", false).unwrap().is_some());
    }
}
