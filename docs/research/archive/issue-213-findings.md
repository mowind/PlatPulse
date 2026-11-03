# Issue #213 findings — trustworthy 24 hour raw Node metric history

#213 (Stage 2 ticket 11/23, milestone 0.2.0, blocks #214, parent #202) asked for a
demonstrable 24 hour raw Node metric history that keeps the real evidence it
already has, never fabricates a value for a stretch nobody observed, and keeps
the observation ledger, the timing evidence and the retention floor visible and
independent. This file records what the Server was actually missing, what was
added, and the measurements Story 46/47 demand. #214 (the one minute and five
minute aggregate tiers plus the 30 day floor) is deliberately untouched: the
read path answers grain "raw" with aggregate_supported false.

## SERVER (the real gaps)

1. **The retained window was a function of the report cadence, not of policy.**
   crates/platpulse-server/src/http/report_ingestion.rs kept
   const NODE_METRIC_SAMPLES_PER_SERIES: i64 = 64 and deleted everything below the
   newest 64 rows of a series after every single write. At the Agent default of
   collection_interval_seconds = 5 (crates/platpulse-agent/src/config.rs:38) that
   is about five minutes per series: "24 hours of raw history" was really
   "however many samples happened to survive". No retention family covered
   node_metric_samples or host_metric_samples at all, so the Node table was hard
   capped while the Host table grew without a floor.

2. **Nothing recorded what a series had observed.** A row could be deleted by
   retention, by Node purge, or by the 64 sample prune, and no record survived of
   the fact that the series had ever been observed. A released row was therefore
   indistinguishable from a series that never existed: first observation,
   observation count and coverage were unrecoverable, and the read path could not
   honestly answer "unavailable" instead of "no data".

3. **A restatement was indistinguishable from a new observation.** Ingestion
   blindly overwrote the row at (node_id, metric, observed_at). A carried
   last-good delivery (the Agent re-sends the same latest_observed_at and value
   after a spool drain) and a genuine new observation both produced one row, so
   any count taken from rows would have counted a replay as an observation —
   exactly what Story 47 forbids.

4. **The timing evidence could not be shown honestly.** The row carried the
   received_at of the write that created the instant and nothing else; the delay
   belonged to whichever delivery created the row, and clock suspicion had to be
   guessed because no stored pair existed to derive it from.

5. **Silence had no representation.** There was no gap concept at all, no way to
   connect the low-space protection pauses #212 already counted in
   capacity_skipped_series to the curve, and no way to tell "nobody observed" from
   "the value was 0".

## What was added

### Schema and floor

- crates/platpulse-server/migrations/0066_node_metric_history.sql — new table
  node_metric_series_state (node_id, metric, first_observed_at, last_observed_at,
  last_received_at, observation_count, replayed_count, corrected_count,
  released_before, updated_at), primary key (node_id, metric), index
  node_metric_series_state_series_idx, backfilled from node_metric_samples with
  MIN(observed_at)/MAX(observed_at)/COUNT(*), plus the range indexes
  node_metric_samples_observed_at_idx (observed_at, node_id, metric) and
  host_metric_samples_observed_at_idx (observed_at, agent_id, metric).
  crates/platpulse-server/migrations/0066_node_metric_history.sql:92 also adds
  capacity_skipped_series_lookup_idx (scope_kind, scope_key, metric,
  last_skipped_at), because the capacity ledger's own primary key leads with
  interval_id (crates/platpulse-server/migrations/0065_capacity_protection.sql:59-71)
  and the series predicate runs on every Owner read of the history. The same
  migration (crates/platpulse-server/migrations/0066_node_metric_history.sql:95-143)
  rebuilds retention_policies so that 'raw_metric_sample' is a legal value of the
  family CHECK list and its seeded row survives: a family the CHECK does not yet
  allow is skipped silently at seed time, which would have left the new policy
  invisible to the Owner and its cleanup unrun. Rebuilding the table is the repo's
  existing pattern for adding a family (migrations 0022, 0027, 0029, 0035, 0059).
- crates/platpulse-server/src/database.rs:24 SERVER_SCHEMA_VERSION: i64 = 66;
  crates/platpulse-server/src/database.rs:90 adds "node_metric_series_state" to
  REQUIRED_TABLES.
- crates/platpulse-server/src/retention.rs:51 FAMILY_RAW_METRIC_SAMPLE: &str =
  "raw_metric_sample"; the catalog entry is the 14th family in
  crates/platpulse-server/src/retention.rs:303 POLICY_CATALOG
  ([PolicyDefaults; 14]) at
  crates/platpulse-server/src/retention.rs:325 with its two targets at
  crates/platpulse-server/src/retention.rs:196 TARGET_RAW_METRIC_SAMPLES —
  "node_metric_samples" (crates/platpulse-server/src/retention.rs:201, ordered
  delete with LIMIT 2048) and "host_metric_samples"
  (crates/platpulse-server/src/retention.rs:207, LIMIT 128) — class Raw, default
  1 day, minimum 1 day (the Raw class floor, so the 24 hour window cannot be
  configured away) and maximum 30 days. Cleanup is
  crates/platpulse-server/src/retention.rs:643 cleanup_expired_metric_samples
  (iterating the family targets over
  crates/platpulse-server/src/retention.rs:541 family_cutoff), called after
  ingestion (crates/platpulse-server/src/http/report_ingestion.rs:3075) and at
  startup (crates/platpulse-server/src/cli.rs:938).
- crates/platpulse-server/src/retention.rs:175 NODE_METRIC_CLEANUP_BATCH: i64 =
  2048 with a compile-time guard at
  crates/platpulse-server/src/retention.rs:180 asserting the bound covers one
  maximal Report (256 Nodes x 5 series = 1280 rows,
  crates/platpulse-core/src/protocol.rs:33 MAX_NODE_OBSERVATIONS).
- crates/platpulse-server/migrations/0066_node_metric_history.sql:51 adds
  released_before TEXT NOT NULL DEFAULT '1970-01-01T00:00:00Z' with a
  20-character CHECK
  (crates/platpulse-server/migrations/0066_node_metric_history.sql:52): the oldest
  cutoff the cleanup has ever pruned this series at. It is advanced by
  crates/platpulse-server/src/retention.rs:235 RAW_METRIC_RELEASED_BEFORE_SQL from
  crates/platpulse-server/src/retention.rs:643 cleanup_expired_metric_samples
  (crates/platpulse-server/src/retention.rs:653), before the bounded deletes and
  only for a series that really holds a row below the cutoff, so its EXISTS probe
  runs against the sample table's primary key as a range seek. A row created by the
  backfill keeps the epoch default
  (crates/platpulse-server/migrations/0066_node_metric_history.sql:63): no cleanup
  under this policy regime has released evidence for that series yet.

### Series ledger and honest counting

