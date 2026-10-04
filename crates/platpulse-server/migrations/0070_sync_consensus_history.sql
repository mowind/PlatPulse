-- Synchronization state and key consensus-height history (issue #217, design
-- §11.4, §11.5, §11.6 and §11.7, parent #202 stories 46, 53 and 59).
--
-- Story 53 asks for two different kinds of evidence, and they cannot share one
-- engine:
--
--   * the *numeric* key heights of a Node - its sync current/highest block and
--     the epoch's highest QC, lock and commit block - are quantities. They
--     belong to the metric-history engine with everything that engine already
--     provides: a 24-hour raw window, the 1-minute and 5-minute aggregate
--     tiers, gap/coverage judging, the replay/correction ledger and bounded
--     queries (design §11.6, "numeric synchronization/consensus history uses
--     the selected precision"). Migration 0070 therefore extends
--     node_metric_samples with exactly those five series and reuses
--     node_metric_aggregates and node_metric_series_state unchanged.
--
--   * *state* evidence is not a quantity. "The chain probe failed", "the value
--     shown is the last good one", "the Agent did not say whether it is
--     syncing" and "the syncing flag went from 1 to 0" are facts about an
--     observation, and a numeric aggregate would erase an Error or Unknown
--     stretch the moment it fell into a bucket, or invent a transition where
--     the Server merely missed a Report. They are recorded here instead, as an
--     append-only log of the states a Node actually reported, with its own
--     ledger and its own retention family.
--
-- Nothing in this table is inferred from Block history: a state entry exists
-- only because a Report carried the sync or consensus component, and a Node
-- that stopped reporting writes nothing at all. Silence is therefore a hole a
-- reader has to see as a hole, never a manufactured transition, and the read
-- path derives transitions from adjacent retained entries.
--
-- Storage history is not Node-scoped, but this is: synchronization and
-- consensus are properties of one Node's view of the chain, so a Node's Purge
-- takes its state entries with it (design §11.4).

-- The raw Node tier gains the five numeric series of story 53. The metric set
-- is a CHECK constraint, so the table is rebuilt rather than re-created, and
-- the rows it already holds are real evidence: every existing sample is copied
-- across and the ledger below keeps counting from where it was.
ALTER TABLE node_metric_samples RENAME TO node_metric_samples_old;

CREATE TABLE node_metric_samples (
    node_id TEXT NOT NULL REFERENCES nodes(node_id) ON DELETE CASCADE,
    metric TEXT NOT NULL CHECK (metric IN (
        'process_cpu_percent',
        'process_memory_percent',
        'data_directory_percent',
        'peer_inbound_count',
        'peer_outbound_count',
        'sync_current_block',
        'sync_highest_block',
        'consensus_highest_qc_block',
        'consensus_highest_lock_block',
        'consensus_highest_commit_block'
    )),
    observed_at TEXT NOT NULL,
    received_at TEXT NOT NULL,
    value REAL NOT NULL CHECK (value >= 0),
    PRIMARY KEY (node_id, metric, observed_at)
);

INSERT INTO node_metric_samples (node_id, metric, observed_at, received_at, value)
SELECT node_id, metric, observed_at, received_at, value
FROM node_metric_samples_old;

DROP TABLE node_metric_samples_old;

CREATE INDEX node_metric_samples_recent_idx
ON node_metric_samples (node_id, received_at DESC, metric);

CREATE INDEX node_metric_samples_observed_at_idx
    ON node_metric_samples (observed_at, node_id, metric);

-- One entry per *recorded* synchronization or consensus state. A state is a
-- vector - (collection state, value source, error code, syncing) - and it is
-- stored in components rather than as one opaque string, so a reader can ask
-- "when did this Node stop collecting consensus?" without parsing.
--
-- entry_kind tells apart the two reasons a row exists:
--
--   * 'change' - the state vector differs from the one the previous entry
--     recorded. This is the evidence story 53 is about.
--   * 'anchor' - the state vector is unchanged and no entry of this series
--     has been written for at least an hour (the ledger holds that instant as
--     last_entry_at; a series whose entries were all released anchors its next
--     delivery at once). Anchors exist so interior continuity stays provable
--     with a bounded number of rows: without them two entries of an unchanged
--     state would be arbitrarily far apart, and a long silence would look
--     exactly like a state that never changed - which is the "missing history
--     described as a known normal period" failure of story 59. Because the
--     anchor instant is a delivery the Node really sent, the stretch between
--     two adjacent entries is a stretch the Node proved, and the read path
--     measures a silence against the anchor interval plus the reporting
--     cadence.
--
-- value_source is derived from the observation the Report carried and from the
-- retained component row, never invented: 'current' means this Report carried
-- the value, 'last_good' means the Server is still showing an earlier reading,
-- and 'none' means the component has no value at all. observed_at is the
-- attempt time the Agent reported, or the Report's generation time for a
-- component that never attempted anything (Disabled, Unsupported), so an entry
-- is always a time the Node was really heard from.
--
-- value_observed_at is the instant the value the entry refers to was really
-- observed, NULL when the state has no value behind it. For a carried last-good
-- it is an instant older than observed_at, which is how far behind the shown
-- value already was at the moment the collection failed - the "last-good age"
-- of design §5.1, kept with the entry rather than joined from the live component
-- row, whose value a later Report replaces. It is recorded but is deliberately
-- not part of the change vector: a fresh value instant on every Report is an
-- observation, not a state change.
--
-- syncing is stored only when the collection state is 'ok'. An attempt that
-- failed did not observe the flag, and storing 0 there would report "not
-- syncing" for a Node whose sync state is Unknown - the Unknown-is-not-false
-- rule of design §5.1.
CREATE TABLE node_state_observations (
    node_id TEXT NOT NULL REFERENCES nodes(node_id) ON DELETE CASCADE,
    component TEXT NOT NULL CHECK (component IN ('sync', 'consensus')),
    observed_at TEXT NOT NULL CHECK (length(observed_at) = 20),
    received_at TEXT NOT NULL CHECK (length(received_at) = 20),
    entry_kind TEXT NOT NULL CHECK (entry_kind IN ('change', 'anchor')),
    collection_state TEXT NOT NULL CHECK (collection_state IN (
        'starting',
        'ok',
        'error',
        'disabled',
        'unsupported'
    )),
    value_source TEXT NOT NULL CHECK (value_source IN ('current', 'last_good', 'none')),
    value_observed_at TEXT CHECK (value_observed_at IS NULL OR length(value_observed_at) = 20),
    error_code TEXT CHECK (error_code IS NULL OR length(error_code) BETWEEN 1 AND 128),
    syncing INTEGER CHECK (syncing IS NULL OR syncing IN (0, 1)),
    PRIMARY KEY (node_id, component, observed_at)
);

-- Cleanup walks observed_at instead: one bounded batch removes the oldest
-- expired entries across every Node, and the read path pins one series
-- newest-first by the primary key.
CREATE INDEX node_state_observations_expiry_idx
    ON node_state_observations (observed_at, node_id, component);

-- The per-series ledger of the state log, the twin of node_metric_series_state:
-- it holds no state value beyond the last one that was recorded, so it is a
-- series ledger and not a second copy of the history. It is what survives the
-- expiration of the entries themselves, and it is what makes one write decide
-- whether a state is new (compare against last_collection_state and friends),
-- whether it restates an unchanged state, and whether that restatement is due
-- an anchor (last_entry_at; last_delivery_at is the newest instant counted at
-- all, which is what tells a repeat apart from an older instant seen for the
-- first time).
--
-- released_before keeps the same meaning it has in the metric ledger: the
-- oldest cutoff this series was ever pruned at, so a state replayed after a
-- policy widening is still recognised as a replay instead of passing for a first
-- sighting (issue #213). It cannot make the count exact everywhere: the instants
-- an unchanged delivery was counted for left no row, so a repeat of an older one
-- of them is indistinguishable from a delivery never seen before.
CREATE TABLE node_state_series_state (
    node_id TEXT NOT NULL REFERENCES nodes(node_id) ON DELETE CASCADE,
    component TEXT NOT NULL CHECK (component IN ('sync', 'consensus')),
    first_observed_at TEXT NOT NULL,
    last_observed_at TEXT NOT NULL,
    last_received_at TEXT NOT NULL,
    last_collection_state TEXT NOT NULL CHECK (last_collection_state IN (
        'starting',
        'ok',
        'error',
        'disabled',
        'unsupported'
    )),
    last_value_source TEXT NOT NULL CHECK (last_value_source IN ('current', 'last_good', 'none')),
    last_value_observed_at TEXT CHECK (last_value_observed_at IS NULL OR length(last_value_observed_at) = 20),
    last_error_code TEXT CHECK (last_error_code IS NULL OR length(last_error_code) BETWEEN 1 AND 128),
    last_syncing INTEGER CHECK (last_syncing IS NULL OR last_syncing IN (0, 1)),
    last_entry_at TEXT CHECK (last_entry_at IS NULL OR length(last_entry_at) = 20),
    -- The newest instant this series counted a delivery for, whether or not that
    -- delivery needed a row of its own. An unchanged state writes no row between
    -- anchors, so the entries alone cannot tell a delivery that has already been
    -- counted from the first sighting of an older instant: without this column a
    -- repeated Report would be counted as a second delivery, and an out-of-order
    -- Report that states a transition the log does not hold would be discarded as
    -- a repeat instead of being recorded.
    last_delivery_at TEXT NOT NULL CHECK (length(last_delivery_at) = 20),
    entry_count INTEGER NOT NULL CHECK (entry_count >= 1),
    change_count INTEGER NOT NULL CHECK (change_count >= 1),
    anchor_count INTEGER NOT NULL DEFAULT 0 CHECK (anchor_count >= 0),
    replayed_count INTEGER NOT NULL DEFAULT 0 CHECK (replayed_count >= 0),
    corrected_count INTEGER NOT NULL DEFAULT 0 CHECK (corrected_count >= 0),
    released_before TEXT NOT NULL DEFAULT '1970-01-01T00:00:00Z'
        CHECK (length(released_before) = 20),
    updated_at TEXT NOT NULL,
    PRIMARY KEY (node_id, component)
);

CREATE UNIQUE INDEX node_state_series_state_series_idx
    ON node_state_series_state (node_id, component);

-- observation_state is a new family name the policy table's constraint does not
-- know yet. With the old constraint in place the default seeding of the family
-- would be skipped silently (the seeding inserts with INSERT OR IGNORE), and a
-- family with no policy row is invisible to the Owner and is skipped by its own
-- cleanup path - the state log would then grow without bound, which is the very
-- outcome the 30-day investigation floor is meant to prevent. The established
-- rebuild pattern extends the constraint without changing any
-- operator-selected value; the family's own row is registered by
-- POLICY_CATALOG, because a policy row is the Owner's setting and a migration
-- must not overwrite one.
ALTER TABLE retention_policies RENAME TO retention_policies_old;

CREATE TABLE retention_policies (
    family TEXT PRIMARY KEY CHECK (family IN (
        'raw_block_summary',
        'one_minute_aggregate',
        'five_minute_aggregate',
        'one_hour_aggregate',
        'history_gap',
        'divergence_observation',
        'audit_event',
        'alert_notification',
        'peer_presence_interval',
        'peer_aggregate_5m',
        'peer_aggregate_1h',
        'validator_daily_snapshot',
        'validator_monthly_aggregate',
        'report_receipt_body',
        'raw_metric_sample',
        'observation_state'
    )),
    retention_days INTEGER NOT NULL CHECK (retention_days >= 0),
    min_days INTEGER NOT NULL CHECK (min_days >= 0),
    max_days INTEGER NOT NULL CHECK (max_days = 0 OR max_days >= min_days),
    supported INTEGER NOT NULL CHECK (supported IN (0, 1)),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    updated_at TEXT NOT NULL,
    updated_by TEXT
);

INSERT INTO retention_policies
    (family, retention_days, min_days, max_days, supported, enabled, updated_at, updated_by)
SELECT family, retention_days, min_days, max_days, supported, enabled, updated_at, updated_by
FROM retention_policies_old;

DROP TABLE retention_policies_old;
