//! Wire types and frame decoding.

use std::fmt;
use std::str::FromStr;

use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// The protocol version spoken by this implementation.
///
/// Adding an operation is a MINOR bump; removing or renaming one is MAJOR.
pub const PROTOCOL_VERSION: Version = Version::new(0, 3, 0);

/// A typed IPC operation id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Operation {
    HealthGet,
    StatusGet,
    ConfigGet,
    PluginsList,
    CapabilitiesList,
    CloudEnroll,
    CloudStatus,
    CloudForget,
    CloudSetPrivilegedExecution,
}

impl Operation {
    pub fn as_str(self) -> &'static str {
        match self {
            Operation::HealthGet => "health.get",
            Operation::StatusGet => "status.get",
            Operation::ConfigGet => "config.get",
            Operation::PluginsList => "plugins.list",
            Operation::CapabilitiesList => "capabilities.list",
            Operation::CloudEnroll => "cloud.enroll",
            Operation::CloudStatus => "cloud.status",
            Operation::CloudForget => "cloud.forget",
            Operation::CloudSetPrivilegedExecution => "cloud.set-privileged-execution",
        }
    }
}

/// Error returned when parsing an unknown operation id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownOperationError;

impl fmt::Display for UnknownOperationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("unknown operation")
    }
}

impl std::error::Error for UnknownOperationError {}

impl FromStr for Operation {
    type Err = UnknownOperationError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "health.get" => Ok(Operation::HealthGet),
            "status.get" => Ok(Operation::StatusGet),
            "config.get" => Ok(Operation::ConfigGet),
            "plugins.list" => Ok(Operation::PluginsList),
            "capabilities.list" => Ok(Operation::CapabilitiesList),
            "cloud.enroll" => Ok(Operation::CloudEnroll),
            "cloud.status" => Ok(Operation::CloudStatus),
            "cloud.forget" => Ok(Operation::CloudForget),
            "cloud.set-privileged-execution" => Ok(Operation::CloudSetPrivilegedExecution),
            _ => Err(UnknownOperationError),
        }
    }
}

/// A request frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)] // payload is serde_json::Value
pub struct Request {
    pub protocol_version: Version,
    pub correlation_id: Uuid,
    pub operation: String,
    #[serde(default)]
    pub payload: Value,
}

impl Request {
    pub fn new(operation: Operation, correlation_id: Uuid, payload: Value) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            correlation_id,
            operation: operation.as_str().to_string(),
            payload,
        }
    }
}

/// Machine-readable error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    Malformed,
    UnsupportedVersion,
    UnknownOperation,
    Unauthorized,
    Denied,
    NotReady,
    Internal,
}

/// The body of an error response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: ErrorCode,
    pub message: String,
}

/// A response frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)] // result is serde_json::Value
pub struct Response {
    pub protocol_version: Version,
    pub correlation_id: Uuid,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
}

impl Response {
    pub fn ok(correlation_id: Uuid, result: Value) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            correlation_id,
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(correlation_id: Uuid, code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            correlation_id,
            ok: false,
            result: None,
            error: Some(ErrorBody {
                code,
                message: message.into(),
            }),
        }
    }

    pub fn error_code(&self) -> Option<ErrorCode> {
        self.error.as_ref().map(|e| e.code)
    }
}

/// Errors that can occur while decoding a single request frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    Malformed,
    UnsupportedVersion(Uuid),
}

/// Parses a raw frame into a [`Request`], enforcing the protocol version.
pub fn decode_request(line: &str) -> Result<Request, DecodeError> {
    let value: Value = serde_json::from_str(line).map_err(|_| DecodeError::Malformed)?;
    let request: Request = serde_json::from_value(value).map_err(|_| DecodeError::Malformed)?;
    if request.protocol_version.major != PROTOCOL_VERSION.major {
        return Err(DecodeError::UnsupportedVersion(request.correlation_id));
    }
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_round_trip() {
        for op in [
            Operation::HealthGet,
            Operation::StatusGet,
            Operation::ConfigGet,
            Operation::PluginsList,
            Operation::CapabilitiesList,
            Operation::CloudEnroll,
            Operation::CloudStatus,
            Operation::CloudForget,
            Operation::CloudSetPrivilegedExecution,
        ] {
            assert_eq!(op.as_str().parse::<Operation>().ok(), Some(op));
        }
        assert_eq!("nope".parse::<Operation>().ok(), None);
    }

    #[test]
    fn decode_valid_request() {
        let line = serde_json::to_string(&Request::new(
            Operation::HealthGet,
            Uuid::new_v4(),
            serde_json::json!({}),
        ))
        .unwrap();
        let decoded = decode_request(&line).unwrap();
        assert_eq!(decoded.operation, "health.get");
    }

    #[test]
    fn decode_malformed_json() {
        assert_eq!(decode_request("{not json"), Err(DecodeError::Malformed));
    }

    #[test]
    fn decode_unsupported_major_version() {
        let cid = Uuid::new_v4();
        let mut req = Request::new(Operation::HealthGet, cid, serde_json::json!({}));
        req.protocol_version = Version::new(99, 0, 0);
        let line = serde_json::to_string(&req).unwrap();
        assert_eq!(
            decode_request(&line),
            Err(DecodeError::UnsupportedVersion(cid))
        );
    }

    #[test]
    fn response_error_serde_round_trip() {
        let resp = Response::err(Uuid::nil(), ErrorCode::Unauthorized, "denied");
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"ok\":false"));
        assert!(json.contains("\"code\":\"UNAUTHORIZED\""));
        let back: Response = serde_json::from_str(&json).unwrap();
        assert_eq!(back.error_code(), Some(ErrorCode::Unauthorized));
    }
}