- crates/platpulse-server/src/metric_history.rs — new module (registered at
  crates/platpulse-server/src/lib.rs:34):
  - crates/platpulse-server/src/metric_history.rs:47 NODE_METRIC_SERIES and
    crates/platpulse-server/src/metric_history.rs:56 is_node_metric.
  - crates/platpulse-server/src/metric_history.rs:563 stored_value reads the value
    already stored at (node, metric, observed_at) inside the ingestion
    transaction, and
    crates/platpulse-server/src/metric_history.rs:450 classify_delivery answers
    the novelty question in a fixed order, returning the
    crates/platpulse-server/src/metric_history.rs:403 Delivery enum: Observed
    past the series high-water mark (canonical instants compare as text, so the
    check is a string compare), Replay when the stored row holds the same value,
    Correction when it holds a different one, and Observed again when no row
    exists but the instant is still inside the raw window — an out-of-order or
    silence-filling reading, which keeps the retained rows equal to the ledger. An
    instant with no row that is already behind the cutoff
    (crates/platpulse-server/src/metric_history.rs:524
    outside_retained_window, stored None and observed_at < cutoff) is treated as a
    replay and never counted: an expired replay is indistinguishable from an
    observation too old to store, and counting it is the one thing that would let
    a carried last-good inflate the lifetime count. The trade-off is stated in
    the code: such a delivery is never counted at all, so the count can only
    under-report, never inflate.
  - The current cutoff alone cannot answer that question after a Retention
    widening, because it moves backwards when the Owner widens the window. The
    novelty floor is therefore the later of the policy cutoff and the series' own
    released_before stamp
    (crates/platpulse-server/src/metric_history.rs:479-482, inside
    crates/platpulse-server/src/metric_history.rs:450 classify_delivery): the stamp
    is the durable answer to "this series held evidence below this instant and a
    cleanup released it", which the cutoff cannot give. Widening lowers the cutoff
    and can never lower the stamp, so an observation that was counted once and then
    released stays a Replay when its carried copy comes back. The write decision
    deliberately keeps using the policy cutoff alone
    (crates/platpulse-server/src/metric_history.rs:524 outside_retained_window):
    the widened window really does re-retain the value, and only the lifetime count
    refuses to move twice for one observation. Regression:
    crates/platpulse-server/src/http/report_ingestion.rs:3852
    a_retention_widening_cannot_recount_an_observation_it_released stores an
    observation, runs the real cleanup with a clock two days ahead, asserts the
    series was stamped above the released instant, widens the policy to two days,
    reopens the database the way a restart does (the floor is durable evidence, not
    process memory) and re-delivers the same instant: one observation, one replay,
    zero corrections, with the row stored again by the widened window.
  - crates/platpulse-server/src/metric_history.rs:534 record_delivery moves
    observation_count (+1 for Observed only), replayed_count and corrected_count
    for the classified delivery; last_observed_at and last_received_at move
    together in one CASE, so the reported delay always belongs to a single real
    sample; the ON CONFLICT branch sets only value = excluded.value, so the row
    keeps the receipt time of the delivery that created it.
- crates/platpulse-server/src/http/report_ingestion.rs:821 save_node_metric and
  crates/platpulse-server/src/http/report_ingestion.rs:914 save_host_metric now
  read the stored value, classify the delivery against the window cutoff
  (crates/platpulse-server/src/http/report_ingestion.rs:801 metric_window_cutoff
  = format_rfc3339(family_cutoff(now, metric_sample_retention_days_tx))), write
  the row unless the instant is outside the retained window, and record the
  delivery; the Paused branch records the skip in capacity_skipped_series
  (SkippedScope::Node) when the value actually changed, instead of silently
  dropping it. The per-series prune and its constant are gone.

### Read path

