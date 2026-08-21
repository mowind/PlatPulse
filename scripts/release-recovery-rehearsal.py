#!/usr/bin/env python3
"""Black-box migration and backup/restore rehearsal for packaged Server releases."""
from __future__ import annotations
import argparse
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import stat
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parent.parent
MIGRATIONS = ROOT / "crates/platpulse-server/migrations"
NOW = (dt.datetime.now(dt.timezone.utc) - dt.timedelta(hours=2)).replace(microsecond=0).isoformat().replace("+00:00", "Z")
AGENT = "0195f2a1-0001-4001-8001-000000000001"
NODE_A = "0195f2a1-0002-4002-8002-000000000002"
NODE_B = "0195f2a1-0003-4003-8003-000000000003"
NETWORK = "recovery-network"

class RehearsalError(RuntimeError):
    pass

def migration_files():
    return [(int(p.name.split("_", 1)[0]), p) for p in sorted(MIGRATIONS.glob("*.sql"))]

CURRENT = migration_files()[-1][0]
CHECKPOINTS = (1, 9, 23, 29, 35, 36, CURRENT)

def digest(path, algorithm):
    h = hashlib.new(algorithm)
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(65536), b""):
            h.update(chunk)
    return h.digest()

def exists(db, table):
    return db.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?", (table,)).fetchone() is not None

def put(db, table, values):
    if not exists(db, table):
        return
    available = {row[1] for row in db.execute(f"PRAGMA table_info({table})")}
    values = {key: value for key, value in values.items() if key in available}
    columns = ",".join(values)
    marks = ",".join("?" for _ in values)
    db.execute(f"INSERT OR IGNORE INTO {table} ({columns}) VALUES ({marks})", tuple(values.values()))

