//! Spec-003 Milestone 6 end-to-end suites (T035): the adaptive-layer
//! acceptance criteria — memory similarity (AC-014), gated promotion
//! (AC-015), impact simulation (AC-016), reporting honesty (AC-017),
//! sentinel escalation + autonomy privilege-neutrality (AC-018), safe-mode
//! degradation (AC-019), and the security invariants (AC-020) — plus the
//! full success-signal scenario threading M1–M6 together.

use argus_daemon::sentinel::{
    Environment, SentinelInputs, autofix_plan, sentinel_evaluate, situation_input,
};
use argus_domain::{
    AutonomyMode, BlastRadius, CapabilityId, PolicyOutcome, ResourceId, RiskClass, Severity,
};
use argus_memory::RelationshipKind;
use argus_memory::{Episode, EpisodeOutcome, EpisodicMemory, IncidentSignature, SemanticMemory};
use argus_observability::{DegradationThresholds, SafeMode, SelfHealth, SelfObservability};
use argus_reporting::{ReportInputs, ReportKind, generate_report};
use argus_risk::{
    Risk, RiskKind, SimulationInput, impact_summary, labeled_prose, predict_capacity,
    prediction_breach_risk, simulate_impact,
};
use argus_runbooks::{
    EvidenceKind, Gate, PromotionError, Runbook, RunbookLibrary, RunbookStatus, RunbookTrigger,
    Step,
};
use chrono::{TimeZone, Utc};
use uuid::Uuid;

fn t0() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 5, 12, 0, 0).unwrap()
}

// ---------------------------------------------------------------- AC-014 --

#[test]
fn ac014_a_new_incident_matches_prior_incidents_over_typed_fields() {
    let mut memory = EpisodicMemory::default();
    // Three prior incidents: two close, one different symptom.
    for (subject, symptom, minutes_ago) in [
        ("api", "restart-loop", 60 * 24 * 3),
        ("api", "restart-loop", 60 * 24 * 10),
        ("billing", "oom-killed", 60 * 24),
    ] {
        memory.record(Episode {
            id: Uuid::new_v4(),
            subject: ResourceId::new("service", subject).unwrap(),
            symptom: symptom.to_string(),
            resource_class: "service".to_string(),
            root_cause: Some("memory leak".into()),
            remediation: Some("restart".into()),
            outcome: EpisodeOutcome::Resolved,
            change_proximity: Some(chrono::Duration::minutes(30)),
            started_at: t0() - chrono::Duration::minutes(minutes_ago),
            resolved_at: Some(t0()),
        });
    }

    let signature = IncidentSignature {
        subject_kind: "service".into(),
        symptom: "restart-loop".into(),
        resource_class: "service".into(),
        change_proximity: Some(chrono::Duration::minutes(30)),
    };
    let similar = memory.similar_incidents(&signature, 0.99);
    assert_eq!(similar.len(), 2);
    // Cited evidence: the matched typed fields come back with every score.
    assert!(similar[0].matched_fields.contains(&"symptom"));
    assert!(similar[0].matched_fields.contains(&"change_proximity"));
    // The different-symptom episode is excluded by the threshold.
    for s in &similar {
        assert_eq!(s.episode.symptom, "restart-loop");
    }
}

// ---------------------------------------------------------------- AC-015 --

#[test]
fn ac015_a_candidate_cannot_skip_the_promotion_gates() {
    let mut rb = Runbook::candidate(
        Uuid::new_v4(),
        "learned-disk-procedure",
        RunbookTrigger::Symptom("disk-pressure".into()),
        vec![EvidenceKind::Metrics],
        vec![Step {
            description: "inspect growth".into(),
            evidence: EvidenceKind::Metrics,
        }],
        vec![],
        vec![CapabilityId::new("host.cgroup.freeze").unwrap()],
        vec![CapabilityId::new("host.cgroup.thaw").unwrap()],
        vec![],
    );

    // No gate may be skipped, and promotion without approval is refused.
    assert_eq!(rb.promote(), Err(PromotionError::NotApproved));
    assert_eq!(rb.approve(), Err(PromotionError::NotReadyForApproval));
    // Out-of-order gates are refused.
    assert!(rb.record_gate(Gate::Policy).is_err());

    for gate in [
        Gate::Evaluation,
        Gate::Simulation,
        Gate::Validation,
        Gate::Policy,
    ] {
        rb.record_gate(gate).unwrap();
    }
    rb.approve().unwrap();
    rb.promote().unwrap();
    assert_eq!(rb.status(), RunbookStatus::Promoted);

    // The library only drives procedures with promoted runbooks.
    let mut lib = RunbookLibrary::default();
    lib.register(rb).unwrap();
    let trigger = RunbookTrigger::Symptom("disk-pressure".into());
    assert_eq!(lib.promotable(&trigger).len(), 1);
    let fresh = Runbook::candidate(
        Uuid::new_v4(),
        "another",
        RunbookTrigger::Symptom("disk-pressure".into()),
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
    );
    let mut lib2 = RunbookLibrary::default();
    lib2.register(fresh).unwrap();
    assert_eq!(lib2.promotable(&trigger).len(), 0);
}

