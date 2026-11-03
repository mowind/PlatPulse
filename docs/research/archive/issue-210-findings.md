# Issue #210 — research findings (retention policy, safety floors, and bound impact preview)

Ticket: https://github.com/mowind/PlatPulse/issues/210 — milestone 0.2.0, parent #202 (Stage 2, ticket 8/23,
User Stories 37, 38, 41, 42). Scope: let an Owner read the retention policy, edit values inside their allowed bounds
and inspect an authoritative impact preview, with execution bound to the exact preview that was reviewed. This ticket
adds no new deletion capability: the bounded cleanup worker, the `/retention/run` route and `KIND_RETENTION_RUN` all
predate this ticket at HEAD 3d25fb7, and this ticket instead narrows execution (free-form family scope is gone; a run
now requires a live `previewId` and executes the plan that preview froze).

## SERVER (the real gaps: no floor, rebuilt plan, free-form run)

`crates/platpulse-server/src/retention.rs` existed with a family catalog (`POLICY_CATALOG`, one `PolicyDefaults` per
data family), a read-only `estimate_impact`, and an execution path that rebuilt its own plan at run time from
`build_plan` + `batch_sql` (nine hand-written DELETE strings chosen by family, with the cutoff computed from the
*current* policy). Three defects followed from that shape: a policy could be saved below the investigation floor
(the catalog only knew per-family `min_days`, so `raw_block_summary` could be set under 24 hours); a run re-estimated
its own scope, so a policy edited between review and execution silently changed what was deleted; and an estimated
count was treated as a frozen row set, so a family whose estimate exceeded the actually expired rows kept returning
"one short" and the entry was retried forever.

What was added (all in `crates/platpulse-server/src/retention.rs`, now 2471 lines):

- Policy classes and floors: `pub enum PolicyClass { Raw, Investigation, Contract }` (:69) with `floor_days()`
  (raw = `MIN_INVESTIGATION_RAW_DAYS` = 1, investigation = `MIN_INVESTIGATION_AGGREGATE_DAYS` = 30, contract = 0) and
  `floor_reference()` returning the design §11.4 sentence for each non-zero floor; `MIN_INVESTIGATION_RAW_HOURS = 24`
  (:37) exists so the raw floor can be asserted in hours. `PolicyDefaults` gained `class`, `targets`, and
  `pub fn safety_floor_days(&self) -> i64 { self.min_days.max(self.class.floor_days()) }` (:147). `validate_policy_days`
  now rejects anything under that floor with `"{} cannot be lowered below {} days ({})"` (:570) carrying the class's
  design reference, so the refusal explains itself.
- Cleanup targets as fixed SQL: `pub enum CleanupKind { Delete, SlimReceiptBody }` and
  `pub struct CleanupTarget { table, kind, count_sql, delete_sql }`; nine target consts
  (`TARGET_BLOCK_SUMMARIES`, `TARGET_HISTORY_GAPS`, `TARGET_DIVERGENCE_EVIDENCE`,
  `TARGET_AUDIT_EVENTS`, `TARGET_NOTIFICATION_EVENTS`, `TARGET_PEER_PRESENCE_INTERVALS`, `TARGET_PEER_AGGREGATE_5M`,
  `TARGET_PEER_AGGREGATE_1H`, `TARGET_RECEIPT_BODIES`, plus `NO_CLEANUP_TARGETS`) whose SQL was extracted verbatim
  from the deleted `estimate_impact`/`batch_sql` bodies, each with exactly one `?` bind and `LIMIT 128`
  (`RETENTION_BATCH`, :32). `pub fn catalog_targets(family) -> &'static [CleanupTarget]` (:371).
- `plan_family_targets` (:657) + `count_targets` (:672) do the counting `estimate_impact` used to do inline; the
  read-only `estimate_impact` keeps its signature and delegates.
- `pub fn audit_catalog() -> Result<(), String>` (:1470) proves the contract at boot and in tests: unique families,
  bounds consistent with defaults, supported+bounded families above their class floor, unsupported families declaring
  no target, no duplicate table per family, exactly one bind per count SQL, Delete SQL ending in `LIMIT`, and at least
  one supported Raw and one supported Investigation family. `crates/platpulse-server/src/cli.rs` calls it before the
  cutover check and prints `retention catalog contract violated: {violation}`.
