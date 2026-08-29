use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Notify, RwLock, Semaphore};
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{accept_hdr_async, WebSocketStream};

use crate::github::{GitHubClient, ManagerRelease, ResolveOptions, ResolveResult};
use crate::installed;
use crate::package::{self, ExpectedManifest, PreparedPackage};
use crate::protocol::{
    decode_params, diagnostic_map, Method, ProgressEvent, RpcError, RpcRequest, RpcResponse,
    RpcResult,
};
use crate::registry::{available_updates, release_supports, RegistryClient, RegistryView};
use crate::state::{PendingSelfUpdate, Receipt, State};
use crate::{PROTOCOL_VERSION, VERSION};

const MAX_RPC_MESSAGE_BYTES: usize = 1024 * 1024;
const MAX_CONCURRENT_REQUESTS: usize = 8;
const MANAGER_NAME: &str = "aseprite-extension-manager";
const MANAGER_OWNER: &str = "soupmasters";
const MANAGER_REPOSITORY: &str = "AsepriteExtensionManager";
const MANAGER_REPOSITORY_URL: &str = "https://github.com/soupmasters/AsepriteExtensionManager";
const RELEASES_URL: &str = "https://github.com/soupmasters/AsepriteExtensionManager/releases";

#[derive(Clone, Debug)]
pub struct ServeOptions {
    pub user_config: PathBuf,
    pub extension_root: PathBuf,
    pub idle_seconds: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchHandshake {
    pub protocol: u32,
    pub port: u16,
    pub token: String,
    pub path: String,
    pub pid: u32,
    pub version: String,
}

#[derive(Clone)]
struct Context {
    user_config: PathBuf,
    extension_root: PathBuf,
    state: State,
    github: GitHubClient,
    registry: RegistryClient,
    registry_view: Arc<RwLock<Option<RegistryView>>>,
    self_update_status: Option<Value>,
}

pub async fn serve(options: ServeOptions) -> RpcResult<()> {
    let state = State::new(&options.user_config)?;
    let self_update_status = reconcile_pending_self_update(&options.extension_root, &state);
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(RpcError::io)?;
    let port = listener.local_addr().map_err(RpcError::io)?.port();
    let mut token_bytes = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut token_bytes);
    let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(token_bytes);
    let path = format!("/v1/{token}");
    let handshake = LaunchHandshake {
        protocol: PROTOCOL_VERSION,
        port,
        token,
        path: path.clone(),
        pid: std::process::id(),
        version: VERSION.to_owned(),
    };
    println!(
        "{}",
        serde_json::to_string(&handshake).map_err(|error| RpcError::internal(error.to_string()))?
    );
    use std::io::Write;
    std::io::stdout().flush().map_err(RpcError::io)?;

    let stream =
        accept_authenticated(&listener, &path, Duration::from_secs(options.idle_seconds)).await?;
    let context = Arc::new(Context {
        user_config: options.user_config,
        extension_root: options.extension_root.clone(),
        github: GitHubClient::new(state.clone())?,
        registry: RegistryClient::new(state.clone(), &options.extension_root),
        state,
        registry_view: Arc::new(RwLock::new(None)),
        self_update_status,
    });
    run_connection(stream, context, Duration::from_secs(options.idle_seconds)).await
}

#[allow(clippy::result_large_err)]
async fn accept_authenticated(
    listener: &TcpListener,
    expected_path: &str,
    timeout: Duration,
) -> RpcResult<WebSocketStream<TcpStream>> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(RpcError::new(
                "IDLE_TIMEOUT",
                "helper exited before a client connected",
                true,
            ));
        }
        let (stream, address) = tokio::time::timeout(remaining, listener.accept())
            .await
            .map_err(|_| {
                RpcError::new(
                    "IDLE_TIMEOUT",
                    "helper exited before a client connected",
                    true,
                )
            })?
            .map_err(RpcError::io)?;
        if !address.ip().is_loopback() {
            continue;
        }
        let expected = expected_path.to_owned();
        let result = accept_hdr_async(stream, move |request: &Request, response: Response| {
            if request.uri().path() == expected {
                Ok(response)
            } else {
                let error: ErrorResponse =
                    http_error_response(401, "invalid extension-manager session");
                Err(error)
            }
        })
        .await;
        match result {
            Ok(websocket) => return Ok(websocket),
            Err(_) => continue,
        }
    }
}

fn http_error_response(status: u16, message: &str) -> ErrorResponse {
    tokio_tungstenite::tungstenite::http::Response::builder()
        .status(status)
        .body(Some(message.to_owned()))
        .expect("static HTTP error response")
}

async fn run_connection(
    websocket: WebSocketStream<TcpStream>,
    context: Arc<Context>,
    idle: Duration,
) -> RpcResult<()> {
    let (mut writer, mut reader) = websocket.split();
    let (sender, mut receiver) = mpsc::unbounded_channel::<Message>();
    let shutdown = Arc::new(Notify::new());
    let writer_task = tokio::spawn(async move {
        while let Some(message) = receiver.recv().await {
            if writer.send(message).await.is_err() {
                break;
            }
        }
        let _ = writer.close().await;
    });
    let concurrency = Arc::new(Semaphore::new(MAX_CONCURRENT_REQUESTS));

    loop {
        let next = tokio::select! {
            _ = shutdown.notified() => break,
            value = tokio::time::timeout(idle, reader.next()) => value,
        };
        let message = match next {
            Err(_) if concurrency.available_permits() < MAX_CONCURRENT_REQUESTS => continue,
            Err(_) => break,
            Ok(Some(Ok(message))) => message,
            Ok(Some(Err(error))) => return Err(RpcError::network(error.to_string())),
            Ok(None) => break,
        };
        if message.is_close() {
            break;
        }
        let Message::Text(text) = message else {
            continue;
        };
        if text.len() > MAX_RPC_MESSAGE_BYTES {
            let response = RpcResponse::failure(
                String::new(),
                RpcError::invalid("MESSAGE_TOO_LARGE", "RPC message exceeds the size limit"),
            );
            send_json(&sender, &response)?;
            continue;
        }
        let request: RpcRequest = match serde_json::from_str(&text) {
            Ok(request) => request,
            Err(error) => {
                let response = RpcResponse::failure(
                    String::new(),
                    RpcError::invalid("INVALID_REQUEST", error.to_string()),
                );
                send_json(&sender, &response)?;
                continue;
            }
        };
        if request.id.is_empty()
            || request.id.len() > 128
            || !request
                .id
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "-._:".contains(character))
        {
            let response = RpcResponse::failure(
                request.id,
                RpcError::invalid("INVALID_REQUEST_ID", "request ID is invalid"),
            );
            send_json(&sender, &response)?;
            continue;
        }
        if request.protocol != PROTOCOL_VERSION {
            let response = RpcResponse::failure(
                request.id,
                RpcError::invalid(
                    "PROTOCOL_MISMATCH",
                    format!("helper supports protocol {PROTOCOL_VERSION}"),
                ),
            );
            send_json(&sender, &response)?;
            continue;
        }
        let permit = match concurrency.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                let response = RpcResponse::failure(
                    request.id,
                    RpcError::new("BUSY", "too many concurrent requests", true),
                );
                send_json(&sender, &response)?;
                continue;
            }
        };
        let context = context.clone();
        let sender = sender.clone();
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let operation_id = request.id.clone();
            let method = request.method.clone();
            let progress = ProgressEvent::new(
                operation_id.clone(),
                "started",
                format!("Starting {method}"),
            );
            let _ = send_json(&sender, &progress);
            let response = match Method::try_from(request.method.as_str()) {
                Ok(method) => match handle_request(context, method, request.params).await {
                    Ok(result) => RpcResponse::success(request.id.clone(), result),
                    Err(error) => RpcResponse::failure(request.id.clone(), error),
                },
                Err(error) => RpcResponse::failure(request.id.clone(), error),
            };
            let should_shutdown = method == "shutdown" && response.ok;
            let _ = send_json(&sender, &response);
            let progress =
                ProgressEvent::new(operation_id, "finished", format!("Finished {method}"));
            let _ = send_json(&sender, &progress);
            if should_shutdown {
                tokio::time::sleep(Duration::from_millis(20)).await;
                shutdown.notify_waiters();
            }
        });
    }
    drop(sender);
    let _ = tokio::time::timeout(Duration::from_secs(2), writer_task).await;
    Ok(())
}

