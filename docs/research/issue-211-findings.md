# Issue #211 — research findings (approved-plan cleanup, one command per confirmation, honest cancellation)

Ticket: https://github.com/mowind/PlatPulse/issues/211 — parent #202 (Stage 2, ticket 9/23, User Stories
38, 39, 40; shared constraints 1–8 and 80–81). Fixed point HEAD `cf9889f`. #210 (the merged blocker) already
bound a run to a live preview and froze its plan; this ticket closes the three gaps left by that shape: a repeated
Owner command could queue a **second** cleanup of the same preview, a cancellation recorded nothing about the work it
stopped, and a restart reported one generic failure no matter what the run had already released.

## SERVER (the real gaps)

- **A repeated command had no identity.** The only id on a run was the middleware HTTP correlation id
  (`crate::http::RequestId`), so a doubled click, a second tab, or a Retry after a lost response queued a second
  cleanup of the same preview. Nothing stopped it either: `retention_run` inserted unconditionally, and the partial
  unique index that keeps one open cleanup per kind did not exist.
- **A queued cancellation left no account.** `cancel_operation` flipped a queued Operation to terminal with
  `result_json` null, so the Operator saw "cancelled" with no statement of what it did or did not release.
- **A restart lost the partial release.** `requeue_interrupted_operations` wrote one generic
  "review state and re-run if needed" error for every interrupted kind and left `result_json` empty, so a cleanup
  that had already deleted rows described itself as if it had never started.
- **The release and the accounting of it were two commits.** A crash in between deleted rows that the record still
  claimed were untouched — the exact contradiction a cancellation summary must never show.

What was added:

- Migration `crates/platpulse-server/migrations/0064_operation_requests.sql`:
  `operation_requests(request_id TEXT PRIMARY KEY CHECK (length BETWEEN 1 AND 128), kind, intent_fingerprint
  CHECK (length BETWEEN 1 AND 200), operation_id REFERENCES operations(operation_id) ON DELETE CASCADE,
  audit_event_id INTEGER, created_at, expires_at)`, `UNIQUE INDEX operation_requests_intent_idx (kind,
  intent_fingerprint)`, `INDEX operation_requests_expires_idx (expires_at)`, and the partial
  `UNIQUE INDEX operations_open_retention_run_idx ON operations (kind) WHERE kind = 'retention_run' AND status IN
  ('queued','running')`. `crates/platpulse-server/src/database.rs:24` `SERVER_SCHEMA_VERSION = 64`;
  `operation_requests` joined `REQUIRED_TABLES` (`crates/platpulse-server/src/database.rs:87`).
- `crates/platpulse-server/src/operations.rs`: `:52` `OPERATION_REQUEST_TTL_HOURS = 24`, `:56`
  `OPERATION_REQUEST_ID_MAX_LEN = 128`, `:63` `OPERATION_EXCLUSIVE_KINDS = &[KIND_RETENTION_RUN]`, `:75`
  `request_intent_fingerprint(kind, target)` = `"{kind}:{target}"`, `:82` `enum QueueOutcome { Queued,
  Replayed, AlreadyOpen, RequestConflict }`, `:163` `create_operation_once`, `:355`/`:371`
  `load_operation_request(_by_intent)` (both `AND expires_at > ?`), `:388` `insert_operation_request`, `:415`
  `prune_expired_operation_requests`, `:429` `open_operation_of_kind`, `:445` `reconcile_recorded_command`,
  `:460` `recorded_command`, `:484` `replay_recorded_command` (read-only, no transaction), `:505`
  `recorded_outcome` (recorded command, then the same-kind exclusivity check), `:742`
  `requeue_interrupted_operations`.
- `crates/platpulse-server/src/retention.rs`: `:1576` `plan_entries` (one decoder for the frozen plan, shared by
  `execute_step`, `queued_run_cancellation` and `preview_binding_mismatch`), `:1592` `interrupted_run_message`,
  `:1604` `queued_run_cancellation` (queued-phase outcome with per-family detail), `:394`
  `slim_receipt_body_batch(conn: &mut sqlx::SqliteConnection, ...)` now runs on the run's own transaction, and
  `execute_step` (`:1222`) commits the bounded batch **and** its plan accounting in one transaction.
