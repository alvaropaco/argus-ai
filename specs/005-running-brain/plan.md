# Implementation Plan: Spec 005 — Running Brain

| # | Task | Touches |
|---|------|---------|
| T001 | `DeepSeekProvider` adapter (JSON-mode chat completions, fail-closed validation, fallback models) + fixtures | `argus-ai-core` |
| T002 | `[model]`/`[brain]` config parsing; provider factory from config + secret store; `Daemon` provider handle | `argus-daemon` |
| T003 | Brain loop module (evidence from live state, `diagnose_once`, `run_remediation`, memory writes, brain state) + spawn in `main` | `argus-daemon` |
| T004 | IPC `brain.diagnose` + `runbooks.list` (protocol 0.7.0); runbook loading at init | `argus-ipc`, `argus-daemon`, CLI |
| T005 | `argus diagnose` / `argus runbooks` CLI commands | `argus-ai-cli` |
| T006 | Verify (fmt/clippy/tests) + spec checkboxes + commit/push |

Key decisions:
1. **Chat-completions adapter is schema-validated, not free-form**: the
   model answers the typed question contract; output is parsed and
   fail-closed validated. No model text becomes an action.
2. **The loop's autonomy defaults to `l0_observe`** — a fresh install
   observes and explains; execution requires the operator to raise
   `[brain] autonomy`, and then only through the existing policy /
   escalation / approval gates.
3. **Provider absence is a valid state**: the loop and daemon run
   observe-only; `brain.diagnose` returns NotReady with the reason.

Verification: unit tests with a fake provider and canned HTTP fixtures;
an ignored live-API test for the DeepSeek adapter (run on the VPS during
release); full workspace gates; the existing security suites untouched.