// ---------------------------------------------------------------- AC-016 --

#[test]
fn ac016_impact_estimation_cites_blast_dependencies_recovery_and_alternative() {
    let mut semantic = SemanticMemory::default();
    let (batch, api) = (
        ResourceId::new("container", "batch-worker").unwrap(),
        ResourceId::new("service", "api").unwrap(),
    );
    semantic.add_relationship(
        api.clone(),
        RelationshipKind::DependsOn,
        batch.clone(),
        t0(),
    );
    let dependents: Vec<ResourceId> = semantic
        .dependents_of(&batch)
        .into_iter()
        .cloned()
        .collect();

    let estimate = simulate_impact(&SimulationInput {
        capability: CapabilityId::new("host.cgroup.freeze").unwrap(),
        subject: batch.clone(),
        risk: RiskClass::Controlled,
        reversibility: argus_domain::Reversibility::Reversible,
        blast_radius: BlastRadius::Host,
        dependents,
        declared_recovery_time_seconds: Some(30),
        declared_rollback: Some(CapabilityId::new("host.cgroup.thaw").unwrap()),
        alternative: Some(CapabilityId::new("host.cgroup.thaw").unwrap()),
    });

    let summary = impact_summary(&estimate);
    assert!(summary.contains("host.cgroup.freeze"));
    assert!(summary.contains("container:batch-worker"));
    assert!(summary.contains("1 affected dependent(s)"));
    assert!(summary.contains("recovery 30s"));
    assert!(summary.contains("safer alternative: host.cgroup.thaw"));
    assert_eq!(estimate.affected_dependencies, vec![api]);
}

// ---------------------------------------------------------------- AC-017 --

#[test]
fn ac017_the_postmortem_carries_only_recorded_figures() {
    let inputs = ReportInputs {
        open_incidents: vec![argus_incidents::Incident {
            id: Uuid::new_v4(),
            dedup_key: "service:api restart-loop".into(),
            severity: Severity::Error,
            status: argus_incidents::IncidentStatus::Investigating,
            started_at: t0(),
            affected: vec![],
            detection_source: "argus-anomaly".into(),
            root_cause: Some("config regression in commit abc1234".into()),
            resolution: None,
            impact: Some("elevated 5xx on checkout".into()),
        }],
        resolved_incidents: vec![argus_incidents::Incident {
            id: Uuid::new_v4(),
            dedup_key: "service:billing disk-pressure".into(),
            severity: Severity::Warning,
            status: argus_incidents::IncidentStatus::Resolved,
            started_at: t0() - chrono::Duration::hours(2),
            affected: vec![],
            detection_source: "argus-anomaly".into(),
            root_cause: Some("log rotation stalled".into()),
            resolution: Some("restarted logrotate; validated disk fall".into()),
            impact: None,
        }],
        risks: vec![],
        prediction_prose: vec![],
        actions_executed: 2,
        actions_validated: 2,
        actions_rolled_back: 0,
        actions_needing_manual: 0,
        policy_denials: 1,
        pending_approvals: 0,
    };
    let postmortem = generate_report(ReportKind::Postmortem, &inputs, Some(t0()), t0());
    let text = postmortem.render();
    assert!(text.contains("restarted logrotate; validated disk fall"));
    assert!(text.contains("rolled-back actions: 0"));
    assert!(text.contains("log rotation stalled"));
    // The sentinel surfaces the live view with all its sections.
    let view = sentinel_evaluate(
        &SentinelInputs {
            autonomy: AutonomyMode::L2Recommend,
            environment: Environment::Production,
            provider_ready: true,
            open_incidents: 1,
            risks: vec![],
            predictions: vec![],
            pending_approvals: 0,
            recent_actions: 2,
            self_health: SelfHealth {
                event_lag_ms: Some(50),
                ..SelfHealth::default()
            },
            situation: None,
        },
        t0(),
    )
    .view;
    assert_eq!(view.open_incidents, 1);
    assert_eq!(view.recent_actions, 2);
    // An open incident with no Error-severity risk degrades but is not critical.
    assert_eq!(
        view.environment_health,
        argus_daemon::sentinel::EnvironmentHealth::Degraded
    );
}

