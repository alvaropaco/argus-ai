//! IPC server transport: accept loop, frame handling, peer credentials.

use std::io;

use argus_domain::Principal;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::net::{UnixListener, UnixStream};
use uuid::Uuid;

use crate::protocol::{DecodeError, ErrorCode, Request, Response, decode_request};

/// Serves IPC requests over a bound [`UnixListener`].
///
/// The handler is invoked once per decoded request with the authoritative
/// peer identity derived from `SO_PEERCRED`. The handler is responsible for
/// authorization and dispatch.
pub async fn serve<H, Fut>(listener: UnixListener, handler: H) -> io::Result<()>
where
    H: Fn(Request, Principal) -> Fut + Send + Sync + Clone + 'static,
    Fut: std::future::Future<Output = Response> + Send,
{
    loop {
        let (stream, _) = listener.accept().await?;
        let handler = handler.clone();
        tokio::spawn(async move {
            if let Err(err) = handle_connection(stream, handler).await {
                tracing::warn!(error = %err, "IPC connection error");
            }
        });
    }
}

async fn handle_connection<H, Fut>(stream: UnixStream, handler: H) -> io::Result<()>
where
    H: Fn(Request, Principal) -> Fut,
    Fut: std::future::Future<Output = Response>,
{
    let principal = stream
        .peer_cred()
        .map(|cred| Principal::new(Some(cred.uid()), Some(cred.gid())))
        .unwrap_or_else(|_| Principal::new(None, None));

    let (read, write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let mut writer = BufWriter::new(write);

    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break;
        }

        let response = match decode_request(&line) {
            Ok(request) => handler(request, principal).await,
            Err(DecodeError::Malformed) => {
                Response::err(Uuid::nil(), ErrorCode::Malformed, "invalid JSON frame")
            }
            Err(DecodeError::UnsupportedVersion(correlation_id)) => Response::err(
                correlation_id,
                ErrorCode::UnsupportedVersion,
                "unsupported protocol version",
            ),
        };

        let mut out = serde_json::to_string(&response)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        out.push('\n');
        writer.write_all(out.as_bytes()).await?;
        writer.flush().await?;
    }

    Ok(())
}