def seed(db, version):
    put(db, "server_settings", {"setting_key":"fixture", "setting_value":"recovery", "updated_at":NOW})
    put(db, "users", {"user_id":"user-owner", "username":"fixture-owner", "role":"owner", "password_hash":"fixture-hash", "created_at":NOW, "updated_at":NOW})
    put(db, "users", {"user_id":"user-viewer", "username":"fixture-viewer", "role":"viewer", "password_hash":"fixture-hash", "created_at":NOW, "updated_at":NOW})
    put(db, "sessions", {"session_id":"session-owner", "user_id":"user-owner", "token_digest":b"owner", "csrf_token_digest":b"csrf", "created_at":NOW, "last_seen_at":NOW, "expires_at":"2100-01-01T00:00:00Z"})
    put(db, "audit_events", {"actor_user_id":"user-owner", "event_kind":"fixture_seeded", "target_kind":"server", "target_id":None, "before_json":None, "after_json":"{}", "created_at":NOW})
    put(db, "networks", {"network_key":NETWORK, "display_name":"Recovery Network", "genesis_hash":"0x"+"a"*64, "chain_id":210425, "p2p_network_id":210425, "address_hrp":"lat", "created_at":NOW, "updated_at":NOW})
    put(db, "agents", {"agent_id":AGENT, "agent_epoch":7, "active_boot_id":"0195f2a1-0004-4004-8004-000000000004", "last_report_sequence":42, "last_received_at":NOW, "created_at":NOW, "updated_at":NOW})
    put(db, "agent_credentials", {"credential_id":"credential-fixture", "agent_id":AGENT, "credential_digest":b"credential", "created_at":NOW})
    for node, name in ((NODE_A, "Recovery Node A"), (NODE_B, "Recovery Node B")):
        put(db, "nodes", {"node_id":node, "agent_id":AGENT, "network_key":NETWORK, "display_name":name, "rpc_endpoint":"ws://127.0.0.1:6790", "lifecycle":"active", "visibility":"public" if node == NODE_A else "private", "inventory_revision":42, "first_seen_at":NOW, "updated_at":NOW})
    if version >= 2:
        put(db, "agent_report_receipts", {"report_id":"report-fixture", "agent_id":AGENT, "agent_epoch":7, "boot_id":"0195f2a1-0004-4004-8004-000000000004", "report_sequence":42, "report_body_sha256":"b"*64, "disposition":"accepted", "receipt_body":b"{}", "received_at":NOW})
        put(db, "current_host_observations", {"agent_id":AGENT, "cpu_percent":12.5, "memory_total_bytes":1000, "memory_used_bytes":500, "load1":0.2, "load5":0.2, "load15":0.2, "network_rx_bytes_per_sec":10, "network_tx_bytes_per_sec":20, "clock_skew_ms":2, "spool_queued_bytes":0, "spool_queued_reports":0, "spool_oldest_queued_age_ms":0, "spool_dropped_reports":0, "spool_dropped_samples":0, "updated_at":NOW})
        for node, block in ((NODE_A, 120), (NODE_B, 121)):
            put(db, "current_node_process_observations", {"node_id":node, "pid":123, "started_at":NOW, "cpu_percent":4.0, "memory_bytes":2048, "uptime_ms":1000, "updated_at":NOW})
            put(db, "current_node_chain_observations", {"node_id":node, "rpc_client_version":"fixture/1.0", "syncing":0, "current_block":block, "highest_block":block, "consensus_epoch":8, "consensus_validator":1, "consensus_highest_commit_block":block, "network_genesis_hash":"0x"+"a"*64, "network_chain_id":210425, "network_p2p_network_id":210425, "network_address_hrp":"lat", "updated_at":NOW})
            put(db, "component_status", {"agent_id":AGENT, "scope":"node", "scope_key":node, "node_id":node, "component_key":"rpc", "state":"ok", "attempted_at":NOW, "observed_at":NOW, "received_at":NOW, "state_revision":42, "value_revision":42})
        put(db, "component_status", {"agent_id":AGENT, "scope":"host", "scope_key":"host", "node_id":None, "component_key":"cpu", "state":"ok", "attempted_at":NOW, "observed_at":NOW, "received_at":NOW, "state_revision":42, "value_revision":42})
        put(db, "block_summaries", {"node_id":NODE_A, "block_number":120, "block_hash":"0x"+"1"*64, "parent_hash":"0x"+"0"*64, "network_genesis_hash":"0x"+"a"*64, "network_chain_id":210425, "network_p2p_network_id":210425, "network_address_hrp":"lat", "block_timestamp_ms":1000, "observed_at":NOW, "transaction_count":3, "block_interval_ms":10, "source":"subscription", "coinbase":"lat1coinbase", "seal_signer_match":"unknown", "protocol_proposer_kind":"unknown", "attribution_reason":"fixture", "accepted_at":NOW})
        put(db, "block_history_state", {"node_id":NODE_A, "historical_high_watermark":120, "cumulative_block_count":1, "cumulative_transaction_count":3, "cumulative_self_seal_count":0, "updated_at":NOW})
        put(db, "block_coverage_intervals", {"node_id":NODE_A, "first_height":120, "last_height":120, "status":"covered", "created_at":NOW, "updated_at":NOW})
        put(db, "block_history_gaps", {"node_id":NODE_A, "from_height":121, "to_height":123, "kind":"fixture", "created_at":NOW, "reason":"fixture"})
    if version >= 23:
        put(db, "alert_rules", {"rule_key":"fixture_rule", "enabled":1, "severity":"warning", "version":1, "condition_json":"{}", "created_at":NOW, "updated_at":NOW})
        put(db, "alert_rule_versions", {"rule_key":"fixture_rule", "version":1, "severity":"warning", "condition_json":"{}", "created_at":NOW})
        put(db, "alert_rule_state", {"rule_key":"fixture_rule", "subject_kind":"node", "subject_key":NODE_A, "state":"normal", "since":NOW, "input_kind":"known", "input_value":1, "last_evaluated_at":NOW})
        put(db, "alert_incidents", {"incident_id":"incident-fixture", "rule_key":"fixture_rule", "rule_version":1, "subject_kind":"node", "subject_key":NODE_A, "severity":"warning", "state":"resolved", "sequence":1, "opened_at":NOW, "resolved_at":NOW, "opened_evidence_json":"{}", "resolved_evidence_json":"{}"})
        put(db, "notification_events", {"event_id":"event-fixture", "event_kind":"incident", "incident_id":"incident-fixture", "rule_key":"fixture_rule", "subject_kind":"node", "subject_key":NODE_A, "severity":"warning", "summary":"fixture notification", "created_at":NOW})
        put(db, "notification_deliveries", {"delivery_id":"delivery-fixture", "event_id":"event-fixture", "channel_kind":"telegram", "destination":"fixture", "state":"succeeded", "attempt_count":1, "last_attempt_at":NOW, "last_result":"ok", "created_at":NOW, "updated_at":NOW})
        put(db, "operations", {"operation_id":"operation-fixture", "kind":"backup_create", "status":"succeeded", "progress_percent":100, "progress_label":"Fixture", "params_json":"{}", "warnings_json":"[]", "errors_json":"[]", "result_json":"{}", "created_by_user_id":"user-owner", "created_at":NOW, "started_at":NOW, "finished_at":NOW})
    if version >= 29:
        put(db, "current_node_peers", {"node_id":NODE_A, "peer_id":"peer-fixture", "remote_ip":"1.1.1.1", "direction":"outbound", "trusted":1, "static_peer":0, "consensus_peer":1, "client_name":"fixture", "updated_at":NOW})
        put(db, "peer_presence_intervals", {"node_id":NODE_A, "peer_id":"peer-fixture", "direction":"outbound", "trusted":1, "static_peer":0, "consensus_peer":1, "client_name":"fixture", "opened_at":NOW})
        put(db, "geo_location_cache", {"canonical_ip":"1.1.1.1", "country_code":"ZZ", "created_at":NOW, "last_lookup_at":NOW, "last_referenced_at":NOW, "expires_at":"2100-01-01T00:00:00Z"})
    if version >= 30:
        put(db, "validators", {"validator_id":"validator-fixture", "network_key":NETWORK, "validator_node_id":"validator-node-fixture", "display_name":"Fixture Validator", "created_at":NOW, "updated_at":NOW})
        put(db, "node_validator_links", {"link_id":"link-fixture", "node_id":NODE_A, "validator_id":"validator-fixture", "role":"primary", "valid_from":NOW, "created_at":NOW, "updated_at":NOW})
        put(db, "current_validator_insights", {"validator_id":"validator-fixture", "source":"fixture", "outcome":"success", "diagnostic":"ok", "provider_timestamp":NOW, "last_attempt_received_at":NOW, "last_good_received_at":NOW, "last_good_provider_timestamp":NOW, "rank":1, "stake_amount":"100", "reward_amount":"2", "reward_rate":"0.02", "delegator_count":3, "epoch":8, "block_count":120, "updated_at":NOW})
        put(db, "validator_ranking_history", {"history_id":"ranking-fixture", "validator_id":"validator-fixture", "previous_rank":2, "current_rank":1, "observed_at":NOW, "provider_timestamp":NOW, "observation_key":"fixture-1"})
        put(db, "validator_counter_history", {"history_id":"counter-fixture", "validator_id":"validator-fixture", "counter_name":"block_count", "previous_value":"119", "current_value":"120", "observed_at":NOW, "provider_timestamp":NOW, "observation_key":"fixture-1"})
        put(db, "validator_daily_snapshots", {"snapshot_id":"daily-fixture", "validator_id":"validator-fixture", "timezone":"UTC", "local_date":"2099-01-01", "month_key":"2099-01", "sample_at":NOW, "received_at":NOW, "provider_timestamp":NOW, "source":"fixture", "observation_key":"fixture-1", "rank":1, "stake_amount":"100", "reward_amount":"2", "reward_rate":"0.02", "delegator_count":3, "epoch":8, "block_count":120})
        put(db, "validator_monthly_aggregates", {"aggregate_id":"monthly-fixture", "validator_id":"validator-fixture", "timezone":"UTC", "month_key":"2099-01", "snapshot_count":1, "first_sample_at":NOW, "last_sample_at":NOW, "rank_min":1, "rank_max":1, "rank_last":1, "stake_last":"100", "reward_last":"2", "reward_rate_last":"0.02", "delegator_count_last":3, "epoch_last":8, "block_count_last":120, "updated_at":NOW})

