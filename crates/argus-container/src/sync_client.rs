//! Blocking Docker-socket operations for the remediation executor.
//!
//! The observer client (`client.rs`) is async because it serves the async
//! observation loop; the executor boundary (`argus-executor`) is deliberately
//! sync, so remediation effects (restart a container, read its running state)
//! use a blocking `std::os::unix::net::UnixStream` instead of bridging runtimes.
//! The request/response parsing is shared with the async client.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

use serde_json::Value;

use crate::ContainerError;

/// A minimal blocking client for the Docker socket's control endpoints.
#[derive(Debug, Clone)]
pub struct SyncDockerClient {
    socket_path: String,
}

impl SyncDockerClient {
    pub fn new() -> Self {
        Self::with_socket(crate::client::DEFAULT_SOCKET)
    }

    pub fn with_socket(socket_path: impl Into<String>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    pub fn socket_path(&self) -> &str {
        &self.socket_path
    }

    /// Restarts a container (`POST /containers/{id}/restart`).
    pub fn restart_container(&self, id: &str) -> Result<(), ContainerError> {
        let path = container_resource_path(id, "restart")?;
        let (_, _) = self.request("POST", &path)?;
        Ok(())
    }

    /// Whether the container is currently running (`GET /containers/{id}/json`).
    ///
    /// This is the desired-state read for `container.restart` idempotency and
    /// post-execution validation (ADR-0028 §5, ADR-0031 §2).
    pub fn container_running(&self, id: &str) -> Result<bool, ContainerError> {
        let path = container_resource_path(id, "json")?;
        let (_, body) = self.request("GET", &path)?;
        let state: Value =
            serde_json::from_str(body.trim()).map_err(|e| ContainerError::Parse(e.to_string()))?;
        state
            .get("State")
            .and_then(|state| state.get("Running"))
            .and_then(Value::as_bool)
            .ok_or_else(|| ContainerError::Parse("inspect response lacks State.Running".into()))
    }

    /// One blocking request over the socket, returning `(status, body)`.
    fn request(&self, verb: &str, path: &str) -> Result<(String, String), ContainerError> {
        let mut stream = UnixStream::connect(&self.socket_path)
            .map_err(|e| ContainerError::Unavailable(format!("{}: {e}", self.socket_path)))?;
        let request = format!(
            "{verb} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
        );
        stream
            .write_all(request.as_bytes())
            .map_err(|e| ContainerError::Other(e.to_string()))?;
        let mut buf = Vec::new();
        stream
            .read_to_end(&mut buf)
            .map_err(|e| ContainerError::Other(e.to_string()))?;
        let (status, body) = crate::http_parse::parse_response(&buf)?;
        Ok((status.to_string(), body))
    }
}

impl Default for SyncDockerClient {
    fn default() -> Self {
        Self::new()
    }
}

/// The endpoint path for a container resource, refusing ids that would alter
/// the request target (path traversal, whitespace, query injection).
fn container_resource_path(id: &str, resource: &str) -> Result<String, ContainerError> {
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(ContainerError::Other(format!(
            "container id '{id}' contains characters outside [A-Za-z0-9_-]"
        )));
    }
    Ok(format!("/containers/{id}/{resource}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_ids_are_validated_before_they_reach_a_path() {
        assert!(container_resource_path("abc123", "restart").is_ok());
        assert!(container_resource_path("a-b_c", "json").is_ok());
        assert!(container_resource_path("", "restart").is_err());
        assert!(container_resource_path("../../etc", "restart").is_err());
        assert!(container_resource_path("a b", "restart").is_err());
        assert!(container_resource_path("id?x=1", "restart").is_err());
    }

    // HTTP parsing (identity + chunked) is tested in `http_parse`.
}