// ---------------------------------------------------------------- AC-018 --

#[test]
fn ac018_autofix_vs_askhuman_and_privilege_neutrality() {
    // A high-confidence reversible low-blast action auto-fixes at L4.
    let safe = situation_input(
        AutonomyMode::L4Autonomous,
        Environment::Production,
        0.9,
        true,
        RiskClass::LowRisk,
        Some(0.9),
        PolicyOutcome::Allow,
    );
    assert_eq!(
        sentinel_evaluate(
            &SentinelInputs {
                situation: Some(safe),
                autonomy: AutonomyMode::L4Autonomous,
                environment: Environment::Production,
                provider_ready: true,
                open_incidents: 1,
                risks: vec![],
                predictions: vec![],
                pending_approvals: 0,
                recent_actions: 0,
                self_health: SelfHealth {
                    event_lag_ms: Some(50),
                    ..SelfHealth::default()
                },
            },
            t0()
        )
        .escalation,
        Some(argus_policy::Escalation::AutoFix)
    );

    // The same situation at L2 is capped at Recommend — never a silent fix.
    let l2 = situation_input(
        AutonomyMode::L2Recommend,
        Environment::Production,
        0.9,
        true,
        RiskClass::LowRisk,
        Some(0.9),
        PolicyOutcome::Allow,
    );
    assert_eq!(
        argus_policy::decide_escalation(&l2),
        argus_policy::Escalation::Recommend
    );

    // An irreversible high-blast action routes to ASK-HUMAN even at L5.
    let dangerous = argus_policy::EscalationInput {
        reversible: false,
        blast_radius: BlastRadius::Environment,
        risk: RiskClass::HighRisk,
        autonomy: AutonomyMode::L5Adaptive,
        confidence: 0.99,
        ..l2
    };
    assert_eq!(
        argus_policy::decide_escalation(&dangerous),
        argus_policy::Escalation::AskHuman
    );

    // Privilege neutrality: the executor-level gate is identical at every
    // level — L5 gains Controlled but never HighRisk/Destructive.
    for (mode, controlled, high) in [
        (AutonomyMode::L0Observe, false, false),
        (AutonomyMode::L2Recommend, false, false),
        (AutonomyMode::L3Assisted, false, false),
        (AutonomyMode::L4Autonomous, true, false),
        (AutonomyMode::L5Adaptive, true, false),
    ] {
        assert_eq!(
            argus_ai_core::decision::autonomy::may_execute_without_approval(
                mode,
                RiskClass::Controlled
            ),
            controlled
        );
        assert_eq!(
            argus_ai_core::decision::autonomy::may_execute_without_approval(
                mode,
                RiskClass::HighRisk
            ),
            high
        );
    }

    // The autofix plan builder refuses what the decision refused.
    assert!(
        autofix_plan(
            &dangerous,
            &CapabilityId::new("k8s.workload.reschedule").unwrap(),
            serde_json::json!({}),
            None
        )
        .is_none()
    );
    let plan = autofix_plan(
        &safe,
        &CapabilityId::new("host.service.restart").unwrap(),
        serde_json::json!({ "unit": "nginx" }),
        None,
    )
    .unwrap();
    assert_eq!(plan.blast_radius, BlastRadius::Host);
    // confidence passes through f32: compare within f32 precision.
    assert!((plan.confidence - f64::from(0.9f32)).abs() < 1e-9);
}

// ---------------------------------------------------------------- AC-019 --

