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

fn is_segment(segment: &str) -> bool {
    let mut chars = segment.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
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
}
