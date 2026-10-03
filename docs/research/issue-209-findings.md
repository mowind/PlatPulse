# Issue #209 — research findings (backup artifact reading and verification closure)

Ticket: https://github.com/mowind/PlatPulse/issues/209 — milestone 0.2.0, parent #202 (Stage 2, ticket 7/23,
User Story 36). Scope: read an existing Backup artifact, request verification, and observe the verification
Operation and its recorded result on its own detail surface. No online create, restore, or delete, and no
scheduling.

## SERVER (mostly present; one DTO field was the real gap)
crates/platpulse-server/src/http/operations_admin.rs:
- `BackupArtifactSummary` (line 495) carried every recorded manifest fact except the link to the Operation that
  verified it. Added `pub verify_operation_id: Option<String>` (doc comment: last Operation that verified this
  artifact; absent means never recorded, not a fabricated id) → serde camelCase `verifyOperationId`.
- `backups_list` (line 1381) SELECT grew 12 → 13 columns (`verify_operation_id`) with its row closure updated;
  `backup_artifact_detail` (line 1446) grew 13 → 14 columns (after `verification_error`).
- `backup_verify` (lines 1519-1556): `mutation_guard(headers, &principal, state.auth(), &request_id, false)`
  → 404 `backup_artifact_not_found` / "unknown backup artifact" when the artifact row does not exist, otherwise
  `queue_operation(..., KIND_BACKUP_VERIFY, json!({"artifactId": artifact_id}), "backup_verify_started")`.
  `queue_operation` (line 1848) publishes the "operations" channel and returns
  `OperationMutationResponse{operation: OperationDetail, audit_event_id}`.
- Router (lines 1917-1938) registers GET /backups, GET /backups/{artifact_id},
  POST /backups/{artifact_id}/verify alongside /operations, /operations/{operation_id},
  /operations/{operation_id}/cancel, /history-window, /retention*, /restore/validate, /restore, /doctor.

crates/platpulse-server/src/backup.rs:
- `verify(state, operation_id)` (line 620) reads artifactId from the Operation params; errors
  `backup_verify_missing_artifact`, `backup_dir_not_configured`, `backup_artifact_not_found`.
- Line 632: `is_cancel_requested` is checked BEFORE any result is written → the Operation is finalized
  `cancelled` and `backup_artifacts` is left untouched, so a cancel preserves the last-good verification state.
- Success result `{"artifactId","verification":"ok","integrity":"ok","checkedAt"}`; failure error code
  `backup_verification_failed` with result `{artifactId,verification:"failed",checkedAt}`.
- `verify_artifact` (line 692) is an independent read-only scan whose order is: open/validate_file →
  sha256 mismatch → `PRAGMA quick_check(1)` → schema version → `validate_snapshot_privacy`. Recorded messages
  are the ones the WebUI later quotes: "cannot open sensitive file", "artifact checksum mismatch",
  "snapshot integrity check failed: {integrity}",
  "snapshot schema version {schema} does not match the recorded version {expected_schema}",
  "snapshot contains unredacted sensitive text", "snapshot contains an unredacted receipt";

crates/platpulse-server/src/operations.rs: `KIND_BACKUP_CREATE = "backup_create"` (line 34),
`KIND_BACKUP_VERIFY = "backup_verify"` (line 35); `finalize` (line 284) writes
status/result_json/finished_at, inserts the `operation_finished` Audit row and publishes; backup_create is
retired at lines 366-396 with `add_error("backup_create_retired", "In-process backup creation was retired
(ADR 0008); run platpulse-server backup in an offline backup window")`.

crates/platpulse-server/migrations/0022_operations.sql:71-91 — `backup_artifacts` already carried every column
the ticket needs: artifact_id PK, filename (1..120), bytes ≥ 0, sha256 (length 64), schema_version,
server_version, created_at, data_range_min/max, `verification NOT NULL DEFAULT 'pending' CHECK IN
('pending','ok','failed')`, verified_at, `verification_error` NULL or 1..300, create_operation_id,
`verify_operation_id`. No migration was needed — the field only had to be exposed. Latest migration on disk is
0062_notification_requests.sql, so a new one would have been 0063; none was added.

ADR 0008 (docs/adr/0008-offline-server-backup.md) is authoritative: creation AND restore are offline-only, the
serve process never creates an artifact, `[backup_schedule]` was removed and its presence in server.toml fails
startup, redaction happens at write time, verification is an independent read-only scan, and Doctor reports the
last successful artifact age plus `.part` leftovers.

