use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;

use aem_helper::server::{self, ServeOptions};
use aem_helper::state::State;
use aem_helper::VERSION;

#[cfg(windows)]
use std::io;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
use windows_sys::Win32::Foundation::{
    GetHandleInformation, SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE,
};
#[cfg(windows)]
use windows_sys::Win32::System::Console::{
    GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};

#[cfg(windows)]
struct StdHandleInheritanceGuard {
    changed: Vec<HANDLE>,
}

#[cfg(windows)]
impl StdHandleInheritanceGuard {
    fn clear() -> io::Result<Self> {
        // Stable Rust passes inheritable handles through CreateProcess. Temporarily
        // exclude the launcher's shell pipes so the detached server cannot keep
        // io.popen or Command::output waiting after the launcher exits.
        let mut guard = Self {
            changed: Vec::new(),
        };
        for identifier in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let handle = unsafe { GetStdHandle(identifier) };
            if handle.is_null() {
                continue;
            }
            if handle == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }
            let mut flags = 0_u32;
            if unsafe { GetHandleInformation(handle, &mut flags) } == 0 {
                return Err(io::Error::last_os_error());
            }
            if flags & HANDLE_FLAG_INHERIT == 0 {
                continue;
            }
            if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) } == 0 {
                return Err(io::Error::last_os_error());
            }
            guard.changed.push(handle);
        }
        Ok(guard)
    }
}

#[cfg(windows)]
impl Drop for StdHandleInheritanceGuard {
    fn drop(&mut self) {
        for handle in self.changed.iter().rev() {
            unsafe {
                SetHandleInformation(*handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);
            }
        }
    }
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("aem-helper: {}", error.message);
        std::process::exit(1);
    }
}

async fn run() -> Result<(), aem_helper::protocol::RpcError> {
    let mut arguments = std::env::args().skip(1);
    let command = arguments.next().unwrap_or_else(|| "help".to_owned());
    if matches!(command.as_str(), "--version" | "-V" | "version") {
        println!("aem-helper {VERSION}");
        return Ok(());
    }
    if command == "smoke" {
        println!(
            "{}",
            serde_json::json!({
                "ok": true,
                "protocol": aem_helper::PROTOCOL_VERSION,
                "version": VERSION
            })
        );
        return Ok(());
    }
    let options = parse_options(arguments.collect())?;
    match command.as_str() {
        "launch" => launch(options).await,
        "serve" => server::serve(options.into()).await,
        _ => Err(aem_helper::protocol::RpcError::invalid(
            "USAGE",
            "usage: aem-helper launch|serve --user-config <path> --extension-root <path>",
        )),
    }
}

#[derive(Debug)]
struct CliOptions {
    user_config: PathBuf,
    extension_root: PathBuf,
    idle_seconds: u64,
}

impl From<CliOptions> for ServeOptions {
    fn from(value: CliOptions) -> Self {
        Self {
            user_config: value.user_config,
            extension_root: value.extension_root,
            idle_seconds: value.idle_seconds,
        }
    }
}

fn parse_options(arguments: Vec<String>) -> Result<CliOptions, aem_helper::protocol::RpcError> {
    let mut user_config = None;
    let mut extension_root = None;
    let mut idle_seconds = 120_u64;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--user-config" => {
                index += 1;
                user_config = arguments.get(index).map(PathBuf::from);
            }
            "--extension-root" => {
                index += 1;
                extension_root = arguments.get(index).map(PathBuf::from);
            }
            "--idle-seconds" => {
                index += 1;
                idle_seconds = arguments
                    .get(index)
                    .ok_or_else(|| {
                        aem_helper::protocol::RpcError::invalid(
                            "USAGE",
                            "--idle-seconds requires a value",
                        )
                    })?
                    .parse()
                    .map_err(|_| {
                        aem_helper::protocol::RpcError::invalid(
                            "USAGE",
                            "--idle-seconds must be an integer",
                        )
                    })?;
            }
            argument => {
                return Err(aem_helper::protocol::RpcError::invalid(
                    "USAGE",
                    format!("unknown argument: {argument}"),
                ));
            }
        }
        index += 1;
    }
    if !(5..=3600).contains(&idle_seconds) {
        return Err(aem_helper::protocol::RpcError::invalid(
            "USAGE",
            "--idle-seconds must be between 5 and 3600",
        ));
    }
    Ok(CliOptions {
        user_config: user_config.ok_or_else(|| {
            aem_helper::protocol::RpcError::invalid("USAGE", "--user-config is required")
        })?,
        extension_root: extension_root.ok_or_else(|| {
            aem_helper::protocol::RpcError::invalid("USAGE", "--extension-root is required")
        })?,
        idle_seconds,
    })
}

async fn launch(options: CliOptions) -> Result<(), aem_helper::protocol::RpcError> {
    let (state, state_lock) = State::new_locked(&options.user_config)?;
    let log = state.open_rotated_log()?;
    drop(state_lock);
    let executable = std::env::current_exe().map_err(aem_helper::protocol::RpcError::io)?;
    let mut command = Command::new(executable);
    command
        .arg("serve")
        .arg("--user-config")
        .arg(&options.user_config)
        .arg("--extension-root")
        .arg(&options.extension_root)
        .arg("--idle-seconds")
        .arg(options.idle_seconds.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(log));
    #[cfg(windows)]
    {
        command.creation_flags(
            windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP
                | windows_sys::Win32::System::Threading::DETACHED_PROCESS,
        );
    }
    #[cfg(windows)]
    let inheritance_guard =
        StdHandleInheritanceGuard::clear().map_err(aem_helper::protocol::RpcError::io)?;
    let child = command.spawn();
    #[cfg(windows)]
    drop(inheritance_guard);
    let mut child = child.map_err(aem_helper::protocol::RpcError::io)?;
    let stdout = child.stdout.take().ok_or_else(|| {
        aem_helper::protocol::RpcError::internal("server did not expose a launch handshake")
    })?;
    let (sender, receiver) = mpsc::sync_channel(1);
    let reader = std::thread::spawn(move || {
        let mut line = String::new();
        let result = BufReader::new(stdout)
            .read_line(&mut line)
            .map(|bytes| (bytes != 0).then_some(line));
        let _ = sender.send(result);
    });
    let line = match receiver.recv_timeout(std::time::Duration::from_secs(10)) {
        Ok(result) => {
            reader.join().map_err(|_| {
                aem_helper::protocol::RpcError::internal("helper handshake reader failed")
            })?;
            result
                .map_err(aem_helper::protocol::RpcError::io)?
                .ok_or_else(|| {
                    aem_helper::protocol::RpcError::new(
                        "LAUNCH_FAILED",
                        "helper exited before returning a handshake",
                        true,
                    )
                })?
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err(aem_helper::protocol::RpcError::new(
                "LAUNCH_TIMEOUT",
                "helper did not start within ten seconds",
                true,
            ));
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err(aem_helper::protocol::RpcError::internal(
                "helper handshake reader failed",
            ));
        }
    };
    let handshake: serde_json::Value = serde_json::from_str(&line).map_err(|error| {
        aem_helper::protocol::RpcError::internal(format!("invalid launch handshake: {error}"))
    })?;
    println!(
        "{}",
        serde_json::to_string(&handshake)
            .map_err(|error| aem_helper::protocol::RpcError::internal(error.to_string()))?
    );
    Ok(())
}
