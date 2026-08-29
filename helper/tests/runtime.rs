use std::fs;
use std::path::Path;
use std::process::Command;

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tempfile::tempdir;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use walkdir::WalkDir;

type TestSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

fn copy_tree(source: &Path, destination: &Path) {
    for entry in WalkDir::new(source) {
        let entry = entry.expect("walk fixture tree");
        let relative = entry
            .path()
            .strip_prefix(source)
            .expect("relative fixture path");
        let target = destination.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target).expect("create fixture directory");
        } else {
            fs::copy(entry.path(), &target).expect("copy fixture file");
        }
    }
}

async fn rpc_response(socket: &mut TestSocket, id: &str, method: &str, params: Value) -> Value {
    socket
        .send(Message::Text(
            json!({"protocol":1,"id":id,"method":method,"params":params}).to_string(),
        ))
        .await
        .expect("send RPC request");
    loop {
        let message = tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
            .await
            .expect("RPC response timeout")
            .expect("RPC connection closed")
            .expect("RPC WebSocket error");
        let Message::Text(text) = message else {
            continue;
        };
        let response: Value = serde_json::from_str(&text).expect("RPC JSON response");
        if response["id"] != id {
            continue;
        }
        return response;
    }
}

async fn rpc_request(socket: &mut TestSocket, id: &str, method: &str, params: Value) -> Value {
    let response = rpc_response(socket, id, method, params).await;
    assert_eq!(response["ok"], true, "RPC {method} failed: {response}");
    response["result"].clone()
}

#[test]
fn version_and_smoke_do_not_require_runtime_paths() {
    let executable = env!("CARGO_BIN_EXE_aem-helper");
    let version = Command::new(executable).arg("--version").output().unwrap();
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8(version.stdout).unwrap().trim(),
        "aem-helper 0.1.0"
    );
    let smoke = Command::new(executable).arg("smoke").output().unwrap();
    assert!(smoke.status.success());
    let value: Value = serde_json::from_slice(&smoke.stdout).unwrap();
    assert_eq!(value["ok"], true);
}