- crates/platpulse-server/src/metric_history.rs:618 load_window (ORDER BY
  observed_at DESC with LIMIT limit + 1, so a truncated answer carries the newest
  samples) and its helpers:
  crates/platpulse-server/src/metric_history.rs:192 sample_timing,
  crates/platpulse-server/src/metric_history.rs:212 gap_threshold_seconds
  (three observed cadences, floor 120 s, ceiling 900 s because the cadence is
  first clamped to crates/platpulse-server/src/metric_history.rs:90
  MAX_OBSERVED_CADENCE_SECONDS = 300 — crates/platpulse-agent/src/config.rs:164
  forbids an Agent from declaring a slower interval),
  crates/platpulse-server/src/metric_history.rs:388 observed_cadence_seconds (the
  minimum positive delta of the returned samples, so the Server never assumes a
  cadence it was not told),
  crates/platpulse-server/src/metric_history.rs:231 continuity — window-clipped,
  so a gap is only ever reported inside the requested window and a paused tail is
  clipped to window_end — with
  crates/platpulse-server/src/metric_history.rs:353 pause_overlapping,
  crates/platpulse-server/src/metric_history.rs:369 pause_count_within
  (skipped_count is attached only to a stretch that covers the whole pause
  interval, because capacity_skipped_series keeps one count per interval with no
  per-skip timestamps) and
  crates/platpulse-server/src/metric_history.rs:380 pause_intersects (a
  known pause shorter than the gap threshold is never counted as proven coverage,
  and since the review's fix set it is reported as a ProtectionPause band rather
  than dropped (crates/platpulse-server/src/metric_history.rs:274). Pauses come from
  crates/platpulse-server/src/metric_history.rs:699 load_pauses using
  crates/platpulse-server/src/metric_history.rs:693 PAUSE_LOOKUP_SQL, which the
  migration's index serves. Gaps are derived at read time and never stored,
  except that protection pauses are read from capacity_skipped_series: the two
  crates/platpulse-server/src/metric_history.rs:132 GapKind values are
  "collection_gap" and "protection_pause"
  (crates/platpulse-server/src/metric_history.rs:142 as_str).
- crates/platpulse-server/src/http/admin.rs:4462 admin_node_metric_history —
  Owner-only GET /api/admin/v1/nodes/{node_id}/metric-history with
  crates/platpulse-server/src/http/admin.rs:4327 AdminMetricHistoryQuery (metric,
  from, to, limit). Defaults and bounds live in
  crates/platpulse-server/src/metric_history.rs:61 DEFAULT_WINDOW_HOURS 24,
  :59 DEFAULT_SAMPLE_LIMIT 5000, :63 MAX_SAMPLE_LIMIT 20000, :67
  CLOCK_SKEW_TOLERANCE_SECONDS 300. from and to must be canonical
  (crates/platpulse-server/src/metric_history.rs:500 canonical_instant), so a
  fractional or offset bound is invalid_history_range instead of silently
  excluding the stored text instants, and a query the extractor cannot parse at
  all answers 400 invalid_query. It answers 400 invalid_metric or
  invalid_history_range, 404 not_found, 503 unavailable, and inherits 401
  auth_required / 403 owner_required; the answer is wrapped in
  crates/platpulse-server/src/http/admin.rs:52 no_store, so an Owner-only history
  is never cached. Availability is null when the request is inside the retained
  raw window, "partial" when it starts before the cutoff (requested_from is
  preserved), and "unavailable" when it ends before it. A truncated answer
  carries the newest samples with truncated = true, so the oldest end is never
  read as a gap. The per-sample delay and clock flags come from the newest row of
  the answer itself, never from the ledger pair, whose last_received_at can
  belong to a later restatement of the same instant.
- crates/platpulse-server/src/openapi.rs:218 and the path entry publish the
  response schemas; Node purge counts the new owned rows
  (crates/platpulse-server/src/node_purge.rs:65 metric_series_state, :98 and :128
  in the total, :217 the count, :337 the table list, exercised by
  crates/platpulse-server/tests/node_purge.rs NODE_OWNED_TABLES).
- crates/platpulse-server/tests/node_metric_history.rs proves the read path over
  real HTTP against a real temporary SQLite database, with every observation
  stamped near now, because the environment clock is 2026-10-03 and the new 24
  hour raw floor releases anything older:
  `owner_reads_the_stored_series_and_other_principals_are_refused` (Story 46 —
  three stored samples a minute apart, the timing evidence on every row, the
  window, the grain and the retention floor in the answer, a series the Node never
  reported answered as never observed rather than zero, an invalid metric, an
  inverted range and a malformed range refused with their own codes, an unknown
  Node answered 404 not_found, an anonymous caller 401 auth_required and a Viewer
  403 owner_required);
  `an_unexplained_silence_is_a_collection_gap_not_proved_coverage` (Stories 47
  and 49 — three samples an hour apart, which no Agent can have produced, because
  crates/platpulse-agent/src/config.rs:164 forbids a declared interval slower than
  300 s: the answer carries two 3600 s collection_gap entries with a null
  skippedCount, every item keeps the value the Node really sent, and
  coverageSeconds is 0, because a stretch nobody observed proves nothing);
  `a_low_space_pause_is_a_protection_gap_not_a_zero` (Stories 47 and 49 — a real
  CapacityProtection floor above any real free space, two Reports still accepted
  while their samples were not stored, the floor released and collection resumed:
  five stored observations, one protection_pause gap with its reason, its 1020 s
  span and its 2 counted losses, and coverageSeconds 900, the three stretches the
  stored observations prove rather than the paused one);
  `history_older_than_the_retention_floor_is_reported_not_faked` (Stories 57 and
  59 — an observation 40 hours old is released by the policy instead of extending
  the window, the ledger row outlives it, and the read answers availability
  "unavailable" with no item and no gap).

## WEB

- platpulse-web/src/metricHistory.ts — the chart mathematics, kept out of the
  component and unit tested: platpulse-web/src/metricHistory.ts:35
  NODE_METRIC_SERIES, :81 nodeMetricDefinition, :87 METRIC_HISTORY_PRESETS
  (1/6/24 hours), :95 METRIC_HISTORY_MAX_COLUMNS = 120, :106
  METRIC_HISTORY_SAMPLE_LIMIT = 20000 (the Server's own MAX_SAMPLE_LIMIT, so the
  panel asks for the largest answer the Server carries instead of asking for
  less), :114 formatCanonicalInstant, :120 metricHistoryRange, :184
  bucketMetricSamples (time buckets keep min and max plus the first and last
  instant they cover — platpulse-web/src/metricHistory.ts:139 firstInstant, :143
  lastInstant — and drop non-finite timestamps or values instead of plotting a
  zero), :250 metricChartGeometry, :306 metricLinePath, :312 metricBandPath, :164
  niceMetricMax, :323 formatHistoryDuration, :352 formatSampleDelay, :360
  metricAvailabilityNotice, :370 metricGapKindLabel.
  platpulse-web/src/metricHistory.test.ts covers it with 12 tests, including the
  silence rule, the same-bucket silence split and the wrong-way delay.
- The line break is data driven, not geometric, and the observations are split
  before they are folded into columns. A bucket is a drawing unit, and the two
  stored observations that bracket a silence can fall inside one bucket, so a
  bucket is not evidence that what it holds was continuous. The geometry therefore
  reads the reported silences first
  (platpulse-web/src/metricHistory.ts:259 silenceIntervals) and cuts the sequence
  into runs at every silence that covers the whole stretch between two consecutive
  stored observations
  (platpulse-web/src/metricHistory.ts:279 splitAtSilences); each run is folded and
  drawn on its own
  (platpulse-web/src/metricHistory.ts:314 metricChartGeometry), so nothing is ever
  drawn between two runs and a same-bucket silence cuts the line instead of being
  drawn through. A column is then placed at the centre of the instants it really
  holds (platpulse-web/src/metricHistory.ts:191 bucketMetricSamples) rather than at
  the centre of the bucket it fell in: a bucket centre is a position no observation
  in that column has to support, and a reported silence can cover it — which is
  exactly how the reviewer's reproduction (samples at 300, 330, 540, 570, 750 and
  780 s in a 24 h window of 120 buckets, silence 330-540 s) used to draw
  `M 2.50 8.00 L 7.50 8.00` straight through its own band at x 2.292-3.750. Two
  earlier variants are recorded in the code as rejected: bucket-index adjacency
  fragmented a sparse series into three segments instead of two, and a plain
  positive-overlap test rendered no line at all. Against this the browser check
  `expectNoLineCrossesTheGap` now allows only the 0.005 the two-decimal path
  formatting can absorb
  (platpulse-web/e2e/metric-history-acceptance.spec.ts), because a drawn stretch
  no longer has any legitimate reason to reach into a reported band.
- platpulse-web/src/api/admin.ts:12 imports the shared bound,
  platpulse-web/src/api/admin.ts:240 nodeMetricHistoryRoot composes the key
  (:243 puts metric and range in it),
  platpulse-web/src/api/admin.ts:531 fetchAdminNodeMetricHistory sends
  query { metric, from, to, limit: METRIC_HISTORY_SAMPLE_LIMIT } (:545), :561
  useAdminNodeMetricHistory serves it, and the key is invalidated by the
  per-resource SSE list (platpulse-web/src/api/admin.ts:1499).
- platpulse-web/src/pages/AdminNodes.tsx:908 MetricHistoryPanel, registered at
  platpulse-web/src/pages/AdminNodes.tsx:649 on the Node detail page. The panel
  shows the series selector, the range presets, the ledger (observations, first
  and last observed, last received, proven coverage against the window, replays
  and corrections counted apart, the samples this answer carries with grain raw,
  the newest delay with its clock note, the retained raw window and the requested
  start), the availability and truncation notices, the never reported and
  observed-but-empty states, the chart with one rect per reported silence, the
  silence list with kind, span, duration, reason and skipped count, and the newest
  observations table. The panel copy states the grain plainly
  (platpulse-web/src/pages/AdminNodes.tsx:957): every value is one observation the
  Node actually sent, never a value averaged over a coarser interval.
- Two frontend findings from review are fixed in the panel. A delay that runs the
  wrong way is a real clock anomaly (platpulse-web/src/metricHistory.ts:352):
  formatSampleDelay reports a negative delay as its magnitude "ahead of receipt"
  instead of letting -300 s format to "0 seconds", and an unknown delay stays
  "Unknown"; the ledger (platpulse-web/src/pages/AdminNodes.tsx:1062) and every
  sample row (platpulse-web/src/pages/AdminNodes.tsx:1239) use it. An observation
  alone in its own column is drawn as a min/max whisker inside that column next to
  its point (platpulse-web/src/pages/AdminNodes.tsx:1165, data-slot
  metric-history-whisker), so an isolated spike is never hidden by a single
  plotted value — and nothing is interpolated across time nobody covered.
