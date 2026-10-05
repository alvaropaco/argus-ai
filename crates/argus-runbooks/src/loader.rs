//! Runbook loading from TOML files (spec 004 FR-002).
//!
//! A runbook directory holds `*.toml` files, each describing one runbook.
//! Loading is deterministic (files visited in sorted name order) and
//! graceful: an invalid file, a duplicate name, or an unearnable gate list
//! skips that file with the reason collected — the rest of the library
//! still loads. A file may record gates the runbook has earned, but the
//! loader **replays** them through the real promotion ladder, so no file
//! can grant a status the ladder would refuse (ADR-0036 §3).

use std::path::{Path, PathBuf};

use serde::Deserialize;
use uuid::Uuid;

use crate::criterion::Criterion;
use crate::library::RunbookLibrary;
use crate::promotion::Gate;
use crate::runbook::{EvidenceKind, Runbook, RunbookTrigger, Step};

/// The wire shape of one runbook file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunbookFile {
    pub name: String,
    pub trigger: RunbookTrigger,
    #[serde(default)]
    pub required_evidence: Vec<EvidenceKind>,
    #[serde(default)]
    pub investigation_steps: Vec<StepFile>,
    #[serde(default)]
    pub decision_criteria: Vec<Criterion>,
    /// Candidate typed capabilities — grants of nothing (ADR-0036 §2).
    #[serde(default)]
    pub allowed_actions: Vec<String>,
    #[serde(default)]
    pub rollback: Vec<String>,
    #[serde(default)]
    pub validation: Vec<Criterion>,
    /// Earned gates, replayed in order through the promotion ladder.
    #[serde(default)]
    pub gates: Vec<Gate>,
}

/// One investigation step in a file.
#[derive(Debug, Clone, Deserialize)]
pub struct StepFile {
    pub description: String,
    pub evidence: EvidenceKind,
}

/// The outcome of loading a runbook directory.
#[derive(Debug, Default)]
pub struct LoadResult {
    pub library: RunbookLibrary,
    /// Files skipped, with the reason. Empty when everything loaded.
    pub skipped: Vec<(PathBuf, String)>,
}

impl LoadResult {
    /// Human-readable summary for logs: how many loaded, what skipped.
    pub fn summary(&self) -> String {
        format!(
            "{} runbook(s) loaded, {} skipped{}",
            self.library.len(),
            self.skipped.len(),
            if self.skipped.is_empty() {
                String::new()
            } else {
                format!(
                    ": {}",
                    self.skipped
                        .iter()
                        .map(|(p, r)| format!("{} ({r})", p.display()))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            },
        )
    }
}

/// Load every `*.toml` runbook in `dir`. A missing directory loads an empty
/// library (nothing skipped) — no runbooks is a valid state.
pub fn load_runbooks(dir: &Path) -> LoadResult {
    let mut result = LoadResult::default();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return result,
    };

    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "toml"))
        .collect();
    paths.sort();

    for path in paths {
        match load_one(&path) {
            Ok(runbook) => {
                if let Err(reason) = result.library.register(runbook) {
                    result.skipped.push((path, reason));
                }
            }
            Err(reason) => result.skipped.push((path, reason)),
        }
    }
    result
}

fn load_one(path: &Path) -> Result<Runbook, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("unreadable: {e}"))?;
    let file: RunbookFile =
        toml::from_str(&text).map_err(|e| format!("invalid runbook file: {e}"))?;

    if file.name.trim().is_empty() {
        return Err("name must be non-empty".to_string());
    }

    let allowed = parse_capabilities(&file.allowed_actions)?;
    let rollback = parse_capabilities(&file.rollback)?;
    let steps = file
        .investigation_steps
        .into_iter()
        .map(|s| Step {
            description: s.description,
            evidence: s.evidence,
        })
        .collect();

    let mut runbook = Runbook::candidate(
        Uuid::new_v4(),
        file.name,
        file.trigger,
        file.required_evidence,
        steps,
        file.decision_criteria,
        allowed,
        rollback,
        file.validation,
    );

    // Replay earned gates through the real ladder: the file can only
    // declare what the ladder would allow, never more.
    for gate in file.gates {
        match gate {
            Gate::Approval => runbook
                .approve()
                .map_err(|e| format!("approval gate not earned: {e}"))?,
            Gate::Promotion => runbook
                .promote()
                .map_err(|e| format!("promotion gate not earned: {e}"))?,
            technical => runbook
                .record_gate(technical)
                .map_err(|e| format!("gate ladder violated: {e}"))?,
        }
    }

    Ok(runbook)
}

