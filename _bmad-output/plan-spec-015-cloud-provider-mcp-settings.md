---
title: 'Spec 015 — Cloud-managed provider & MCP settings'
type: 'feature'
ticket: ''
created: '2026-10-11'
status: 'ready-for-dev'
baseline_revision: 'dc959d2'
route: 'full'
route_source: 'auto'
review: ''
review_source: ''
lenses_ran: []
review_loop_iteration: 0
followup_review_recommended: false
context: ['argus-project/app-platform-argus-ai/specs/015-cloud-managed-provider-mcp-settings/spec.md', 'argus-ai/docs/adr/0045-provider-secrets-write-only-over-control-channel.md']
warnings: []
deferred: []
---

<intent-contract>

## Intent

**Problem:** The AI provider token and the MCP server topology — the two most operator-relevant configurations — require shell access to the host; the dashboard cannot set them, and MCP topology is file-only (spec 014).

**Approach:** A new `mcp` managed-configuration kind (document content, the runbook precedent) applied **live** through the MCP wiring (per-server reconcile: connect added, disconnect removed/changed, unchanged untouched, persisted, re-seeded at startup cloud-over-file); the existing `provider` kind's token delivery surfaces on the dashboard as a **write-only** form; and an instance Settings panel deploying both through the existing configuration ladder with state and errors where the operator is looking.

## Boundaries & Constraints