fn send_json(sender: &mpsc::UnboundedSender<Message>, value: &impl Serialize) -> RpcResult<()> {
    let json =
        serde_json::to_string(value).map_err(|error| RpcError::internal(error.to_string()))?;
    sender
        .send(Message::Text(json))
        .map_err(|_| RpcError::network("WebSocket connection closed"))
}

async fn handle_request(context: Arc<Context>, method: Method, params: Value) -> RpcResult<Value> {
    match method {
        Method::Ping => {
            let _: EmptyParams = decode_params(params)?;
            Ok(serde_json::json!({
                "version": VERSION,
                "protocol": PROTOCOL_VERSION
            }))
        }
        Method::ScanInstalled => {
            let _: EmptyParams = decode_params(params)?;
            let packages = installed::scan(&context.user_config, &context.state)?;
            Ok(serde_json::json!({ "packages": packages }))
        }
        Method::RefreshRegistry => {
            let _: EmptyParams = decode_params(params)?;
            let view = context.registry.refresh(Utc::now())?;
            *context.registry_view.write().await = Some(view.clone());
            serde_json::to_value(view).map_err(|error| RpcError::internal(error.to_string()))
        }
        Method::ResolveGitHub => {
            let options: ResolveOptions = decode_params(params)?;
            let result = context.github.resolve(options).await?;
            if let ResolveResult::Ready { package, .. } = &result {
                reject_self_update(package)?;
            }
            serde_json::to_value(result).map_err(|error| RpcError::internal(error.to_string()))
        }
        Method::PreparePackage => {
            let params: PreparePackageParams = decode_params(params)?;
            let effective_resolution = params.resolution.as_ref().or_else(|| {
                params
                    .source
                    .as_ref()
                    .and_then(|source| source.get("resolution"))
            });
            if effective_resolution.is_none() {
                if let Some(source) = params.source.as_ref() {
                    match source.get("kind").and_then(Value::as_str) {
                        Some("local") => {
                            let package_json = source
                                .get("packageJsonPath")
                                .and_then(Value::as_str)
                                .ok_or_else(|| {
                                    RpcError::invalid(
                                        "INVALID_MANAGED_SOURCE",
                                        "local source is missing packageJsonPath",
                                    )
                                })?;
                            let package_json =
                                std::fs::canonicalize(package_json).map_err(RpcError::io)?;
                            let package =
                                package::package_local_folder(&context.state, &package_json)?;
                            reject_self_update(&package)?;
                            return Ok(serde_json::json!({
                                "status": "ready",
                                "artifactPath": package.artifact_path,
                                "name": package.name,
                                "displayName": package.display_name,
                                "version": package.version,
                                "sha256": package.sha256,
                                "byteLength": package.byte_length,
                                "contentHash": package.content_hash,
                                "source": {
                                    "kind": "local",
                                    "packageJsonPath": package_json,
                                    "contentHash": package.content_hash
                                }
                            }));
                        }
                        Some("github-release" | "github-snapshot") => {
                            let url = managed_github_resolution_url(source)?;
                            return match context
                                .github
                                .resolve(ResolveOptions {
                                    url,
                                    selection: github_asset_selection(source),
                                })
                                .await?
                            {
                                ResolveResult::Ready { package, source } => {
                                    if let Some(version) = params.version.as_deref() {
                                        if package.version != version {
                                            return Err(RpcError::invalid(
                                                "MANIFEST_MISMATCH",
                                                "resolved GitHub version differs from the selected update",
                                            ));
                                        }
                                    }
                                    reject_self_update(&package)?;
                                    serde_json::to_value(ResolveResult::Ready { package, source })
                                        .map_err(|error| RpcError::internal(error.to_string()))
                                }
                                ResolveResult::SelectionRequired { .. } => Err(RpcError::invalid(
                                    "ASSET_SELECTION_REQUIRED",
                                    "managed GitHub source now has multiple matching assets",
                                )),
                            };
                        }
                        _ => {}
                    }
                }
            }
            if let Some(package_id) = params.package_id.as_deref() {
                if params.artifact_path.is_none() && effective_resolution.is_none() {
                    let version = params.version.as_deref().ok_or_else(|| {
                        RpcError::invalid(
                            "INVALID_PARAMS",
                            "catalog preparation requires a version",
                        )
                    })?;
                    let cached_view = { context.registry_view.read().await.clone() };
                    let view = if let Some(view) = cached_view {
                        view
                    } else {
                        let view = context.registry.refresh(Utc::now())?;
                        *context.registry_view.write().await = Some(view.clone());
                        view
                    };
                    if view.expired {
                        return Err(RpcError::invalid(
                            "REGISTRY_EXPIRED",
                            "expired registry metadata cannot authorize an installation",
                        ));
                    }
                    let registry_package = view
                        .packages
                        .iter()
                        .find(|package| package.id == package_id.to_lowercase())
                        .ok_or_else(|| {
                            RpcError::invalid(
                                "CATALOG_PACKAGE_NOT_FOUND",
                                "package is not present in the authenticated catalog",
                            )
                        })?;
                    let release = registry_package
                        .releases
                        .iter()
                        .find(|release| release.version == version)
                        .ok_or_else(|| {
                            RpcError::invalid(
                                "CATALOG_RELEASE_NOT_FOUND",
                                "release is not present in the authenticated catalog",
                            )
                        })?;
                    if release.yanked {
                        return Err(RpcError::invalid(
                            "CATALOG_RELEASE_YANKED",
                            "this catalog release has been withdrawn",
                        ));
                    }
                    let aseprite_version = params.aseprite_version.as_deref().unwrap_or("1.3.15");
                    let api_version = params.api_version.unwrap_or(35);
                    if !release_supports(release, aseprite_version, api_version) {
                        return Err(RpcError::invalid(
                            "ASEPRITE_INCOMPATIBLE",
                            "catalog release is incompatible with this Aseprite version",
                        ));
                    }
                    let package = context
                        .github
                        .prepare_authenticated_asset(
                            &release.asset.url,
                            &release.asset.sha256,
                            release.asset.byte_length,
                            &registry_package.manifest_name,
                            &release.version,
                        )
                        .await?;
                    reject_self_update(&package)?;
                    return Ok(serde_json::json!({
                        "status": "ready",
                        "artifactPath": package.artifact_path,
                        "name": package.name,
                        "displayName": package.display_name,
                        "version": package.version,
                        "sha256": package.sha256,
                        "byteLength": package.byte_length,
                        "source": {
                            "kind": "registry",
                            "packageId": registry_package.id,
                            "repository": registry_package.repository,
                            "immutableUrl": release.asset.url,
                            "release": release.asset.release_tag,
                            "assetId": release.asset.asset_id,
                            "commit": release.asset.commit
                        }
                    }));
                }
            }
            let artifact_path = params
                .artifact_path
                .or_else(|| {
                    effective_resolution
                        .and_then(|resolution| resolution.get("artifactPath"))
                        .and_then(Value::as_str)
                        .map(PathBuf::from)
                })
                .ok_or_else(|| {
                    RpcError::invalid(
                        "CATALOG_PREPARATION_UNAVAILABLE",
                        "this alpha can prepare direct GitHub and local resolutions only",
                    )
                })?;
            let artifact = trusted_artifact_path(&context.state, &artifact_path)?;
            let package = package::validate_and_stage(
                &context.state,
                &artifact,
                ExpectedManifest {
                    name: params
                        .expected_name
                        .as_deref()
                        .or(params.package_id.as_deref()),
                    version: params
                        .expected_version
                        .as_deref()
                        .or(params.version.as_deref()),
                },
            )?;
            reject_self_update(&package)?;
            let source = effective_resolution
                .and_then(|resolution| resolution.get("source"))
                .cloned()
                .or_else(|| params.source.clone())
                .unwrap_or_else(|| serde_json::json!({ "kind": "unknown" }));
            Ok(package_result(&package, source))
        }
        Method::PrepareSelfUpdate => {
            let _: EmptyParams = decode_params(params)?;
            if context.state.pending_self_update()?.is_some() {
                return Err(RpcError::invalid(
                    "SELF_UPDATE_PENDING",
                    "finish or recover the pending manager update before preparing another",
                ));
            }
            let (package, release) = context
                .github
                .prepare_manager_release(MANAGER_OWNER, MANAGER_REPOSITORY)
                .await?;
            if !manager_release_is_update(VERSION, &release) {
                return Err(RpcError::invalid(
                    "SELF_UPDATE_NOT_NEWER",
                    "the latest manager release is not newer than this installation",
                ));
            }
            let source = serde_json::json!({
                "kind": "github-release",
                "repository": MANAGER_REPOSITORY_URL,
                "immutableUrl": release.download_url,
                "release": release.tag,
                "assetId": release.asset_id,
                "assetName": release.asset_name,
                "sha256": release.sha256
            });
            prepare_manager_transaction(&context, package, source, false)
        }
        Method::PrepareSelfRollback => {
            let _: EmptyParams = decode_params(params)?;
            if context.state.pending_self_update()?.is_some() {
                return Err(RpcError::invalid(
                    "SELF_UPDATE_PENDING",
                    "finish or recover the pending manager update before preparing another",
                ));
            }
            let receipt = context.state.read_receipt(MANAGER_NAME)?.ok_or_else(|| {
                RpcError::invalid(
                    "ROLLBACK_UNAVAILABLE",
                    "the manager has no verified previous release",
                )
            })?;
            let previous_version = receipt.previous_version.as_deref().ok_or_else(|| {
                RpcError::invalid(
                    "ROLLBACK_UNAVAILABLE",
                    "the manager has no verified previous release",
                )
            })?;
            let artifact = context
                .state
                .cached_artifact(MANAGER_NAME, true)?
                .ok_or_else(|| {
                    RpcError::invalid(
                        "ROLLBACK_UNAVAILABLE",
                        "the manager recovery package is missing",
                    )
                })?;
            let package = package::validate_manager_recovery_and_stage(
                &context.state,
                &artifact,
                previous_version,
            )?;
            verify_rollback_artifact_integrity(&receipt, true, &package)?;
            let source = receipt.previous_source.unwrap_or_else(|| {
                serde_json::json!({
                    "kind": "self-recovery",
                    "repository": MANAGER_REPOSITORY_URL
                })
            });
            prepare_manager_transaction(&context, package, source, true)
        }
        Method::SyncLocal => {
            let params: SyncLocalParams = decode_params(params)?;
            let package_json_path =
                std::fs::canonicalize(&params.package_json_path).map_err(RpcError::io)?;
            let package = package::package_local_folder(&context.state, &package_json_path)?;
            reject_self_update(&package)?;
            Ok(serde_json::json!({
                "status": "ready",
                "artifactPath": package.artifact_path,
                "name": package.name,
                "displayName": package.display_name,
                "version": package.version,
                "sha256": package.sha256,
                "byteLength": package.byte_length,
                "contentHash": package.content_hash,
                "source": {
                    "kind": "local",
                    "packageJsonPath": package_json_path,
                    "contentHash": package.content_hash
                }
            }))
        }
        Method::VerifyInstall => verify_install(&context, decode_params(params)?),
        Method::ListUpdates => {
            let params: ListUpdatesParams = decode_params(params)?;
            let aseprite_version = params.aseprite_version.as_deref().unwrap_or("1.3.15");
            let api_version = params.api_version.unwrap_or(35);
            let cached_view = { context.registry_view.read().await.clone() };
            let view = if let Some(view) = cached_view {
                view
            } else {
                let view = context.registry.refresh(Utc::now())?;
                *context.registry_view.write().await = Some(view.clone());
                view
            };
            let mut packages = installed::scan(&context.user_config, &context.state)?;
            let receipts = context.state.receipts()?;
            let managed_versions: Vec<_> = packages
                .iter()
                .filter(|package| {
                    package.managed
                        && receipts.iter().any(|receipt| {
                            receipt.source_kind == "registry"
                                && receipt.package_name.eq_ignore_ascii_case(&package.name)
                                && receipt.installed_version == package.version
                        })
                })
                .map(|package| (package.name.clone(), package.version.clone()))
                .collect();
            let mut updates = if view.expired {
                Default::default()
            } else {
                available_updates(
                    &view.packages,
                    &managed_versions,
                    aseprite_version,
                    api_version,
                )
            };
            let mut update_errors = BTreeMap::new();
            for receipt in &receipts {
                if receipt.package_name.eq_ignore_ascii_case(MANAGER_NAME) {
                    continue;
                }
                let lineage_matches = packages.iter().any(|package| {
                    package.managed
                        && package.name.eq_ignore_ascii_case(&receipt.package_name)
                        && package.version == receipt.installed_version
                });
                if !lineage_matches {
                    continue;
                }
                if updates.contains_key(&receipt.package_name.to_lowercase()) {
                    continue;
                }
                let prepared: RpcResult<Option<Value>> = match receipt.source_kind.as_str() {
                    "local" => (|| {
                        let path = receipt
                            .source
                            .get("packageJsonPath")
                            .and_then(Value::as_str)
                            .ok_or_else(|| {
                                RpcError::invalid(
                                    "INVALID_MANAGED_SOURCE",
                                    "linked local source is missing packageJsonPath",
                                )
                            })?;
                        let candidate =
                            package::package_local_folder(&context.state, Path::new(path))?;
                        ensure_candidate_identity(receipt, &candidate)?;
                        if candidate.content_hash == receipt.content_hash {
                            return Ok(None);
                        }
                        Ok(Some({
                            let package_json_path = receipt
                                .source
                                .get("packageJsonPath")
                                .cloned()
                                .unwrap_or(Value::Null);
                            let fresh_source = serde_json::json!({
                                "kind": "local",
                                "packageJsonPath": package_json_path,
                                "contentHash": candidate.content_hash
                            });
                            let resolution = serde_json::json!({
                                "status": "ready",
                                "artifactPath": candidate.artifact_path,
                                "name": candidate.name,
                                "displayName": candidate.display_name,
                                "version": candidate.version,
                                "sha256": candidate.sha256,
                                "byteLength": candidate.byte_length,
                                "contentHash": candidate.content_hash,
                                "source": fresh_source
                            });
                            serde_json::json!({
                                "kind": "local",
                                "version": candidate.version,
                                "source": {
                                    "kind": "prepared",
                                    "resolution": resolution
                                }
                            })
                        }))
                    })(),
                    "github-release" | "github-snapshot" => {
                        async {
                            let url = managed_github_resolution_url(&receipt.source)?;
                            match context
                                .github
                                .resolve(ResolveOptions {
                                    url,
                                    selection: github_asset_selection(&receipt.source),
                                })
                                .await?
                            {
                                ResolveResult::Ready { package, source } => {
                                    ensure_candidate_identity(receipt, &package)?;
                                    if github_candidate_is_update(receipt, &package) {
                                        let resolution = serde_json::json!({
                                            "status": "ready",
                                            "artifactPath": package.artifact_path,
                                            "name": package.name,
                                            "displayName": package.display_name,
                                            "version": package.version,
                                            "sha256": package.sha256,
                                            "byteLength": package.byte_length,
                                            "source": source
                                        });
                                        Ok(Some(serde_json::json!({
                                            "kind": "github",
                                            "version": package.version,
                                            "source": {
                                                "kind": "prepared",
                                                "resolution": resolution
                                            }
                                        })))
                                    } else {
                                        Ok(None)
                                    }
                                }
                                ResolveResult::SelectionRequired { .. } => Err(RpcError::invalid(
                                    "ASSET_SELECTION_REQUIRED",
                                    "the saved GitHub release asset is no longer unambiguous",
                                )),
                            }
                        }
                        .await
                    }
                    _ => Ok(None),
                };
                match prepared {
                    Ok(Some(prepared)) => {
                        updates.insert(receipt.package_name.to_lowercase(), prepared);
                    }
                    Ok(None) => {}
                    Err(error) => {
                        update_errors.insert(
                            receipt.package_name.to_lowercase(),
                            serde_json::to_value(error)
                                .map_err(|error| RpcError::internal(error.to_string()))?,
                        );
                    }
                }
            }

            if let Some(manager) = packages
                .iter()
                .find(|package| package.name.eq_ignore_ascii_case(MANAGER_NAME))
            {
                match context
                    .github
                    .latest_manager_release(MANAGER_OWNER, MANAGER_REPOSITORY)
                    .await
                {
                    Ok(Some(release)) if manager_release_is_update(&manager.version, &release) => {
                        updates.insert(
                            MANAGER_NAME.to_owned(),
                            serde_json::json!({
                                "kind": "self",
                                "version": release.version,
                                "source": {
                                    "kind": "self-update",
                                    "repository": MANAGER_REPOSITORY_URL,
                                    "release": release.tag,
                                    "assetId": release.asset_id,
                                    "assetName": release.asset_name,
                                    "sha256": release.sha256,
                                    "immutableUrl": release.download_url
                                }
                            }),
                        );
                    }
                    Ok(_) => {}
                    Err(error) => {
                        update_errors.insert(
                            MANAGER_NAME.to_owned(),
                            serde_json::to_value(error)
                                .map_err(|error| RpcError::internal(error.to_string()))?,
                        );
                    }
                }
                if let Some(error) = &context.self_update_status {
                    update_errors.insert(MANAGER_NAME.to_owned(), error.clone());
                }
            }
            installed::attach_updates(&mut packages, &updates);
            installed::attach_update_errors(&mut packages, &update_errors);
            let update_list: Vec<_> = updates
                .iter()
                .map(|(package_name, update)| {
                    serde_json::json!({
                        "packageName": package_name,
                        "update": update
                    })
                })
                .collect();
            let update_error_list: Vec<_> = update_errors
                .iter()
                .map(|(package_name, error)| {
                    serde_json::json!({
                        "packageName": package_name,
                        "error": error
                    })
                })
                .collect();
            Ok(serde_json::json!({
                "packages": packages,
                "updates": update_list,
                "updateErrors": update_error_list,
                "catalogExpired": view.expired
            }))
        }
        Method::PrepareRollback => {
            let params: PackageNameParams = decode_params(params)?;
            let receipt = context.state.read_receipt(&params.name)?;
            let installed = installed::find(&context.user_config, &context.state, &params.name)?;
            let installation_matches_receipt = match (&receipt, &installed) {
                (Some(receipt), Some(installed)) => receipt.installed_version == installed.version,
                _ => false,
            };
            let artifact = context
                .state
                .cached_artifact(&params.name, installation_matches_receipt)?
                .ok_or_else(|| {
                    RpcError::invalid(
                        "ROLLBACK_UNAVAILABLE",
                        "no previous managed artifact is cached",
                    )
                })?;
            let package = package::validate_and_stage(
                &context.state,
                &artifact,
                ExpectedManifest {
                    name: Some(&params.name),
                    version: None,
                },
            )?;
            if let Some(receipt) = receipt.as_ref() {
                verify_rollback_artifact_integrity(
                    receipt,
                    installation_matches_receipt,
                    &package,
                )?;
            }
            let rollback_source =
                select_rollback_source(receipt.as_ref(), installation_matches_receipt);
            Ok(serde_json::json!({
                "status": "ready",
                "artifactPath": package.artifact_path,
                "name": package.name,
                "version": package.version,
                "sha256": package.sha256,
                "byteLength": package.byte_length
                ,"source": rollback_source
            }))
        }
        Method::CacheStatus => {
            let _: EmptyParams = decode_params(params)?;
            let entries = context.state.cache_status()?;
            let bytes: u64 = entries.iter().map(|entry| entry.bytes).sum();
            Ok(serde_json::json!({ "entries": entries, "bytes": bytes }))
        }
        Method::ClearCache => {
            let params: ClearCacheParams = decode_params(params)?;
            let removed = if params.preserve_restore_points {
                context.state.clear_transient_caches()?
            } else {
                context.state.clear_cache(params.package_name.as_deref())?
            };
            Ok(serde_json::json!({ "removedBytes": removed }))
        }
        Method::Diagnostics => {
            let _: EmptyParams = decode_params(params)?;
            let installed_count = installed::scan(&context.user_config, &context.state)?.len();
            Ok(diagnostic_map([
                ("version".to_owned(), Value::String(VERSION.to_owned())),
                (
                    "protocol".to_owned(),
                    Value::Number(PROTOCOL_VERSION.into()),
                ),
                (
                    "platform".to_owned(),
                    Value::String(std::env::consts::OS.to_owned()),
                ),
                (
                    "architecture".to_owned(),
                    Value::String(std::env::consts::ARCH.to_owned()),
                ),
                (
                    "installedCount".to_owned(),
                    Value::Number(installed_count.into()),
                ),
                (
                    "registryBundled".to_owned(),
                    Value::Bool(context.extension_root.join("registry/root.json").is_file()),
                ),
                ("telemetry".to_owned(), Value::Bool(false)),
            ]))
        }
        Method::Shutdown => {
            let _: EmptyParams = decode_params(params)?;
            Ok(serde_json::json!({ "shuttingDown": true }))
        }
    }
}

