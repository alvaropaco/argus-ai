//! Synchronous (one request → one response) IPC client.

use std::io;
use std::path::Path;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::net::UnixStream;
use uuid::Uuid;

use crate::protocol::{Operation, Request, Response};

/// A line-framed client for the `argusd` Unix socket.
pub struct Client {
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: BufWriter<tokio::net::unix::OwnedWriteHalf>,
}

impl Client {
    pub async fn connect(path: impl AsRef<Path>) -> io::Result<Self> {
        let stream = UnixStream::connect(path).await?;
        let (read, write) = stream.into_split();
        Ok(Self {
            reader: BufReader::new(read),
            writer: BufWriter::new(write),
        })
    }

    /// Sends one request and reads the matching response.
    pub async fn request(
        &mut self,
        operation: Operation,
        correlation_id: Uuid,
        payload: Value,
    ) -> io::Result<Response> {
        let request = Request::new(operation, correlation_id, payload);
        let mut line = serde_json::to_string(&request)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        line.push('\n');

        self.writer.write_all(line.as_bytes()).await?;
        self.writer.flush().await?;

        let mut buf = String::new();
        self.reader.read_line(&mut buf).await?;
        if buf.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed before response",
            ));
        }

        serde_json::from_str(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}