- platpulse-web/e2e/metric-history-acceptance.spec.ts — the browser acceptance
  path: a real disposable Server, a real Agent Enrollment, real Reports whose
  observations are stamped near now, a real low-space protection transition
  (floor raised, Server restarted, reports skipped and recorded, floor cleared,
  collection resumed), then the REST answer and the panel, including the fixed
  viewport and theme matrix (the shared VIEWPORTS of
  platpulse-web/e2e/admin-flow.ts:15 x light/dark, run once against one disposable
  Server, with touch targets and keyboard focus checked at 768 px and below). Both
  tests call expectNoLineCrossesTheGap
  (platpulse-web/e2e/metric-history-acceptance.spec.ts), which parses every drawn
  path and proves that no line crosses the band the Server reported.

## DECISIONS

- The ledger stores no value. node_metric_series_state is a ledger about the
  series, not a second copy of the history: it keeps first/last observed instants,
  the counts and nothing that could disagree with node_metric_samples about a
  value.
- Gaps are derived on read, never stored. The only stored silence is the capacity
  pause #212 already records; deriving everything else at read time means a
  retention cleanup can never leave a stale gap row behind, and the answer always
  describes the samples it actually returns.
- The Server measures the observed cadence instead of being told it. Assuming the
  configured interval would make any series that reports on its own schedule look
  permanently broken, so the threshold is max(3 x observed cadence, 120 s) where
  the observed cadence is the minimum positive delta of the returned samples.
- Coverage counts proven stretches only. Pairs closer than the threshold add to
  coverage; anything else is a gap, so a silence is never averaged into the
  coverage the ledger claims.
- No aggregate tier in this ticket. The response carries grain "raw" and
  aggregate_supported false (mirroring AdminBlockHistoryResponse) so #214 can add
  the tiers without changing what this answer means.
- Retention owns the window, not the write path. Deleting the 64 sample prune
  without a family would have left Host samples unbounded, so the new family
  targets both tables; the Host preference split belongs to a follow-up ticket.
- A debug build, one Agent and one Node were measured. Every number in the
  baseline below is tied to those declared conditions, and the substitutions are
  listed verbatim under Not delivered instead of being extrapolated.
- The per-Report cleanup budget has to cover what one Report can add. Retention
  runs opportunistically after each accepted Report, so a batch smaller than one
  Report's own contribution would let the backlog grow even though every Report
  expires its oldest rows; the failure mode is a lagging expiry rate that grows
  the table until low-space protection pauses the very history this family exists
  to keep. NODE_METRIC_CLEANUP_BATCH (2048, with a compile-time guard against the
  protocol maximum) therefore replaces the ordinary 128 for the Node table, while
  the Host table keeps 128 because one Report adds a handful of host rows. No
  scheduler was added: a bound that covers one maximal Report makes the two rates
  equal at steady state.
- A skipped count is attached only to a stretch that covers the whole pause. The
  capacity ledger keeps one count per interval and no per-skip timestamps, so a
  window or a gap that clips a pause cannot claim its losses: it reports the
  silence without a skippedCount instead of guessing how many of those skips fall
  inside the stretch it describes. For the same reason a known pause that starts
  and ends inside a pair of samples closer than the gap threshold is subtracted
  from coverage rather than counted as proven.
- The pause lookup got its own index. capacity_skipped_series is keyed by
  interval_id first (migration 0065), so the (scope_kind, scope_key, metric,
  last_skipped_at) predicate this read runs on every Owner request would scan the
  table; migration 0066 adds the covering index instead of changing the 0065 key.
- Bounds must be canonical. Stored instants are compared as text, so a from or to
  that represents the right instant in another shape (fractional seconds, a
  +08:00 offset, another precision) would silently exclude rows; the read path
  accepts only the Server's canonical second-precision UTC form and answers
  invalid_history_range otherwise, while an unparsable query answers
  invalid_query.
- Owner-only history is served no-store. The response is per-Node operational
  evidence behind the Owner role, so it is never cached (the same helper the other
  Owner reads use).
- Accepted bound, disclosed rather than changed: capacity_skipped_series has no
  retention target and capacity_protection_intervals is never deleted, so the
  pause ledger grows by one row per interval per series forever. It is tiny, and
  giving it a lifecycle is a separate decision about audit evidence.
- The released-before stamp is per series, not one watermark per family. A single
  stamp on the retention policy row would have cost one UPDATE per cleanup instead
  of this EXISTS-probed pass, but it would suppress out-of-order counting for every
  series of the family — including series that never released anything — and it
  would have forced the same edit into the CHECK-heavy retention_policies rebuild
  of crates/platpulse-server/migrations/0066_node_metric_history.sql:103-143.
  Keeping it on node_metric_series_state keeps the suppression where evidence was
  really released, and it costs no extra query: the delivery path already selects
  that row.
- The stamp is written before the bounded deletes, so it claims "this cutoff was
  applied to this series" rather than "these exact rows were removed". The delete
  budget can legitimately leave a backlog — that is what
  crates/platpulse-server/src/retention.rs:175 NODE_METRIC_CLEANUP_BATCH is for —
  and a later widening must not make that backlog look like new observations.
- A gap never starts before the window it answers
  (crates/platpulse-server/src/metric_history.rs:369-372, with the window bound at
  crates/platpulse-server/src/metric_history.rs:629): the pause query returns
  the whole overlapping interval, so the continuity walk clamps its lower bound to
  the requested start, and a clipped interval reports no skippedCount at all rather
  than crediting this answer with losses outside it.
- One skipped instant disproves the whole stretch it sits in
  (crates/platpulse-server/src/metric_history.rs:380 pause_intersects): a pause
  whose first and last skipped instant are the same still means the ledger recorded
  a loss between the two samples that bracket it, and the ledger records which
  instants were skipped, never how long the unobserved stretch between them was, so
  such a pair proves no coverage at all.
- Bounds must be second precision, not merely parseable. The RFC 3339 parser
  accepts fractional seconds and prints them back, so the round-trip check alone
  would have let `…T00:00:00.5Z` through as a valid bound and quietly answered a
  window that excludes the stored `…T00:00:00Z` row it orders before.
  crates/platpulse-server/src/metric_history.rs:500 canonical_instant therefore
  requires nanosecond == 0 as well.

## STORY 47 BASELINE (scripts/metric-history-baseline.py)

Report: `target/metric-history-baseline/20261003T143219Z/baseline.json` and
`baseline.md`, generated 2026-10-03T14:32:19Z from
`target/debug/platpulse-server`; 21 of 21 checks ok. The script drives the
production binary: a real Owner login through the Admin API, a real Agent
Enrollment, real Reports through `/api/agent/v1/reports` into a temp SQLite
database, and real capacity transitions by rewriting the `[capacity]` of
`server.toml` and restarting the Server. Nothing is synthesised inside the Admin
API, and every row the reads return came from an accepted Report.

### Declared conditions

- Hardware: 32 CPUs, 125.65 GiB memory, AMD EPYC 7313P 16-Core Processor.
- Filesystem: / (ext4), 1.32 TiB available.
- Declared cadence: 30s over 23.0h = 2760 rounds, seed 213. The requests below
  span that whole timeline inside the 24 hour window, and the unparameterised
  request is measured too.
- Server mode: development, 127.0.0.1:50767.

### Steady write path

- 2754 observations stored; 6 further rounds were submitted while low space
  protection was adopted and were skipped as one recorded pause. Every Report
  answered `accepted` (2763 of 2763, zero rejections).