fn prepare_manager_transaction(
    context: &Context,
    target: PreparedPackage,
    source: Value,
    target_is_recovery: bool,
) -> RpcResult<Value> {
    let installed = package::validate_manager_directory(&context.extension_root, VERSION)?;
    let recovery = package::package_manager_directory(&context.state, &context.extension_root)?;
    let prior_receipt = context.state.read_receipt(MANAGER_NAME)?;
    let previous_source = prior_receipt
        .as_ref()
        .filter(|receipt| receipt.installed_version == installed.version)
        .map(|receipt| receipt.source.clone())
        .or_else(|| {
            Some(serde_json::json!({
                "kind": "self-recovery",
                "repository": MANAGER_REPOSITORY_URL
            }))
        });
    let pending = PendingSelfUpdate {
        schema_version: 1,
        target_version: target.version.clone(),
        previous_version: installed.version,
        target_is_recovery,
        source: source.clone(),
        previous_source,
        artifact_sha256: target.sha256.clone(),
        artifact_byte_length: target.byte_length,
        recovery_sha256: recovery.sha256.clone(),
        recovery_byte_length: recovery.byte_length,
        prepared_at: Utc::now(),
    };
    let (candidate_path, recovery_path) = context.state.begin_self_update(
        &target.artifact_path,
        &recovery.artifact_path,
        &pending,
    )?;
    Ok(serde_json::json!({
        "status": "ready",
        "artifactPath": candidate_path,
        "recoveryArtifact": recovery_path,
        "name": target.name,
        "displayName": target.display_name,
        "version": target.version,
        "sha256": target.sha256,
        "byteLength": target.byte_length,
        "source": source,
        "selfUpdate": true,
        "restartRequired": true
    }))
}