#[test]
fn ac019_a_broken_sensor_or_stale_queue_degrades_to_a_safe_mode() {
    // A saturated event pipeline (10-minute lag) degrades to StaleInputs…
    let mut health = SelfHealth {
        event_lag_ms: Some(10 * 60 * 1000),
        ..SelfHealth::default()
    };
    let obs = SelfObservability::new();
    for _ in 0..500 {
        obs.record_backlog_delta(1_000);
    }
    health.peak_backlog = obs.snapshot().peak_backlog;
    assert!(health.peak_backlog >= 500_000);
    let mode = argus_daemon::sentinel::safe_mode_from(&health, &DegradationThresholds::default());
    assert_eq!(mode, SafeMode::StaleInputs);

    // …and stale inputs degrade an otherwise-AutoFix escalation to AskHuman.
    let decision = sentinel_evaluate(
        &SentinelInputs {
            autonomy: AutonomyMode::L4Autonomous,
            environment: Environment::Production,
            provider_ready: true,
            open_incidents: 1,
            risks: vec![],
            predictions: vec![],
            pending_approvals: 0,
            recent_actions: 0,
            self_health: health,
            situation: Some(situation_input(
                AutonomyMode::L4Autonomous,
                Environment::Production,
                0.95,
                true,
                RiskClass::LowRisk,
                Some(0.9),
                PolicyOutcome::Allow,
            )),
        },
        t0(),
    );
    assert_eq!(
        decision.view.safe_mode,
        argus_daemon::sentinel::SafeModeSerde::StaleInputs
    );
    assert_eq!(
        decision.escalation,
        Some(argus_policy::Escalation::AskHuman)
    );

    // Executor breaches suspend execution outright.
    let failing = SelfHealth {
        executor_errors: 6,
        ..SelfHealth::default()
    };
    assert_eq!(
        argus_daemon::sentinel::safe_mode_from(&failing, &DegradationThresholds::default()),
        SafeMode::ExecutionSuspended
    );
}

// ---------------------------------------------------------------- AC-020 --

#[test]
fn ac020_security_invariants_hold_across_the_adaptive_layer() {
    // 1. Policy deny is final over every escalation and every level.
    for mode in [AutonomyMode::L4Autonomous, AutonomyMode::L5Adaptive] {
        let denied = argus_policy::EscalationInput {
            autonomy: mode,
            policy: PolicyOutcome::Deny,
            confidence: 1.0,
            reversible: true,
            blast_radius: BlastRadius::None,
            risk: RiskClass::Read,
            ..situation_input(
                mode,
                Environment::Production,
                1.0,
                true,
                RiskClass::Read,
                None,
                PolicyOutcome::Deny,
            )
        };
        assert_eq!(
            argus_policy::decide_escalation(&denied),
            argus_policy::Escalation::Observe
        );
        // And no autofix plan can be built from it.
        assert!(
            autofix_plan(
                &denied,
                &CapabilityId::new("host.service.restart").unwrap(),
                serde_json::json!({}),
                None
            )
            .is_none()
        );
    }

    // 2. Runbook allowed_actions never widen the capability set: an
    //    unregistered capability stays unregistered regardless of runbooks.
    let mut lib = RunbookLibrary::default();
    lib.register(Runbook::candidate(
        Uuid::new_v4(),
        "escape-attempt",
        RunbookTrigger::Manual,
        vec![],
        vec![],
        vec![],
        vec![CapabilityId::new("host.process.signal").unwrap()],
        vec![],
        vec![],
    ))
    .unwrap();
    // The runbook lists it as a *candidate*; it still has to cross policy —
    // the bootstrap policy for host.process.signal requires approval, and
    // the executor's guardrails still apply at run time.
    let rb = lib.by_status(RunbookStatus::Candidate)[0];
    assert!(rb.allows_candidate(&CapabilityId::new("host.process.signal").unwrap()));
    // But a capability the runbook does not list is not even a candidate.
    assert!(!rb.allows_candidate(&CapabilityId::new("k8s.node.drain").unwrap()));

    // 3. A prediction never becomes an authorization: the risk it produces
    //    is advisory and its prose stays labeled.
    let signal = argus_domain::SignalKey::new(
        ResourceId::new("host", "local").unwrap(),
        "disk.used_percent",
    )
    .unwrap();
    let samples: Vec<argus_anomaly::TrendSample> = [72.0, 76.0, 79.0]
        .iter()
        .enumerate()
        .map(|(i, &v)| argus_anomaly::TrendSample {
            at: t0() + chrono::Duration::hours(i as i64),
            value: v,
        })
        .collect();
    let prediction = predict_capacity(signal, &samples, chrono::Duration::hours(24), vec![])
        .expect("enough samples");
    let risk = prediction_breach_risk(&prediction, 90.0, RiskKind::Capacity).unwrap();
    assert!(labeled_prose(&prediction).starts_with("PREDICTED"));
    // The risk carries the "never through this signal" recommendation.
    assert!(
        risk.recommendation
            .as_deref()
            .unwrap()
            .contains("never through this signal")
    );
}