- Report latency p50 15.655ms, p95 28.044ms, max 42.3ms — ingestion plus the new
  ledger upsert plus the stored value read that classifies the delivery.
- Wall time 47.235s to store the history before the pause, 2.36s after it.

### Storage

- Database 10.68 MiB over 2734 pages of 4096 bytes.
- Rows: node_metric_samples 5508, host_metric_samples 5508,
  node_metric_series_state 2, capacity_skipped_series 4,
  capacity_protection_intervals 1, agent_report_receipts 2763, nodes 1.
- 1016.564 bytes per stored optional sample (the two series of one Report plus its
  receipt). The ledger for this 24 hour history is two rows, not a second copy of
  the samples.

### Read path

- 1h: 114 items, truncated false, 3.546ms, 16.67 KiB payload, coverage 3390s, gaps 0.
- 6h: 712 items, truncated false, 16.275ms, 101.57 KiB payload, coverage 21300s, gaps 1.
- 24h: 2754 items, truncated false, 49.425ms, 391.57 KiB payload, coverage 82560s, gaps 1.
- 24h with limit 10: 10 items, `truncated` true, 2.19ms, 2.03 KiB payload.
- no from and no to: 86400s window, 2754 items, 45.452ms, 391.57 KiB payload, so
  the declared 24 hour floor is what an unparameterised request answers with.
- Coverage is 82560s of the 82855s requested: only proven stretches count, the
  210s protection pause and the joins around it are never assumed.
- A series this Node never reported (`data_directory_percent`) answers
  `observed false` with 0 items and 0 gaps in 1.657ms and a 588 byte payload — an
  absent line, not a zeroed one.
- Refusals: unusable metric token 400 `invalid_metric`; inverted range 400
  `invalid_history_range`; unknown Node 404 `not_found` (no series leaked); a
  range entirely older than the released history answers 200 with availability
  `unavailable`, `requestedFrom` preserved and 0 items.

### What the paused stretch proved

- The silence is one `protection_pause` from 13:31:49Z to 13:35:19Z (210 seconds)
  with `skippedCount` 6 — and its kind is `protection_pause` rather than
  `collection_gap` only because `capacity_skipped_series` overlaps it.
- None of the six paused instants appears in the items, and `coverageSeconds`
  stays at the proven 82560s instead of the window.
- The cadence the Server measured is 30s, so its own silence threshold is the 120s
  floor. The first version of this script paused only 90s and the Server reported
  no gap at all; that was the script being wrong, not the Server: silence shorter
  than `max(3 x observed cadence, 120s)` is deliberately not reported as a gap.
- A carried last good delivery is a replay (`observationCount` unchanged,
  `replayedCount` +1) and a restated value is a correction (`correctedCount` +1);
  the series answers with the corrected 10.25.
- The ledger outlives the release. A Report whose instant was older than the one
  day floor was retained, then released by retention cleanup: `observationCount`
  2755, `sampledCount` 2754, `firstObservedAt` moved to the released instant, and
  `node_metric_series_state` still held both rows while the sample row was gone.

### Rerun of record (28 checks, current tree)

Report `target/metric-history-baseline/20261003T152435Z/baseline.json` and
`baseline.md`: 28 of 28 checks ok, exit 0, against
`target/debug/platpulse-server` built 2026-10-03T15:17:14Z from HEAD c542313 plus
the working tree this document describes. It supersedes the 23.0 hour first run
above and adds the multi-Node arrival and cleanup-budget phases.

- The declared window is now the full 24.0h at 30s = 2860 rounds of a possible 2880,
  with a 600s guard that keeps the planned timeline off the expiry cliff, so the
  plan and the Server's own retained window agree instead of nearly agreeing.
- Write: 2854 observations stored, 6 paused rounds as one recorded pause, 2926 of
  2926 Reports accepted and no rejections, Report latency p50 16.201ms / p95
  23.447ms / max 37.28ms, 47.961s before the pause and 1.834s after it.
- Read 24h: 2854 items, window 85855s, truncated false, 50.337ms, 415516 byte
  payload; the unparameterised request answers the 86400s floor with the same 2854
  items.
- Coverage is now checked against the plan rather than against a recomputation of
  the Server's own rule: planned proven seconds 85560 over 2 stretches with a 210s
  straddle deliberately longer than the Server's 120s threshold, and the Server
  answers exactly that, with the rule-derived number kept only as an informational
  cross-check. The earlier oracle restated the rule it was checking, which the
  reviewer correctly called circular.
- One gap: protection_pause 2026-10-03T14:24:05Z → 14:27:35Z, 210s, skippedCount 6,
  matching the planned instants that straddle the injected pause. Ledger:
  observationCount 2854, replayedCount 2, correctedCount 1, sampledCount 2854,
  coverageSeconds 85560.
- Storage: node_metric_samples 12108, host_metric_samples 5782,
  node_metric_series_state 802, nodes 161, agent_report_receipts 2926,
  database_bytes 19398656, 1084.33 bytes per stored optional sample.
- Multi-Node arrival (part 1): 48 Reports of 33 Nodes at a 2400s cadence, all
  accepted, Report latency p50 344.299ms / p95 500.395ms / max 546.228ms, 17.756s
  wall; 32 cloned Node rows and 160 series ledger rows registered; exactly 5760 rows
  stored, which is the 36 rounds inside the Server's raw window times the 160 rows
  each Report carries, with 12 rounds arriving already outside it.
- The per-Report cleanup budget (crates/platpulse-server/src/retention.rs:175
  NODE_METRIC_CLEANUP_BATCH = 2048): the drain phase (12 pressure Reports of 129
  Nodes, one anchor plus 128 clones, paced 2.0s, each adding 640 rows inside the
  window) released 4480 rows in total with a single cleanup releasing up to 1280,
  drained the backlog to 0 expired rows, and left the two following pure replays
  changing nothing at all (stored rows stayed at 640).
- The old 128 bound is falsified by measurement plus arithmetic rather than by
  running it: one accepted Report adds 640 rows while one opportunistic cleanup
  released 1280 of them (1920 in a 1 hour pre-flight), so under a 128 per-Report
  bound the backlog floor after that burst is
  max(0, additions - 128 x cleanups) = 2944 rows, and the two pure replays could not
  have stayed unchanged. The 128-row variant cannot be executed at all, because
  editing crates/platpulse-server/src/retention.rs to lower the constant is out of
  scope for this ticket; this is stated as a measured bound with arithmetic, not as
  a run.
- Check 13 changed meaning with the novelty floor: an expired redelivery with no
  stored sample is a Replay, so observationCount and sampledCount stay put while
  replayedCount advances. The first run of the rewritten script scored 27 of 28
  against the stale "+1 observation" expectation, which is how the concurrent
  semantics change was caught and is the reason the check now asserts the design's
  own rule. firstObservedAt deliberately moves backwards to the released instant
  (crates/platpulse-server/src/metric_history.rs:534 record_delivery keeps
  MIN(first_observed_at, observed_at) even for a replay); the script records that as
  a datum instead of asserting on it.