fn reconcile_pending_self_update(extension_root: &Path, state: &State) -> Option<Value> {
    match reconcile_pending_self_update_inner(extension_root, state) {
        Ok(()) => None,
        Err(error) => {
            let recovery_error = if error.code == "SELF_UPDATE_RECOVERY_REQUIRED" {
                error
            } else {
                RpcError::invalid(
                    "SELF_UPDATE_RECOVERY_REQUIRED",
                    format!(
                        "the pending manager update could not be reconciled: {}",
                        error.message
                    ),
                )
                .with_details(serde_json::json!({
                    "causeCode": error.code,
                    "recoveryArtifact": state.self_update_artifact(true),
                    "releaseUrl": RELEASES_URL
                }))
            };
            Some(serde_json::to_value(recovery_error).unwrap_or_else(|_| {
                serde_json::json!({
                    "code": "SELF_UPDATE_RECOVERY_REQUIRED",
                    "message": "the pending manager update needs manual recovery",
                    "retryable": false
                })
            }))
        }
    }
}

fn reconcile_pending_self_update_inner(extension_root: &Path, state: &State) -> RpcResult<()> {
    let Some(pending) = state.pending_self_update()? else {
        return Ok(());
    };

    if pending.schema_version != 1 {
        return Err(self_update_recovery_error(
            state,
            &pending,
            "the pending manager update uses an unsupported journal version",
        ));
    }

    if VERSION == pending.previous_version
        && package::validate_manager_directory(extension_root, &pending.previous_version).is_ok()
    {
        state.clear_self_update()?;
        return Ok(());
    }

    if VERSION != pending.target_version {
        return Err(self_update_recovery_error(
            state,
            &pending,
            "the installed helper version does not match the pending manager update",
        ));
    }
    let installed_target = if pending.target_is_recovery {
        package::validate_manager_directory(extension_root, &pending.target_version)
    } else {
        package::validate_manager_release_directory(extension_root, &pending.target_version)
    };
    installed_target.map_err(|error| {
        self_update_recovery_error(
            state,
            &pending,
            &format!(
                "the updated manager installation is incomplete: {}",
                error.message
            ),
        )
    })?;

    let candidate_path = state.self_update_artifact(false).ok_or_else(|| {
        self_update_recovery_error(
            state,
            &pending,
            "the verified manager update package is missing",
        )
    })?;
    let recovery_path = state.self_update_artifact(true).ok_or_else(|| {
        self_update_recovery_error(state, &pending, "the manager recovery package is missing")
    })?;
    let candidate = if pending.target_is_recovery {
        package::validate_manager_recovery_and_stage(
            state,
            &candidate_path,
            &pending.target_version,
        )
    } else {
        package::validate_manager_and_stage(state, &candidate_path, &pending.target_version)
    }
    .map_err(|error| {
        self_update_recovery_error(
            state,
            &pending,
            &format!("the cached manager update is invalid: {}", error.message),
        )
    })?;
    let recovery = package::validate_manager_recovery_and_stage(
        state,
        &recovery_path,
        &pending.previous_version,
    )
    .map_err(|error| {
        self_update_recovery_error(
            state,
            &pending,
            &format!("the manager recovery package is invalid: {}", error.message),
        )
    })?;
    if candidate.sha256 != pending.artifact_sha256
        || candidate.byte_length != pending.artifact_byte_length
        || recovery.sha256 != pending.recovery_sha256
        || recovery.byte_length != pending.recovery_byte_length
    {
        return Err(self_update_recovery_error(
            state,
            &pending,
            "a cached manager update or recovery package changed unexpectedly",
        ));
    }

    contextless_cache_reconciled_self_update(state, &pending, &candidate, &recovery)?;
    state.clear_self_update()
}

