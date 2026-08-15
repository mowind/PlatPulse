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
BACKUP_DIR="$(mktemp -d /tmp/platpulse-e2e-backups-XXXXXX)"
CONFIG="$STATE_DIR/server.toml"

# Build the production bundle the Server will host.
cd "$WEB_DIR"
npm run build >/dev/null

cat > "$CONFIG" <<EOF
state_dir = "$STATE_DIR"
db_path = "$STATE_DIR/platpulse.db"
pepper_file = "$STATE_DIR/server-pepper"
web_root = "$WEB_DIR/dist"
# Backup artifacts for the Admin backup surface (issue #50) live in a
# dedicated directory, never inside the Server state directory (design
# §20.1).
backup_dir = "$BACKUP_DIR"
listen = "127.0.0.1:$PORT"
public_base_url = "http://127.0.0.1:$PORT"
development = true

# Notification delivery fixture (issue #49): Telegram is configured with a
# fake token file and an aggressive bounded-retry policy so the Playwright
# suite can exercise retry/dead-letter deterministically without touching a
# real provider. The token content never enters the Server database or the
# WebUI; only the redacted destination and the secret file base name are
# exposed.
[notifications.telegram]
enabled = true
token_file = "$STATE_DIR/telegram-token"
chat_id = "987654321"
max_attempts = 2
retry_base_seconds = 1
EOF

