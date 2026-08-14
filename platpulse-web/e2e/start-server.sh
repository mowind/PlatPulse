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
# Node B's observations are older than the 120s freshness window so its
# Server-owned freshness dimension is deterministically `stale`.
stale = (datetime.now(timezone.utc) - timedelta(minutes=10)).strftime("%Y-%m-%dT%H:%M:%SZ")
agent_id = "0195f2a1-0011-4011-8011-000000000011"
network_key = "platon-e2e"
network_name = "PlatON E2E Network"
network_genesis = "0x" + "1" * 64
node_a = "0195f2a1-0014-4014-8014-000000000014"
node_b = "0195f2a1-0015-4015-8015-000000000015"
# Node C is used only by the Owner Overview mutation test, which publishes
# and then retracts it, so parallel projects never observe a mutated Node.
node_c = "0195f2a1-0016-4016-8016-000000000016"
# Node D is retired: present in an earlier Inventory, absent from the
# latest one. It keeps its identity and history but produces no live
# alerts; the Admin surface shows the reactivation guidance.
node_d = "0195f2a1-0017-4017-8017-000000000017"
# Node E is used only by the PAGE-ADMIN-NODE-VISIBILITY mutation test, so
# it never shares mutation state with the Overview test (Node C) or the
# metadata test (Node C); parallel projects never observe a mutated Node.
node_e = "0195f2a1-0018-4018-8018-000000000018"
# Node F is dedicated to PAGE-ADMIN-NODE-TRANSFER: its seeded history
# covers identity mismatch, completed, cancelled, expired, and conflict
# outcomes, and the mutation test creates + cancels one pending transfer.
node_f = "0195f2a1-0019-4019-8019-000000000019"
# Node G belongs to the target Agent after a completed Transfer, so the
# completed outcome is observable on a Node whose ownership already moved.
node_g = "0195f2a1-0020-4020-8020-000000000020"
target_agent = "0195f2a1-0021-4021-8021-000000000021"

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
        "INSERT OR IGNORE INTO agent_credentials (credential_id, agent_id, credential_digest, created_at, revoked_at, revoke_after) VALUES (?, ?, x'00', ?, NULL, NULL)",
        ("0195f2a1-0021-4021-8021-000000000021", agent_id, now),
    )
    db.execute(
        "INSERT OR IGNORE INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES (?, 1, ?, ?)",
        (target_agent, now, now),
    )
    db.execute(
        "INSERT OR IGNORE INTO agent_credentials (credential_id, agent_id, credential_digest, created_at, revoked_at, revoke_after) VALUES (?, ?, x'01', ?, NULL, NULL)",
        ("0195f2a1-0022-4022-8022-000000000022", target_agent, now),
    )
    db.execute(
        "INSERT OR IGNORE INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES (?, ?, ?, 210425, 210425, 'lat', ?, ?)",
        (network_key, network_name, network_genesis, now, now),
    )
    for node_id, name, endpoint, visibility in (
        (node_a, "Node A", "ws://127.0.0.1:6790", "public"),
        (node_b, "Node B (private)", "ws://127.0.0.1:6791", "private"),
        (node_c, "Node C", "ws://127.0.0.1:6792", "private"),
        (node_d, "Node D (retired)", "ws://127.0.0.1:6793", "private"),
        (node_e, "Node E (private)", "ws://127.0.0.1:6794", "private"),
        (node_f, "Node F (transfer)", "ws://127.0.0.1:6795", "private"),
        (node_g, "Node G (transferred)", "ws://127.0.0.1:6796", "private"),
    ):
        lifecycle = "retired" if node_id == node_d else "active"
        owner = target_agent if node_id == node_g else agent_id
        db.execute(
            "INSERT OR IGNORE INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
            (node_id, owner, network_key, name, endpoint, lifecycle, visibility, now, now),
        )

    # Node A: healthy and current (RPC, sync, and consensus all ok and fresh).
    # The observed Network identity matches the Registry tuple exactly.
    db.execute(
        "INSERT INTO current_node_chain_observations (node_id, rpc_client_version, syncing, current_block, highest_block, consensus_epoch, consensus_validator, consensus_highest_commit_block, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, updated_at) VALUES (?, 'platon/1.5.1', 0, 12842019, 12842019, 42, 1, 12842019, ?, 210425, 210425, 'lat', ?)",
        (node_a, network_genesis, fresh),
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
        "INSERT INTO current_node_chain_observations (node_id, rpc_client_version, syncing, current_block, highest_block, consensus_epoch, consensus_validator, consensus_highest_commit_block, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, updated_at) VALUES (?, 'platon/1.5.1', 0, 12842018, 12842018, 41, 1, 12842018, ?, 999999, 210425, 'lat', ?)",
        (node_b, network_genesis, stale),
    )
    for component in ("rpc", "sync", "consensus"):
        if component == "rpc":
            db.execute(
                "INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, attempted_at, observed_at, received_at, state_revision, value_revision, error_code, error_message) VALUES (?, 'node', ?, ?, 'rpc', 'error', ?, ?, ?, 2, 1, 'rpc_unreachable', 'RPC probe failed')",
                (agent_id, node_b, node_b, stale, stale, stale),
            )
        else:
            db.execute(
                "INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, attempted_at, observed_at, received_at, state_revision, value_revision) VALUES (?, 'node', ?, ?, ?, 'ok', ?, ?, ?, 1, 1)",
                (agent_id, node_b, node_b, component, stale, stale, stale),
            )

    # Transfer history (issue #46): Node F carries every terminal/conflict
    # outcome; Node G shows a completed Transfer on a Node already owned by
    # the target Agent. Expiry is Server-authoritative: the expired pending
    # row is materialized by the list route on first read.
    def transfer_time(days: int) -> str:
        return (datetime.now(timezone.utc) - timedelta(days=days)).strftime("%Y-%m-%dT%H:%M:%SZ")

    for transfer_id, node_id, status, extra in (
        (
            "0195f2a1-0031-4031-8031-000000000031",
            node_f,
            "identity_mismatch",
            (None, None, "identity_mismatch", "the target-declared Network identity contradicts the registered Network; ownership stays with the source Agent", '["genesis_hash", "address_hrp"]'),
        ),
        (
            "0195f2a1-0032-4032-8032-000000000032",
            node_g,
            "completed",
            (transfer_time(9), None, None, None, None),
        ),
        (
            "0195f2a1-0033-4033-8033-000000000033",
            node_f,
            "cancelled",
            (None, transfer_time(6), None, None, None),
        ),
        (
            "0195f2a1-0034-4034-8034-000000000034",
            node_f,
            "expired",
            (None, None, None, None, None),
        ),
        (
            "0195f2a1-0035-4035-8035-000000000035",
            node_f,
            "conflict",
            (None, None, None, None, None),
        ),
    ):
        created = transfer_time(12)
        completed_at, cancelled_at, rejection_code, rejection_reason, mismatched = extra
        # The expired row must be pending in storage with a past deadline so
        # the list route materializes it as `expired` (never auto-extends).
        status_value = "pending" if status == "expired" else status
        expires = (
            transfer_time(1)
            if status == "expired"
            else (transfer_time(4) if status == "identity_mismatch" else transfer_time(11))
        )
        db.execute(
            "INSERT OR IGNORE INTO node_transfers (transfer_id, node_id, source_agent_id, target_agent_id, status, operator_reason, created_at, expires_at, cancelled_at, completed_at, rejection_code, rejection_reason, mismatched_fields, updated_at) VALUES (?, ?, ?, ?, ?, 'move the validator host', ?, ?, ?, ?, ?, ?, ?, ?)",
            (transfer_id, node_id, agent_id, target_agent, status_value, created, expires, cancelled_at, completed_at, rejection_code, rejection_reason, mismatched, created),
        )
PY

cd "$ROOT"
exec cargo run -q -p platpulse-server -- serve --config "$CONFIG"