**Always:** malformed `mcp` content → `config.result failed` with the validation reason, running topology untouched; reconcile is per-server isolated (a failing server never blocks another's apply, the brain loop never waits on the wiring); the token crosses the channel once, is stored 0600 locally, and NEVER appears in any response, log, audit metadata, config-state, or IPC read (presence only: set/unset); applied documents/settings persist and re-seed at startup (cloud supersedes file on server-name conflict for `mcp`; provider settings per the existing managed-settings precedence); reads are viewer-role, deploys operator-role, the price-map-style admin boundary applies to the token deploy; zero deployed configs → byte-identical behavior.

**Never:** the token value readable back from anywhere (cloud, config-state, audit, IPC); no daemon restart required for provider/mcp applies; no cloud-channel EXECUTION of mcp tools (configuration ≠ execution; the host-changing rule stands); no change to the ladder, procedure construction, or autonomy machinery; no MCP server auth in this spec; do not commit `.mimosa/` or user artifacts.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| mcp deploy | valid servers document | Live reconcile (connect added, disconnect removed/changed, unchanged untouched); applied version in config-state | Per-server connect failure → applied with that server in retry state (visible) |
| Malformed mcp content | TOML/JSON parse or validation failure | `config.result failed` + reason; running topology untouched | — |
| Provider token deploy | provider config with api_token | Token stored 0600 in the secret store; applied version in config-state; token value absent from every response | Store failure → failed result |
| Token presence read | config-state / any read | Presence boolean only | — |
| Reconcile per-server failure | a declared server unreachable | Applied; that server in retry/backoff state in config-state; others live | — |
| Restart | daemon restarts | Delivered mcp document + provider settings re-applied from the managed store; file-declared servers preserved (no name conflict) | Store read failure → file config only (logged) |
| Duplicate server name | delivered server conflicts with a file-declared one | Cloud supersedes file | — |

</intent-contract>

## Code Map

**argus-ai** (test: `export PATH="$HOME/.cargo/bin:$PATH"; cargo test --locked --workspace --all-targets`; baseline 1221 passed)
- `crates/argus-daemon/src/cloud.rs` — `apply_configuration:918` (kinds dispatch), `split_provider_content:921` (token → `ProviderCredential`), `store_provider_credential:229` (0600 secret store — the token path EXISTS), `apply_runbook_delivery` (the document-kind pattern from spec 009: validate → apply → persist → result). NEW `apply_mcp_delivery` beside it: parse the content document with the existing `McpConfig`/`McpServerConfig` rules (config.rs), validate, then hand to the wiring.
- `crates/argus-daemon/src/mcp.rs` — `McpWiring` (production/client slots, lifecycle spawn): NEW `apply_servers(new_servers, &mut registry)` reconcile entry — per-name diff (added/removed/changed) against the running slots, per spec 014's isolation discipline; the lifecycle loop adopts the reconciled slots. Investigate the slot shape (`McpClientRuntime` per-server slots) for the disconnect path.
- `crates/argus-daemon/src/config.rs` — `KIND_*` consts `:409+` + `disposition():446`: add `KIND_MCP = "mcp"` (Apply).
- Persistence/re-seed: the delivered document persists in the repository (the delivered-runbooks pattern — `put_json` keyed row, e.g. `mcp_delivery`) and reloads at startup into `RunbookManager`-style init: `Daemon::init_inner` merges managed-delivered servers over `config.mcp` before the wiring spawns. NOTE the startup-order constraint discovered live (2026-10-11): the FIRST cycle after a restart can read the file's `[brain] autonomy` before the managed re-seed lands — the same tolerance applies here (a one-cycle-old topology is acceptable; document it).
- Config-state: `ConfigStatePayload`/`ConfigurationStateEntry` already carry applied versions per configuration — verify the mcp kind flows through it unchanged (it should: the state rows are kind-generic).
- Token presence: config-state for the provider kind gains a `token_set: bool` (from the secret store read) — additive, skip-if-none.
- Tests: `cloud.rs` supervisor tests (the `apply_runbook_delivery` test pattern: applied/malformed/missing-document), daemon init merge test (cloud-over-file precedence), reconcile unit tests in mcp.rs.

**app-platform-argus-ai** (pnpm+turbo; `pnpm build` 8/8, `pnpm test` 12/12)
- Configurations API: the existing versions→deployments ladder carries ANY kind (verify — the autonomy control deploys `brain` configs through it; `provider` too? investigate `apps/api/src/routes/config.ts`); the token field needs the api to pass the content through untouched (it already stores version content server-side; check for any content schema that would strip `api_token`).
- NEW `POST /v1/tenants/:tenant_id/instances/:id/configurations` variant is NOT needed if the configurations API is instance-scoped — investigate `routes/config.ts`: the autonomy control deploys through it; reuse verbatim.
- Web Settings panel: NEW `apps/web/components/instance/settings-panel.tsx` on the instance page — Provider card (base_url, model, token write-only input with set/unset badge from config-state presence) + MCP Servers card (list editor: name/transport/command|url/enabled, per-server state chip from config-state) + Deploy buttons per card (POST configuration version + deployment, the AutonomyControl pattern) + apply status/errors inline. Types in `lib/types.ts`; the config-state hook follows the existing config-state query (already on the instance page).
- Tests: api route tests per the house layout; web typecheck/lint.

## Tasks & Acceptance

**Execution:**
- [x] argus-ai: `KIND_MCP` disposition + `apply_mcp_delivery` (validate via McpConfig rules → reconcile live via the wiring → persist → result) + supervisor tests
- [x] argus-ai: `McpWiring.apply_servers` reconcile (added/removed/changed per name, isolation preserved) + unit tests
- [x] argus-ai: persistence + startup re-seed (managed-delivered servers over file, cloud-over-file precedence) + init merge test
- [x] argus-ai: token presence in config-state (additive, boolean, skip-if-none) + test
- [x] app-platform web: Settings panel (provider form with write-only token + MCP servers editor + deploy/status) + types
- [x] app-platform: verify the configurations API carries both kinds and the token content untouched (no content schema stripping) — fix only if it strips (the content schema never stripped; the KIND enum rejected provider/mcp — extended with a migration; also redacted `api_token` on every content read to `api_token_set` per ADR-0045 and applied the price-map admin boundary to token deploys)
- [ ] both repos — full verification; deploy CLOUD FIRST then daemon; live E2E: deploy an mcp server from the dashboard and watch it connect live (verification commands green in both repos; the deploy + live E2E needs the real cloud/daemon hosts)

**Acceptance Criteria:**
- Given a valid `mcp` deployment from the dashboard, when it applies, then the declared servers connect live (tools adopted, published) and config-state shows the applied version (AC-001).
- Given a subsequent deployment removing/changing a server, when it applies, then exactly that server disconnects/reconnects live, no restart (AC-002).
- Given malformed content, when deployed, then `failed` + reason and the running topology untouched (AC-003).
- Given a provider deployment with an api_token, when applied, then the token is stored 0600 locally and every read surface shows presence only — the value appears nowhere (AC-004).
- Given a restart, when the daemon initializes, then the delivered mcp document and provider settings re-apply from the managed store (AC-005).
- Given the Settings panel open, when deployments happen, then status/errors render where the operator is looking (AC-006).

## Implementation Notes

- Implemented by the full-route subagent (2026-10-11). Daemon: `KIND_MCP` (Apply) with `apply_mcp_delivery` (parse via McpConfig rules → validate fail-closed → persist `mcp_delivery` row → live reconcile → result), `McpWiring.apply_servers` reconcile with per-server ownership (file vs cloud) and shared `apply_order` mutex, startup re-seed merging the delivered document over the file section, `token_set` presence in config-state, `McpServerApplier` port. Cloud+web: ConfigurationKind extended (provider/mcp — it previously REJECTED both), migration 0008 (config kind enum), api token redaction on all content reads (`api_token_set` presence) + admin boundary on token-carrying versions + purge-on-applied + audit boolean, Settings panel (provider form write-only token, MCP servers editor, per-server state chips, deploy/status/errors), `lib/settings.ts` pure grammar helpers.
- Review (quick lens, 2026-10-11): 11 findings — 10 patched, 1 rejected... corrected: all 11 addressed (see triage). The heavy ones: live delivery severing file-declared servers (topology flip-flop), the apply×lifecycle race, missing FR-004 per-server state, the token badge source, the create race, and AC-005's provider-half (delivered settings had NO consumer — now rebuilt live via set_provider).
- Deploy + live (2026-10-11): cloud first (`0008` enum migration), then daemon.

## Auto Run Result

- **Summary:** Spec 015 delivered the operator story end-to-end: MCP server topology and provider settings (base URL, model, API token) are settable from the dashboard Settings panel, deployed through the managed-configuration ladder, applied LIVE by the daemon (per-server reconcile for MCP; live provider rebuild for settings), persisted across restarts with cloud-over-file precedence, and fully visible in config-state (applied versions, per-server state, token presence). ADR-0045's write-only boundary enforced in code: token stored in the version content for delivery, purged on applied, redacted on every read, admin-gated on write, audited as a boolean.
- **Files changed:** argus-ai — config.rs (KIND_MCP + merged_over), mcp.rs (desired-state ownership + apply_servers + server_states), cloud.rs (apply_mcp_delivery + McpServerApplier + token_set in config-state), runtime.rs (init merge + ProviderRebuilder + effective_model), domain/state (McpDeliveryRecord + registry.unregister_server + mcp_delivery row). app-platform — contracts (ConfigurationKind + token_set wire + audit), db (0008 kind enum migration), api (config routes redaction + admin boundary + purge-on-applied), web (Settings panel, settings helpers, tests).
- **Review findings:** quick lens, 11 findings — 10 patched (file-server preservation, per-server state, badge source, create race, apply×lifecycle race, provider consumer, purge-on-applied, failure reason, retry-state hygiene, security route tests) + 1 folded (provider:openai removal), 0 rejected. All re-verified.
- **Follow-up review recommendation: true** — seven medium entries patched. Specific unverified risks: the live reconcile (connect/disconnect of real MCP servers from a dashboard delivery) and the live provider rebuild (DeepSeek re-dial with new credentials) are integration/live checks outstanding at finalize time.
- **Verification performed:** argus-ai 1250 passed / 2 ignored post-patches, clippy 0 warnings, fmt clean. app-platform build 8/8, test 12/12, typecheck 13/13, lint clean; the security route tests ran against a real Postgres under RLS. Diff reviewed by the orchestrator + quick lens.
- **Residual risks:** the delivered token sits in configuration_versions content until the applied purge (tenant-DB trust domain, admin-gated writes, redacted reads — ADR-0045 consequence amended); the reconcile's staged-registry swap can hide a mid-apply lifecycle registration for up to one 10s probe interval (self-heals); startup-order tolerance (one cycle on the file ceiling before the managed re-seed) documented.

## Plan Change Log

## Review Triage Log

### 2026-10-11 — Review pass (quick lens)
- verdicts: 11 findings — high 1, medium 7, low 3, false 0, maybe-false 0
- findings:
  - `[high]` `[patch]` live delivery severs file-declared servers that the restart then restores (topology flip-flop): apply_servers diffs against the MERGED file+cloud set, so servers absent from the delivered document are disconnected live — including file-declared ones — and `merged_over` restores them at restart — verified (runtime.rs init merge + apply_servers reconcile vs desired_servers). Fix: track ownership (file vs cloud) per server; the delivery replaces only the CLOUD-declared subset; file-declared servers without a name conflict are never touched live.
  - `[medium]` `[patch]` FR-004's live per-server MCP state (connected/retrying/failed + tool count) is implemented nowhere: config-state gained only token_set, the panel shows "N declared" + status — a failing server's retry state is invisible to the operator — verified. Fix: config-state carries the wiring's per-server state for the mcp kind; the panel renders state chips.
  - `[medium]` `[patch]` the token presence badge derives from the latest cloud VERSION CONTENT instead of the daemon's config-state: any token-less provider update flips the badge to "token unset" while the 0600 store still holds it — verified (settings-panel reads latestProvider; config-state's token_set is the correct source per the plan). Fix: badge from config-state token_set.
  - `[medium]` `[patch]` the panel's create-if-missing path races the configurations list and the DB unique constraint (configurations_tenant_name_unique): duplicate inserts → opaque 500; stale detail → expected_version −1 → spurious 409 — verified. Fix: map 23505 to fetch-existing-and-continue; gate the deploy on the detail being loaded.
  - `[medium]` `[patch]` apply_servers races the per-server lifecycle pass: a removed/changed server can be reconnected by an in-flight pass and left connected-but-unmanaged (no future pass probes it) — verified structurally (no generation guard between the pass snapshot and apply). Fix: shared ordering (mutex/generation) so apply and lifecycle passes serialize.
  - `[medium]` `[patch]` AC-005's provider half: delivered provider settings have NO consumer — base_url/model changes do nothing live (the provider is never rebuilt) and nothing after restart (no code reads the managed provider keys back); only the token survives — verified (build_provider reads config.model from the file; the managed provider keys have no reader). Fix: live provider rebuild on a provider delivery (set_provider exists) + build_provider preferring the managed provider keys; restart re-applies from the store.
  - `[medium]` `[patch]` ADR-0045 §1's letter violated: the token persists cloud-side in configuration_versions.content (delivery requires storing content) with sha256 in content_hash, no purge/erasure — verified; the redaction covers reads only. Fix: purge-on-applied — when the daemon's config.result reports a provider version applied, the stored content's api_token is replaced by `api_token_set: true` (rollback of that version re-delivers settings without the token — documented); the ADR consequence line amended in the finalize.
  - `[low]` `[patch]` the apply-failure reason round-trips to deployments.detail but the panel renders only the status word — fix: render detail.reason for failed/rejected deploys.
  - `[low]` `[patch]` retry-state entries for removed servers are never cleared (HashMap grows; stale entries scanned) — fix: clear_retry in the removal path.
  - `[medium]` `[patch]` the cloud-side security logic (redactContent on the three read routes, the admin boundary on token deploys, rollback re-delivery) ships with zero tests — ADR-0045 calls regressions there security defects. Fix: route tests for redaction + admin boundary + a token-deploy-by-viewer 403.
  - `[low]` `[patch]` providerContent hardcodes `provider: "openai"` into every deploy — a silent provider switch once F6's consumer lands. Fix: omit the provider key from panel deploys (per-key merge means absent = no override).

## Design Notes

- The runbook kind (009) is the structural precedent: document content, local validation, live effect, persistence, restart re-seed. The mcp kind copies its skeleton and swaps the validator and the live effect.
- The reconcile diff is per-server NAME; "changed" means any transport/command/url/args/enabled delta — unchanged names with identical config are untouched (no session churn).
- Token presence comes from the secret store read (`read_provider_credential().is_ok_and(|s| !s.expose_secret().is_empty())` shape) — never the value.
- The Settings panel deploys via the SAME configurations API the autonomy control uses — one deployment mechanism on the page, three content kinds.

## Verification

**Commands:**
- argus-ai: `export PATH="$HOME/.cargo/bin:$PATH"; cargo test --locked --workspace --all-targets` — expected: all pass (baseline 1221), clippy 0 warnings, fmt clean.
- app-platform: `pnpm build && pnpm test && pnpm typecheck` — expected: 8/8, 12/12, clean.

**Manual checks (post-deploy, cloud first):**
- Dashboard Settings: deploy an mcp server document → `mcp server connected` in the journal + tools in `argus capabilities` + config-state applied; change it → live reconcile; deploy the provider token → cloud status/auth still works on the next provider call and the token value appears nowhere in any response or log.