#[tokio::test]
async fn launcher_authenticates_one_session_and_routes_concurrent_requests() {
    let temporary = tempdir().expect("tempdir");
    let executable = env!("CARGO_BIN_EXE_aem-helper");
    let launch_started = std::time::Instant::now();
    let output = Command::new(executable)
        .arg("launch")
        .arg("--user-config")
        .arg(temporary.path().join("config"))
        .arg("--extension-root")
        .arg(temporary.path().join("extension"))
        .arg("--idle-seconds")
        .arg("10")
        .output()
        .expect("launch");
    assert!(
        launch_started.elapsed() < std::time::Duration::from_secs(5),
        "the short-lived launcher waited for the detached server"
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let handshake: Value = serde_json::from_slice(&output.stdout).expect("handshake");
    assert_eq!(handshake["protocol"], 1);
    let token = handshake["token"].as_str().expect("token");
    assert_eq!(
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(token)
            .expect("base64")
            .len(),
        32
    );
    let port = handshake["port"].as_u64().expect("port");

    let invalid = format!("ws://127.0.0.1:{port}/v1/not-the-session");
    assert!(tokio_tungstenite::connect_async(invalid).await.is_err());

    let valid = format!(
        "ws://127.0.0.1:{port}{}",
        handshake["path"].as_str().expect("path")
    );
    let (mut socket, _) = tokio_tungstenite::connect_async(valid)
        .await
        .expect("authenticated session");
    let second = tokio::time::timeout(
        std::time::Duration::from_millis(250),
        tokio_tungstenite::connect_async(format!(
            "ws://127.0.0.1:{port}{}",
            handshake["path"].as_str().expect("path")
        )),
    )
    .await;
    assert!(
        !matches!(second, Ok(Ok(_))),
        "a second WebSocket client was accepted"
    );
    for id in ["one", "two", "three", "four"] {
        socket
            .send(Message::Text(
                json!({"protocol":1,"id":id,"method":"ping","params":{}}).to_string(),
            ))
            .await
            .expect("send ping");
    }
    let mut responses = std::collections::BTreeSet::new();
    while responses.len() < 4 {
        let message = tokio::time::timeout(std::time::Duration::from_secs(2), socket.next())
            .await
            .expect("message timeout")
            .expect("message")
            .expect("websocket");
        let Message::Text(text) = message else {
            continue;
        };
        let value: Value = serde_json::from_str(&text).expect("json");
        if value["ok"] == true {
            responses.insert(value["id"].as_str().expect("response id").to_owned());
        }
    }
    assert_eq!(
        responses,
        ["four", "one", "three", "two"]
            .into_iter()
            .map(ToOwned::to_owned)
            .collect()
    );

    socket
        .send(Message::Text(
            json!({"protocol":999,"id":"old","method":"ping","params":{}}).to_string(),
        ))
        .await
        .expect("protocol mismatch");
    loop {
        let message = socket.next().await.expect("message").expect("websocket");
        let Message::Text(text) = message else {
            continue;
        };
        let value: Value = serde_json::from_str(&text).expect("json");
        if value["id"] == "old" {
            assert_eq!(value["error"]["code"], "PROTOCOL_MISMATCH");
            break;
        }
    }

    socket
        .send(Message::Text(
            json!({"protocol":1,"id":"stop","method":"shutdown","params":{}}).to_string(),
        ))
        .await
        .expect("shutdown");
    loop {
        let Some(message) = socket.next().await else {
            break;
        };
        let message = message.expect("websocket");
        if let Message::Text(text) = message {
            let value: Value = serde_json::from_str(&text).expect("json");
            if value["id"] == "stop" {
                assert_eq!(value["ok"], true);
                break;
            }
        }
    }
}

#[tokio::test]
async fn linked_local_package_detects_same_version_content_updates() {
    let temporary = tempdir().expect("tempdir");
    let user_config = temporary.path().join("config");
    let extension_root = temporary.path().join("manager-extension");
    let bundled_registry = Path::new(env!("CARGO_MANIFEST_DIR")).join("../registry/bundled");
    copy_tree(&bundled_registry, &extension_root.join("registry"));

    let local_source = temporary.path().join("linked-source");
    fs::create_dir_all(&local_source).expect("local source");
    let manifest = br#"{
        "name": "linked-local",
        "displayName": "Linked Local",
        "version": "1.0.0",
        "main": "main.lua"
    }"#;
    fs::write(local_source.join("package.json"), manifest).expect("source manifest");
    fs::write(local_source.join("main.lua"), b"return 'first'\n").expect("source script");

    let executable = env!("CARGO_BIN_EXE_aem-helper");
    let output = Command::new(executable)
        .arg("launch")
        .arg("--user-config")
        .arg(&user_config)
        .arg("--extension-root")
        .arg(&extension_root)
        .arg("--idle-seconds")
        .arg("10")
        .output()
        .expect("launch helper");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let handshake: Value = serde_json::from_slice(&output.stdout).expect("handshake");
    let address = format!(
        "ws://127.0.0.1:{}{}",
        handshake["port"].as_u64().expect("port"),
        handshake["path"].as_str().expect("path")
    );
    let (mut socket, _) = tokio_tungstenite::connect_async(address)
        .await
        .expect("authenticated session");

    let synced = rpc_request(
        &mut socket,
        "sync-local",
        "syncLocal",
        json!({"packageJsonPath": local_source.join("package.json")}),
    )
    .await;
    assert_eq!(synced["name"], "linked-local");
    assert_eq!(synced["version"], "1.0.0");
    assert_eq!(synced["source"]["kind"], "local");
    let original_content_hash = synced["contentHash"]
        .as_str()
        .expect("initial content hash")
        .to_owned();
    let canonical_source =
        fs::canonicalize(local_source.join("package.json")).expect("canonical source");
    assert_eq!(
        synced["source"]["packageJsonPath"]
            .as_str()
            .expect("serialized canonical source"),
        canonical_source.to_str().expect("Unicode fixture path")
    );

    let installed = user_config.join("extensions/linked-local");
    fs::create_dir_all(&installed).expect("installed extension directory");
    fs::write(installed.join("package.json"), manifest).expect("installed manifest");
    let verified = rpc_request(
        &mut socket,
        "verify-local",
        "verifyInstall",
        json!({
            "name": synced["name"],
            "version": synced["version"],
            "artifactPath": synced["artifactPath"],
            "source": synced["source"]
        }),
    )
    .await;
    assert_eq!(verified["verified"], true);
    assert_eq!(verified["receipt"]["sourceKind"], "local");
    assert_eq!(verified["receipt"]["contentHash"], original_content_hash);
    assert!(user_config
        .join("extension-manager/receipts/linked-local.json")
        .is_file());

    fs::write(local_source.join("main.lua"), b"return 'second'\n").expect("mutate linked source");
    let listed = rpc_request(&mut socket, "list-local", "listUpdates", json!({})).await;
    let updates = listed["updates"].as_array().expect("updates array");
    let update = updates
        .iter()
        .find(|entry| entry["packageName"] == "linked-local")
        .expect("linked local update");
    assert_eq!(update["update"]["kind"], "local");
    assert_eq!(update["update"]["version"], "1.0.0");
    let resolution = &update["update"]["source"]["resolution"];
    let updated_content_hash = resolution["contentHash"]
        .as_str()
        .expect("updated content hash");
    assert_ne!(updated_content_hash, original_content_hash);
    assert_eq!(resolution["source"]["contentHash"], updated_content_hash);
    assert_eq!(resolution["source"]["kind"], "local");
    assert!(Path::new(
        resolution["artifactPath"]
            .as_str()
            .expect("prepared update artifact")
    )
    .is_file());
    assert!(listed["updateErrors"]
        .as_array()
        .expect("update errors")
        .is_empty());
    let installed_package = listed["packages"]
        .as_array()
        .expect("installed packages")
        .iter()
        .find(|package| package["name"] == "linked-local")
        .expect("installed linked package");
    assert_eq!(
        installed_package["update"]["source"]["resolution"]["contentHash"],
        updated_content_hash
    );

    rpc_request(&mut socket, "stop-local", "shutdown", json!({})).await;
}

