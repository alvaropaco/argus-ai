//! Internal string-grammar helpers shared across identifier types.

/// True when `kind` is a non-empty lowercase-kebab identifier: starts with a
/// lowercase ASCII letter and continues with lowercase letters, digits, or `-`.
pub(crate) fn is_kind(kind: &str) -> bool {
    let mut chars = kind.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// True when `path` is a dotted lowercase path with at least two non-empty
/// segments, each starting with a lowercase ASCII letter (e.g. `host.status.read`).
pub(crate) fn is_dotted_path(path: &str) -> bool {
    let segments: Vec<&str> = path.split('.').collect();
    segments.len() >= 2 && segments.iter().all(|s| is_segment(s))
}

/// True when `segment` starts with a lowercase ASCII letter and continues with
/// lowercase letters, digits, `-`, or `_`.
///
/// The `-`/`_` continuation is the spec-014 extension (ADR-0044 §2): external
/// MCP server and tool names carry them (`mcp.prometheus.query_range`,
/// `mcp.my-server.get_weather`), and the id grammar must accept the
/// deterministic `mcp.<server>.<tool>` namespace without weakening its other
/// rules — a segment still never starts with a digit, `-`, `_`, or an
/// uppercase letter, so no existing id changes meaning.
fn is_segment(segment: &str) -> bool {
    let mut chars = segment.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_grammar() {
        assert!(is_kind("host"));
        assert!(is_kind("k8s-node"));
        assert!(is_kind("foo-bar-2"));
        assert!(!is_kind(""));
        assert!(!is_kind("Host"));
        assert!(!is_kind("1host"));
        assert!(!is_kind("-host"));
        assert!(!is_kind("host_"));
    }

    #[test]
    fn dotted_path_grammar() {
        assert!(is_dotted_path("host.status.read"));
        assert!(is_dotted_path("argus.health.read"));
        assert!(!is_dotted_path("host"));
        assert!(!is_dotted_path("host."));
        assert!(!is_dotted_path(".host"));
        assert!(!is_dotted_path("Host.status"));
        assert!(!is_dotted_path("host.1status"));
        assert!(!is_dotted_path("host.status.read "));
    }

    #[test]
    fn the_mcp_namespace_fits_the_dotted_path_grammar() {
        // Spec 014: deterministic `mcp.<server>.<tool>` ids. Server and tool
        // names carry `-` and `_` — the segments stay lowercase-led, so every
        // pre-existing valid id remains valid and nothing else loosens.
        assert!(is_dotted_path("mcp.metrics.query"));
        assert!(is_dotted_path("mcp.my-server.get_weather"));
        assert!(is_dotted_path("mcp.srv.echo-2"));
    }

    #[test]
    fn the_segment_extension_does_not_admit_malformed_segments() {
        assert!(!is_dotted_path("mcp.-server.tool"), "- cannot lead");
        assert!(!is_dotted_path("mcp._server.tool"), "_ cannot lead");
        assert!(!is_dotted_path("mcp.2server.tool"), "a digit cannot lead");
        assert!(!is_dotted_path("mcp.Server.tool"), "no uppercase");
        assert!(!is_dotted_path("mcp.serv er.tool"), "no spaces");
        assert!(!is_dotted_path("mcp..tool"), "no empty segments");
    }
}