printf '%s\n' 'fake-e2e-telegram-token' > "$STATE_DIR/telegram-token"

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

    # Notification fixtures (issue #49): durable Events with per-channel
    # Deliveries in every visible state. The delivery worker runs against
    # the fake token, so retried rows fail fast (network/API error) and
    # reach Dead letter after the configured 2 attempts; seeded terminal
    # rows stay untouched and deterministic for the suite.
    notif_time = transfer_time(0)  # a few hours ago, stable ordering
    for event_id, kind, incident, rule, subject, severity, summary, in (
        (
            "0195f2a1-0041-4041-8041-000000000041",
            "incident",
            None,
            "node.rpc_unreachable",
            ("node", node_a),
            "warning",
            "Incident opened: node.rpc_unreachable on node " + node_a,
        ),
        (
            "0195f2a1-0042-4042-8042-000000000042",
            "incident",
            None,
            "agent.offline",
            ("agent", agent_id),
            "warning",
            "Incident opened: agent.offline on agent " + agent_id,
        ),
        (
            "0195f2a1-0043-4043-8043-000000000043",
            "incident",
            None,
            "node.observation_stale",
            ("node", node_b),
            "critical",
            "Incident opened: node.observation_stale on node " + node_b,
        ),
        (
            "0195f2a1-0044-4044-8044-000000000044",
            "test",
            None,
            None,
            None,
            "info",
            "Test notification via telegram",
        ),
        (
            # Dedicated Event for the manual-retry e2e: its Delivery is
            # retried (and re-dead-lettered) without perturbing the rows
            # other assertions rely on.
            "0195f2a1-0045-4045-8045-000000000045",
            "incident",
            None,
            "node.process_not_running",
            ("node", node_b),
            "critical",
            "Incident opened: node.process_not_running on node " + node_b,
        ),
    ):
        db.execute(
            "INSERT OR IGNORE INTO notification_events (event_id, event_kind, incident_id, rule_key, subject_kind, subject_key, severity, summary, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            (event_id, kind, incident, rule, subject[0] if subject else None, subject[1] if subject else None, severity, summary, notif_time),
        )

    # Dead letter: exhausted bounded retries (2/2) with a redacted provider
    # result. Manual retry re-arms this row for one more attempt.
    db.execute(
        "INSERT OR IGNORE INTO notification_deliveries (delivery_id, event_id, channel_kind, destination, state, attempt_count, next_attempt_at, last_attempt_at, last_result, last_error_kind, retry_after_seconds, created_at, updated_at) VALUES (?, ?, 'telegram', '****4321', 'dead_letter', 2, NULL, ?, 'telegram_network_error', 'network', NULL, ?, ?)",
        ("0195f2a1-0051-4051-8051-000000000051", "0195f2a1-0041-4041-8041-000000000041", notif_time, notif_time, notif_time),
    )
    db.execute(
        "INSERT OR IGNORE INTO delivery_attempts (attempt_id, delivery_id, attempt_number, attempted_at, outcome, provider_result, error_kind, duration_ms, retry_after_seconds) VALUES ('0195f2a1-0061-4061-8061-000000000061', ?, 1, ?, 'failed', 'telegram_network_error', 'network', 800, NULL)",
        ("0195f2a1-0051-4051-8051-000000000051", notif_time),
    )
    db.execute(
        "INSERT OR IGNORE INTO delivery_attempts (attempt_id, delivery_id, attempt_number, attempted_at, outcome, provider_result, error_kind, duration_ms, retry_after_seconds) VALUES ('0195f2a1-0062-4062-8062-000000000062', ?, 2, ?, 'failed', 'telegram_api_error 429', 'telegram_api', 210, 5)",
        ("0195f2a1-0051-4051-8051-000000000051", notif_time),
    )
    # Delivered: one successful destination; failed neighbors never erase it.
    db.execute(
        "INSERT OR IGNORE INTO notification_deliveries (delivery_id, event_id, channel_kind, destination, state, attempt_count, next_attempt_at, last_attempt_at, last_result, last_error_kind, retry_after_seconds, created_at, updated_at) VALUES (?, ?, 'telegram', '****4321', 'succeeded', 1, NULL, ?, 'ok', NULL, NULL, ?, ?)",
        ("0195f2a1-0052-4052-8052-000000000052", "0195f2a1-0042-4042-8042-000000000042", notif_time, notif_time, notif_time),
    )
    db.execute(
        "INSERT OR IGNORE INTO delivery_attempts (attempt_id, delivery_id, attempt_number, attempted_at, outcome, provider_result, error_kind, duration_ms, retry_after_seconds) VALUES ('0195f2a1-0063-4063-8063-000000000063', ?, 1, ?, 'succeeded', 'ok', NULL, 350, NULL)",
        ("0195f2a1-0052-4052-8052-000000000052", notif_time),
    )
    # Suppressed: a Silence matched at Event creation; not retryable.
    db.execute(
        "INSERT OR IGNORE INTO notification_deliveries (delivery_id, event_id, channel_kind, destination, state, attempt_count, next_attempt_at, last_attempt_at, last_result, last_error_kind, retry_after_seconds, created_at, updated_at) VALUES (?, ?, 'telegram', '****4321', 'suppressed', 0, NULL, NULL, 'suppressed_by_silence:0195f2a1-0071-4071-8071-000000000071', NULL, NULL, ?, ?)",
        ("0195f2a1-0053-4053-8053-000000000053", "0195f2a1-0043-4043-8043-000000000043", notif_time, notif_time),
    )
    # Failed test: the seeded test Event was sent once and failed.
    db.execute(
        "INSERT OR IGNORE INTO notification_deliveries (delivery_id, event_id, channel_kind, destination, state, attempt_count, next_attempt_at, last_attempt_at, last_result, last_error_kind, retry_after_seconds, created_at, updated_at) VALUES (?, ?, 'telegram', '****4321', 'failed', 1, NULL, ?, 'telegram_api_error 401', 'telegram_api', NULL, ?, ?)",
        ("0195f2a1-0054-4054-8054-000000000054", "0195f2a1-0044-4044-8044-000000000044", notif_time, notif_time, notif_time),
    )
    # Manual-retry target: exhausted Dead letter, retried by the e2e flow.
    db.execute(
        "INSERT OR IGNORE INTO notification_deliveries (delivery_id, event_id, channel_kind, destination, state, attempt_count, next_attempt_at, last_attempt_at, last_result, last_error_kind, retry_after_seconds, created_at, updated_at) VALUES (?, ?, 'telegram', '****4321', 'dead_letter', 2, NULL, ?, 'telegram_network_error', 'network', NULL, ?, ?)",
        ("0195f2a1-0055-4055-8055-000000000055", "0195f2a1-0045-4045-8045-000000000045", notif_time, notif_time, notif_time),
    )
    db.execute(
        "INSERT OR IGNORE INTO delivery_attempts (attempt_id, delivery_id, attempt_number, attempted_at, outcome, provider_result, error_kind, duration_ms, retry_after_seconds) VALUES ('0195f2a1-0064-4064-8064-000000000064', ?, 1, ?, 'failed', 'telegram_network_error', 'network', 900, NULL)",
        ("0195f2a1-0055-4055-8055-000000000055", notif_time),
    )
    db.execute(
        "INSERT OR IGNORE INTO delivery_attempts (attempt_id, delivery_id, attempt_number, attempted_at, outcome, provider_result, error_kind, duration_ms, retry_after_seconds) VALUES ('0195f2a1-0065-4065-8065-000000000065', ?, 2, ?, 'failed', 'telegram_api_error 400', 'telegram_api', 150, NULL)",
        ("0195f2a1-0055-4055-8055-000000000055", notif_time),
    )

    # Retention fixture (issue #50): 2000 raw Block Summaries for Node A
    # older than every retention policy (400 days) so the bounded run
    # takes ~16 batches at 128 rows each and the suite can observe a
    # Running Operation and cancel it deterministically. One fresh row
    # stays inside every policy window and must survive the run.
    old_block_time = (datetime.now(timezone.utc) - timedelta(days=400)).strftime("%Y-%m-%dT%H:%M:%SZ")
    fresh_block_time = (datetime.now(timezone.utc) - timedelta(minutes=5)).strftime("%Y-%m-%dT%H:%M:%SZ")
    coinbase = "0x0000000000000000000000000000000000000000"
    db.execute(
        "INSERT INTO block_history_state (node_id, historical_high_watermark, cumulative_block_count, cumulative_transaction_count, cumulative_self_seal_count, updated_at) VALUES (?, 3000, 2002, 4004, 0, ?)",
        (node_a, now),
    )
    db.execute(
        "INSERT INTO block_coverage_intervals (node_id, first_height, last_height, status, created_at, updated_at) VALUES (?, 0, 3000, 'covered', ?, ?)",
        (node_a, old_block_time, old_block_time),
    )
    db.executemany(
        "INSERT OR IGNORE INTO block_summaries (node_id, block_number, block_hash, parent_hash, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, block_timestamp_ms, observed_at, transaction_count, source, coinbase, seal_signer_match, protocol_proposer_kind, attribution_reason, accepted_at) VALUES (?, ?, ?, ?, ?, 210425, 210425, 'lat', ?, ?, 2, 'subscription', ?, 'unknown', 'unknown', 'test', ?)",
        [
            (
                node_a,
                height,
                "0x" + format(height, "064x"),
                ("0x" + format(height - 1, "064x")) if height > 0 else "0x" + "0" * 64,
                network_genesis,
                height,
                old_block_time,
                coinbase,
                old_block_time,
            )
            for height in range(0, 2000)
        ],
    )
    db.execute(
        "INSERT OR IGNORE INTO block_summaries (node_id, block_number, block_hash, parent_hash, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, block_timestamp_ms, observed_at, transaction_count, source, coinbase, seal_signer_match, protocol_proposer_kind, attribution_reason, accepted_at) VALUES (?, 3000, ?, ?, ?, 210425, 210425, 'lat', 3000, ?, 2, 'subscription', ?, 'unknown', 'unknown', 'test', ?)",
        (node_a, "0x" + format(3000, "064x"), "0x" + format(2999, "064x"), network_genesis, fresh_block_time, coinbase, fresh_block_time),
    )