Test coverage added:
- crates/platpulse-server/src/http/operations_admin.rs:2322 unit test
  `offline_created_backup_exposes_sanitized_metadata_and_verifies` extended: `verifyOperationId` null before
  verification; still `pending` right after POST (acceptance ≠ result); after `process_operations` a string
  `verifiedAt` and list/detail both pointing at the queued Operation id; a tampered file → failed + `checksum`
  and a NEW operation id; a deleted file → failed + `cannot open` with the artifact row still present.
- crates/platpulse-server/tests/backup_verification_flow.rs (new, 4 tests, green):
  `offline_artifact_is_readable_and_acceptance_is_not_the_result`,
  `corrupt_and_unreadable_artifacts_are_distinct_recorded_failures`,
  `cancelled_verification_leaves_the_last_recorded_outcome_intact`,
  `the_backup_surface_is_owner_only_behind_real_origin_and_csrf`. Asserts sanitized list/detail (no directory
  path, Server pepper, server.db or password), 404 `backup_artifact_not_found` for an unknown id on GET and POST,
  exactly one `operation_finished` Audit row per accepted verification, cancel leaving the artifact `ok` with
  unchanged `verifiedAt`/`verifyOperationId`, and anonymous 401 / viewer 403 / missing CSRF or Origin 403.

## WEB (the real deliverable: the API client existed with no consumer)
- platpulse-web/src/api/admin.ts already had `adminKeys.backups` (line 269), `adminKeys.backupDetail(id)`
  (line 270), `fetchAdminBackups`/`useAdminBackups(generation)` (2521-2533),
  `fetchAdminBackup`/`useAdminBackup` (2535-2551), `verifyBackupEntry(artifactId, csrfToken)` (2554-2573),
  and `validateRestoreEntry`/`useRestoreValidation` (2581+) — and no page consumed them.
  docs/design/webui.md listed Backup/Restore as unrouted (lines 13, 38, 195, 978).
