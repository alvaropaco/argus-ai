# Data Model: ARGUS Autonomous Operations Brain

Extends `docs/architecture/domain-model.md`. New typed entities for this phase;
existing `Observation`, `Incident`, `Plan`, `Action`, `Execution`, `Hypothesis`
are extended where noted. All types are serde-serializable, pure data, no IO
(they live in `argus-domain`).

## 1. Situation (CAP-6)

```rust
struct Situation {
    id: Uuid,
    correlation_key: String,     // shared subject + causal linkage
    members: Vec<EventRef>,      // the correlated events/observations
    confidence: f32,
    first_seen_at: DateTime<Utc>,
    last_seen_at: DateTime<Utc>,
}
```

## 2. Baseline (CAP-4)

```rust
struct Baseline {
    id: Uuid,
    signal: SignalKey,           // e.g. "cpu.utilization" + subject ResourceId
    window: Duration,
    mean: f64,
    variance: f64,
    updated_at: DateTime<Utc>,
    stale_after: Duration,       // freshness marker
}

struct Deviation {
    signal: SignalKey,
    kind: DeviationKind,         // StaticRule | BaselineExcursion | MultiSignal
    severity: Severity,
    evidence: Vec<ObservationRef>,
    detected_at: DateTime<Utc>,
}
```

## 3. Risk (CAP-7, advisory)

```rust
struct Risk {
    id: Uuid,
    kind: RiskKind,              // Reliability | Security | Capacity | Configuration
    subject: ResourceId,
    severity: Severity,
    evidence: Vec<EvidenceRef>,
    recommendation: Option<String>, // deterministic prose only, never generated
    detected_at: DateTime<Utc>,
}
```

## 4. Prediction (CAP-11, labeled)

```rust
struct Prediction {
    id: Uuid,
    signal: SignalKey,
    current_value: f64,
    growth_rate: f64,            // per unit time
    projected_value: f64,
    horizon: Duration,
    method: String,              // e.g. "linear-trend"
    confidence: f32,
    uncertainty: Range<f64>,     // explicit uncertainty band
    evidence: Vec<ObservationRef>,
}
```

## 5. Change (CAP-18)

```rust
struct Change {
    id: Uuid,
    source: ChangeSource,        // Git | Deploy | Image | Config | K8s | Terraform
                                 // | Cloud | Package | Kernel | ServiceConfig
    subject: ResourceId,
    before: Option<String>,
    after: Option<String>,
    actor: Option<String>,
    changed_at: DateTime<Utc>,
}
```

## 6. Runbook (CAP-17)

```rust
struct Runbook {
    id: Uuid,
    name: String,                // e.g. "diagnose_disk_pressure"
    trigger: RunbookTrigger,
    required_evidence: Vec<EvidenceKind>,
    investigation_steps: Vec<Step>,
    decision_criteria: Vec<Criterion>,
    allowed_actions: Vec<CapabilityId>, // candidate capabilities, not grants
    rollback: Vec<CapabilityId>,
    validation: Vec<Criterion>,
    historical_success_rate: f32,
    status: RunbookStatus,       // Candidate | Approved | Promoted
}
```

## 7. Autonomy model (CAP-23)

```rust
enum AutonomyMode { L0Observe, L1Explain, L2Recommend, L3Assisted, L4Autonomous, L5Adaptive }

enum Escalation { Observe, Explain, Recommend, AskHuman, AutoFix }
```

`AutonomyMode` replaces the spec-002 three-variant enum; a persisted-value mapping
preserves compatibility (ADR-0035).

## 8. Investigation (CAP-8)

`Hypothesis` is extended with investigation fields:

```rust
struct InvestigationHypothesis {
    hypothesis: Hypothesis,
    supporting_evidence: Vec<EvidenceRef>,
    contradicting_evidence: Vec<EvidenceRef>,
    confidence: f32,
    tests_performed: Vec<Test>,
    conclusion: Option<Conclusion>,   // RootCause | Eliminated | Inconclusive
    unresolved_uncertainty: String,
}
```

## 9. Memory layers (CAP-16, ADR-0038)

All layers are structured, deterministic records behind `DomainRepository`:

| Layer | Persisted | Backing |
|---|---|---|
| Working | no (in-process) | bounded investigation context |
| Operational | yes | environment graph (`argus-correlate`) |
| Episodic | yes | incident/execution/evidence records |
| Semantic | yes | typed facts + graph relationships (no embeddings) |
| Procedural | yes | runbooks (`argus-runbooks`) |

## 10. Graph nodes and edges (CAP-5, ADR-0033)

Nodes and edges are typed domain enums (never strings). See the BMad spec
`glossary.md` for the full node/relationship catalog. The graph is a projection
over the canonical observation/event store and is rebuildable from it.
