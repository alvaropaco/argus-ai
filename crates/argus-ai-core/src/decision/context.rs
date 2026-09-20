//! Bounded, deduplicated evidence context (FR-002).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// One piece of evidence: a subject, an attribute, and a value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceEntry {
    pub subject: String,
    pub attribute: String,
    pub value: Value,
}

/// Builds the `state` context sent to the decision engine.
///
/// Evidence is deduplicated so artifacts (duplicate readings) are not misread
/// as signals, and the context is bounded to what the caller explicitly adds
/// (no secrets, no unrelated operational state).
#[derive(Debug, Clone, Default)]
pub struct ContextBuilder {
    entries: Vec<EvidenceEntry>,
}

impl ContextBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an evidence entry, ignoring an exact duplicate `(subject,
    /// attribute, value)` triple.
    pub fn evidence(
        &mut self,
        subject: impl Into<String>,
        attribute: impl Into<String>,
        value: Value,
    ) -> &mut Self {
        let entry = EvidenceEntry {
            subject: subject.into(),
            attribute: attribute.into(),
            value,
        };
        if !self.entries.contains(&entry) {
            self.entries.push(entry);
        }
        self
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Serializes the deduplicated evidence into a `state` value.
    pub fn into_state(self) -> Value {
        let evidence: Vec<Value> = self
            .entries
            .into_iter()
            .map(|e| serde_json::to_value(&e).unwrap_or(Value::Null))
            .collect();
        let mut map = Map::new();
        map.insert("evidence".to_string(), Value::Array(evidence));
        Value::Object(map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deduplicates_identical_evidence() {
        let mut ctx = ContextBuilder::new();
        ctx.evidence("host:a", "memory.pressure", serde_json::json!(0.8));
        ctx.evidence("host:a", "memory.pressure", serde_json::json!(0.8));
        ctx.evidence("host:a", "cpu", serde_json::json!(0.5));
        assert_eq!(ctx.len(), 2);
    }

    #[test]
    fn state_contains_deduplicated_evidence() {
        let mut ctx = ContextBuilder::new();
        ctx.evidence("host:a", "unit", serde_json::json!("nginx.service"));
        let state = ctx.into_state();
        assert_eq!(state["evidence"][0]["subject"], "host:a");
        assert_eq!(state["evidence"][0]["attribute"], "unit");
    }
}