- platpulse-web/src/pages/backupShared.tsx (new, 113 lines): `NOT_RECORDED = 'Not recorded'`;
  `formatArtifactBytes` (null/non-finite/negative → 'Unknown', <1024 → "N bytes", else KiB/MiB/GiB/TiB at one
  decimal); `formatDataRange` (missing bound → 'Not recorded'); `readVerification(verification,
  verificationError)`; `readFailure(err)` mapping the Server's recorded text to a plain reading by lowercase
  substring (checksum / 'cannot open' / integrity / schema / unsafe) and returning null when nothing known
  matches so the raw Server text is shown alone; `VERIFICATION_SCOPE_NOTE` (contains "It is not a restore
  rehearsal and it never proves that a production restore will succeed."); `BACKUP_SURFACE_SCOPE_NOTE`
  (contains "it never creates, restores, or deletes a backup, and it offers no scheduling."); `operationHref`.
- platpulse-web/src/pages/AdminBackups.tsx (new, 412 lines): `AdminBackupsList` (h1 "Backups";
  `table[data-slot="backups-table"]` with columns Artifact/Created/Size/Schema/Data window/Server version/Creating
  task/Verification (the creating task links to its Operation);
  filename links to /admin/backups/:artifactId; `span[data-slot="backup-verification-state"]` showing
  Verified / Verification failed / Not verified plus "Last checked <age> ago" or "No verification outcome
  recorded"; empty state "No backup artifact is recorded yet. Artifacts appear here after an offline backup run
  on the Server host.") and `AdminBackupDetailPage` (404 code `backup_artifact_not_found`/`not_found` → h1
  "Backup artifact not found" + "Back to Backups"; loaded h1 is the recorded filename; CardX "Recorded artifact"
  DetailList of id/file name/size/SHA-256/schema version/Server version/created/data window/created-by task;
  `VerificationCard` with `div[data-slot="backup-verification"]`,
  `span[data-slot="backup-verification-reason"]` carrying the raw Server text (`Not recorded` when the Server
  recorded a failure without a reason), the "Request verification" button (min-h-11, disabled while pending or
  without a CSRF token), and a task block `div[data-slot="backup-verify-task"]` with the short-id link, an
  `OperationStatus`/`OperationProgress` reading, `p[role=status][data-slot="backup-verify-task-state"]`, the
  task's recorded errors/warnings (`IssueList`) and its result payload (`ResultBlock`); feedback says
  "Acceptance is not a result:". The block reads the Server's own Operation detail for `acceptedOperationId ??
  artifact.verifyOperationId` through `useAdminOperation(generation, id, true)`, so the in-flight reading ends
  on the task's recorded terminal status - never on a timer - and a cancelled or failed task reports its own
  state instead of leaving the page claiming work the Server no longer has.)
- platpulse-web/src/App.tsx routes `backups` → `AdminBackupsList` and `backups/:artifactId` →
  `AdminBackupDetailPage`; platpulse-web/src/layouts/AdminLayout.tsx adds the `Archive`-icon link
  "Backups" → /admin/backups; platpulse-web/src/components/StatusBadge.tsx gained Verified: Check,
  'Verification failed': X, 'Not verified': Clock.
- platpulse-web/src/api/generated/types.gen.ts was regenerated: `verifyOperationId?: string | null` (line 891).
- Tests: platpulse-web/src/pages/AdminBackups.test.tsx (new, 12 cases, green: manifest columns, empty and
  retry states, the recorded reason plus its plain reading, an unreadable task ledger named as unread, acceptance
  not a result with the queued task tracked, the terminal task clearing the in-flight reading, a cancelled task,
  the task's own errors/result, the indeterminate transport outcome) and the App route inventory in
  platpulse-web/src/App.test.tsx grew the /admin/backups and /admin/backups/:artifactId rows, the nav link count
  (14 → 15), the Backups active-link mapping, the `Archive` glyph entry, and dropped 'Backups' from the list of
  page groups that must stay unlinked. Acceptance: platpulse-web/e2e/backup-verification-acceptance.spec.ts
  (new, three tests: the readable/verify flow, the corrupted-vs-missing distinction, and the
  viewport/theme/keyboard/touch matrix) drives the production build against a throwaway Server and creates its
  artifact with the harness's `createOfflineBackup()`, which stops the serve process, runs
  `platpulse-server backup --config ...`, and restarts it. Its sign-in, authenticated-route, keyboard-focus,
  local-scroll, and resolved-theme helpers, plus the five-viewport table, live in
  platpulse-web/e2e/admin-flow.ts and are shared with platpulse-web/e2e/operations-doctor-acceptance.spec.ts
  (issue #208) instead of being copied into each spec.

## DOCS
- docs/design/webui.md §4.3 is the authoritative Page ID table: added `PAGE-ADMIN-BACKUPS` (/admin/backups) and
  `PAGE-ADMIN-BACKUP-DETAIL` (/admin/backups/:artifactId), rewrote the paragraph after the table, removed
  Backup from the unrouted enumerations (lines 13 and 38), and added §15.12 for issue #209.
- The historical note at docs/design/webui.md:978 stays as history: it records the deferral, not a current route.
- docs/research/issue-209-findings.md is this file (the #208 precedent is docs/research/issue-208-findings.md).

## REVIEW
- /code-review ran at the fixed point HEAD b0c5160 (#208) over the uncommitted work, on two parallel axes using
  provider cliproxyapi, model gpt-6.1-sol, reasoning_effort high (issue-tracker.md reads the issue; the Standards axis
  also carries the Fowler smell baseline). Findings and the fixes that followed:
- Standards (3 hard violations + 1 judgement): (1) the page claimed an in-flight task forever, because the queued note
  cleared only when `artifact.verifyOperationId === queuedOperationId` while a cancelled task keeps the old link
  (crates/platpulse-server/src/backup.rs:632 finalizes `cancelled` without touching the artifact); (2) the list
  omitted the Server version and creating-task columns webui.md §15.12 requires; (3) a recorded failure with no reason
  text rendered nothing instead of `Not recorded`; (4) judgement - the e2e sign-in/route/focus/scroll/theme helpers were
  copied from platpulse-web/e2e/operations-doctor-acceptance.spec.ts.
- Spec (3 findings, P2): the artifact detail neither read nor rendered the Operation's own status/progress/issues/result
  (so a task that failed before writing an artifact outcome was invisible); the same permanent in-flight claim after a
  cancel; coverage gaps (Site Access Mode change, principal switch, Public projection, live reconciliation).
- Fixes: platpulse-web/src/pages/AdminBackups.tsx now tracks `taskOperationId = acceptedOperationId ??
  artifact.verifyOperationId` through `useAdminOperation(generation, id, true)` and renders the task block, so the
  in-flight reading ends on the Server's recorded terminal status rather than on a timer or an effect; the reason block
  is gated on `artifact.verification === 'failed' || Boolean(verificationError)` with `NOT_RECORDED` as its fallback;
  the list gained the Server version and Creating task columns; `readTaskLifecycle` in
  platpulse-web/src/pages/backupShared.tsx is the single place the per-state copy lives; `IssueList` and `ResultBlock`
  moved into platpulse-web/src/pages/operationsShared.tsx so the Operations pages and the backup page render the same
  definitions; the duplicated e2e helpers moved into platpulse-web/e2e/admin-flow.ts and both Admin acceptance specs
  import them.
- Coverage closed by tests: platpulse-web/src/pages/AdminBackups.test.tsx adds the cancelled-task, unreadable-task, and
  task-result cases (12 cases total). Site Access Mode, principal switch, and Public-projection coverage for the backup
  surface stays open and is recorded here rather than claimed as done.
- Second (verification) pass, same reviewer route, over the fixes above: BLOCKING with three P1 findings, all in the copy
  that decides what the page claims about the two records. (1) every `failed` task read "failed before it recorded an
  artifact outcome", but a checksum failure writes `backup_artifacts.verification = failed` and the same
  `verify_operation_id` first and only then finalizes the Operation as failed
  (crates/platpulse-server/tests/backup_verification_flow.rs:332-345), so the ordinary failure case lied. (2) an Operation
  reported `succeeded` was rendered as "recorded the outcome shown above" although the independently fetched artifact row
  could still be showing an older outcome. (3) `acceptedOperationId`/feedback/pending were not reset per artifact or per
  auth generation, so a previous task's card state could appear beside another artifact or in a new session. Two P2s: three
  e2e assertions reloaded the page before checking the terminal state (so a broken live reconciliation still passed), and
  one unit assertion matched `/Verification task:/` while the real label has no colon.
- Fixes for the second pass: `readTaskLifecycle` now takes `{status, loaded, unreadable, linkedToArtifact, wroteOutcome}`
  and only calls the outcome its own when `artifact.verifyOperationId === taskOperationId`; when the two records have not
  been linked yet it says so and calls the artifact state the last outcome the Server recorded for it. A terminal task
  triggers `refetchAdminBackup(artifactId)` (new export in platpulse-web/src/api/admin.ts; `adminKeys` stays module
  private) so the card re-reads the artifact instead of assuming correspondence. `VerificationCard` is keyed by
  `generation + ':' + artifact.artifactId`, guards every post-await write behind a `mounted` ref, and the e2e specs assert
  the live page (no reload) reaches the terminal state before they reload. Unit tests cover the unlinked-then-linked
  transition (platpulse-web/src/pages/AdminBackups.test.tsx, 13 cases; 560 Web tests in total).
- Third (verification) pass over those fixes, same reviewer route: still BLOCKING, with one defect the second pass had
  introduced and two the copy still carried. (1) the new `mounted` ref only cleared itself on cleanup and never restored
  itself on setup, so under `StrictMode` (platpulse-web/src/main.tsx, development) the second effect run left it `false`
  forever: every response was dropped, the button stayed on "Requesting verification…", and the accepted task was never
  read. (2) the unlinked branches asserted "the Server has not linked its outcome to the artifact record above yet",
  which is not knowable: a newer verification replaces `verify_operation_id`, so a task that was linked can stop being
  the linked one. (3) the linked `failed` branch equated a failed task with a failed verification outcome, and the two
  really do come apart: crates/platpulse-server/src/backup.rs:667-676 commits `verification = ok` and the link before
  finalizing, so a Server restart in that window leaves the artifact `Verified` while
  crates/platpulse-server/src/operations.rs:331-345 marks the running task `failed`. The reviewer also confirmed the new
  terminal refetch introduces no request loop or stale-cache read, and that isolating SSE from the polling fallback is
  not required by this ticket.
- Fixes for the third pass: the `mounted` effect restores the flag on setup; the unlinked branches now say the artifact
  record does not point at this task, so this task's result is not attributed to the state above, and promise no future
  link; `readTaskLifecycle` takes `artifactOutcome` and, when a linked task ended as `failed`, separates the two facts
  unless the artifact's own recorded outcome is `failed`. The queued/running note and the acceptance feedback both say
  that any verification task's outcome updates the state above, not only the tracked one. Tests:
  platpulse-web/src/pages/AdminBackups.test.tsx renders the card under `StrictMode` and keeps the accepted task, and adds
  the `ok`-artifact-with-failed-task case (15 cases; the e2e corrupt-file assertions read the new failure copy).

## WORKFLOW
- Labels: `ready-for-agent` is removed and `implemented` added on close (closed issues such as #207 carry only
  `implemented`).
- Frontier after #209: #210 retention policy/preview, #211 retention execution, #212-#217 history, #218/#219
  validators, #220/#221 investigation, #222-#225 Home. Parent #202 stays open.
- Commit style (de2cf80): conventional commit `feat(server,web): <summary> (#209)` plus structured
  Server/Web/Docs/Gates bullets.
- Reviewer route for /code-review: provider cliproxyapi, model gpt-6.1-sol, reasoning_effort high.