// ------------------------------------------------- success-signal (T035) --

/// The full M1–M6 success-signal scenario in one deterministic thread:
/// pressure observed → predicted → impact simulated → matched against memory
/// → runbook gated → sentinel escalation decided → safe-mode respected.
#[test]
fn the_success_signal_threads_every_milestone_layer_together() {
    // M1: observation of rising disk usage (3 samples, +4%/h with noise).
    let signal = argus_domain::SignalKey::new(
        ResourceId::new("host", "local").unwrap(),
        "disk.used_percent",
    )
    .unwrap();
    let samples: Vec<argus_anomaly::TrendSample> = [72.0, 75.7, 79.6, 83.1]
        .iter()
        .enumerate()
        .map(|(i, &v)| argus_anomaly::TrendSample {
            at: t0() + chrono::Duration::hours(i as i64),
            value: v,
        })
        .collect();

    // M5: a labeled prediction with uncertainty.
    let prediction = predict_capacity(
        signal.clone(),
        &samples,
        chrono::Duration::hours(48),
        vec![argus_domain::ObservationRef::new(Uuid::new_v4()); 4],
    )
    .unwrap();
    assert!(prediction.projected_value() > 90.0);
    let risk = prediction_breach_risk(&prediction, 90.0, RiskKind::Capacity).unwrap();

    // M6 similarity: the same signature matched prior episodes.
    let mut memory = EpisodicMemory::default();
    memory.record(Episode {
        id: Uuid::new_v4(),
        subject: ResourceId::new("host", "local").unwrap(),
        symptom: "disk-pressure".into(),
        resource_class: "host".into(),
        root_cause: Some("log growth".into()),
        remediation: Some("cleanup + expand".into()),
        outcome: EpisodeOutcome::Resolved,
        change_proximity: None,
        started_at: t0() - chrono::Duration::days(14),
        resolved_at: Some(t0() - chrono::Duration::days(13)),
    });
    let similar = memory.similar_incidents(
        &IncidentSignature {
            subject_kind: "host".into(),
            symptom: "disk-pressure".into(),
            resource_class: "host".into(),
            change_proximity: None,
        },
        0.80, // above the 0.65 two-field floor, below the 0.85 three-field score
    );
    assert_eq!(similar.len(), 1);
    assert_eq!(
        similar[0].episode.remediation.as_deref(),
        Some("cleanup + expand")
    );

    // M6 impact: freezing a non-critical consumer is bounded and reversible.
    let mut semantic = SemanticMemory::default();
    semantic.add_relationship(
        ResourceId::new("service", "api").unwrap(),
        RelationshipKind::DependsOn,
        ResourceId::new("container", "batch").unwrap(),
        t0(),
    );
    let estimate = simulate_impact(&SimulationInput {
        capability: CapabilityId::new("host.cgroup.freeze").unwrap(),
        subject: ResourceId::new("container", "batch").unwrap(),
        risk: RiskClass::Controlled,
        reversibility: argus_domain::Reversibility::Reversible,
        blast_radius: BlastRadius::Host,
        dependents: semantic
            .dependents_of(&ResourceId::new("container", "batch").unwrap())
            .into_iter()
            .cloned()
            .collect(),
        declared_recovery_time_seconds: Some(20),
        declared_rollback: Some(CapabilityId::new("host.cgroup.thaw").unwrap()),
        alternative: None,
    });
    assert_eq!(estimate.rollback, argus_risk::RollbackFeasibility::Feasible);

    // M6 runbook: the cleanup procedure is earned through the gates.
    let mut rb = Runbook::candidate(
        Uuid::new_v4(),
        "disk-pressure-cleanup",
        RunbookTrigger::Symptom("disk-pressure".into()),
        vec![EvidenceKind::Metrics],
        vec![],
        vec![],
        vec![CapabilityId::new("host.cgroup.freeze").unwrap()],
        vec![CapabilityId::new("host.cgroup.thaw").unwrap()],
        vec![],
    );
    for gate in [
        Gate::Evaluation,
        Gate::Simulation,
        Gate::Validation,
        Gate::Policy,
    ] {
        rb.record_gate(gate).unwrap();
    }
    rb.approve().unwrap();
    rb.promote().unwrap();

    // M6 sentinel: healthy self → the bounded fix escalates to AUTO-FIX at
    // L4, and the plan builder produces the one-step candidate plan.
    let situation = argus_policy::EscalationInput {
        evidence_quality: argus_policy::EvidenceQuality::Strong,
        confidence: 0.9,
        reversible: true,
        blast_radius: BlastRadius::Host,
        risk: RiskClass::Controlled,
        criticality: argus_policy::Criticality::BestEffort,
        environment: Environment::Production,
        historical_success: Some(0.95),
        autonomy: AutonomyMode::L4Autonomous,
        policy: PolicyOutcome::Allow,
    };
    let decision = sentinel_evaluate(
        &SentinelInputs {
            autonomy: AutonomyMode::L4Autonomous,
            environment: Environment::Production,
            provider_ready: true,
            open_incidents: 1,
            risks: vec![risk],
            predictions: vec![labeled_prose(&prediction)],
            pending_approvals: 0,
            recent_actions: 0,
            self_health: SelfHealth {
                event_lag_ms: Some(80),
                ..SelfHealth::default()
            },
            situation: Some(situation),
        },
        t0(),
    );
    assert_eq!(decision.escalation, Some(argus_policy::Escalation::AutoFix));
    assert_eq!(
        decision.view.environment_health,
        argus_daemon::sentinel::EnvironmentHealth::Critical
    );
    assert!(decision.view.predictions[0].starts_with("PREDICTED"));

    let plan = autofix_plan(
        &situation,
        &CapabilityId::new("host.cgroup.freeze").unwrap(),
        serde_json::json!({ "path": "batch.slice" }),
        Some(CapabilityId::new("host.cgroup.thaw").unwrap()),
    )
    .unwrap();
    assert_eq!(
        plan.steps[0].rollback.as_ref().unwrap().capability.as_str(),
        "host.cgroup.thaw"
    );

    // And the report of record stays honest end-to-end.
    let report = generate_report(
        ReportKind::Capacity,
        &ReportInputs {
            risks: vec![],
            prediction_prose: vec![labeled_prose(&prediction)],
            open_incidents: vec![argus_incidents::Incident {
                id: Uuid::new_v4(),
                dedup_key: "host:local disk-pressure".into(),
                severity: Severity::Warning,
                status: argus_incidents::IncidentStatus::Open,
                started_at: t0(),
                affected: vec![],
                detection_source: "argus-anomaly".into(),
                root_cause: None,
                resolution: None,
                impact: None,
            }],
            ..ReportInputs::default()
        },
        Some(t0()),
        t0(),
    );
    assert!(report.render().contains("PREDICTED (linear-trend"));
}

