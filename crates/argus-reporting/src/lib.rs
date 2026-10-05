//! ARGUS deterministic reporting (spec 003 M6, CAP-20, FR-021, T032).
//!
//! Seven report kinds — real-time, daily, weekly, security, capacity,
//! root-cause, postmortem — rendered from **supplied, typed inputs only**:
//! every figure in a report comes from the caller's incidents, risks,
//! predictions, and counts. Nothing is invented; where a figure is unknown
//! the report says so (FR-021: "no invented metrics or conclusions").
//!
//! Reports are data and prose, never authorization.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use argus_domain::ResourceId;
use argus_incidents::{Incident, IncidentStatus};
use argus_risk::Risk;

/// The supported report kinds (FR-021).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportKind {
    RealTime,
    Daily,
    Weekly,
    Security,
    Capacity,
    RootCause,
    Postmortem,
}

impl ReportKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RealTime => "real-time",
            Self::Daily => "daily",
            Self::Weekly => "weekly",
            Self::Security => "security",
            Self::Capacity => "capacity",
            Self::RootCause => "root-cause",
            Self::Postmortem => "postmortem",
        }
    }
}

/// The typed inputs a report is rendered from. Every field is supplied by
/// the caller (daemon/cloud/TUI); the report adds no figures of its own.
#[derive(Debug, Clone, Default)]
pub struct ReportInputs {
    /// Open incidents at render time.
    pub open_incidents: Vec<Incident>,
    /// Incidents resolved within the reporting period, with resolution prose.
    pub resolved_incidents: Vec<Incident>,
    /// Active advisory risks at render time.
    pub risks: Vec<Risk>,
    /// Labeled predictions currently held (rendered via their labeled prose).
    pub prediction_prose: Vec<String>,
    /// Actions executed in the period, with how many were validated.
    pub actions_executed: u32,
    pub actions_validated: u32,
    /// Actions rolled back / needing manual intervention in the period.
    pub actions_rolled_back: u32,
    pub actions_needing_manual: u32,
    /// Policy denials and pending approvals at render time.
    pub policy_denials: u32,
    pub pending_approvals: u32,
}

/// One rendered report: a kind, a period, and deterministic prose sections.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub kind: ReportKind,
    pub generated_at: DateTime<Utc>,
    /// The report period, or the moment of rendering for real-time.
    pub period_start: Option<DateTime<Utc>>,
    pub period_end: DateTime<Utc>,
    pub sections: Vec<Section>,
}

/// One titled prose section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    pub title: String,
    pub body: String,
}

impl Report {
    /// Rendered markdown-ish text of the whole report.
    pub fn render(&self) -> String {
        let mut out = format!(
            "# {} report — {}\n",
            self.kind.as_str(),
            self.period_end.to_rfc3339()
        );
        for section in &self.sections {
            out.push_str(&format!("\n## {}\n\n{}\n", section.title, section.body));
        }
        out
    }
}

