//! Typed decision/validation criteria evaluated over typed readings — no
//! free-text predicates, no model calls.

use serde::{Deserialize, Serialize};

/// A comparison a criterion makes, over strings or numbers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Comparison {
    /// Numeric comparison: value < bound.
    LessThan(f64),
    /// Numeric comparison: value ≤ bound.
    AtMost(f64),
    /// Numeric comparison: value ≥ bound.
    AtLeast(f64),
    /// Numeric comparison: value > bound.
    GreaterThan(f64),
    /// Exact string equality (e.g. unit.active_state == "failed").
    Equal(String),
}

impl Comparison {
    /// Apply the comparison to a typed reading.
    fn holds(&self, value: &Reading) -> bool {
        match (self, value) {
            (Comparison::LessThan(bound), Reading::Number(v)) => v < bound,
            (Comparison::AtMost(bound), Reading::Number(v)) => v <= bound,
            (Comparison::AtLeast(bound), Reading::Number(v)) => v >= bound,
            (Comparison::GreaterThan(bound), Reading::Number(v)) => v > bound,
            (Comparison::Equal(expected), Reading::State(v)) => v == expected,
            _ => false,
        }
    }

    /// Human-readable rendering used in deterministic report prose.
    pub fn describe(&self) -> String {
        match self {
            Comparison::LessThan(b) => format!("< {b}"),
            Comparison::AtMost(b) => format!("<= {b}"),
            Comparison::AtLeast(b) => format!(">= {b}"),
            Comparison::GreaterThan(b) => format!("> {b}"),
            Comparison::Equal(s) => format!("== \"{s}\""),
        }
    }
}

/// One typed reading a criterion evaluates against.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Reading {
    Number(f64),
    State(String),
}

/// A declarative decision or validation criterion: `attribute` compared via
/// `comparison`. Evaluation is a lookup + comparison; a missing attribute
/// fails the criterion (fail-closed, never "assumed satisfied").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Criterion {
    /// Deterministic prose describing what this criterion checks.
    pub description: String,
    pub attribute: String,
    pub comparison: Comparison,
}

impl Criterion {
    /// Whether the criterion holds over `readings`. Unknown attributes fail.
    pub fn satisfied(&self, readings: &[(String, Reading)]) -> bool {
        readings
            .iter()
            .find(|(attribute, _)| attribute == &self.attribute)
            .is_some_and(|(_, value)| self.comparison.holds(value))
    }

    /// Deterministic prose for the criterion, for reports and audit.
    pub fn describe(&self) -> String {
        format!(
            "{} ({} {})",
            self.description,
            self.attribute,
            self.comparison.describe()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn readings() -> Vec<(String, Reading)> {
        vec![
            ("disk.used_percent".to_string(), Reading::Number(91.0)),
            (
                "unit.active_state".to_string(),
                Reading::State("failed".into()),
            ),
        ]
    }

    #[test]
    fn numeric_comparisons_hold() {
        let c = Criterion {
            description: "disk nearly full".into(),
            attribute: "disk.used_percent".into(),
            comparison: Comparison::AtLeast(90.0),
        };
        assert!(c.satisfied(&readings()));
        let strict = Criterion {
            comparison: Comparison::GreaterThan(95.0),
            ..c
        };
        assert!(!strict.satisfied(&readings()));
    }

    #[test]
    fn state_equality_holds() {
        let c = Criterion {
            description: "unit failed".into(),
            attribute: "unit.active_state".into(),
            comparison: Comparison::Equal("failed".into()),
        };
        assert!(c.satisfied(&readings()));
    }

    #[test]
    fn missing_attributes_fail_closed() {
        let c = Criterion {
            description: "pressure low".into(),
            attribute: "mem.pressure".into(),
            comparison: Comparison::LessThan(50.0),
        };
        assert!(!c.satisfied(&readings()));
        // A numeric comparison against a state reading also fails.
        let mixed = Criterion {
            attribute: "unit.active_state".into(),
            comparison: Comparison::LessThan(50.0),
            ..c
        };
        assert!(!mixed.satisfied(&readings()));
    }

    #[test]
    fn description_is_deterministic_prose() {
        let c = Criterion {
            description: "disk nearly full".into(),
            attribute: "disk.used_percent".into(),
            comparison: Comparison::AtLeast(90.0),
        };
        assert_eq!(c.describe(), "disk nearly full (disk.used_percent >= 90)");
    }
}
