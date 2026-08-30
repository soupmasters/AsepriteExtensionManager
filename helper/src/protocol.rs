use std::collections::BTreeMap;
use std::io;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::PROTOCOL_VERSION;

pub type RpcResult<T> = Result<T, RpcError>;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpcRequest {
    pub protocol: u32,
    pub id: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Serialize)]
pub struct RpcResponse {
    pub protocol: u32,
    pub id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl RpcResponse {
    pub fn success(id: String, result: impl Serialize) -> Self {
        match serde_json::to_value(result) {
            Ok(result) => Self {
                protocol: PROTOCOL_VERSION,
                id,
                ok: true,
                result: Some(result),
                error: None,
            },
            Err(error) => Self::failure(id, RpcError::internal(error.to_string())),
        }
    }

    pub fn failure(id: String, error: RpcError) -> Self {
        Self {
            protocol: PROTOCOL_VERSION,
            id,
            ok: false,
            result: None,
            error: Some(error),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for RpcError {}

impl RpcError {
    pub fn new(code: impl Into<String>, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable,
            details: None,
        }
    }

    pub fn with_details(mut self, details: impl Serialize) -> Self {
        self.details = serde_json::to_value(details).ok();
        self
    }

    pub fn invalid(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(code, message, false)
    }

    pub fn io(error: io::Error) -> Self {
        Self::new("IO_ERROR", error.to_string(), is_transient_io(&error))
    }

    pub fn network(message: impl Into<String>) -> Self {
        Self::new("NETWORK_ERROR", message, true)
    }

    pub fn state(message: impl Into<String>) -> Self {
        Self::new("INVALID_STATE", message, false)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new("INTERNAL_ERROR", message, false)
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressEvent {
    pub protocol: u32,
    pub event: &'static str,
    pub operation_id: String,
    pub phase: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
}

impl ProgressEvent {
    pub fn new(
        operation_id: impl Into<String>,
        phase: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            protocol: PROTOCOL_VERSION,
            event: "progress",
            operation_id: operation_id.into(),
            phase: phase.into(),
            message: message.into(),
            current: None,
            total: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Method {
    Ping,
    ScanInstalled,
    UninstallPackage,
    RefreshRegistry,
    ListGitHubRepositories,
    ResolveGitHub,
    PreparePackage,
    PrepareSelfUpdate,
    PrepareSelfRollback,
    SyncLocal,
    VerifyInstall,
    ListUpdates,
    PrepareRollback,
    CacheStatus,
    ClearCache,
    Diagnostics,
    Shutdown,
}

impl TryFrom<&str> for Method {
    type Error = RpcError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "ping" => Ok(Self::Ping),
            "scanInstalled" => Ok(Self::ScanInstalled),
            "uninstallPackage" => Ok(Self::UninstallPackage),
            "refreshRegistry" => Ok(Self::RefreshRegistry),
            "listGitHubRepositories" => Ok(Self::ListGitHubRepositories),
            "resolveGitHub" => Ok(Self::ResolveGitHub),
            "preparePackage" => Ok(Self::PreparePackage),
            "prepareSelfUpdate" => Ok(Self::PrepareSelfUpdate),
            "prepareSelfRollback" => Ok(Self::PrepareSelfRollback),
            "syncLocal" => Ok(Self::SyncLocal),
            "verifyInstall" => Ok(Self::VerifyInstall),
            "listUpdates" => Ok(Self::ListUpdates),
            "prepareRollback" => Ok(Self::PrepareRollback),
            "cacheStatus" => Ok(Self::CacheStatus),
            "clearCache" => Ok(Self::ClearCache),
            "diagnostics" => Ok(Self::Diagnostics),
            "shutdown" => Ok(Self::Shutdown),
            _ => Err(RpcError::invalid(
                "METHOD_NOT_ALLOWED",
                "the requested method is not available",
            )),
        }
    }
}

pub fn decode_params<T>(mut params: Value) -> RpcResult<T>
where
    T: for<'de> Deserialize<'de>,
{
    if params.is_null() || params.as_array().is_some_and(Vec::is_empty) {
        params = empty_object();
    }
    serde_json::from_value(params)
        .map_err(|error| RpcError::invalid("INVALID_PARAMS", error.to_string()))
}

pub fn empty_object() -> Value {
    Value::Object(serde_json::Map::new())
}

pub fn diagnostic_map(items: impl IntoIterator<Item = (String, Value)>) -> Value {
    let map: BTreeMap<String, Value> = items.into_iter().collect();
    serde_json::to_value(map).unwrap_or_else(|_| empty_object())
}

fn is_transient_io(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::Interrupted
            | io::ErrorKind::TimedOut
            | io::ErrorKind::WouldBlock
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionReset
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_allowlist_is_closed() {
        assert_eq!(
            Method::try_from("scanInstalled").unwrap(),
            Method::ScanInstalled
        );
        assert_eq!(
            Method::try_from("uninstallPackage").unwrap(),
            Method::UninstallPackage
        );
        assert_eq!(
            Method::try_from("listGitHubRepositories").unwrap(),
            Method::ListGitHubRepositories
        );
        assert_eq!(
            Method::try_from("prepareSelfUpdate").unwrap(),
            Method::PrepareSelfUpdate
        );
        assert_eq!(
            Method::try_from("prepareSelfRollback").unwrap(),
            Method::PrepareSelfRollback
        );
        assert!(Method::try_from("runCommand").is_err());
        assert!(Method::try_from("fetchUrl").is_err());
    }

    #[test]
    fn unknown_request_fields_are_rejected() {
        let result = serde_json::from_str::<RpcRequest>(
            r#"{"protocol":1,"id":"1","method":"ping","params":{},"extra":true}"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn empty_lua_array_decodes_as_empty_parameter_object() {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Empty {}

        assert!(decode_params::<Empty>(serde_json::json!([])).is_ok());
        assert!(decode_params::<Empty>(serde_json::Value::Null).is_ok());
        assert!(decode_params::<Empty>(serde_json::json!([1])).is_err());
    }
}