/// Render a report of the requested kind from the supplied inputs
/// (deterministic; same inputs → same report, modulo `generated_at`).
pub fn generate_report(
    kind: ReportKind,
    inputs: &ReportInputs,
    period_start: Option<DateTime<Utc>>,
    period_end: DateTime<Utc>,
) -> Report {
    let mut sections = Vec::new();
    let _ = period_start;

    match kind {
        ReportKind::RealTime => {
            sections.push(realtime_summary(inputs));
            sections.push(open_incidents_section(inputs));
            sections.push(predictions_section(inputs));
        }
        ReportKind::Daily | ReportKind::Weekly => {
            sections.push(period_summary(kind, inputs));
            sections.push(open_incidents_section(inputs));
            sections.push(resolved_section(inputs));
            sections.push(risks_section(kind, inputs));
        }
        ReportKind::Security => {
            sections.push(Section {
                title: "Security posture".into(),
                body: format!(
                    "policy denials in period: {}; pending approvals: {}; actions needing \
                     manual intervention: {}.\nNo security conclusions are drawn by this \
                     report; the figures above are the recorded audit counts.",
                    inputs.policy_denials, inputs.pending_approvals, inputs.actions_needing_manual
                ),
            });
            sections.push(risks_section(kind, inputs));
        }
        ReportKind::Capacity => {
            sections.push(Section {
                title: "Capacity signals".into(),
                body: if inputs.prediction_prose.is_empty() {
                    "no labeled predictions held; nothing to report.".to_string()
                } else {
                    inputs.prediction_prose.join("\n")
                },
            });
            sections.push(capacity_risks_section(inputs));
        }
        ReportKind::RootCause => {
            sections.push(open_incidents_section(inputs));
            sections.push(Section {
                title: "Root causes on record".into(),
                body: root_causes_prose(inputs),
            });
        }
        ReportKind::Postmortem => {
            sections.push(resolved_section(inputs));
            sections.push(Section {
                title: "Postmortem".into(),
                body: format!(
                    "resolved incidents: {}; rolled-back actions: {}; actions needing manual \
                     intervention: {}.\nEach resolved incident's recorded root cause and \
                     resolution follow.\n{}",
                    inputs.resolved_incidents.len(),
                    inputs.actions_rolled_back,
                    inputs.actions_needing_manual,
                    root_causes_prose(inputs),
                ),
            });
        }
    }

    Report {
        kind,
        generated_at: Utc::now(),
        period_start,
        period_end,
        sections,
    }
}

fn realtime_summary(inputs: &ReportInputs) -> Section {
    Section {
        title: "Now".into(),
        body: format!(
            "open incidents: {}; active risks: {}; labeled predictions: {}; pending \
             approvals: {}.\nexecuted actions to date: {} ({} validated, {} rolled back, \
             {} needing manual).",
            inputs.open_incidents.len(),
            inputs.risks.len(),
            inputs.prediction_prose.len(),
            inputs.pending_approvals,
            inputs.actions_executed,
            inputs.actions_validated,
            inputs.actions_rolled_back,
            inputs.actions_needing_manual,
        ),
    }
}

fn period_summary(kind: ReportKind, inputs: &ReportInputs) -> Section {
    Section {
        title: format!("{} summary", kind.as_str()),
        body: format!(
            "open incidents: {}; resolved in period: {}; active risks: {}; executed \
             actions: {} ({} validated); policy denials: {}.",
            inputs.open_incidents.len(),
            inputs.resolved_incidents.len(),
            inputs.risks.len(),
            inputs.actions_executed,
            inputs.actions_validated,
            inputs.policy_denials,
        ),
    }
}

