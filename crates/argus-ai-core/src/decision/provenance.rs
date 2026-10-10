//! Decision provenance: which model, version, calibration set, and context
//! produced an accepted decision.
//!
//! Provenance is data, never authority: it records a decision for audit and
//! reproduction; it never authorizes anything.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::decision::types::DecisionResponse;

/// The sentinel recorded where the decision engine advertises no value.
pub const UNKNOWN: &str = "unknown";

/// The provenance bound to an accepted decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionProvenance {
    pub model_id: String,
    pub model_version: String,
    pub calibration_set: String,
    pub context_hash: String,
}

impl DecisionProvenance {
    /// Binds a decision to the context (`state`) that produced it.
    ///
    /// The context hash is a deterministic FNV-1a hex digest of the canonical
    /// serialized state — stable across processes, so a decision can be
    /// reconstructed from its record. Laya advertises the checkpoint
    /// (`response.model`) but no version; version and calibration set fall back
    /// to [`UNKNOWN`] rather than being fabricated.
    pub fn from_response(response: &DecisionResponse, state: &Value) -> Self {
        let canonical = serde_json::to_string(state).unwrap_or_default();
        Self {
            model_id: response
                .model
                .clone()
                .unwrap_or_else(|| UNKNOWN.to_string()),
            model_version: UNKNOWN.to_string(),
            calibration_set: UNKNOWN.to_string(),
            context_hash: fnv1a_hex(&canonical),
        }
    }
}

/// A deterministic FNV-1a (64-bit) hex digest of `input`.
fn fnv1a_hex(input: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in input.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn response(model: Option<&str>) -> DecisionResponse {
        DecisionResponse {
            model: model.map(str::to_string),
            usage: None,
            answers: std::collections::BTreeMap::new(),
        }
    }

    #[test]
    fn context_hash_is_deterministic_and_state_sensitive() {
        let state = json!({ "evidence": [{"subject": "host:a"}] });
        let first = DecisionProvenance::from_response(&response(Some("english")), &state);
        let second = DecisionProvenance::from_response(&response(Some("english")), &state);
        assert_eq!(first, second, "the same state hashes the same");

        let other = json!({ "evidence": [{"subject": "host:b"}] });
        let third = DecisionProvenance::from_response(&response(Some("english")), &other);
        assert_ne!(
            first.context_hash, third.context_hash,
            "a different state hashes differently"
        );
    }

    #[test]
    fn model_id_falls_back_to_unknown_and_version_is_unknown() {
        let provenance = DecisionProvenance::from_response(&response(None), &json!({}));
        assert_eq!(provenance.model_id, UNKNOWN);
        assert_eq!(provenance.model_version, UNKNOWN);
        assert_eq!(provenance.calibration_set, UNKNOWN);
        assert_eq!(provenance.context_hash.len(), 16, "a 64-bit hex digest");
    }
}