/// Deterministic-AI suite: the same inputs always produce the same
/// decisions, plans, and reports — the adaptive layer adds no randomness of
/// its own.
#[test]
fn the_adaptive_layer_is_deterministic_end_to_end() {
    let build = || {
        let situation = situation_input(
            AutonomyMode::L4Autonomous,
            Environment::Staging,
            0.8,
            true,
            RiskClass::Controlled,
            Some(0.8),
            PolicyOutcome::Allow,
        );
        let decision = sentinel_evaluate(
            &SentinelInputs {
                autonomy: AutonomyMode::L4Autonomous,
                environment: Environment::Staging,
                provider_ready: true,
                open_incidents: 0,
                risks: vec![Risk {
                    kind: RiskKind::Reliability,
                    subject: ResourceId::new("service", "api").unwrap(),
                    severity: Severity::Warning,
                    evidence: vec!["loop".into()],
                    recommendation: None,
                }],
                predictions: vec!["PREDICTED (linear-trend, confidence 0.5): x".into()],
                pending_approvals: 0,
                recent_actions: 1,
                self_health: SelfHealth {
                    event_lag_ms: Some(10),
                    ..SelfHealth::default()
                },
                situation: Some(situation),
            },
            t0(),
        );
        let plan = autofix_plan(
            &situation,
            &CapabilityId::new("host.service.restart").unwrap(),
            serde_json::json!({ "unit": "nginx" }),
            None,
        );
        (
            decision.view,
            decision.escalation,
            plan.map(|p| (p.objective, p.steps.len(), p.confidence)),
        )
    };
    let a = build();
    let b = build();
    assert_eq!(a, b);
}
