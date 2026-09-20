# Decision Protocol Contract

**Spec:** `specs/002-ai-runtime/spec.md`
**Status:** Draft
**Date:** 2026-09-19

The interface between the Rust `DecisionProvider` adapter and a JEV ("System One")
decision engine (`simple-jev` sidecar or a hosted endpoint). The contract mirrors
the JEV `/v1/classifier` API; it is the wire shape only — the core depends on the
native `argus-ai-core::decision` types, never on the transport.

## 1. Request

```jsonc
{
  "model": "featherless-ai/gemma-4-26B-A4B-classifier",  // engine-specific id
  "state": { /* bounded, deduplicated evidence context */ },
  "questions": {
    "service_healthy": {
      "type": "noul",
      "instructions": "Is the service currently running and healthy?",
      "criteria": { "true": "…", "false": "…" }
    },
    "remediation": {
      "type": "choice",
      "instructions": "Which remediation best matches the observed condition?",
      "criteria": { "restart": "…", "noop": "…" }
    },
    "risk": {
      "type": "score",
      "instructions": "How risky is a restart here?",
      "criteria": ["None", "Low", "High"]
    }
  }
}
```

- `state` is a string, object, or array; the gateway serializes a bounded,
  deduplicated evidence context (FR-002).
- `choice` criteria: 2–50 candidate IDs; `score` criteria: 2–50 ordered levels
  lowest→highest; `noul` criteria: optional `true`/`false` descriptions.

## 2. Response

```jsonc
{
  "model": "…",
  "answers": {
    "service_healthy": { "type": "noul", "noul": 0.93 },
    "remediation": {
      "type": "choice",
      "choice": "restart",
      "confidence": 0.88,
      "probabilities": { "restart": 0.88, "noop": 0.12 }
    },
    "risk": {
      "type": "score",
      "score": 1.2,
      "confidence": 0.7,
      "probabilities": { "0": 0.2, "1": 0.6, "2": 0.2 },
      "legend": { "0": "None", "1": "Low", "2": "High" }
    }
  },
  "usage": { "input_tokens": 600, "output_tokens": 0 }
}
```

## 3. Rules

- The server constructs the JSON from next-token logits; there is no completion
  sampling, streaming, or tool calling.
- `confidence` and `noul` are **uncalibrated**; the gateway uses them for gating
  and ranking only, never as authorization (policy remains authoritative).
- Unknown request fields are ignored; unknown fields inside questions/options are
  rejected.
- A request is atomic: all questions share one context prefix; the engine scores
  each label branch and returns one answer per question.

## 4. Adapter boundary

```text
argus-ai-core::decision::{DecisionQuestion, DecisionAnswer}
        ▲  maps to/from the wire contract
DecisionProvider trait (native Rust)
        ▲  HTTP client
JEV /v1/classifier  (local simple-jev sidecar or hosted)
```