def make_fixture(path, maximum):
    db = sqlite3.connect(path)
    db.execute("PRAGMA foreign_keys=OFF")
    db.execute("CREATE TABLE IF NOT EXISTS _sqlx_migrations (version BIGINT PRIMARY KEY, description TEXT NOT NULL, installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP, success BOOLEAN NOT NULL, checksum BLOB NOT NULL, execution_time BIGINT NOT NULL)")
    for version, migration in migration_files():
        if version > maximum:
            break
        db.executescript(migration.read_text(encoding="utf-8"))
        db.execute("INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) VALUES (?, ?, 1, ?, 0)", (version, migration.stem.split("_", 1)[1].replace("_", " "), digest(migration, "sha384")))
    seed(db, maximum)
    db.commit()
    db.close()
    os.chmod(path, 0o600)

def run(args, check=True, timeout=1200):
    result = subprocess.run(args, cwd=ROOT, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=timeout)
    if check and result.returncode:
        raise RehearsalError("command failed: " + (result.stderr or result.stdout).strip()[:500])
    return result

def value(path, query, params=()):
    with sqlite3.connect(path) as db:
        return db.execute(query, params).fetchone()[0]

def assert_private_regular(path):
    metadata = path.stat()
    if not stat.S_ISREG(metadata.st_mode) or stat.S_IMODE(metadata.st_mode) != 0o600 or metadata.st_uid != os.getuid():
        raise RehearsalError(f"{path.name} is not a same-user mode-0600 regular file")