- Not delivered, verbatim from this run: "The fixture reports only
  process_cpu_percent and process_memory_percent for the one measured Node:
  data_directory_percent, peer_inbound_count and peer_outbound_count are carried by
  the multi-Node clones below but their one-Node path has integration-test coverage
  only, not a measured history here."; "The one-Node shape cannot show per-Report
  drain behaviour under a multi-Node arrival rate; the multi-Node phase below
  measures it instead (rows added per Report, rows released by one cleanup, and the
  two pure replays that must release nothing)."; "A declared cadence of 1s, 2s or 3s
  over the full day exceeds the bounded read limit (DEFAULT_SAMPLE_LIMIT 5000 per
  request, MAX_SAMPLE_LIMIT 20000), so such a window is answered truncated to the
  newest samples rather than in full."
- Also unverifiable in this run: the multi-Node inventory is synthetic (each Report
  declares its own cloned Nodes at a raised inventory revision) and could not be
  compared against a real multi-Agent deployment, and the drain geometry is
  timing-sensitive — it needs one 129-Node Report to be written in under about 8s,
  and it measured 2.0-2.9s.

### Rerun of record on the fixed tree (28 checks, 20261003T164511Z)

Report `target/metric-history-baseline/20261003T164511Z/baseline.json` and
`baseline.md`: 28 of 28 checks ok, exit 0, against
`target/debug/platpulse-server` built 2026-10-03T16:17:22Z from the same HEAD
c542313 plus the working tree this document describes — so the review's fix set
(the Replay write gate at crates/platpulse-server/src/http/report_ingestion.rs:874
and the ProtectionPause band at crates/platpulse-server/src/metric_history.rs:274)
was in the binary this run measured. Its own check list is the gate: every
semantic number the first run of record reported is reproduced exactly.

- Unchanged, to the value: 2854 observations stored of 2860 planned rounds with 6
  paused rounds and one recorded pause; 2926 of 2926 Reports accepted and no
  rejections; the 24 hour read answers 2854 items over an 85865 s window with
  truncated false and 85560 s of proved coverage; one gap,
  protection_pause 2026-10-03T15:44:41Z → 15:48:11Z, 210 s, skippedCount 6; ledger
  observationCount 2854, replayedCount 2, correctedCount 1, sampledCount 2854;
  storage node_metric_samples 12108, host_metric_samples 5782,
  node_metric_series_state 802, nodes 161, agent_report_receipts 2926; multi-Node
  part 1 accepted all 48 Reports of 33 Nodes and stored 5760 rows with 32 Node rows
  and 160 series rows; the drain released 4480 rows in total with a single cleanup
  releasing 1280, drained to 0 expired rows and left the two following pure replays
  with nothing left to release; the release phase still moves replayedCount by one
  while observationCount and sampledCount stay at 2854 and the released instant is
  still not stored (released_instant_stored false).
- Moved, and only these: Report latency is slower in this run (p50 20.652ms / p95
  30.711ms / max 45.884ms against 16.201 / 23.447 / 37.28), the 24 hour payload is
  415516 bytes over 51.478ms, the multi-Node phase is faster (p50 277.815ms / p95
  348.134ms / max 364.514ms), and the drain's release order shifted within the same
  budget (rows released per Report [0,0,0,0,640,1280,1280,1280,0,0,0,0] instead of
  [0,0,0,0,1280,1280,1280,640,0,0,0,0], both summing to the same 4480 with the same
  1280 maximum). The host is not idle: a `platpulse-agent` process (~57% CPU) and a
  `platon` process (~26%) were running throughout, which is why the write timing
  differs between two runs of the same binary; the readings are reported as
  measured, not as a controlled comparison.
- The 6 hour read's window edge lands one sample later this run: sampledCount 711
  and coverageSeconds 21270 against 712 and 21300, because the window is anchored
  to the run's own instant. Its check passes either way, since the expectation is
  derived from the plan's instants rather than from the Server's rule.

### Not delivered (verbatim from the report; a follow-up ticket appends to it)

- The 24 hour window was compressed in wall time: every observation instant is
  real, but the Reports were submitted as fast as the Server accepted them instead
  of one per declared cadence.
- Agent-side collection was not measured: Reports came from the fixture through
  the real ingestion path, not from a running platpulse-agent process.
- One Agent, one Node and one mount were measured; no multi-disk or network
  filesystem deployment.
- The aggregate tiers (one minute and five minute) belong to issue #214 and were
  not exercised.
- The Server ran in development mode without TLS and without a reverse proxy, and
  the build is a debug build.
- These items must be appended to this same report by a follow-up ticket; nothing
  here is a production guarantee.

## DOCS

- docs/design/platpulse.md section 11.6 (new) records the three changes, the
  counting rule, the read time gap derivation, the availability semantics, the
  ledger outliving released rows and the raw 24 hour floor; the Metric History row
  of the section 11.4 data boundary table now names the raw_metric_sample family.
- docs/design/webui.md section 15.16 (new) records the panel layer by layer:
  what the Operator sees per series, what the ledger states independently, which
  unanswerable requests are stated instead of filled in, and that no control
  writes history, edits policy, pauses collection or deletes samples.

## WORKFLOW

- Rust: `cargo fmt --check` clean; `cargo clippy --all-targets --all-features -- -D
  warnings` clean; `cargo test --workspace` green — 969 tests passed and 0 failed
  over 27 targets, among them 617 Server unit tests and every integration target
  (capacity_protection 7, node_metric_history 5, node_purge 13, migration 11). The
  review's fix set is counted in those totals: it added three regressions (a lib
  unit test for the evidence floor, a lib unit test and an HTTP integration test
  for a widening that must not recount an observation it released) and rewrote
  three that had asserted the reviewed behaviour (the two pause-below-threshold
  unit tests that expected no gap and the widening HTTP test that expected the
  carry to be written back). Two integration fixtures had to be
  restamped near the current clock first, because the environment clock is
  2026-10-03 and the new 24 hour raw floor releases anything older:
  crates/platpulse-server/tests/capacity_protection.rs:300 built every report with a
  fixed 2026-08-12 generated_at (its helper report_at now moves the newest instant of
  the fixture onto a caller-chosen instant, so the fixture's own relative offsets
  survive), and crates/platpulse-server/tests/node_metric_history.rs spaced its three
  samples an hour apart, which the Server correctly reports as two collection gaps,
  because no Agent can declare an interval slower than 300 s
  (crates/platpulse-agent/src/config.rs:164); that fixture now spaces them a minute
  apart, and a second test asserts the hourly spacing is answered as two
  collection_gap entries of 3600 s with a null skippedCount and zero proved coverage.
  A `cargo test --workspace` abort stops at the first failing target, so every
  integration binary was also run on its own after the fixes.
- Dependencies: `cargo deny check` clean (advisories, bans, licenses, sources all ok)
  and `cargo audit --ignore RUSTSEC-2023-0071 --ignore RUSTSEC-2026-0253` reports 3
  allowed warnings (chacha20 0.10.1 yanked, RUSTSEC-2024-0436) and no vulnerability.
- Web: `npm run lint`, `npm run typecheck`, `npm test` (38 files, 605 tests) and
  `npm run build` all clean; the chunk size advisory is the only build warning.
