# Contracts — Events (spec 003)

New domain event families for the Autonomous Operations Brain. Transport is the
local event bus (NATS optional, ADR-012). Event types are string constants in
`argus-events` (matching the existing `types` module pattern).

## Observation / sensor

- `sensor.healthy` / `sensor.degraded` — per-sensor health (ADR-0032).
- `observation.collected` — an observation was recorded (optional high-volume).

## Situation

- `situation.opened` — a new correlated situation was created (CAP-6).
- `situation.updated` — a situation gained members or confidence changed.
- `situation.closed` — a situation resolved or was subsumed.

## Baseline

- `baseline.established` — a baseline became ready for a signal (CAP-4).
- `baseline.deviation` — a multi-signal deviation above baseline was detected.

## Risk

- `risk.detected` — a typed advisory risk signal was emitted (CAP-7).

## Incident

- `incident.opened` — a new incident was created (CAP-10).
- `incident.deduplicated` — an observation merged into an existing incident.
- `incident.status.changed` — lifecycle transition (open→investigating→…→closed).
- `incident.resolved` — resolution recorded.

## Investigation

- `investigation.started` / `investigation.hypothesis.generated` /
  `investigation.hypothesis.eliminated` / `investigation.concluded` (CAP-8/9).

## Prediction

- `prediction.issued` — a labeled prediction was emitted (CAP-11).

## Change

- `change.detected` — a correlated change was observed (CAP-18).

## Runbook

- `runbook.selected` / `runbook.completed` / `runbook.failed` (CAP-17).
- `runbook.candidate.proposed` / `runbook.promoted` — gated promotion (ADR-036).

## Escalation / autonomy

- `escalation.decided` — the escalation decision (OBSERVE/…/AUTO-FIX) (CAP-22).
- `autonomy.level.changed` — operator raised/lowered the autonomy level (CAP-23).

## Self-observability

- `self.degraded` / `self.recovered` — ARGUS degraded to a safe mode / recovered
  (CAP-24).

All events carry the canonical `Event` envelope (id, type, timestamp, source,
subject, severity, evidence, correlation_id, causation_id) per `domain-model.md`
§9.
