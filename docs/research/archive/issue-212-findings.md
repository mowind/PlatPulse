# Issue #212 — research findings (capacity and low-space protection closed loop)

Ticket: https://github.com/mowind/PlatPulse/issues/212 — parent #202 (Stage 2, ticket 10/23; User Stories 43, 44,
45 and 46, shared constraints included). Fixed point HEAD `be829fb`. #212 asks for four things at once: the
Operations surface must show the database's capacity and whether the configured low-space policy is protecting it;
optional history must pause under storage pressure and resume afterwards **with a visible gap**; a Report whose core
persistence did not commit must never be accepted; and a 2-Agent/6-Node capacity and performance baseline report
must be delivered with its sampling, mount and hardware conditions declared. The parent spec (#202 §6) additionally
forbids silent deletion inside the retention floor and any precision downgrade, and forbids inventing a threshold.

## SERVER (the real gaps)

- **Nothing measured the filesystem the database lives on.** The Server had no capacity policy, no configuration
  key for one, and no status surface: the only storage signal was a write that eventually failed. An Operator could
  not see the free space, could not see whether protection was engaged, and could not set a floor without inventing
  a number the deployment had never measured.
- **A pause would have been invisible.** If the generic optional metric writer had simply stopped inserting, history
  would have lost samples with nothing left behind to explain them — the exact silent gap the parent spec forbids.
  The pause therefore needed a durable record: the interval that was open, and the per-series count of what it cost.
- **The skip and its record had to be atomic.** Recording a skip outside the Report's own transaction would allow
  "sample lost, nothing recorded" or "gap recorded, sample written". Both are lies about what the Server kept.
- **The gate had to be readable without the pool.** `SERVER_WRITE_CONNECTIONS = 1`: an ingestion transaction
  that began by reading the capacity state from the pool would wait on the only write connection it was already
  holding, so the gate is answered from memory and only the *transition* touches the database.

## What was added

- `crates/platpulse-server/migrations/0065_capacity_protection.sql`:
  `capacity_protection_intervals` (interval id, source mount, `started_at`,
  `started_reason` CHECK IN ('low_space'), the opened total/available bytes, the pause and resume floors,
  `ended_at`, `ended_reason` CHECK IN ('resumed','protection_disabled'), the resumed
  total/available bytes, timestamps) with `CREATE UNIQUE INDEX capacity_protection_open_interval_idx ... WHERE
  ended_at IS NULL` so at most one interval can ever be open, and `capacity_skipped_series`
  (`interval_id` REFERENCES ... ON DELETE CASCADE`, `scope_kind` CHECK IN
  `('node','host')`, `scope_key`, `metric`, `skipped_count > 0`,
  `first_skipped_at`, `last_skipped_at`, `PRIMARY KEY (interval_id, scope_kind, scope_key,
  metric)`) — the row is the gap's identity, so a second skip updates it instead of adding a duplicate.
  `crates/platpulse-server/src/database.rs:24` `SERVER_SCHEMA_VERSION: i64 = 65`; both tables
  joined `REQUIRED_TABLES` (`crates/platpulse-server/src/database.rs:34`).
- `crates/platpulse-server/src/capacity.rs` (new, 1188 lines, 9 unit tests):
  `CapacityConfig` at `:58` with `:80 pub const MAX_PERSISTED_BYTES: u64 =
  i64::MAX as u64` (the thresholds are stored in SQLite, so the ceiling is the signed maximum, not
  `u64::MAX`); `:45 DEFAULT_SAMPLE_INTERVAL_SECONDS = 60`, `:47`/`:49`
  the accepted 5..=86400 bounds, `:52 ADMIN_SKIPPED_SERIES_LIMIT = 20`, `:54
  ADMIN_RECENT_INTERVAL_LIMIT = 10`;
  `:87 from_declared(enabled, pause_below_bytes, resume_above_bytes, sample_interval_seconds, origin) ->
  Result<Self, String>` (`:149`–`:174` accessors); `:192`/`:211
  sample_filesystem(path)` through `nix::sys::statvfs` with `total_bytes = f_frsize *
  f_blocks` and `available_bytes = f_bavail * f_frsize`; `:224 protection_required(currently
  protected, available, pause, resume)` hysteresis; `:239 enum HistoryGate { Record, Paused { interval_id
  } }` and `:249 enum SkippedScope { Node, Host }`; `:268 CapacityStatus`,
  `:289 CapacityIntervalRecord`, `:314 CapacitySkippedSeries` (camelCase over the wire);
  `:331 struct ProtectionState` (which carries `adoption_pending`) and
  `:361 CapacityProtection` with `:370 new`, `:392 disabled(db)`,
  `:407 status()`, `:425 history_gate()`, `:441 reconcile(pool)`,
  `:458 adopt_open_interval(pool)`, `:484 check_now(pool)` and `:619 run_worker(
  pool, shutdown)`; `:712 open_interval` / `:731 close_interval` next to `:689 load_open_interval`;
  `:770 record_skipped_series(tx, interval_id, scope, scope_key, metric,
  observed_at)` writes the series row **on the Report's own transaction**, counting one skip per reading newer
  than the recorded high-water mark and keeping MIN/MAX timestamps over those readings;
  `:793 recent_intervals(pool, limit)` reads them back for the Admin surface.
- `crates/platpulse-server/src/config.rs`: `:93 pub capacity: Option<CapacitySectionFile>`,
  `:104 CapacitySectionFile` (`deny_unknown_fields`, `enabled`,
  `pause_below_bytes`, `resume_above_bytes`, `sample_interval_seconds`),
  `:232 pub capacity: CapacityConfig`, `:369 ConfigError::InvalidCapacity { path, reason }`,
  `:543`/`:573 fn resolve_capacity` (no section means "no policy declared").
- `crates/platpulse-server/src/http/mod.rs` `:407 AppState.capacity: Arc<CapacityProtection>`
  (default `:519 disabled(Some(db.path()))`, so a Server without a declared policy still samples the
  directory it writes to), `:564 with_capacity`, `:573 capacity()`.
- `crates/platpulse-server/src/http/report_ingestion.rs`: `:804 save_node_metric` and
  `:859 save_host_metric` now open with `if let crate::capacity::HistoryGate::Paused { interval_id } =
  history` (`:813`, `:868`) and return `record_skipped_series(...)` (`:828`, `:882`) as
  `Result<(), sqlx::Error>`; before that they read the stored value for the same identity (`:817`, `:871`) and
  return early when it is unchanged (`:825`, `:879`), so a replayed reading is a replay here too and cannot
  inflate the gap; `ingest_report` takes `let history = state.capacity().history_gate()` once (`:2496`) so
  every sample in one Report sees one gate. The paused branch **never** runs the per-series prune, because
  deleting history under pressure is the silent deletion the parent spec forbids.
- `crates/platpulse-server/src/http/operations_admin.rs`: `:2302 CapacitySkippedSeriesDto`,
  `:2324 CapacityIntervalDto`, `:2352 CapacityOverview` (its count texts state the high-water
  contract), `:2392 capacity_overview` (Owner-only), `:2449
  capacity_interval_dto`, route registered at `:2579 route("/capacity", get(capacity_overview))`.
  The handler samples the filesystem before answering (`check_now`), so the Operator sees the current
  measurement rather than the last tick.
- `crates/platpulse-server/src/doctor.rs`: `:194 storage_capacity_check` (check 12):
  no policy declared is `NOT_CONFIGURED`, an engaged protection is `FAIL` with the floors and
  the measured free space, a transition that could not be written is `WARNING`, a measurement that failed is
  `WARNING` carrying the reason (`:232`–`:237`, so a retained last-good sample can no longer pass as healthy),
  a filesystem that was never measured is `WARNING` (`:261`), and a healthy sample with the policy declared
  is `PASS`.
- `crates/platpulse-server/src/metrics.rs`: gauges `:416`/`:422
  platpulse_capacity_total_bytes`, `:429`/`:435 platpulse_capacity_available_bytes`
  (omitted, never zero, when the measurement is unknown) and `:442`/`:448
  platpulse_capacity_paused`.
- `crates/platpulse-server/src/cli.rs`: `:968 CapacityProtection::new`, `:976` awaits `reconcile`
  **before** the app is built, so an interval an earlier process left open is adopted before the first request
  is served; a failure there is deferred rather than fatal (`:978`, logged redacted) and the worker retries the
  adoption on its next tick; `:982 with_capacity`; the sampling worker is spawned only when the policy is
  enabled (`:1145`).
- `crates/platpulse-server/src/openapi.rs`: the path and four schemas registered;
  `docs/openapi/openapi.json` and `platpulse-web/src/api/generated/` regenerated.
- `crates/platpulse-server/tests/capacity_protection.rs` (new, 7 tests): a visible gap under pressure with
  the released interval keeping its counts, and a restart that resumes it; a Report whose receipt write is
  refused by an injected trigger is rejected (503) and its gap rolls back; a replayed reading is not counted as
  a second lost sample; a tick adopts an open interval the startup read missed; skip counting advances only on a
  reading newer than the mark; the route is Owner-only (Viewer 403, anonymous 401); and the Doctor warns when
  the state filesystem cannot be measured.

## WEB

- `platpulse-web/src/api/admin.ts`: `:2810 CAPACITY_POLL_MS = 30000`, used unconditionally as
  `refetchInterval` at `:2822` (the earlier "only while protected" variant was dropped: protection
  *starting* has to become visible without a reload), `:2827 CapacityState`,
  `:2844 capacityPresentation` — one classifier over the whole overview, so an enabled policy with no
  reading, a failed measurement or a failed transition renders **Unknown**, never Monitoring —
  and `:2867 capacityIntervalReasonLabel` (Low space / Resumed / Protection disabled / Unknown).
- `platpulse-web/src/pages/AdminOperations.tsx`: `capacityStateNote` (`:75`, the Unknown /
  Protecting / measuring copy, asserted at `:138`), the interval table at `:189` with the slots
  `capacity-intervals-table` and `capacity-skipped-series`, and `CapacityPanel` rendered at `:348`.
  A row states its volume and both boundaries as `<available> of <total>`, and no value is fabricated: a state
  that is not known renders as Unknown instead of 0 or false.
- `platpulse-web/src/pages/AdminOperations.test.tsx`: 19 tests, including the paused-panel, the
  boundary/mount and the three Unknown-state regressions; the full web suite is 37 files / 589 tests.

## DECISIONS

- **The receipt protocol did not change.** `ReportReceipt.samples` describes block-history samples
  (`SampleDisposition` over `SampleRef`), while the low-space gate pauses the *generic optional
  metric writer* (`host_metric_samples`, `node_metric_samples`). Those samples were never part of
  a receipt, so pretending otherwise would have changed the wire contract to describe a different writer. The
  visible gap therefore lives in `capacity_skipped_series` and on the Admin surface, and the receipt keeps
  asserting only what it already asserted.
- **A Report with no core persistence still cannot be accepted** (Story 45), and the new record cannot be a way
  around it: `record_skipped_series` runs inside the receipt transaction, so a refused receipt write rolls
  the gap back with it and the Agent sees a retryable `503` instead of a silent skip.
- **No threshold default is invented.** `[capacity] enabled = true` requires both
  `pause_below_bytes` and `resume_above_bytes` and fails with "enabled capacity protection
  requires pause_below_bytes; no default is invented because the threshold must be measured for this deployment",
  with `resume_above_bytes` >= `pause_below_bytes` > 0 and `sample_interval_seconds`
  in 5..=86400 (default 60). A deployment declares a floor it has measured; nothing in this ticket guesses one.
- **The database is written before the in-memory gate moves.** A crash between the two leaves a persisted interval
  the next startup can adopt; the reverse order would leave the Server claiming a protection no record explains.
- **A failed measurement does not change the state.** `statvfs` failing keeps the gate where it was and
  records `sampling_error`, so a transient error can neither pause history nor release an open interval,
  and the failure is visible as a failure instead of as a state.
- **Hysteresis, not a single line.** Protection engages below the pause floor and releases only above the resume
  floor, so a deployment sitting on the boundary does not flap between writing and skipping on every tick.
- **Startup adoption is explicit.** `reconcile` adopts an open interval, keeps protecting, and closes it
  with `resumed` or `protection_disabled` according to the *current* configuration (the
  `resumed_*` columns stay NULL when protection was switched off rather than recovered) — an Operator who
  deletes the section gets a closed interval stating that, not a gap with no explanation.
- **The skip counter is a gap record, not an error.** `HistoryGate::Paused` returns `Ok`: the
  outage is *recorded*, and the receipt keeps its meaning. Confusing an optional skip with a core transaction
  failure is the primary risk the ticket names.

## STORY 46 BASELINE (`scripts/capacity-baseline.py`)

The baseline is a script rather than a hand run because the conditions have to travel with the numbers. It declares
the hardware, mount, binary and cadence it measured, self-checks eleven invariants, and refuses to read the database
while the Server is running: an external SQLite connection next to live Server writes unlinks the WAL sidecars and
the measurement would corrupt what it describes ("refusing to read ... while the Server is running"). It also mints
its enrollment tokens in a stopped-Server window for the same reason. It *does* measure the live `db` and `wal`
sizes while the Server runs, with `stat` rather than SQLite: closing the last connection checkpoints the WAL away,
so a reading taken after the stop reports a WAL of zero bytes for a load that wrote megabytes. The first run of this
script did exactly that and the review caught it, which is why the numbers below come from the re-run.

Result: `target/capacity-baseline/20261003T080335Z/` (`baseline.json`, `baseline.md`, `server.toml`,
`state/`), generated 2026-10-03T08:04:37Z, seed 212, schema version 65, **11/11 self-checks ok** — measured on the
binary that contains the review fixes (`target/debug/platpulse-server`, built 2026-10-03T07:57:29Z). The earlier run
of the same script is superseded: its WAL figure was read after the shutdown and its other figures came from an older
binary.

### Declared conditions

- Hardware: AMD EPYC 7313P 16-Core Processor, 32 cores, 125.65 GiB RAM, Linux 7.2.7-arch1-1
- Mount: `/` (ext4, `/dev/nvme1n1p2`), 1.32 TiB free of 1.79 TiB
  (`1454295384064` of `1967743246336` bytes at start, fragment size 4096)
- Binary: `target/debug/platpulse-server`, 294622296 bytes, built 2026-10-03T07:57:29Z, **debug
  profile** — the throughput figures below are therefore a lower bound and are not extrapolated
- Cadence: 2 Agents x 3 Nodes every 5.0s, slow scan every 4 rounds carrying the data-directory and peer snapshots,
  capacity sampling every 5s
- Topology: 2 Agents (`434db2a8-ad1c-4620-82e1-008b7ed0624c`,
  `19afc034-80d2-4d58-af3e-66e0bccefb78`) with 3 Nodes each, network `platon-mainnet`
  (chain id 210425)

### Steady write path

| Measurement | Value |
| --- | --- |
| Rounds / submissions | 12 / 24 |
| Wall duration | 56.34s |
| Optional samples written | 246 (expected 246) |
| Host series / Node series | 4 / 30 |
| Report latency p50 / p95 | 59.02ms / 79.03ms |
| Database growth | 204.00 KiB (1032192 -> 1241088 bytes, read live) |
| Database / WAL after load (Server still running) | 1.18 MiB / 4.04 MiB |
| WAL peak during load | 4.04 MiB (176303 bytes per submission) |
| Database / WAL after shutdown | 1.28 MiB / 0.00 B — the shutdown checkpoints the WAL away, so this is *not* the load's WAL |
| Core receipts | 24 |

The optional writer is what the ticket allows to be measured: `network_rx_bytes_per_sec` and
`network_tx_bytes_per_sec` per Agent (2 scopes, 12 rows each) and, per Node,
`data_directory_percent`, `peer_inbound_count`, `peer_outbound_count` (18 samples each
across 6 scopes) plus `process_cpu_percent` and `process_memory_percent` (72 samples each).

### Read path (20 samples each, every one 200)

| Endpoint | p50 / p95 |
| --- | --- |
| `/api/admin/v1/capacity` | 2.32ms / 2.44ms |
| `/metrics` | 1.25ms / 1.37ms |
| `/api/admin/v1/operations?limit=50` | 1.44ms / 2.21ms |

### Thresholds used, pressure, gap and recovery

| Phase | Pause / resume floor | Observed |
| --- | --- | --- |
| steady | 1 GiB / 2 GiB | protection stays off, 246/246 optional samples |
| pressure | `i64::MAX` / `i64::MAX` (9223372036854775807) | protected = True, 48 samples
  skipped across 16 series, receipts 24 -> 30 |
| recovery | half the measured available space (`727146629120`) | protected = False, resumed, optional
  samples 246 -> 278 |

- The pressure interval (`ec448de3-3d58-487d-b117-e0eb5660a751`) opened `2026-10-03T08:04:34Z` with
  `started_reason = low_space` and `opened_total_bytes = 1967743246336` /
  `opened_available_bytes = 1454295384064`; the gap covers `08:04:34Z`–`08:04:36Z` and is recorded as
  48 skipped samples over 16 series (both Agent host network series, and the Node
  process/data-directory/peer series).
- During pressure the gauges read `platpulse_capacity_total_bytes` `1967743246336`,
  `platpulse_capacity_available_bytes` `1454292086784`, `platpulse_capacity_paused`
  `1`; the receipt count rose 24 -> 30, all six accepted, while the optional sample count did not move
  (246 -> 246) — optional history paused, core reporting did not.
- Recovery closed the interval exactly once with `ended_reason = resumed`,
  `resumed_total_bytes = 1967743246336` and `resumed_available_bytes = 1454293258240`, keeping the
  48-sample record: the gap survives the recovery instead of being tidied away.
- The database ends with 1 protection interval, 16 skipped-series rows for 48 skipped samples, 102 block summaries,
  2 block-history gaps, 56 host and 222 node metric samples, 1433600 bytes.

### Two defects the baseline exposed in the measurement itself

- **The first run recorded every receipt as `partially_accepted`.** The cause was the loader, not the
  gate: it replayed the canonical fixture's fixed block height while declaring a much higher
  `current_block`, so each block sample was classified
  `RejectionCode::ResyncReplay` — "Normal resync replay at or below the historical high-water mark"
  (`crates/platpulse-core/src/receipt.rs:331`-`:332`, code at `:381`) — and any
  non-accepted sample forces `partially_accepted` (`crates/platpulse-server/src/http/report_ingestion.rs:2726`-`:2736`).
  Fixed by advancing the fixture's heights and hash chain per round
  (`baseline_block_hash(node_id, height)` = sha256 of `node_id:height`) so every round reports a
  new head. **Lesson: a `partially_accepted` receipt is not evidence about a paused gate** — a measurement
  that mixes the two would have blamed capacity protection for a replay.
- **The run reported dispositions and receipt ids as `null`.** `submit_round` read camelCase
  top-level fields from a body shaped `{"receipt": {"report_id": ..., "disposition": ...}}`, so the
  JSON said null while the database said accepted. Both defects are the same class: the measurement described itself
  wrongly. Neither was visible without cross-checking the database, which is why the baseline stores both the
  receipts it recorded and the rows it read back.

### Not delivered (verbatim from the report; a follow-up ticket appends to the same report)

- No history family beyond the generic optional metric writer was built or measured.
- No 30 day retention or precision-tier downsampling was exercised; the visible gap evidence covers minutes, and
  upscaled old buckets were never produced.
- Agent-side collection was not measured: Reports were generated by the loader, not by a live `platpulse-agent` process, and validator hosts were not contacted.
- Only one mount, one filesystem and one writer path were measured; no multi-disk or network filesystem deployment
  was exercised.
- The Server ran in development mode without TLS and without a reverse proxy.
- A release-profile build was not measured.
- These items must be appended to this same report by a follow-up ticket; nothing here is a production guarantee.

## DOCS

- `docs/design/platpulse.md`: §8.3 step 7 (`:401`) and its note (`:407`) describe the
  gate inside ingestion; new "### 11.5 低空间保护（issue #212）" at `:609` covers the policy, the
  interval table, the skip record and the fail-open sampling rule.
- `docs/design/webui.md`: new §15.15 at `:1209` (delivered), the delivered lists in §3 and §13,
  the PAGE-ADMIN-OPERATIONS row (`:191`) and `:198`, §5.5 (`:247`), and the change-log
  row after #211.
- `docs/openapi/openapi.json` regenerated with the capacity path and schemas; the web client is generated
  from it and never hand-edited.
- The review fixes moved three sentences in `docs/design/webui.md` §15.15: `:1215` (the two ways the state
  is Unknown, plus the hysteresis and the level that releases it), `:1216` (an interval row shows its volume and
  both boundary totals) and `:1218` (the poll is unconditional).

## WORKFLOW

- Labels on close: remove `ready-for-agent`, add `implemented` (the convention #207-#211
  follow).

## REVIEW

Reviewed on the fixed point `189d7e1` with provider `cliproxyapi`, model `gpt-6.1-sol`, reasoning
effort high: two independent read-only passes over `be829fb..189d7e1`, one against the repository's
standards (`AGENTS.md`, `docs/design/platpulse.md`, `docs/design/webui.md`) and one against this
ticket's specification (issue #212, Stories 43-46, the acceptance body and the linked design
sections). Neither pass ran a build or the tests, so every finding below is a source-level claim
that the listed regression now verifies. Severity is that of the reviewing pass, and the two axes are
reported separately rather than ranked against each other.

### Standards axis

- **P2, fixed — `platpulse-web/src/api/admin.ts:2827`**: the presentation pair looked at
  `enabled`/`protected` alone, so an enabled policy whose first `statvfs` had failed (no `sample`, no
  `sampledAt`) still rendered as a green **Monitoring**, against `docs/design/webui.md:1215`
  ("Unknown when no reading is available"); the error paragraph below the badge did not correct a badge
  that already claimed a healthy state. `capacityStateLabel`/`capacityStateTone` are now one classifier,
  `capacityPresentation(...)` (`platpulse-web/src/api/admin.ts:2844`), which answers Unknown for "no
  reading", for a failed latest measurement and for a failed transition, and Monitoring only for a
  measured, unprotected, error-free enabled policy. Regressions:
  `platpulse-web/src/pages/AdminOperations.test.tsx` — "keeps an enabled policy Unknown until a reading
  arrives", "reports a failed capacity measurement as Unknown, not Monitoring", "reports a failed
  protection transition as Unknown, not Monitoring".
- **P2, fixed — `platpulse-web/src/pages/AdminOperations.tsx:216-255`**: an interval row printed the
  available bytes but not `sourceMount`, `openedTotalBytes` or `resumedTotalBytes`, so after a
  relocation an operator could not attribute a historical window to its volume, nor tell how full the
  filesystem was at either boundary (`docs/design/webui.md:1216`). The row now names the mount and
  prints both boundaries as `<available> of <total>`; the existing gap test asserts
  `/var/lib/platpulse` and `1.00 GiB of 100 GiB` inside the interval table.
- **P2, fixed (raised independently by both axes)**: the protecting copy said "below the pause floor",
  which is wrong inside the hysteresis band (`pause <= available < resume`) and unproven when the
  retained sample is stale. The copy now states the band and the level that releases it
  (`platpulse-web/src/pages/AdminOperations.tsx:88-94`), and `docs/design/webui.md:1215` says the same.
- **Confirmed clean**: no invented threshold; the hysteresis and the in-memory gate belong to the
  Server; the skip counter is written inside the receipt transaction; a failed receipt cannot be
  reported as an accepted one; the paused path deletes nothing (and the closed-interval pruning was
  removed); capacity errors are redacted on the Admin surface and in the Doctor output; the last-good
  measurement survives a sampling failure; Node-scoped gap rows are reachable by Owner Purge; the 30s
  poll is unconditional, which is what makes protection *starting* visible at all.
- **Judgement calls, recorded and not changed**: the two duplicated four-line state assignments in
  `crates/platpulse-server/src/capacity.rs` (the success paths clear different error fields, so
  extracting a helper would change their semantics); the enabled/protected branching appearing in both
  `capacityPresentation` and the page copy (an API presentation helper and page prose, not one rule
  written twice); the generic-series pruning on the healthy path, which predates this ticket.

### Spec axis

- **P2, fixed — `crates/platpulse-server/src/cli.rs:976-979`**: a startup reconciliation that failed was
  only logged, and no later tick retried it. Because the partial unique index admits one open interval,
  every subsequent open attempt was refused, so protection could stay defeated until another restart,
  and an already-recorded interval could never be closed — against `docs/design/platpulse.md:617`
  ("重启时 `reconcile` 接管已打开的区间"). `CapacityProtection` now carries `adoption_pending`
  (`crates/platpulse-server/src/capacity.rs:331`), cleared only by a successful read in
  `adopt_open_interval` (`:458`), and every tick retries the adoption before it samples (`:490`).
  Regression:
  `crates/platpulse-server/tests/capacity_protection.rs::a_tick_adopts_an_open_interval_the_startup_read_missed`
  — a tick adopts the orphaned interval, opens no second one, still pauses optional history, and closes
  it `resumed` once a later tick measures free space.
- **P2, fixed — the gap upsert (then at `crates/platpulse-server/src/capacity.rs:720-737`, now in
  `record_skipped_series` at `:770`)**: deduplication compared only the greatest timestamp, so readings
  `t1, t2, t1, t1` counted four skips for two identities, contradicting the DTO contract at
  `crates/platpulse-server/src/http/operations_admin.rs:2302`. The upsert now
  advances only when the refused reading is newer than the recorded high-water mark
  (`WHERE excluded.last_skipped_at > capacity_skipped_series.last_skipped_at`), the DTO and generated
  client text states that contract, and the residual limitation is documented instead of hidden: an
  older reading arriving late is *under*-counted, because telling it apart from a replay would need one
  ledger row per skipped reading — at least as much disk as the history the pause is protecting.
  Regressions: the new `skipped_counting_advances_only_on_a_reading_newer_than_the_mark` (sequence
  `t1,t2,t1,t2,t3` counts `1,2,2,2,3`) and the existing replay test.
- **P2, fixed — `scripts/capacity-baseline.py:953-993`**: `storage_after` was sampled after the Server
  had stopped *and* after an external SQLite connection had opened and closed the database, which
  checkpoints the WAL away. The report therefore printed "WAL bytes after load | 0.00 B" for a load
  that had written one — an unmeasured value presented as a measured zero, against the acceptance
  body's DB/WAL requirement. The script now reads the live `db`/`wal` files with `stat` while the
  Server is still running (before the stop) and keeps the per-round peak, and it keeps the
  post-shutdown reading under its own label so the difference is visible in the report.
- **P2, fixed** — the two Web findings above, confirmed independently on this axis.
- **Confirmed clean**: the skip, gap and receipt share one transaction, and the failure path has its own
  test; the normal pressure/recovery flow, restart adoption, Owner-only access, Node-scoped purge,
  retained intervals, the Doctor warning for a known measurement failure, and the production
  UI→HTTP→SQLite acceptance across five viewports all hold in the source.
- **Judgement calls, recorded and not changed**: the deferred 30 day retention and the other history
  families sit inside the ticket's own scope note; calling debug-profile throughput a lower bound is a
  claim the script does not prove (the report says "not extrapolated"); the pre-existing healthy-path
  pruning is out of scope.

## GATES

Recorded on the final tree: the fixed point was `189d7e1`, and every number below belongs to the
second-round fixes described in `## REVIEW` (three commits on `main`: `fab39e0` feature,
`4bb2eb3` baseline and browser acceptance, `189d7e1` the first review round).

- **Rust**: `cargo fmt --check` clean; `cargo clippy --all-targets --all-features -- -D warnings`
  clean; `cargo test --workspace` green (941 tests passed, 0 failed), which includes the new
  `crates/platpulse-server/tests/capacity_protection.rs` (7 tests) and the extended
  `crates/platpulse-server/tests/node_purge.rs` (13 tests); `cargo deny check` clean;
  `cargo audit --ignore RUSTSEC-2023-0071 --ignore RUSTSEC-2026-0253` clean (3 allowed warnings,
  all pre-existing).
- **Web**: `npm run lint` clean, `npm run typecheck` clean, `npm test` 589 tests in 37 files
  green, `npm run build` green.
- **Generated artifacts**: `cargo run -p platpulse-server --quiet -- --print-openapi >
  docs/openapi/openapi.json` regenerates with no drift beyond the documented DTO text and the new
  `capacity_skipped_series` Purge count; `npm run generate:api` regenerates
  `platpulse-web/src/api/generated/types.gen.ts` from that spec (the generated client is never
  hand-edited).
- **Browser**: the full `npm run e2e` suite green - 688 passed, 192 skipped, 0 failed in 23.3 minutes over
  the five fixed viewport projects (41 specs). The two capacity tests own `desktop-1280` and every other project
  skips them by design (both `platpulse-web/e2e/capacity-acceptance.spec.ts:130:3` and `:275:3` passed, and the
  matrix test drives the other four viewports through `setViewportSize`). Run alone, the same spec is green too
  (`npx playwright test e2e/capacity-acceptance.spec.ts`: 2 passed, 8 skipped). Two earlier full runs are part of
  the record: one failed 67 specs because a concurrent `npm run build` replaced the bundles the shared Server had
  cached, and the next one failed this spec alone on its stale exact-copy assertion for the `Protecting` note
  (both are lessons in `## VERIFICATION`; the second one is also a fix in `## REVIEW`).

## VERIFICATION

- The primary acceptance path stays the production one: `platpulse-web/e2e/capacity-acceptance.spec.ts`
  drives the production WebUI build against a real Server over HTTP with a temporary SQLite database, on a
  throwaway state directory, in a real browser. `platpulse-web/e2e/server-harness.ts` gained one option
  (`DisposableCapacityOptions`) that writes a real `[capacity]` section, and the byte counts
  travel as strings because `i64::MAX` sits one past `Number.MAX_SAFE_INTEGER` and a JavaScript
  number would reach TOML rounded up to a floor the Server rejects.
- Two tests: the full flow (declared floor above the filesystem and above every real disk, so the pressure is a real
  measurement; pause; a Report that is still accepted; the per-series gap in the UI; the floor lowered to one byte
  and the Server restarted on the same state directory; the same interval closed `resumed` with its counts
  intact; a second, genuinely new Report accepted with the skip counter unmoved), and a viewport/theme matrix over
  all five fixed viewports in light and dark, with the local table scroll, no horizontal overflow, the 44px targets
  and keyboard focus asserted while the gap is on screen.
- The second Report is deliberately a *new* report (fresh id, sequence 2): resubmitting the identical body is an
  exact replay the Server answers from the immutable receipt without re-running the writer, and would have proved
  nothing about resumed history.
- Lesson carried from #211: a new end-to-end assertion is an unverified claim until it has actually run. This spec's
  first run failed on two strict-mode violations (the Agent's two host series share the `host:<agent id>`
  scope token, so the row is identified by its metric, not by the token alone) — the assertions were corrected, not
  the product.
- The capacity surface is read-only, so the matrix states what it can honestly prove: the hosting page keeps its
  touch, focus and overflow guarantees while the gap is displayed.
- Lesson: a copy change is a contract change. The corrected `Protecting` sentence (both declared levels instead of
  "below the pause floor") left the spec's exact-text assertion at `platpulse-web/e2e/capacity-acceptance.spec.ts:164`
  behind, and the full suite caught it while the spec alone had passed earlier. Both assertions - the browser one and
  the unit one in `AdminOperations.test.tsx` - now move with the sentence.
- Lesson: a full `npm run e2e` run only means what the shared Server is serving. `platpulse-web/e2e/start-server.sh:19`
  rebuilds `platpulse-web/dist` before the Server starts, but a *second* build landing while that Server is alive (the
  Web gate's `npm run build`, run at 07:58 UTC against a Server started at 07:40 UTC) replaces the hashed bundles
  underneath it, and `reuseExistingServer` (`platpulse-web/playwright.config.ts:49`) then silently reuses the stale
  Server. The cached `index.html` still referenced the deleted bundle, so `GET /assets/index-*.js` answered 404, the SPA
  never booted, and every login-based spec failed at `platpulse-web/e2e/helpers.ts:25` after its fixed 30 s budget -
  67 failures that had nothing to do with the product. Never rebuild `platpulse-web/dist` while a suite is in flight.