PY

# Keep Node A's seeded observations fresh for the whole suite: the
# Server's freshness window is 120 seconds and the full Playwright run is
# longer, so the healthy/current assertions on Node A would age out
# mid-run without a refresher. This mirrors the existing seeding pattern
# (the harness already provisions fixtures directly in SQLite); Node B
# stays deliberately stale. The refresher is bounded to 20 minutes and
# self-terminates when the suite's temporary state directory is gone.
timeout 1200 python3 - "$STATE_DIR/platpulse.db" > /dev/null 2>&1 <<'REFRESH' &
import sqlite3
import sys
import time
from datetime import datetime, timedelta, timezone

path = sys.argv[1]
node_a = "0195f2a1-0014-4014-8014-000000000014"
while True:
    time.sleep(45)
    fresh = (datetime.now(timezone.utc) - timedelta(seconds=20)).strftime("%Y-%m-%dT%H:%M:%SZ")
    try:
        with sqlite3.connect(path, timeout=5) as db:
            db.execute(
                "UPDATE component_status SET attempted_at = ?, observed_at = ?, received_at = ? WHERE node_id = ?",
                (fresh, fresh, fresh, node_a),
            )
    except Exception:
        break
REFRESH

cd "$ROOT"
# The e2e suite needs the backup directory to exercise failure
# preservation (tampering with an artifact must fail verification without
# deleting the artifact). The marker lives in /tmp because Playwright
# clears its test-results output directory after the server starts.
printf '%s\n' "$BACKUP_DIR" > /tmp/platpulse-e2e-backup-dir

exec cargo run -q -p platpulse-server -- serve --config "$CONFIG"