fn contextless_cache_reconciled_self_update(
    state: &State,
    pending: &PendingSelfUpdate,
    candidate: &PreparedPackage,
    recovery: &PreparedPackage,
) -> RpcResult<()> {
    let _ = state.cache_artifact(MANAGER_NAME, &recovery.artifact_path)?;
    let (current, previous) = state.cache_artifact(MANAGER_NAME, &candidate.artifact_path)?;
    let receipt = Receipt {
        schema_version: 1,
        package_name: MANAGER_NAME.to_owned(),
        source_kind: pending
            .source
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("self-update")
            .to_owned(),
        source: pending.source.clone(),
        installed_version: pending.target_version.clone(),
        commit: string_field(&pending.source, "commit"),
        release: string_field(&pending.source, "release"),
        asset: string_field(&pending.source, "assetName"),
        artifact_sha256: candidate.sha256.clone(),
        artifact_byte_length: candidate.byte_length,
        installed_at: Utc::now(),
        local_folder: None,
        content_hash: None,
        previous_artifact: previous,
        previous_source: pending.previous_source.clone(),
        previous_version: Some(pending.previous_version.clone()),
        previous_artifact_sha256: Some(recovery.sha256.clone()),
        previous_artifact_byte_length: Some(recovery.byte_length),
    };
    state.write_receipt(&receipt)?;
    if !current.is_file() {
        return Err(RpcError::state(
            "the reconciled manager package was not retained in cache",
        ));
    }
    Ok(())
}

