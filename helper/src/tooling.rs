use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

const MAX_COMMAND_OUTPUT_BYTES: u64 = 4096;
const MAX_VERSION_BYTES: usize = 80;
const VERSION_TIMEOUT: Duration = Duration::from_secs(2);
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(4);
const AUTH_TIMEOUT: Duration = Duration::from_secs(4);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Tool {
    Git,
    Gh,
}

impl Tool {
    fn executable_name(self) -> &'static str {
        match self {
            Self::Git => executable_name("git"),
            Self::Gh => executable_name("gh"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolStatus {
    installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    authenticated: Option<bool>,
}

impl ToolStatus {
    fn missing() -> Self {
        Self {
            installed: false,
            version: None,
            authenticated: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ToolingDiagnostics {
    git: ToolStatus,
    gh: ToolStatus,
}

struct LocatedTool {
    path: PathBuf,
    version: Option<String>,
}

struct CommandOutput {
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

pub async fn diagnostics() -> ToolingDiagnostics {
    let (git, gh) = tokio::join!(status(Tool::Git), status(Tool::Gh));
    ToolingDiagnostics { git, gh }
}

async fn status(tool: Tool) -> ToolStatus {
    let Some(located) = locate(tool).await else {
        return ToolStatus::missing();
    };

    let authenticated = if tool == Tool::Gh {
        match run_command(
            &located.path,
            &[
                "api",
                "--hostname",
                "github.com",
                "--method",
                "GET",
                "--silent",
                "user",
            ],
            AUTH_TIMEOUT,
            false,
        )
        .await
        {
            Ok(output) => Some(output.success),
            Err(()) => None,
        }
    } else {
        None
    };

    ToolStatus {
        installed: true,
        version: located.version,
        authenticated,
    }
}

pub async fn find(tool: Tool) -> Option<PathBuf> {
    locate(tool).await.map(|located| located.path)
}

fn candidates(tool: Tool) -> Vec<PathBuf> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    let executable = tool.executable_name();

    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            if directory.is_absolute() {
                push_candidate(&mut result, &mut seen, directory.join(executable));
            }
        }
    }

    add_platform_candidates(tool, &mut result, &mut seen);
    result
}

async fn locate(tool: Tool) -> Option<LocatedTool> {
    tokio::time::timeout(
        DISCOVERY_TIMEOUT,
        locate_with_candidates(tool, candidates(tool)),
    )
    .await
    .ok()
    .flatten()
}

async fn locate_with_candidates(tool: Tool, candidates: Vec<PathBuf>) -> Option<LocatedTool> {
    for candidate in candidates {
        if !tokio::fs::metadata(&candidate)
            .await
            .is_ok_and(|metadata| metadata.is_file())
            || !candidate_is_safe_to_probe(tool, &candidate).await
        {
            continue;
        }
        let Ok(output) = run_command(&candidate, &["--version"], VERSION_TIMEOUT, true).await
        else {
            continue;
        };
        if !output.success {
            continue;
        }
        let path = tokio::fs::canonicalize(&candidate)
            .await
            .unwrap_or(candidate);
        return Some(LocatedTool {
            path,
            version: first_version(tool, &output.stdout, &output.stderr),
        });
    }
    None
}

async fn run_command(
    executable: &Path,
    arguments: &[&str],
    timeout: Duration,
    capture_output: bool,
) -> Result<CommandOutput, ()> {
    let mut command = Command::new(executable);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "Never")
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("GH_NO_EXTENSION_UPDATE_NOTIFIER", "1")
        .env("NO_COLOR", "1")
        .kill_on_drop(true);

    if capture_output {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
    } else {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }

    #[cfg(windows)]
    command.as_std_mut().creation_flags(CREATE_NO_WINDOW);

    let mut child = command.spawn().map_err(|_| ())?;
    let stdout_task = child
        .stdout
        .take()
        .map(|stream| tokio::spawn(read_bounded(stream)));
    let stderr_task = child
        .stderr
        .take()
        .map(|stream| tokio::spawn(read_bounded(stream)));

    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => Some(status),
        Ok(Err(_)) => None,
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            None
        }
    };
    let stdout = collect_output(stdout_task).await;
    let stderr = collect_output(stderr_task).await;
    let status = status.ok_or(())?;
    Ok(CommandOutput {
        success: status.success(),
        stdout,
        stderr,
    })
}

async fn read_bounded<R>(reader: R) -> std::io::Result<Vec<u8>>
where
    R: AsyncRead + Unpin,
{
    let mut bytes = Vec::new();
    reader
        .take(MAX_COMMAND_OUTPUT_BYTES + 1)
        .read_to_end(&mut bytes)
        .await?;
    bytes.truncate(MAX_COMMAND_OUTPUT_BYTES as usize);
    Ok(bytes)
}

async fn collect_output(
    task: Option<tokio::task::JoinHandle<std::io::Result<Vec<u8>>>>,
) -> Vec<u8> {
    match task {
        Some(task) => task.await.ok().and_then(Result::ok).unwrap_or_default(),
        None => Vec::new(),
    }
}

fn first_version_line(stdout: &[u8], stderr: &[u8]) -> Option<String> {
    [stdout, stderr].into_iter().find_map(|bytes| {
        String::from_utf8_lossy(bytes)
            .lines()
            .map(sanitize_version)
            .find(|line| !line.is_empty())
    })
}

fn first_version(tool: Tool, stdout: &[u8], stderr: &[u8]) -> Option<String> {
    let line = first_version_line(stdout, stderr)?;
    let prefix = match tool {
        Tool::Git => "git version ",
        Tool::Gh => "gh version ",
    };
    let value = line
        .strip_prefix(prefix)
        .and_then(|value| value.split_whitespace().next())
        .unwrap_or(&line);
    Some(sanitize_version(value))
}

fn sanitize_version(value: &str) -> String {
    let mut result = String::new();
    for character in value.trim().chars() {
        let character = if character.is_control() {
            ' '
        } else {
            character
        };
        if result.len() + character.len_utf8() > MAX_VERSION_BYTES {
            break;
        }
        result.push(character);
    }
    result.trim().to_owned()
}

fn push_candidate(result: &mut Vec<PathBuf>, seen: &mut HashSet<OsString>, path: PathBuf) {
    let key = candidate_key(&path);
    if seen.insert(key) {
        result.push(path);
    }
}

fn candidate_key(path: &Path) -> OsString {
    #[cfg(windows)]
    {
        return path.as_os_str().to_string_lossy().to_lowercase().into();
    }
    #[cfg(not(windows))]
    {
        path.as_os_str().to_owned()
    }
}

#[cfg(windows)]
fn executable_name(stem: &'static str) -> &'static str {
    match stem {
        "git" => "git.exe",
        "gh" => "gh.exe",
        _ => stem,
    }
}

#[cfg(not(windows))]
fn executable_name(stem: &'static str) -> &'static str {
    stem
}

#[cfg(target_os = "macos")]
fn add_platform_candidates(tool: Tool, result: &mut Vec<PathBuf>, seen: &mut HashSet<OsString>) {
    let executable = tool.executable_name();
    for directory in [
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/opt/local/bin",
        "/usr/bin",
    ] {
        push_candidate(result, seen, Path::new(directory).join(executable));
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn add_platform_candidates(tool: Tool, result: &mut Vec<PathBuf>, seen: &mut HashSet<OsString>) {
    let executable = tool.executable_name();
    for directory in ["/usr/local/bin", "/usr/bin", "/bin", "/snap/bin"] {
        push_candidate(result, seen, Path::new(directory).join(executable));
    }
}

#[cfg(windows)]
fn add_platform_candidates(tool: Tool, result: &mut Vec<PathBuf>, seen: &mut HashSet<OsString>) {
    let mut roots = Vec::new();
    for name in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
        if let Some(root) = std::env::var_os(name) {
            roots.push(PathBuf::from(root));
        }
    }
    for root in roots {
        match tool {
            Tool::Git => {
                push_candidate(result, seen, root.join("Git/cmd/git.exe"));
                push_candidate(result, seen, root.join("Git/bin/git.exe"));
            }
            Tool::Gh => {
                push_candidate(result, seen, root.join("GitHub CLI/gh.exe"));
            }
        }
    }

    if let Some(root) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        match tool {
            Tool::Git => push_candidate(result, seen, root.join("Programs/Git/cmd/git.exe")),
            Tool::Gh => {
                push_candidate(result, seen, root.join("Programs/GitHub CLI/gh.exe"));
                push_candidate(result, seen, root.join("Microsoft/WinGet/Links/gh.exe"));
            }
        }
    }
    if let Some(root) = std::env::var_os("USERPROFILE").map(PathBuf::from) {
        push_candidate(
            result,
            seen,
            root.join("scoop/shims").join(tool.executable_name()),
        );
    }
    if let Some(root) = std::env::var_os("ChocolateyInstall").map(PathBuf::from) {
        push_candidate(result, seen, root.join("bin").join(tool.executable_name()));
    }
}

#[cfg(not(any(unix, windows)))]
fn add_platform_candidates(_tool: Tool, _result: &mut Vec<PathBuf>, _seen: &mut HashSet<OsString>) {
}

#[cfg(target_os = "macos")]
async fn candidate_is_safe_to_probe(tool: Tool, candidate: &Path) -> bool {
    if tool != Tool::Git {
        return true;
    }
    let resolved = tokio::fs::canonicalize(candidate)
        .await
        .unwrap_or_else(|_| candidate.to_owned());
    if resolved != Path::new("/usr/bin/git") {
        return true;
    }
    run_command(
        Path::new("/usr/bin/xcode-select"),
        &["-p"],
        VERSION_TIMEOUT,
        false,
    )
    .await
    .is_ok_and(|output| output.success)
}

#[cfg(not(target_os = "macos"))]
async fn candidate_is_safe_to_probe(_tool: Tool, _candidate: &Path) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[test]
    fn version_text_is_single_line_clean_and_bounded() {
        let noisy = format!("\r\n  git version 2.50.0\t{}\nignored", "x".repeat(300));
        let version = first_version_line(noisy.as_bytes(), b"").expect("version");
        assert!(version.starts_with("git version 2.50.0"));
        assert!(version.len() <= MAX_VERSION_BYTES);
        assert!(!version.contains('\n'));
        assert!(!version.contains('\t'));
        assert_eq!(
            first_version(Tool::Git, b"git version 2.50.0.windows.1\n", b"").as_deref(),
            Some("2.50.0.windows.1")
        );
        assert_eq!(
            first_version(Tool::Gh, b"gh version 2.98.0 (2026-08-13)\n", b"").as_deref(),
            Some("2.98.0")
        );
    }

    #[tokio::test]
    async fn bounded_reader_never_keeps_more_than_the_limit() {
        let (mut writer, reader) = tokio::io::duplex(8192);
        let write = tokio::spawn(async move {
            let _ = writer
                .write_all(&vec![b'x'; MAX_COMMAND_OUTPUT_BYTES as usize + 512])
                .await;
        });
        let bytes = read_bounded(reader).await.expect("bounded read");
        let _ = write.await;
        assert_eq!(bytes.len(), MAX_COMMAND_OUTPUT_BYTES as usize);
    }

    #[test]
    fn candidates_are_unique_and_use_the_platform_executable_name() {
        let values = candidates(Tool::Git);
        let unique: HashSet<_> = values.iter().map(|path| candidate_key(path)).collect();
        assert_eq!(values.len(), unique.len());
        assert!(values.iter().all(
            |path| path.file_name() == Some(std::ffi::OsStr::new(Tool::Git.executable_name()))
        ));
        assert!(values.iter().all(|path| path.is_absolute()));
    }

    #[tokio::test]
    async fn git_status_is_internally_consistent_on_the_current_host() {
        let git = status(Tool::Git).await;
        if git.installed {
            assert!(git
                .version
                .as_deref()
                .is_some_and(|version| !version.is_empty()));
            assert!(find(Tool::Git).await.is_some_and(|path| path.is_absolute()));
        } else {
            assert_eq!(git.version, None);
            assert!(find(Tool::Git).await.is_none());
        }
        assert_eq!(git.authenticated, None);
    }

    #[test]
    fn missing_status_omits_optional_details() {
        let value = serde_json::to_value(ToolStatus::missing()).expect("serialize");
        assert_eq!(value, serde_json::json!({ "installed": false }));
    }

    #[test]
    fn diagnostics_schema_keeps_tool_and_authentication_status_separate() {
        let value = serde_json::to_value(ToolingDiagnostics {
            git: ToolStatus {
                installed: true,
                version: Some("git version 2.50.0".to_owned()),
                authenticated: None,
            },
            gh: ToolStatus {
                installed: true,
                version: Some("gh version 2.75.0".to_owned()),
                authenticated: Some(true),
            },
        })
        .expect("serialize");
        assert_eq!(
            value,
            serde_json::json!({
                "git": {
                    "installed": true,
                    "version": "git version 2.50.0"
                },
                "gh": {
                    "installed": true,
                    "version": "gh version 2.75.0",
                    "authenticated": true
                }
            })
        );
    }
}