fn open_incidents_section(inputs: &ReportInputs) -> Section {
    Section {
        title: "Open incidents".into(),
        body: if inputs.open_incidents.is_empty() {
            "none.".to_string()
        } else {
            inputs
                .open_incidents
                .iter()
                .map(|i| {
                    format!(
                        "- {} [{}] {} started {} — detection source: {}{}",
                        i.id,
                        status_name(i.status),
                        i.dedup_key,
                        i.started_at.to_rfc3339(),
                        i.detection_source,
                        i.impact
                            .as_deref()
                            .map(|m| format!("; impact: {m}"))
                            .unwrap_or_default(),
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        },
    }
}

fn resolved_section(inputs: &ReportInputs) -> Section {
    Section {
        title: "Resolved incidents".into(),
        body: if inputs.resolved_incidents.is_empty() {
            "none in the reporting period.".to_string()
        } else {
            inputs
                .resolved_incidents
                .iter()
                .map(|i| {
                    format!(
                        "- {} [{}] {} — resolution: {}",
                        i.id,
                        status_name(i.status),
                        i.dedup_key,
                        i.resolution.as_deref().unwrap_or("not recorded"),
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        },
    }
}

fn risks_section(kind: ReportKind, inputs: &ReportInputs) -> Section {
    // Security reports list only security risks; others list all.
    let risks: Vec<&Risk> = inputs
        .risks
        .iter()
        .filter(|r| {
            kind != ReportKind::Security || matches!(r.kind, argus_risk::RiskKind::Security)
        })
        .collect();
    Section {
        title: if kind == ReportKind::Security {
            "Security risks".into()
        } else {
            "Active risks".into()
        },
        body: if risks.is_empty() {
            "none.".to_string()
        } else {
            risks
                .iter()
                .map(|r| {
                    format!(
                        "- {:?} {} on {}: {}",
                        r.severity,
                        format!("{:?}", r.kind).to_lowercase(),
                        r.subject.as_str(),
                        r.evidence.join("; "),
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        },
    }
}

fn capacity_risks_section(inputs: &ReportInputs) -> Section {
    let capacity: Vec<&Risk> = inputs
        .risks
        .iter()
        .filter(|r| matches!(r.kind, argus_risk::RiskKind::Capacity))
        .collect();
    Section {
        title: "Capacity risks".into(),
        body: if capacity.is_empty() {
            "none.".to_string()
        } else {
            capacity
                .iter()
                .map(|r| {
                    format!(
                        "- {:?} on {}: {}",
                        r.severity,
                        r.subject.as_str(),
                        r.evidence.join("; ")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        },
    }
}

fn predictions_section(inputs: &ReportInputs) -> Section {
    Section {
        title: "Predictions (labeled)".into(),
        body: if inputs.prediction_prose.is_empty() {
            "none held.".to_string()
        } else {
            inputs.prediction_prose.join("\n")
        },
    }
}

fn root_causes_prose(inputs: &ReportInputs) -> String {
    let with_cause: Vec<&Incident> = inputs
        .open_incidents
        .iter()
        .chain(inputs.resolved_incidents.iter())
        .filter(|i| i.root_cause.is_some())
        .collect();
    if with_cause.is_empty() {
        return "no recorded root causes; nothing is inferred here.".to_string();
    }
    with_cause
        .iter()
        .map(|i| {
            format!(
                "- {} ({}): {}",
                i.dedup_key,
                i.id,
                i.root_cause.as_deref().unwrap_or("not recorded"),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn status_name(status: IncidentStatus) -> &'static str {
    match status {
        IncidentStatus::Open => "open",
        IncidentStatus::Investigating => "investigating",
        IncidentStatus::Mitigated => "mitigated",
        IncidentStatus::Resolved => "resolved",
        IncidentStatus::Closed => "closed",
    }
}

/// Convenience: the subject of an incident as a ResourceId when parseable
/// (dedup keys are `kind:name`-shaped); used by callers assembling inputs.
pub fn incident_subject(incident: &Incident) -> Option<ResourceId> {
    // A dedup key is `kind:name extra…` — split off the first two tokens'
    // kind and name and validate through the domain constructor.
    let first_word = incident.dedup_key.split_whitespace().next()?;
    let (kind, name) = first_word.split_once(':')?;
    ResourceId::new(kind, name).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use argus_domain::Severity;
    use chrono::TimeZone;
    use uuid::Uuid;

    fn ts() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 5, 12, 0, 0).unwrap()
    }

    fn incident(status: IncidentStatus, dedup: &str, root_cause: Option<&str>) -> Incident {
        Incident {
            id: Uuid::new_v4(),
            dedup_key: dedup.to_string(),
            severity: Severity::Warning,
            status,
            started_at: ts(),
            affected: vec![],
            detection_source: "argus-anomaly".to_string(),
            root_cause: root_cause.map(String::from),
            resolution: if status == IncidentStatus::Resolved {
                Some("restarted the unit".to_string())
            } else {
                None
            },
            impact: None,
        }
    }

    fn inputs() -> ReportInputs {
        ReportInputs {
            open_incidents: vec![incident(IncidentStatus::Investigating, "service:api restart-loop", Some("memory leak"))],
            resolved_incidents: vec![incident(IncidentStatus::Resolved, "service:billing disk-pressure", Some("log rotation stalled"))],
            risks: vec![Risk {
                kind: argus_risk::RiskKind::Capacity,
                subject: ResourceId::new("host", "local").unwrap(),
                severity: Severity::Warning,
                evidence: vec!["disk 90%".to_string()],
                recommendation: Some("add capacity".to_string()),
            }],
            prediction_prose: vec![
                "PREDICTED (linear-trend, confidence 0.72): disk.used_percent on host:local currently 80.00, projected 98.00 in 6h0m — uncertainty [90.00, 105.00]".to_string(),
            ],
            actions_executed: 12,
            actions_validated: 11,
            actions_rolled_back: 1,
            actions_needing_manual: 0,
            policy_denials: 3,
            pending_approvals: 1,
        }
    }

    #[test]
    fn realtime_report_counts_only_supplied_figures() {
        let r = generate_report(ReportKind::RealTime, &inputs(), None, ts());
        let text = r.render();
        assert!(text.contains("# real-time report"));
        assert!(text.contains("open incidents: 1"));
        assert!(text.contains("active risks: 1"));
        assert!(text.contains("pending approvals: 1"));
        assert!(text.contains("executed actions to date: 12 (11 validated, 1 rolled back"));
        assert!(text.contains("service:api restart-loop"));
        assert!(text.contains("PREDICTED (linear-trend"));
    }

    #[test]
    fn empty_inputs_render_honest_negatives_not_invented_zeros_as_activity() {
        let r = generate_report(ReportKind::Daily, &ReportInputs::default(), None, ts());
        let text = r.render();
        assert!(text.contains("open incidents: 0"));
        assert!(text.contains("none."));
        // The kinds that carry causes/predictions state their absence
        // explicitly rather than inventing content.
        let rc = generate_report(ReportKind::RootCause, &ReportInputs::default(), None, ts());
        assert!(rc.render().contains("no recorded root causes"));
        let cap = generate_report(ReportKind::Capacity, &ReportInputs::default(), None, ts());
        assert!(cap.render().contains("no labeled predictions held"));
    }

    #[test]
    fn security_report_lists_only_security_risks_and_no_conclusions() {
        let r = generate_report(ReportKind::Security, &inputs(), None, ts());
        let text = r.render();
        assert!(text.contains("policy denials in period: 3"));
        assert!(text.contains("No security conclusions are drawn"));
        // The capacity risk is not a security risk: excluded.
        assert!(!text.contains("disk 90%"));
    }

    #[test]
    fn capacity_report_lists_predictions_and_capacity_risks() {
        let r = generate_report(ReportKind::Capacity, &inputs(), None, ts());
        let text = r.render();
        assert!(text.contains("PREDICTED"));
        assert!(text.contains("disk 90%"));
    }

    #[test]
    fn root_cause_report_lists_only_recorded_causes() {
        let r = generate_report(ReportKind::RootCause, &inputs(), None, ts());
        let text = r.render();
        assert!(text.contains("memory leak"));
        assert!(text.contains("log rotation stalled"));
        assert!(
            text.contains("nothing is inferred here") || text.contains("Root causes on record")
        );
    }

    #[test]
    fn postmortem_includes_resolutions_and_rollback_counts() {
        let r = generate_report(ReportKind::Postmortem, &inputs(), None, ts());
        let text = r.render();
        assert!(text.contains("resolved incidents: 1"));
        assert!(text.contains("rolled-back actions: 1"));
        assert!(text.contains("restarted the unit"));
    }

    #[test]
    fn reports_serialize() {
        let r = generate_report(ReportKind::Weekly, &inputs(), Some(ts()), ts());
        let json = serde_json::to_string(&r).unwrap();
        let back: Report = serde_json::from_str(&json).unwrap();
        assert_eq!(back.kind, ReportKind::Weekly);
        assert_eq!(back.sections.len(), r.sections.len());
    }
}
