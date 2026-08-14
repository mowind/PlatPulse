#!/usr/bin/env bash
# Boot a development-mode platpulse-server with a fresh state directory and
# one Owner plus one Viewer for the Playwright suite. The suite runs against
# the real Server serving the production WebUI build, so login and the
# Home/Admin shells are exercised end to end (design §12.1, §12.2, §14.1).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WEB_DIR="$ROOT/platpulse-web"
PORT="${E2E_PORT:-4173}"
PASSWORD="${PLATPULSE_E2E_PASSWORD:-platpulse-e2e-admin-2026}"
VIEWER_PASSWORD="${PLATPULSE_E2E_VIEWER_PASSWORD:-platpulse-e2e-viewer-2026}"
STATE_DIR="$(mktemp -d /tmp/platpulse-e2e-XXXXXX)"
CONFIG="$STATE_DIR/server.toml"

# Build the production bundle the Server will host.
cd "$WEB_DIR"
npm run build >/dev/null

cat > "$CONFIG" <<EOF
state_dir = "$STATE_DIR"
db_path = "$STATE_DIR/platpulse.db"
pepper_file = "$STATE_DIR/server-pepper"
web_root = "$WEB_DIR/dist"
listen = "127.0.0.1:$PORT"
public_base_url = "http://127.0.0.1:$PORT"
development = true
EOF

cd "$ROOT"
cargo run -q -p platpulse-server -- init --config "$CONFIG" >/dev/null
printf '%s\n' "$PASSWORD" | cargo run -q -p platpulse-server -- owner create --config "$CONFIG" --username admin >/dev/null
printf '%s\n' "$VIEWER_PASSWORD" | cargo run -q -p platpulse-server -- viewer create --config "$CONFIG" --username viewer >/dev/null

# Seed two independent Nodes only for the local Playwright release-candidate
# run. Node A is explicitly public; Node B remains private and must never
# appear in the Public projection. This uses the temporary SQLite database
# created by `init`, not a production bootstrap shortcut.
python3 - "$STATE_DIR/platpulse.db" <<'PY'
import sqlite3
import sys
from datetime import datetime, timedelta, timezone

path = sys.argv[1]
now = "2026-08-12T08:00:00Z"
# Observation timestamps are relative to the real clock so the Server's
# freshness window (120s) and liveness window behave deterministically.
fresh = (datetime.now(timezone.utc) - timedelta(seconds=20)).strftime("%Y-%m-%dT%H:%M:%SZ")
agent_id = "0195f2a1-0011-4011-8011-000000000011"
network_key = "platon-e2e"
network_name = "PlatON E2E Network"
network_genesis = "0x" + "1" * 64
node_a = "0195f2a1-0014-4014-8014-000000000014"
node_b = "0195f2a1-0015-4015-8015-000000000015"
# Node C is used only by the Owner Overview mutation test, which publishes
# and then retracts it, so parallel projects never observe a mutated Node.
node_c = "0195f2a1-0016-4016-8016-000000000016"

with sqlite3.connect(path) as db:
    db.execute(
        "INSERT OR IGNORE INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES (?, 1, ?, ?)",
        (agent_id, now, now),
    )
    db.execute(
        "UPDATE agents SET last_received_at = ?, shutdown_state = 'running', last_report_sequence = 42 WHERE agent_id = ?",
        (fresh, agent_id),
    )
    db.execute(
        "INSERT OR IGNORE INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES (?, ?, ?, 210425, 210425, 'lat', ?, ?)",
        (network_key, network_name, network_genesis, now, now),
    )
    for node_id, name, endpoint, visibility in (
        (node_a, "Node A", "ws://127.0.0.1:6790", "public"),
        (node_b, "Node B (private)", "ws://127.0.0.1:6791", "private"),
        (node_c, "Node C", "ws://127.0.0.1:6792", "private"),
    ):
        db.execute(
            "INSERT OR IGNORE INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES (?, ?, ?, ?, ?, 'active', ?, 1, ?, ?)",
            (node_id, agent_id, network_key, name, endpoint, visibility, now, now),
        )

    # Node A: healthy and current (RPC, sync, and consensus all ok and fresh).
    db.execute(
        "INSERT INTO current_node_chain_observations (node_id, rpc_client_version, syncing, current_block, highest_block, consensus_epoch, consensus_validator, consensus_highest_commit_block, updated_at) VALUES (?, 'platon/1.5.1', 0, 12842019, 12842019, 42, 1, 12842019, ?)",
        (node_a, fresh),
    )
    for component in ("rpc", "sync", "consensus"):
        db.execute(
            "INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, attempted_at, observed_at, received_at, state_revision, value_revision) VALUES (?, 'node', ?, ?, ?, 'ok', ?, ?, ?, 1, 1)",
            (agent_id, node_a, node_a, component, fresh, fresh, fresh),
        )
    for namespace in ("platon", "net", "admin"):
        db.execute(
            "INSERT INTO current_node_rpc_namespaces (node_id, namespace, updated_at) VALUES (?, ?, ?)",
            (node_a, namespace, fresh),
        )

    # Node B: RPC collection failed but last-good sync values remain visible
    # (the Server preserves last-good semantics; the WebUI must keep showing
    # them with the Error context).
    db.execute(
        "INSERT INTO current_node_chain_observations (node_id, rpc_client_version, syncing, current_block, highest_block, consensus_epoch, consensus_validator, consensus_highest_commit_block, updated_at) VALUES (?, 'platon/1.5.1', 0, 12842018, 12842018, 41, 1, 12842018, ?)",
        (node_b, fresh),
    )
    for component in ("rpc", "sync", "consensus"):
        if component == "rpc":
            db.execute(
                "INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, attempted_at, observed_at, received_at, state_revision, value_revision, error_code, error_message) VALUES (?, 'node', ?, ?, 'rpc', 'error', ?, ?, ?, 2, 1, 'rpc_unreachable', 'RPC probe failed')",
                (agent_id, node_b, node_b, fresh, fresh, fresh),
            )
        else:
            db.execute(
                "INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, attempted_at, observed_at, received_at, state_revision, value_revision) VALUES (?, 'node', ?, ?, ?, 'ok', ?, ?, ?, 1, 1)",
                (agent_id, node_b, node_b, component, fresh, fresh, fresh),
            )
PY

cd "$ROOT"
exec cargo run -q -p platpulse-server -- serve --config "$CONFIG"