- Browser: `npx playwright test e2e/metric-history-acceptance.spec.ts
  --project=desktop-1280` — 2 passed in 40.3 s, and the viewport and theme matrix with
  its gap intersection check runs inside those two tests. The full suite was then
  run on this tree (`npx playwright test --reporter=list` from platpulse-web, the
  same harness that rebuilds the bundle and runs the Server from source): 689
  passed, 200 skipped, 1 failed in 23.5 min. The single failure is not on this
  change's path and is a race inside the spec of issue #209:
  platpulse-web/e2e/backup-verification-acceptance.spec.ts:165 asserts the
  artifact's verification state is still `pending` immediately after the request,
  which the Server's own verification task wins whenever the host is fast enough.
  It was reproduced twice on an idle host (expected "pending", received "ok") and
  passed three of three under deliberate CPU load, and neither the endpoint that
  queues the task nor that spec is touched by this change. The pre-fix baseline
  (690 passed, 200 skipped, 0 failed) ran while the host was also building this
  fix set — the very contention that racing assertion needs.
- OpenAPI: the document printed twice, and the client generated twice, produce no
  diff against the committed artifact, so the served contract and
  platpulse-web/src/api/generated are the same document.

## REVIEW

A second agent (cliproxyapi/gpt-6.1-sol, high reasoning effort, read-only) read
every new file and every key hunk of this change twice, on two trees. Its first
dual-axis report, against the acceptance criteria as written at that moment, was
"do not ship". Its second report read the tree that carried the first round's
fixes and still returned "do not ship", naming four defects that round had not
reached: an observation counted below the floor, a carry re-inserted after a
widening, a known short pause drawn as continuity, and the resource cost of the
floor update. Both reports are kept below as the findings of record, each item
followed by what this tree now does about it.

### Standards axis

Conventions followed, in the reviewer's own reading: the Retention policy migration
(crates/platpulse-server/migrations/0066_node_metric_history.sql:92-143), the
indexed pause lookup, the bounded cleanup with its compile-time guard
(crates/platpulse-server/src/retention.rs:175-181), sanitized query errors and
`no-store` responses
(crates/platpulse-server/src/http/admin.rs:4663-4684). Two earlier findings were
confirmed fixed: the invented-zero delay
(platpulse-web/src/metricHistory.ts:394 formatSampleDelay) and the fresh timestamp
on a carried directory value
(crates/platpulse-server/src/http/report_ingestion.rs:1375-1384).

No cross-Node or cross-Agent series disclosure was found. The history query pins
Node and metric (crates/platpulse-server/src/metric_history.rs:629), ingestion
checks Node ownership and validated transfers
(crates/platpulse-server/src/http/report_ingestion.rs:2270-2292), and Owner access
is intentionally fleet-wide. This is a GET behind the existing Owner guard, not a
control operation, so the existing Origin/CSRF and durable mutation-audit
boundaries are reused rather than duplicated.

### Spec axis (first report)