fn self_update_recovery_error(
    state: &State,
    pending: &PendingSelfUpdate,
    message: &str,
) -> RpcError {
    RpcError::invalid("SELF_UPDATE_RECOVERY_REQUIRED", message).with_details(serde_json::json!({
        "targetVersion": pending.target_version,
        "previousVersion": pending.previous_version,
        "recoveryArtifact": state.self_update_artifact(true),
        "releaseUrl": RELEASES_URL
    }))
}

fn verify_install(context: &Context, params: VerifyInstallParams) -> RpcResult<Value> {
    let artifact = trusted_artifact_path(&context.state, &params.artifact_path)?;
    let prepared = package::validate_and_stage(
        &context.state,
        &artifact,
        ExpectedManifest {
            name: Some(&params.name),
            version: Some(&params.version),
        },
    )?;
    match installed::verify(
        &context.user_config,
        &context.state,
        &params.name,
        &params.version,
    ) {
        Ok(_) => {}
        Err(error) if error.code == "INSTALL_VERIFICATION_FAILED" => {
            let previous_receipt = context.state.read_receipt(&params.name)?;
            let currently_installed =
                installed::find(&context.user_config, &context.state, &params.name)?;
            let current_intact = previous_receipt
                .as_ref()
                .zip(currently_installed.as_ref())
                .is_some_and(|(receipt, installed)| receipt.installed_version == installed.version);
            return Ok(serde_json::json!({
                "verified": false,
                "message": error.message,
                "currentIntact": current_intact,
                "rollbackAvailable": !current_intact
                    && context.state.cached_artifact(&params.name, false)?.is_some()
            }));
        }
        Err(error) => return Err(error),
    }
    let prior_receipt = context.state.read_receipt(&params.name)?;
    let (current, previous) = context
        .state
        .cache_artifact(&params.name, &prepared.artifact_path)?;
    let (
        previous_source,
        previous_version,
        previous_artifact_sha256,
        previous_artifact_byte_length,
    ) = previous_identity(prior_receipt.as_ref(), &prepared.sha256, previous.is_some());
    let source_kind = params
        .source
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    let receipt = Receipt {
        schema_version: 1,
        package_name: params.name.clone(),
        source_kind,
        commit: string_field(&params.source, "commit"),
        release: string_field(&params.source, "release"),
        asset: string_field(&params.source, "assetName"),
        installed_version: params.version,
        artifact_sha256: prepared.sha256,
        artifact_byte_length: prepared.byte_length,
        installed_at: Utc::now(),
        local_folder: params
            .source
            .get("packageJsonPath")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .and_then(|path| path.parent().map(Path::to_owned)),
        content_hash: params
            .source
            .get("contentHash")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        previous_artifact: previous,
        previous_source,
        previous_version,
        previous_artifact_sha256,
        previous_artifact_byte_length,
        source: params.source,
    };
    let receipt_path = context.state.write_receipt(&receipt)?;
    Ok(serde_json::json!({
        "verified": true,
        "receipt": receipt,
        "receiptPath": receipt_path,
        "cachedArtifact": current
    }))
}

fn trusted_artifact_path(state: &State, path: &Path) -> RpcResult<PathBuf> {
    let canonical = std::fs::canonicalize(path).map_err(RpcError::io)?;
    let staging = std::fs::canonicalize(state.root().join("staging")).map_err(RpcError::io)?;
    let cache = std::fs::canonicalize(state.root().join("cache")).map_err(RpcError::io)?;
    if !canonical.starts_with(staging) && !canonical.starts_with(cache) {
        return Err(RpcError::invalid(
            "UNTRUSTED_ARTIFACT_PATH",
            "artifact must have been prepared by this helper session",
        ));
    }
    Ok(canonical)
}

fn reject_self_update(package: &PreparedPackage) -> RpcResult<()> {
    if package.name.eq_ignore_ascii_case(MANAGER_NAME) {
        return Err(RpcError::invalid(
            "SELF_UPDATE_RESTRICTED",
            "use the manager's dedicated update action for Aseprite Extension Manager releases",
        )
        .with_details(serde_json::json!({ "releaseUrl": RELEASES_URL })));
    }
    Ok(())
}

