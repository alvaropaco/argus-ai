//! Protocol version negotiation.
//!
//! The cloud is authoritative: the negotiated version is the highest version
//! supported by both peers. Within a major version, changes are additive and
//! unknown fields are ignored.

use semver::Version;

/// The protocol version this implementation speaks.
pub const PROTOCOL_VERSION: &str = "1.0.0";

/// Versions this implementation supports, newest first.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["1.0.0"];

/// Selects the highest mutually supported version, or `None` if there is no
/// overlap. A `None` result means the installation must surface an
/// `UNSUPPORTED_VERSION` diagnostic rather than retry at full rate.
pub fn negotiate(peer_versions: &[String]) -> Option<Version> {
    let mut shared: Vec<Version> = SUPPORTED_PROTOCOL_VERSIONS
        .iter()
        .filter_map(|v| Version::parse(v).ok())
        .filter(|v| {
            peer_versions
                .iter()
                .filter_map(|p| Version::parse(p).ok())
                .any(|p| p == *v)
        })
        .collect();

    shared.sort();
    shared.pop()
}

/// Whether a peer version is compatible for additive tolerance purposes
/// (same major).
pub fn is_same_major(candidate: &Version, reference: &Version) -> bool {
    candidate.major == reference.major
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negotiates_the_shared_version() {
        let peer = vec!["1.0.0".to_string()];
        assert_eq!(negotiate(&peer), Some(Version::new(1, 0, 0)));
    }

    #[test]
    fn no_overlap_yields_none() {
        let peer = vec!["2.0.0".to_string(), "3.1.4".to_string()];
        assert_eq!(negotiate(&peer), None);
    }

    #[test]
    fn malformed_peer_versions_do_not_panic() {
        let peer = vec!["not-a-version".to_string(), "1.0.0".to_string()];
        assert_eq!(negotiate(&peer), Some(Version::new(1, 0, 0)));
    }

    #[test]
    fn empty_peer_list_yields_none() {
        assert_eq!(negotiate(&[]), None);
    }

    #[test]
    fn same_major_reports_additive_compatibility() {
        assert!(is_same_major(
            &Version::new(1, 4, 2),
            &Version::new(1, 0, 0)
        ));
        assert!(!is_same_major(
            &Version::new(2, 0, 0),
            &Version::new(1, 0, 0)
        ));
    }
}