| Criterion | Reviewer | State in this tree |
| --- | --- | --- |
| 1. Real 24 hour history, five raw series | Partial | Partial and disclosed: the measured one-Node history carries two series, the other three have integration coverage only, and a 1/2/3 s cadence over a full day is answered truncated to the newest samples (see the baseline's Not delivered list) |
| 2. Independent timing, source, enablement, coverage | Partial | Improved: observed, received, delay, clock suspicion and proven coverage are independent per sample and per series, and the DTO no longer calls the first observation enablement (crates/platpulse-server/src/http/admin.rs:4375-4380). Report-source evidence and per-series collector failure/stale/disabled state are still not delivered and are listed as such |
| 3. Carry and replay cannot inflate counts or coverage | Does not satisfy in both reports | Fixed in this tree by the fix set below: no replay writes a row back (crates/platpulse-server/src/http/report_ingestion.rs:874), a counted-but-unstored instant leaves an evidence floor (crates/platpulse-server/src/metric_history.rs:589), a single skipped instant never proves coverage and is reported as a pause (crates/platpulse-server/src/metric_history.rs:274), with regressions at crates/platpulse-server/src/http/report_ingestion.rs:3852 and :3961 and crates/platpulse-server/tests/node_metric_history.rs:778 |
| 4. Distinguishable missing states, no fabricated lines or zeroes | Partial | Improved: never observed, observed-but-empty and unavailable stay distinct, and a same-bucket silence no longer receives a line (platpulse-web/src/metricHistory.ts:279 splitAtSilences). Collector failure and stale/disabled states remain undelivered |
| 5. Retention family, floor, preview and capacity guard | Satisfies structurally | Unchanged, plus the released_before stamp the same family now carries |
| 6. Authorization, security, audit | Satisfies by inspected integration | Unchanged |
| 7. Responsive, themes, keyboard, touch | Partial verification | Verified in this tree: the second browser test loops the fixed viewport and theme matrix with a collection gap on screen and the gap intersection check, and both tests passed (npx playwright test e2e/metric-history-acceptance.spec.ts --project=desktop-1280, 2 passed in 23.6 s). The reviewer itself did not run browsers |

### Second report and the fix set

The second report re-judged the same seven criteria on the tree that carried the
first round's fixes: 2 and 4 improved but still partial, 3 still "does not satisfy"
because of the two replay defects below, 5 and 6 satisfied by inspection, and 7
verified against the acceptance criteria rather than against a browser run of its
own. Its blocking and medium findings, and the fix this tree carries for each:

| Finding (second report) | Fix in this tree |
| --- | --- |
| BLOCKING — an observation counted while nothing stores it leaves no record that it was counted, so a widening of the window over the same instant counts the Agent's carried copy as a second observation | `counted_evidence_floor` (crates/platpulse-server/src/metric_history.rs:589) stamps the next second above the counted instant as that series' evidence floor, and `stamp_evidence_floor` (crates/platpulse-server/src/metric_history.rs:595) moves the floor forward only. Ingestion calls it when the delivery was counted while `outside_retained_window` held (crates/platpulse-server/src/http/report_ingestion.rs:901). Regressions: crates/platpulse-server/src/http/report_ingestion.rs:3961 (a 40 h old first observation, the window widened to two days, a restart, the same instant carried again, count stays 1) and crates/platpulse-server/tests/node_metric_history.rs:778 (the same over real HTTP) |
| BLOCKING — a correctly classified replay is still written back after a widening, and the recreated row carries the current receipt, so a carry of adjacent expired instants rebuilds a covered interval and restamps its evidence | The write gate (crates/platpulse-server/src/http/report_ingestion.rs:874) writes a row only for a delivery that is not a classified Replay: a widened window is read again, never rebuilt. The widening regression (crates/platpulse-server/src/http/report_ingestion.rs:3852) now asserts `samples == 0` after the widening instead of endorsing the reinsertion it used to assert |
| BLOCKING — a pause the Server recorded and counted is drawn as continuity whenever the stretch between two stored observations is shorter than the jitter threshold, so the chart joins across a loss it knows about | Any pair the ledger holds a counted loss inside is reported as a `protection_pause` band (crates/platpulse-server/src/metric_history.rs:274), however short the stretch, and the interval-wide skip count is attached only when the stretch covers the whole pause. The two unit tests that asserted `gaps.is_empty()` now assert the band (crates/platpulse-server/src/metric_history.rs:912 and :941), so the web layer cuts the line where it already cuts it for an inferred gap |
| MEDIUM — the floor UPDATE scans or probes every series on every accepted Report, and its comment treats the per-Report protocol bound as a fleet cap | The stamp is bounded to the keys of the delete candidates: the same range read, ordering and bound as the delete it precedes (`WITH expired AS MATERIALIZED ... LIMIT 2048`, crates/platpulse-server/src/retention.rs:235), so the ledger is probed by primary key, and the comment now states the fleet argument instead of misreading the protocol bound |

Two findings were left disclosed rather than fixed, and both are named in the
acceptance note below: Report-source provenance and an independent per-series
collector failure, stale or disabled state (the second report's HIGH item), and
bounded raw paging at the 1, 2 and 3 second cadences (its MEDIUM item), which
belongs with the aggregate tiers of issue #214.

### The first report's defects and what happened to them

- Blocking, replay recount after a Retention widening: "Classification:443-444
  treats an absent instant inside the current cutoff as new. Store T, expire its row
  under one day, widen to two days, then carry T at age 30h: lifetime
  observationCount increments again." Fixed with the persistent per-series
  released_before floor and the expire → widen → restart → carry regression.
- Blocking, a fabricated line through a same-bucket gap, reproduced by the reviewer
  itself: six observations across a 330-540 s silence in a 24 hour window produced a
  band at x 2.292-3.750 while the path was still `M 2.50 8.00 L 7.50 8.00`. Fixed
  by splitting the observations at reported silences before folding and placing each
  column at the centre of its own observations; the reviewer's exact reproduction is
  now a unit test.
- Blocking, one skipped sample inflating coverage: "Overlap:356-358 returns zero
  when firstSkippedAt == lastSkippedAt. Stored samples T/T+60 with skipped T+30
  therefore get all 60s credited." Fixed by pause_intersects, which treats a
  single-instant pause as disproving the pair, with a regression test.
- High, canonical validation accepting fractions: fixed in canonical_instant
  (nanosecond == 0 plus the round-trip check), with the reason recorded in the code.
- High, pause lower bounds extending outside the answer: fixed by passing
  window_start into continuity and clamping both boundaries, with a regression test.
- High/spec, missing provenance and collector states: partially addressed — the
  Server no longer labels the first observation "enablement"
  (crates/platpulse-server/src/http/admin.rs:4375-4380) and the panel already reads
  "First observed". Report-source evidence and per-series collector state are not
  delivered here and are disclosed as such rather than faked.
- Medium, incomplete full-day retrieval at supported cadences: disclosed, not
  fixed. Bounded raw pagination belongs to issue #214 together with the aggregate
  tiers; the panel asks for the Server's own bound (20000) and shows the truncation
  notice instead of silently asking for less.

The reviewer also corrected one label in the baseline script: the check written as
"observationCount unchanged" had actually observed 2754 → 2755 in the earlier run,
because a released row keeps its ledger entry and only the sample row goes. That
check was rewritten (now check 13) to assert the lifetime count unchanged, the
replay count +1 and the released instant not stored, which is what the design
actually claims.

## VERIFICATION

Every acceptance criterion was checked on this tree, with the check that proves it.

1. A demonstrable 24 hour raw history end to end, without rebuilding history the
   Server no longer holds. Ingestion writes every in-window observation
   (crates/platpulse-server/src/http/report_ingestion.rs), the range API serves it
   (Owner-only `GET /api/admin/v1/nodes/{node_id}/metric-history`,
   crates/platpulse-server/src/http/admin.rs:4462), and the Node Admin panel draws
   what it returns (platpulse-web/src/pages/AdminNodes.tsx:908 MetricHistoryPanel).
   Evidence: crates/platpulse-server/tests/node_metric_history.rs (5 tests over real
   HTTP against a real temporary SQLite database),
   platpulse-web/e2e/metric-history-acceptance.spec.ts (2 tests, both passing), and
   the 28-check baseline run of record
   (target/metric-history-baseline/20261003T164511Z, re-measured on this tree),
   whose 24 hour read returned 2854 stored observations inside a 85865 s window
   with truncated false and 85560 s of proved coverage.
2. Last-good carry and Report replay never inflate the count or the coverage.
   crates/platpulse-server/src/http/report_ingestion.rs
   (replay_and_correction_never_inflate_the_series_observation_count) and the
   baseline's check 13: a replay leaves observationCount and sampledCount unchanged,
   moves replayedCount by one and stores no row for the released instant. The
   retention widening regression in crates/platpulse-server/src/metric_history.rs
   covers the released-before floor that makes this hold across a policy widening.
3. Failures, stale series, gaps and never observed series are shown without zeros or
   fabricated lines. Server side, crates/platpulse-server/tests/node_metric_history.rs
   asserts the never-observed answer (observed false, observationCount 0, no item),
   the availability 'unavailable' answer for a range older than the floor, the
   collection gap an implausible silence produces and the protection pause with its
   counted losses. Web side, platpulse-web/src/metricHistory.test.ts (12 tests,
   including the reviewer's same-bucket silence reproduction) plus
   expectNoLineCrossesTheGap in both browser tests, which parses every drawn path in
   every viewport and theme of the matrix.
4. Observed, source and receipt timestamps, the delay, clock suspicion, first
   observation and the actual coverage are reported independently.
   crates/platpulse-server/tests/node_metric_history.rs asserts the timing evidence of
   every row and the ledger fields apart; platpulse-web/src/pages/AdminNodes.tsx
   renders them without collapsing one into another.
   crates/platpulse-server/src/http/admin.rs reports the first observation the Server
   has evidence for and no longer calls it enablement.
5. The Retention family, its floor and preview, and the capacity guard are wired.
   crates/platpulse-server/migrations/0066_node_metric_history.sql registers
   raw_metric_sample at the 1 day floor, the cleanup stamps the per-series
   released_before, and the protection pause is read back from
   capacity_skipped_series and shown as a protection gap.
   crates/platpulse-server/tests/capacity_protection.rs (7 tests) and the baseline's
   drain phase exercise the guard; the cleanup budget and its compile-time guard are
   crates/platpulse-server/src/retention.rs:175-181.
6. Authorization, Site Access Mode, Origin and CSRF, cache isolation, sanitized
   errors, last-good age, Unknown is not zero, durable Audit and no secret leak.
   crates/platpulse-server/tests/node_metric_history.rs asserts 401 auth_required for
   an anonymous caller, 403 owner_required for a Viewer, 400 invalid_metric and
   invalid_history_range, and 404 not_found for an unknown Node; the browser spec
   asserts the same codes over real HTTP. The answer is wrapped in no_store
   (crates/platpulse-server/src/http/admin.rs:52), so an Owner-only history is never
   cached, and the read writes nothing, so it records no audit event of its own: the
   family's mutations keep the audit records they already had.
7. The UI holds at 360x800, 390x844, 768x1024, 1280x800 and 1440x900 in light and
   dark, with keyboard focus and touch sized targets. The second browser test loops
   exactly that matrix against one disposable Server with a collection gap on screen,
   checking the resolved theme, the gap band, expectNoLineCrossesTheGap, the local
   table scroll, no horizontal overflow, touch sized interactive targets and visible
   keyboard focus at 768 px and below.

Open items are not asserted as done: bounded raw paging at the 1, 2 and 3 second
cadences and the per-series collector state remain partials, and both are disclosed in
the report (the 'Not delivered' section above and the review triage).
