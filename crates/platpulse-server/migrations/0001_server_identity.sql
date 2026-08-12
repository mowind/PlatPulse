-- Server identity, enrollment, human access, and Node Inventory state.
-- The Agent and Server migration directories are intentionally independent.

CREATE TABLE server_settings (
    setting_key TEXT PRIMARY KEY,
    setting_value TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE users (
    user_id TEXT PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    role TEXT NOT NULL CHECK (role IN ('owner', 'viewer')),
    password_hash TEXT NOT NULL,
    disabled_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE sessions (
    session_id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(user_id),
    token_digest BLOB NOT NULL UNIQUE,
    csrf_token_digest BLOB NOT NULL,
    created_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    revoked_at TEXT
);

-- A bounded audit sink is required by Phase 1 initialization and Network
-- bootstrap; full audit management remains a later operations feature.
CREATE TABLE audit_events (
    audit_event_id INTEGER PRIMARY KEY,
    actor_user_id TEXT REFERENCES users(user_id),
    event_kind TEXT NOT NULL,
    target_kind TEXT NOT NULL,
    target_id TEXT,
    before_json TEXT,
    after_json TEXT,
    created_at TEXT NOT NULL
);

CREATE TABLE networks (
    network_key TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    genesis_hash TEXT NOT NULL,
    chain_id INTEGER NOT NULL CHECK (chain_id >= 0),
    p2p_network_id INTEGER NOT NULL CHECK (p2p_network_id >= 0),
    address_hrp TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE agents (
    agent_id TEXT PRIMARY KEY,
    agent_epoch INTEGER NOT NULL CHECK (agent_epoch >= 0),
    active_boot_id TEXT,
    last_report_sequence INTEGER CHECK (last_report_sequence IS NULL OR last_report_sequence > 0),
    last_received_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE agent_credentials (
    credential_id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL REFERENCES agents(agent_id),
    credential_digest BLOB NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    revoked_at TEXT
);

CREATE TABLE nodes (
    node_id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL REFERENCES agents(agent_id),
    network_key TEXT NOT NULL REFERENCES networks(network_key),
    display_name TEXT,
    rpc_endpoint TEXT NOT NULL,
    lifecycle TEXT NOT NULL CHECK (lifecycle IN ('active', 'retired')),
    visibility TEXT NOT NULL DEFAULT 'private' CHECK (visibility IN ('private', 'public')),
    inventory_revision INTEGER NOT NULL CHECK (inventory_revision > 0),
    first_seen_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (agent_id, node_id)
);

CREATE INDEX sessions_user_idx ON sessions (user_id, revoked_at, expires_at);
CREATE INDEX audit_events_created_idx ON audit_events (created_at, audit_event_id);
CREATE INDEX nodes_agent_idx ON nodes (agent_id, lifecycle);
CREATE INDEX nodes_network_visibility_idx ON nodes (network_key, visibility, lifecycle);