fn string_field(value: &Value, name: &str) -> Option<String> {
    value
        .get(name)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn previous_identity(
    prior: Option<&Receipt>,
    prepared_sha256: &str,
    has_previous_artifact: bool,
) -> (Option<Value>, Option<String>, Option<String>, Option<u64>) {
    if !has_previous_artifact {
        return (None, None, None, None);
    }
    let Some(prior) = prior else {
        return (None, None, None, None);
    };
    if prior.artifact_sha256.eq_ignore_ascii_case(prepared_sha256) {
        (
            prior.previous_source.clone(),
            prior.previous_version.clone(),
            prior.previous_artifact_sha256.clone(),
            prior.previous_artifact_byte_length,
        )
    } else {
        (
            Some(prior.source.clone()),
            Some(prior.installed_version.clone()),
            Some(prior.artifact_sha256.clone()),
            Some(prior.artifact_byte_length),
        )
    }
}

fn verify_rollback_artifact_integrity(
    receipt: &Receipt,
    previous_artifact: bool,
    package: &PreparedPackage,
) -> RpcResult<()> {
    let expected = if previous_artifact {
        match (
            receipt.previous_artifact_sha256.as_deref(),
            receipt.previous_artifact_byte_length,
        ) {
            (Some(hash), Some(length)) => Some((hash, length)),
            (None, None) => None,
            _ => {
                return Err(RpcError::state(
                    "the rollback receipt contains incomplete previous artifact integrity data",
                ));
            }
        }
    } else {
        Some((
            receipt.artifact_sha256.as_str(),
            receipt.artifact_byte_length,
        ))
    };
    let Some((expected_hash, expected_length)) = expected else {
        return Ok(());
    };
    if package.byte_length != expected_length || !package.sha256.eq_ignore_ascii_case(expected_hash)
    {
        return Err(RpcError::invalid(
            "ROLLBACK_ARTIFACT_MISMATCH",
            "the cached rollback package no longer matches its verified receipt",
        )
        .with_details(serde_json::json!({
            "expectedSha256": expected_hash,
            "actualSha256": package.sha256,
            "expectedByteLength": expected_length,
            "actualByteLength": package.byte_length
        })));
    }
    Ok(())
}

fn package_result(package: &PreparedPackage, source: Value) -> Value {
    serde_json::json!({
        "status": "ready",
        "artifactPath": package.artifact_path,
        "name": package.name,
        "displayName": package.display_name,
        "version": package.version,
        "sha256": package.sha256,
        "byteLength": package.byte_length,
        "contentHash": package.content_hash,
        "source": source
    })
}

fn select_rollback_source(receipt: Option<&Receipt>, previous_artifact: bool) -> Value {
    if previous_artifact {
        receipt
            .and_then(|receipt| receipt.previous_source.clone())
            .unwrap_or(Value::Null)
    } else {
        receipt
            .map(|receipt| receipt.source.clone())
            .unwrap_or(Value::Null)
    }
}

fn github_asset_selection(source: &Value) -> Option<String> {
    source
        .get("assetName")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn managed_github_resolution_url(source: &Value) -> RpcResult<String> {
    let repository = source
        .get("repository")
        .or_else(|| source.get("immutableUrl"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            RpcError::invalid(
                "INVALID_MANAGED_SOURCE",
                "GitHub source is missing its repository",
            )
        })?;
    if source.get("kind").and_then(Value::as_str) != Some("github-snapshot") {
        return Ok(repository.to_owned());
    }
    let tracked_ref = source
        .get("trackedRef")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            RpcError::invalid(
                "INVALID_MANAGED_SOURCE",
                "GitHub snapshot source is missing its tracked ref",
            )
        })?;
    let mut url = url::Url::parse(repository)
        .map_err(|error| RpcError::invalid("INVALID_MANAGED_SOURCE", error.to_string()))?;
    url.set_query(None);
    url.set_fragment(None);
    {
        let mut segments = url.path_segments_mut().map_err(|_| {
            RpcError::invalid(
                "INVALID_MANAGED_SOURCE",
                "GitHub repository URL cannot track a ref",
            )
        })?;
        segments.pop_if_empty();
        segments.push("tree");
        for component in tracked_ref.split('/') {
            if component.is_empty() {
                return Err(RpcError::invalid(
                    "INVALID_MANAGED_SOURCE",
                    "GitHub tracked ref is invalid",
                ));
            }
            segments.push(component);
        }
    }
    Ok(url.to_string())
}

fn ensure_candidate_identity(receipt: &Receipt, candidate: &PreparedPackage) -> RpcResult<()> {
    if candidate.name.eq_ignore_ascii_case(&receipt.package_name) {
        return Ok(());
    }
    Err(RpcError::invalid(
        "SOURCE_IDENTITY_CHANGED",
        "the linked source now contains a different extension package",
    )
    .with_details(serde_json::json!({
        "expectedName": receipt.package_name,
        "actualName": candidate.name
    })))
}

fn github_candidate_is_update(receipt: &Receipt, candidate: &PreparedPackage) -> bool {
    if candidate.sha256 == receipt.artifact_sha256 {
        return false;
    }
    match (
        semver::Version::parse(&receipt.installed_version),
        semver::Version::parse(&candidate.version),
    ) {
        (Ok(current), Ok(candidate)) if candidate > current => true,
        (Ok(current), Ok(candidate)) if candidate == current => {
            receipt.source_kind == "github-snapshot"
        }
        (Ok(_), Ok(_)) => false,
        _ => true,
    }
}

