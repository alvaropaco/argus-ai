//! Bounded, deduplicated, redacted evidence context (FR-002).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The most evidence entries a context carries, so it stays within the decision
/// engine's token budget. Over this, the newest entries win and the oldest are
/// trimmed.
pub const MAX_CONTEXT_ENTRIES: usize = 64;

/// Attribute-name fragments that mark a value as secret-bearing; such a value
/// is masked whole.
const SENSITIVE_MARKERS: [&str; 5] = ["password", "secret", "token", "api_key", "credential"];

/// Value prefixes that mark a string as a secret regardless of its attribute.
const SECRET_VALUE_PREFIXES: [&str; 4] = ["sk-", "ghp_", "AKIA", "-----BEGIN"];

/// The placeholder a redacted value is replaced with.
pub const REDACTED: &str = "[REDACTED]";

fn is_sensitive_attribute(attribute: &str) -> bool {
    let lower = attribute.to_ascii_lowercase();
    SENSITIVE_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
}

/// Deterministic value redaction: any string that looks like a secret, at any
/// depth, is masked.
fn redact_value(value: &Value) -> Value {
    match value {
        Value::String(text) if SECRET_VALUE_PREFIXES.iter().any(|p| text.contains(p)) => {
            Value::String(REDACTED.to_string())
        }
        Value::Array(items) => Value::Array(items.iter().map(redact_value).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), redact_value(value)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// One piece of evidence: a subject, an attribute, and a value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceEntry {
    pub subject: String,
    pub attribute: String,
    pub value: Value,
}

/// Builds the `state` context sent to the decision engine.
///
/// Evidence is deduplicated (on its original value, so distinct secrets stay
/// distinct) so artifacts are not misread as signals, assembled only from
/// allowlisted attributes when an allowlist is set, redacted so secret values
/// never leave the process, and bounded to [`MAX_CONTEXT_ENTRIES`].
#[derive(Debug, Clone, Default)]
pub struct ContextBuilder {
    entries: Vec<EvidenceEntry>,
    /// Raw `(subject, attribute, value)` keys for dedup, before redaction.
    seen: Vec<(String, String, Value)>,
    allowlist: Option<Vec<String>>,
    dropped: usize,
}

impl ContextBuilder {
    /// A builder that accepts any attribute. Prefer [`ContextBuilder::allowlisted`].
    pub fn new() -> Self {
        Self::default()
    }

    /// A builder that accepts only the named attributes; others are dropped.
    pub fn allowlisted<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            entries: Vec::new(),
            seen: Vec::new(),
            allowlist: Some(names.into_iter().map(Into::into).collect()),
            dropped: 0,
        }
    }

    /// Adds an evidence entry.
    ///
    /// An attribute outside the allowlist is dropped and counted. A value whose
    /// attribute is secret-bearing is masked whole; any string value that looks
    /// like a secret is masked. An exact duplicate of the original
    /// `(subject, attribute, value)` is ignored.
    pub fn evidence(
        &mut self,
        subject: impl Into<String>,
        attribute: impl Into<String>,
        value: Value,
    ) -> &mut Self {
        let subject = subject.into();
        let attribute = attribute.into();
        let not_allowed = self
            .allowlist
            .as_ref()
            .is_some_and(|allowlist| !allowlist.iter().any(|allowed| allowed == &attribute));
        if not_allowed {
            self.dropped += 1;
            return self;
        }
        if self
            .seen
            .iter()
            .any(|(s, a, v)| s == &subject && a == &attribute && v == &value)
        {
            return self;
        }
        self.seen
            .push((subject.clone(), attribute.clone(), value.clone()));
        let value = if is_sensitive_attribute(&attribute) {
            Value::String(REDACTED.to_string())
        } else {
            redact_value(&value)
        };
        self.entries.push(EvidenceEntry {
            subject,
            attribute,
            value,
        });
        self
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Entries dropped because their attribute was not allowlisted.
    pub fn dropped(&self) -> usize {
        self.dropped
    }

    /// Serializes the deduplicated, redacted, bounded evidence into a `state`.
    ///
    /// Over the cap, the newest entries win: the oldest are trimmed.
    pub fn into_state(mut self) -> Value {
        if self.entries.len() > MAX_CONTEXT_ENTRIES {
            let excess = self.entries.len() - MAX_CONTEXT_ENTRIES;
            self.entries.drain(0..excess);
        }
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
    use serde_json::json;

    #[test]
    fn deduplicates_identical_evidence() {
        let mut ctx = ContextBuilder::new();
        ctx.evidence("host:a", "memory.pressure", json!(0.8));
        ctx.evidence("host:a", "memory.pressure", json!(0.8));
        ctx.evidence("host:a", "cpu", json!(0.5));
        assert_eq!(ctx.len(), 2);
    }

    #[test]
    fn state_contains_deduplicated_evidence() {
        let mut ctx = ContextBuilder::new();
        ctx.evidence("host:a", "unit", json!("nginx.service"));
        let state = ctx.into_state();
        assert_eq!(state["evidence"][0]["subject"], "host:a");
        assert_eq!(state["evidence"][0]["attribute"], "unit");
    }

    #[test]
    fn sensitive_attribute_value_is_redacted() {
        let mut ctx = ContextBuilder::new();
        ctx.evidence("host:a", "cloud.api_key", json!("sk-live-planted-secret"));
        let state = ctx.into_state();
        assert!(!state.to_string().contains("planted-secret"));
        assert_eq!(state["evidence"][0]["value"], REDACTED);
    }

    #[test]
    fn secret_shaped_value_is_redacted_regardless_of_attribute() {
        let mut ctx = ContextBuilder::new();
        ctx.evidence("host:a", "note", json!("token ghp_plantedsecretvalue"));
        assert!(!ctx.into_state().to_string().contains("plantedsecretvalue"));
    }

    #[test]
    fn nested_string_values_are_redacted() {
        let mut ctx = ContextBuilder::new();
        ctx.evidence("host:a", "note", json!({ "raw": "sk-nestedsecret" }));
        assert!(!ctx.into_state().to_string().contains("nestedsecret"));
    }

    #[test]
    fn distinct_secrets_are_both_kept_each_redacted() {
        let mut ctx = ContextBuilder::new();
        ctx.evidence("host:a", "cloud.secret", json!("sk-first"));
        ctx.evidence("host:a", "cloud.secret", json!("sk-second"));
        assert_eq!(ctx.len(), 2, "distinct secrets are distinct evidence");
        let state = ctx.into_state().to_string();
        assert!(!state.contains("sk-first"));
        assert!(!state.contains("sk-second"));
    }

    #[test]
    fn unknown_attribute_is_dropped() {
        let mut ctx = ContextBuilder::allowlisted(["service.nginx.state"]);
        ctx.evidence("host:a", "service.nginx.state", json!("failed"));
        ctx.evidence("host:a", "unlisted.thing", json!("x"));
        assert_eq!(ctx.dropped(), 1);
        let state = ctx.into_state();
        let entries = state["evidence"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["attribute"], "service.nginx.state");
    }

    #[test]
    fn over_budget_keeps_the_newest_entries() {
        let mut ctx = ContextBuilder::new();
        for i in 0..(MAX_CONTEXT_ENTRIES + 5) {
            ctx.evidence("host:a", "seq", json!(i));
        }
        let state = ctx.into_state();
        let entries = state["evidence"].as_array().unwrap();
        assert_eq!(entries.len(), MAX_CONTEXT_ENTRIES);
        // The oldest 5 (values 0..=4) were trimmed; the first retained is 5.
        assert_eq!(entries[0]["value"], json!(5));
    }
}
