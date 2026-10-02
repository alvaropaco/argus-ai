//! Docker socket client: a minimal HTTP GET over `tokio::net::UnixStream`.
//!
//! The socket I/O is Linux/Docker-gated, but the response parsing (status line +
//! body split) is a pure function and is unit-tested. Docker's `/containers/json`
//! is a plain JSON body with `Content-Length` and `Connection: close`, so a
//! single request/response round-trip suffices.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use crate::ContainerError;
use crate::model::{Container, parse_containers};

/// Default Docker socket path (Docker Desktop on macOS and dockerd on Linux).
pub const DEFAULT_SOCKET: &str = "/var/run/docker.sock";

/// A minimal client for the Docker socket's `/containers/json` endpoint.
#[derive(Debug, Clone)]
pub struct DockerClient {
    socket_path: String,
}

impl DockerClient {
    pub fn new() -> Self {
        Self::with_socket(DEFAULT_SOCKET)
    }

    pub fn with_socket(socket_path: impl Into<String>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    pub fn socket_path(&self) -> &str {
        &self.socket_path
    }

    /// List all containers with state and restart count.
    pub async fn list_containers(&self) -> Result<Vec<Container>, ContainerError> {
        let body = self.get("/containers/json").await?;
        parse_containers(&body).map_err(|e| ContainerError::Parse(e.to_string()))
    }

    /// One HTTP GET over the socket, returning the response body.
    async fn get(&self, path: &str) -> Result<String, ContainerError> {
        let mut stream = UnixStream::connect(&self.socket_path)
            .await
            .map_err(|e| ContainerError::Unavailable(format!("{}: {e}", self.socket_path)))?;

        let request =
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        stream
            .write_all(request.as_bytes())
            .await
            .map_err(|e| ContainerError::Other(e.to_string()))?;

        let mut buf = Vec::new();
        stream
            .read_to_end(&mut buf)
            .await
            .map_err(|e| ContainerError::Other(e.to_string()))?;

        let response = String::from_utf8_lossy(&buf);
        let (status, body) = parse_http_response(&response)?;
        if status != "200" {
            return Err(ContainerError::Other(format!("HTTP status {status}")));
        }
        Ok(body.to_string())
    }
}

impl Default for DockerClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Split an HTTP/1.1 response into its status code and body.
fn parse_http_response(response: &str) -> Result<(&str, &str), ContainerError> {
    let (head, body) = response.split_once("\r\n\r\n").ok_or_else(|| {
        ContainerError::Parse("response has no header/body separator".to_string())
    })?;
    let status = head
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| ContainerError::Parse("response has no status line".to_string()))?;
    Ok((status, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_status_and_body() {
        let response =
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 3\r\n\r\n[]\n";
        let (status, body) = parse_http_response(response).unwrap();
        assert_eq!(status, "200");
        assert_eq!(body, "[]\n");
    }

    #[test]
    fn rejects_missing_separator() {
        assert!(parse_http_response("HTTP/1.1 200 OK").is_err());
    }

    #[test]
    fn rejects_missing_status() {
        assert!(parse_http_response("HTTP/1.1\r\n\r\nbody").is_err());
    }
}