- `crates/platpulse-server/src/http/operations_admin.rs`: `:546` `RetentionRunRequest.request_id` (required),
  `:560` `RetentionRunResponse { operation, audit_event_id, request_id, deduplicated }`, `:1568` `retention_run`
  (validates the request id, then answers a recorded identity **before** validating the live preview, then queues),
  `:1776` `retention_run_response`, `:906` `cancel_operation` (records the queued cancellation outcome in the same
  UPDATE, `result_json = COALESCE(result_json, ?)`).
- Reused `doctor::last_run` (`crates/platpulse-server/src/doctor.rs:91`) as the invariant that a run which never
  started must not claim a result: a queued cancellation writes a payload **only** for a cleanup (which had a plan to
  account for), so a cancelled diagnostic keeps `result_json` empty instead of surfacing as "the last diagnostic".

## WEB

- `platpulse-web/src/api/admin.ts:2534` `newRetentionRunRequestId()` = `crypto.randomUUID()`;
  `:2544` `runRetentionEntry(previewId, requestId, csrfToken)` posts `{ previewId, requestId }`.
- `platpulse-web/src/pages/AdminRetention.tsx:845` mints `commandId` for the preview it bound, `:855` rotates it in
  the `[boundPreviewId]` effect, `:915` rotates it after `operation_request_id_conflict`, and `:875` sends it with
  the run; the page has a dedup notice, a `retention_run_in_progress` branch and a conflict branch.
- `platpulse-web/src/pages/operationsShared.tsx:219`–`:294`: `CancellationPhase`, `CancelledFamilyTotal`,
  `CancellationOutcome`, `:236` `asRowCount`, `:240` `parseCancellationOutcome` (a family the Server recorded no
  counts for is **omitted**, never zeroed), `:282` `CancellationSummary` with the `operation-cancellation`,
  `-phase`, `-released`, `-remaining`, `-families` and `-note` slots; `AdminOperations.tsx` renders it in the Result
  section and keeps the raw recorded payload below it.
- Tests: `AdminRetention.test.tsx` 18 tests, `AdminOperations.test.tsx` 14 tests (including the omitted-family and
  no-invented-zero regression); full web suite 37 files / 584 tests.

## DECISIONS

- The Owner command identity is a **client-supplied `requestId` in the JSON body**, mirroring #206's
  `notification_requests` ledger, because `operations.request_id` is the middleware's HTTP correlation id and
  cannot also identify a command.
- The intent fingerprint is `"retention_run:" + previewId`: one confirmation is one reviewed plan, so reusing the id
  for a different preview must conflict rather than act.
- The partial unique index is scoped to `retention_run` because only a cleanup is destructive and re-runnable from a
  single confirmation; other kinds are still queued unconditionally by `create_operation`.
- `OPERATION_REQUEST_TTL_HOURS = 24` matches `retention::PREVIEW_TTL_HOURS` (`retention.rs:42`): a command id can
  never outlive the preview it was confirmed for.
- Expired ledger rows are pruned **before** the new row is inserted, because a stale row with the same intent would
  otherwise make the UNIQUE intent index reject the new command.
- A recorded identity is answered before the live preview is validated; the other order told an Operator "nothing was
  queued" about work that had already happened.
- Cancellation is recorded by whoever makes it terminal: the HTTP handler for a queued Operation, the worker at its
  next safe checkpoint for a running one. `remainingTargets` counts unfinished plan entries because a remaining row
  count cannot be derived from an upper-bound estimate.
- Release and accounting commit together, so "columns say deleted, record says untouched" cannot exist.
- A restart reports what an interrupted cleanup already released (per-row, from its own recorded plan) instead of one
  generic message.
- A run that never started records no result at all: inventing one would be read back as a produced outcome.

## DOCS

