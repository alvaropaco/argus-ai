//! Shared HTTP-over-Unix-socket response parsing for the Docker clients.
//!
//! The Docker daemon answers with `Transfer-Encoding: chunked` whenever it
//! does not know the content length up front (true for `/containers/{id}/json`
//! on current daemons). A naive "split at the blank line, take the rest"
//! parser then feeds the chunk framing to serde — a live-only failure that
//! fixture sockets returning identity-framed bodies never catch. Both the
//! sync (executor) and async (observer) clients parse through here so the
//! framing rules live in exactly one place.

use crate::ContainerError;

/// Split a raw response into its status code, header block, and the decoded
/// body (chunked transfer decoded, identity passed through).
pub fn parse_response(raw: &[u8]) -> Result<(&str, String), ContainerError> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| ContainerError::Parse("malformed HTTP response".into()))?;
    let head = std::str::from_utf8(&raw[..split])
        .map_err(|_| ContainerError::Parse("non-UTF-8 response head".into()))?;
    let status = head
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| ContainerError::Parse("malformed HTTP status line".into()))?;
    let body = &raw[split + 4..];
    let decoded = if is_chunked(head) {
        decode_chunks(body)?
    } else {
        body.to_vec()
    };
    Ok((status, String::from_utf8_lossy(&decoded).into_owned()))
}

/// Whether the header block declares chunked transfer encoding.
fn is_chunked(head: &str) -> bool {
    head.lines().any(|line| {
        let lower = line.to_ascii_lowercase();
        lower.starts_with("transfer-encoding:") && lower.contains("chunked")
    })
}

/// Decode a chunked body: `size\r\n data \r\n` repetitions ending at a zero
/// chunk. Chunk extensions (`size;name=value`) are ignored per RFC 9112 §7.1.
fn decode_chunks(mut input: &[u8]) -> Result<Vec<u8>, ContainerError> {
    let mut out = Vec::new();
    loop {
        let line_end = input
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or_else(|| ContainerError::Parse("truncated chunk header".into()))?;
        let size_token = std::str::from_utf8(&input[..line_end])
            .map_err(|_| ContainerError::Parse("non-UTF-8 chunk size".into()))?
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        let size = usize::from_str_radix(size_token, 16)
            .map_err(|_| ContainerError::Parse(format!("invalid chunk size '{size_token}'")))?;
        input = &input[line_end + 2..];
        if size == 0 {
            break;
        }
        if input.len() < size {
            return Err(ContainerError::Parse("truncated chunk data".into()));
        }
        out.extend_from_slice(&input[..size]);
        input = &input[size..];
        // Each data chunk is followed by CRLF.
        if input.starts_with(b"\r\n") {
            input = &input[2..];
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD_IDENTITY: &str = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n";
    const HEAD_CHUNKED: &str =
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Type: application/json\r\n";

    #[test]
    fn identity_bodies_pass_through() {
        let raw = format!("{HEAD_IDENTITY}\r\n{{\"a\":1}}").into_bytes();
        let (status, body) = parse_response(&raw).unwrap();
        assert_eq!(status, "200");
        assert_eq!(body, "{\"a\":1}");
    }

    #[test]
    fn chunked_bodies_are_decoded() {
        // Two data chunks (3 and 5 bytes) then the zero chunk, exactly as
        // Docker frames them; built byte-wise to keep the framing explicit.
        let mut raw = format!("{HEAD_CHUNKED}\r\n").into_bytes();
        raw.extend_from_slice(b"3\r\n{\"a\r\n");
        raw.extend_from_slice(b"5\r\n\":1}}\r\n");
        raw.extend_from_slice(b"0\r\n\r\n");
        let (_, body) = parse_response(&raw).unwrap();
        assert_eq!(body, "{\"a\":1}}");
    }

    #[test]
    fn chunk_extensions_are_ignored() {
        let mut raw = format!("{HEAD_CHUNKED}\r\n").into_bytes();
        raw.extend_from_slice(b"3;ext=1\r\n{\"a\r\n0\r\n\r\n");
        let (_, body) = parse_response(&raw).unwrap();
        assert_eq!(body, "{\"a");
    }

    #[test]
    fn truncated_chunks_fail_closed() {
        let raw = format!("{HEAD_CHUNKED}\r\nA\r\nshort").into_bytes();
        assert!(parse_response(&raw).is_err());
    }

    #[test]
    fn a_real_shaped_docker_inspect_parses() {
        let json = r#"{"Id":"9b59","State":{"Running":true},"Name":"/argus-live-test"}"#;
        let size = format!("{:X}", json.len());
        let raw = format!("{HEAD_CHUNKED}\r\n{size}\r\n{json}\r\n0\r\n\r\n").into_bytes();
        let (status, body) = parse_response(&raw).unwrap();
        assert_eq!(status, "200");
        let parsed: serde_json::Value = serde_json::from_str(body.trim()).unwrap();
        assert_eq!(parsed["State"]["Running"], true);
    }

    #[test]
    fn responses_without_a_blank_line_are_malformed() {
        assert!(parse_response(b"HTTP/1.1 200 OK\r\n").is_err());
    }
}