fn manager_release_is_update(current: &str, release: &ManagerRelease) -> bool {
    match (
        semver::Version::parse(current),
        semver::Version::parse(&release.version),
    ) {
        (Ok(current), Ok(candidate)) => candidate > current,
        _ => false,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyParams {}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreparePackageParams {
    #[serde(default)]
    artifact_path: Option<PathBuf>,
    #[serde(default)]
    expected_name: Option<String>,
    #[serde(default)]
    expected_version: Option<String>,
    #[serde(default)]
    package_id: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    source: Option<Value>,
    #[serde(default)]
    resolution: Option<Value>,
    #[serde(default)]
    aseprite_version: Option<String>,
    #[serde(default)]
    api_version: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncLocalParams {
    package_json_path: PathBuf,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VerifyInstallParams {
    name: String,
    version: String,
    artifact_path: PathBuf,
    source: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PackageNameParams {
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ClearCacheParams {
    #[serde(default)]
    package_name: Option<String>,
    #[serde(default = "default_true")]
    preserve_restore_points: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListUpdatesParams {
    #[serde(default)]
    aseprite_version: Option<String>,
    #[serde(default)]
    api_version: Option<u32>,
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_never_serializes_extra_fields() {
        let handshake = LaunchHandshake {
            protocol: 1,
            port: 1234,
            token: "secret".to_owned(),
            path: "/v1/secret".to_owned(),
            pid: 1,
            version: "0.1.0".to_owned(),
        };
        let value = serde_json::to_value(handshake).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 6);
    }

    #[test]
    fn generic_package_paths_cannot_bypass_the_dedicated_self_update_flow() {
        let package = PreparedPackage {
            artifact_path: PathBuf::from("manager.zip"),
            name: MANAGER_NAME.to_owned(),
            display_name: None,
            version: "0.2.0".to_owned(),
            sha256: "0".repeat(64),
            byte_length: 1,
            content_hash: None,
        };
        let error = reject_self_update(&package).expect_err("restricted");
        assert_eq!(error.code, "SELF_UPDATE_RESTRICTED");
    }

    #[test]
    fn generic_prepared_result_preserves_fresh_local_source() {
        let package = PreparedPackage {
            artifact_path: PathBuf::from("staging/fresh.aseprite-extension"),
            name: "sample".to_owned(),
            display_name: Some("Sample".to_owned()),
            version: "1.0.0".to_owned(),
            sha256: "1".repeat(64),
            byte_length: 12,
            content_hash: Some("fresh-content".to_owned()),
        };
        let source = serde_json::json!({
            "kind": "local",
            "packageJsonPath": "/development/sample/package.json",
            "contentHash": "fresh-content"
        });
        let result = package_result(&package, source);
        assert_eq!(result["source"]["kind"], "local");
        assert_eq!(result["source"]["contentHash"], "fresh-content");
        assert_eq!(result["contentHash"], "fresh-content");
    }

    #[test]
    fn rollback_uses_the_source_identity_of_the_selected_artifact() {
        let current_source = serde_json::json!({
            "kind": "github-release",
            "release": "v2.0.0",
            "assetId": 200
        });
        let previous_source = serde_json::json!({
            "kind": "github-release",
            "release": "v1.0.0",
            "assetId": 100
        });
        let receipt = Receipt {
            schema_version: 1,
            package_name: "sample".to_owned(),
            source_kind: "github-release".to_owned(),
            source: current_source.clone(),
            installed_version: "2.0.0".to_owned(),
            commit: None,
            release: Some("v2.0.0".to_owned()),
            asset: Some("sample.aseprite-extension".to_owned()),
            artifact_sha256: "2".repeat(64),
            artifact_byte_length: 2,
            installed_at: Utc::now(),
            local_folder: None,
            content_hash: None,
            previous_artifact: Some(PathBuf::from("cache/sample/previous.aseprite-extension")),
            previous_source: Some(previous_source.clone()),
            previous_version: Some("1.0.0".to_owned()),
            previous_artifact_sha256: Some("1".repeat(64)),
            previous_artifact_byte_length: Some(1),
        };
        assert_eq!(
            select_rollback_source(Some(&receipt), true),
            previous_source
        );
        assert_eq!(
            select_rollback_source(Some(&receipt), false),
            current_source
        );

        let previous_package = PreparedPackage {
            artifact_path: PathBuf::from("previous.aseprite-extension"),
            name: "sample".to_owned(),
            display_name: None,
            version: "1.0.0".to_owned(),
            sha256: "1".repeat(64),
            byte_length: 1,
            content_hash: None,
        };
        verify_rollback_artifact_integrity(&receipt, true, &previous_package)
            .expect("verified previous artifact");
        let mut tampered = previous_package;
        tampered.sha256 = "f".repeat(64);
        assert_eq!(
            verify_rollback_artifact_integrity(&receipt, true, &tampered)
                .expect_err("tampered previous artifact")
                .code,
            "ROLLBACK_ARTIFACT_MISMATCH"
        );
    }

    #[test]
    fn repeated_identical_sync_keeps_previous_artifact_identity() {
        let current_source = serde_json::json!({
            "kind": "local",
            "contentHash": "current"
        });
        let previous_source = serde_json::json!({
            "kind": "local",
            "contentHash": "previous"
        });
        let receipt = Receipt {
            schema_version: 1,
            package_name: "sample".to_owned(),
            source_kind: "local".to_owned(),
            source: current_source,
            installed_version: "1.0.0".to_owned(),
            commit: None,
            release: None,
            asset: None,
            artifact_sha256: "a".repeat(64),
            artifact_byte_length: 2,
            installed_at: Utc::now(),
            local_folder: None,
            content_hash: Some("current".to_owned()),
            previous_artifact: Some(PathBuf::from("cache/sample/previous.aseprite-extension")),
            previous_source: Some(previous_source.clone()),
            previous_version: Some("0.9.0".to_owned()),
            previous_artifact_sha256: Some("9".repeat(64)),
            previous_artifact_byte_length: Some(1),
        };
        let identity = previous_identity(Some(&receipt), &"a".repeat(64), true);
        assert_eq!(identity.0, Some(previous_source));
        assert_eq!(identity.1.as_deref(), Some("0.9.0"));
        assert_eq!(identity.2, Some("9".repeat(64)));
        assert_eq!(identity.3, Some(1));
    }

    #[test]
    fn managed_github_update_reuses_stored_asset_selection() {
        let source = serde_json::json!({
            "kind": "github-release",
            "repository": "https://github.com/example/sample",
            "assetName": "sample-windows.aseprite-extension"
        });
        assert_eq!(
            github_asset_selection(&source).as_deref(),
            Some("sample-windows.aseprite-extension")
        );
    }

    #[test]
    fn managed_snapshot_reuses_its_exact_tracked_ref() {
        let source = serde_json::json!({
            "kind": "github-snapshot",
            "repository": "https://github.com/example/sample",
            "commit": "1".repeat(40),
            "trackedRef": "release/1.x"
        });
        assert_eq!(
            managed_github_resolution_url(&source).unwrap(),
            "https://github.com/example/sample/tree/release/1.x"
        );
        let legacy = serde_json::json!({
            "kind": "github-snapshot",
            "repository": "https://github.com/example/sample",
            "commit": "1".repeat(40)
        });
        assert_eq!(
            managed_github_resolution_url(&legacy)
                .expect_err("missing lineage")
                .code,
            "INVALID_MANAGED_SOURCE"
        );
    }

    #[test]
    fn github_semantic_updates_never_downgrade_and_snapshots_detect_changed_content() {
        let receipt = Receipt {
            schema_version: 1,
            package_name: "sample".to_owned(),
            source_kind: "github-snapshot".to_owned(),
            source: serde_json::json!({
                "kind":"github-snapshot",
                "trackedRef":"main"
            }),
            installed_version: "2.0.0".to_owned(),
            commit: Some("1".repeat(40)),
            release: None,
            asset: None,
            artifact_sha256: "1".repeat(64),
            artifact_byte_length: 1,
            installed_at: Utc::now(),
            local_folder: None,
            content_hash: None,
            previous_artifact: None,
            previous_source: None,
            previous_version: None,
            previous_artifact_sha256: None,
            previous_artifact_byte_length: None,
        };
        let candidate = |version: &str| PreparedPackage {
            artifact_path: PathBuf::from("candidate.aseprite-extension"),
            name: "sample".to_owned(),
            display_name: None,
            version: version.to_owned(),
            sha256: "2".repeat(64),
            byte_length: 2,
            content_hash: None,
        };
        assert!(!github_candidate_is_update(&receipt, &candidate("1.9.9")));
        assert!(github_candidate_is_update(&receipt, &candidate("2.0.0")));
        assert!(github_candidate_is_update(&receipt, &candidate("2.0.1")));

        let mut release = receipt.clone();
        release.source_kind = "github-release".to_owned();
        assert!(!github_candidate_is_update(&release, &candidate("2.0.0")));

        let mut opaque = receipt;
        opaque.installed_version = "development".to_owned();
        assert!(github_candidate_is_update(
            &opaque,
            &candidate("development")
        ));
    }

    #[tokio::test]
    async fn unauthenticated_listener_exits_after_idle_timeout() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let error = accept_authenticated(&listener, "/v1/fixture", Duration::from_millis(20))
            .await
            .expect_err("idle timeout");
        assert_eq!(error.code, "IDLE_TIMEOUT");
    }

    #[tokio::test]
    async fn client_disconnect_ends_the_single_client_server() {
        let temporary = tempfile::tempdir().unwrap();
        let user_config = temporary.path().join("config");
        let extension_root = temporary.path().join("extension");
        std::fs::create_dir_all(&extension_root).unwrap();
        let state = State::new(&user_config).unwrap();
        let context = Arc::new(Context {
            user_config,
            extension_root: extension_root.clone(),
            github: GitHubClient::new(state.clone()).unwrap(),
            registry: RegistryClient::new(state.clone(), &extension_root),
            state,
            registry_view: Arc::new(RwLock::new(None)),
            self_update_status: None,
        });
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let socket = accept_authenticated(&listener, "/v1/fixture", Duration::from_secs(2))
                .await
                .unwrap();
            run_connection(socket, context, Duration::from_secs(2))
                .await
                .unwrap();
        });
        let (mut socket, _) =
            tokio_tungstenite::connect_async(format!("ws://{address}/v1/fixture"))
                .await
                .unwrap();
        socket.close(None).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .expect("server stopped")
            .unwrap();
    }
}