- Preview record: `pub enum PreviewLoad { Ready, NotFound, Stale(Vec<String>) }` (:720), `pub enum PreviewError`,
  `PreviewSkip { family, code, message }`, `PreviewFamily { family, retention_days, policy_version, cutoff,
  estimated_rows, targets }`, `RetentionPreview { preview_id, created_at, created_by, expires_at, policy_version,
  scope, families, skipped, estimated_rows }`, and a `PreviewRow` for sqlx.
- Versions: `policy_version(family, retention_days, updated_at)` (:810) and `preview_version(families)` (:818) are
  the hex sha256 of their parts (`crate::secrets::encode_hex`); `preview_notes()` (:838) is the three-sentence
  Server note (estimates are upper bounds; the preview is bound to these policy versions, scope and cutoffs; bounded
  deletes touch no protected state).
- `create_preview(pool, created_by, families, now)` (:884): an explicit scope is deduped and an unknown or empty list
  is refused (`PreviewError::InvalidScope`, surfaced as 400 `invalid_query` on `families`); a default-scope preview
  silently takes every family with `skip_reason == None` and records no skips, while an explicit scope records every
  requested family the Server will not act on (`retention_unknown_family`, `retention_unsupported`,
  `retention_disabled`, `retention_keep_forever`, :847-871). The id is deterministic:
  `"rp-" + sha256("{version}|{scope_json}|{created_at}")[..32]` (:1007), written with an UPSERT after pruning
  `expires_at <= now` in batches of 128 (`PREVIEW_TTL_HOURS = 24`, :42).
- `load_preview_for_run(pool, preview_id, now)` (:1016) is the binding check. Missing → `NotFound`; unreadable JSON or
  an expired row → `Stale`; a default-scope preview recomputes the enabled/supported/bounded family set and rejects a
  changed set; an explicit-scope preview compares the recorded skip codes with the current ones; every entry re-checks
  supported / enabled / `retention_days` / `policy_version`. Reasons are deduped by `push_reason` (:1150) and
  truncated to three plus `"and {n} more change(s)"`. `latest_live_preview` (:1158) returns the newest unexpired
  preview that still loads `Ready`, and `preview_plan` (:1181) converts it to the execution plan.
- Execution now follows the preview: `PlanEntry` gained `#[serde(default)] done: bool`; `execute_step` requires
  `params["plan"]` and fails closed with `retention_plan_missing` (:1255) instead of rebuilding one; `build_plan`
  and `batch_sql` are deleted; the target row is looked up through `catalog_targets` and dispatched either to
  `SlimReceiptBody` or to the target's own `delete_sql`; `apply_batch` marks an entry `done` on a zero-row batch,
  which fixes the infinite-retry defect without pretending the estimate was exact; `finish_run` reports per-family
  `deletedRows`/`estimatedRows` and returns `succeeded_with_warnings` when the recorded `skippedWarnings` are
  present (recorded once, behind the persisted `warningsRecorded` flag).

Schema and API:

- `crates/platpulse-server/migrations/0063_retention_previews.sql` (37 lines): `retention_previews(preview_id PK,
  created_at, created_by, policy_version, scope_json, entries_json, estimated_rows >= 0, expires_at)` plus an
  `expires_at` index; `SERVER_SCHEMA_VERSION` 62 → 63 in `crates/platpulse-server/src/database.rs:24`.