- `docs/design/webui.md`: new §15.14 "Retention execution bound to a reviewed plan, one command per confirmation, and
  honest cancellation (issue #211, delivered)" plus the §14 change-log row.
- `docs/design/platpulse.md`: §11.4 gained the execution/dedup/cancellation paragraph (command identity, the
  `(kind, intent_fingerprint)` index, one open cleanup per kind, queued vs safe-checkpoint cancellation, no rollback,
  restart accounting, one transaction, no invented results).
- `docs/openapi/openapi.json` regenerated: `RetentionRunRequest` gains required `requestId`, `RetentionRunResponse`
  is new, and the run endpoint's 200 becomes that schema. `platpulse-web/src/api/generated/` was regenerated from it,
  never hand-edited. `README.md` untouched (the #209/#210 precedent).

## WORKFLOW

- Labels on close: remove `ready-for-agent`, add `implemented` (the convention #207–#210 follow).
- Frontier after #211: #212–#217 history, #218/#219 validators, #220/#221 investigation, #222+.

## REVIEW (two axes, provider `cliproxyapi`, model `gpt-6.1-sol`, reasoning effort high, fixed point `cf9889f`)

### Standards

- HARD: `operationsShared.tsx` fabricated `0` for a family the Server recorded no count for, contradicting
  `docs/design/webui.md` §15.11 (a Server-owned fact is never invented). Fixed by omitting the family from the
  summary while the raw payload stays visible.
- `clippy::explicit_auto_deref` on five `&mut *tx` arguments in `create_operation_once`, and one in
  `execute_step`'s `slim_receipt_body_batch` call: a helper that takes a concrete `&mut sqlx::SqliteConnection`
  wants `&mut tx`, while a helper generic over `sqlx::Executor<'_>` (e.g. `insert_audit_event`) needs the explicit
  reborrow. Getting this backwards is E0277, not a warning.

### Spec

- P1: `retention.rs` committed the destructive batch separately from the plan write. Fixed with one transaction
  (regression: `a_release_and_the_accounting_of_it_commit_together` installs a trigger that aborts the accounting write
  and proves the release rolled back with it).
- P2: a retried already-accepted command after a policy edit answered `retention_preview_stale` ("nothing was queued")
  because the live preview was validated first. Fixed by `replay_recorded_command` before preview validation
  (regression: `a_repeated_confirmation_reconciles_even_after_its_policy_moved`).
- P2: restart recovery left `result_json` null with one generic message. Fixed with per-row accounting from the
  recorded plan (regression: `a_restart_reports_what_an_interrupted_cleanup_already_released`).
- Reported gap, not silently dropped: there is **no browser-driven cancellation scenario** in the Playwright suite.
  The worker ticks every second (`crates/platpulse-server/src/operations.rs:28` `OPERATION_INTERVAL`), a seeded run
  finishes inside the first status poll, and no configuration holds a run queued, so such a scenario would mostly race
  the Server. The contract is covered deterministically at the unit, direct-handler and WebUI layers, and
  `platpulse-web/e2e/retention-acceptance.spec.ts` states that in its header comment.

### Disposition

- Fixed: the standards HARD finding, both P2 findings and the P1 finding, each with a new regression test.
- Refused, with the reason recorded: a Playwright cancellation scenario (see the gap above).
- Left as-is: `create_operation` stays unconditional for non-retention kinds, and the queue-time validation window
  #210 documented stays open (the frozen plan still bounds what a run can release).

## GATES (final tree, before the commit)

`cargo fmt --check` clean; `cargo clippy --all-targets --all-features -- -D warnings` clean;
`cargo test --workspace` 922 passed / 0 failed across 25 targets; `cargo deny check` advisories/bans/licenses/
sources ok; `cargo audit --ignore RUSTSEC-2023-0071 --ignore RUSTSEC-2026-0253` ok (3 allowed warnings, all
pre-existing); `cargo run -p platpulse-server -- --print-openapi` diffed against `docs/openapi/openapi.json` with
**zero** lines of difference; `platpulse-web`: `npm run lint`, `npm run typecheck`, `npm test` (37 files / 584
tests) and `npm run build` all green (only the pre-existing >500 kB chunk notice). Playwright, the primary
acceptance path (production WebUI -> real Server HTTP -> temporary SQLite, five viewport projects): **686 passed /
184 skipped / 0 failed** (23.1m, 870 tests).

## VERIFICATION

- The acceptance run is what caught the last defect: the new deduplication scenario asserted the recorded task's
  **shortened** id in the run notice ("first 8 chars + … + last 4"), but the page names the recorded task in full —
  the same id the link and the Operations API use — and only the compact "Last recorded retention run" field
  shortens it (matched by the unit test, which asserts the full id). The assertion had been written and never
  executed before the acceptance run. Corrected in `platpulse-web/e2e/retention-acceptance.spec.ts:423`-:427 (the
  UI is unchanged); the lesson is that a new end-to-end assertion is an unverified claim until it has actually run.
- Recorded as a known gap rather than papered over: the cancellation summary has no viewport/theme matrix of its
  own, because the cancellation cannot be driven deterministically from the browser (see REVIEW above).