#[tokio::test]
async fn prepare_package_loads_registry_before_reporting_missing_catalog_package() {
    let temporary = tempdir().expect("tempdir");
    let extension_root = temporary.path().join("manager-extension");
    let bundled_registry = Path::new(env!("CARGO_MANIFEST_DIR")).join("../registry/bundled");
    copy_tree(&bundled_registry, &extension_root.join("registry"));

    let executable = env!("CARGO_BIN_EXE_aem-helper");
    let output = Command::new(executable)
        .arg("launch")
        .arg("--user-config")
        .arg(temporary.path().join("config"))
        .arg("--extension-root")
        .arg(&extension_root)
        .arg("--idle-seconds")
        .arg("10")
        .output()
        .expect("launch helper");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let handshake: Value = serde_json::from_slice(&output.stdout).expect("handshake");
    let address = format!(
        "ws://127.0.0.1:{}{}",
        handshake["port"].as_u64().expect("port"),
        handshake["path"].as_str().expect("path")
    );
    let (mut socket, _) = tokio_tungstenite::connect_async(address)
        .await
        .expect("authenticated session");

    let response = rpc_response(
        &mut socket,
        "missing-catalog",
        "preparePackage",
        json!({"packageId": "does-not-exist", "version": "1.0.0"}),
    )
    .await;
    assert_eq!(response["ok"], false);
    assert_eq!(response["error"]["code"], "CATALOG_PACKAGE_NOT_FOUND");

    rpc_request(&mut socket, "stop-catalog", "shutdown", json!({})).await;
}