fn parse_capabilities(raw: &[String]) -> Result<Vec<argus_domain::CapabilityId>, String> {
    raw.iter()
        .map(|c| {
            argus_domain::CapabilityId::new(c).map_err(|e| format!("invalid capability '{c}': {e}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::criterion::Comparison;
    use crate::runbook::RunbookStatus;

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    fn valid_body(name: &str) -> String {
        valid_body_with_gates(name, &[])
    }

    /// All document-root keys sit in the header: in TOML, a key after a
    /// `[[table]]` section belongs to that table, not the root.
    fn valid_body_with_gates(name: &str, gates: &[&str]) -> String {
        let gates = if gates.is_empty() {
            String::new()
        } else {
            format!(
                "gates = [{}]",
                gates
                    .iter()
                    .map(|g| format!("\"{g}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        format!(
            r#"
name = "{name}"
trigger = {{ symptom = "restart-loop" }}
required_evidence = ["service", "logs"]
allowed_actions = ["host.service.restart"]
rollback = ["host.service.restart"]
{gates}

[[investigation_steps]]
description = "read the unit's recent logs"
evidence = "logs"

[[decision_criteria]]
description = "unit is failing"
attribute = "unit.active_state"
comparison = {{ equal = "failed" }}

[[validation]]
description = "unit is active again"
attribute = "unit.active_state"
comparison = {{ equal = "active" }}
"#
        )
    }

    #[test]
    fn a_directory_of_valid_runbooks_loads_deterministically() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "b-second.toml", &valid_body("bravo"));
        write(dir.path(), "a-first.toml", &valid_body("alpha"));
        write(dir.path(), "notes.txt", "not a runbook"); // ignored extension

        let result = load_runbooks(dir.path());
        assert!(result.skipped.is_empty(), "{:?}", result.skipped);
        assert_eq!(result.library.len(), 2);
        let trigger = RunbookTrigger::Symptom("restart-loop".into());
        let names: Vec<&str> = result
            .library
            .matching(&trigger)
            .iter()
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(names, ["alpha", "bravo"]);
        // Everything starts as a Candidate.
        assert_eq!(result.library.by_status(RunbookStatus::Candidate).len(), 2);
    }

    #[test]
    fn a_missing_directory_is_an_empty_library() {
        let result = load_runbooks(Path::new("/nonexistent/runbooks"));
        assert_eq!(result.library.len(), 0);
        assert!(result.skipped.is_empty());
        assert!(result.summary().contains("0 runbook(s) loaded"));
    }

    #[test]
    fn invalid_files_are_skipped_with_reasons() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "good.toml", &valid_body("good"));
        write(dir.path(), "broken.toml", "name = "); // invalid TOML
        write(
            dir.path(),
            "bad-cap.toml",
            &valid_body("bad-cap").replace("\"host.service.restart\"", "\"Not A Capability\""),
        );
        write(
            dir.path(),
            "no-name.toml",
            "trigger = { symptom = \"x\" }\n",
        );

        let result = load_runbooks(dir.path());
        assert_eq!(result.library.len(), 1);
        assert_eq!(result.skipped.len(), 3);
        let reasons: Vec<&str> = result.skipped.iter().map(|(_, r)| r.as_str()).collect();
        assert!(reasons.iter().any(|r| r.contains("invalid runbook file")));
        assert!(reasons.iter().any(|r| r.contains("invalid capability")));
        // The nameless file fails with serde's missing-field error.
        assert!(reasons.iter().any(|r| r.contains("missing field `name`")));
    }

    #[test]
    fn duplicate_names_across_files_skip_the_later_file() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.toml", &valid_body("same-name"));
        write(dir.path(), "b.toml", &valid_body("same-name"));

        let result = load_runbooks(dir.path());
        assert_eq!(result.library.len(), 1);
        assert_eq!(result.skipped.len(), 1);
        assert!(result.skipped[0].1.contains("already used"));
        // The earlier (sorted) file won.
        assert!(result.skipped[0].0.ends_with("b.toml"));
    }

    #[test]
    fn gates_replay_through_the_real_ladder() {
        let dir = tempfile::tempdir().unwrap();
        let promoted = valid_body_with_gates(
            "promoted",
            &[
                "evaluation",
                "simulation",
                "validation",
                "policy",
                "approval",
                "promotion",
            ],
        );
        write(dir.path(), "promoted.toml", &promoted);
        let approved = valid_body_with_gates(
            "approved",
            &[
                "evaluation",
                "simulation",
                "validation",
                "policy",
                "approval",
            ],
        );
        write(dir.path(), "approved.toml", &approved);

        let result = load_runbooks(dir.path());
        assert!(result.skipped.is_empty(), "{:?}", result.skipped);
        assert_eq!(result.library.by_status(RunbookStatus::Promoted).len(), 1);
        assert_eq!(result.library.by_status(RunbookStatus::Approved).len(), 1);
        // Promoted runbooks drive procedures.
        let trigger = RunbookTrigger::Symptom("restart-loop".into());
        assert_eq!(result.library.promotable(&trigger).len(), 1);
    }

    #[test]
    fn unearnable_gate_lists_fail_the_file() {
        let dir = tempfile::tempdir().unwrap();
        // Approval without its technical prerequisites.
        write(
            dir.path(),
            "skip-approval.toml",
            &valid_body_with_gates("skip-approval", &["approval"]),
        );
        // Out-of-order technical gates.
        write(
            dir.path(),
            "out-of-order.toml",
            &valid_body_with_gates("out-of-order", &["simulation"]),
        );

        let result = load_runbooks(dir.path());
        assert_eq!(result.library.len(), 0);
        assert_eq!(result.skipped.len(), 2);
        let reasons: Vec<&str> = result.skipped.iter().map(|(_, r)| r.as_str()).collect();
        assert!(reasons.iter().any(|r| r.contains("not earned")));
        assert!(reasons.iter().any(|r| r.contains("ladder violated")));
    }

    #[test]
    fn comparison_wire_forms_round_trip() {
        // Numeric and state comparisons both deserialize from their wire
        // forms, so files can express both kinds of criterion.
        let numeric: Comparison = toml::from_str("at_least = 90.0").unwrap();
        assert_eq!(numeric, Comparison::AtLeast(90.0));
        let state: Comparison = toml::from_str("equal = 'failed'").unwrap();
        assert_eq!(state, Comparison::Equal("failed".into()));
        let _ = Criterion {
            description: "d".into(),
            attribute: "a".into(),
            comparison: numeric,
        };
        let _ = crate::criterion::Reading::Number(1.0);
    }
}