def port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]

def start(server, config, log):
    process = subprocess.Popen([str(server), "serve", "--config", str(config)], stdout=log.open("w"), stderr=subprocess.STDOUT, text=True)
    listen = int(config.read_text().split("listen = ")[1].split(":")[-1].split('"')[0])
    import urllib.request
    for _ in range(120):
        if process.poll() is not None:
            raise RehearsalError("Server exited during startup")
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{listen}/health/live", timeout=1) as response:
                if response.status == 200:
                    return process
        except Exception:
            time.sleep(0.1)
    process.terminate()
    process.wait(timeout=10)
    raise RehearsalError("Server did not become live")

def stop(process):
    if process.poll() is None:
        process.send_signal(signal.SIGTERM)
        try:
            process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)

def config(state, listen, web_root):
    return f'''state_dir = "{state}"\ndb_path = "{state}/server.db"\npepper_file = "{state}/pepper"\nbackup_dir = "{state}/backups"\nweb_root = "{web_root}"\nlisten = "127.0.0.1:{listen}"\npublic_base_url = "http://127.0.0.1:{listen}"\ndevelopment = true\n'''

def config_port(config_path):
    return int(config_path.read_text().split("listen = ")[1].split(":")[-1].split('"')[0])

def check_fixture(path, checkpoint):
    for table in ("users", "sessions", "audit_events", "agents", "nodes"):
        if value(path, f"SELECT COUNT(*) FROM {table}") < 1:
            raise RehearsalError(f"schema {checkpoint} lost {table}")
    if value(path, "SELECT agent_epoch FROM agents WHERE agent_id=?", (AGENT,)) != 7:
        raise RehearsalError("Agent Epoch was not preserved")
    if value(path, "SELECT COUNT(*) FROM nodes WHERE node_id IN (?, ?)", (NODE_A, NODE_B)) != 2:
        raise RehearsalError("Node identity was not preserved")
    if checkpoint >= 2:
        checks = (
            ("agent_report_receipts", "report receipt"),
            ("current_host_observations", "current Host projection"),
            ("current_node_chain_observations", "current Node projection"),
            ("block_summaries", "block history"),
            ("block_history_state", "block high-water state"),
            ("block_coverage_intervals", "block coverage"),
            ("block_history_gaps", "block gaps"),
        )
        for table, label in checks:
            if value(path, f"SELECT COUNT(*) FROM {table}") < 1:
                raise RehearsalError(f"{label} was not preserved")
        if value(path, "SELECT historical_high_watermark FROM block_history_state WHERE node_id=?", (NODE_A,)) != 120:
            raise RehearsalError("block high-watermark changed")
        if value(path, "SELECT first_height FROM block_coverage_intervals WHERE node_id=?", (NODE_A,)) != 120:
            raise RehearsalError("block coverage changed")
    if checkpoint >= 23:
        for table, label in (("alert_rules", "Alert rules"), ("notification_events", "Notification events"), ("notification_deliveries", "Notification deliveries"), ("operations", "Operations")):
            if value(path, f"SELECT COUNT(*) FROM {table}") < 1:
                raise RehearsalError(f"{label} were not preserved")
    if checkpoint >= 29:
        for table, label in (("current_node_peers", "Peer projection"), ("peer_presence_intervals", "Peer history"), ("geo_location_cache", "Geo cache")):
            if value(path, f"SELECT COUNT(*) FROM {table}") < 1:
                raise RehearsalError(f"{label} was not preserved")
    if checkpoint >= 30:
        for table, label in (("validators", "Validators"), ("current_validator_insights", "Validator insights"), ("validator_ranking_history", "Validator ranking history")):
            if value(path, f"SELECT COUNT(*) FROM {table}") < 1:
                raise RehearsalError(f"{label} was not preserved")
    if checkpoint >= 37:
        if value(path, "SELECT setting_value FROM server_settings WHERE setting_key='site_access_mode'") not in ("public", "private"):
            raise RehearsalError("Site Access Mode was not preserved")
        if value(path, "SELECT setting_value FROM server_settings WHERE setting_key='authorization_generation'") != "0":
            raise RehearsalError("authorization generation was not preserved")

