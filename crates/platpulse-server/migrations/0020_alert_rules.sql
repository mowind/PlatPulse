-- Phase 2 Alert Rules, Incidents, Silence, and Maintenance (issue #48).
--
-- Rules are typed and Server-owned: the catalog keys live in Rust, and
-- `condition_json` is a strict, per-rule parameter object validated at the
-- trust boundary. Rule edits append immutable versions; Incidents retain
-- the rule version they opened under. Rule evaluation state is persisted
-- so timers survive Server restarts. Silence and Maintenance are
-- time-bounded policies that never mutate evaluation facts or Incidents.

CREATE TABLE alert_rules (
    rule_key TEXT PRIMARY KEY,
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    severity TEXT NOT NULL CHECK (severity IN ('info', 'warning', 'critical')),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    condition_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE alert_rule_versions (
    rule_key TEXT NOT NULL REFERENCES alert_rules(rule_key) ON DELETE CASCADE,
    version INTEGER NOT NULL CHECK (version >= 1),
    severity TEXT NOT NULL CHECK (severity IN ('info', 'warning', 'critical')),
    condition_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (rule_key, version)
);

CREATE TABLE alert_rule_overrides (
    rule_key TEXT NOT NULL REFERENCES alert_rules(rule_key) ON DELETE CASCADE,
    scope_kind TEXT NOT NULL CHECK (scope_kind IN ('network', 'node')),
    scope_value TEXT NOT NULL,
    enabled INTEGER CHECK (enabled IN (0, 1)),
    severity TEXT CHECK (severity IN ('info', 'warning', 'critical')),
    condition_json TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (rule_key, scope_kind, scope_value),
    CHECK (enabled IS NOT NULL OR severity IS NOT NULL OR condition_json IS NOT NULL)
);

CREATE TABLE alert_rule_state (
    rule_key TEXT NOT NULL REFERENCES alert_rules(rule_key) ON DELETE CASCADE,
    subject_kind TEXT NOT NULL CHECK (
        subject_kind IN ('agent', 'host', 'node', 'network', 'validator', 'server')
    ),
    subject_key TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('normal', 'pending', 'firing', 'recovering')),
    since TEXT NOT NULL,
    pending_since TEXT,
    firing_since TEXT,
    recovering_since TEXT,
    input_kind TEXT NOT NULL CHECK (input_kind IN ('known', 'unknown', 'stale', 'unsupported')),
    input_value REAL,
    input_detail TEXT,
    evidence_json TEXT,
    evaluation_unavailable INTEGER NOT NULL DEFAULT 0 CHECK (evaluation_unavailable IN (0, 1)),
    last_evaluated_at TEXT NOT NULL,
    PRIMARY KEY (rule_key, subject_key)
);

CREATE TABLE alert_incidents (
    incident_id TEXT PRIMARY KEY,
    rule_key TEXT NOT NULL REFERENCES alert_rules(rule_key),
    rule_version INTEGER NOT NULL,
    subject_kind TEXT NOT NULL CHECK (
        subject_kind IN ('agent', 'host', 'node', 'network', 'validator', 'server')
    ),
    subject_key TEXT NOT NULL,
    severity TEXT NOT NULL CHECK (severity IN ('info', 'warning', 'critical')),
    state TEXT NOT NULL CHECK (state IN ('open', 'resolved')),
    sequence INTEGER NOT NULL CHECK (sequence >= 1),
    opened_at TEXT NOT NULL,
    resolved_at TEXT,
    opened_evidence_json TEXT NOT NULL,
    resolved_evidence_json TEXT,
    UNIQUE (rule_key, subject_key, sequence)
);

CREATE TABLE silences (
    silence_id TEXT PRIMARY KEY,
    matcher_kind TEXT NOT NULL CHECK (matcher_kind IN ('all', 'agent', 'node', 'network')),
    matcher_value TEXT,
    reason TEXT NOT NULL CHECK (length(reason) > 0),
    starts_at TEXT NOT NULL,
    ends_at TEXT NOT NULL,
    created_by TEXT NOT NULL REFERENCES users(user_id),
    created_at TEXT NOT NULL,
    cancelled_at TEXT,
    cancelled_by TEXT REFERENCES users(user_id),
    CHECK (ends_at > starts_at),
    CHECK (matcher_kind = 'all' OR (matcher_value IS NOT NULL AND matcher_value <> ''))
);

CREATE TABLE maintenance_windows (
    window_id TEXT PRIMARY KEY,
    scope_kind TEXT NOT NULL CHECK (scope_kind IN ('agent', 'node', 'network')),
    scope_value TEXT NOT NULL,
    expected_rule_keys TEXT NOT NULL DEFAULT '[]',
    reason TEXT NOT NULL CHECK (length(reason) > 0),
    starts_at TEXT NOT NULL,
    ends_at TEXT NOT NULL,
    created_by TEXT NOT NULL REFERENCES users(user_id),
    created_at TEXT NOT NULL,
    cancelled_at TEXT,
    cancelled_by TEXT REFERENCES users(user_id),
    CHECK (ends_at > starts_at)
);

CREATE INDEX alert_incidents_open_idx ON alert_incidents (state, opened_at DESC);
CREATE INDEX alert_incidents_rule_idx ON alert_incidents (rule_key, subject_key, sequence);
CREATE INDEX silences_active_idx ON silences (ends_at DESC);
CREATE INDEX maintenance_active_idx ON maintenance_windows (ends_at DESC);
