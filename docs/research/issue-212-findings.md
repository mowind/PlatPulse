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
- `crates/platpulse-server/src/capacity.rs` (new, ~1080 lines, 9 unit tests):
  `CapacityConfig` at `:63` with `:85 pub const MAX_PERSISTED_BYTES: u64 =
  i64::MAX as u64` (the thresholds are stored in SQLite, so the ceiling is the signed maximum, not
  `u64::MAX`); `:45 DEFAULT_SAMPLE_INTERVAL_SECONDS = 60`, `:47`/`:49`
  the accepted 5..=86400 bounds, `:54 CLOSED_INTERVAL_HISTORY_LIMIT = 200`, `:57
  ADMIN_SKIPPED_SERIES_LIMIT = 20`, `:59 ADMIN_RECENT_INTERVAL_LIMIT = 10`;
  `:92 from_declared(enabled, pause_below_bytes, resume_above_bytes, sample_interval_seconds, origin) ->
  Result<Self, String>` (`:154`–`:174` accessors); `:197`/`:216
  sample_filesystem(path)` through `nix::sys::statvfs` with `total_bytes = f_frsize *
  f_blocks` and `available_bytes = f_bavail * f_frsize`; `:229 protection_required(currently
  protected, available, pause, resume)` hysteresis; `:244 enum HistoryGate { Record, Paused { interval_id
  } }` and `:254 enum SkippedScope { Node, Host }`; `:273 CapacityStatus`,
  `:294 CapacityIntervalRecord`, `:319 CapacitySkippedSeries` (camelCase over the wire);
  `:357 CapacityProtection` with `:366 new`, `:387 disabled(db)`,
  `:402 status()`, `:420 history_gate()`, `:436 reconcile(pool)`,
  `:451 check_now(pool)` and `:566 run_worker`; `:648 open_interval` /
  `:664 close_interval`; `record_skipped_series(tx, interval_id, scope, scope_key, metric,
  observed_at)` inserts or updates the series row with `skipped_count + 1` and MIN/MAX timestamps
  **on the Report's own transaction**; `recent_intervals(pool, limit)` reads them back for the Admin
  surface.
- `crates/platpulse-server/src/config.rs`: `:93 pub capacity: Option<CapacitySectionFile>`,
  `:104 CapacitySectionFile` (`deny_unknown_fields`, `enabled`,
  `pause_below_bytes`, `resume_above_bytes`, `sample_interval_seconds`),
  `:232 pub capacity: CapacityConfig`, `:369 ConfigError::InvalidCapacity { path, reason }`,
  `:543`/`:573 fn resolve_capacity` (no section means "no policy declared").
- `crates/platpulse-server/src/http/mod.rs` `:407 AppState.capacity: Arc<CapacityProtection>`
  (default `:519 disabled(Some(db.path()))`, so a Server without a declared policy still samples the
  directory it writes to), `:564 with_capacity`, `:573 capacity()`.
- `crates/platpulse-server/src/http/report_ingestion.rs`: `:804`
  `save_node_metric` and `:845 save_host_metric` now open with `if let
  HistoryGate::Paused { interval_id } = history { return record_skipped_series(...).await }` and return
  `Result<(), sqlx::Error>`; `ingest_report` takes `let history =
  state.capacity().history_gate()` once (`:2469`) so every sample in one Report sees one gate.
  The paused branch **never** runs the per-series prune, because deleting history under pressure is the silent
  deletion the parent spec forbids.
- `crates/platpulse-server/src/http/operations_admin.rs`: `:2321 CapacityIntervalDto`,
  `:2365 recent_intervals`, `:2382 capacity_overview` (Owner-only), `:2431
  capacity_interval_dto`, route registered at `:2561 route("/capacity", get(capacity_overview))`.
  The handler samples the filesystem before answering (`check_now`), so the Operator sees the current
  measurement rather than the last tick.
- `crates/platpulse-server/src/doctor.rs`: `:194 storage_capacity_check` (check 12):
  no policy declared is `NOT_CONFIGURED`, an engaged protection is `FAIL` with the floors and
  the measured free space, a transition that could not be written is `WARNING`, and a healthy sample
  with the policy declared is `PASS`.
- `crates/platpulse-server/src/metrics.rs`: gauges `:416`/`:422
  platpulse_capacity_total_bytes`, `:429`/`:435 platpulse_capacity_available_bytes`
  (omitted, never zero, when the measurement is unknown) and `:442`/`:448
  platpulse_capacity_paused`.
- `crates/platpulse-server/src/cli.rs`: `:968 CapacityProtection::new`,
  `:972` awaits `reconcile` **before** the app is built (`:1030`), so an interval an
  earlier process left open is adopted before the first request is served; `:978 with_capacity`; the
  sampling worker is spawned only when the policy is enabled (`:1141`).
- `crates/platpulse-server/src/openapi.rs`: the path and four schemas registered;
  `docs/openapi/openapi.json` and `platpulse-web/src/api/generated/` regenerated.
- `crates/platpulse-server/tests/capacity_protection.rs` (new, 3 tests): a visible gap under pressure
  with the released interval keeping its counts; a Report whose receipt write is refused by an injected trigger is
  rejected (503) and its gap rolls back; the route is Owner-only (Viewer 403, anonymous 401).

## WEB

- `platpulse-web/src/api/admin.ts`: `:2810 CAPACITY_POLL_MS = 30000`,
  `:2812 pollActiveCapacity` (30s **only** while the answered state is protected),
  `:2833 capacityStateLabel` (Unknown / Disabled / Protecting / Monitoring),
  `:2848 capacityStateTone`, `:2863 capacityIntervalReasonLabel` (Low space / Resumed /
  Protection disabled / Unknown).