- `crates/platpulse-server/src/http/operations_admin.rs`: `RetentionPolicyDto` gained `policyVersion`;
  `RetentionOverview` gained `preview`; new `RetentionPreviewDto` (+ `RetentionPreviewFamily`,
  `RetentionPreviewSkip`, `RetentionPreviewRequest`); `RetentionPolicyUpdateRequest` is now
  `{ retentionDays, expectedPolicyVersion }`; `RetentionRunRequest` is now `{ previewId }` alone. New handler
  `retention_preview` on `POST /api/admin/v1/retention/preview` behind `mutation_guard(..., true)`, and the router
  registers it beside `/retention/impact`. `retention_run` loads the preview first: `NotFound` → 404
  `retention_preview_not_found` (:1554, "the confirmed retention preview does not exist or has expired; compose a new
  preview"), `Stale` → 409 `retention_preview_stale` (:1566) with
  `"the retention preview no longer matches the current policies ({}); compose a new preview"` (:1515); on success the
  queued Operation params carry `previewId`, `previewPolicyVersion`, `skippedWarnings` and the frozen `plan`.
  `update_retention_policy` compares the submitted `expectedPolicyVersion` with the freshly read
  `retention_policies` row and answers 409 `retention_policy_version_conflict` (:1344, "the policy changed since it was
  read; reload the current value and preview again") without writing. `retention_overview` exposes
  `latest_live_preview` so a reloaded page offers the same reviewed plan.
- `crates/platpulse-server/src/openapi.rs` registers the new DTOs, and `docs/openapi/openapi.json` is regenerated with
  `cargo run -p platpulse-server --quiet -- --print-openapi > docs/openapi/openapi.json` (README.md:155 and
  .github/workflows/ci.yml:40 both fail on a diff), then the strict client via `npm run generate:api`.

## DECISIONS

- Freeze the plan at queue time, not at execution: `build_plan` was deleted rather than kept as a fallback, because
  anything re-derived at run time can diverge from what the Owner reviewed. The run is now idempotent with respect to
  later policy edits by construction.
- The policy version is an opaque `sha256(family|days|updated_at)` rather than a new column, because
  `retention_policies` has no version column and adding one is schema churn for one endpoint. Timestamps are recorded
  at second precision, so two edits inside the same second can share an `updated_at`; `load_preview_for_run` therefore
  compares `retention_days` explicitly on every entry (the stale-policy test uses 7 vs 21 days precisely so the digest
  differs either way).
- The floor lives in the catalog (`min_days.max(class.floor_days())`) instead of in the handler, so a future family
  cannot be added below the investigation constraint, and `audit_catalog` turns that into a boot-time and test-time
  invariant.
- Families the Server cannot clean declare no target at all, so the preview can never imply history that does not
  exist; `skipped` carries the Server's own reason code and message rather than a frontend guess.
- Composing a preview audits nothing (it changes nothing); only an accepted policy save and an accepted run write Audit
  Events. The estimate is documented as an upper bound, and a shortfall completes an entry instead of failing a run.
- Evidence boundaries stay intact: the targets name Block Summary, history gaps, divergence evidence, Audit,
  notification events, peer presence/aggregates and receipt bodies only; Report/receipt identity, Incident evidence,
  Audit records themselves and the independent Purge boundary are never targeted, and a slimmed receipt keeps its
  disposition, sequence and identity while only its body is emptied.

## DOCS

- `docs/design/webui.md`: status line, the not-routed list, the §4.3 route table row `PAGE-ADMIN-RETENTION`
  (`/admin/retention`), the API-only paragraph, the §14 change log, and a new §15.13 "Retention policy, safety floors,
  and bound impact preview (issue #210, delivered)".
- `docs/design/platpulse.md`: registered-page mentions, the API-only paragraph, and a §11.4 paragraph describing the
  retention catalog contract (classes and floors, refusal below a floor, no target for unsupported families, fixed-SQL
  bounded batches following the preview's cutoff, estimate as an upper bound, no auto-shortening of recorded history).
- `README.md` was left as-is: its Admin route list is a curated overview that already omits operations, doctor and
  backups, and issue #209 set the precedent of not touching it for a new routed page.

## WORKFLOW

- Labels on close: remove `ready-for-agent`, add `implemented` (the convention closed issues such as #207 and #209
  follow).
- Frontier after #210: #211 retention execution/cancel/dedupe (blocked by this ticket), #212-#217 history, #218/#219
  validators, #220/#221 investigation, #222+.

## WEB (the routed surface, and the debt it exposed)

- New `platpulse-web/src/pages/AdminRetention.tsx` (959 lines) with four panels — the policy table (family label,
  current value, `minDays`/`maxDays`, default, supported/enabled, last write, Edit per actionable family), the
  single-family editor (typed confirmation `retention <family> <days>`, Server `retention_impact` estimate, 409
  `retention_policy_version_conflict` recovery), the preview panel (compose, scope, per-family cutoff, expiry, notes,
  `skipped` reasons) and the run panel (queue `previewId`, 409 `retention_preview_stale` and 404
  `retention_preview_not_found` recovery, link to the recorded Operation) plus protected-state and last-run cards, with
  17 stable `data-slot` hooks.
- `AdminRetention.test.tsx` (616 lines, 11 tests) covers the catalogue render including an unsupported family, bounds
  refusals, the audited save notice, the version conflict, preview composition, an expired preview, the stale-run
  refusal with recovery, the last-run link and last-good/`0`-is-not-evidence semantics; `e2e/retention-acceptance.spec.ts`
  (482 lines, 4 tests) drives the production WebUI against a disposable real Server over HTTP for tests 1-3 and checks
  geometry/theme/focus/44x44/local-scroll on all five viewports in test 4.
- Navigation and routing: `AdminLayout.tsx` gained the `Retention` NavLink (Hourglass icon, after Doctor, before
  Backups), `App.tsx` the route `/admin/retention` (legacy `/admin/data/retention` still hits the Admin fallback), and
  `App.test.tsx` the route table, nav count 15 -> 16, active-link chain, glyph and removed-label expectations.
  `api/admin.ts` gained `createRetentionPreviewEntry` and adapted `updateRetentionPolicyEntry` /
  `runRetentionEntry`; `src/api/generated/` is regenerated from the Server's OpenAPI, not hand-edited.
- Two e2e lessons worth keeping: a policy row must be located by the Server family **key**, because `hasText` is
  substring-based and `"1-Hour Aggregates"` also matches the `Peer 1-Hour Aggregates` row (strict-mode violation); and
  below the `lg` breakpoint the Admin nav is a closed drawer (`AdminLayout.tsx:205`), so `includeHidden: true` is needed
  on **both** the nav locator and the inner link locator or the active-link chain resolves to nothing.
- Pre-existing red specs found while running the suite, and repaired mechanically because #209 (commit 3d25fb7) shipped
  the Backups nav entry without updating them: `e2e/emerald-refinement.spec.ts:162` expected 14 nav icons while HEAD
  already had 15 (now 16), and `e2e/convergence-acceptance.spec.ts:272` expected the Admin nav to equal a 14-entry
  `MVP_ADMIN_SECTIONS` that still listed `Retention`/`Backups` in `REMOVED_ADMIN_LABELS`.
- Gate results: `npm run lint && npm run typecheck && npm test && npm run build` exit 0 (37 files / 573 tests);
  `npx playwright test e2e/retention-acceptance.spec.ts` 8 passed / 12 skipped / 0 failed across all five projects.

## REVIEW (two axes, provider `cliproxyapi`, model `gpt-6.1-sol`, reasoning effort high, fixed point HEAD 3d25fb7)

### Standards

The standards axis reported **0 findings; worst: none** — no documented-standard or Fowler-smell problem in the frozen
backend/docs diff. It handed two behavioral leads to the Spec axis rather than claiming them itself: the same-second
policy-version resurrection (`retention.rs:810-813` with `auth.rs:409-414`) and the TOCTOU between
`load_preview_for_run` and `queue_operation` (`operations_admin.rs:1542-1598`).

### Spec

Verbatim Spec-axis findings:

- **P1 MUST — scope creep:** the production “Run retention” calls the deletion worker (`AdminRetention.tsx:811`,
  `retention.rs:1334–1338`); violates “本票不执行删除”. An existing worker does not justify newly exposing deletion in
  this ticket.
- **P1 MUST — stale-preview race:** validation finishes before the independent queue transaction
  (`operations_admin.rs:1542–1598`, `operations.rs:81`); a concurrent policy edit can commit between them, yet the old
  preview queues. Violates Story 38 and “旧版本/范围/截止条件变更须重预览”.
- **P1 MUST — nonunique policy versions:** hashes use days plus second-resolution timestamps (`retention.rs:810–814`,
  `auth.rs:409–414`); same-second A→B→A restores the old version and revives stale confirmation. Violates Story 38’s
  changed-version re-preview requirement.
- **P2 MUST — estimate remains execution quota:** entries stop when deleted reaches estimated total (`retention.rs:1282`);
  zero estimates never query, underestimated targets finish with eligible rows remaining. Violates “计数是估计不是冻结行集”
  and the advertised exhaustion-based cleanup contract.
- **P2 MUST — incomplete strict API contract:** policy updates return 409 but OpenAPI omits it
  (`operations_admin.rs:1267`); generated errors consequently exclude conflicts (`types.gen.ts:4901–4905`). Leaves
  “OpenAPI及严格客户端” conflict delivery partial.

Spec coverage: floors (raw/investigation) verified Server-enforced; longer existing values survive seeding; unsupported
families claim no retained history; persisted Server-authoritative scope/cutoffs/expiry, sanitized preview DTOs, used
schema, production route, generated preview client and delivered-surface docs verified; transactional policy Audit and
ordinary stale/conflict recovery verified; real production-WebUI→HTTP→SQLite Owner flows and five-viewport
theme/focus/local-scroll acceptance tests exist. Not established by that axis: negative Viewer/Guest/Origin-CSRF and
identity-switch acceptance, last-good age, comprehensive protected-Audit preservation (it ran no suites).

Spec: 5 findings; worst: production retention execution deletes data despite the ticket's explicit no-deletion scope.

### Disposition (what changed, what was refused and why)

1. **Scope creep — refused, with evidence.** The deletion worker, the `/retention/run` route
   (`operations_admin.rs:1940` at HEAD), its `RetentionRunRequest` (:487), the handler (:1323), the
   `KIND_RETENTION_RUN` dispatch (`operations.rs:33,362`) and all nine `DELETE FROM` statements inside `execute_step`
   (`retention.rs:649-673` at HEAD) pre-date this ticket, so “本票不执行删除” was read as *this ticket adds no deletion
   capability*. That reading is the only one consistent with the ticket's own acceptance criterion for Story 38
   (“execution bound to the preview's policy version, scope, and cutoff”) and with its demand for a production-route UI.
   What this ticket did instead was narrow execution: free-form family scope is gone, `/retention/run` accepts only a
   `previewId` with a live Server-composed preview, and the queued plan is frozen. No catalog entry, target or DELETE
   statement was added.
2. **Stale-preview race — accepted residual, documented.** Validation and queueing remain two statements, so a policy
   save can commit in between. The frozen plan bounds the consequence: the window can neither widen the scope nor
   lengthen a cutoff, so nothing the Owner never reviewed is released, and every later run must compose a fresh preview.
   Recorded in `docs/design/platpulse.md` §11.4 and `docs/design/webui.md` §15.13, with folding the validation into the
   queue transaction noted as follow-up work.
3. **Same-second version resurrection — accepted residual, documented.** `policy_version` digests exactly the fields the
   plan reads (`{family}|{retention_days}|{updated_at}`), and `load_preview_for_run` additionally compares
   `retention_days` itself, so every edit that would change what a run releases is refused. An edit that returns a family
   to exactly the value that was previewed leaves nothing new to review, which is why no revision counter was added to
   `retention_policies`. Documented in the same two places.
4. **Estimate as execution quota — fixed.** `retention.rs:1282` now selects the first entry with `!entry.done`, and
   `apply_batch` completes an entry only when a batch comes back shorter than `RETENTION_BATCH` (a short batch proves the
   frozen cutoff has no expired row left), so the estimate is reporting-only and can be exceeded. This is the ticket's own
   named main risk (“把估计计数当冻结计划”). Covered by the new
   `a_run_releases_what_the_frozen_cutoff_covers_not_what_the_estimate_said` (three rows released against a stored
   estimate of one, result reports `deletedRows: 3` / `estimatedRows: 1`) and the rewritten
   `a_short_batch_completes_its_entry_and_a_full_one_does_not`.
5. **Missing 409 in OpenAPI — fixed.** The policy `PUT` now declares `409` alongside 200/400/404/503
   (`operations_admin.rs:1267`), `docs/openapi/openapi.json` was regenerated from the Server binary, and the strict
   generated client was regenerated from it, so `retention_policy_version_conflict` is part of the typed contract.

## ROUTER-LEVEL AUTH COVERAGE (review follow-up)

- The unit tests in `operations_admin.rs` call the handlers directly with `Extension(session())`, so they never exercise
  the admin role guard or the Origin/CSRF guard. Added
  `crates/platpulse-server/src/http/mod.rs` test `http::tests::retention_mutation_routes_require_owner_and_csrf`,
  which drives both POST `/api/admin/v1/retention/preview` and POST `/api/admin/v1/retention/run` through
  `build_app`: anonymous → 401 `auth_required`; Owner with the session cookie and Origin but no `x-csrf-token` → 403
  `csrf_validation_failed`; Owner with a valid token sent from a different Origin (`http://127.0.0.1:9999`) → 403
  `csrf_validation_failed`; a well-formed Owner preview → 200 with a non-empty `previewId`; a well-formed Owner run for
  an unknown id → 404 `retention_preview_not_found` (sanitized, no leak).
- The Viewer/Guest matrix in `http::tests::viewer_session_reaches_public_home_but_never_admin` now also lists
  `/api/admin/v1/retention/preview`, `/api/admin/v1/retention/run` and
  `/api/admin/v1/retention/policies/raw_block_summary`, so a Viewer session is proven forbidden on the whole retention
  surface (the role guard runs before routing, so unknown admin paths answer the same 403).

## GATES (final tree, before the commit)

- Backend: `cargo fmt --all -- --check` FMT_OK; `cargo clippy --all-targets --all-features -- -D warnings` clean;
  `cargo test --workspace` exit 0 with 25 `test result: ok` groups, 910 tests passed and no failure (/tmp/ws210c.log);
  `cargo deny check && cargo audit --ignore RUSTSEC-2023-0071 --ignore RUSTSEC-2026-0253` exit 0 with the same three
  allowed warnings (/tmp/deny210b.log).
- Frontend: `npm run lint && npm run typecheck && npm test && npm run build` exit 0 (37 files / 573 tests; vite build);
  `npm run generate:api` is idempotent (all four generated files hash identically before and after) and the strict client
  already carries the policy-PUT 409 (`types.gen.ts:4904`, `UpdateRetentionPolicyErrors` = 400/404/409/503).
- Playwright: the four specs this ticket can affect × four projects (phone-360-touch, phone-390-touch, tablet-768-touch,
  desktop-1280) = 76 passed / 16 skipped / 0 failed, run twice with identical counts; `retention-acceptance.spec.ts` on
  `desktop-1440` = 1 passed / 3 skipped, which completes the five-viewport coverage (its tests 1-3 drive the real HTTP
  path on desktop-1280 by their own skip rule, test 4 asserts geometry/theme/focus/44×44/local scroll per viewport).

## PRE-EXISTING E2E DEBT (specs that pin the Admin nav)

- Three specs hard-code the Admin navigation. `git HEAD` 3d25fb7 already added Backups (15 nav entries) and this ticket
  adds Retention (16), so all three were already stale at HEAD; they were repaired mechanically, never restructured:
  - `platpulse-web/e2e/admin-overview.spec.ts:200-217` `mvpNav` pinned the drawer Tab order (14 entries). The drawer
    focus test failed at :218 with `expect(locator).toBeFocused() failed ... Expected: focused / Received: inactive` on
    phone-360-touch, phone-390-touch and tablet-768-touch, because focus landed on the extra nav link. Now 16 entries
    (`Retention` :207, `Backups` :208, before `Settings` :209), matching the render order in
    `AdminLayout.tsx:265-295`.
  - `platpulse-web/e2e/convergence-acceptance.spec.ts:29-41` `MVP_ADMIN_SECTIONS` (14 entries, count asserted at :272 and
    :315) now has 16 with `Retention`/`Backups`, and both labels were removed from `REMOVED_ADMIN_LABELS` (:92-104).
  - `platpulse-web/e2e/emerald-refinement.spec.ts:164` `toHaveCount(14)` → `toHaveCount(16)`.
- Evidence that these were the only failures: the interrupted full-suite run reported `3 failed / 676 passed`, all three
  the same admin-overview drawer-focus assertion, and the specs that looked like separate failures
  (home-geo-map, linked-validator, node-detail-six-charts) are Playwright capture/work directories, not failures —
  re-running exactly those three gives 85 passed / 7 skipped / 0 failed, and none of them imports a module this ticket
  changed or asserts anything about the Admin nav. A read-only sweep found no other hard-coded Admin-nav literal.

## REVIEW ROUND 2 (both axes re-run on the shipped commit 59ff5dc, fixed point 3d25fb7)

The same two axes (provider `cliproxyapi`, model `gpt-6.1-sol`, reasoning effort `high`) were re-run over the
committed diff, this time including the Web slice, the new router-level auth test, and the three repaired nav specs.
Both axes independently reported the same two Web defects, and both sit exactly on sentences this ticket wrote into
`docs/design/webui.md`:

1. (P2, both axes) The policy save was bound to the live prop `policy.policyVersion`, so a background overview refetch
   — composing a preview invalidates `adminKeys.retention` (`platpulse-web/src/api/admin.ts:2492`), and so does another
   Owner's save — handed the editor a newer version while the Operator's typed confirmation still stood. The save then
   submitted that newer `expectedPolicyVersion` together with the older draft: no 409, and a value the Operator never
   read was overwritten. It contradicted `docs/design/webui.md:1187` ("the save submits the `policyVersion` the
   operator read, so a stale tab cannot overwrite a value it never reviewed"). FIXED: `PolicyEditor` now holds
   `reviewed = {version, days}` captured when the draft began and submits `reviewed.version`; the baseline is
   re-adopted only while the editor holds no draft of its own (`hasDraft`); a `data-slot="retention-policy-moved"`
   notice names the recorded value and fingerprint; a refused save refetches the overview (`onStale`) and re-baselines
   so the recovery is a fresh read and a fresh confirmation, never a retry; and the "Policy version" row displays the
   version this save will submit.
2. (P2, Standards axis) The run was armed by preview validity alone (`AdminRetention.tsx:802-811`, `:876-883`): a
   single click on a bound preview queued the release command, while `docs/design/webui.md:1194` requires an explicit
   Owner command behind typed confirmation, and `PATTERN-CONFIRMATION` (`docs/design/webui.md:320`) is the established
   pattern. FIXED: the run panel requires the exact token `run <opening characters of the preview id>`, the full id is
   rendered at `data-slot="retention-run-preview-id"`, the button stays disabled until the token matches, the token is
   ASCII the Operator can type on a phone keyboard, and binding a different preview clears the confirmation so a
   replacement preview is never executed on the previous preview's confirmation.
3. (P2, Spec axis) The page was bound to `composed ?? data?.preview`, so a preview it merely *restored* on load was
   not pinned: the next overview refetch substituted the Server's newest preview while the run stayed available, and
   the run then sent an id whose scope and cutoffs the Operator never reviewed (Story 38, "旧版本/范围/截止条件变更须重预览").
   The page's own comment claimed "A Server refetch never rebinds the page to a different preview: only composing one
   does", which held only after this page composed one. FIXED: `pinned` holds the first preview the overview reports
   (`boundPreview = composed ?? pinned ?? serverPreview`) and only an explicit compose on this page replaces it;
   `docs/design/webui.md:1191` now states the pin.
- Tests added with the fixes: `refuses to queue a run until the confirmation names the bound preview` (a confirmation
  for another preview arms nothing and queues nothing; the exact token queues exactly one run with the same body) and
  `keeps the version it read when the page refetches under a confirmed draft` (composing a preview under a standing
  confirmation leaves the submitted `expectedPolicyVersion` at the reviewed version, the notice appears, the Server
  refuses, and the draft is dropped for what is now recorded). `platpulse-web/e2e/retention-acceptance.spec.ts` now
  drives the same confirmation through the real WebUI, and its stale-preview test re-confirms the replacement preview
  after the refusal instead of inheriting the old confirmation.
- Nothing else was reported: no new backend, migration, OpenAPI, or Standards finding, no repetition of the three
  dispositioned items, and the three nav-spec repairs were checked as honest (same assertions, updated inventory).
- CI evidence for the pre-commit failure those repairs address: run 37082514394 on 3d25fb7 failed only in
  `WebUI e2e (fixed viewports)` (Rust workspace, WebUI, release package and release-candidate harness were green), and
  the failing tests were exactly `e2e/admin-overview.spec.ts:179`, `e2e/convergence-acceptance.spec.ts:256` and
  `e2e/emerald-refinement.spec.ts:143` — the three specs that pin the Admin nav and were repaired mechanically here.
- Tests added for the pin: `stays bound to the preview it restored when the Server reports a newer one` (after
  `adminKeys.retention` is invalidated with a different preview in the overview, the run still sends the pinned id).
- Playwright verification on the fixed tree: the four ticket specs across the five projects gave
  `92 passed / 22 skipped / 1 failed` on the first run, and the single failure was the test's own locator, not the
  page: `e2e/retention-acceptance.spec.ts:278` looked up `editor.getByRole('status')` and Playwright's strict mode
  refused `expected 1 element, found 2` (`locator('[data-slot="retention-edit"]').getByRole('status') resolved to 2
  elements`) once the editor carried both the save notice and the new `data-slot="retention-policy-moved"` status.
  LESSON: a new `role="status"` inside a component makes every bare `getByRole('status')` scoped to that component
  ambiguous — the save notice now carries `data-slot="retention-save-notice"` and the spec addresses it by slot.
  That single test then passed on `desktop-1280`, and the full four-spec sweep was re-run after the fix.