def self_test():
    with tempfile.TemporaryDirectory(prefix="platpulse-recovery-") as root:
        for checkpoint in CHECKPOINTS:
            path = Path(root) / f"fixture-{checkpoint}.db"
            make_fixture(path, checkpoint)
            if value(path, "SELECT MAX(version) FROM _sqlx_migrations") != checkpoint:
                raise RehearsalError(f"fixture {checkpoint} has wrong schema")
    print("Release recovery fixture self-test: PASS")

def rehearsal(server, output, skip_package):
    output.mkdir(parents=True, exist_ok=True)
    if not skip_package:
        package = output / "package"
        run([str(ROOT / "scripts/package-release.sh"), str(package)])
        archive = next((package / "release-set").glob("platpulse-server-*.tar.gz"), None)
        if archive is None:
            raise RehearsalError("packaged Server archive is missing")
        extracted = package / "extracted-server"
        extracted.mkdir(exist_ok=True)
        run(["tar", "-xzf", str(archive), "-C", str(extracted)])
        server = extracted / "usr/bin/platpulse-server"
    if not server or not server.is_file():
        raise RehearsalError("packaged Server is missing")
    web_root = server.parents[2] / "usr/share/platpulse/web"
    evidence = {"started_at":dt.datetime.now(dt.timezone.utc).isoformat(), "scenarios":[]}
    fixtures = output / "fixtures"
    fixtures.mkdir(exist_ok=True)
    for checkpoint in CHECKPOINTS:
        fixture = fixtures / f"schema-{checkpoint:04d}.db"
        make_fixture(fixture, checkpoint)
        state = output / f"state-{checkpoint}"
        state.mkdir(mode=0o700, exist_ok=True)
        shutil.copy2(fixture, state / "server.db")
        os.chmod(state / "server.db", 0o600)
        assert_private_regular(state / "server.db")
        (state / "pepper").write_bytes(b"0" * 64)
        os.chmod(state / "pepper", 0o600)
        assert_private_regular(state / "pepper")
        (state / "backups").mkdir(mode=0o700, exist_ok=True)
        cfg = state / "server.toml"
        cfg.write_text(config(state, port(), web_root))
        process = start(server, cfg, state / "server.log")
        stop(process)
        if value(state / "server.db", "SELECT MAX(version) FROM _sqlx_migrations") != CURRENT:
            raise RehearsalError(f"schema {checkpoint} did not migrate to {CURRENT}")
        check_fixture(state / "server.db", checkpoint)
        evidence["scenarios"].append({"name":f"forward-migration-{checkpoint}", "status":"PASS", "detail":"migrated and preserved representative data"})

    state = output / f"state-{CURRENT}"
    cfg = state / "server.toml"
    db = state / "server.db"
    pepper = (state / "pepper").read_bytes()
    backup_result = run([str(server), "backup", "--config", str(cfg)])
    filename = backup_result.stdout.strip().split("'")[1]
    artifact = state / "backups" / filename
    assert_private_regular(artifact)
    artifact_id = value(db, "SELECT artifact_id FROM backup_artifacts WHERE filename=?", (filename,))
    evidence["scenarios"].append({"name":"online-backup", "status":"PASS", "detail":"created a private backup artifact with a registry manifest"})

    before_fixture = value(db, "SELECT setting_value FROM server_settings WHERE setting_key='fixture'")
    before_nodes = value(db, "SELECT COUNT(*) FROM nodes")
    artifact.write_bytes(artifact.read_bytes() + b"tampered")
    failed = run([str(server), "restore", "--config", str(cfg), "--artifact-id", artifact_id, "--yes"], check=False)
    if failed.returncode == 0 or value(db, "SELECT setting_value FROM server_settings WHERE setting_key='fixture'") != before_fixture or value(db, "SELECT COUNT(*) FROM nodes") != before_nodes or not artifact.is_file():
        raise RehearsalError("checksum failure changed current data or removed artifact")
    evidence["scenarios"].append({"name":"checksum-invalid-restore", "status":"PASS", "detail":"failed without replacing database or deleting artifact"})

    run([str(server), "backup", "--config", str(cfg)])
    good_id = value(db, "SELECT artifact_id FROM backup_artifacts WHERE verification IN ('pending', 'ok') ORDER BY rowid DESC LIMIT 1")
    with sqlite3.connect(db) as connection:
        connection.execute("INSERT OR REPLACE INTO server_settings VALUES ('restore-marker', 'present', ?)", (NOW,))
    live = start(server, cfg, state / "running-restore.log")
    refused = run([str(server), "restore", "--config", str(cfg), "--artifact-id", good_id, "--yes"], check=False)
    stop(live)
    if refused.returncode == 0 or value(db, "SELECT setting_value FROM server_settings WHERE setting_key='restore-marker'") != "present":
        raise RehearsalError("running restore was not refused safely")
    evidence["scenarios"].append({"name":"restore-requires-stopped-server", "status":"PASS", "detail":"running Server refused destructive restore"})

    run([str(server), "restore", "--config", str(cfg), "--artifact-id", good_id, "--yes"])
    if value(db, "SELECT COUNT(*) FROM server_settings WHERE setting_key='restore-marker'") != 0:
        raise RehearsalError("successful restore did not replace database")
    safety_copies = list(state.glob("server.db.restore-safety-*"))
    if not safety_copies or (state / "pepper").read_bytes() != pepper:
        raise RehearsalError("restore did not preserve safety copy and secret")
    assert_private_regular(db)
    for safety_copy in safety_copies:
        assert_private_regular(safety_copy)
    restored_at = value(db, "SELECT updated_at FROM current_host_observations WHERE agent_id=?", (AGENT,))
    restored_time = dt.datetime.fromisoformat(restored_at.replace("Z", "+00:00"))
    if restored_time > dt.datetime.now(dt.timezone.utc) - dt.timedelta(minutes=30):
        raise RehearsalError("restored current observations were unexpectedly refreshed")
    if value(db, "SELECT agent_epoch FROM agents WHERE agent_id=?", (AGENT,)) != 7 or value(db, "SELECT COUNT(*) FROM agent_report_receipts") < 1 or value(db, "SELECT COUNT(*) FROM nodes WHERE node_id IN (?, ?)", (NODE_A, NODE_B)) != 2:
        raise RehearsalError("restore reset identity or receipt state")
    evidence["scenarios"].append({"name":"offline-restore", "status":"PASS", "detail":"atomic replacement preserved safety copy, secret file, identity, receipts, and historical observation timestamps"})
    restarted = start(server, cfg, state / "post-restore.log")
    try:
        import urllib.request
        with urllib.request.urlopen(f"http://127.0.0.1:{config_port(cfg)}/health/ready", timeout=5) as response:
            if response.status != 200:
                raise RehearsalError("post-restore readiness was not healthy")
    finally:
        stop(restarted)
    evidence["scenarios"].append({"name":"post-restore-readiness-and-staleness", "status":"PASS", "detail":"restart reported ready while restored observation timestamps remained naturally stale"})

    corrupt = output / "corrupt.db"
    corrupt.write_bytes(b"not sqlite")
    os.chmod(corrupt, 0o600)
    corrupt_before = corrupt.read_bytes()
    corrupt_state = output / "corrupt-state"
    corrupt_state.mkdir(mode=0o700, exist_ok=True)
    (corrupt_state / "pepper").write_bytes(b"0" * 64)
    os.chmod(corrupt_state / "pepper", 0o600)
    corrupt_cfg = output / "corrupt.toml"
    corrupt_cfg.write_text(f'state_dir = "{corrupt_state}"\ndb_path = "{corrupt}"\npepper_file = "{corrupt_state}/pepper"\n')
    if run([str(server), "serve", "--config", str(corrupt_cfg)], check=False).returncode == 0 or corrupt.read_bytes() != corrupt_before:
        raise RehearsalError("corrupt database was accepted or replaced")
    evidence["scenarios"].append({"name":"corrupt-database", "status":"PASS", "detail":"startup refused corrupt input"})

    higher = output / "higher.db"
    shutil.copy2(db, higher)
    higher_before = higher.read_bytes()
    with sqlite3.connect(higher) as connection:
        connection.execute("INSERT INTO _sqlx_migrations VALUES (999, 'future schema', CURRENT_TIMESTAMP, 1, zeroblob(48), 0)")
    os.chmod(higher, 0o600)
    higher_state = output / "higher-state"
    higher_state.mkdir(mode=0o700, exist_ok=True)
    (higher_state / "pepper").write_bytes(b"0" * 64)
    os.chmod(higher_state / "pepper", 0o600)
    higher_cfg = output / "higher.toml"
    higher_cfg.write_text(f'state_dir = "{higher_state}"\ndb_path = "{higher}"\npepper_file = "{higher_state}/pepper"\n')
    if run([str(server), "serve", "--config", str(higher_cfg)], check=False).returncode == 0 or higher.read_bytes() != higher_before:
        raise RehearsalError("higher schema was accepted or replaced")
    evidence["scenarios"].append({"name":"higher-schema", "status":"PASS", "detail":"unsupported future schema was refused"})
    evidence["finished_at"] = dt.datetime.now(dt.timezone.utc).isoformat()
    (output / "recovery-rehearsal.json").write_text(json.dumps(evidence, indent=2) + "\n")
    (output / "recovery-rehearsal.md").write_text("# Recovery rehearsal\n\n" + "\n".join(f"- **{item['status']}** {item['name']}: {item['detail']}" for item in evidence["scenarios"]) + "\n")
    print(f"Release recovery rehearsal: PASS ({output / 'recovery-rehearsal.json'})")

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--server", type=Path)
    parser.add_argument("--output", type=Path, default=ROOT / "target/recovery-rehearsal")
    parser.add_argument("--skip-package", action="store_true")
    args = parser.parse_args()
    try:
        if args.self_test:
            self_test()
        else:
            rehearsal(args.server, args.output, args.skip_package)
    except (OSError, sqlite3.Error, subprocess.SubprocessError, RehearsalError) as error:
        print(f"Release recovery rehearsal: FAIL: {error}", file=sys.stderr)
        return 1
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