- `platpulse-web/src/pages/AdminOperations.tsx`: `CapacityPanel` rendered at `:320`
  with the slots `capacity-intervals-table` and `capacity-skipped-series`, three note variants
  (no policy declared / storage below the pause floor / measuring on cadence) and no fabricated value: a state that
  is not known renders as Unknown instead of 0 or false.
- `platpulse-web/src/pages/AdminOperations.test.tsx`: 16 tests, including the paused-panel and
  unknown-state regressions; the full web suite is 37 files / 586 tests.

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
its enrollment tokens in a stopped-Server window for the same reason.

Result: `target/capacity-baseline/20261003T070926Z/` (`baseline.json`, `baseline.md`,
`server.toml`, `state/`), generated 2026-10-03T07:10:28Z, seed 212, schema version 65, **11/11
self-checks ok**.

### Declared conditions

- Hardware: AMD EPYC 7313P 16-Core Processor, 32 cores, 125.65 GiB RAM, Linux 7.2.7-arch1-1
- Mount: `/` (ext4, `/dev/nvme1n1p2`), 1.32 TiB free of 1.79 TiB
  (`1455507468288` of `1967743246336` bytes at start, fragment size 4096)
- Binary: `target/debug/platpulse-server`, 294397064 bytes, built 2026-10-03T06:47:14Z, **debug
  profile** — the throughput figures below are therefore a lower bound and are not extrapolated
- Cadence: 2 Agents x 3 Nodes every 5.0s, slow scan every 4 rounds carrying the data-directory and peer snapshots,
  capacity sampling every 5s
- Topology: 2 Agents (`4666a600-6af2-4bc4-9363-839c1cce74b7`,
  `2493c32f-a305-4302-ac2a-7a1e503648a0`) with 3 Nodes each, network `platon-mainnet`
  (chain id 210425)

### Steady write path

| Measurement | Value |
| --- | --- |
| Rounds / submissions | 12 / 24 |
| Wall duration | 56.32s |
| Optional samples written | 246 (expected 246) |
| Host series / Node series | 4 / 30 |
| Report latency p50 / p95 | 53.95ms / 85.72ms |
| Database growth | 300.00 KiB (1032192 -> 1339392 bytes) |
| WAL bytes after load | 0.00 B |
| Core receipts | 24 |

The optional writer is what the ticket allows to be measured: `network_rx_bytes_per_sec` and
`network_tx_bytes_per_sec` per Agent (2 scopes, 12 rows each) and, per Node,
`data_directory_percent`, `peer_inbound_count`, `peer_outbound_count` (18 samples each
across 6 scopes) plus `process_cpu_percent` and `process_memory_percent` (72 samples each).

### Read path (20 samples each, every one 200)

| Endpoint | p50 / p95 |
| --- | --- |
| `/api/admin/v1/capacity` | 1.82ms / 2.63ms |
| `/metrics` | 1.60ms / 1.86ms |
| `/api/admin/v1/operations?limit=50` | 1.73ms / 2.03ms |

### Thresholds used, pressure, gap and recovery

| Phase | Pause / resume floor | Observed |
| --- | --- | --- |
| steady | 1 GiB / 2 GiB | protection stays off, 246/246 optional samples |
| pressure | `i64::MAX` / `i64::MAX` (9223372036854775807) | protected = True, 48 samples
  skipped across 16 series, receipts 24 -> 30 |
| recovery | half the measured available space (`727755628544`) | protected = False, resumed, optional
  samples 246 -> 278 |

- The pressure interval opened `2026-10-03T07:10:25Z` with `started_reason = low_space` and
  `opened_total_bytes = 1967743246336` / `opened_available_bytes = 1455511400448`; the gap
  covers `07:10:25Z`–`07:10:27Z` and is recorded as 48 skipped samples over 16 series (both
  Agent host network series, and the Node process/data-directory/peer series).
- During pressure the gauges read `platpulse_capacity_total_bytes` `1967743246336`,
  `platpulse_capacity_available_bytes` `1455510073344`, `platpulse_capacity_paused`
  `1`; the receipt count rose 24 -> 30, all six accepted, while the optional sample count did not move
  (246 -> 246) — optional history paused, core reporting did not.
- Recovery closed the interval exactly once with `ended_reason = resumed` and
  `resumed_available_bytes = 1455511257088`, keeping the 48-sample record: the gap survives the
  recovery instead of being tidied away.
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
- **The run reported dispositions and receipt ids as `null%%BT%.** `submit_round` read camelCase
  top-level fields from a body shaped `{"receipt": {"report_id": ..., "disposition": ...}}`, so the
  JSON said null while the database said accepted. Both defects are the same class: the measurement described itself
  wrongly. Neither was visible without cross-checking the database, which is why the baseline stores both the
  receipts it recorded and the rows it read back.

### Not delivered (verbatim from the report; a follow-up ticket appends to the same report)

- No history family beyond the generic optional metric writer was built or measured.
- No 30 day retention or precision-tier downsampling was exercised; the visible gap evidence covers minutes, and
  upscaled old buckets were never produced.
- Agent-side collection was not measured: Reports were generated by the loader, not by a live `platpulse-agent
  `process, and validator hosts were not contacted.
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

## WORKFLOW

- Labels on close: remove `ready-for-agent`, add `implemented` (the convention #207-#211
  follow).

## REVIEW

(recorded after the review of the fixed point with provider `cliproxyapi`, model
`gpt-6.1-sol`, reasoning effort high)

## GATES

(recorded after the final gate run on the committed tree)

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
