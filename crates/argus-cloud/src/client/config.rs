//! Configuration delivery decisions.
//!
//! A delivered configuration can arrive out of order, be re-delivered after a
//! reconnect, or be an explicit rollback. Deciding what to do with it *before*
//! touching any settings is what keeps the version the installation reports as
//! applied aligned with the version it actually holds (FR-030, FR-031).

use crate::protocol::messages::ConfigApplyMode;

/// What to do with a delivered configuration version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryDecision {
    /// Newer than what is held: make it effective.
    Advance,
    /// Not newer, so ignored. The reported version stays the version held.
    IgnoreStale,
    /// An explicit rollback to a previously applied version.
    Rollback,
}

impl DeliveryDecision {
    /// Whether honouring this delivery changes what the installation holds.
    pub fn changes_held_version(self) -> bool {
        !matches!(self, Self::IgnoreStale)
    }
}

/// Classifies a delivery against what the installation currently holds.
///
/// A rollback is honoured regardless of version ordering: it is an explicit
/// instruction to reinstate something, not a version to compare. An ordinary
/// delivery only advances on a strictly newer version, so a stale or replayed
/// message cannot move the installation backwards or re-apply what it already
/// has.
pub fn classify_delivery(
    held_version: i64,
    incoming_version: i64,
    mode: ConfigApplyMode,
) -> DeliveryDecision {
    match mode {
        ConfigApplyMode::Rollback => DeliveryDecision::Rollback,
        ConfigApplyMode::Apply => {
            if incoming_version > held_version {
                DeliveryDecision::Advance
            } else {
                DeliveryDecision::IgnoreStale
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ConfigApplyMode::{Apply, Rollback};

    #[test]
    fn a_newer_version_advances() {
        assert_eq!(classify_delivery(4, 5, Apply), DeliveryDecision::Advance);
        assert!(classify_delivery(4, 5, Apply).changes_held_version());
    }

    #[test]
    fn the_version_already_held_is_ignored() {
        // Re-delivery after a reconnect must not re-apply anything.
        assert_eq!(
            classify_delivery(4, 4, Apply),
            DeliveryDecision::IgnoreStale
        );
        assert!(!classify_delivery(4, 4, Apply).changes_held_version());
    }

    #[test]
    fn an_older_version_is_ignored_rather_than_applied() {
        // Out-of-order delivery must not move the installation backwards, or the
        // version it reports would stop matching the version it holds.
        assert_eq!(
            classify_delivery(4, 3, Apply),
            DeliveryDecision::IgnoreStale
        );
        assert_eq!(
            classify_delivery(4, 1, Apply),
            DeliveryDecision::IgnoreStale
        );
        assert!(!classify_delivery(4, 3, Apply).changes_held_version());
    }

    #[test]
    fn applying_from_nothing_advances() {
        // A configuration with nothing applied yet accepts its first version.
        assert_eq!(classify_delivery(0, 1, Apply), DeliveryDecision::Advance);
    }

    #[test]
    fn a_rollback_is_honoured_regardless_of_ordering() {
        // The rollback carries explicit intent, so version comparison does not
        // apply to it.
        assert_eq!(
            classify_delivery(4, 3, Rollback),
            DeliveryDecision::Rollback
        );
        assert_eq!(
            classify_delivery(4, 4, Rollback),
            DeliveryDecision::Rollback
        );
        assert_eq!(
            classify_delivery(4, 9, Rollback),
            DeliveryDecision::Rollback
        );
        assert!(classify_delivery(4, 3, Rollback).changes_held_version());
    }

    #[test]
    fn only_a_stale_ordinary_delivery_leaves_the_held_version_alone() {
        let decisions = [
            (0, 1, Apply, true),
            (1, 1, Apply, false),
            (1, 0, Apply, false),
            (1, 0, Rollback, true),
        ];
        for (held, incoming, mode, expected) in decisions {
            assert_eq!(
                classify_delivery(held, incoming, mode).changes_held_version(),
                expected,
                "held={held} incoming={incoming} mode={mode:?}"
            );
        }
    }
}
