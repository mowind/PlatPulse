#!/usr/bin/env python3
"""Issue #213 Story 47 baseline: a measured 24 hour raw Node metric history.

This script produces a *measured baseline*, not a production capacity promise.
Story 47 asks for a trustworthy 24 hour raw Node metric history, so every
number below is tied to

  * the exact Server binary built from this checkout,
  * the filesystem the temporary state directory actually lives on,
  * the declared Agent cadence and the declared report window, and
  * real Server rows written through the production ingestion path and read
    back through the Owner-only metric history API.

The Server is driven through its real surfaces: the production binary, the real
Owner login and Admin API, real Agent Enrollment over the Agent HTTP surface,
real Report ingestion into a temporary SQLite database, and the real low-space
protection transitions produced by rewriting the deployment policy and
restarting the process. Reports are generated from the canonical wire fixture
with a deterministic load generator seeded from --seed.

Two substitutions are declared under "not_delivered" instead of being
extrapolated into a guarantee: the report window is compressed in wall time
(backdated observation instants on real ingestions), and Agent-side collection
is not measured.

The aggregate tiers issue #214 added are exercised by the last phase of the same
run: a declared thirty day hourly history is seeded through the same ingestion
path for one fresh Node, so the one minute and five minute rows, the answers they
serve and the cleanup that releases them are measured against what the Server
counted rather than against what it can still hold.

The Host family that issue #215 made shared per Agent is measured by the last
phase of the same run: the eight quantities every Report above already carried
are audited once for the Agent, the Agent route and the Node host route of that
Agent's Nodes are compared point by point, and one Report carrying two mounts
measures what a mount path adds to the family and to its ledger.

Usage:
    python3 scripts/metric-history-baseline.py --output-root target/metric-history-baseline
"""

from __future__ import annotations

import argparse
import copy
import http.client
import json
import os
import random
import signal
import socket
import sqlite3
import subprocess
import sys
import time
import uuid
from datetime import datetime, timedelta, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / "crates/platpulse-core/tests/fixtures/report_v1_minimal.json"
DEFAULT_BINARY = ROOT / "target/debug/platpulse-server"

OWNER_USERNAME = "admin"
OWNER_PASSWORD = "platpulse-baseline-owner-2026"

# The largest floor the Server accepts: thresholds are stored in SQLite, so the
# ceiling is the signed 64 bit maximum.
MAX_PERSISTED_BYTES = 9223372036854775807

# The Network Registry tuple the e2e harness declares: a Report's Node is only
# applied once its Network exists, and the genesis hash deliberately differs
# from the one the fixture observes, proving Node acceptance does not depend on
# Registry identity agreement.
NETWORK_KEY = "platon-mainnet"
NETWORK_GENESIS = "0x0000000000000000000000000000000000000000000000000000000000000001"
CLEARED_FLOOR = 1

NODE_SERIES = (
    "process_cpu_percent",
    "process_memory_percent",
    "data_directory_percent",
    "peer_inbound_count",
    "peer_outbound_count",
)
NEVER_REPORTED_SERIES = "data_directory_percent"

# How many Reports are submitted while low-space protection is adopted. The
# silent stretch they leave must exceed the Server's own minimum silence
# threshold (3 x the observed cadence with a 120 second floor), otherwise the
# Server correctly refuses to call it a gap: six rounds at a 30 second cadence
# are 180 seconds of reported silence against that 120 second floor.
PAUSE_ROUNDS = 6

CANONICAL = "%Y-%m-%dT%H:%M:%SZ"


class BaselineError(RuntimeError):
    pass


def utc_now() -> str:
    return datetime.now(timezone.utc).strftime(CANONICAL)


def instant(seconds_from_now: float) -> str:
    moment = datetime.now(timezone.utc) + timedelta(seconds=seconds_from_now)
    return moment.strftime(CANONICAL)


def parse_instant(value: str) -> datetime:
    return datetime.strptime(value, CANONICAL).replace(tzinfo=timezone.utc)


def shift_instant(value: str, seconds: float) -> str:
    return (parse_instant(value) + timedelta(seconds=seconds)).strftime(CANONICAL)


def command(args, *, input_text: str | None = None, timeout: float = 900, cwd: Path = ROOT):
    completed = subprocess.run(
        [str(item) for item in args],
        input=input_text,
        capture_output=True,
        text=True,
        cwd=str(cwd),
        timeout=timeout,
        check=False,
    )
    if completed.returncode != 0:
        raise BaselineError(
            "command failed with status "
            + str(completed.returncode)
            + ": "
            + " ".join(str(item) for item in args)
            + "\n"
            + completed.stderr.strip()[:2000]
        )
    return completed


def free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        probe.bind(("127.0.0.1", 0))
        return int(probe.getsockname()[1])


def json_bytes(value) -> bytes:
    return json.dumps(value, separators=(",", ":"), sort_keys=True).encode("utf-8")


def percentile(values: list[float], fraction: float) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    if len(ordered) == 1:
        return ordered[0]
    position = fraction * (len(ordered) - 1)
    lower = int(position)
    upper = min(lower + 1, len(ordered) - 1)
    weight = position - lower
    return ordered[lower] * (1 - weight) + ordered[upper] * weight


def human_bytes(value) -> str:
    if value is None:
        return "unknown"
    size = float(value)
    for unit in ("B", "KiB", "MiB", "GiB", "TiB"):
        if size < 1024 or unit == "TiB":
            return "%.2f %s" % (size, unit)
        size /= 1024
    return "%.2f TiB" % size


def file_bytes(path: Path) -> int:
    total = 0
    for candidate in (path, Path(str(path) + "-wal"), Path(str(path) + "-shm")):
        if candidate.is_file():
            total += candidate.stat().st_size
    return total


def mount_conditions(path: Path) -> dict:
    resolved = str(path.resolve())
    best = None
    mounts = Path("/proc/mounts")
    if mounts.is_file():
        for line in mounts.read_text(encoding="utf-8").splitlines():
            parts = line.split()
            if len(parts) < 3:
                continue
            point, filesystem_type = parts[1], parts[2]
            if resolved == point or resolved.startswith(point.rstrip("/") + "/"):
                if best is None or len(point) > len(best[0]):
                    best = (point, filesystem_type, parts[0])
    if best is None:
        return {"mount_point": None, "filesystem_type": None, "mount_source": None}
    stat = os.statvfs(str(path))
    unit = stat.f_frsize or stat.f_bsize
    return {
        "mount_point": best[0],
        "filesystem_type": best[1],
        "mount_source": best[2],
        "total_bytes": unit * stat.f_blocks,
        "available_bytes": unit * stat.f_bavail,
    }


def hardware_conditions() -> dict:
    uname = os.uname()
    cpu_model = None
    cpuinfo = Path("/proc/cpuinfo")
    if cpuinfo.is_file():
        for line in cpuinfo.read_text(encoding="utf-8").splitlines():
            if line.lower().startswith("model name"):
                cpu_model = line.split(":", 1)[1].strip()
                break
    memory_total = None
    meminfo = Path("/proc/meminfo")
    if meminfo.is_file():
        for line in meminfo.read_text(encoding="utf-8").splitlines():
            if line.startswith("MemTotal:"):
                memory_total = int(line.split()[1]) * 1024
                break
    return {
        "uname": " ".join(uname),
        "cpu_model": cpu_model,
        "cpu_count": os.cpu_count(),
        "memory_total_bytes": memory_total,
    }


class Client:
    """Minimal HTTP client: one connection per request, so a blocked request
    can never be mistaken for Server work."""

    def __init__(self, port: int, timeout: float = 30.0):
        self.port = port
        self.timeout = timeout

    def request(self, method: str, path: str, body: bytes | None = None, headers: dict | None = None):
        started = time.perf_counter()
        connection = http.client.HTTPConnection("127.0.0.1", self.port, timeout=self.timeout)
        try:
            connection.request(method, path, body=body, headers=headers or {})
            response = connection.getresponse()
            payload = response.read()
            elapsed_ms = (time.perf_counter() - started) * 1000.0
            collected = {}
            for key, value in response.getheaders():
                collected[key.lower()] = value
            return response.status, collected, payload, elapsed_ms
        finally:
            connection.close()


class ServerProcess:
    def __init__(self, binary: Path, config: Path, work_dir: Path):
        self.binary = binary
        self.config = config
        self.work_dir = work_dir
        self.process = None
        self.log_handle = None
        self.port = None

    def start(self, port: int) -> None:
        self.port = port
        log_path = self.work_dir / ("server-" + str(port) + ".log")
        self.log_handle = open(log_path, "ab")
        self.process = subprocess.Popen(
            [str(self.binary), "serve", "--config", str(self.config)],
            stdout=self.log_handle,
            stderr=subprocess.STDOUT,
            cwd=str(ROOT),
        )
        deadline = time.time() + 60
        client = Client(port)
        while time.time() < deadline:
            if self.process.poll() is not None:
                raise BaselineError("Server exited during startup: " + str(log_path))
            try:
                status, _, _, _ = client.request("GET", "/health/ready")
                if status == 200:
                    return
            except OSError:
                pass
            time.sleep(0.2)
        raise BaselineError("Server did not become ready")

    def stop(self) -> None:
        if self.process is None:
            return
        if self.process.poll() is None:
            self.process.send_signal(signal.SIGTERM)
            try:
                self.process.wait(timeout=20)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        if self.log_handle is not None:
            self.log_handle.close()
        self.process = None
        self.log_handle = None


def write_config(path: Path, state_dir: Path, port: int, floor: int) -> None:
    path.write_text(
        "\n".join(
            [
                'state_dir = "' + str(state_dir) + '"',
                'db_path = "' + str(state_dir / "platpulse.db") + '"',
                'pepper_file = "' + str(state_dir / "server-pepper") + '"',
                'web_root = "' + str(ROOT / "platpulse-web/dist") + '"',
                'backup_dir = "' + str(state_dir / "backups") + '"',
                'listen = "127.0.0.1:' + str(port) + '"',
                'public_base_url = "http://127.0.0.1:' + str(port) + '"',
                "development = true",
                "",
                "[capacity]",
                "enabled = true",
                "pause_below_bytes = " + str(floor),
                "resume_above_bytes = " + str(floor),
                "sample_interval_seconds = 5",
                "",
            ]
        ),
        encoding="utf-8",
    )


def table_columns(db_path: Path, table: str) -> tuple:
    return tuple(row["name"] for row in sqlite_rows(db_path, "PRAGMA table_info(" + table + ")"))


def sqlite_rows(db_path: Path, sql: str) -> list:
    connection = sqlite3.connect("file:" + str(db_path) + "?mode=ro", uri=True)
    try:
        connection.row_factory = sqlite3.Row
        return [dict(row) for row in connection.execute(sql).fetchall()]
    finally:
        connection.close()


def sqlite_scalar(db_path: Path, sql: str):
    connection = sqlite3.connect("file:" + str(db_path) + "?mode=ro", uri=True)
    try:
        row = connection.execute(sql).fetchone()
        return None if row is None else row[0]
    finally:
        connection.close()


def login(client: Client) -> tuple:
    status, headers, body, _ = client.request(
        "POST",
        "/api/public/v1/login",
        body=json_bytes({"username": OWNER_USERNAME, "password": OWNER_PASSWORD}),
        headers={"Content-Type": "application/json", "Origin": "http://127.0.0.1:" + str(client.port)},
    )
    if status != 200:
        raise BaselineError("Owner login failed with status " + str(status))
    cookie = headers.get("set-cookie", "").split(";", 1)[0]
    return cookie, str(json.loads(body).get("csrfToken", ""))


def admin_get(client: Client, cookie: str, path: str):
    return client.request("GET", path, headers={"Cookie": cookie})


def history_url(node_id: str, metric: str, query: str = "") -> str:
    return "/api/admin/v1/nodes/" + node_id + "/metric-history?metric=" + metric + query


def read_history(client: Client, cookie: str, node_id: str, metric: str, query: str = ""):
    status, _, body, elapsed_ms = admin_get(client, cookie, history_url(node_id, metric, query))
    if status != 200:
        raise BaselineError(
            "GET metric-history failed with status " + str(status) + ": " + body.decode("utf-8")[:400]
        )
    return json.loads(body), elapsed_ms, len(body)


def agent_history_url(agent_id: str, metric: str, query: str = "") -> str:
    """Issue #215: the Owner-only route that serves the Host series one Agent
    collected once, for every Node on it."""
    return "/api/admin/v1/agents/" + agent_id + "/metric-history?metric=" + metric + query


def node_host_history_url(node_id: str, metric: str, query: str = "") -> str:
    """The same shared series reached through one of the Agent's Nodes."""
    return "/api/admin/v1/nodes/" + node_id + "/host-metric-history?metric=" + metric + query


def read_shared_history(client: Client, cookie: str, path: str):
    status, _, body, elapsed_ms = admin_get(client, cookie, path)
    if status != 200:
        raise BaselineError(
            "GET " + path + " failed with status " + str(status) + ": " + body.decode("utf-8")[:400]
        )
    return json.loads(body), elapsed_ms, len(body)


def refusal_code(client: Client, cookie: str, path: str) -> dict:
    """The refusal a route owes a request for the other family's series."""
    status, _, body, _ = admin_get(client, cookie, path)
    try:
        payload = json.loads(body)
    except ValueError:
        payload = {}
    error = payload.get("error") if isinstance(payload.get("error"), dict) else {}
    return {
        "status": status,
        "code": error.get("code"),
        "message": str(error.get("message") or "")[:200],
    }


def create_enrollment_token(binary: Path, config: Path) -> str:
    output = command([binary, "agent", "create-enrollment-token", "--config", config])
    tokens = [line.strip() for line in output.stdout.splitlines() if line.strip().startswith("pp_enroll_")]
    if not tokens:
        raise BaselineError("create-enrollment-token returned no token")
    return tokens[-1]


def enroll_agent(token: str, client: Client) -> dict:
    url = "http://127.0.0.1:" + str(client.port) + "/api/agent/v1/enroll"
    completed = subprocess.run(
        [
            "curl", "-sS", "--connect-timeout", "2", "--max-time", "30",
            "-H", "Authorization: Bearer " + token,
            "-X", "POST", url, "-w", "\n%{http_code}",
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    if completed.returncode != 0:
        raise BaselineError("Agent Enrollment transport failed: " + completed.stderr.strip()[:400])
    body, status_text = completed.stdout.rsplit("\n", 1)
    if int(status_text) != 200:
        raise BaselineError("Agent Enrollment failed with status " + status_text + ": " + body.strip()[:400])
    payload = json.loads(body)
    return {
        "agent_id": payload["agent_id"],
        "agent_epoch": int(payload["agent_epoch"]),
        "credential": payload["credential"],
        "boot_id": str(uuid.uuid4()),
    }


def restamp(value, observed_at: str) -> None:
    """Every observation in the fixture is stamped at the declared instant, so
    the stored sample belongs to the window under measurement."""
    if isinstance(value, dict):
        for key, item in value.items():
            if key in ("attempted_at", "latest_observed_at") and isinstance(item, str):
                value[key] = observed_at
            else:
                restamp(item, observed_at)
    elif isinstance(value, list):
        for item in value:
            restamp(item, observed_at)


def build_report(
    fixture: dict,
    agent: dict,
    sequence: int,
    observed_at: str,
    cpu: float,
    pid: int,
    report_id: str,
    memory_bytes: int | None = None,
) -> dict:
    report = copy.deepcopy(fixture)
    report["agent_id"] = agent["agent_id"]
    report["agent_epoch"] = agent["agent_epoch"]
    report["boot_id"] = agent["boot_id"]
    report["boot_transition"] = "continuing"
    report["report_sequence"] = sequence
    report["report_id"] = report_id
    report["generated_at"] = observed_at
    restamp(report, observed_at)
    node = report["nodes"][0]
    node["process"] = {
        "status": "ok",
        "attempted_at": observed_at,
        "latest_observed_at": observed_at,
        "state_revision": sequence,
        "value_revision": sequence,
        "latest": {
            "pid": pid,
            "started_at": observed_at,
            "cpu_percent": cpu,
            "memory_bytes": memory_bytes if memory_bytes is not None else 536870912 + (sequence % 97),
            "uptime_ms": 7200000 + sequence * 1000,
        },
    }
    return report


def submit_report(client: Client, credential: str, report: dict) -> dict:
    status, _, body, elapsed_ms = client.request(
        "POST",
        "/api/agent/v1/reports",
        body=json_bytes(report),
        headers={"Authorization": "Bearer " + credential, "Content-Type": "application/json"},
    )
    if status != 200:
        raise BaselineError("report submission failed with status " + str(status) + ": " + body.decode("utf-8")[:400])
    payload = json.loads(body)
    receipt = payload.get("receipt") if isinstance(payload.get("receipt"), dict) else {}
    disposition = payload.get("disposition") or receipt.get("disposition")
    return {
        "disposition": disposition,
        "reason": receipt.get("reason") or receipt.get("detail") or payload.get("reason"),
        "elapsed_ms": elapsed_ms,
    }


def report_id_for(sequence: int) -> str:
    return "0195f2a1-0092-4092-8092-" + str(sequence).zfill(12)


def check(name: str, expected, observed, ok=None) -> dict:
    """One measurement: what the plan says should happen, and what happened."""
    if ok is None:
        ok = expected == observed
    return {"name": name, "expected": expected, "observed": observed, "ok": bool(ok)}


# -- the multi-Node arrival and the per-Report cleanup budget -------------
#
# Facts under test, read from the tree when this run was written:
#   * crates/platpulse-server/src/retention.rs:185 carries
#     "const NODE_METRIC_CLEANUP_BATCH: i64 = 4096;" and the raw Node sample
#     statement at crates/platpulse-server/src/retention.rs:214 deletes at
#     most that many expired rows per call. The bound it replaced was 128, and
#     issue #217 raised it from 2048 so it still covers one maximal Report.
#   * crates/platpulse-core/src/protocol.rs:33 allows MAX_NODE_OBSERVATIONS =
#     256 Nodes in one Report, and the Server stores at most ten series per
#     Node (crates/platpulse-server/src/metric_history.rs:98), so one
#     accepted Report can add up to 2560 rows at once.
#   * cleanup runs opportunistically after every accepted Report
#     (crates/platpulse-server/src/http/report_ingestion.rs:3075) and
#     there is no periodic scheduler, so the per-Report bound has to cover a
#     whole Report worth of expiry: below the arrival rate the expired backlog
#     grows without bound and ends in the low-space protection that pauses the
#     history this ticket exists to keep.
MULTI_NODE_CLONE_COUNT = 32
MULTI_NODE_REPORTS = 48
MULTI_NODE_CADENCE_SECONDS = 2400
MULTI_NODE_NEWEST_AGE_SECONDS = 300
MULTI_NODE_CLONE_BLOCK = 0x1000
# How many Node series one Report stores for a Node that carries the fixture's
# observations. The fixture states five (rpc, sync, consensus, network_identity,
# static_metadata); issue #217 stores two more from the sync observation's chain
# heights, sync_current_block and sync_highest_block, so a clone registers seven
# (crates/platpulse-server/src/metric_history.rs NODE_METRIC_SERIES, migration
# 0070). The consensus observation is "unsupported" with no attempted_at in the
# fixture, so it stores no consensus height.
MULTI_NODE_SERIES_PER_NODE = 7
DRAIN_CLONE_COUNT = 128
DRAIN_CLONE_BLOCK = 0x1100
DRAIN_REPORTS = 12
DRAIN_PACE_SECONDS = 2.0
DRAIN_INSIDE_CLIFF_SECONDS = 8
# One anchor Report per phase keeps rows an hour inside the window, so the
# stored set is never empty and "unchanged" is a claim with content.
DRAIN_ANCHOR_AGE_SECONDS = 3600
# Inventory revisions, so the content revision of each multi-Node inventory
# moves strictly up from the fixture revision 1 the single-Node path uses.
MULTI_NODE_INVENTORY_REVISION = 1001
DRAIN_INVENTORY_REVISION = 1002
NODE_METRIC_CLEANUP_BATCH = 4096
PREVIOUS_NODE_METRIC_CLEANUP_BATCH = 128
# The declared window is the same 24 hours the raw retention floor keeps, so a
# plan that reached exactly base - window would already be outside the cliff
# when its oldest Report is written (a row is stored only while its instant is
# inside the window, crates/platpulse-server/src/metric_history.rs:524-526).
# The plan keeps this much margin inside the cliff instead, and every phase
# reads long before the margin is spent.
PLAN_CLIFF_GUARD_SECONDS = 600
# How far back the Server itself keeps raw Node samples, which is the horizon the
# burst's storage arithmetic has to use: a row is stored only while its instant is
# inside the raw family's own policy cutoff, not inside the window this run asks
# the read path for. crates/platpulse-server/src/retention.rs:303 declares the
# family "raw_metric_sample" with "24 hours of raw history, design §11.4" and
# forbids shortening it below one day, and
# crates/platpulse-server/src/metric_history.rs:60-61 carries the same window for
# the read default. A run with a different --hours still writes into this same
# 24 hour window.
SERVER_RAW_RETENTION_SECONDS = 86400
# How far back any tier answers at all, which is what the read path calls
# "availability" since issue #214: crates/platpulse-server/src/metric_history.rs:141
# declares FIVE_MINUTE_MAX_AGE_DAYS = 30 for the 5 minute tier, and
# crates/platpulse-server/src/http/admin.rs:4643-4649 decides availability from
# now - that many days alone. Issue #213 measured the same field against the raw
# window's cutoff instead, so a range the 24 hour Server called unavailable is
# answerable now.
SERVER_HISTORY_HORIZON_DAYS = 30

# -- the aggregate tiers (issue #214) -------------------------------------
#
# Facts under test, read from the tree when this run was written:
#   * crates/platpulse-server/migrations/0067_node_metric_aggregates.sql creates
#     node_metric_aggregates keyed (node_id, metric, grain_seconds, bucket_start)
#     with grain_seconds in (60, 300), bucket_start on the UTC-aligned start of
#     the bucket, sample_count/min_value/max_value/last_value and the observation
#     instants, plus the expiry index (grain_seconds, bucket_start). A bucket row
#     exists only when at least one observation arrived in it, so an empty bucket
#     is absent rather than zero.
#   * crates/platpulse-server/src/metric_history.rs:892 records both grains for
#     every observation the Server accepted as new, whether or not the raw window
#     stored it: the 1 minute tier answers day 7 up to the raw cutoff and the 5
#     minute tier day 30 up to day 7 (:1064-1066), and nothing is re-derived at
#     read time, so zooming an old stretch can never recover a raw sample.
#   * crates/platpulse-server/src/retention.rs:248 bounds one tier cleanup at
#     AGGREGATE_CLEANUP_BATCH = 4096 rows (asserted at :253-264 to cover a maximal
#     Report): the 1 minute tier is deleted below the 7 day window, the 5 minute
#     tier below the 30 day one, and the pass runs after every accepted Report
#     (crates/platpulse-server/src/http/report_ingestion.rs:3116).
TIER_REPORTS = 720
TIER_CADENCE_SECONDS = 3600
TIER_SPAN_SECONDS = (TIER_REPORTS - 1) * TIER_CADENCE_SECONDS
# The newest instant is half an hour old, so the Server's moving raw cutoff
# (now - 24 hours) and both tier floors (now - 7 days, now - 30 days) pass no
# grid instant for the whole phase: the set a read sees stays still while the
# phase, its paging walk and its cleanup probes run.
TIER_NEWEST_AGE_SECONDS = 1800
TIER_ONE_MINUTE_SECONDS = 60
TIER_FIVE_MINUTE_SECONDS = 300
TIER_ONE_MINUTE_WINDOW_SECONDS = 7 * 86400
TIER_FIVE_MINUTE_WINDOW_SECONDS = 30 * 86400
# One block per phase keeps the earlier phases' Node counts exact.
TIER_CLONE_BLOCK = 0x2000
TIER_PLANT_CLONE_BLOCK = 0x2100
TIER_PAGE_LIMIT = 5
TIER_READ_LIMIT = 20000
# The raw-only read must sit entirely inside the raw window: a 48 hour read would
# reach past the raw cutoff and be answered by 1 minute buckets for its older
# half, which is exactly what the mixed read below measures instead.
TIER_RAW_READ_HOURS = 20
# How many series one tier Report stores samples for: the fixture's
# process_cpu_percent and process_memory_percent, plus the two chain height
# series issue #217 stores from the sync observation (sync_current_block,
# sync_highest_block). The tier plan multiplies every bucket estimate by this,
# so it is the series count per Report that the sample and aggregate counts see.
TIER_SERIES = 4
# The value of an hourly instant is a function of its index alone - a ladder
# from 10 to 28 and back - so a tier's floor and ceiling are known before it is
# read, and one index carries a planted spike no bucket may round away.
TIER_VALUE_BASE = 10.0
TIER_VALUE_STEP = 3.0
TIER_VALUE_STEP_COUNT = 7
TIER_SPIKE_INDEX = 671
TIER_SPIKE_CPU = 99.5
# One planted observation past both tier windows and one past the 1 minute
# window only: the first must leave no bucket behind, the second must keep its 5
# minute bucket and lose its 1 minute one.
TIER_PLANT_EXPIRED_AGE_SECONDS = 31 * 86400
TIER_PLANT_SURVIVING_AGE_SECONDS = 29 * 86400
# Every tier inventory is a different single-Node inventory, so each declares a
# revision strictly above DRAIN_INVENTORY_REVISION (1002), the last change the
# earlier phases made. A frozen v1 Report whose declared revision equals the
# accepted one is rejected with InventoryRevisionConflict when its content
# differs (crates/platpulse-server/src/http/report_ingestion.rs:2084-2104), and
# one below the accepted revision is rejected outright (:2048-2065). The seeded
# Node declares the first revision - all 720 Reports carry the same inventory,
# so they share it, the way MULTI_NODE_INVENTORY_REVISION (1001) carries the
# multi-Node inventory - and each planted Node plus the trigger Node declares
# the next one.
TIER_INVENTORY_REVISIONS = (2001, 2002, 2003, 2004)

# -- issue #215: the Host family stored once per Agent --------------------
#
# A Report states its Host quantities once, for the Agent that collected them,
# and the Server stores them under that Agent
# (crates/platpulse-server/src/http/report_ingestion.rs:807-874 host_series_samples,
# called at :1163-1173). HOST_METRIC_SERIES
# (crates/platpulse-server/src/metric_history.rs:116-127) is ten series: eight
# stated per Host observation, plus two storage series whose dimension is the
# mount path the Agent reported. Nothing in the family is counted per Node, so
# the phase below audits its cardinality against the Nodes this run registered
# and against the nodes that stored Node series of their own.
HOST_SHARED_SERIES = (
    "cpu_percent",
    "load1",
    "load5",
    "load15",
    "memory_total_bytes",
    "memory_used_bytes",
    "network_rx_bytes_per_sec",
    "network_tx_bytes_per_sec",
)
HOST_MOUNT_SERIES = ("disk_total_bytes", "disk_used_bytes")
DECLARED_HOST_FAMILY = (
    "cpu_percent",
    "disk_total_bytes",
    "disk_used_bytes",
    "load1",
    "load15",
    "load5",
    "memory_total_bytes",
    "memory_used_bytes",
    "network_rx_bytes_per_sec",
    "network_tx_bytes_per_sec",
)
# The declared family is the guard: editing HOST_SHARED_SERIES without editing
# DECLARED_HOST_FAMILY, or drifting from the Server's own tuple, fails the run.
HOST_FAMILY = tuple(sorted(HOST_SHARED_SERIES + HOST_MOUNT_SERIES))
# The per-Host mount contract limit, and the batches the Server cleans the
# family with (crates/platpulse-server/src/metric_history.rs:83 MAX_HOST_MOUNTS;
# crates/platpulse-server/src/retention.rs:216-222
# HOST_METRIC_CLEANUP_BATCH = 512 (asserted at :231-236); AGGREGATE_CLEANUP_BATCH = 4096
# both guarded by compile-time assertions that they cover one maximal Host
# Report: HOST_METRIC_SERIES.len() + 2 * MAX_HOST_MOUNTS = 266 rows).
MAX_HOST_MOUNTS = 128
HOST_METRIC_CLEANUP_BATCH = 512
AGGREGATE_CLEANUP_BATCH = 4096
# What one Report really carries: eight shared rows, plus two per mount.
MAX_HOST_ROWS_PER_REPORT = len(HOST_SHARED_SERIES) + 2 * MAX_HOST_MOUNTS
SERVER_MAX_HOST_ROWS_PER_REPORT = len(HOST_FAMILY) + 2 * MAX_HOST_MOUNTS
# Issue #215 keeps the family as it is: no Swap series and no disk throughput
# series are part of it, and this run neither registers nor measures any.
HOST_FORBIDDEN_TOKENS = (
    "swap",
    "iops",
    "disk_read",
    "disk_write",
    "io_wait",
    "read_bytes_per_sec",
    "write_bytes_per_sec",
)
# The planted Host Report carries the fixture's own single-Node inventory, so it
# cannot reuse a tier revision: a Report whose declared revision is below the
# accepted one is rejected outright, and the tier phase left 2004
# (TIER_INVENTORY_REVISIONS[-1]) accepted. It declares the next block instead,
# the way MULTI_NODE_INVENTORY_REVISION (1001) followed DRAIN_INVENTORY_REVISION
# (1002) and the tier revisions followed that.
# Issue #216 keeps the same family: the storage pair is answered per mount
# path, and the Owner's list of those paths is a bounded walk of the ledger, so
# one Report still carries at most MAX_HOST_ROWS_PER_REPORT rows however many
# paths the Agent holds. The bound below is the read side of that declaration:
# the list answers the newest MOUNT_COVERAGE_LIMIT paths of one Agent.
MOUNT_USED_METRIC = "disk_used_bytes"
MOUNT_CAPACITY_METRIC = "disk_total_bytes"
MOUNT_CADENCE_METRIC = "cpu_percent"
MOUNT_COVERAGE_LIMIT = 2 * MAX_HOST_MOUNTS
COVERAGE_CADENCE_SAMPLES = 16
MAX_OBSERVED_CADENCE_SECONDS = 300
GAP_CADENCE_FACTOR = 3
MIN_GAP_SECONDS = 120
# The mount list is read MOUNT_COVERAGE_READS times so its latency is published
# as a distribution over real reads rather than as one sample.
MOUNT_COVERAGE_READS = 12
# One Report is at most MAX_HOST_MOUNTS mounts, so more paths than the list can
# answer takes three Reports of disjoint paths, planted at three ages so the
# order the list promises can be read off the answer itself.
MOUNT_BULK_PATHS_PER_REPORT = 100
MOUNT_RETIRED_AGE_SECONDS = 7200
# The newest instant is the next second on the Agent clock rather than the instant
# the ledger already holds: a reading the Server sees ahead of its own clock is
# stated as reported however long this run has been going, instead of an age the
# script would have to guess at.
MOUNT_NEWEST_OFFSET_SECONDS = 1
# The rhythm the mount phase leaves as the Agent's newest observations, and the
# reason it is planted at all: the Server states the cadence the Agent is keeping
# now, trusting the newest interval only when the interval before it agrees within
# CADENCE_AGREEMENT_FACTOR (crates/platpulse-server/src/metric_history.rs
# `current_cadence_seconds`). Three Reports state that rhythm, because the newest
# interval is trusted only when the interval before it agrees, and the third is
# what puts the interval before the newest rhythm Report on the rhythm instead of
# on the pair of observations one second apart the phases above end with. Two
# Reports would leave the list answering cadence 0 - unknown - and no state below
# could be judged at all; and because that pair stays the fastest interval the
# ledger holds, the cadence the list answers is evidence that the newest rhythm
# decided it rather than the fastest interval still stored.
#
# The interval is computed from the ledger rather than fixed: all three Reports
# have to sit above the newest observation the ledger already holds, so a fixed
# interval wide enough to read as a rhythm could reach below that pair on a slower
# or faster machine, leaving every state below it judged against an unmeasurable
# cadence. The three Reports take MOUNT_RHYTHM_DIVISOR-th of the room between the
# newest instant and that observation, which leaves a quarter of it as clearance.
MOUNT_RHYTHM_FLOOR_SECONDS = 5
MOUNT_RHYTHM_DIVISOR = 4
MOUNT_SOLO_NODE_BLOCK = 0x2160
MOUNT_SOLO_PATH = "/solo-data"
MOUNT_SOLO_CACHE_PATH = "/solo-cache"
MOUNT_RETIRED_PATH = "/archive-data"
MOUNT_LIVE_PATH = "/live-data"
MOUNT_RELEASED_PATH = "/released-data"
# Migration 0069's index, and the plans that would mean the walk sorted instead
# of seeking it. The statement below is the Server's own coverage read
# (crates/platpulse-server/src/metric_history.rs:2286), binds and all.
MOUNT_COVERAGE_INDEX = "host_metric_series_state_mount_idx"
MOUNT_COVERAGE_FORBIDDEN_PLANS = ("TEMP B-TREE", "SCAN l ", "SCAN host_metric_series_state")
MOUNT_COVERAGE_SQL = (
    "SELECT l.dimension, l.first_observed_at, l.last_observed_at, l.last_received_at,"
    " l.observation_count, l.replayed_count, l.corrected_count, l.released_before, s.value,"
    " s.received_at FROM host_metric_series_state l LEFT JOIN host_metric_samples s"
    " ON s.agent_id = l.agent_id AND s.metric = l.metric AND s.dimension = l.dimension"
    " AND s.observed_at = l.last_observed_at WHERE l.agent_id = ? AND l.metric = ?"
    " ORDER BY l.last_observed_at DESC, l.dimension ASC LIMIT ?"
)

# ---------------------------------------------------------------------------
# Issue #217: the recorded sync and consensus state log, and the five chain
# height series the same Report states.
# ---------------------------------------------------------------------------

# The five Node series issue #217 added to the metric engine, in the order
# crates/platpulse-server/src/metric_history.rs NODE_METRIC_SERIES declares them.
STATE_NODE_SERIES = (
    "sync_current_block",
    "sync_highest_block",
    "consensus_highest_qc_block",
    "consensus_highest_lock_block",
    "consensus_highest_commit_block",
)
# The two components whose state the Server records, and the names a paused
# delivery is filed under in capacity_skipped_series (the component name, with
# an empty dimension).
STATE_COMPONENTS = ("sync", "consensus")
STATE_CHANGE_KIND = "change"
STATE_ANCHOR_KIND = "anchor"
# The anchor window in crates/platpulse-server/src/state_history.rs
# (STATE_ANCHOR_SECONDS): an unchanged state is restated at most once per hour.
STATE_ANCHOR_SECONDS = 3600
# The read limits the Server declares (DEFAULT_STATE_LIMIT / MAX_STATE_LIMIT).
STATE_DEFAULT_LIMIT = 2000
STATE_MAX_LIMIT = 20000
# Rows one recorded-state cleanup batch may release
# (crates/platpulse-server/src/retention.rs STATE_CLEANUP_BATCH), which the
# compile-time assert holds at one maximal Report: MAX_NODE_OBSERVATIONS
# (platpulse-core/src/protocol.rs:33) x STATE_COMPONENTS.
STATE_CLEANUP_BATCH = 2048
MAX_NODE_OBSERVATIONS = 256
MAX_STATE_ROWS_PER_REPORT = MAX_NODE_OBSERVATIONS * len(STATE_COMPONENTS)
# The window every state answer declares, in days
# (retention.rs MIN_INVESTIGATION_AGGREGATE_DAYS).
STATE_RETENTION_DAYS = 30
# This phase's own Node block, rhythm and read limits.
STATE_NODE_BLOCK = 0x2170
STATE_INVENTORY_REVISION = 3002
STATE_RHYTHM_SECONDS = 600
STATE_LEAD_SECONDS = 28800
STATE_PAGE_LIMIT = 5
STATE_PAUSE_REPORTS = 3
STATE_READS = 12
STATE_HISTORY_PATH = "/api/admin/v1/nodes/{node_id}/state-history"
# The index the read path must seek, and the plans that would mean it walked
# the whole table or sorted instead (measured against migration 0070's DDL).
STATE_READ_INDEX = "sqlite_autoindex_node_state_observations_1"
STATE_LEDGER_INDEX = "node_state_series_state_series_idx"
STATE_READ_FORBIDDEN_PLANS = (
    "TEMP B-TREE",
    "SCAN node_state_observations",
    "SCAN node_state_series_state",
)
# The Server's own read statements
# (crates/platpulse-server/src/state_history.rs load_range and fetch_ledger),
# binds and all. The paged variant adds one predicate to the same statement.
STATE_READ_SQL = (
    "SELECT observed_at, received_at, entry_kind, collection_state, value_source,"
    " value_observed_at, error_code, syncing FROM node_state_observations"
    " WHERE node_id = ? AND component = ? AND observed_at > ? AND observed_at <= ?"
    " ORDER BY observed_at DESC LIMIT ?"
)
STATE_READ_PAGED_SQL = (
    "SELECT observed_at, received_at, entry_kind, collection_state, value_source,"
    " value_observed_at, error_code, syncing FROM node_state_observations"
    " WHERE node_id = ? AND component = ? AND observed_at > ? AND observed_at <= ?"
    " AND observed_at < ? ORDER BY observed_at DESC LIMIT ?"
)
STATE_LEDGER_SQL = (
    "SELECT first_observed_at, last_observed_at, last_received_at,"
    " last_collection_state, last_value_source, last_value_observed_at,"
    " last_error_code, last_syncing, last_entry_at, entry_count, change_count,"
    " anchor_count, replayed_count, corrected_count, released_before, updated_at"
    " FROM node_state_series_state WHERE node_id = ? AND component = ?"
)
# A chain height is read back exactly like every other Node metric: the same
# range statement the metric engine runs for a Node series
# (crates/platpulse-server/src/metric_history.rs RANGE_SAMPLE_SQL, which the
# issue #217 series widened with the same five names).
STATE_HEIGHT_SQL = (
    "SELECT observed_at, received_at, value FROM node_metric_samples"
    " WHERE node_id = ? AND metric = ? AND observed_at >= ? AND observed_at <= ?"
    " ORDER BY observed_at DESC LIMIT ?"
)
STATE_HEIGHT_FORBIDDEN_PLANS = ("TEMP B-TREE", "SCAN node_metric_samples")


def gap_threshold_seconds(cadence_seconds: int) -> int:
    """The silence bound of a measured cadence: three cadences, clamped to the
    Server's own ceiling and never below two minutes
    (crates/platpulse-server/src/metric_history.rs:734)."""
    clamped = min(max(cadence_seconds, 1), MAX_OBSERVED_CADENCE_SECONDS)
    return max(clamped * GAP_CADENCE_FACTOR, MIN_GAP_SECONDS)


def storage_mounts_url(agent_id: str) -> str:
    """Issue #216's Owner-only route: the storage family of one Agent, one entry
    per mount path it has reported, newest first."""
    return "/api/admin/v1/agents/" + agent_id + "/storage-mounts"


def bulk_mount_path(index: int) -> str:
    """One disjoint block of mount paths per planted Report, so which Report a
    path belongs to reads off the path itself."""
    return "/bulk-" + str(index).zfill(3)


def mount_used_bytes(path: str) -> int:
    """A used reading that is a function of the path alone, so the value the
    Server answers for one path is checkable without planting a second Report."""
    return (1 << 30) + sum(ord(character) for character in path) * 4096


def mount_payload(paths: list) -> list:
    """One entry per path in the Report's own mount shape: both readings are
    functions of the path alone, and used stays below total."""
    return [
        {
            "mount_path": path,
            "total_bytes": mount_used_bytes(path) + (1 << 33),
            "used_bytes": mount_used_bytes(path),
        }
        for path in paths
    ]


def mount_entry(read: dict, path: str) -> dict:
    """The one entry a list answered for one path, flattened to the fields the
    checks read. A path the list does not answer is an absent entry rather than a
    crash, so a broken promise fails a check instead of the run."""
    for entry in read["mounts"]:
        if entry.get("mountPath") == path:
            used = entry.get("used") or {}
            capacity = entry.get("capacity") or {}
            return {
                "answered": True,
                "mount_path": path,
                "observation_state": entry.get("observationState"),
                "silent_seconds": entry.get("silentSeconds"),
                "used_observed": used.get("observed"),
                "used_latest_value": used.get("latestValue"),
                "used_released_before": used.get("releasedBefore"),
                "used_first_observed_at": used.get("firstObservedAt"),
                "used_last_observed_at": used.get("lastObservedAt"),
                "used_observation_count": used.get("observationCount"),
                "capacity_observed": capacity.get("observed"),
                "capacity_latest_value": capacity.get("latestValue"),
                "capacity_released_before": capacity.get("releasedBefore"),
                "capacity_last_observed_at": capacity.get("lastObservedAt"),
            }
    return {"answered": False, "mount_path": path}


def mount_read_summary(read: dict) -> dict:
    """One mount list read without its per-path rows: the bound it answered
    under, the cadence it judged against, and what the read cost."""
    silent = sorted(
        {entry.get("silentSeconds") for entry in read["mounts"]},
        key=lambda value: (value is None, value if value is not None else 0),
    )
    return {
        "path": read["path"],
        "reads": read["reads"],
        "answered_at": read["answered_at"],
        "cadence_seconds": read["cadence_seconds"],
        "silence_threshold_seconds": read["silence_threshold_seconds"],
        "used_metric": read["used_metric"],
        "capacity_metric": read["capacity_metric"],
        "mount_limit": read["mount_limit"],
        "truncated": read["truncated"],
        "cache_control": read["cache_control"],
        "mounts": len(read["mounts"]),
        "first_path": read["mounts"][0]["mountPath"] if read["mounts"] else None,
        "last_path": read["mounts"][-1]["mountPath"] if read["mounts"] else None,
        "states": sorted({entry.get("observationState") for entry in read["mounts"]}),
        "silent_seconds": silent[:8],
        "payload_bytes": read["payload_bytes"],
        "latency_ms": read["latency_ms"],
    }


def mount_ledger_order(db_path: Path, agent_id: str) -> list:
    """The order the list promises, recomputed from the ledger itself: newest
    observation first with ties settled by the path, which is the Server's own
    ORDER BY over host_metric_series_state."""
    rows = sqlite_rows(
        db_path,
        "SELECT dimension, last_observed_at FROM host_metric_series_state WHERE agent_id = '"
        + agent_id
        + "' AND metric IN ('"
        + MOUNT_USED_METRIC
        + "', '"
        + MOUNT_CAPACITY_METRIC
        + "')",
    )
    newest = {}
    for row in rows:
        path = row["dimension"]
        if path not in newest or row["last_observed_at"] > newest[path]:
            newest[path] = row["last_observed_at"]
    ordered = sorted(newest)
    ordered.sort(key=lambda path: newest[path], reverse=True)
    return [[path, newest[path]] for path in ordered]


def host_cadence_instant(db_path: Path, agent_id: str) -> str | None:
    """The newest observation the ledger already holds for the Agent's cadence
    series. The rhythm below is planted above it, so the newest intervals the list
    measures are the planted rhythm and not what the phases above left there."""
    rows = sqlite_rows(
        db_path,
        "SELECT last_observed_at FROM host_metric_series_state WHERE agent_id = '"
        + agent_id
        + "' AND metric = '"
        + MOUNT_CADENCE_METRIC
        + "' AND dimension = ''",
    )
    return rows[0]["last_observed_at"] if rows else None


def mount_rhythm_seconds(newest_at: str, held_at: str | None) -> int:
    """The interval three planted Reports state as the Agent's rhythm: the newest
    instant, the newest observation the ledger already holds below it, and the three
    Reports that have to fit between the two."""
    if held_at is None:
        return MOUNT_RHYTHM_FLOOR_SECONDS
    room = (parse_instant(newest_at) - parse_instant(held_at)).total_seconds()
    return max(MOUNT_RHYTHM_FLOOR_SECONDS, int(room // MOUNT_RHYTHM_DIVISOR))


def mount_rows(db_path: Path, agent_id: str) -> dict:
    """The mount rows one Agent really holds next to the Nodes it has: the counts
    that show the mount family is keyed by Agent and path, so neither its series
    nor its samples follow the Node count."""
    metric_filter = " AND metric IN ('" + MOUNT_USED_METRIC + "', '" + MOUNT_CAPACITY_METRIC + "')"
    return {
        "mount_series": sqlite_scalar(
            db_path,
            "SELECT COUNT(*) FROM host_metric_series_state WHERE agent_id = '" + agent_id + "'" + metric_filter,
        ),
        "mount_samples": sqlite_scalar(
            db_path, "SELECT COUNT(*) FROM host_metric_samples WHERE agent_id = '" + agent_id + "'" + metric_filter
        ),
        "distinct_paths": sqlite_scalar(
            db_path,
            "SELECT COUNT(DISTINCT dimension) FROM host_metric_series_state WHERE agent_id = '"
            + agent_id
            + "'"
            + metric_filter,
        ),
        "nodes": sqlite_scalar(db_path, "SELECT COUNT(*) FROM nodes WHERE agent_id = '" + agent_id + "'"),
        # The Node ledger is keyed by (node_id, metric) - it has no agent_id at
        # all (migrations/0066_node_metric_history.sql:42) - so its rows follow the
        # Node count, which is exactly what the mount ledger does not do.
        "node_series": sqlite_scalar(
            db_path,
            "SELECT COUNT(*) FROM node_metric_series_state s JOIN nodes n ON n.node_id = s.node_id"
            " WHERE n.agent_id = '"
            + agent_id
            + "'",
        ),
    }


def mount_coverage_plan(db_path: Path, agent_id: str, metric: str, limit: int) -> list:
    """EXPLAIN QUERY PLAN of the coverage read under the binds the Server itself
    uses: the plan is what shows the read seeks the mount index instead of
    sorting every path the Agent ever reported."""
    connection = sqlite3.connect("file:" + str(db_path) + "?mode=ro", uri=True)
    try:
        connection.row_factory = sqlite3.Row
        rows = connection.execute("EXPLAIN QUERY PLAN " + MOUNT_COVERAGE_SQL, (agent_id, metric, limit)).fetchall()
        return [dict(row)["detail"] for row in rows]
    finally:
        connection.close()


HOST_INVENTORY_REVISION = 3001


def aligned_bucket_start(observed_at: str, grain_seconds: int) -> str:
    """The UTC-aligned bucket an instant belongs to, computed the way the Server
    aligns it (crates/platpulse-server/src/metric_history.rs:892), so a planted
    bucket can be looked up by its own start instead of by its row order."""
    epoch = int(parse_instant(observed_at).timestamp())
    aligned = datetime.fromtimestamp(epoch - (epoch % grain_seconds), timezone.utc)
    return aligned.strftime(CANONICAL)


def clone_node_id(block: int, ordinal: int) -> str:
    """A Node id in the fixture shape (0195f2a1-0014-4014-8014-000000000014),
    one distinct block per phase so the earlier phases counts stay exact."""
    value = block + ordinal
    return "0195f2a1-%04x-%04x-%04x-%012x" % (
        value,
        0x4000 | (value & 0xFFF),
        0x8000 | (value & 0xFFF),
        value,
    )


def state_node_id(ordinal: int) -> str:
    """This phase's own Node block, so no earlier phase count moves."""
    return clone_node_id(STATE_NODE_BLOCK, ordinal)


def state_sync_block(
    observed_at: str, syncing: bool, current_block: int, highest_block: int, revision: int
) -> dict:
    """chain.sync as a probe that answered, for one instant.

    The two heights move on every Report while the recorded state vector holds
    only collection_state, value_source, error_code and syncing
    (crates/platpulse-server/src/state_history.rs StateVector), so a Report
    whose heights moved but whose vector did not is still a counted delivery
    that stores no second entry.
    """
    return {
        "status": "ok",
        "attempted_at": observed_at,
        "latest_observed_at": observed_at,
        "state_revision": revision,
        "value_revision": revision,
        "latest": {
            "syncing": syncing,
            "current_block": current_block,
            "highest_block": highest_block,
            "pulled_states": 0,
            "known_states": 0,
        },
    }


def state_consensus_block(
    observed_at: str,
    highest_qc_block: int,
    highest_lock_block: int,
    highest_commit_block: int,
    revision: int,
) -> dict:
    """chain.consensus as a probe that answered, for one instant."""
    return {
        "status": "ok",
        "attempted_at": observed_at,
        "latest_observed_at": observed_at,
        "state_revision": revision,
        "value_revision": revision,
        "latest": {
            "epoch": 1,
            "view_number": 1,
            "validator": False,
            "highest_qc_block": highest_qc_block,
            "highest_lock_block": highest_lock_block,
            "highest_commit_block": highest_commit_block,
        },
    }


def state_error_block(observed_at: str, revision: int, code: str, message: str) -> dict:
    """A chain component whose probe failed: status error, a message, and no
    latest reading at all (crates/platpulse-core/src/component.rs:204-215
    forbids a latest reading without a latest_observed_at, and an error without
    one). The metric writer then stores no height for the series that component
    owns (crates/platpulse-server/src/http/report_ingestion.rs metric_observed_at
    returns None), while the state log still records the transition.
    """
    return {
        "status": "error",
        "attempted_at": observed_at,
        "state_revision": revision,
        "value_revision": revision,
        "error": {"code": code, "message": message},
    }


def state_report(
    fixture: dict,
    agent: dict,
    sequence: int,
    observed_at: str,
    sync_blocks: list,
    consensus_blocks: list,
    inventory_revision: int,
) -> dict:
    """A Report stating exactly this phase's Nodes, one chain block each.

    The inventory carries exactly the Nodes the Report observes
    (crates/platpulse-core/src/envelope.rs requires one observation per
    inventory Node and none outside it), so the revision moves up and names one
    Node per ordinal: the probe Node (ordinal 0) keeps its rows for the SQLite
    and plan reads, and the ledger Node (ordinal 1) is the one that is purged.
    Every Node keeps the fixture's disabled process observation, so the only
    Node series these Nodes ever state are the five chain heights issue #217
    added (crates/platpulse-server/src/metric_history.rs NODE_METRIC_SERIES).
    """
    if len(sync_blocks) != len(consensus_blocks):
        raise BaselineError("a state Report needs one chain block pair per Node")
    report = copy.deepcopy(fixture)
    report["agent_id"] = agent["agent_id"]
    report["agent_epoch"] = agent["agent_epoch"]
    report["boot_id"] = agent["boot_id"]
    report["boot_transition"] = "continuing"
    report["report_sequence"] = sequence
    report["report_id"] = report_id_for(sequence)
    report["generated_at"] = observed_at
    restamp(report, observed_at)
    template = report["nodes"][0]
    inventory = list(report["inventory"]["nodes"])
    nodes = []
    inventory_nodes = []
    for ordinal, (sync_block, consensus_block) in enumerate(zip(sync_blocks, consensus_blocks)):
        node = copy.deepcopy(template)
        node_id = state_node_id(ordinal)
        node["node_id"] = node_id
        node["chain"]["sync"] = sync_block
        node["chain"]["consensus"] = consensus_block
        nodes.append(node)
        entry = copy.deepcopy(inventory[0])
        entry["node_id"] = node_id
        inventory_nodes.append(entry)
    report["nodes"] = nodes
    report["inventory"]["nodes"] = inventory_nodes
    report["inventory"]["revision"] = inventory_revision
    return report


def state_history_url(
    node_id: str,
    component: str,
    from_instant: str,
    to_instant: str,
    limit: int | None = None,
    before: str | None = None,
) -> str:
    """The Owner-only recorded-state route (crates/platpulse-server/src/http/admin.rs
    admin_node_state_history), query and all."""
    path = STATE_HISTORY_PATH.format(node_id=node_id)
    path += "?component=" + component + "&from=" + from_instant + "&to=" + to_instant
    if limit is not None:
        path += "&limit=" + str(limit)
    if before is not None:
        path += "&before=" + before
    return path


def node_height_history_url(node_id: str, metric: str, from_instant: str, to_instant: str, limit: int) -> str:
    """One of the five chain height series, read back through the metric route
    that already existed (crates/platpulse-server/src/http/admin.rs
    admin_node_metric_history)."""
    return history_url(
        node_id,
        metric,
        "&from=" + from_instant + "&to=" + to_instant + "&limit=" + str(limit),
    )


def read_surface(client: Client, cookie: str, path: str) -> dict:
    """One admin GET, answered or refused, with its parsed body and latency."""
    status, headers, body, elapsed_ms = admin_get(client, cookie, path)
    try:
        payload = json.loads(body)
    except ValueError:
        payload = {}
    return {
        "path": path,
        "status": status,
        "payload": payload,
        "error": (payload.get("error") or {}) if isinstance(payload.get("error"), dict) else {},
        "headers": headers,
        "bytes": len(body),
        "latency_ms": elapsed_ms,
    }


def state_rows(db_path: Path, node_id: str) -> dict:
    """What the two recorded-state tables and the metric table hold for one
    Node, read while the Server is stopped (the development-mode WAL holds one
    writer at a time)."""
    by_component = {}
    kind_rows = {}
    for row in sqlite_rows(
        db_path,
        "SELECT component, entry_kind, COUNT(*) AS rows FROM node_state_observations"
        " WHERE node_id = '" + node_id + "' GROUP BY component, entry_kind",
    ):
        by_component[row["component"]] = by_component.get(row["component"], 0) + int(row["rows"])
        kind_rows[str(row["entry_kind"])] = kind_rows.get(str(row["entry_kind"]), 0) + int(row["rows"])
    ledger = {}
    for row in sqlite_rows(
        db_path,
        "SELECT component, entry_count, change_count, anchor_count, replayed_count,"
        " corrected_count, last_entry_at, last_observed_at FROM node_state_series_state"
        " WHERE node_id = '" + node_id + "'",
    ):
        ledger[str(row["component"])] = {
            "entry_count": int(row["entry_count"]),
            "change_count": int(row["change_count"]),
            "anchor_count": int(row["anchor_count"]),
            "replayed_count": int(row["replayed_count"]),
            "corrected_count": int(row["corrected_count"]),
            "last_entry_at": row["last_entry_at"],
            "last_observed_at": row["last_observed_at"],
        }
    heights = {}
    for row in sqlite_rows(
        db_path,
        "SELECT metric, COUNT(*) AS rows, MIN(value) AS low, MAX(value) AS high"
        " FROM node_metric_samples WHERE node_id = '" + node_id + "' GROUP BY metric",
    ):
        heights[str(row["metric"])] = {
            "rows": int(row["rows"]),
            "low": row["low"],
            "high": row["high"],
        }
    return {
        "node_id": node_id,
        "observations": sqlite_scalar(
            db_path, "SELECT COUNT(*) FROM node_state_observations WHERE node_id = '" + node_id + "'"
        ),
        "series_rows": sqlite_scalar(
            db_path, "SELECT COUNT(*) FROM node_state_series_state WHERE node_id = '" + node_id + "'"
        ),
        "by_component": by_component,
        "kinds": kind_rows,
        "ledger": ledger,
        "heights": heights,
    }


def state_plan(db_path: Path, sql: str, binds: tuple) -> list:
    """The query plan the Server's own recorded-state statement produces with
    its own binds, read from a read-only connection."""
    connection = sqlite3.connect("file:" + str(db_path) + "?mode=ro", uri=True)
    connection.row_factory = sqlite3.Row
    try:
        rows = connection.execute("EXPLAIN QUERY PLAN " + sql, binds).fetchall()
        return [dict(row)["detail"] for row in rows]
    finally:
        connection.close()


def entry_instants(entries: list) -> list:
    return [str(entry["observedAt"]) for entry in entries]


def covered_windows(entries: list, anchor_seconds: int) -> list:
    """The stretch each stored state speaks for: its own instant plus the
    anchor window the Server declares
    (crates/platpulse-server/src/state_history.rs STATE_ANCHOR_SECONDS)."""
    return [
        (str(entry["observedAt"]), shift_instant(str(entry["observedAt"]), anchor_seconds))
        for entry in entries
    ]


def covered_seconds(windows: list, from_instant: str, to_instant: str) -> int:
    """The union of entry windows, clipped to the answered range, in seconds."""
    from_time = parse_instant(from_instant)
    to_time = parse_instant(to_instant)
    intervals = []
    for start, end in windows:
        low = max(parse_instant(start), from_time)
        high = min(parse_instant(end), to_time)
        if high > low:
            intervals.append((low, high))
    intervals.sort()
    total = 0.0
    current_low = None
    current_high = None
    for low, high in intervals:
        if current_low is None:
            current_low, current_high = low, high
        elif low <= current_high:
            current_high = max(current_high, high)
        else:
            total += (current_high - current_low).total_seconds()
            current_low, current_high = low, high
    if current_low is not None:
        total += (current_high - current_low).total_seconds()
    return int(total)


def state_gap_kinds(entries_payload: dict) -> dict:
    """The gaps an answer carries, counted by kind, with their skipped counts."""
    counts = {}
    skipped = {}
    for gap in entries_payload.get("gaps") or []:
        kind = str(gap.get("kind"))
        counts[kind] = counts.get(kind, 0) + 1
        skipped.setdefault(kind, []).append(gap.get("skippedCount"))
    return {"counts": counts, "skipped": skipped}


def node_with_all_series(template: dict, node_id: str, ordinal: int, cpu: float, observed_at: str) -> dict:
    """One Node observation carrying all five stored series.

    The fixture carries only process_cpu_percent and process_memory_percent;
    the other three need the two data directory readings and a chain peer
    snapshot (crates/platpulse-server/src/http/report_ingestion.rs:1360-1405).
    Every value is a deterministic function of the ordinal and never of the
    sequence, so a replay built with the same ordinal carries the same values
    and the Server must classify it as a restatement rather than an arrival.
    """
    node = copy.deepcopy(template)
    node["node_id"] = node_id
    revision = ordinal + 1
    node["process"] = {
        "status": "ok",
        "attempted_at": observed_at,
        "latest_observed_at": observed_at,
        "state_revision": revision,
        "value_revision": revision,
        "latest": {
            "pid": 21000 + ordinal,
            "started_at": observed_at,
            "cpu_percent": cpu,
            "memory_bytes": 1073741824 + ordinal * 67108864,
            "uptime_ms": 3600000 + ordinal * 1000,
        },
    }
    node["data_directory_size_bytes"] = {
        "status": "ok",
        "attempted_at": observed_at,
        "latest_observed_at": observed_at,
        "state_revision": revision,
        "value_revision": revision,
        "latest": 300000000000 + ordinal * 1073741824,
    }
    node["data_directory_capacity_bytes"] = {
        "status": "ok",
        "attempted_at": observed_at,
        "latest_observed_at": observed_at,
        "state_revision": revision,
        "value_revision": revision,
        "latest": 549755813888,
    }
    chain = node.get("chain")
    if not isinstance(chain, dict):
        chain = {}
        node["chain"] = chain
    peers = []
    for direction in ("inbound", "outbound"):
        peers.append(
            {
                "peer_id": "peer-" + str(ordinal) + "-" + direction,
                "direction": direction,
                "trusted": True,
                "static_peer": False,
                "consensus_peer": False,
                "caps": [],
            }
        )
    chain["peers"] = {
        "status": "ok",
        "attempted_at": observed_at,
        "latest_observed_at": observed_at,
        "state_revision": revision,
        "value_revision": revision,
        "latest": {"peers": peers},
    }
    return node


def multi_node_report(
    fixture: dict,
    agent: dict,
    sequence: int,
    observed_at: str,
    clone_block: int,
    clone_count: int,
    cpu_base: float,
    inventory_revision: int,
) -> dict:
    """A Report carrying the fixture Node plus clone_count cloned Nodes.

    The inventory is additive and carries exactly the Nodes the Report
    observes: crates/platpulse-core/src/envelope.rs:248-307 requires one
    observation per inventory Node and none outside it, so the inventory
    revision moves up and holds the fixture Node plus every clone. The fixture
    Node keeps its disabled process observation, so it stores no series and
    every row in this phase belongs to a clone.
    """
    report = copy.deepcopy(fixture)
    report["agent_id"] = agent["agent_id"]
    report["agent_epoch"] = agent["agent_epoch"]
    report["boot_id"] = agent["boot_id"]
    report["boot_transition"] = "continuing"
    report["report_sequence"] = sequence
    report["report_id"] = report_id_for(sequence)
    report["generated_at"] = observed_at
    restamp(report, observed_at)
    template = report["nodes"][0]
    nodes = [template]
    inventory = list(report["inventory"]["nodes"])
    for ordinal in range(clone_count):
        node_id = clone_node_id(clone_block, ordinal)
        nodes.append(
            node_with_all_series(template, node_id, ordinal, cpu_base + ordinal * 0.125, observed_at)
        )
        entry = copy.deepcopy(inventory[0])
        entry["node_id"] = node_id
        inventory.append(entry)
    report["nodes"] = nodes
    report["inventory"]["nodes"] = inventory
    report["inventory"]["revision"] = inventory_revision
    return report


def sqlite_count(path: Path, sql: str, attempts: int = 5) -> int:
    """A read-only count that tolerates the Server holding the write lock:
    the phases below query the live database, unlike the storage phase."""
    last = None
    for attempt in range(attempts):
        try:
            return int(sqlite_scalar(path, sql))
        except sqlite3.Error as error:
            last = error
            time.sleep(0.2)
    raise BaselineError("read-only count failed: " + sql + ": " + str(last))


class BaselineRun:
    def __init__(self, args) -> None:
        self.args = args
        self.random = random.Random(args.seed)
        self.window_seconds = int(args.hours * 3600)
        self.cadence = int(args.cadence_seconds)
        # The declared window and the raw retention floor are the same 24 hours,
        # so a plan that reached exactly base - window would already be outside
        # the cliff when its oldest Report is written. The plan leaves this guard
        # inside the cliff and every read happens long before it is spent.
        self.cliff_guard_seconds = min(
            PLAN_CLIFF_GUARD_SECONDS, max(self.cadence, self.window_seconds // 4)
        )
        self.rounds = max(2, int((self.window_seconds - self.cliff_guard_seconds) / self.cadence))
        self.pause_after = max(1, self.rounds - int(60 * 60 / self.cadence))
        self.output_root = Path(args.output_root).resolve()
        self.stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
        self.run_dir = self.output_root / self.stamp
        self.state_dir = self.run_dir / "state"
        self.config = self.run_dir / "server.toml"
        self.db_path = self.state_dir / "platpulse.db"
        self.port = int(args.port or free_port())
        self.binary = Path(args.binary).resolve()
        self.fixture = json.loads(FIXTURE.read_text(encoding="utf-8"))
        self.node_id = self.fixture["nodes"][0]["node_id"]
        self.agent = None
        self.cookie = ""
        self.csrf = ""
        self.client = None
        self.server = None
        self.latencies = []
        self.storage = []
        self.dispositions = {}
        self.rejections = {}
        self.rounds_log = []
        self.sequence = 0
        # One fixed timeline for the whole run. Every observation instant is
        # computed once, so the requested window cannot drift while the reports
        # are submitted and the paused stretch stays exactly between its
        # neighbours instead of overlapping them.
        self.base = datetime.now(timezone.utc)
        self.instants = [self.timeline_instant(index) for index in range(self.rounds)]
        self.last_round = None

    def timeline_instant(self, index: int) -> str:
        return (
            self.base
            + timedelta(seconds=-(self.window_seconds - self.cliff_guard_seconds) + index * self.cadence)
        ).strftime(CANONICAL)

    def next_sequence(self) -> int:
        self.sequence += 1
        return self.sequence

    # -- lifecycle ---------------------------------------------------------

    def start_server(self, floor: int) -> None:
        write_config(self.config, self.state_dir, self.port, floor)
        self.server = ServerProcess(self.binary, self.config, self.run_dir)
        self.server.start(self.port)
        self.client = Client(self.port)
        self.cookie, self.csrf = login(self.client)

    def stop_server(self) -> None:
        if self.server is not None:
            self.server.stop()
        self.server = None

    def restart_with_floor(self, floor: int) -> None:
        self.stop_server()
        self.start_server(floor)

    def sample_storage(self, label: str) -> dict:
        row = {
            "label": label,
            "database_bytes": file_bytes(self.db_path),
            "database_only_bytes": self.db_path.stat().st_size if self.db_path.is_file() else 0,
            "at": utc_now(),
        }
        self.storage.append(row)
        return row

    def instant_for(self, index: int) -> str:
        """The declared window runs from now-window to now at the declared
        cadence; index 0 is the oldest instant."""
        return self.instants[index]

    # -- ingestion ---------------------------------------------------------

    def submit(
        self,
        kind: str,
        observed_at: str,
        cpu: float,
        pid: int,
        memory_bytes=None,
        report: dict | None = None,
        nodes: int = 1,
    ) -> dict:
        """Submit one Report on the fixed timeline.

        A carried last-good delivery is the same call with the previous round's
        instant and value: it is a genuinely new Report (new id and sequence)
        whose observation is the one already stored, so the Server must
        recognise it as a restatement rather than a new observation.

        `report` submits an already built payload (the multi-Node phase builds
        its own): the sequence, and therefore the report id, is always fresh, so
        a replayed payload is a new Report carrying the same observations.
        """
        sequence = self.next_sequence()
        if report is None:
            report = build_report(
                self.fixture, self.agent, sequence, observed_at, cpu, pid, report_id_for(sequence), memory_bytes
            )
        else:
            report = copy.deepcopy(report)
            report["report_sequence"] = sequence
            report["report_id"] = report_id_for(sequence)
        status, _, body, elapsed_ms = self.client.request(
            "POST",
            "/api/agent/v1/reports",
            body=json_bytes(report),
            headers={"Authorization": "Bearer " + self.agent["credential"], "Content-Type": "application/json"},
        )
        payload = json.loads(body)
        receipt = payload.get("receipt") if isinstance(payload.get("receipt"), dict) else {}
        disposition = payload.get("disposition") or receipt.get("disposition")
        reason = (
            receipt.get("rejection")
            or receipt.get("reason")
            or receipt.get("detail")
            or receipt.get("error_code")
            or payload.get("reason")
        )
        self.latencies.append(elapsed_ms)
        key = str(disposition)
        self.dispositions[key] = self.dispositions.get(key, 0) + 1
        if key not in ("accepted", "partially_accepted"):
            refusal = str(reason)
            self.rejections[refusal] = self.rejections.get(refusal, 0) + 1
        self.rounds_log.append(
            {
                "kind": kind,
                "nodes": nodes,
                "sequence": sequence,
                "instant": observed_at,
                "disposition": disposition,
                "reason": reason,
                "http_status": status,
            }
        )
        self.last_round = {
            "sequence": sequence,
            "instant": observed_at,
            "cpu": cpu,
            "memory": memory_bytes if memory_bytes is not None else 536870912 + (sequence % 97),
        }
        return {"disposition": disposition, "reason": reason, "elapsed_ms": elapsed_ms}

    def build_agent(self) -> None:
        # Enrollment and Owner creation touch the database from the CLI, so the
        # Server is stopped for them exactly as an Operator would run them.
        write_config(self.config, self.state_dir, self.port, CLEARED_FLOOR)
        command([self.binary, "init", "--config", self.config])
        # The Node in a Report is only applied once its Network is registered,
        # exactly as the e2e harness declares it.
        command(
            [
                self.binary, "network", "create", "--config", self.config,
                "--key", NETWORK_KEY, "--display-name", "PlatON Mainnet",
                "--genesis-hash", NETWORK_GENESIS,
                "--chain-id", "210425", "--p2p-network-id", "210425",
                "--address-hrp", "lat",
            ]
        )
        command(
            [self.binary, "owner", "create", "--config", self.config, "--username", OWNER_USERNAME],
            input_text=OWNER_PASSWORD + "\n",
        )
        token = create_enrollment_token(self.binary, self.config)
        self.start_server(CLEARED_FLOOR)
        self.agent = enroll_agent(token, self.client)
        self.node_id = self.fixture["nodes"][0]["node_id"]

    # -- phases ------------------------------------------------------------

    def phase_load(self) -> dict:
        started = time.perf_counter()
        skipped = 0
        for index in range(self.pause_after):
            cpu = self.cpu_for(index)
            self.submit("load", self.instant_for(index), cpu, 12000 + index)
            if index and index % 250 == 0:
                self.sample_storage("load+" + str(index))
        first_half = time.perf_counter() - started

        # Low-space protection: the declared floor is one no real filesystem
        # clears, so the Server adopts protection and every optional sample of
        # the next Reports is skipped and recorded as a pause.
        self.restart_with_floor(MAX_PERSISTED_BYTES)
        paused_instants = [
            self.instant_for(self.pause_after + offset) for offset in range(PAUSE_ROUNDS)
        ]
        for offset, observed_at in enumerate(paused_instants):
            self.submit("pause", observed_at, self.cpu_for(self.pause_after + offset), 13000 + offset)
            skipped += 1
        protected_storage = self.sample_storage("protected")

        # Releasing the floor resumes collection on the same series.
        self.restart_with_floor(CLEARED_FLOOR)
        resumed = time.perf_counter()
        for index in range(self.pause_after + PAUSE_ROUNDS, self.rounds):
            cpu = self.cpu_for(index)
            self.submit("load", self.instant_for(index), cpu, 14000 + index)
        second_half = time.perf_counter() - resumed
        self.sample_storage("load-complete")
        accepted = [
            entry["instant"]
            for entry in self.rounds_log
            if entry["kind"] == "load" and str(entry["disposition"]) in ("accepted", "partially_accepted")
        ]
        return {
            "declared_window_seconds": self.window_seconds,
            "declared_cadence_seconds": self.cadence,
            "planned_rounds": self.rounds,
            "stored_rounds": len(set(accepted)),
            "accepted_instants": sorted(set(accepted)),
            "rejections": self.rejections,
            "paused_rounds": skipped,
            "paused_seconds": skipped * self.cadence,
            "minimum_silence_seconds": max(3 * self.cadence, 120),
            "paused_instants": paused_instants,
            "wall_seconds_before_pause": round(first_half, 3),
            "wall_seconds_after_pause": round(second_half, 3),
            "dispositions": self.dispositions,
            "write_latency_ms": {
                "p50_ms": round(percentile(self.latencies, 0.5), 3),
                "p95_ms": round(percentile(self.latencies, 0.95), 3),
                "max_ms": round(max(self.latencies), 3),
            },
            "protected_storage": protected_storage,
        }

    def cpu_for(self, index: int) -> float:
        if index == self.rounds // 2:
            return 88.5
        return 2.5 + (index % 7) * 0.25

    def phase_restatements(self) -> dict:
        """A replay and a correction of the newest stored observation, each
        carried by a genuinely new Report."""
        newest = self.last_round
        observed_at = newest["instant"]
        stored = newest["cpu"]
        span = (self.window_from(), self.window_to(), 5000)
        before, _, _ = self.read(*span)
        self.submit("replay", observed_at, stored, 15000, memory_bytes=newest["memory"])
        replayed, _, _ = self.read(*span)
        corrected_value = round(stored + 7.5, 3)
        self.submit("correction", observed_at, corrected_value, 15001, memory_bytes=newest["memory"])
        corrected, _, _ = self.read(*span)
        return {
            "instant": observed_at,
            "stored_value": stored,
            "corrected_value": corrected_value,
            "before": self.ledger(before),
            "after_replay": self.ledger(replayed),
            "after_correction": self.ledger(corrected),
        }

    def phase_release(self) -> dict:
        """An observation older than the raw floor: the Server stores it,
        counts it, and the retention cleanup releases the row."""
        observed_at = instant(-(self.window_seconds + 2 * 3600))
        span = (self.window_from(), self.window_to(), 5000)
        before, _, _ = self.read(*span)
        self.submit("release", observed_at, 41.5, 16000)
        self.restart_with_floor(CLEARED_FLOOR)
        after, _, _ = self.read(*span)
        return {
            "released_instant": observed_at,
            "before": self.ledger(before),
            "after": self.ledger(after),
            "released_instant_stored": any(item["observedAt"] == observed_at for item in after["items"]),
            "sampled_count_after": after["series"]["sampledCount"],
        }

    def ledger(self, payload: dict) -> dict:
        series = payload["series"]
        return {
            "observed": series["observed"],
            "observationCount": series["observationCount"],
            "replayedCount": series["replayedCount"],
            "correctedCount": series["correctedCount"],
            "sampledCount": series["sampledCount"],
            "coverageSeconds": series["coverageSeconds"],
            "firstObservedAt": series.get("firstObservedAt"),
            "lastObservedAt": series.get("lastObservedAt"),
        }

    def read(
        self,
        from_instant: str,
        to_instant: str,
        limit: int,
        metric: str = "process_cpu_percent",
        node_id: str | None = None,
    ):
        query = "&from=" + from_instant + "&to=" + to_instant + "&limit=" + str(limit)
        return read_history(self.client, self.cookie, node_id or self.node_id, metric, query)

    def window_from(self) -> str:
        return self.instants[0]

    def window_to(self) -> str:
        return instant(1)

    def phase_reads(self) -> dict:
        reads = {}
        for label, from_instant, limit in (
            ("24h", self.window_from(), 5000),
            ("6h", instant(-6 * 3600), 5000),
            ("1h", instant(-3600), 5000),
            ("24h_limited", self.window_from(), 10),
        ):
            to_instant = self.window_to()
            payload, elapsed_ms, payload_bytes = self.read(from_instant, to_instant, limit)
            reads[label] = {
                "requested_from": from_instant,
                "requested_to": to_instant,
                "requested_span_seconds": int(
                    (parse_instant(to_instant) - parse_instant(from_instant)).total_seconds()
                ),
                "window_seconds": payload["windowSeconds"],
                "items": len(payload["items"]),
                "truncated": payload["truncated"],
                "availability": payload.get("availability"),
                "gaps": [
                    {
                        "kind": gap["kind"],
                        "from": gap["from"],
                        "to": gap["to"],
                        "seconds": gap["seconds"],
                        "skippedCount": gap.get("skippedCount"),
                    }
                    for gap in payload["gaps"]
                ],
                "latency_ms": round(elapsed_ms, 3),
                "payload_bytes": payload_bytes,
                "series": self.ledger(payload),
                "observed_instants": [item["observedAt"] for item in payload["items"]],
                "newest_observed_at": payload["items"][-1]["observedAt"] if payload["items"] else None,
                "newest_value": payload["items"][-1]["value"] if payload["items"] else None,
                "oldest_observed_at": payload["items"][0]["observedAt"] if payload["items"] else None,
                "delays_seconds": sorted({item.get("delaySeconds") for item in payload["items"]}, key=lambda value: (value is None, value)),
                "clock_suspect": any(item["clockSuspect"] for item in payload["items"]),
            }
            if label == "24h":
                reads[label]["coverage_recomputed"] = self.recompute_coverage(payload["items"])
                reads[label]["extremes"] = {
                    "min": min((item["value"] for item in payload["items"]), default=None),
                    "max": max((item["value"] for item in payload["items"]), default=None),
                }
        never_reported, never_ms, never_bytes = self.read(
            self.window_from(), self.window_to(), 5000, NEVER_REPORTED_SERIES
        )
        # No from and no to: the Server must fall back to the declared 24 hour
        # floor and answer the whole series inside it.
        default_payload, default_ms, default_bytes = read_history(
            self.client, self.cookie, self.node_id, "process_cpu_percent", "&limit=5000"
        )
        reads["default_window"] = {
            "window_seconds": default_payload["windowSeconds"],
            "items": len(default_payload["items"]),
            "latency_ms": round(default_ms, 3),
            "payload_bytes": default_bytes,
        }
        reads["never_reported"] = {
            "metric": NEVER_REPORTED_SERIES,
            "items": len(never_reported["items"]),
            "gaps": len(never_reported["gaps"]),
            "series": self.ledger(never_reported),
            "latency_ms": round(never_ms, 3),
            "payload_bytes": never_bytes,
        }
        refusals = {}
        status, _, body, _ = admin_get(
            self.client, self.cookie, history_url(self.node_id, "carrier_pigeons")
        )
        refusals["invalid_metric"] = {"status": status, "code": json.loads(body)["error"]["code"]}
        status, _, body, _ = admin_get(
            self.client,
            self.cookie,
            history_url(self.node_id, "process_cpu_percent", "&from=" + instant(-3600) + "&to=" + instant(-7200)),
        )
        refusals["invalid_range"] = {"status": status, "code": json.loads(body)["error"]["code"]}
        status, _, body, _ = admin_get(
            self.client,
            self.cookie,
            history_url("0195f2a1-00ff-40ff-80ff-0000000000ff", "process_cpu_percent"),
        )
        refusals["unknown_node"] = {"status": status, "code": json.loads(body)["error"]["code"]}
        # Issue #213 asked this same 30 hour old range and expected availability
        # "unavailable", which is what the Server answered while availability
        # followed the raw window's own cutoff: the whole range sat outside the
        # released raw history. Issue #214 moved the question to the investigation
        # horizon (crates/platpulse-server/src/http/admin.rs:4643-4649), so that
        # range is now inside what the Server can answer and its availability is
        # null; only a range ending before now - SERVER_HISTORY_HORIZON_DAYS days is
        # still called unavailable. Both branches are asked here and both are
        # asserted by the check that keeps the issue #213 name.
        released_from = instant(-30 * 3600)
        released_to = instant(-29 * 3600)
        status, _, body, _ = admin_get(
            self.client,
            self.cookie,
            history_url(
                self.node_id,
                "process_cpu_percent",
                "&from=" + released_from + "&to=" + released_to,
            ),
        )
        released = json.loads(body)
        refusals["released_range"] = {
            "status": status,
            "availability": released.get("availability"),
            "requestedFrom": released.get("requestedFrom"),
            "requestedFromParam": released_from,
            "requestedToParam": released_to,
            "effectiveFrom": released.get("from"),
            "effectiveTo": released.get("to"),
            "historyHorizonDays": released.get("historyHorizonDays"),
            "rawRetentionDays": released.get("rawRetentionDays"),
            "grain": released.get("grain"),
            "items": len(released.get("items", [])),
        }
        beyond_from = instant(-31 * 86400)
        beyond_to = instant(-SERVER_HISTORY_HORIZON_DAYS * 86400 - 3600)
        status, _, body, _ = admin_get(
            self.client,
            self.cookie,
            history_url(
                self.node_id,
                "process_cpu_percent",
                "&from=" + beyond_from + "&to=" + beyond_to,
            ),
        )
        beyond = json.loads(body)
        refusals["beyond_horizon"] = {
            "status": status,
            "availability": beyond.get("availability"),
            "requestedFrom": beyond.get("requestedFrom"),
            "requestedFromParam": beyond_from,
            "requestedToParam": beyond_to,
            "effectiveFrom": beyond.get("from"),
            "effectiveTo": beyond.get("to"),
            "historyHorizonDays": beyond.get("historyHorizonDays"),
            "rawRetentionDays": beyond.get("rawRetentionDays"),
            "grain": beyond.get("grain"),
            "items": len(beyond.get("items", [])),
        }
        reads["refusals"] = refusals
        return reads

    def planned_coverage(self) -> dict:
        """The coverage the plan itself proves, without asking the Server.

        This oracle may not restate the Server rule (three observed cadences,
        floor 120 seconds): a number recomputed from the returned samples can
        only confirm that the script and the Server agree, never that either is
        right. What the script knows on its own is the timeline it planned and
        the silence it injected: `rounds` instants at the declared cadence, minus
        the PAUSE_ROUNDS instants whose samples were deliberately skipped, which
        leaves exactly two contiguous stretches of stored observations. Every
        interior delta of a stretch is one cadence, and the straddling delta is
        (PAUSE_ROUNDS + 1) cadences = 210s, deliberately longer than the Server
        gap threshold of max(3 x cadence, 120s) = 120s, so the silence has to be
        reported as one gap and may not be counted as proven coverage.
        """
        stored = self.rounds - PAUSE_ROUNDS
        stretches = 2 if PAUSE_ROUNDS else 1
        return {
            "planned_rounds": self.rounds,
            "planned_cadence_seconds": self.cadence,
            "planned_stored_rounds": stored,
            "planned_stretches": stretches,
            "planned_proven_seconds": (stored - stretches) * self.cadence,
            "planned_straddle_seconds": (PAUSE_ROUNDS + 1) * self.cadence,
            "server_gap_threshold_seconds": max(3 * self.cadence, 120),
            "pause_from": self.instants[self.pause_after - 1],
            "pause_to": self.instants[self.pause_after + PAUSE_ROUNDS],
        }

    def metric_counts(self, scope: str) -> dict:
        """Live row counts for one set of Nodes, with the cutoff the Server's own
        raw retention policy uses (observed_at < now - 24 hours), not the window
        this run asks the read path for. The Server keeps running here, so the
        counts are read read-only while Reports are still arriving."""
        cutoff = instant(-SERVER_RAW_RETENTION_SECONDS)
        where = "node_metric_samples WHERE " + scope
        return {
            "total": sqlite_count(self.db_path, "SELECT COUNT(*) FROM " + where),
            "inside": sqlite_count(
                self.db_path,
                "SELECT COUNT(*) FROM " + where + " AND observed_at >= '" + cutoff + "'",
            ),
            "expired": sqlite_count(
                self.db_path,
                "SELECT COUNT(*) FROM " + where + " AND observed_at < '" + cutoff + "'",
            ),
        }

    def phase_multi_node(self) -> dict:
        """The per-Report Node sample cleanup budget under a multi-Node arrival.

        Part 1 is the specified burst: 32 cloned Nodes on a 2400s cadence whose
        newest instant is a few minutes old, so the 48 Reports span more than the
        24 hour window and the rounds behind the arrival are already outside it.
        Nothing expires while it runs, which is what makes the pure replay
        decisive: the same values at the same instants may not change one row.

        Part 2 supplies the arrival rate that actually separates 2048 from 128.
        Its Nodes sit a few seconds inside the same cliff, so each accepted
        Report adds a whole Report worth of rows while the reports themselves
        keep pushing that many rows across the cutoff: one cleanup ends up owing
        more rows than the old per-Report bound could pay, the backlog survives,
        and a following pure replay is forced to release rows instead of none.
        A single anchor Report per 128 Nodes stays an hour inside the window, so
        the stored set is never empty and "unchanged" means something.
        """
        started = time.perf_counter()
        # The horizon these rows are stored against is the Server's own raw
        # retention window, so the arithmetic below holds for any --hours.
        retention_seconds = SERVER_RAW_RETENTION_SECONDS

        def clone_scope(block: int, count: int) -> str:
            ids = [clone_node_id(block, ordinal) for ordinal in range(count)]
            return "node_id IN (" + ",".join("'" + node_id + "'" for node_id in ids) + ")"

        def percentile_row(values: list) -> dict:
            return {
                "p50_ms": round(percentile(values, 0.5), 3),
                "p95_ms": round(percentile(values, 0.95), 3),
                "max_ms": round(max(values), 3),
            }

        def tally(counts: dict, key) -> dict:
            counts[key] = counts.get(key, 0) + 1
            return counts

        # -- Part 1: the specified burst ----------------------------------
        base = datetime.now(timezone.utc)
        part1_instants = [
            (base - timedelta(seconds=MULTI_NODE_NEWEST_AGE_SECONDS + offset * MULTI_NODE_CADENCE_SECONDS)).strftime(CANONICAL)
            for offset in range(MULTI_NODE_REPORTS)
        ]
        rows_per_report = MULTI_NODE_CLONE_COUNT * MULTI_NODE_SERIES_PER_NODE
        in_window_rounds = sum(
            1
            for offset in range(MULTI_NODE_REPORTS)
            if MULTI_NODE_NEWEST_AGE_SECONDS + offset * MULTI_NODE_CADENCE_SECONDS < retention_seconds
        )
        planned_rows = in_window_rounds * rows_per_report
        part1_scope = clone_scope(MULTI_NODE_CLONE_BLOCK, MULTI_NODE_CLONE_COUNT)
        part1_latencies = []
        part1_dispositions = {}
        newest_report = None
        for offset in reversed(range(MULTI_NODE_REPORTS)):
            observed_at = part1_instants[offset]
            report = multi_node_report(
                self.fixture,
                self.agent,
                0,
                observed_at,
                MULTI_NODE_CLONE_BLOCK,
                MULTI_NODE_CLONE_COUNT,
                self.cpu_for(offset),
                MULTI_NODE_INVENTORY_REVISION,
            )
            outcome = self.submit(
                "multi_node", observed_at, 0.0, 0, report=report, nodes=MULTI_NODE_CLONE_COUNT + 1
            )
            part1_latencies.append(outcome["elapsed_ms"])
            tally(part1_dispositions, str(outcome["disposition"]))
            if offset == 0:
                newest_report = report
        part1_wall = round(time.perf_counter() - started, 3)
        part1_counts = self.metric_counts(part1_scope)
        part1 = {
            "clone_nodes": MULTI_NODE_CLONE_COUNT,
            "reports": MULTI_NODE_REPORTS,
            "cadence_seconds": MULTI_NODE_CADENCE_SECONDS,
            "newest_age_seconds": MULTI_NODE_NEWEST_AGE_SECONDS,
            "retention_seconds": retention_seconds,
            "planned_in_window_rounds": in_window_rounds,
            "planned_out_of_window_rounds": MULTI_NODE_REPORTS - in_window_rounds,
            "planned_rows_per_report": rows_per_report,
            "planned_stored_rows": planned_rows,
            "accepted_reports": part1_dispositions.get("accepted", 0) + part1_dispositions.get("partially_accepted", 0),
            "dispositions": part1_dispositions,
            "latency_ms": percentile_row(part1_latencies),
            "wall_seconds": part1_wall,
            "node_rows": sqlite_count(self.db_path, "SELECT COUNT(*) FROM nodes WHERE " + part1_scope),
            "series_ledger_rows": sqlite_count(
                self.db_path, "SELECT COUNT(*) FROM node_metric_series_state WHERE " + part1_scope
            ),
            "stored_rows": part1_counts["total"],
            "inside_rows": part1_counts["inside"],
            "expired_rows": part1_counts["expired"],
        }
        part1["replay"] = {"before": part1_counts, "after": None}
        part1["replay"]["outcome"] = self.submit(
            "multi_node_replay", part1_instants[0], 0.0, 0, report=newest_report, nodes=MULTI_NODE_CLONE_COUNT + 1
        )["disposition"]
        part1["replay"]["after"] = self.metric_counts(part1_scope)

        # -- Part 2: the arrival rate that separates 2048 from 128 ---------
        drain_started = time.perf_counter()
        drain_base = datetime.now(timezone.utc)
        drain_scope = clone_scope(DRAIN_CLONE_BLOCK, DRAIN_CLONE_COUNT)
        drain_rows_per_report = DRAIN_CLONE_COUNT * MULTI_NODE_SERIES_PER_NODE
        drain_latencies = []
        drain_dispositions = {}
        anchor_instant = (drain_base - timedelta(seconds=retention_seconds - DRAIN_ANCHOR_AGE_SECONDS)).strftime(
            CANONICAL
        )
        anchor_report = multi_node_report(
            self.fixture, self.agent, 0, anchor_instant, DRAIN_CLONE_BLOCK, DRAIN_CLONE_COUNT, 12.5,
            DRAIN_INVENTORY_REVISION,
        )
        anchor_outcome = self.submit(
            "drain_anchor", anchor_instant, 0.0, 0, report=anchor_report, nodes=DRAIN_CLONE_COUNT + 1
        )
        drain_latencies.append(anchor_outcome["elapsed_ms"])
        tally(drain_dispositions, str(anchor_outcome["disposition"]))
        previous = self.metric_counts(drain_scope)
        additions = []
        releases = []
        totals = []
        last_instant = anchor_instant
        last_report = anchor_report
        for offset in range(DRAIN_REPORTS):
            last_instant = (
                drain_base
                + timedelta(seconds=offset)
                - timedelta(seconds=retention_seconds - DRAIN_INSIDE_CLIFF_SECONDS)
            ).strftime(CANONICAL)
            last_report = multi_node_report(
                self.fixture, self.agent, 0, last_instant, DRAIN_CLONE_BLOCK, DRAIN_CLONE_COUNT, 20.0,
                DRAIN_INVENTORY_REVISION,
            )
            outcome = self.submit(
                "drain", last_instant, 0.0, 0, report=last_report, nodes=DRAIN_CLONE_COUNT + 1
            )
            drain_latencies.append(outcome["elapsed_ms"])
            tally(drain_dispositions, str(outcome["disposition"]))
            # Rows are attributed by their exact instant, so no row crossing the
            # cutoff between two reads can be mistaken for a stored one.
            added = sqlite_count(
                self.db_path,
                "SELECT COUNT(*) FROM node_metric_samples WHERE "
                + drain_scope
                + " AND observed_at = '" + last_instant + "'",
            )
            current = self.metric_counts(drain_scope)
            additions.append(added)
            releases.append(added - (current["total"] - previous["total"]))
            totals.append(current["total"])
            previous = current
            if offset < DRAIN_REPORTS - 1:
                target = drain_started + (offset + 1) * DRAIN_PACE_SECONDS
                time.sleep(max(0.0, target - time.perf_counter()))
        drain_wall = round(time.perf_counter() - drain_started, 3)
        drain_before_replay = self.metric_counts(drain_scope)
        drain_replay = self.submit(
            "drain_replay", last_instant, 0.0, 0, report=last_report, nodes=DRAIN_CLONE_COUNT + 1
        )
        drain_after_replay = self.metric_counts(drain_scope)
        return {
            "part1": part1,
            "drain": {
                "clone_nodes": DRAIN_CLONE_COUNT,
                "reports": DRAIN_REPORTS,
                "pace_seconds": DRAIN_PACE_SECONDS,
                "inside_cliff_seconds": DRAIN_INSIDE_CLIFF_SECONDS,
                "anchor_age_seconds": DRAIN_ANCHOR_AGE_SECONDS,
                "rows_per_report": drain_rows_per_report,
                "rows_added_per_report": additions,
                "rows_released_per_report": releases,
                "max_released_by_one_cleanup": max(releases, default=0),
                "total_released": sum(releases),
                "stored_rows_per_report": totals,
                "stored_rows_after_burst": drain_before_replay["total"],
                "expired_rows_after_burst": drain_before_replay["expired"],
                "backlog_floor_under_previous_bound": max(
                    0, sum(additions) - PREVIOUS_NODE_METRIC_CLEANUP_BATCH * len(additions)
                ),
                "dispositions": drain_dispositions,
                "latency_ms": percentile_row(drain_latencies),
                "wall_seconds": drain_wall,
                "replay": {
                    "outcome": drain_replay["disposition"],
                    "before": drain_before_replay,
                    "after": drain_after_replay,
                },
            },
            "bound_under_test": NODE_METRIC_CLEANUP_BATCH,
            "falsified_bound": PREVIOUS_NODE_METRIC_CLEANUP_BATCH,
            "wall_seconds": round(time.perf_counter() - started, 3),
        }

    def recompute_coverage(self, items: list) -> dict:
        """Informational cross-check only; the oracle is planned_coverage above.
        The coverage the returned samples prove on their own: every pair
        closer than the Server's gap threshold is proven, and the threshold is
        the same rule the Server documents (three observed cadences, floor 120
        seconds)."""
        instants = [parse_instant(item["observedAt"]) for item in items]
        deltas = [int((right - left).total_seconds()) for left, right in zip(instants, instants[1:])]
        positive = [delta for delta in deltas if delta > 0]
        cadence = min(positive) if positive else 0
        threshold = max(cadence * 3, 120)
        proven = sum(delta for delta in positive if delta < threshold)
        return {"observed_cadence_seconds": cadence, "gap_threshold_seconds": threshold, "proven_seconds": proven}

    # -- the aggregate tiers (issue #214) ----------------------------------
    #
    # The tier phase is deliberately the last one: it seeds a month of hourly
    # Reports that the raw window does not keep but both aggregate tiers do, and
    # it needs the Server stopped to take a page-level storage baseline before and
    # after. phase_storage already stopped it, so the phase begins with a
    # checkpointed database and restarts the Server itself.

    def tier_instants(self, base: datetime) -> list[datetime]:
        """The declared hour grid, oldest first.

        The newest instant is half an hour old (TIER_NEWEST_AGE_SECONDS), so the
        Server's moving raw cutoff (now - 24 hours) and both tier floors (now - 7
        days, now - 30 days) pass no grid instant for the whole phase: the set a
        read sees cannot shift while the reads, the paging walk and the planted
        probes run.
        """
        newest = base - timedelta(seconds=TIER_NEWEST_AGE_SECONDS)
        return [
            newest - timedelta(seconds=(TIER_REPORTS - 1 - index) * TIER_CADENCE_SECONDS)
            for index in range(TIER_REPORTS)
        ]

    def tier_cpu(self, index: int) -> float:
        """The value of one hour as a function of its index alone: a ladder from
        TIER_VALUE_BASE to TIER_VALUE_BASE + (STEP_COUNT - 1) * STEP and back, so
        a tier's floor and ceiling are known before it is read, with one planted
        spike at TIER_SPIKE_INDEX that no bucket may round away."""
        if index == TIER_SPIKE_INDEX:
            return TIER_SPIKE_CPU
        return TIER_VALUE_BASE + (index % TIER_VALUE_STEP_COUNT) * TIER_VALUE_STEP

    def tier_memory(self, index: int) -> int:
        return 2147483648 + (index % 11) * 67108864

    def tier_report(
        self, node_id: str, observed_at: str, cpu: float, memory_bytes: int, inventory_revision: int
    ) -> dict:
        """A Report whose only Node is the given id, carrying the fixture's two
        series (process_cpu_percent, process_memory_percent) at observed_at.

        The inventory revision is declared, not inherited from the fixture: the
        fixture's revision 1 belongs to its own single-Node inventory, and a
        changed inventory declared at an accepted revision is rejected. The
        placeholder sequence and report id do not matter either: BaselineRun.submit
        replaces both with the run's own next_sequence() before it sends.
        """
        report = copy.deepcopy(self.fixture)
        for entry in report["inventory"]["nodes"]:
            entry["node_id"] = node_id
        report["inventory"]["revision"] = inventory_revision
        report["nodes"][0]["node_id"] = node_id
        return build_report(report, self.agent, 0, observed_at, cpu, 21000, report_id_for(0), memory_bytes=memory_bytes)

    def grain_row_counts(self, scope: str = "") -> dict:
        """The rows each tier holds, the series they describe and their buckets.

        The scope is a bare SQL predicate ("node_id = '...'") or "" for the whole
        table, and is combined with the grain predicate exactly the way the
        Server's own tier cleanup does (retention.rs:237/247).
        """
        counts = {}
        for grain_seconds, label in (
            (TIER_ONE_MINUTE_SECONDS, "one_minute"),
            (TIER_FIVE_MINUTE_SECONDS, "five_minute"),
        ):
            where = ["grain_seconds = " + str(grain_seconds)]
            if scope:
                where.append(scope)
            counts[label] = sqlite_count(
                self.db_path,
                "SELECT COUNT(*) FROM node_metric_aggregates WHERE " + " AND ".join(where),
            )
        counts["total"] = counts["one_minute"] + counts["five_minute"]
        where = " WHERE " + scope if scope else ""
        counts["distinct_series"] = sqlite_count(
            self.db_path,
            "SELECT COUNT(*) FROM (SELECT DISTINCT node_id, metric FROM node_metric_aggregates" + where + ")",
        )
        counts["distinct_buckets"] = sqlite_count(
            self.db_path,
            "SELECT COUNT(*) FROM (SELECT DISTINCT node_id, metric, bucket_start FROM node_metric_aggregates" + where + ")",
        )
        return counts

    def bucket_row(self, node_id: str, metric: str, grain_seconds: int, bucket_start: str) -> dict:
        """The one bucket an instant belongs to, looked up by its own start."""
        sql = (
            "SELECT sample_count, min_value, max_value, last_value, first_observed_at, last_observed_at "
            "FROM node_metric_aggregates WHERE node_id = '" + node_id + "'"
            " AND metric = '" + metric + "'"
            " AND grain_seconds = " + str(grain_seconds) +
            " AND bucket_start = '" + bucket_start + "'"
        )
        try:
            rows = sqlite_rows(self.db_path, sql)
        except sqlite3.Error as error:
            return {"found": 0, "row": None, "error": str(error)}
        return {"found": len(rows), "row": rows[0] if rows else None, "error": None}

    def aggregate_footprint(self) -> dict:
        """What node_metric_aggregates and its indexes really occupy.

        PRAGMA page_count is the whole database; dbstat attributes pages to one
        object, which is what a bytes per bucket figure needs. A SQLite build
        without dbstat reports available false rather than a guess.
        """
        objects = (
            "node_metric_aggregates",
            "sqlite_autoindex_node_metric_aggregates_1",
            "node_metric_aggregates_expiry_idx",
        )
        try:
            pages = {}
            for name in objects:
                pages[name] = sqlite_scalar(
                    self.db_path,
                    "SELECT SUM(pgsize) FROM dbstat WHERE name = '" + name + "'",
                )
        except sqlite3.Error as error:
            return {
                "available": False,
                "reason": str(error),
                "objects": {},
                "bytes": None,
                "pages": None,
                "page_size": None,
                "page_count_database": None,
            }
        page_size = sqlite_scalar(self.db_path, "PRAGMA page_size")
        total_bytes = sum(value for value in pages.values() if value)
        return {
            "available": True,
            "reason": None,
            "objects": pages,
            "bytes": total_bytes,
            "pages": round(total_bytes / page_size, 3) if page_size else None,
            "page_size": page_size,
            "page_count_database": sqlite_scalar(self.db_path, "PRAGMA page_count"),
        }

    def tier_read(self, label: str, from_instant: str, to_instant: str, node_id: str, limit: int) -> dict:
        """One metric-history read, reduced to the fields the tier checks assert.

        Every field is JSON-safe: which grains one answer mixed, how many points
        came from a bucket rather than from a stored raw sample, the segments the
        Server consulted, the envelopes the buckets carried, the gaps it named and
        how long the answer took to arrive.
        """
        requested_span = int((parse_instant(to_instant) - parse_instant(from_instant)).total_seconds())
        payload, elapsed_ms, payload_bytes = self.read(from_instant, to_instant, limit, node_id=node_id)
        items = payload.get("items") or []
        grains: dict[str, dict] = {}
        observed_instants = []
        raw_instants = []
        for item in items:
            grain = item.get("grain")
            found = grains.setdefault(
                grain,
                {
                    "points": 0,
                    "minValue": None,
                    "maxValue": None,
                    "sampleCount": 0,
                    "withoutSamples": 0,
                    "sources": [],
                },
            )
            found["points"] += 1
            low = item.get("minValue")
            high = item.get("maxValue")
            if low is not None:
                found["minValue"] = low if found["minValue"] is None else min(found["minValue"], low)
            if high is not None:
                found["maxValue"] = high if found["maxValue"] is None else max(found["maxValue"], high)
            count = item.get("sampleCount")
            if count is None:
                found["withoutSamples"] += 1
            else:
                found["sampleCount"] += count
            source = item.get("source")
            if source not in found["sources"]:
                found["sources"].append(source)
            observed_instants.append(item.get("observedAt"))
            if item.get("grain") == "raw":
                raw_instants.append(item.get("observedAt"))
        gaps = payload.get("gaps") or []
        series = payload.get("series") or {}
        return {
            "label": label,
            "requested_from": from_instant,
            "requested_to": to_instant,
            "requested_span_seconds": requested_span,
            "availability": payload.get("availability"),
            "effective_from": payload.get("from"),
            "effective_to": payload.get("to"),
            "history_horizon_days": payload.get("historyHorizonDays"),
            "raw_retention_days": payload.get("rawRetentionDays"),
            "grain": payload.get("grain"),
            "window_seconds": payload.get("windowSeconds"),
            "items": len(items),
            "grains": {grain: found for grain, found in sorted(grains.items(), key=lambda pair: str(pair[0]))},
            "sources": sorted({source for found in grains.values() for source in found["sources"]}),
            "aggregate_points": sum(found["points"] for grain, found in grains.items() if grain != "raw"),
            "without_samples": sum(found["withoutSamples"] for found in grains.values()),
            "segments": [
                {
                    "from": segment.get("from"),
                    "to": segment.get("to"),
                    "grain": segment.get("grain"),
                    "source": segment.get("source"),
                    "pointCount": segment.get("pointCount"),
                    "truncated": bool(segment.get("truncated")),
                }
                for segment in payload.get("segments") or []
            ],
            "truncated": bool(payload.get("truncated")),
            "continuation": payload.get("continuation"),
            "coverage_seconds": payload.get("coverageSeconds"),
            "observation_count": series.get("observationCount"),
            "sampled_count": series.get("sampledCount"),
            "replayed_count": series.get("replayedCount"),
            "corrected_count": series.get("correctedCount"),
            "series_coverage_seconds": series.get("coverageSeconds"),
            "series_window_seconds": series.get("windowSeconds"),
            "first_observed_at": series.get("firstObservedAt"),
            "last_observed_at": series.get("lastObservedAt"),
            "gaps": len(gaps),
            "gap_kinds": sorted({gap.get("kind") for gap in gaps}),
            "gap_max_seconds": max((gap.get("seconds") or 0) for gap in gaps) if gaps else 0,
            "gap_seconds_total": sum((gap.get("seconds") or 0) for gap in gaps),
            "latency_ms": round(elapsed_ms, 3),
            "payload_bytes": payload_bytes,
            "observed_instants": observed_instants,
            "raw_instants": raw_instants,
        }

    def tier_walk(self, node_id: str, from_instant: str, to_instant: str, expected: list) -> dict:
        """Page the whole window at TIER_PAGE_LIMIT and follow continuation home.

        The walk asserts on itself page by page - every page is strictly older
        than the cursor that produced it, the cursor only ever moves backwards,
        the last page is untruncated with no continuation - and then against the
        single read's coordinates, so a repeated or skipped coordinate cannot hide
        in an average. The loop is bounded, so a route that always claims more
        evidence fails the check instead of hanging the run.
        """
        bound = 4 * ((TIER_REPORTS + TIER_PAGE_LIMIT - 1) // TIER_PAGE_LIMIT)
        cursor = None
        walked = []
        seen = set()
        repeats = 0
        out_of_order = 0
        cursor_not_older = 0
        without_samples = 0
        latency = []
        payload_bytes = 0
        grain_counts: dict = {}
        pages = []
        last_truncated = None
        last_continuation = "unset"
        last_items = 0
        while True:
            query = "&from=" + from_instant + "&to=" + to_instant + "&limit=" + str(TIER_PAGE_LIMIT)
            if cursor:
                query += "&before=" + cursor
            payload, elapsed_ms, page_bytes = read_history(
                self.client, self.cookie, node_id, "process_cpu_percent", query
            )
            items = payload.get("items") or []
            page_instants = [item.get("observedAt") for item in items]
            latency.append(round(elapsed_ms, 3))
            payload_bytes += page_bytes
            for item in items:
                grain = item.get("grain")
                grain_counts[grain] = grain_counts.get(grain, 0) + 1
                if item.get("sampleCount") is None:
                    without_samples += 1
                if item.get("observedAt") in seen:
                    repeats += 1
                seen.add(item.get("observedAt"))
                walked.append(item.get("observedAt"))
            if cursor and page_instants and max(page_instants) >= cursor:
                out_of_order += 1
            pages.append(
                {
                    "page": len(pages) + 1,
                    "items": len(items),
                    # The route returns each page oldest first, so the extremes are
                    # taken rather than assumed from the ends.
                    "newest": max(page_instants) if page_instants else None,
                    "oldest": min(page_instants) if page_instants else None,
                    "truncated": bool(payload.get("truncated")),
                    "continuation": payload.get("continuation"),
                    "elapsed_ms": round(elapsed_ms, 3),
                    "payload_bytes": page_bytes,
                }
            )
            last_truncated = bool(payload.get("truncated"))
            last_continuation = payload.get("continuation")
            last_items = len(items)
            if not last_truncated:
                break
            next_cursor = payload.get("continuation")
            if not next_cursor or not page_instants:
                break
            if cursor and next_cursor >= cursor:
                cursor_not_older += 1
            cursor = next_cursor
            if len(pages) >= bound:
                break
        expected_set = set(expected)
        seen_set = {value for value in seen if value}
        return {
            "limit": TIER_PAGE_LIMIT,
            "pages": len(pages),
            "bound": bound,
            "bounded": len(pages) >= bound and bool(last_truncated),
            "items": len(walked),
            "distinct_coordinates": len(seen_set),
            "repeats": repeats,
            "out_of_order_pages": out_of_order,
            "cursor_not_older": cursor_not_older,
            "without_samples": without_samples,
            "matches_single_read": seen_set == expected_set,
            "missing_from_walk": len(expected_set - seen_set),
            "extra_in_walk": len(seen_set - expected_set),
            "strictly_descending": all(
                pages[index]["newest"] is not None
                and pages[index - 1]["oldest"] is not None
                and pages[index - 1]["oldest"] > pages[index]["newest"]
                for index in range(1, len(pages))
            ),
            "last_page_items": last_items,
            "last_truncated": last_truncated,
            "last_continuation": last_continuation,
            "grain_counts": {
                grain: grain_counts[grain]
                for grain in sorted(grain_counts, key=lambda value: (value is None, value))
            },
            "latency_ms": {
                "p50_ms": round(percentile(latency, 0.5), 3) if latency else None,
                "p95_ms": round(percentile(latency, 0.95), 3) if latency else None,
                "max_ms": round(max(latency), 3) if latency else None,
            },
            "payload_bytes_total": payload_bytes,
            "first_pages": pages[:3],
            "last_pages": pages[-3:],
        }

    def phase_tiers(self) -> dict:
        """Seed a month of hourly Reports and measure what the tiers kept.

        The Server counts every new observation into both tiers whether or not the
        raw window stored it, so a backdated month on a fresh Node is the only way
        to fill the 1 minute and 5 minute tiers without filling node_metric_samples:
        node_metric_samples must hold the 24 instants the raw window keeps, while
        all 720 hourly instants are counted and the measured bucket rows exist.
        """
        started = time.monotonic()
        node_id = clone_node_id(TIER_CLONE_BLOCK, 0)
        scope = "node_id = '" + node_id + "'"
        before = {
            "page_count": sqlite_scalar(self.db_path, "PRAGMA page_count"),
            "database_bytes": self.db_path.stat().st_size if self.db_path.is_file() else 0,
            "with_wal_bytes": file_bytes(self.db_path),
            "grain_rows": self.grain_row_counts(),
            "footprint": self.aggregate_footprint(),
        }
        self.restart_with_floor(CLEARED_FLOOR)

        base = datetime.now(timezone.utc)
        instants = self.tier_instants(base)
        seeded = []
        accepted = 0
        rejected: dict = {}
        seed_latencies = []
        seed_started = time.monotonic()
        for index, moment in enumerate(instants):
            observed_at = moment.strftime(CANONICAL)
            cpu = self.tier_cpu(index)
            outcome = self.submit(
                "tier",
                observed_at,
                cpu,
                21000,
                report=self.tier_report(
                    node_id, observed_at, cpu, self.tier_memory(index), TIER_INVENTORY_REVISIONS[0]
                ),
            )
            seeded.append(observed_at)
            seed_latencies.append(outcome["elapsed_ms"])
            if outcome["disposition"] == "accepted":
                accepted += 1
            else:
                reason = outcome.get("reason") or "unknown"
                rejected[reason] = rejected.get(reason, 0) + 1
        seed_wall = round(time.monotonic() - seed_started, 3)

        raw_rows = self.metric_counts(scope)
        tier_rows = self.grain_row_counts(scope)

        reads = {
            "30d": self.tier_read(
                "30d", instant(-TIER_FIVE_MINUTE_WINDOW_SECONDS), instant(1), node_id, TIER_READ_LIMIT
            ),
            "20h": self.tier_read(
                "20h", instant(-TIER_RAW_READ_HOURS * 3600), instant(1), node_id, TIER_READ_LIMIT
            ),
            "10d": self.tier_read("10d", instant(-10 * 86400), instant(-8 * 86400), node_id, TIER_READ_LIMIT),
            "4d": self.tier_read("4d", instant(-4 * 86400), instant(-2 * 86400), node_id, TIER_READ_LIMIT),
        }
        # Nothing is submitted between the single read above and this walk: every
        # Report runs a cleanup pass and moves the floors, so the two answers can
        # only be compared while the Server sees exactly the same buckets.
        walk = self.tier_walk(
            node_id,
            instant(-TIER_FIVE_MINUTE_WINDOW_SECONDS),
            instant(1),
            reads["30d"]["observed_instants"],
        )
        # What "availability" means is a fact of the tiers, not of the raw window:
        # the 5 minute tier's own age bound is the investigation horizon
        # (crates/platpulse-server/src/metric_history.rs:141) and
        # crates/platpulse-server/src/http/admin.rs:4643-4649 compares the request
        # with that horizon alone, so a range whose start is inside it answers with
        # a null availability, one that straddles it is "partial", and only a range
        # that ends before it is "unavailable". The issue #213 check in evaluate()
        # still expects the raw window's rule for a 30 hour old range, so this pair
        # asks that same range and one older than the whole horizon, and records
        # what the issue #214 Server answers.
        horizon = {
            "inside": self.tier_read(
                "inside_horizon", instant(-30 * 3600), instant(-29 * 3600), node_id, TIER_READ_LIMIT
            ),
            "beyond": self.tier_read(
                "beyond_horizon",
                instant(-TIER_FIVE_MINUTE_WINDOW_SECONDS - 86400),
                instant(-TIER_FIVE_MINUTE_WINDOW_SECONDS - 3600),
                node_id,
                TIER_READ_LIMIT,
            ),
        }

        plants = []
        for ordinal, plant in enumerate(
            (
                (TIER_PLANT_EXPIRED_AGE_SECONDS, 44.5, "expired"),
                (TIER_PLANT_SURVIVING_AGE_SECONDS, 44.5, "surviving"),
            )
        ):
            age, value, kind = plant
            plant_node = clone_node_id(TIER_PLANT_CLONE_BLOCK, ordinal)
            observed_at = instant(-age)
            outcome = self.submit(
                "tier-plant",
                observed_at,
                value,
                21000,
                report=self.tier_report(
                    plant_node, observed_at, value, 2147483648, TIER_INVENTORY_REVISIONS[ordinal + 1]
                ),
            )
            buckets = {}
            for grain_seconds, label in (
                (TIER_ONE_MINUTE_SECONDS, "one_minute"),
                (TIER_FIVE_MINUTE_SECONDS, "five_minute"),
            ):
                bucket_start = aligned_bucket_start(observed_at, grain_seconds)
                buckets[label] = dict(
                    self.bucket_row(plant_node, "process_cpu_percent", grain_seconds, bucket_start),
                    bucket_start=bucket_start,
                )
            entry = {
                "kind": kind,
                "node_id": plant_node,
                "age_seconds": age,
                "observed_at": observed_at,
                "disposition": outcome["disposition"],
                "tier_rows_after_own_report": self.grain_row_counts("node_id = '" + plant_node + "'"),
                "buckets_after_own_report": buckets,
            }
            plants.append(entry)

        # A later accepted Report runs one more cleanup pass
        # (report_ingestion.rs:3116). It plants its own Node, so the phase's Node -
        # and with it the reads and the walk above - cannot be disturbed, and it
        # must release nothing more for the planted instants: the pass runs inside
        # the ingestion request that created a bucket, so the creating Report
        # already released what its instant owed.
        trigger_node = clone_node_id(TIER_PLANT_CLONE_BLOCK, 2)
        trigger_at = instant(-60)
        trigger = self.submit(
            "tier-trigger",
            trigger_at,
            51.5,
            21000,
            report=self.tier_report(
                trigger_node, trigger_at, 51.5, 2147483648, TIER_INVENTORY_REVISIONS[3]
            ),
        )
        for plant in plants:
            plant_node = plant["node_id"]
            after_plant = self.grain_row_counts("node_id = '" + plant_node + "'")
            plant["tier_rows_after_later_report"] = after_plant
            plant["released_by_later_report"] = {
                "one_minute": plant["tier_rows_after_own_report"]["one_minute"] - after_plant["one_minute"],
                "five_minute": plant["tier_rows_after_own_report"]["five_minute"] - after_plant["five_minute"],
            }
            buckets = {}
            for grain_seconds, label in (
                (TIER_ONE_MINUTE_SECONDS, "one_minute"),
                (TIER_FIVE_MINUTE_SECONDS, "five_minute"),
            ):
                bucket_start = aligned_bucket_start(plant["observed_at"], grain_seconds)
                buckets[label] = dict(
                    self.bucket_row(plant_node, "process_cpu_percent", grain_seconds, bucket_start),
                    bucket_start=bucket_start,
                )
            plant["buckets_after_later_report"] = buckets

        node_rows_after_probes = self.grain_row_counts(scope)
        self.stop_server()
        after = {
            "page_count": sqlite_scalar(self.db_path, "PRAGMA page_count"),
            "database_bytes": self.db_path.stat().st_size if self.db_path.is_file() else 0,
            "with_wal_bytes": file_bytes(self.db_path),
            "grain_rows": self.grain_row_counts(),
            "footprint": self.aggregate_footprint(),
        }

        raw_instants = (SERVER_RAW_RETENTION_SECONDS - TIER_NEWEST_AGE_SECONDS) // TIER_CADENCE_SECONDS + 1
        one_minute_buckets = (
            TIER_ONE_MINUTE_WINDOW_SECONDS - TIER_NEWEST_AGE_SECONDS
        ) // TIER_CADENCE_SECONDS + 1
        one_minute_region = (
            TIER_ONE_MINUTE_WINDOW_SECONDS - SERVER_RAW_RETENTION_SECONDS
        ) // TIER_CADENCE_SECONDS
        five_minute_region = (
            TIER_FIVE_MINUTE_WINDOW_SECONDS - TIER_ONE_MINUTE_WINDOW_SECONDS
        ) // TIER_CADENCE_SECONDS
        planned = {
            "reports": TIER_REPORTS,
            "accepted_reports": accepted,
            "cadence_seconds": TIER_CADENCE_SECONDS,
            "declared_span_seconds": TIER_SPAN_SECONDS,
            "newest_age_seconds": TIER_NEWEST_AGE_SECONDS,
            "series_per_report": TIER_SERIES,
            "raw_instants": raw_instants,
            "raw_rows": raw_instants * TIER_SERIES,
            "one_minute_buckets_per_series": one_minute_buckets,
            "one_minute_rows": one_minute_buckets * TIER_SERIES,
            "five_minute_buckets_per_series": TIER_REPORTS,
            "five_minute_rows": TIER_REPORTS * TIER_SERIES,
            "aggregate_rows": (one_minute_buckets + TIER_REPORTS) * TIER_SERIES,
            "one_minute_region_instants": one_minute_region,
            "five_minute_region_instants": five_minute_region,
            "thirty_day_items": raw_instants + one_minute_region + five_minute_region,
            "raw_read_items": (TIER_RAW_READ_HOURS * 3600 - TIER_NEWEST_AGE_SECONDS) // TIER_CADENCE_SECONDS + 1,
            "ten_day_items": (10 * 86400 - 8 * 86400) // TIER_CADENCE_SECONDS,
            "four_day_items": (4 * 86400 - 2 * 86400) // TIER_CADENCE_SECONDS,
            "spike_index": TIER_SPIKE_INDEX,
            "spike_cpu": TIER_SPIKE_CPU,
            "spike_age_seconds": TIER_NEWEST_AGE_SECONDS
            + (TIER_REPORTS - 1 - TIER_SPIKE_INDEX) * TIER_CADENCE_SECONDS,
            "ladder_floor": TIER_VALUE_BASE,
            "ladder_ceiling": TIER_VALUE_BASE + (TIER_VALUE_STEP_COUNT - 1) * TIER_VALUE_STEP,
            "page_limit": TIER_PAGE_LIMIT,
            "walk_pages": (TIER_REPORTS + TIER_PAGE_LIMIT - 1) // TIER_PAGE_LIMIT,
        }
        footprint_before = before["footprint"]["bytes"]
        footprint_after = after["footprint"]["bytes"]
        rows_before = before["grain_rows"]["total"]
        rows_after = after["grain_rows"]["total"]
        rows_added = rows_after - rows_before
        bytes_added = (
            footprint_after - footprint_before
            if footprint_before is not None and footprint_after is not None
            else None
        )
        bytes_per_bucket = (
            round(footprint_after / rows_after, 3) if footprint_after is not None and rows_after else None
        )
        storage = {
            "page_count_before": before["page_count"],
            "page_count_after": after["page_count"],
            "page_size": after["footprint"]["page_size"],
            "database_bytes_before": before["database_bytes"],
            "database_bytes_after": after["database_bytes"],
            "with_wal_bytes_before": before["with_wal_bytes"],
            "with_wal_bytes_after": after["with_wal_bytes"],
            "aggregate_rows_before": rows_before,
            "aggregate_rows_after": rows_after,
            "rows_added": rows_added,
            "pages_added": (after["page_count"] or 0) - (before["page_count"] or 0),
            "database_bytes_added": (after["database_bytes"] or 0) - (before["database_bytes"] or 0),
            "aggregate_footprint_before": before["footprint"],
            "aggregate_footprint_after": after["footprint"],
            "aggregate_bytes_before": footprint_before,
            "aggregate_bytes_after": footprint_after,
            "aggregate_bytes_added": bytes_added,
            "bytes_per_bucket": bytes_per_bucket,
            "bytes_per_bucket_added": (
                round(bytes_added / rows_added, 3) if bytes_added is not None and rows_added else None
            ),
            # dbstat attributes pages, not rows, so a per-tier page split would be a
            # guess: the per-tier figure below is the measured bytes per bucket times
            # that tier's measured rows, and says so.
            "bytes_per_tier": {
                "one_minute": (
                    round(bytes_per_bucket * after["grain_rows"]["one_minute"], 3)
                    if bytes_per_bucket
                    else None
                ),
                "five_minute": (
                    round(bytes_per_bucket * after["grain_rows"]["five_minute"], 3)
                    if bytes_per_bucket
                    else None
                ),
                "method": "measured bytes per bucket x that tier's measured rows",
            },
            "tier_row_counts_before": before["grain_rows"],
            "tier_row_counts_after": after["grain_rows"],
            "tier_row_counts_for_node": node_rows_after_probes,
        }
        return {
            "issue": 214,
            "node_id": node_id,
            "planned": planned,
            "seeded_reports": len(seeded),
            "dispositions": {key: self.dispositions[key] for key in sorted(self.dispositions)},
            "rejections": rejected,
            "seed_wall_seconds": seed_wall,
            "write_latency_ms": {
                "p50_ms": round(percentile(seed_latencies, 0.5), 3) if seed_latencies else None,
                "p95_ms": round(percentile(seed_latencies, 0.95), 3) if seed_latencies else None,
                "max_ms": round(max(seed_latencies), 3) if seed_latencies else None,
            },
            "instants": {
                "newest": seeded[-1],
                "oldest": seeded[0],
                "declared_cadence_seconds": TIER_CADENCE_SECONDS,
                "declared_span_seconds": int(
                    (parse_instant(seeded[-1]) - parse_instant(seeded[0])).total_seconds()
                ),
                "newest_age_seconds": int((base - parse_instant(seeded[-1])).total_seconds()),
                "oldest_age_seconds": int((base - parse_instant(seeded[0])).total_seconds()),
            },
            "raw_rows": raw_rows,
            "tier_rows": tier_rows,
            "reads": reads,
            "horizon": horizon,
            "walk": walk,
            "planted": {
                "plants": plants,
                "later_report": {
                    "node_id": trigger_node,
                    "observed_at": trigger_at,
                    "disposition": trigger["disposition"],
                    "tier_rows": self.grain_row_counts("node_id = '" + trigger_node + "'"),
                },
                "expired_age_seconds": TIER_PLANT_EXPIRED_AGE_SECONDS,
                "surviving_age_seconds": TIER_PLANT_SURVIVING_AGE_SECONDS,
            },
            "storage": storage,
            "wall_seconds": round(time.monotonic() - started, 3),
        }

    # -- issue #215: the Host family stored once per Agent -----------------

    def host_family_rows(self) -> list:
        """One row per stored (metric, dimension) series of the Host family."""
        return sqlite_rows(
            self.db_path,
            "SELECT metric, dimension, COUNT(*) AS samples,"
            " MIN(observed_at) AS first_observed_at, MAX(observed_at) AS last_observed_at"
            " FROM host_metric_samples GROUP BY metric, dimension"
            " ORDER BY metric, dimension",
        )

    def host_ledger_rows(self, agent_id: str) -> list:
        """The per-series ledger of the Host family, for one Agent."""
        return sqlite_rows(
            self.db_path,
            "SELECT metric, dimension FROM host_metric_series_state"
            " WHERE agent_id = '" + agent_id + "' ORDER BY metric, dimension",
        )

    def host_footprint(self) -> dict:
        """The page footprint of the raw Host table, its indexes and its ledger,
        measured the way the tier phase measures the aggregates (dbstat), so the
        bytes per row below are a measured number rather than a projection."""
        page_size = sqlite_scalar(self.db_path, "PRAGMA page_size")
        try:
            rows = sqlite_rows(
                self.db_path,
                "SELECT name, SUM(pgsize) AS bytes FROM dbstat WHERE name LIKE 'host_metric%'"
                " GROUP BY name ORDER BY name",
            )
        except sqlite3.Error as error:
            return {"available": False, "reason": str(error)}
        objects = {row["name"]: row["bytes"] for row in rows}
        total = sum(objects.values())
        raw = objects.get("host_metric_samples", 0)
        samples = sqlite_scalar(self.db_path, "SELECT COUNT(*) FROM host_metric_samples") or 0
        return {
            "available": True,
            "objects": objects,
            "raw_bytes": raw,
            "family_bytes": total,
            "bytes_per_sample": round(raw / samples, 3) if samples else None,
            "pages": round(total / page_size, 3) if page_size else None,
            "page_size": page_size,
            "samples": samples,
        }

    def host_route_reads(self, agent_id: str, node_ids: list) -> dict:
        """The Agent route, and the host route of the Agent's Nodes, read over
        the same window so their answers can be compared point by point."""
        window = "&from=" + self.window_from() + "&to=" + self.window_to() + "&limit=5000"
        reads = {}
        latencies = []
        for metric in ("cpu_percent", "memory_used_bytes", "load1", "network_rx_bytes_per_sec"):
            payload, elapsed_ms, payload_bytes = read_shared_history(
                self.client, self.cookie, agent_history_url(agent_id, metric, window)
            )
            latencies.append(elapsed_ms)
            reads[metric] = {
                "items": len(payload["items"]),
                "window_seconds": payload["windowSeconds"],
                "scope_kind": payload.get("scopeKind"),
                "scope_key": payload.get("scopeKey"),
                "node_id": payload.get("nodeId"),
                "dimension": payload.get("dimension"),
                "gaps": len(payload["gaps"]),
                "latency_ms": round(elapsed_ms, 3),
                "payload_bytes": payload_bytes,
                "points": [[item["observedAt"], item["value"]] for item in payload["items"]],
                "series": self.ledger(payload),
            }
        by_node = {}
        for node_id in node_ids:
            payload, elapsed_ms, payload_bytes = read_shared_history(
                self.client, self.cookie, node_host_history_url(node_id, "cpu_percent", window)
            )
            by_node[node_id] = {
                "items": len(payload["items"]),
                "scope_kind": payload.get("scopeKind"),
                "scope_key": payload.get("scopeKey"),
                "node_id": payload.get("nodeId"),
                "latency_ms": round(elapsed_ms, 3),
                "payload_bytes": payload_bytes,
                "points": [[item["observedAt"], item["value"]] for item in payload["items"]],
            }
        return {"reads": reads, "by_node": by_node, "latency_ms": latencies}

    def phase_host(self, release: dict) -> dict:
        """Issue #215: the Host quantities a Report states are stored once for
        the Agent that collected them, not once per Node, and the storage pair is
        named by the mount path the Agent reported.

        Every Report above already carried the fixture's Host block, which states
        the eight shared quantities and no mount, so this phase audits the family
        those Reports produced, reads it back through every route that serves it,
        and then plants one Report that carries two mounts to measure what a
        mount adds to the family.
        """
        started = time.perf_counter()
        self.restart_with_floor(CLEARED_FLOOR)
        agent_id = self.agent["agent_id"]
        columns = table_columns(self.db_path, "host_metric_samples")
        family_before = self.host_family_rows()
        ledger_before = self.host_ledger_rows(agent_id)
        node_ids = [
            row["node_id"]
            for row in sqlite_rows(
                self.db_path,
                "SELECT node_id FROM nodes WHERE agent_id = '" + agent_id + "' ORDER BY node_id",
            )
        ]
        nodes_with_their_own_series = sqlite_scalar(
            self.db_path, "SELECT COUNT(DISTINCT node_id) FROM node_metric_series_state"
        )
        routes = self.host_route_reads(agent_id, node_ids[:2])
        mountless, _, _ = read_shared_history(
            self.client,
            self.cookie,
            agent_history_url(
                agent_id,
                "disk_used_bytes",
                "&from=" + self.window_from() + "&to=" + self.window_to() + "&limit=5000",
            ),
        )
        refusals = {
            "the Agent route refuses a Node series": refusal_code(
                self.client, self.cookie, agent_history_url(agent_id, "process_cpu_percent")
            ),
            "the Node host route refuses a Node series": refusal_code(
                self.client, self.cookie, node_host_history_url(node_ids[0], "process_cpu_percent")
            ),
            "the Node route refuses a Host series": refusal_code(
                self.client, self.cookie, history_url(node_ids[0], "cpu_percent")
            ),
        }
        rows_at_or_before_the_released_instant = sqlite_scalar(
            self.db_path,
            "SELECT COUNT(*) FROM host_metric_samples WHERE observed_at <= '"
            + release["released_instant"]
            + "'",
        )
        footprint = self.host_footprint()
        # One Report that carries two mounts: the storage pair is stored once per
        # mount path, and only for the Agent that reported it.
        high_water = sqlite_scalar(self.db_path, "SELECT MAX(observed_at) FROM host_metric_samples")
        planted_at = (parse_instant(high_water) + timedelta(seconds=1)).strftime(CANONICAL)
        mounts = [
            {"mount_path": "/", "total_bytes": 274877906944, "used_bytes": 137438953472},
            {"mount_path": "/data", "total_bytes": 1099511627776, "used_bytes": 549755813888},
        ]
        report = build_report(self.fixture, self.agent, 0, planted_at, 12.5, 21000, report_id_for(0))
        # The fixture's revision belongs to the fixture's own single-Node
        # inventory and the accepted revision is already the last tier one, so the
        # planted Report declares the next revision with the inventory it carries.
        report["inventory"]["revision"] = HOST_INVENTORY_REVISION
        report["host"]["disk"]["latest"] = {"mounts": copy.deepcopy(mounts)}
        planted = self.submit("host mounts", planted_at, 12.5, 21000, report=report)
        family_after = self.host_family_rows()
        ledger_after = self.host_ledger_rows(agent_id)
        span = "&from=" + planted_at + "&to=" + instant(1) + "&limit=5000"
        mount_reads = {}
        for mount in mounts:
            dimension = mount["mount_path"]
            payload, elapsed_ms, payload_bytes = read_shared_history(
                self.client,
                self.cookie,
                agent_history_url(agent_id, "disk_used_bytes", "&dimension=" + dimension + span),
            )
            mount_reads[dimension] = {
                "items": len(payload["items"]),
                "dimension": payload.get("dimension"),
                "scope_kind": payload.get("scopeKind"),
                "points": [[item["observedAt"], item["value"]] for item in payload["items"]],
                "latency_ms": round(elapsed_ms, 3),
                "payload_bytes": payload_bytes,
            }
        return {
            "issue": 215,
            "agent_id": agent_id,
            "columns": list(columns),
            "family_before": family_before,
            "family_after": family_after,
            "ledger_before": ledger_before,
            "ledger_after": ledger_after,
            "nodes": len(node_ids),
            "nodes_with_their_own_series": nodes_with_their_own_series,
            "routes": routes,
            "mountless_read": {
                "items": len(mountless["items"]),
                "dimension": mountless.get("dimension"),
                "scope_kind": mountless.get("scopeKind"),
                "series": self.ledger(mountless),
            },
            "refusals": refusals,
            "released_instant": release["released_instant"],
            "rows_at_or_before_the_released_instant": rows_at_or_before_the_released_instant,
            "footprint": footprint,
            "planted": {
                "observed_at": planted_at,
                "inventory_revision": HOST_INVENTORY_REVISION,
                "disposition": planted["disposition"],
                "reason": planted["reason"],
                "mounts": mounts,
                "mount_reads": mount_reads,
            },
            "bounds": {
                "declared_host_family": list(DECLARED_HOST_FAMILY),
                "measured_family": list(HOST_FAMILY),
                "shared_series": list(HOST_SHARED_SERIES),
                "mount_series": list(HOST_MOUNT_SERIES),
                "max_host_mounts": MAX_HOST_MOUNTS,
                "max_host_rows_per_report": MAX_HOST_ROWS_PER_REPORT,
                "server_max_host_rows_per_report": SERVER_MAX_HOST_ROWS_PER_REPORT,
                "host_metric_cleanup_batch": HOST_METRIC_CLEANUP_BATCH,
                "aggregate_cleanup_batch": AGGREGATE_CLEANUP_BATCH,
                "forbidden_tokens": list(HOST_FORBIDDEN_TOKENS),
                "forbidden_in_family": [
                    token for token in HOST_FORBIDDEN_TOKENS if any(token in metric for metric in HOST_FAMILY)
                ],
            },
            "wall_seconds": round(time.monotonic() - started, 3),
        }

    def post_report(self, agent: dict, report: dict) -> dict:
        """One Report under an explicit credential: the phases above all report as
        self.agent, and the second Agent this phase enrolls must not."""
        status, _, body, elapsed_ms = self.client.request(
            "POST",
            "/api/agent/v1/reports",
            body=json_bytes(report),
            headers={"Authorization": "Bearer " + agent["credential"], "Content-Type": "application/json"},
        )
        payload = json.loads(body)
        receipt = payload.get("receipt") if isinstance(payload.get("receipt"), dict) else {}
        return {
            "status": status,
            "elapsed_ms": round(elapsed_ms, 3),
            "disposition": payload.get("disposition") or receipt.get("disposition"),
            "reason": receipt.get("reason") or receipt.get("detail") or payload.get("reason"),
        }

    def read_mounts(self, agent_id: str, reads: int = 1) -> dict:
        """The Owner's mount list of one Agent, read `reads` times so the latency
        below is a distribution over real reads rather than one sample."""
        path = storage_mounts_url(agent_id)
        latencies = []
        sizes = []
        headers = {}
        payload = {}
        for _ in range(reads):
            status, headers, body, elapsed_ms = admin_get(self.client, self.cookie, path)
            if status != 200:
                raise BaselineError(
                    "GET " + path + " failed with status " + str(status) + ": " + body.decode("utf-8")[:400]
                )
            latencies.append(elapsed_ms)
            sizes.append(len(body))
            payload = json.loads(body)
        return {
            "path": path,
            "reads": reads,
            "answered_at": payload.get("answeredAt"),
            "cadence_seconds": payload.get("cadenceSeconds"),
            "silence_threshold_seconds": payload.get("silenceThresholdSeconds"),
            "used_metric": payload.get("usedMetric"),
            "capacity_metric": payload.get("capacityMetric"),
            "mount_limit": payload.get("mountLimit"),
            "truncated": payload.get("truncated"),
            "cache_control": headers.get("cache-control"),
            "payload_bytes": {"min": min(sizes), "max": max(sizes)},
            "latency_ms": {
                "p50": round(percentile(latencies, 0.5), 3),
                "p95": round(percentile(latencies, 0.95), 3),
                "min": round(min(latencies), 3),
                "max": round(max(latencies), 3),
            },
            "mounts": payload.get("mounts") or [],
        }

    def phase_mount_coverage(self) -> dict:
        """Issue #216: the storage family is answered per mount path, and the list
        the Owner reads is a bounded, newest-first walk of the mount ledger.

        Four measurements share one phase because that list is one bounded answer
        per Agent: a second Agent whose only Report cannot measure a cadence, a
        path whose newest reading Retention already released, more distinct paths
        than the list can answer, and the rhythm the Agent is keeping now - the
        cadence, and with it the silence bound every state below is judged
        against, planted as the Agent's newest observations. Every Report above
        stated no mount - the fixture's disk block carries an empty mount list -
        so every mount row read here belongs to a path this phase planted.
        """
        started = time.perf_counter()
        agent_id = self.agent["agent_id"]

        # The footprint is measured while the Server is stopped and before any
        # path of this phase exists, so the delta below belongs to this phase.
        self.stop_server()
        footprint_before = self.host_footprint()
        # The ledger is not empty here: the Host family phase above (issue #215)
        # plants a Report of its own. Those paths belong in the arithmetic below,
        # because the bound cuts the whole ledger, not only what this phase plants.
        preexisting = mount_ledger_order(self.db_path, agent_id)
        cadence_held_at = host_cadence_instant(self.db_path, agent_id)
        token = create_enrollment_token(self.binary, self.config)
        self.start_server(CLEARED_FLOOR)

        # -- an Agent whose only Report cannot measure a cadence -------------
        solo_agent = enroll_agent(token, self.client)
        solo_paths = [MOUNT_SOLO_PATH, MOUNT_SOLO_CACHE_PATH]
        solo_at = instant(-300)
        solo_sequence = self.next_sequence()
        solo_report = build_report(
            self.fixture, solo_agent, solo_sequence, solo_at, 7.5, 22000, report_id_for(solo_sequence)
        )
        solo_node_id = clone_node_id(MOUNT_SOLO_NODE_BLOCK, 0)
        solo_report["nodes"][0]["node_id"] = solo_node_id
        solo_report["inventory"]["nodes"][0]["node_id"] = solo_node_id
        solo_report["host"]["disk"]["latest"] = {"mounts": mount_payload(solo_paths)}
        solo_post = self.post_report(solo_agent, solo_report)
        solo_read = self.read_mounts(solo_agent["agent_id"])

        # -- the rhythm the list's cadence is measured from -------------------
        # Planted first, and planted above the newest observation the phases above
        # left behind, so every state read below is judged against a rhythm the
        # Agent is really keeping rather than against a cadence nobody could
        # measure. The rhythm is three Reports because the newest interval is
        # trusted only when the one before it agrees, and the pair of observations
        # one second apart the phases above end with stays the fastest interval the
        # ledger holds, so the cadence the list answers is evidence of which rule
        # decided it. The released read below happens before the bulk Reports, so
        # with two Reports it would still answer cadence 0 and call that path
        # unknown.
        newest_at = instant(MOUNT_NEWEST_OFFSET_SECONDS)
        rhythm_seconds = mount_rhythm_seconds(newest_at, cadence_held_at)
        planted = {}

        def plant(name, kind, observed_at, paths, cpu, pid):
            sequence = self.next_sequence()
            report = build_report(self.fixture, self.agent, sequence, observed_at, cpu, pid, report_id_for(sequence))
            report["inventory"]["revision"] = HOST_INVENTORY_REVISION
            report["host"]["disk"]["latest"] = {"mounts": mount_payload(paths)}
            submission = self.submit(kind, observed_at, cpu, pid, report=report)
            planted[name] = {
                "kind": kind,
                "observed_at": observed_at,
                "paths": len(paths),
                "first_path": paths[0] if paths else None,
                "last_path": paths[-1] if paths else None,
                "disposition": submission["disposition"],
                "reason": submission["reason"],
            }

        # The three Reports below carry no mount: they are the rhythm alone.
        for step in (3, 2, 1):
            plant(
                "rhythm-" + str(step),
                "mount rhythm",
                shift_instant(newest_at, -step * rhythm_seconds),
                [],
                10.5,
                27000,
            )

        # -- a path whose newest reading Retention already released ----------
        released_at = instant(-(self.window_seconds + 2 * 3600))
        released_sequence = self.next_sequence()
        released_report = build_report(
            self.fixture, self.agent, released_sequence, released_at, 6.5, 23000, report_id_for(released_sequence)
        )
        released_report["inventory"]["revision"] = HOST_INVENTORY_REVISION
        released_report["host"]["disk"]["latest"] = {"mounts": mount_payload([MOUNT_RELEASED_PATH])}
        released_post = self.submit("released mount", released_at, 6.5, 23000, report=released_report)
        # Retention releases the readings at or before the raw floor on the way
        # up, the same restart that released the raw rows of the phases above.
        self.restart_with_floor(CLEARED_FLOOR)
        # Read while the list still answers every path it holds: the released
        # path is the oldest series this Agent has, so the bounded read below
        # leaves it out, and the state it is given is judged against the rhythm
        # planted above rather than against an unmeasurable cadence.
        released_read = self.read_mounts(agent_id)

        # -- more distinct paths than the list can answer --------------------
        retired_at = instant(-MOUNT_RETIRED_AGE_SECONDS)
        middle_at = instant(-MOUNT_RETIRED_AGE_SECONDS // 2)
        retired_paths = [MOUNT_RETIRED_PATH] + [
            bulk_mount_path(index) for index in range(MOUNT_BULK_PATHS_PER_REPORT)
        ]
        middle_paths = [
            bulk_mount_path(index)
            for index in range(MOUNT_BULK_PATHS_PER_REPORT, 2 * MOUNT_BULK_PATHS_PER_REPORT)
        ]
        newest_paths = [
            bulk_mount_path(index)
            for index in range(2 * MOUNT_BULK_PATHS_PER_REPORT, 3 * MOUNT_BULK_PATHS_PER_REPORT)
        ]
        newest_paths.append(MOUNT_LIVE_PATH)
        for name, kind, observed_at, paths, cpu, pid in (
            ("retired", "retired mount", retired_at, retired_paths, 8.5, 24000),
            ("middle", "middle mount", middle_at, middle_paths, 9.5, 25000),
            ("newest", "newest mount", newest_at, newest_paths, 10.5, 26000),
        ):
            plant(name, kind, observed_at, paths, cpu, pid)
        coverage_read = self.read_mounts(agent_id, MOUNT_COVERAGE_READS)
        # The silent path is still judged on the reading it stored, so that
        # reading is read back over the window that contains it.
        retired_history, retired_latency, retired_bytes = read_shared_history(
            self.client,
            self.cookie,
            agent_history_url(
                agent_id,
                MOUNT_USED_METRIC,
                "&dimension="
                + MOUNT_RETIRED_PATH
                + "&from="
                + retired_at
                + "&to="
                + self.window_to()
                + "&limit=5000",
            ),
        )

        # -- the ledger the list walks, and the plan it walks it with --------
        self.stop_server()
        footprint_after = self.host_footprint()
        order = mount_ledger_order(self.db_path, agent_id)
        plan = mount_coverage_plan(self.db_path, agent_id, MOUNT_USED_METRIC, MOUNT_COVERAGE_LIMIT)
        agent_rows = mount_rows(self.db_path, agent_id)
        solo_rows = mount_rows(self.db_path, solo_agent["agent_id"])

        answered = [entry["mountPath"] for entry in coverage_read["mounts"]]
        # Every path this Agent's mount ledger can hold: what the phases above
        # already planted, what this phase planted, and the released path.
        ledger_at = {path: observed_at for path, observed_at in preexisting}
        for paths, observed_at in (
            (retired_paths, retired_at),
            (middle_paths, middle_at),
            (newest_paths, newest_at),
            ([MOUNT_RELEASED_PATH], released_at),
        ):
            for path in paths:
                ledger_at[path] = observed_at
        # What the declaration promises, computed from those known instants alone:
        # newest observation first with ties settled by the path, and only the first
        # MOUNT_COVERAGE_LIMIT paths of that order answered. The paths the Host
        # phase above planted are newer than the middle instant, so they are kept
        # whole and the retired instant is cut that much shorter.
        declared_order = sorted(ledger_at)
        declared_order.sort(key=lambda path: ledger_at[path], reverse=True)
        expected_answer = declared_order[:MOUNT_COVERAGE_LIMIT]
        kept_by_instant = {}
        dropped_by_instant = {}
        for path in answered:
            key = ledger_at.get(path)
            kept_by_instant[key] = kept_by_instant.get(key, 0) + 1
        dropped = sorted(set(ledger_at) - set(answered))
        for path in dropped:
            key = ledger_at[path]
            dropped_by_instant[key] = dropped_by_instant.get(key, 0) + 1
        expected_kept = {}
        for path in expected_answer:
            key = ledger_at[path]
            expected_kept[key] = expected_kept.get(key, 0) + 1
        expected_dropped = {}
        for path in declared_order[MOUNT_COVERAGE_LIMIT:]:
            key = ledger_at[path]
            expected_dropped[key] = expected_dropped.get(key, 0) + 1
        # The paths the bound leaves out are the oldest ones: nothing answered is
        # older than anything dropped.
        newest_dropped = max((ledger_at[path] for path in dropped), default=None)
        oldest_kept = min((ledger_at[path] for path in answered), default=None)

        return {
            "issue": 216,
            "agent_id": agent_id,
            "solo_agent_id": solo_agent["agent_id"],
            "preexisting": preexisting,
            "bounds": {
                "mount_series": list(HOST_MOUNT_SERIES),
                "used_metric": MOUNT_USED_METRIC,
                "capacity_metric": MOUNT_CAPACITY_METRIC,
                "max_host_mounts": MAX_HOST_MOUNTS,
                "coverage_limit": MOUNT_COVERAGE_LIMIT,
                "coverage_reads": MOUNT_COVERAGE_READS,
                "bulk_paths_per_report": MOUNT_BULK_PATHS_PER_REPORT,
                "retired_age_seconds": MOUNT_RETIRED_AGE_SECONDS,
                "newest_offset_seconds": MOUNT_NEWEST_OFFSET_SECONDS,
                "rhythm_seconds": rhythm_seconds,
                "rhythm_held_at": cadence_held_at,
                "cadence_samples": COVERAGE_CADENCE_SAMPLES,
                "max_observed_cadence_seconds": MAX_OBSERVED_CADENCE_SECONDS,
                "gap_cadence_factor": GAP_CADENCE_FACTOR,
                "min_gap_seconds": MIN_GAP_SECONDS,
                "coverage_index": MOUNT_COVERAGE_INDEX,
                "forbidden_plans": list(MOUNT_COVERAGE_FORBIDDEN_PLANS),
            },
            "solo": {
                "node_id": solo_node_id,
                "observed_at": solo_at,
                "paths": solo_paths,
                "expected": {path: mount_used_bytes(path) for path in solo_paths},
                "post": solo_post,
                "read": mount_read_summary(solo_read),
                "entries": [mount_entry(solo_read, path) for path in solo_paths],
            },
            "released": {
                "observed_at": released_at,
                "post": released_post,
                "read": mount_read_summary(released_read),
                "entry": mount_entry(released_read, MOUNT_RELEASED_PATH),
            },
            "planted": planted,
            "coverage": {
                "read": mount_read_summary(coverage_read),
                "answered": answered,
                "dropped": dropped,
                "order": order,
                "order_size": len(order),
                "kept_by_instant": kept_by_instant,
                "dropped_by_instant": dropped_by_instant,
                "expected_kept": expected_kept,
                "expected_dropped": expected_dropped,
                "declared_order_size": len(declared_order),
                "expected_answer_size": len(expected_answer),
                "newest_dropped": newest_dropped,
                "oldest_kept": oldest_kept,
                "instants": {
                    "released_at": released_at,
                    "retired_at": retired_at,
                    "middle_at": middle_at,
                    "newest_at": newest_at,
                },
                "retired_entry": mount_entry(coverage_read, MOUNT_RETIRED_PATH),
                "live_entry": mount_entry(coverage_read, MOUNT_LIVE_PATH),
                "live_path": MOUNT_LIVE_PATH,
                "retired_path": MOUNT_RETIRED_PATH,
            },
            "history": {
                "dimension": retired_history.get("dimension"),
                "scope_kind": retired_history.get("scopeKind"),
                "items": len(retired_history["items"]),
                "points": [[item["observedAt"], item["value"]] for item in retired_history["items"]],
                "expected_value": mount_used_bytes(MOUNT_RETIRED_PATH),
                "latency_ms": round(retired_latency, 3),
                "payload_bytes": retired_bytes,
            },
            "rows": {
                "agent": agent_rows,
                "solo": solo_rows,
                "columns": list(table_columns(self.db_path, "host_metric_samples")),
            },
            "plan": {
                "sql": MOUNT_COVERAGE_SQL,
                "binds": [agent_id, MOUNT_USED_METRIC, MOUNT_COVERAGE_LIMIT],
                "lines": plan,
                "forbidden": [
                    token for token in MOUNT_COVERAGE_FORBIDDEN_PLANS if any(token in line for line in plan)
                ],
            },
            "footprint": {
                "before": footprint_before,
                "after": footprint_after,
                "family_delta_bytes": footprint_after["family_bytes"] - footprint_before["family_bytes"],
                "mount_index_bytes": footprint_after["objects"].get(MOUNT_COVERAGE_INDEX, 0),
                "bytes_per_mount_series": (
                    round(footprint_after["objects"].get(MOUNT_COVERAGE_INDEX, 0) / (agent_rows["mount_series"] or 1), 3)
                ),
            },
            "wall_seconds": round(time.monotonic() - started, 3),
        }

    def phase_state_history(self) -> dict:
        """Issue #217: the recorded sync/consensus state log, the five chain
        height series that ride the Node metric engine, and the Owner-only read
        surface that answers them.

        One fresh Agent owns two cloned Nodes and every Report of this phase
        states both of them. The probe Node (ordinal 0) keeps the rows the
        SQLite counts, the query plans and the paged walk are read from, and the
        ledger Node (ordinal 1) is the one purged at the end. Both keep the
        fixture's disabled process observation, so the only Node series they
        ever state are the five heights issue #217 added
        (crates/platpulse-server/src/metric_history.rs NODE_METRIC_SERIES).

        The recorded vector holds only collection_state, value_source,
        error_code and syncing (crates/platpulse-server/src/state_history.rs
        StateVector), so a Report whose heights moved but whose vector did not
        is a counted delivery that stores no second entry: sync changes when its
        flag flips (units 0, 1 and 2) and then anchors once per 3600 s of
        unchanged silence (units 8, 14 and 26), while the constant consensus
        vector anchors at units 6, 12 and 26. One failure, repeated once, is one
        change and one counted delivery that stores nothing.
        """
        started = time.perf_counter()
        self.stop_server()
        storage_before = self.sample_storage("state-before")
        token = create_enrollment_token(self.binary, self.config)
        self.start_server(CLEARED_FLOOR)
        agent = enroll_agent(token, self.client)
        self.agent = agent

        probe = state_node_id(0)
        ledger_node = state_node_id(1)
        base = instant(-STATE_LEAD_SECONDS)
        head = STATE_NODE_BLOCK * 1000

        def at(unit: int) -> str:
            """One instant of this phase's own rhythm."""
            return shift_instant(base, unit * STATE_RHYTHM_SECONDS)

        def height(unit: int, ordinal: int, index: int) -> int:
            """One distinct value per stored series, so each of the five heights
            is judged against its own list and the two Nodes cannot be taken for
            one another."""
            return head + unit * 5 + ordinal * 1000 + index

        def chain_blocks(unit: int, syncing: bool, failed: bool) -> list:
            blocks = []
            for ordinal in range(2):
                if failed:
                    # An error carries no latest reading at all, which is what
                    # makes the metric writer store no height for the two series
                    # this component owns while the state log still records the
                    # transition (crates/platpulse-server/src/http/
                    # report_ingestion.rs metric_observed_at).
                    blocks.append(
                        (
                            state_error_block(at(unit), unit, "probe_failed", "the chain probe failed"),
                            state_error_block(at(unit), unit, "probe_failed", "the chain probe failed"),
                        )
                    )
                else:
                    blocks.append(
                        (
                            state_sync_block(
                                at(unit), syncing, height(unit, ordinal, 0), height(unit, ordinal, 1), unit
                            ),
                            state_consensus_block(
                                at(unit),
                                height(unit, ordinal, 2),
                                height(unit, ordinal, 3),
                                height(unit, ordinal, 4),
                                unit,
                            ),
                        )
                    )
            return blocks

        deliveries = []

        def deliver(unit: int, syncing: bool = False, failed: bool = False, kind: str = "state") -> dict:
            blocks = chain_blocks(unit, syncing, failed)
            report = state_report(
                self.fixture,
                agent,
                0,
                at(unit),
                [pair[0] for pair in blocks],
                [pair[1] for pair in blocks],
                STATE_INVENTORY_REVISION,
            )
            receipt = self.submit(kind, at(unit), 1.0, 1, report=report, nodes=2)
            entry = {
                "unit": unit,
                "instant": at(unit),
                "kind": kind,
                "failed": failed,
                "syncing": None if failed else syncing,
                "disposition": receipt["disposition"],
                "reason": receipt["reason"],
                "elapsed_ms": receipt["elapsed_ms"],
            }
            deliveries.append(entry)
            return entry

        # Fifteen Reports keep the rhythm, and the sync flag flips once: the
        # vector it belongs to changes three times while the heights move on
        # every single Report.
        for unit in range(15):
            deliver(unit, syncing=(unit == 1))
        # Units 15 to 25 are silence: no Report arrives at all.
        deliver(26)
        # A failed chain probe: the transition is recorded, the heights are not.
        deliver(27, failed=True)
        # The very same failure again: counted, and no second entry.
        deliver(28, failed=True)

        # -- the low-space pause ----------------------------------------------
        # The protection floor is raised and the paused Reports follow at once:
        # the pause is in force without waiting for a sampler tick (the
        # phase_load precedent), and a paused Report is still accepted.
        self.restart_with_floor(MAX_PERSISTED_BYTES)
        for unit in (29, 30, 31):
            deliver(unit, kind="state paused")
        # Stopping here is what makes the frozen ledger a plain SQLite read: the
        # pause has closed nothing yet, and the three paused deliveries were
        # counted as skipped instead of recorded.
        self.stop_server()
        frozen = {
            "probe": state_rows(self.db_path, probe),
            "ledger": state_rows(self.db_path, ledger_node),
        }
        self.start_server(CLEARED_FLOOR)
        # Resuming closes the protection interval, and the recovered delivery
        # records a state again.
        recovery = deliver(32, kind="state resumed")

        read_from = shift_instant(base, -STATE_RHYTHM_SECONDS)
        read_to = instant(0)

        def read_state(node_id: str, component: str, before=None, limit=None) -> dict:
            return read_surface(
                self.client,
                self.cookie,
                state_history_url(node_id, component, read_from, read_to, limit=limit, before=before),
            )

        sync_read = read_state(probe, "sync")
        consensus_read = read_state(probe, "consensus")
        ledger_sync_read = read_state(ledger_node, "sync")

        latency = []
        for _ in range(STATE_READS):
            attempt = read_state(probe, "sync")
            if attempt["status"] != 200:
                raise BaselineError(
                    "GET " + attempt["path"] + " answered " + str(attempt["status"]) + " instead of 200"
                )
            latency.append(attempt["latency_ms"])

        page_one = read_state(probe, "sync", limit=STATE_PAGE_LIMIT)
        continuation = page_one["payload"].get("continuation")
        page_two = None
        if isinstance(continuation, str):
            page_two = read_state(probe, "sync", limit=STATE_PAGE_LIMIT, before=continuation)

        unknown_node = clone_node_id(STATE_NODE_BLOCK, 9)
        refusals = {
            "unknown component": read_state(probe, "pouet"),
            "unknown node": read_surface(
                self.client, self.cookie, state_history_url(unknown_node, "sync", read_from, read_to)
            ),
            "before at from": read_state(probe, "sync", before=read_from),
            "before past to": read_state(probe, "sync", before=shift_instant(read_to, 60)),
            "clamped limit": read_state(probe, "sync", limit=STATE_MAX_LIMIT + 1),
            "horizon straddled": read_surface(
                self.client,
                self.cookie,
                state_history_url(probe, "sync", instant(-(STATE_RETENTION_DAYS * 86400 + 3600)), read_to),
            ),
            "horizon released": read_surface(
                self.client,
                self.cookie,
                state_history_url(
                    probe,
                    "sync",
                    instant(-(STATE_RETENTION_DAYS * 86400 + 7200)),
                    instant(-(STATE_RETENTION_DAYS * 86400 + 3600)),
                ),
            ),
        }

        heights = {}
        for label, node_id in (("probe", probe), ("ledger", ledger_node)):
            series = {}
            for metric in STATE_NODE_SERIES:
                reading = read_surface(
                    self.client,
                    self.cookie,
                    node_height_history_url(node_id, metric, read_from, read_to, STATE_DEFAULT_LIMIT),
                )
                payload = reading["payload"]
                series[metric] = {
                    "status": reading["status"],
                    "latency_ms": reading["latency_ms"],
                    "items": [
                        {"observedAt": item.get("observedAt"), "value": item.get("value")}
                        for item in payload.get("items") or []
                    ],
                    "gaps": payload.get("gaps"),
                    # The Node metric answer states the series' own coverage and
                    # counts (AdminMetricSeries,
                    # crates/platpulse-server/src/http/admin.rs:4421-4458) and
                    # states no cadence at all: AdminMetricHistoryResponse
                    # (admin.rs:4489-4539) has no cadenceSeconds field, unlike the
                    # state answer, whose cadenceSeconds is the state ledger's.
                    "cadence_seconds": payload.get("cadenceSeconds"),
                    "coverage_seconds": (payload.get("series") or {}).get("coverageSeconds"),
                    "observation_count": (payload.get("series") or {}).get("observationCount"),
                    "sampled_count": (payload.get("series") or {}).get("sampledCount"),
                    "window_seconds": payload.get("windowSeconds"),
                    "scope_kind": payload.get("scopeKind"),
                    "scope_key": payload.get("scopeKey"),
                }
            heights[label] = series

        # -- the purge ---------------------------------------------------------
        def admin_post(path: str, payload: dict, session: str = "guarded") -> dict:
            # A mutation needs the session cookie, the JSON content type, a
            # matching Origin and the session's CSRF token
            # (crates/platpulse-server/src/http/admin.rs:59-75 mutation_guard_ok,
            # which answers 403 csrf_validation_failed). The two weaker requests
            # below drop one of those on purpose: "no_token" keeps the session and
            # the browser-shaped headers but omits x-csrf-token, and "anonymous"
            # carries no session at all, which the auth middleware refuses with
            # 401 before the guard is consulted.
            headers = {"Content-Type": "application/json", "Origin": "http://127.0.0.1:" + str(self.port)}
            if session != "anonymous":
                headers["Cookie"] = self.cookie
            if session == "guarded":
                headers["x-csrf-token"] = self.csrf
            status, _, body, elapsed_ms = self.client.request(
                "POST", path, body=json_bytes(payload), headers=headers
            )
            try:
                parsed = json.loads(body)
            except ValueError:
                parsed = {}
            error = parsed.get("error") if isinstance(parsed.get("error"), dict) else {}
            return {
                "path": path,
                "status": status,
                "code": error.get("code"),
                "message": error.get("message"),
                "removed": parsed.get("removed"),
                "latency_ms": elapsed_ms,
                "bytes": len(body),
            }

        # The echoed Node ID is camelCase on the wire: NodePurgeRequest is
        # #[serde(rename_all = "camelCase")] (admin.rs:422-426), so a snake_case
        # body is refused as invalid_json before the confirmation is compared.
        purge_path = "/api/admin/v1/nodes/" + ledger_node + "/purge"
        mismatched = admin_post(purge_path, {"confirmNodeId": probe})
        still_there = read_state(ledger_node, "sync")
        anonymous = admin_post(purge_path, {"confirmNodeId": ledger_node}, session="anonymous")
        unguarded = admin_post(purge_path, {"confirmNodeId": ledger_node}, session="no_token")
        purged = admin_post(purge_path, {"confirmNodeId": ledger_node})
        after_purge = {
            "ledger": read_state(ledger_node, "sync"),
            "probe": read_state(probe, "sync"),
        }

        self.stop_server()
        storage_after = self.sample_storage("state-after")
        rows_after = {
            "probe": state_rows(self.db_path, probe),
            "ledger": state_rows(self.db_path, ledger_node),
        }
        sql = {
            "sync entries": sqlite_rows(
                self.db_path,
                "SELECT observed_at, entry_kind FROM node_state_observations WHERE node_id = '"
                + probe
                + "' AND component = 'sync' ORDER BY observed_at",
            ),
            "consensus entries": sqlite_rows(
                self.db_path,
                "SELECT observed_at, entry_kind FROM node_state_observations WHERE node_id = '"
                + probe
                + "' AND component = 'consensus' ORDER BY observed_at",
            ),
            "entries in the pause": sqlite_scalar(
                self.db_path,
                "SELECT COUNT(*) FROM node_state_observations WHERE node_id = '"
                + probe
                + "' AND observed_at >= '"
                + at(29)
                + "' AND observed_at <= '"
                + at(31)
                + "'",
            ),
            "heights at the failed and paused instants": sqlite_scalar(
                self.db_path,
                "SELECT COUNT(*) FROM node_metric_samples WHERE node_id = '"
                + probe
                + "' AND observed_at IN ('"
                + "', '".join([at(27), at(28), at(29), at(30), at(31)])
                + "')",
            ),
            "entries after the recovery": sqlite_scalar(
                self.db_path,
                "SELECT COUNT(*) FROM node_state_observations WHERE node_id IN ('"
                + probe
                + "', '"
                + ledger_node
                + "') AND observed_at > '"
                + at(32)
                + "'",
            ),
            "heights after the recovery": sqlite_scalar(
                self.db_path,
                "SELECT COUNT(*) FROM node_metric_samples WHERE node_id IN ('"
                + probe
                + "', '"
                + ledger_node
                + "') AND observed_at > '"
                + at(32)
                + "'",
            ),
            "state rows": sqlite_scalar(self.db_path, "SELECT COUNT(*) FROM node_state_observations"),
            "state series": sqlite_scalar(self.db_path, "SELECT COUNT(*) FROM node_state_series_state"),
            "skipped series": sqlite_rows(
                self.db_path,
                "SELECT metric, dimension, skipped_count, first_skipped_at, last_skipped_at"
                " FROM capacity_skipped_series WHERE scope_kind = 'node' AND scope_key = '"
                + probe
                + "' ORDER BY metric",
            ),
        }
        plans = {
            "read": state_plan(
                self.db_path, STATE_READ_SQL, (probe, "sync", read_from, read_to, STATE_DEFAULT_LIMIT)
            ),
            "paged": state_plan(
                self.db_path,
                STATE_READ_PAGED_SQL,
                (probe, "sync", read_from, read_to, at(8), STATE_PAGE_LIMIT),
            ),
            "ledger": state_plan(self.db_path, STATE_LEDGER_SQL, (probe, "sync")),
            "heights": state_plan(
                self.db_path,
                STATE_HEIGHT_SQL,
                (probe, STATE_NODE_SERIES[0], read_from, read_to, STATE_DEFAULT_LIMIT),
            ),
        }

        # -- what the arithmetic above must produce ---------------------------
        stored_units = list(range(15)) + [26, 32]
        sync_changes = [at(unit) for unit in (0, 1, 2, 27, 32)]
        sync_anchors = [at(unit) for unit in (8, 14, 26)]
        consensus_changes = [at(unit) for unit in (0, 27, 32)]
        consensus_anchors = [at(unit) for unit in (6, 12, 26)]
        sync_entries = sorted(sync_changes + sync_anchors)
        consensus_entries = sorted(consensus_changes + consensus_anchors)
        entry_count = len([entry for entry in deliveries if entry["kind"] != "state paused"])
        frozen_count = len([entry for entry in deliveries if entry["unit"] <= 28])
        span_seconds = stored_units[-1] * STATE_RHYTHM_SECONDS
        sync_gaps = [
            {"from": at(14), "to": at(26), "kind": "collection_gap", "skipped": None},
            {"from": at(27), "to": at(32), "kind": "protection_pause", "skipped": STATE_PAUSE_REPORTS},
        ]
        consensus_gaps = [
            {"from": at(12), "to": at(26), "kind": "collection_gap", "skipped": None},
            {"from": at(27), "to": at(32), "kind": "protection_pause", "skipped": STATE_PAUSE_REPORTS},
        ]
        height_gaps = [
            {"from": at(14), "to": at(26), "kind": "collection_gap", "skipped": None},
            {"from": at(26), "to": at(32), "kind": "protection_pause", "skipped": STATE_PAUSE_REPORTS},
        ]

        def coverage(instants: list, gaps: list) -> int:
            """What the answer counts as covered: the stretches between
            consecutive entries, less the ones the answer itself calls a gap."""
            total = 0
            for left, right in zip(instants, instants[1:]):
                if any(gap["from"] == left and gap["to"] == right for gap in gaps):
                    continue
                total += int((parse_instant(right) - parse_instant(left)).total_seconds())
            return total

        expect = {
            "sync_changes": sync_changes,
            "sync_anchors": sync_anchors,
            "sync_entries": sync_entries,
            "consensus_changes": consensus_changes,
            "consensus_anchors": consensus_anchors,
            "consensus_entries": consensus_entries,
            "entry_count": entry_count,
            "frozen_count": frozen_count,
            "height_units": stored_units,
            "heights": {
                metric: {
                    "probe": [height(unit, 0, index) for unit in stored_units],
                    "ledger": [height(unit, 1, index) for unit in stored_units],
                }
                for index, metric in enumerate(STATE_NODE_SERIES)
            },
            # The product states one cadence per answered component, and it
            # divides the span by the LEDGER's counted deliveries for BOTH
            # components (crates/platpulse-server/src/state_history.rs:689
            # delivery_cadence_seconds), not by the entries the component happens
            # to have stored: consensus answers the same 1066 s cadence as sync
            # even though it holds three changes and three anchors, not eight.
            "sync_cadence_seconds": span_seconds // (entry_count - 1),
            "consensus_cadence_seconds": span_seconds // (entry_count - 1),
            # The rhythm between stored height samples. The Node metric answer
            # states no cadence at all, so this is only the spacing its items have.
            "height_rhythm_seconds": span_seconds // (len(stored_units) - 1),
            "sync_gaps": sync_gaps,
            "consensus_gaps": consensus_gaps,
            "height_gaps": height_gaps,
            "sync_coverage_seconds": coverage(sync_entries, sync_gaps),
            "consensus_coverage_seconds": coverage(consensus_entries, consensus_gaps),
            "height_coverage_seconds": coverage([at(unit) for unit in stored_units], height_gaps),
            "frozen": {
                "entry_count": frozen_count,
                "last_entry_at": at(27),
                "last_observed_at": at(28),
            },
            "pause": {"from": at(29), "to": at(31), "reports": STATE_PAUSE_REPORTS},
            "skipped_series": sorted(list(STATE_COMPONENTS) + list(STATE_NODE_SERIES)),
            "purge": {
                "state_observations": len(sync_entries) + len(consensus_entries),
                "state_series_state": len(STATE_COMPONENTS),
            },
            "read_window": {
                "from": read_from,
                "to": read_to,
                # Both endpoints are wall-clock instants truncated to the second
                # (instant() at scripts/metric-history-baseline.py:106), so the
                # span the Server derives from them — its windowSeconds — can
                # differ from the requested span by the truncation jitter at each
                # end, which is why the check allows a few seconds.
                "seconds": int((parse_instant(read_to) - parse_instant(read_from)).total_seconds()),
                "tolerance_seconds": 3,
            },
        }

        return {
            "issue": 217,
            "title": "Recorded synchronization and consensus state history",
            "instrument": {
                "agent_id": agent["agent_id"],
                "agents": 1,
                "nodes": 2,
                "probe_node_id": probe,
                "ledger_node_id": ledger_node,
                "components": list(STATE_COMPONENTS),
                "node_series": list(STATE_NODE_SERIES),
                "reports": len(deliveries),
                "rhythm_seconds": STATE_RHYTHM_SECONDS,
                "lead_seconds": STATE_LEAD_SECONDS,
                "anchor_seconds": STATE_ANCHOR_SECONDS,
                "page_limit": STATE_PAGE_LIMIT,
                "pause_reports": STATE_PAUSE_REPORTS,
                "reads": STATE_READS,
                "retention_days": STATE_RETENTION_DAYS,
                "cleanup_batch": STATE_CLEANUP_BATCH,
                "max_state_rows_per_report": MAX_STATE_ROWS_PER_REPORT,
                "read_index": STATE_READ_INDEX,
                "ledger_index": STATE_LEDGER_INDEX,
                "forbidden_plans": list(STATE_READ_FORBIDDEN_PLANS),
                "height_forbidden_plans": list(STATE_HEIGHT_FORBIDDEN_PLANS),
                "mount": mount_conditions(self.state_dir),
                "hardware": hardware_conditions(),
                "database_bytes": storage_after["database_bytes"],
                "sampled_at": storage_after["at"],
            },
            "deliveries": deliveries,
            "expect": expect,
            "reads": {
                "sync": sync_read,
                "consensus": consensus_read,
                "ledger node sync": ledger_sync_read,
                "before purge": still_there,
                "after purge": after_purge,
                "page one": page_one,
                "page two": page_two,
                "refusals": refusals,
                "latency_ms": latency,
                "heights": heights,
            },
            "frozen": frozen,
            "recovery": recovery,
            "purge": {
                "mismatched": mismatched,
                "anonymous": anonymous,
                "unguarded": unguarded,
                "purged": purged,
            },
            "sql": sql,
            "plans": plans,
            "rows_after": rows_after,
            "storage": {"before": storage_before, "after": storage_after},
            "wall_seconds": round(time.perf_counter() - started, 3),
        }

    def phase_storage(self) -> dict:
        self.stop_server()
        tables = (
            "node_metric_samples",
            "host_metric_samples",
            "node_metric_series_state",
            "capacity_skipped_series",
            "capacity_protection_intervals",
            "agent_report_receipts",
            "nodes",
        )
        counts = {}
        for table in tables:
            try:
                counts[table] = sqlite_scalar(self.db_path, "SELECT COUNT(*) FROM " + table)
            except sqlite3.Error as error:
                counts[table] = None
        page = sqlite_scalar(self.db_path, "PRAGMA page_count")
        page_size = sqlite_scalar(self.db_path, "PRAGMA page_size")
        return {
            "row_counts": counts,
            "page_count": page,
            "page_size": page_size,
            "database_bytes": self.db_path.stat().st_size if self.db_path.is_file() else 0,
            "with_wal_bytes": file_bytes(self.db_path),
            "storage_samples": self.storage,
            "bytes_per_optional_sample": self.bytes_per_sample(counts),
        }

    def bytes_per_sample(self, counts: dict):
        optional = (counts.get("node_metric_samples") or 0) + (counts.get("host_metric_samples") or 0)
        if not optional:
            return None
        return round((self.db_path.stat().st_size if self.db_path.is_file() else 0) / optional, 3)

    # -- checks ------------------------------------------------------------

    def evaluate(
        self,
        load: dict,
        restatements: dict,
        release: dict,
        reads: dict,
        multi_node: dict,
        tiers: dict,
        storage: dict,
        host: dict,
        mount: dict,
        state: dict,
    ) -> list:
        full = reads["24h"]
        planned = self.planned_coverage()
        part1 = multi_node["part1"]
        drain = multi_node["drain"]
        ledger_after_load = restatements["before"]
        released_range = reads["refusals"]["released_range"]
        beyond_horizon = reads["refusals"]["beyond_horizon"]
        checks = [
            check(
                "every Report was accepted",
                "every disposition accepted",
                json.dumps({"dispositions": load["dispositions"], "rejections": load["rejections"]}),
                bool(load["dispositions"])
                and all(key in ("accepted", "partially_accepted") for key in load["dispositions"]),
            ),
            check(
                "the raw window holds exactly the observations that were stored",
                len(set(load["accepted_instants"])),
                len(full["observed_instants"]),
                set(full["observed_instants"]) == set(load["accepted_instants"]),
            ),
            check(
                "the ledger counts exactly the stored observations",
                len(set(load["accepted_instants"])),
                ledger_after_load["observationCount"],
                ledger_after_load["observationCount"] == len(set(load["accepted_instants"])),
            ),
            check(
                "the answered window is the requested window",
                full["requested_span_seconds"],
                full["window_seconds"],
                full["window_seconds"] == full["requested_span_seconds"],
            ),
            check(
                "the default window is the declared 24 hour floor and it answers the whole series",
                str(self.window_seconds) + "s (the declared window) with the whole series",
                str(reads["default_window"]["window_seconds"]) + "s with " + str(reads["default_window"]["items"]) + " items",
                reads["default_window"]["window_seconds"] == self.window_seconds
                and reads["default_window"]["items"] == full["items"],
            ),
            check(
                "a carried last-good delivery is a replay, not a new observation",
                "observationCount unchanged, replayedCount +1",
                json.dumps(
                    {
                        "observationCount": restatements["after_replay"]["observationCount"],
                        "replayedCount": restatements["after_replay"]["replayedCount"],
                    }
                ),
                restatements["after_replay"]["observationCount"] == restatements["before"]["observationCount"]
                and restatements["after_replay"]["replayedCount"] == restatements["before"]["replayedCount"] + 1,
            ),
            check(
                "a restated value is a correction, not a new observation",
                "observationCount unchanged, correctedCount +1",
                json.dumps(
                    {
                        "observationCount": restatements["after_correction"]["observationCount"],
                        "correctedCount": restatements["after_correction"]["correctedCount"],
                    }
                ),
                restatements["after_correction"]["observationCount"] == restatements["before"]["observationCount"]
                and restatements["after_correction"]["correctedCount"] == restatements["before"]["correctedCount"] + 1,
            ),
            check(
                "the correction is the value the series answers with",
                restatements["corrected_value"],
                full["newest_value"],
                full["newest_value"] == restatements["corrected_value"],
            ),
            check(
                "the pause is one reported silence, not a zero or a missing point",
                "one protection_pause of "
                + str(planned["planned_straddle_seconds"])
                + "s from "
                + planned["pause_from"]
                + " (the planned instant before the silence) to "
                + planned["pause_to"]
                + " (the planned instant after it) with "
                + str(load["paused_rounds"])
                + " skipped observations",
                json.dumps(full["gaps"]),
                len(full["gaps"]) == 1
                and full["gaps"][0]["kind"] == "protection_pause"
                and full["gaps"][0]["skippedCount"] == load["paused_rounds"]
                and full["gaps"][0]["seconds"] == planned["planned_straddle_seconds"]
                and full["gaps"][0]["from"] == planned["pause_from"]
                and full["gaps"][0]["to"] == planned["pause_to"],
            ),
            check(
                "no paused instant was stored",
                "none of the paused instants appears in the items",
                json.dumps(load["paused_instants"]),
                not any(
                    observed in load["paused_instants"] for observed in full.get("observed_instants", [])
                ),
            ),
            # The oracle is the plan, not the Server rule: recomputing the
            # threshold from the returned samples (coverage_recomputed, kept
            # below as an informational cross-check) could only show that two
            # copies of the same rule agree. A plan whose straddle is longer
            # than the Server threshold has to answer with the plan number.
            check(
                "coverage is proven, not assumed, across the silence",
                json.dumps(
                    {
                        "planned_proven_seconds": planned["planned_proven_seconds"],
                        "planned_stored_rounds": planned["planned_stored_rounds"],
                        "planned_stretches": planned["planned_stretches"],
                        "planned_cadence_seconds": planned["planned_cadence_seconds"],
                        "planned_straddle_seconds": planned["planned_straddle_seconds"],
                        "server_gap_threshold_seconds": planned["server_gap_threshold_seconds"],
                    }
                ),
                json.dumps(
                    {
                        "coverageSeconds": full["series"]["coverageSeconds"],
                        "items": full["items"],
                        "server_rule_cross_check": full["coverage_recomputed"]["proven_seconds"],
                    }
                ),
                full["series"]["coverageSeconds"] == planned["planned_proven_seconds"]
                and full["items"] == planned["planned_stored_rounds"],
            ),
            check(
                "coverage stays below the window that holds a silence",
                "proven seconds < window seconds",
                full["series"]["coverageSeconds"],
                full["series"]["coverageSeconds"] < self.window_seconds,
            ),
            # The release phase delivers one observation 26 hours old, outside the raw
            # retention window, so no row can be stored for it. What the ledger does
            # with that delivery is the assertion. The Server asks its three
            # classification questions in order and, with no sample held at that
            # instant and the instant older than the evidence floor, answers "replay"
            # (crates/platpulse-server/src/metric_history.rs:450-486, design §11.3),
            # so the lifetime count must not move: counting an expired redelivery
            # would let a replayed last-good inflate the one number the ledger exists
            # to keep honest. The ledger entry itself survives, the replay is
            # recorded, and the released instant is still not stored.
            check(
                "an expired delivery keeps its ledger entry, is classified as a replay, and stores no row",
                "observationCount unchanged at "
                + str(release["before"]["observationCount"])
                + ", replayedCount +1, sampledCount unchanged, and the released instant not stored",
                json.dumps(
                    {
                        "before": {
                            "observationCount": release["before"]["observationCount"],
                            "replayedCount": release["before"]["replayedCount"],
                            "sampledCount": release["before"]["sampledCount"],
                            "firstObservedAt": release["before"]["firstObservedAt"],
                        },
                        "after": {
                            "observationCount": release["after"]["observationCount"],
                            "replayedCount": release["after"]["replayedCount"],
                            "sampledCount": release["after"]["sampledCount"],
                            "firstObservedAt": release["after"]["firstObservedAt"],
                        },
                        "released_instant": release["released_instant"],
                        "released_instant_stored": release["released_instant_stored"],
                    }
                ),
                release["after"]["observationCount"] == release["before"]["observationCount"]
                and release["after"]["replayedCount"] == release["before"]["replayedCount"] + 1
                and release["after"]["sampledCount"] == release["before"]["sampledCount"]
                and release["released_instant_stored"] is False,
            ),
            check(
                "a truncated answer carries the newest samples",
                "truncated true, newest item equals the newest stored instant",
                json.dumps(
                    {
                        "items": reads["24h_limited"]["items"],
                        "truncated": reads["24h_limited"]["truncated"],
                        "newest": reads["24h_limited"]["newest_observed_at"],
                    }
                ),
                reads["24h_limited"]["truncated"] is True
                and reads["24h_limited"]["items"] == 10
                and reads["24h_limited"]["newest_observed_at"] == full["newest_observed_at"],
            ),
            check(
                "a series this Node never reported is absent, not zero",
                "observed false with no items and no gaps",
                json.dumps(reads["never_reported"]),
                reads["never_reported"]["series"]["observed"] is False
                and reads["never_reported"]["items"] == 0
                and reads["never_reported"]["gaps"] == 0,
            ),
            check(
                "the series carries no clock suspicion for observations received after they were taken",
                "no sample is clock suspect",
                full["clock_suspect"],
                full["clock_suspect"] is False,
            ),
            check(
                # Issue #213 wrote this as "a range older than the released history
                # is answered as unavailable", because availability followed the raw
                # window's own cutoff then: the 30 hour old range sat entirely
                # outside the released raw history. Issue #214 moved the field to the
                # investigation horizon (crates/platpulse-server/src/http/admin.rs:4643-4649),
                # so that range is answerable now and only a range ending before
                # now - SERVER_HISTORY_HORIZON_DAYS days is still refused. The name
                # keeps the issue #213 name and states the boundary this check really
                # holds the Server to; the #213 statement that changed is quoted here.
                "a range older than the investigation horizon is answered as unavailable "
                "(issue #213 held this range to the released raw history instead)",
                "the 30 hour old range is answered, not refused: availability null, 0 items on this "
                "Node whose raw rows were released, requested start preserved; a range ending more than "
                + str(SERVER_HISTORY_HORIZON_DAYS)
                + " days before now answers unavailable with its requested start preserved, 0 items and "
                "its effective start clamped to now - "
                + str(SERVER_HISTORY_HORIZON_DAYS)
                + " days. The tier Node's same 30 hour old range answers 1 item with grain 1m and source "
                "aggregate, asserted by the availability check in the issue #214 section.",
                json.dumps(
                    {
                        "inside": reads["refusals"]["released_range"],
                        "beyond": reads["refusals"]["beyond_horizon"],
                    }
                ),
                released_range["availability"] is None
                and released_range["items"] == 0
                and released_range["historyHorizonDays"] == SERVER_HISTORY_HORIZON_DAYS
                and parse_instant(released_range["requestedFrom"])
                == parse_instant(released_range["requestedFromParam"])
                and beyond_horizon["availability"] == "unavailable"
                and beyond_horizon["items"] == 0
                and parse_instant(beyond_horizon["requestedFrom"])
                == parse_instant(beyond_horizon["requestedFromParam"])
                and parse_instant(beyond_horizon["effectiveTo"])
                == parse_instant(beyond_horizon["requestedToParam"])
                # The clamp lands exactly on the horizon, which sits 3600s after the
                # requested end; the tolerance only covers the time between the
                # script writing the query and the Server reading it.
                and 3595
                <= (
                    parse_instant(beyond_horizon["effectiveFrom"])
                    - parse_instant(beyond_horizon["requestedToParam"])
                ).total_seconds()
                <= 3605,
            ),
            check(
                "an unusable series token is refused",
                "400 invalid_metric",
                json.dumps(reads["refusals"]["invalid_metric"]),
                reads["refusals"]["invalid_metric"]["status"] == 400
                and reads["refusals"]["invalid_metric"]["code"] == "invalid_metric",
            ),
            check(
                "an inverted range is refused",
                "400 invalid_history_range",
                json.dumps(reads["refusals"]["invalid_range"]),
                reads["refusals"]["invalid_range"]["status"] == 400
                and reads["refusals"]["invalid_range"]["code"] == "invalid_history_range",
            ),
            check(
                "an unknown Node never leaks a series",
                "404 not_found",
                json.dumps(reads["refusals"]["unknown_node"]),
                reads["refusals"]["unknown_node"]["status"] == 404,
            ),
            check(
                "the ledger outlives the raw rows the retention floor released",
                "node_metric_series_state survives while the sample row is gone",
                json.dumps(
                    {
                        "ledger_rows": storage["row_counts"]["node_metric_series_state"],
                        "released_stored": release["released_instant_stored"],
                    }
                ),
                (storage["row_counts"]["node_metric_series_state"] or 0) > 0 and release["released_instant_stored"] is False,
            ),
            # -- the multi-Node arrival and the per-Report cleanup budget ---
            check(
                "every multi-Node Report was accepted",
                str(part1["reports"])
                + " accepted Reports of "
                + str(part1["clone_nodes"] + 1)
                + " Nodes each, with the phase latency and wall time as throughput evidence",
                json.dumps(
                    {
                        "reports": part1["reports"],
                        "accepted": part1["accepted_reports"],
                        "nodes_per_report": part1["clone_nodes"] + 1,
                        "latency_ms": part1["latency_ms"],
                        "wall_seconds": part1["wall_seconds"],
                        "dispositions": part1["dispositions"],
                    }
                ),
                part1["accepted_reports"] == part1["reports"],
            ),
            check(
                "every cloned Node and every series it reported is registered",
                str(part1["clone_nodes"])
                + " Node rows and "
                + str(part1["clone_nodes"] * MULTI_NODE_SERIES_PER_NODE)
                + " series ledger rows",
                json.dumps(
                    {"node_rows": part1["node_rows"], "series_ledger_rows": part1["series_ledger_rows"]}
                ),
                part1["node_rows"] == part1["clone_nodes"]
                and part1["series_ledger_rows"] == part1["clone_nodes"] * MULTI_NODE_SERIES_PER_NODE,
            ),
            check(
                "the burst stored exactly the rows the raw retention window can hold",
                str(part1["planned_stored_rows"])
                + " rows ("
                + str(part1["planned_in_window_rounds"])
                + " of "
                + str(part1["reports"])
                + " rounds inside the Server's raw window x "
                + str(part1["planned_rows_per_report"])
                + " rows per Report), tolerance one round",
                str(part1["stored_rows"])
                + " rows stored, "
                + str(part1["planned_out_of_window_rounds"])
                + " rounds arrived already outside the Server's raw window",
                abs(part1["stored_rows"] - part1["planned_stored_rows"]) <= part1["planned_rows_per_report"],
            ),
            check(
                "a pure replay stores and releases nothing when nothing is due",
                "stored rows unchanged at " + str(part1["replay"]["before"]["total"]),
                json.dumps(part1["replay"]),
                part1["replay"]["after"]["total"] == part1["replay"]["before"]["total"]
                and part1["replay"]["before"]["expired"] == 0
                and part1["replay"]["after"]["expired"] == 0,
            ),
            check(
                "one cleanup releases more expired rows than the previous per-Report bound allowed",
                "one accepted Report releases more than "
                + str(multi_node["falsified_bound"])
                + " expired rows and no more than "
                + str(multi_node["bound_under_test"]),
                json.dumps(
                    {
                        "rows_added_per_report": drain["rows_added_per_report"],
                        "rows_released_per_report": drain["rows_released_per_report"],
                        "max_released_by_one_cleanup": drain["max_released_by_one_cleanup"],
                        "total_released": drain["total_released"],
                        "bound_under_test": multi_node["bound_under_test"],
                        "falsified_bound": multi_node["falsified_bound"],
                    }
                ),
                drain["max_released_by_one_cleanup"] > multi_node["falsified_bound"]
                and drain["max_released_by_one_cleanup"] <= multi_node["bound_under_test"],
            ),
            check(
                "the drain leaves no expired row behind",
                "0 expired rows after the burst, with the anchor rows still inside the window",
                json.dumps(
                    {
                        "expired_rows": drain["expired_rows_after_burst"],
                        "stored_rows": drain["stored_rows_after_burst"],
                        "rows_released_per_report": drain["rows_released_per_report"],
                        "backlog_floor_under_previous_bound": drain["backlog_floor_under_previous_bound"],
                    }
                ),
                drain["expired_rows_after_burst"] == 0 and drain["stored_rows_after_burst"] > 0,
            ),
            check(
                "the decisive replay releases nothing once the backlog is drained",
                "stored rows unchanged at " + str(drain["replay"]["before"]["total"]),
                json.dumps(drain["replay"]),
                drain["replay"]["after"]["total"] == drain["replay"]["before"]["total"]
                and drain["replay"]["before"]["expired"] == 0
                and drain["replay"]["before"]["total"] > 0,
            ),
        ]
        # -- the aggregate tiers (issue #214) --------------------------------
        tier_planned = tiers["planned"]
        thirty = tiers["reads"]["30d"]
        raw_only_read = tiers["reads"]["20h"]
        ten_day = tiers["reads"]["10d"]
        four_day = tiers["reads"]["4d"]
        inside_horizon = tiers["horizon"]["inside"]
        beyond_horizon = tiers["horizon"]["beyond"]
        walk = tiers["walk"]
        tier_storage = tiers["storage"]
        plants = {plant["kind"]: plant for plant in tiers["planted"]["plants"]}
        expired_plant = plants["expired"]
        surviving_plant = plants["surviving"]
        later_report = tiers["planted"]["later_report"]
        segment_grains = [segment["grain"] for segment in thirty["segments"]]
        one_minute_grain = thirty["grains"].get("1m") or {}
        five_minute_grain = thirty["grains"].get("5m") or {}
        raw_grain = thirty["grains"].get("raw") or {}
        four_day_grain = four_day["grains"].get("1m") or {}
        ten_day_from = instant(-10 * 86400)
        ten_day_to = instant(-8 * 86400)
        raw_samples_in_stretch = sum(
            1 for value in thirty["raw_instants"] if ten_day_from <= value < ten_day_to
        )
        rows_expected = tier_planned["aggregate_rows"] + 3 * TIER_SERIES
        checks.extend([
            check(
                "the tier phase seeded a month of hourly Reports and every one was accepted",
                "seeded "
                + str(TIER_REPORTS)
                + " reports over a declared "
                + str(TIER_SPAN_SECONDS)
                + "s span, all accepted, no rejections",
                json.dumps(
                    {
                        "seeded": tiers["seeded_reports"],
                        "accepted": tier_planned["accepted_reports"],
                        "rejections": tiers["rejections"],
                        "declared_span_seconds": tiers["instants"]["declared_span_seconds"],
                        "oldest_age_seconds": tiers["instants"]["oldest_age_seconds"],
                        "write_latency_ms": tiers["write_latency_ms"],
                        "seed_wall_seconds": tiers["seed_wall_seconds"],
                    }
                ),
                tiers["seeded_reports"] == TIER_REPORTS
                and tier_planned["accepted_reports"] == TIER_REPORTS
                and not tiers["rejections"],
            ),
            check(
                "the backdated month is counted into the tiers without being stored as raw samples",
                "ledger "
                + str(TIER_REPORTS)
                + " observations, node_metric_samples rows "
                + str(tier_planned["raw_rows"])
                + ", 0 expired",
                json.dumps(
                    {
                        "ledger_observations": thirty["observation_count"],
                        "ledger_sampled": thirty["sampled_count"],
                        "ledger_replayed": thirty["replayed_count"],
                        "ledger_corrected": thirty["corrected_count"],
                        "raw_rows": tiers["raw_rows"],
                        "aggregate_rows": tiers["tier_rows"],
                    }
                ),
                thirty["observation_count"] == TIER_REPORTS
                and tiers["raw_rows"]["total"] == tier_planned["raw_rows"]
                and tiers["raw_rows"]["expired"] == 0,
            ),
            check(
                "each counted hour became one bucket per series in each tier",
                "1m rows "
                + str(tier_planned["one_minute_rows"])
                + " and 5m rows "
                + str(tier_planned["five_minute_rows"])
                + " (+/- "
                + str(TIER_SERIES)
                + " series rows for the moving tier floor), "
                + " distinct (node, metric, bucket_start) coordinates between "
                + str(TIER_REPORTS * TIER_SERIES)
                + " (the 5m tier alone) and "
                + str(tier_planned["aggregate_rows"])
                + " (every row its own: a 1m start that coincides with a 5m start merges the two)",
                json.dumps(
                    {
                        "tier_rows": tiers["tier_rows"],
                        "planned": {
                            "one_minute_rows": tier_planned["one_minute_rows"],
                            "five_minute_rows": tier_planned["five_minute_rows"],
                            "buckets_per_series": {
                                "one_minute": tier_planned["one_minute_buckets_per_series"],
                                "five_minute": tier_planned["five_minute_buckets_per_series"],
                            },
                        },
                    }
                ),
                abs(tiers["tier_rows"]["one_minute"] - tier_planned["one_minute_rows"]) <= TIER_SERIES
                and abs(tiers["tier_rows"]["five_minute"] - tier_planned["five_minute_rows"]) <= TIER_SERIES
                and tiers["tier_rows"]["distinct_series"] == TIER_SERIES
                # The 5m tier alone contributes 720 bucket starts per series; a 1m
                # start on a 5 minute boundary coincides with a 5m start and merges
                # into one coordinate, and one off the boundary adds a row, so the
                # exact count depends on which minute of the hour the run's clock
                # sits on. The bounds hold either way.
                and TIER_REPORTS * TIER_SERIES
                <= tiers["tier_rows"]["distinct_buckets"]
                <= tier_planned["aggregate_rows"],
            ),
            check(
                "the tiers cost one row per bucket at a measurable bytes per bucket",
                "rows added "
                + str(rows_expected)
                + " (tier Node "
                + str(tier_planned["aggregate_rows"])
                + " + surviving plant "
                + str(TIER_SERIES)
                + " + later Report "
                + str(2 * TIER_SERIES)
                + "), 0 < bytes per bucket < page size "
                + str(tier_storage["page_size"]),
                json.dumps(
                    {
                        "rows_added": tier_storage["rows_added"],
                        "aggregate_rows_before": tier_storage["aggregate_rows_before"],
                        "aggregate_rows_after": tier_storage["aggregate_rows_after"],
                        "aggregate_footprint_after": tier_storage["aggregate_footprint_after"],
                        "bytes_per_bucket": tier_storage["bytes_per_bucket"],
                        "bytes_per_bucket_added": tier_storage["bytes_per_bucket_added"],
                        "bytes_per_tier": tier_storage["bytes_per_tier"],
                        "pages_added": tier_storage["pages_added"],
                        "database_bytes_added": tier_storage["database_bytes_added"],
                    }
                ),
                tier_storage["rows_added"] == rows_expected
                and tier_storage["aggregate_footprint_after"]["available"]
                and (tier_storage["bytes_per_bucket"] or 0) > 0
                and (tier_storage["bytes_per_bucket"] or 0) < (tier_storage["page_size"] or 0),
            ),
            check(
                "a read wholly inside the raw window is answered by raw samples alone",
                "grain raw, sources ['raw'], items "
                + str(tier_planned["raw_read_items"])
                + ", 0 aggregate points",
                json.dumps(
                    {
                        "items": raw_only_read["items"],
                        "grain": raw_only_read["grain"],
                        "sources": raw_only_read["sources"],
                        "aggregate_points": raw_only_read["aggregate_points"],
                        "grains": raw_only_read["grains"],
                        "segments": raw_only_read["segments"],
                        "truncated": raw_only_read["truncated"],
                        "coverage_seconds": raw_only_read["coverage_seconds"],
                        "window_seconds": raw_only_read["window_seconds"],
                        "gaps": raw_only_read["gaps"],
                        "gap_kinds": raw_only_read["gap_kinds"],
                        "latency_ms": raw_only_read["latency_ms"],
                        "payload_bytes": raw_only_read["payload_bytes"],
                    }
                ),
                raw_only_read["grain"] == "raw"
                and raw_only_read["sources"] == ["raw"]
                and raw_only_read["aggregate_points"] == 0
                and raw_only_read["items"] == tier_planned["raw_read_items"],
            ),
            check(
                "one 30 day answer mixes all three tiers, newest tier first",
                "items "
                + str(tier_planned["thirty_day_items"])
                + " (raw "
                + str(tier_planned["raw_instants"])
                + " + 1m "
                + str(tier_planned["one_minute_region_instants"])
                + " + 5m "
                + str(tier_planned["five_minute_region_instants"])
                + "), segments 5m/1m/raw (oldest region first), truncated false, continuation null",
                json.dumps(
                    {
                        "items": thirty["items"],
                        "grains": thirty["grains"],
                        "segments": thirty["segments"],
                        "truncated": thirty["truncated"],
                        "continuation": thirty["continuation"],
                        "availability": thirty["availability"],
                        "coverage_seconds": thirty["coverage_seconds"],
                        "window_seconds": thirty["window_seconds"],
                        "gaps": thirty["gaps"],
                        "gap_kinds": thirty["gap_kinds"],
                        "gap_max_seconds": thirty["gap_max_seconds"],
                        "latency_ms": thirty["latency_ms"],
                        "payload_bytes": thirty["payload_bytes"],
                    }
                ),
                thirty["items"] == tier_planned["thirty_day_items"]
                and raw_grain.get("points", 0) == tier_planned["raw_instants"]
                and one_minute_grain.get("points", 0) == tier_planned["one_minute_region_instants"]
                and five_minute_grain.get("points", 0) == tier_planned["five_minute_region_instants"]
                and segment_grains == ["5m", "1m", "raw"]
                and thirty["truncated"] is False
                and thirty["continuation"] is None,
            ),
            check(
                "a read older than the raw window is answered by buckets, and widening cannot recover raw samples",
                "grain '5m', sources ['aggregate'], items "
                + str(tier_planned["ten_day_items"])
                + ", 0 raw samples in that stretch even inside the 30 day answer (the Server's tier regions put 1m at day 7 up to the raw cutoff and 5m at day 30 up to day 7, so a day 10..8 read is a 5m read; the ticket expected 1m there)",
                json.dumps(
                    {
                        "items": ten_day["items"],
                        "grain": ten_day["grain"],
                        "sources": ten_day["sources"],
                        "aggregate_points": ten_day["aggregate_points"],
                        "segments": ten_day["segments"],
                        "truncated": ten_day["truncated"],
                        "coverage_seconds": ten_day["coverage_seconds"],
                        "latency_ms": ten_day["latency_ms"],
                        "payload_bytes": ten_day["payload_bytes"],
                        "raw_samples_in_stretch_of_thirty_day_answer": raw_samples_in_stretch,
                    }
                ),
                ten_day["items"] == tier_planned["ten_day_items"]
                and ten_day["grain"] == "5m"
                and ten_day["sources"] == ["aggregate"]
                and ten_day["aggregate_points"] == ten_day["items"]
                and raw_samples_in_stretch == 0,
            ),
            check(
                "a read between day 7 and the raw cutoff is answered by 1m buckets that keep the spike and the floor",
                "grain '1m', items "
                + str(tier_planned["four_day_items"])
                + ", min "
                + str(TIER_VALUE_BASE)
                + ", max "
                + str(TIER_SPIKE_CPU)
                + ", no item without a sampleCount",
                json.dumps(
                    {
                        "items": four_day["items"],
                        "grain": four_day["grain"],
                        "sources": four_day["sources"],
                        "grains": four_day["grains"],
                        "segments": four_day["segments"],
                        "without_samples": four_day["without_samples"],
                        "truncated": four_day["truncated"],
                        "latency_ms": four_day["latency_ms"],
                        "payload_bytes": four_day["payload_bytes"],
                    }
                ),
                four_day["items"] == tier_planned["four_day_items"]
                and four_day["grain"] == "1m"
                and four_day["sources"] == ["aggregate"]
                and four_day["aggregate_points"] == four_day["items"]
                and four_day_grain.get("maxValue") == TIER_SPIKE_CPU
                and four_day_grain.get("minValue") == TIER_VALUE_BASE
                and four_day["without_samples"] == 0,
            ),
            check(
                "the paging walk visits every coordinate exactly once",
                "pages "
                + str(tier_planned["walk_pages"])
                + " at limit "
                + str(TIER_PAGE_LIMIT)
                + ", items "
                + str(tier_planned["thirty_day_items"])
                + ", 0 repeats, 0 skipped, same set as the single read",
                json.dumps(
                    {
                        "pages": walk["pages"],
                        "items": walk["items"],
                        "distinct_coordinates": walk["distinct_coordinates"],
                        "repeats": walk["repeats"],
                        "missing_from_walk": walk["missing_from_walk"],
                        "extra_in_walk": walk["extra_in_walk"],
                        "matches_single_read": walk["matches_single_read"],
                        "grain_counts": walk["grain_counts"],
                        "without_samples": walk["without_samples"],
                        "bounded": walk["bounded"],
                        "latency_ms": walk["latency_ms"],
                        "payload_bytes_total": walk["payload_bytes_total"],
                        "first_pages": walk["first_pages"],
                        "last_pages": walk["last_pages"],
                    }
                ),
                walk["pages"] == tier_planned["walk_pages"]
                and walk["items"] == tier_planned["thirty_day_items"]
                and walk["repeats"] == 0
                and walk["distinct_coordinates"] == tier_planned["thirty_day_items"]
                and walk["missing_from_walk"] == 0
                and walk["extra_in_walk"] == 0
                and walk["matches_single_read"]
                and not walk["bounded"],
            ),
            check(
                "every continuation cursor moves strictly backwards and the walk ends untruncated",
                "0 pages out of order, 0 cursors not older, last page truncated false with continuation null",
                json.dumps(
                    {
                        "pages": walk["pages"],
                        "out_of_order_pages": walk["out_of_order_pages"],
                        "cursor_not_older": walk["cursor_not_older"],
                        "strictly_descending": walk["strictly_descending"],
                        "last_page_items": walk["last_page_items"],
                        "last_truncated": walk["last_truncated"],
                        "last_continuation": walk["last_continuation"],
                    }
                ),
                walk["out_of_order_pages"] == 0
                and walk["cursor_not_older"] == 0
                and walk["strictly_descending"]
                and walk["last_truncated"] is False
                and walk["last_continuation"] is None,
            ),
            check(
                "a 31 day instant is counted, then released by both tiers",
                "0 rows in both tiers for the planted Node and no bucket in either grain",
                json.dumps(
                    {
                        "age_seconds": expired_plant["age_seconds"],
                        "disposition": expired_plant["disposition"],
                        "tier_rows": expired_plant["tier_rows_after_own_report"],
                        "buckets": expired_plant["buckets_after_own_report"],
                        "released_by_later_report": expired_plant["released_by_later_report"],
                    }
                ),
                expired_plant["disposition"] == "accepted"
                and expired_plant["tier_rows_after_own_report"]["one_minute"] == 0
                and expired_plant["tier_rows_after_own_report"]["five_minute"] == 0
                and expired_plant["buckets_after_own_report"]["one_minute"]["found"] == 0
                and expired_plant["buckets_after_own_report"]["five_minute"]["found"] == 0,
            ),
            check(
                "a 29 day instant keeps its 5m bucket and loses its 1m bucket",
                "1m rows 0, 5m rows "
                + str(TIER_SERIES)
                + ", one cpu bucket holding 44.5 with sampleCount 1",
                json.dumps(
                    {
                        "age_seconds": surviving_plant["age_seconds"],
                        "disposition": surviving_plant["disposition"],
                        "tier_rows": surviving_plant["tier_rows_after_own_report"],
                        "buckets": surviving_plant["buckets_after_own_report"],
                        "released_by_later_report": surviving_plant["released_by_later_report"],
                    }
                ),
                surviving_plant["disposition"] == "accepted"
                and surviving_plant["tier_rows_after_own_report"]["one_minute"] == 0
                and surviving_plant["tier_rows_after_own_report"]["five_minute"] == TIER_SERIES
                and surviving_plant["buckets_after_own_report"]["one_minute"]["found"] == 0
                and surviving_plant["buckets_after_own_report"]["five_minute"]["found"] == 1
                and (surviving_plant["buckets_after_own_report"]["five_minute"]["row"] or {}).get("sample_count")
                == 1
                and (surviving_plant["buckets_after_own_report"]["five_minute"]["row"] or {}).get("min_value")
                == 44.5
                and (surviving_plant["buckets_after_own_report"]["five_minute"]["row"] or {}).get("max_value")
                == 44.5,
            ),
            check(
                "a later Report's cleanup pass releases nothing more for the planted instants",
                "the later Report is accepted, releases 0 rows in both tiers for both plants, lands in both tiers itself, and leaves the tier Node's rows unchanged",
                json.dumps(
                    {
                        "later_report": later_report,
                        "expired_released": expired_plant["released_by_later_report"],
                        "surviving_released": surviving_plant["released_by_later_report"],
                        "tier_node_rows_after_probes": tier_storage["tier_row_counts_for_node"],
                        "tier_node_rows_before_probes": tiers["tier_rows"],
                    }
                ),
                later_report["disposition"] == "accepted"
                and later_report["tier_rows"]["one_minute"] == TIER_SERIES
                and later_report["tier_rows"]["five_minute"] == TIER_SERIES
                and expired_plant["released_by_later_report"] == {"one_minute": 0, "five_minute": 0}
                and surviving_plant["released_by_later_report"] == {"one_minute": 0, "five_minute": 0}
                and tier_storage["tier_row_counts_for_node"] == tiers["tier_rows"],
            ),
            check(
                "availability follows the investigation horizon, not the raw window",
                "a 30 hour old range inside the 30 day horizon is answered rather than refused: "
                "availability null, its one hourly instant answered by the 1m tier (1 item, grain "
                "1m, source aggregate); a range that ends before the whole horizon answers "
                "unavailable with its requested start preserved, 0 items and its effective start "
                "clamped to the horizon. The issue #213 check above still expects the raw "
                "window's rule for that same 30 hour old range.",
                json.dumps(
                    {
                        "inside": {
                            "availability": inside_horizon["availability"],
                            "items": inside_horizon["items"],
                            "grain": inside_horizon["grain"],
                            "sources": inside_horizon["sources"],
                            "grains": inside_horizon["grains"],
                            "horizon_days": inside_horizon["history_horizon_days"],
                            "requested_from": inside_horizon["requested_from"],
                            "effective_from": inside_horizon["effective_from"],
                        },
                        "beyond": {
                            "availability": beyond_horizon["availability"],
                            "items": beyond_horizon["items"],
                            "requested_from": beyond_horizon["requested_from"],
                            "effective_from": beyond_horizon["effective_from"],
                            "effective_to": beyond_horizon["effective_to"],
                        },
                    }
                ),
                inside_horizon["availability"] is None
                # The tier Node counted one hourly instant there, past the raw
                # window and inside the 1 minute tier, so the answer carries that
                # one bucket and nothing else.
                and inside_horizon["items"] == 1
                and inside_horizon["grain"] == "1m"
                and inside_horizon["sources"] == ["aggregate"]
                and inside_horizon["history_horizon_days"]
                == TIER_FIVE_MINUTE_WINDOW_SECONDS // 86400
                and beyond_horizon["availability"] == "unavailable"
                and beyond_horizon["items"] == 0
                and beyond_horizon["requested_from"] is not None
                and beyond_horizon["effective_from"] > beyond_horizon["requested_from"],
            ),
        ])

        # -- issue #215: the Host family stored once per Agent --
        host_bounds = host["bounds"]
        host_family_before = sorted({row["metric"] for row in host["family_before"]})
        host_dimensions_before = sorted({row["dimension"] for row in host["family_before"]})
        host_family_after = sorted({row["metric"] for row in host["family_after"]})
        host_dimensions_after = sorted({row["dimension"] for row in host["family_after"]})
        mount_paths = sorted(mount["mount_path"] for mount in host["planted"]["mounts"])
        mount_series_expected = 2 * len(mount_paths)
        agent_reads = host["routes"]["reads"]
        node_reads = host["routes"]["by_node"]
        mounts_read = host["planted"]["mount_reads"]
        checks.extend([
            check(
                "the declared Host family is the Server's ten series",
                "DECLARED_HOST_FAMILY equals the eight shared series plus the two storage series, "
                "and no Swap or disk-IO token is part of either",
                json.dumps(
                    {
                        "declared": host_bounds["declared_host_family"],
                        "family": host_bounds["measured_family"],
                        "shared": host_bounds["shared_series"],
                        "mount": host_bounds["mount_series"],
                        "forbidden_in_family": host_bounds["forbidden_in_family"],
                    }
                ),
                host_bounds["declared_host_family"] == host_bounds["measured_family"]
                and host_bounds["measured_family"] == sorted(HOST_SHARED_SERIES + HOST_MOUNT_SERIES)
                and host_bounds["forbidden_in_family"] == [],
            ),
            check(
                "the Host family stays inside the mount contract and the cleanup batches",
                "MAX_HOST_MOUNTS "
                + str(MAX_HOST_MOUNTS)
                + ", so one maximal Host Report is "
                + str(SERVER_MAX_HOST_ROWS_PER_REPORT)
                + " rows: the raw batch "
                + str(HOST_METRIC_CLEANUP_BATCH)
                + " and the aggregate batch "
                + str(AGGREGATE_CLEANUP_BATCH)
                + " both cover it",
                json.dumps(host_bounds),
                host_bounds["max_host_mounts"] == 128
                and host_bounds["max_host_rows_per_report"] == 264
                and host_bounds["server_max_host_rows_per_report"] == 266
                and host_bounds["host_metric_cleanup_batch"] == 512
                and host_bounds["aggregate_cleanup_batch"] == 4096
                and host_bounds["host_metric_cleanup_batch"] >= host_bounds["server_max_host_rows_per_report"]
                and host_bounds["aggregate_cleanup_batch"] >= host_bounds["server_max_host_rows_per_report"],
            ),
            check(
                "every Report above stored the eight shared Host series and no mount series",
                "the fixture states no mount, so the family holds exactly "
                + str(len(HOST_SHARED_SERIES))
                + " series, all of them with an empty dimension",
                json.dumps(
                    {
                        "metrics": host_family_before,
                        "dimensions": host_dimensions_before,
                        "ledger_rows": len(host["ledger_before"]),
                        "series": [
                            {"metric": row["metric"], "dimension": row["dimension"], "samples": row["samples"]}
                            for row in host["family_before"]
                        ],
                    }
                ),
                host_family_before == sorted(HOST_SHARED_SERIES)
                and host_dimensions_before == [""]
                and len(host["ledger_before"]) == len(HOST_SHARED_SERIES),
            ),
            check(
                "a Host row is owned by its Agent and dimensioned by a mount, never by a Node",
                "host_metric_samples carries agent_id and dimension and no node_id column",
                json.dumps({"columns": host["columns"]}),
                "agent_id" in host["columns"]
                and "dimension" in host["columns"]
                and "node_id" not in host["columns"],
            ),
            check(
                "the Host family does not scale with the Node count",
                str(host["nodes"])
                + " Nodes of the one Agent share "
                + str(len(HOST_SHARED_SERIES) + mount_series_expected)
                + " series: "
                + str(len(HOST_SHARED_SERIES))
                + " shared plus "
                + str(mount_series_expected)
                + " for the two planted mounts",
                json.dumps(
                    {
                        "nodes": host["nodes"],
                        "nodes_with_their_own_series": host["nodes_with_their_own_series"],
                        "ledger_rows": len(host["ledger_after"]),
                        "ledger_metrics": sorted({row["metric"] for row in host["ledger_after"]}),
                        "family_after": host_family_after,
                        "dimensions_after": host_dimensions_after,
                    }
                ),
                len(host["ledger_after"]) == len(HOST_SHARED_SERIES) + mount_series_expected
                and host_family_after == sorted(HOST_FAMILY)
                and host_dimensions_after == sorted([""] + mount_paths)
                and host["nodes"] > len(host["ledger_after"])
                and host["nodes_with_their_own_series"] > len(host["ledger_after"]),
            ),
            check(
                "a mount path names its own storage series",
                "the planted Report is accepted and its two mounts are read back at "
                + host["planted"]["observed_at"]
                + " with the used bytes it reported",
                json.dumps(
                    {
                        "observed_at": host["planted"]["observed_at"],
                        "disposition": host["planted"]["disposition"],
                        "reason": host["planted"]["reason"],
                        "mounts": host["planted"]["mounts"],
                        "reads": mounts_read,
                    }
                ),
                host["planted"]["disposition"] == "accepted"
                and all(
                    mounts_read[path]["items"] == 1
                    and mounts_read[path]["dimension"] == path
                    and mounts_read[path]["points"] == [[host["planted"]["observed_at"], mount["used_bytes"]]]
                    for path, mount in (
                        (mount["mount_path"], mount) for mount in host["planted"]["mounts"]
                    )
                ),
            ),
            check(
                "a storage series with no mount path holds nothing",
                "disk_used_bytes is only ever stored under the mount path that reported it, so the "
                "dimension-less series answers 0 items and an empty ledger",
                json.dumps(host["mountless_read"]),
                host["mountless_read"]["items"] == 0
                and host["mountless_read"]["dimension"] == ""
                and host["mountless_read"]["series"]["observationCount"] == 0,
            ),
            check(
                "every Node of the Agent reads the shared series the Agent route serves",
                "cpu_percent answers the same "
                + str(agent_reads["cpu_percent"]["items"])
                + " points through the Agent route and through "
                + str(len(node_reads))
                + " of its Nodes, and the Agent route names the Agent as the owner",
                json.dumps(
                    {
                        "agent_items": {metric: agent_reads[metric]["items"] for metric in sorted(agent_reads)},
                        "agent_scope": {
                            "scope_kind": agent_reads["cpu_percent"]["scope_kind"],
                            "scope_key": agent_reads["cpu_percent"]["scope_key"],
                            "node_id": agent_reads["cpu_percent"]["node_id"],
                            "dimension": agent_reads["cpu_percent"]["dimension"],
                            "gaps": agent_reads["cpu_percent"]["gaps"],
                        },
                        "nodes": {
                            node_id: {
                                "items": row["items"],
                                "scope_kind": row["scope_kind"],
                                "scope_key": row["scope_key"],
                                "node_id": row["node_id"],
                                "same_points": row["points"] == agent_reads["cpu_percent"]["points"],
                            }
                            for node_id, row in sorted(node_reads.items())
                        },
                        "latency_ms": [round(value, 3) for value in host["routes"]["latency_ms"]],
                        "payload_bytes": {
                            metric: agent_reads[metric]["payload_bytes"] for metric in sorted(agent_reads)
                        },
                    }
                ),
                agent_reads["cpu_percent"]["items"] > 0
                and agent_reads["cpu_percent"]["scope_kind"] == "host"
                and agent_reads["cpu_percent"]["scope_key"] == host["agent_id"]
                and agent_reads["cpu_percent"]["node_id"] is None
                and all(
                    row["points"] == agent_reads["cpu_percent"]["points"] for row in node_reads.values()
                )
                and all(
                    agent_reads[metric]["items"] == agent_reads["cpu_percent"]["items"] for metric in agent_reads
                ),
            ),
            check(
                "every route refuses the other family's series",
                "the Agent route and the Node host route answer 400 invalid_metric for a Node series, "
                "and the Node route answers 400 invalid_metric for a Host series",
                json.dumps(host["refusals"]),
                all(
                    row["status"] == 400 and row["code"] == "invalid_metric"
                    for row in host["refusals"].values()
                ),
            ),
            check(
                "the raw window releases the Host rows the same way it releases Node rows",
                "no Host row is left at or before the released instant " + host["released_instant"],
                json.dumps(
                    {
                        "released_instant": host["released_instant"],
                        "rows_at_or_before": host["rows_at_or_before_the_released_instant"],
                    }
                ),
                host["rows_at_or_before_the_released_instant"] == 0,
            ),
            check(
                "the Host family footprint is measured rather than projected",
                "dbstat reports the raw table and every index on it, so the bytes per row are measured",
                json.dumps(host["footprint"]),
                bool(host["footprint"]["available"])
                and host["footprint"]["samples"] > 0
                and host["footprint"]["raw_bytes"] > 0
                and host["footprint"]["bytes_per_sample"] is not None,
            ),
        ])
        # -- issue #216: the storage family answered per mount path ---------
        mount_bounds = mount["bounds"]
        coverage = mount["coverage"]
        mount_read = coverage["read"]
        order_paths = [row[0] for row in coverage["order"]]
        answered = coverage["answered"]
        dropped = coverage["dropped"]
        retired_entry = coverage["retired_entry"]
        live_entry = coverage["live_entry"]
        threshold = coverage["read"]["silence_threshold_seconds"]
        released = mount["released"]
        released_entry = released["entry"]
        solo = mount["solo"]
        solo_read = solo["read"]
        agent_rows = mount["rows"]["agent"]
        solo_rows = mount["rows"]["solo"]
        checks.extend([
            check(
                "the mount list declares the bound and the two storage series",
                "mountLimit "
                + str(MOUNT_COVERAGE_LIMIT)
                + " with usedMetric "
                + MOUNT_USED_METRIC
                + " and capacityMetric "
                + MOUNT_CAPACITY_METRIC
                + ", the two series of the declared mount family",
                json.dumps(
                    {
                        "mount_limit": mount_read["mount_limit"],
                        "used_metric": mount_read["used_metric"],
                        "capacity_metric": mount_read["capacity_metric"],
                        "declared_mount_series": list(HOST_MOUNT_SERIES),
                    }
                ),
                mount_read["mount_limit"] == MOUNT_COVERAGE_LIMIT
                and (mount_read["used_metric"], mount_read["capacity_metric"])
                == (MOUNT_USED_METRIC, MOUNT_CAPACITY_METRIC)
                and sorted([mount_read["used_metric"], mount_read["capacity_metric"]])
                == sorted(HOST_MOUNT_SERIES),
            ),
            check(
                "the mount list states the silence bound of the cadence it measured",
                "silenceThresholdSeconds is gap_threshold_seconds(cadenceSeconds): the clamp of "
                + str(MAX_OBSERVED_CADENCE_SECONDS)
                + " seconds times "
                + str(GAP_CADENCE_FACTOR)
                + " floored at "
                + str(MIN_GAP_SECONDS)
                + ", measured over "
                + str(COVERAGE_CADENCE_SAMPLES)
                + " of the Agent's newest Host observations",
                json.dumps(
                    {
                        "cadence_seconds": mount_read["cadence_seconds"],
                        "silence_threshold_seconds": mount_read["silence_threshold_seconds"],
                        "gap_threshold_seconds": gap_threshold_seconds(mount_read["cadence_seconds"])
                        if isinstance(mount_read["cadence_seconds"], int)
                        else None,
                    }
                ),
                isinstance(mount_read["cadence_seconds"], int)
                and mount_read["cadence_seconds"] >= 1
                and mount_read["silence_threshold_seconds"]
                == gap_threshold_seconds(mount_read["cadence_seconds"]),
            ),
            check(
                "the cadence the mount list states is the rhythm the Agent is keeping now",
                "cadenceSeconds is the newest interval ("
                + str(mount["bounds"]["rhythm_seconds"])
                + "s, the rhythm planted above the newest observation the phases above left) rather"
                " than the fastest interval the ledger still holds (those phases left a pair of"
                " observations one second apart), so the silence bound follows the rhythm the Agent is"
                " keeping and not the fastest interval still stored",
                json.dumps(
                    {
                        "cadence_seconds": mount_read["cadence_seconds"],
                        "planted_rhythm_seconds": mount["bounds"]["rhythm_seconds"],
                    }
                ),
                mount_read["cadence_seconds"] == mount["bounds"]["rhythm_seconds"],
            ),
            check(
                "an Agent whose only Report cannot measure a cadence is never called silent",
                "cadenceSeconds 0 with silenceThresholdSeconds 0, every state unknown, every silentSeconds"
                " null, and both readings of both of its paths still answered",
                json.dumps({"read": solo_read, "entries": solo["entries"]}),
                solo_read["cadence_seconds"] == 0
                and solo_read["silence_threshold_seconds"] == 0
                and solo_read["states"] == ["unknown"]
                and solo_read["mounts"] == len(solo["paths"])
                and solo_read["truncated"] is False
                and all(entry["silent_seconds"] is None for entry in solo["entries"])
                and all(
                    entry["answered"]
                    and entry["used_observed"] is True
                    and entry["used_latest_value"] == solo["expected"][entry["mount_path"]]
                    and entry["capacity_observed"] is True
                    and entry["capacity_latest_value"]
                    == solo["expected"][entry["mount_path"]] + (1 << 33)
                    for entry in solo["entries"]
                ),
            ),
            check(
                "the mount list is answered per Agent",
                "the second Agent's list answers its own "
                + str(len(solo["paths"]))
                + " paths and none of the "
                + str(coverage["order_size"]),
                json.dumps(
                    {
                        "solo_agent_id": mount["solo_agent_id"],
                        "agent_id": mount["agent_id"],
                        "solo_paths": [entry["mount_path"] for entry in solo["entries"]],
                        "first_agent_first_path": answered[0] if answered else None,
                    }
                ),
                sorted(entry["mount_path"] for entry in solo["entries"]) == sorted(solo["paths"])
                and not set(solo["paths"]) & set(order_paths)
                and mount["solo_agent_id"] != mount["agent_id"],
            ),
            check(
                "a released newest reading is answered as unknown with its release boundary",
                "the path is answered and observed, hands out no value rather than a zero, and states"
                " releasedBefore at or after "
                + released["observed_at"]
                + " with its last observation at that instant",
                json.dumps(released_entry),
                released_entry["answered"] is True
                and released_entry["used_observed"] is True
                and released_entry["used_latest_value"] is None
                and isinstance(released_entry["used_released_before"], str)
                and released_entry["used_released_before"] >= released["observed_at"]
                and released_entry["used_first_observed_at"] == released["observed_at"]
                and released_entry["used_last_observed_at"] == released["observed_at"]
                and released_entry["used_observation_count"] == 1
                and released_entry["capacity_observed"] is True
                and released_entry["capacity_latest_value"] is None
                and released_entry["capacity_released_before"]
                == released_entry["used_released_before"],
            ),
            check(
                "a released path is judged on the silence axis, not called unknown",
                "state silent with the real seconds since its newest reading, which is a path that"
                " stopped reporting rather than a series that never reported",
                json.dumps(
                    {
                        "state": released_entry["observation_state"],
                        "silent_seconds": released_entry["silent_seconds"],
                        "silence_threshold_seconds": released["read"]["silence_threshold_seconds"],
                        "released_at": released["observed_at"],
                    }
                ),
                released_entry["observation_state"] == "silent"
                and isinstance(released_entry["silent_seconds"], (int, float))
                and released_entry["silent_seconds"]
                > (released["read"]["silence_threshold_seconds"] or 0),
            ),
            check(
                "the mount list answers at most the declared number of paths",
                "mounts "
                + str(MOUNT_COVERAGE_LIMIT)
                + " and truncated true for an Agent holding "
                + str(coverage["order_size"])
                + " mount paths",
                json.dumps(
                    {
                        "mounts": mount_read["mounts"],
                        "mount_limit": mount_read["mount_limit"],
                        "truncated": mount_read["truncated"],
                        "order_size": coverage["order_size"],
                        "planted": mount["planted"],
                    }
                ),
                len(answered) == MOUNT_COVERAGE_LIMIT
                and coverage["read"]["truncated"] is True
                and coverage["read"]["mount_limit"] == MOUNT_COVERAGE_LIMIT
                and coverage["order_size"] > MOUNT_COVERAGE_LIMIT,
            ),
            check(
                "the list answers the newest paths first",
                "the answered paths are the first "
                + str(MOUNT_COVERAGE_LIMIT)
                + " of the order recomputed from host_metric_series_state by the Server's own ORDER BY",
                json.dumps(
                    {
                        "answered": len(answered),
                        "order": len(order_paths),
                        "first_answered": answered[0] if answered else None,
                        "first_in_order": order_paths[0] if order_paths else None,
                        "last_answered": answered[-1] if answered else None,
                        "expected_last": order_paths[MOUNT_COVERAGE_LIMIT - 1]
                        if len(order_paths) >= MOUNT_COVERAGE_LIMIT
                        else None,
                    }
                ),
                answered == order_paths[:MOUNT_COVERAGE_LIMIT],
            ),
            check(
                "the paths the list drops are the oldest, not a random subset",
                "the dropped paths are exactly the ledger order beyond the bound: the retired instant cut"
                " short at "
                + str(coverage["expected_kept"][coverage["instants"]["retired_at"]])
                + " paths and the released one, with the newest instant answered whole",
                json.dumps(
                    {
                        "dropped": len(dropped),
                        "dropped_by_instant": coverage["dropped_by_instant"],
                        "expected_dropped": coverage["expected_dropped"],
                        "kept_by_instant": coverage["kept_by_instant"],
                        "expected_kept": coverage["expected_kept"],
                        "first_dropped": dropped[0] if dropped else None,
                        "last_dropped": dropped[-1] if dropped else None,
                        "newest_dropped": coverage["newest_dropped"],
                        "oldest_kept": coverage["oldest_kept"],
                        "preexisting_paths": [path for path, _ in mount["preexisting"]],
                    }
                ),
                set(dropped) == set(order_paths[MOUNT_COVERAGE_LIMIT:])
                and coverage["dropped_by_instant"] == coverage["expected_dropped"]
                and coverage["kept_by_instant"] == coverage["expected_kept"]
                and coverage["newest_dropped"] <= coverage["oldest_kept"],
            ),
            check(
                "the order is the newest observation first, not the path order",
                "the path "
                + coverage["live_path"]
                + " is answered although it sorts after every bulk path, while bulk paths of the older"
                " instants are dropped",
                json.dumps(
                    {
                        "live_entry": live_entry,
                        "bulk_dropped": [path for path in dropped if path.startswith("/bulk-")][:3],
                        "bulk_answered": [path for path in answered if path.startswith("/bulk-")][:3],
                        "instants": coverage["instants"],
                    }
                ),
                live_entry["answered"] is True
                and live_entry["used_latest_value"] == mount_used_bytes(coverage["live_path"])
                and any(path.startswith("/bulk-") for path in dropped)
                and any(path.startswith("/bulk-") for path in answered),
            ),
            check(
                "a path that stopped reporting is silent with its seconds, not unknown",
                "state silent with silentSeconds near the "
                + str(MOUNT_RETIRED_AGE_SECONDS)
                + " seconds since its newest reading and above the threshold, while the reading it was"
                " judged on is still stored and still answered by the history route",
                json.dumps({"entry": retired_entry, "history": mount["history"]}),
                retired_entry["observation_state"] == "silent"
                and isinstance(retired_entry["silent_seconds"], (int, float))
                and retired_entry["silent_seconds"] > (threshold or 0)
                and abs(retired_entry["silent_seconds"] - MOUNT_RETIRED_AGE_SECONDS) < 3600
                and retired_entry["used_latest_value"] == mount["history"]["expected_value"]
                and retired_entry["used_observation_count"] == 1
                and mount["history"]["items"] == 1
                and mount["history"]["dimension"] == coverage["retired_path"]
                and mount["history"]["points"][0][1] == mount["history"]["expected_value"],
            ),
            check(
                "a path whose reading is the newest is stated reported",
                "state reported with silentSeconds within the measured silence threshold",
                json.dumps({"entry": live_entry, "silence_threshold_seconds": threshold}),
                live_entry["observation_state"] == "reported"
                and (
                    live_entry["silent_seconds"] is None
                    or live_entry["silent_seconds"] <= (threshold or 0)
                )
                and live_entry["used_latest_value"] == mount_used_bytes(coverage["live_path"]),
            ),
            check(
                "mount rows are keyed by Agent and path, so they do not follow the Node count",
                "two series per path for the Agent with "
                + str(agent_rows["nodes"])
                + " Nodes and for the Agent with "
                + str(solo_rows["nodes"])
                + " Node, with no node_id column on host_metric_samples, so mount rows cannot multiply"
                " with Nodes",
                json.dumps(
                    {
                        "agent": agent_rows,
                        "solo": solo_rows,
                        "columns": mount["rows"]["columns"],
                        "mount_series_per_path": len(HOST_MOUNT_SERIES),
                    }
                ),
                agent_rows["mount_series"] == len(HOST_MOUNT_SERIES) * agent_rows["distinct_paths"]
                and agent_rows["mount_samples"]
                == len(HOST_MOUNT_SERIES) * (agent_rows["distinct_paths"] - 1)
                and agent_rows["nodes"] > 1
                and solo_rows["mount_series"] == len(HOST_MOUNT_SERIES) * solo_rows["distinct_paths"]
                and solo_rows["mount_samples"] == len(HOST_MOUNT_SERIES) * solo_rows["distinct_paths"]
                and solo_rows["nodes"] == 1
                and "node_id" not in mount["rows"]["columns"],
            ),
            check(
                "the mount coverage read is index backed and sorts nothing",
                "EXPLAIN QUERY PLAN seeks "
                + MOUNT_COVERAGE_INDEX
                + " under the Server's own binds, with no temp B-TREE and no scan of the ledger",
                json.dumps(mount["plan"]),
                bool(mount["plan"]["lines"])
                and any(MOUNT_COVERAGE_INDEX in line for line in mount["plan"]["lines"])
                and mount["plan"]["forbidden"] == [],
            ),
            check(
                "migration 0069's mount index is a measured object of the family",
                "dbstat reports "
                + MOUNT_COVERAGE_INDEX
                + " holding bytes after this phase planted "
                + str(coverage["order_size"])
                + " paths, at "
                + str(mount["footprint"]["bytes_per_mount_series"])
                + " bytes per mount series",
                json.dumps(mount["footprint"]),
                mount["footprint"]["mount_index_bytes"] > 0
                and mount["footprint"]["family_delta_bytes"] > 0
                and mount["footprint"]["bytes_per_mount_series"] is not None,
            ),
            check(
                "the mount list is answered uncached, and its body and latency are measured",
                str(MOUNT_COVERAGE_READS)
                + " real reads, each answering "
                + str(mount_read["mounts"])
                + " paths in a body of at most "
                + str(mount_read["payload_bytes"]["max"])
                + " bytes with Cache-Control no-store",
                json.dumps(
                    {
                        "reads": mount_read["reads"],
                        "payload_bytes": mount_read["payload_bytes"],
                        "latency_ms": mount_read["latency_ms"],
                        "cache_control": mount_read["cache_control"],
                        "answered_at": mount_read["answered_at"],
                        "bounds": mount_bounds,
                    }
                ),
                mount_read["reads"] == MOUNT_COVERAGE_READS
                and mount_read["cache_control"] == "no-store"
                and mount_read["payload_bytes"]["max"] > 0
                and mount_read["latency_ms"]["p50"] > 0,
            ),
        ])
        # -- the recorded sync/consensus state and chain height surface (issue #217)
        st_probe = state["instrument"]["probe_node_id"]
        st_ledger_node = state["instrument"]["ledger_node_id"]
        st_expect = state["expect"]
        def st_payload(answer: dict) -> dict:
            """read_surface answers wrap the parsed body; these checks compare the
            body itself, the way the phase's own paging walk does."""
            st_body = answer.get("payload")
            return st_body if isinstance(st_body, dict) else answer

        st_sync = st_payload(state["reads"]["sync"])
        st_consensus = st_payload(state["reads"]["consensus"])
        st_ledger_answer = st_payload(state["reads"]["ledger node sync"])
        st_refusals = state["reads"]["refusals"]
        st_page_one = st_payload(state["reads"]["page one"])
        st_page_two_answer = state["reads"]["page two"]
        st_page_two = None if st_page_two_answer is None else st_payload(st_page_two_answer)
        st_heights = state["reads"]["heights"]
        st_sql = state["sql"]
        st_plans = state["plans"]
        st_rows_after = state["rows_after"]
        st_deliveries = state["deliveries"]
        st_frozen = state["frozen"]

        def st_entries(answer: dict) -> list:
            return [item.get("observedAt") for item in answer.get("entries") or []]

        def st_last(values: list):
            """A missing answer must fail a check, never crash the instrument."""
            return values[-1] if values else None

        def st_kind(answer: dict, kind: str) -> list:
            return [
                item.get("observedAt")
                for item in answer.get("entries") or []
                if item.get("entryKind") == kind
            ]

        def st_gaps(answer: dict) -> list:
            return [
                {
                    "from": item.get("from"),
                    "to": item.get("to"),
                    "kind": item.get("kind"),
                    "skipped": item.get("skippedCount"),
                }
                for item in answer.get("gaps") or []
            ]

        def st_series(component: str) -> dict:
            return (st_sync if component == "sync" else st_consensus).get("series") or {}

        def st_accepted() -> int:
            return len(
                [
                    entry
                    for entry in st_deliveries
                    if entry["disposition"] in ("accepted", "partially_accepted")
                ]
            )

        st_spacings = {}
        for st_component in STATE_COMPONENTS:
            st_previous = None
            st_walk = []
            for st_row in st_sql[st_component + " entries"]:
                if st_row["entry_kind"] == "anchor":
                    st_walk.append(
                        None
                        if st_previous is None
                        else int(
                            (
                                parse_instant(st_row["observed_at"]) - parse_instant(st_previous)
                            ).total_seconds()
                        )
                    )
                st_previous = st_row["observed_at"]
            st_spacings[st_component] = st_walk

        st_height_ok = {}
        for st_label, st_node in (("probe", st_probe), ("ledger", st_ledger_node)):
            st_matched = 0
            st_ascending = 0
            for st_metric, st_series_expect in st_expect["heights"].items():
                st_reading = st_heights[st_label][st_metric]
                st_items = sorted(
                    st_reading["items"], key=lambda item: item["observedAt"] or ""
                )
                st_observed = [float(item["value"]) for item in st_items]
                st_wanted = [float(value) for value in st_series_expect[st_label]]
                if st_reading["status"] == 200 and st_observed == st_wanted:
                    st_matched += 1
                if [item["observedAt"] for item in st_items] == [
                    item["observedAt"] for item in st_reading["items"]
                ]:
                    st_ascending += 1
            st_height_ok[st_label] = {"matched": st_matched, "ascending": st_ascending}

        st_height_gaps = 0
        for st_metric in STATE_NODE_SERIES:
            if st_gaps(st_heights["probe"][st_metric]) == st_expect["height_gaps"]:
                st_height_gaps += 1
        if st_gaps(st_heights["ledger"]["sync_current_block"]) == st_expect["height_gaps"]:
            st_height_gaps += 1

        st_read_plans = {
            "read": st_plans["read"],
            "paged": st_plans["paged"],
            "ledger": st_plans["ledger"],
            "heights": st_plans["heights"],
        }

        def st_seeks(plan: list, index: str) -> bool:
            return any(index in detail and "SEARCH" in detail for detail in plan)

        def st_forbidden(plan: list, tokens: list) -> list:
            return [detail for detail in plan if any(token in detail for token in tokens)]

        st_pages = [st_page_one]
        if st_page_two is not None:
            st_pages.append(st_page_two)
        st_walked = []
        for st_page in reversed(st_pages):
            st_walked.extend(st_entries(st_page))

        checks.extend([
            check(
                "every state Report was accepted, and the failed probes with it",
                str(len(st_deliveries))
                + " Reports accepted ("
                + str(len([entry for entry in st_deliveries if entry["failed"]]))
                + " of them failed chain probes)",
                json.dumps(
                    {
                        "reports": state["instrument"]["reports"],
                        "accepted": st_accepted(),
                        "failed": len([entry for entry in st_deliveries if entry["failed"]]),
                        "paused": len([entry for entry in st_deliveries if entry["kind"] != "state"]),
                        "rejections": [entry["reason"] for entry in st_deliveries if entry["disposition"] not in ("accepted", "partially_accepted")],
                    }
                ),
                st_accepted() == len(st_deliveries)
                and len([entry for entry in st_deliveries if entry["failed"]]) == 2,
            ),
            check(
                "the state surface is declared with its window, its anchor rule and its bound",
                "components sync and consensus, "
                + str(STATE_RETENTION_DAYS)
                + " day retention, "
                + str(STATE_ANCHOR_SECONDS)
                + " s anchors, a "
                + str(STATE_CLEANUP_BATCH)
                + " row cleanup batch that covers one maximal state Report of "
                + str(MAX_STATE_ROWS_PER_REPORT)
                + " rows",
                json.dumps(
                    {
                        "components": sorted(list(st_series("sync").get("components") or []) + list(st_series("consensus").get("components") or []))
                        or state["instrument"]["components"],
                        "retentionDays": st_sync.get("retentionDays"),
                        "anchorSeconds": st_sync.get("anchorSeconds"),
                        "windowSeconds": st_sync.get("windowSeconds"),
                        "series_windowSeconds": st_series("sync").get("windowSeconds"),
                        "requested_window_seconds": st_expect["read_window"]["seconds"],
                        "cleanup_batch": state["instrument"]["cleanup_batch"],
                        "max_state_rows_per_report": state["instrument"]["max_state_rows_per_report"],
                        "node_series": state["instrument"]["node_series"],
                    }
                ),
                state["instrument"]["components"] == list(STATE_COMPONENTS)
                and st_sync.get("retentionDays") == STATE_RETENTION_DAYS
                and st_sync.get("anchorSeconds") == STATE_ANCHOR_SECONDS
                # The answered window is the requested range, not the anchor
                # window: anchorSeconds declares the 3600 s silence rule, while
                # windowSeconds is the span of what was asked for, and the series
                # states the same span.
                and st_series("sync").get("windowSeconds") == st_sync.get("windowSeconds")
                and abs(
                    (st_sync.get("windowSeconds") if isinstance(st_sync.get("windowSeconds"), int) else -10**9)
                    - st_expect["read_window"]["seconds"]
                )
                <= st_expect["read_window"]["tolerance_seconds"]
                and state["instrument"]["cleanup_batch"] >= state["instrument"]["max_state_rows_per_report"],
            ),
            check(
                "the two Nodes state the five chain heights as their own series",
                "5 stored height series per Node, each answering "
                + str(len(st_expect["height_units"]))
                + " own values",
                json.dumps(
                    {
                        "stored_series": sorted((st_rows_after["probe"]["heights"] or {}).keys()),
                        "probe": st_height_ok["probe"],
                        "ledger": st_height_ok["ledger"],
                        "expected_series": sorted(STATE_NODE_SERIES),
                    }
                ),
                sorted((st_rows_after["probe"]["heights"] or {}).keys()) == sorted(STATE_NODE_SERIES)
                and st_height_ok["probe"]["matched"] == len(STATE_NODE_SERIES)
                and st_height_ok["ledger"]["matched"] == len(STATE_NODE_SERIES)
                and st_height_ok["probe"]["ascending"] == len(STATE_NODE_SERIES),
            ),
            check(
                "a chain height is stored once per passing Report and read back by height",
                "each of the five series holds "
                + str(len(st_expect["height_units"]))
                + " samples for the probe Node",
                json.dumps(
                    {
                        st_metric: (st_rows_after["probe"]["heights"] or {}).get(st_metric)
                        for st_metric in STATE_NODE_SERIES
                    }
                ),
                all(
                    ((st_rows_after["probe"]["heights"] or {}).get(st_metric) or {}).get("rows") == len(st_expect["height_units"])
                    for st_metric in STATE_NODE_SERIES
                ),
            ),
            check(
                "a failed chain probe stores no height, and a paused delivery none either",
                "0 height rows at the failed instants and the paused instants",
                json.dumps(
                    {
                        "heights_at_failed_and_paused": st_sql["heights at the failed and paused instants"],
                        "expected": 0,
                    }
                ),
                st_sql["heights at the failed and paused instants"] == 0,
            ),
            check(
                "a failed chain probe is still a counted delivery",
                "the two failed Reports are counted in the ledger while only one entry records the transition",
                json.dumps(
                    {
                        "failed_deliveries": len([entry for entry in st_deliveries if entry["failed"]]),
                        "counted_before_the_pause": (st_frozen["probe"]["ledger"] or {}).get("sync", {}).get("entry_count"),
                        "frozen_count": st_expect["frozen_count"],
                    }
                ),
                len([entry for entry in st_deliveries if entry["failed"]]) == 2
                and (st_frozen["probe"]["ledger"] or {}).get("sync", {}).get("entry_count") == st_expect["frozen_count"],
            ),
            check(
                "one change entry per state transition, and nothing at all while no Report arrives",
                "sync changes at "
                + json.dumps(st_expect["sync_changes"])
                + " and no entry between "
                + st_expect["sync_anchors"][1]
                + " and "
                + st_expect["sync_anchors"][2]
                + " where eleven Reports were skipped",
                json.dumps(
                    {
                        "changes": st_kind(st_sync, "change"),
                        "silent_stretch_entries": [
                            instant_value
                            for instant_value in st_entries(st_sync)
                            if st_expect["sync_anchors"][1] < instant_value < st_expect["sync_anchors"][2]
                        ],
                        "consensus_changes": st_kind(st_consensus, "change"),
                    }
                ),
                st_kind(st_sync, "change") == st_expect["sync_changes"]
                and st_kind(st_consensus, "change") == st_expect["consensus_changes"]
                and [
                    instant_value
                    for instant_value in st_entries(st_sync)
                    if st_expect["sync_anchors"][1] < instant_value < st_expect["sync_anchors"][2]
                ]
                == [],
            ),
            check(
                "the heights moving is not a state change: the unchanged vector anchors instead",
                "sync anchors at "
                + json.dumps(st_expect["sync_anchors"])
                + " and consensus anchors at "
                + json.dumps(st_expect["consensus_anchors"]),
                json.dumps(
                    {
                        "sync_anchors": st_kind(st_sync, "anchor"),
                        "consensus_anchors": st_kind(st_consensus, "anchor"),
                        "sync_entries": st_entries(st_sync),
                        "consensus_entries": st_entries(st_consensus),
                    }
                ),
                st_kind(st_sync, "anchor") == st_expect["sync_anchors"]
                and st_kind(st_consensus, "anchor") == st_expect["consensus_anchors"]
                and st_entries(st_sync) == st_expect["sync_entries"]
                and st_entries(st_consensus) == st_expect["consensus_entries"],
            ),
            check(
                "an anchor is written only after an hour of unchanged silence",
                "every anchor at least "
                + str(STATE_ANCHOR_SECONDS)
                + " s after the entry before it, for both components",
                json.dumps(st_spacings),
                all(
                    spacing is not None and spacing >= STATE_ANCHOR_SECONDS
                    for walk in st_spacings.values()
                    for spacing in walk
                )
                and st_spacings == {"sync": [3600, 3600, 7200], "consensus": [3600, 3600, 8400]},
            ),
            check(
                "the per-component ledger counts the deliveries, the changes and the anchors",
                "sync entry_count "
                + str(st_expect["entry_count"])
                + " / change_count 5 / anchor_count 3, consensus entry_count "
                + str(st_expect["entry_count"])
                + " / change_count 3 / anchor_count 3, no replay and no correction",
                json.dumps(
                    {
                        "sync": (st_rows_after["probe"]["ledger"] or {}).get("sync"),
                        "consensus": (st_rows_after["probe"]["ledger"] or {}).get("consensus"),
                    }
                ),
                all(
                    (st_rows_after["probe"]["ledger"] or {}).get(st_component, {}).get("entry_count") == st_expect["entry_count"]
                    and (st_rows_after["probe"]["ledger"] or {}).get(st_component, {}).get("change_count")
                    == len(st_expect[st_component + "_changes"])
                    and (st_rows_after["probe"]["ledger"] or {}).get(st_component, {}).get("anchor_count")
                    == len(st_expect[st_component + "_anchors"])
                    and (st_rows_after["probe"]["ledger"] or {}).get(st_component, {}).get("replayed_count") == 0
                    and (st_rows_after["probe"]["ledger"] or {}).get(st_component, {}).get("corrected_count") == 0
                    and (st_rows_after["probe"]["ledger"] or {}).get(st_component, {}).get("last_entry_at")
                    == st_expect[st_component + "_entries"][-1]
                    for st_component in STATE_COMPONENTS
                ),
            ),
            check(
                "the read surface answers the entries oldest first, with their ledger and their window",
                "sync answers its "
                + str(len(st_expect["sync_entries"]))
                + " entries oldest first with entry_count "
                + str(st_expect["entry_count"])
                + " and cadence "
                + str(st_expect["sync_cadence_seconds"])
                + " s",
                json.dumps(
                    {
                        "entries": st_entries(st_sync),
                        "entry_count": st_series("sync").get("entryCount"),
                        "change_count": st_series("sync").get("changeCount"),
                        "anchor_count": st_series("sync").get("anchorCount"),
                        "cadence_seconds": st_sync.get("cadenceSeconds"),
                        "window_seconds": st_sync.get("windowSeconds"),
                        "anchor_seconds": st_sync.get("anchorSeconds"),
                        "node_id": st_sync.get("nodeId"),
                        "component": st_sync.get("component"),
                        "coverage_seconds": st_sync.get("coverageSeconds"),
                        "consensus_cadence_seconds": st_consensus.get("cadenceSeconds"),
                    }
                ),
                st_entries(st_sync) == st_expect["sync_entries"]
                and st_entries(st_consensus) == st_expect["consensus_entries"]
                and st_series("sync").get("entryCount") == st_expect["entry_count"]
                and st_series("sync").get("changeCount") == len(st_expect["sync_changes"])
                and st_series("sync").get("anchorCount") == len(st_expect["sync_anchors"])
                and st_sync.get("cadenceSeconds") == st_expect["sync_cadence_seconds"]
                and st_consensus.get("cadenceSeconds") == st_expect["consensus_cadence_seconds"]
                and st_sync.get("nodeId") == st_probe
                and st_sync.get("component") == "sync"
                and st_sync.get("from") == st_expect["read_window"]["from"]
                and st_sync.get("to") == st_expect["read_window"]["to"],
            ),
            check(
                "the read surface accounts for its coverage and its gaps",
                "sync coverage "
                + str(st_expect["sync_coverage_seconds"])
                + " s with two gaps, consensus coverage "
                + str(st_expect["consensus_coverage_seconds"])
                + " s with two gaps",
                json.dumps(
                    {
                        "sync": {"coverage": st_sync.get("coverageSeconds"), "gaps": st_gaps(st_sync)},
                        "consensus": {
                            "coverage": st_consensus.get("coverageSeconds"),
                            "gaps": st_gaps(st_consensus),
                        },
                    }
                ),
                st_sync.get("coverageSeconds") == st_expect["sync_coverage_seconds"]
                and st_consensus.get("coverageSeconds") == st_expect["consensus_coverage_seconds"]
                and st_gaps(st_sync) == st_expect["sync_gaps"]
                and st_gaps(st_consensus) == st_expect["consensus_gaps"],
            ),
            check(
                "a stretch with no recorded entry is a collection gap, and it carries no loss count",
                "neither surface claims a skipped count for the silent stretch",
                json.dumps(
                    {
                        "sync": [gap for gap in st_gaps(st_sync) if gap["kind"] == "collection_gap"],
                        "consensus": [gap for gap in st_gaps(st_consensus) if gap["kind"] == "collection_gap"],
                    }
                ),
                all(
                    gap["skipped"] is None
                    for answer in (st_sync, st_consensus)
                    for gap in st_gaps(answer)
                    if gap["kind"] == "collection_gap"
                ),
            ),
            check(
                "the two Nodes read their own heights, and the height series reports the pause",
                "5 series on the probe Node answer the expected gaps, and the ledger Node's sync_current_block does too",
                json.dumps(
                    {
                        "matching_probe_series": [
                            st_metric
                            for st_metric in STATE_NODE_SERIES
                            if st_gaps(st_heights["probe"][st_metric]) == st_expect["height_gaps"]
                        ],
                        "sync_current_block_height_coverage": st_heights["probe"]["sync_current_block"].get("coverage_seconds"),
                        "sync_current_block_sampled": st_heights["probe"]["sync_current_block"].get("sampled_count"),
                        "sync_current_block_items": len(st_heights["probe"]["sync_current_block"].get("items") or []),
                        "sync_current_block_cadence": st_heights["probe"]["sync_current_block"].get("cadence_seconds"),
                        "measured_height_rhythm_seconds": st_expect["height_rhythm_seconds"],
                        "ledger_node_sync_current_block": st_gaps(st_heights["ledger"]["sync_current_block"]),
                        "ledger_node_scope_key": st_heights["ledger"]["sync_current_block"].get("scope_key"),
                    }
                ),
                st_height_gaps == len(STATE_NODE_SERIES) + 1
                and st_heights["probe"]["sync_current_block"].get("coverage_seconds") == st_expect["height_coverage_seconds"]
                and len(st_heights["probe"]["sync_current_block"].get("items") or []) == len(st_expect["height_units"])
                # The metric route states no cadence for a Node series
                # (AdminMetricHistoryResponse has no cadenceSeconds field), so the
                # answer must leave it unstated rather than invent one.
                and st_heights["probe"]["sync_current_block"].get("cadence_seconds") is None
                and st_heights["ledger"]["sync_current_block"].get("scope_key") == st_ledger_node,
            ),
            check(
                "the same low-space pause appears on both surfaces, carrying its loss count",
                "the sync state read and the sync height series both report one protection pause over "
                + st_expect["pause"]["from"]
                + " to "
                + st_expect["pause"]["to"]
                + " skipping "
                + str(st_expect["pause"]["reports"])
                + " deliveries",
                json.dumps(
                    {
                        "state_pauses": [gap for gap in st_gaps(st_sync) if gap["kind"] == "protection_pause"],
                        "height_pauses": [
                            gap
                            for gap in st_gaps(st_heights["probe"]["sync_current_block"])
                            if gap["kind"] == "protection_pause"
                        ],
                    }
                ),
                [gap for gap in st_gaps(st_sync) if gap["kind"] == "protection_pause"]
                == [gap for gap in st_expect["sync_gaps"] if gap["kind"] == "protection_pause"]
                and [
                    gap
                    for gap in st_gaps(st_heights["probe"]["sync_current_block"])
                    if gap["kind"] == "protection_pause"
                ] == [gap for gap in st_expect["height_gaps"] if gap["kind"] == "protection_pause"],
            ),
            check(
                "a paused Report records no state entry and does not move the ledger",
                "0 state rows inside the paused stretch, and both components still led by "
                + st_expect["frozen"]["last_entry_at"],
                json.dumps(
                    {
                        "entries_in_the_pause": st_sql["entries in the pause"],
                        "frozen": {
                            st_component: (st_frozen["probe"]["ledger"] or {}).get(st_component)
                            for st_component in STATE_COMPONENTS
                        },
                    }
                ),
                st_sql["entries in the pause"] == 0
                and all(
                    (st_frozen["probe"]["ledger"] or {}).get(st_component, {}).get("entry_count") == st_expect["frozen"]["entry_count"]
                    and (st_frozen["probe"]["ledger"] or {}).get(st_component, {}).get("last_entry_at") == st_expect["frozen"]["last_entry_at"]
                    and (st_frozen["probe"]["ledger"] or {}).get(st_component, {}).get("last_observed_at") == st_expect["frozen"]["last_observed_at"]
                    for st_component in STATE_COMPONENTS
                ),
            ),
            check(
                "the paused deliveries are counted as skipped series, keyed by component and by height",
                str(len(st_expect["skipped_series"]))
                + " series each carrying a skipped count of "
                + str(st_expect["pause"]["reports"])
                + " for the probe Node",
                json.dumps(st_sql["skipped series"]),
                sorted([row["metric"] for row in st_sql["skipped series"]]) == st_expect["skipped_series"]
                and all(
                    row["skipped_count"] == st_expect["pause"]["reports"]
                    and row["dimension"] == ""
                    and row["first_skipped_at"] == st_expect["pause"]["from"]
                    and row["last_skipped_at"] == st_expect["pause"]["to"]
                    for row in st_sql["skipped series"]
                ),
            ),
            check(
                "a resumed Report records a state again",
                "the recovery delivery at "
                + st_expect["sync_entries"][-1]
                + " is a change row for both components",
                json.dumps(
                    {
                        "sync_last_entry": st_last(st_entries(st_sync)),
                        "consensus_last_entry": st_last(st_entries(st_consensus)),
                        "recovery": state["recovery"],
                    }
                ),
                st_last(st_entries(st_sync)) == st_expect["sync_changes"][-1]
                and st_last(st_entries(st_consensus)) == st_expect["consensus_changes"][-1]
                and state["recovery"]["disposition"] in ("accepted", "partially_accepted"),
            ),
            check(
                "the read window is what was asked for, and an out-of-window cursor is refused",
                "availability is unclaimed inside the retained window, the page cursor must satisfy from < before <= to",
                json.dumps(
                    {
                        "in_window_availability": st_sync.get("availability"),
                        "straddling_availability": st_refusals["horizon straddled"]["payload"].get("availability"),
                        "released_availability": st_refusals["horizon released"]["payload"].get("availability"),
                        "before_at_from": {
                            "status": st_refusals["before at from"]["status"],
                            "code": (st_refusals["before at from"]["payload"].get("error") or {}).get("code"),
                        },
                        "before_past_to": {
                            "status": st_refusals["before past to"]["status"],
                            "code": (st_refusals["before past to"]["payload"].get("error") or {}).get("code"),
                        },
                    }
                ),
                st_sync.get("availability") is None
                and st_refusals["horizon straddled"]["payload"].get("availability") == "partial"
                and st_refusals["horizon released"]["payload"].get("availability") == "unavailable"
                and st_refusals["before at from"]["status"] == 400
                and (st_refusals["before at from"]["payload"].get("error") or {}).get("code") == "invalid_history_range"
                and st_refusals["before past to"]["status"] == 400
                and (st_refusals["before past to"]["payload"].get("error") or {}).get("code") == "invalid_history_range",
            ),
            check(
                "an unknown component and an unknown Node are refused",
                "400 invalid_component and 404",
                json.dumps(
                    {
                        "unknown_component": {
                            "status": st_refusals["unknown component"]["status"],
                            "code": (st_refusals["unknown component"]["payload"].get("error") or {}).get("code"),
                        },
                        "unknown_node": {"status": st_refusals["unknown node"]["status"]},
                        "clamped_limit": {
                            "status": st_refusals["clamped limit"]["status"],
                            "entries": len(st_entries(st_payload(st_refusals["clamped limit"]))),
                        },
                    }
                ),
                st_refusals["unknown component"]["status"] == 400
                and (st_refusals["unknown component"]["payload"].get("error") or {}).get("code") == "invalid_component"
                and st_refusals["unknown node"]["status"] == 404
                and st_refusals["clamped limit"]["status"] == 200
                and len(st_entries(st_payload(st_refusals["clamped limit"]))) == len(st_expect["sync_entries"]),
            ),
            check(
                "the before cursor pages older without a hole",
                "page one answers the newest "
                + str(STATE_PAGE_LIMIT)
                + " entries and hands back the oldest of them as the cursor; the older page ends the walk",
                json.dumps(
                    {
                        "page_one": {
                            "entries": st_entries(st_page_one),
                            "truncated": st_page_one.get("truncated"),
                            "continuation": st_page_one.get("continuation"),
                        },
                        "page_two": None
                        if st_page_two is None
                        else {
                            "entries": st_entries(st_page_two),
                            "truncated": st_page_two.get("truncated"),
                            "continuation": st_page_two.get("continuation"),
                        },
                        "walked": st_walked,
                    }
                ),
                st_page_two is not None
                and len(st_entries(st_page_one)) == STATE_PAGE_LIMIT
                and st_page_one.get("truncated") is True
                and st_page_one.get("continuation") == st_expect["sync_entries"][-STATE_PAGE_LIMIT]
                and st_page_two.get("truncated") is False
                and st_page_two.get("continuation") is None
                and st_entries(st_page_two) == st_expect["sync_entries"][: -STATE_PAGE_LIMIT]
                and st_walked == st_expect["sync_entries"],
            ),
            check(
                "the recorded state is read with a bounded query, not with a scan",
                "SEARCH "
                + STATE_READ_INDEX
                + " on the read and the paged plan, SEARCH "
                + STATE_LEDGER_INDEX
                + " on the ledger, and no "
                + json.dumps(list(STATE_READ_FORBIDDEN_PLANS))
                + " anywhere",
                json.dumps(st_read_plans),
                st_seeks(st_plans["read"], STATE_READ_INDEX)
                and st_seeks(st_plans["paged"], STATE_READ_INDEX)
                and st_seeks(st_plans["ledger"], STATE_LEDGER_INDEX)
                and not st_forbidden(st_plans["read"], list(STATE_READ_FORBIDDEN_PLANS))
                and not st_forbidden(st_plans["paged"], list(STATE_READ_FORBIDDEN_PLANS))
                and not st_forbidden(st_plans["ledger"], list(STATE_READ_FORBIDDEN_PLANS)),
            ),
            check(
                "a chain height is read back by the metric engine's own bounded statement",
                "SEARCH node_metric_samples with no "
                + json.dumps(list(STATE_HEIGHT_FORBIDDEN_PLANS)),
                json.dumps(st_plans["heights"]),
                st_seeks(st_plans["heights"], "node_metric_samples")
                and not st_forbidden(st_plans["heights"], list(STATE_HEIGHT_FORBIDDEN_PLANS)),
            ),
            check(
                "the state read surface answers under measured latency",
                str(STATE_READS) + " real reads with a positive p50",
                json.dumps({"latency_ms": state["reads"]["latency_ms"]}),
                len(state["reads"]["latency_ms"]) == STATE_READS
                and percentile(state["reads"]["latency_ms"], 0.5) > 0,
            ),
            check(
                "a purge without the mutation guard and a purge with a mismatched confirmation both change nothing",
                "401 auth_required without a session, 403 csrf_validation_failed with a session but no CSRF token, 400 confirmation_mismatch for a mismatched confirmation, with the rows still there afterwards",
                json.dumps(
                    {
                        "anonymous": state["purge"]["anonymous"],
                        "unguarded": state["purge"]["unguarded"],
                        "mismatched": state["purge"]["mismatched"],
                        "still_there": {
                            "status": state["reads"]["before purge"]["status"],
                            # read_surface wraps the parsed body, so the body has to
                            # be unwrapped before its entries are read.
                            "entries": len(st_entries(st_payload(state["reads"]["before purge"]))),
                        },
                    }
                ),
                # A mutation with no session is refused by the auth middleware
                # before the mutation guard is consulted, and a session without the
                # token is refused by the guard itself
                # (crates/platpulse-server/src/http/admin.rs:59-75).
                state["purge"]["anonymous"]["status"] == 401
                and state["purge"]["anonymous"]["code"] == "auth_required"
                and state["purge"]["anonymous"]["removed"] is None
                and state["purge"]["unguarded"]["status"] == 403
                and state["purge"]["unguarded"]["code"] == "csrf_validation_failed"
                and state["purge"]["unguarded"]["removed"] is None
                and state["purge"]["mismatched"]["status"] == 400
                and state["purge"]["mismatched"]["code"] == "confirmation_mismatch"
                and state["purge"]["mismatched"]["removed"] is None
                and state["reads"]["before purge"]["status"] == 200
                and len(st_entries(st_payload(state["reads"]["before purge"]))) == len(st_expect["sync_entries"]),
            ),
            check(
                "a purge removes the Node's recorded state and its ledger rows",
                "the answer counts "
                + str(st_expect["purge"]["state_observations"])
                + " state entries and "
                + str(st_expect["purge"]["state_series_state"])
                + " ledger rows removed",
                json.dumps(
                    {
                        "status": state["purge"]["purged"]["status"],
                        "removed": state["purge"]["purged"]["removed"],
                    }
                ),
                state["purge"]["purged"]["status"] == 200
                and isinstance(state["purge"]["purged"]["removed"], dict)
                and state["purge"]["purged"]["removed"].get("state_observations") == st_expect["purge"]["state_observations"]
                and state["purge"]["purged"]["removed"].get("state_series_state") == st_expect["purge"]["state_series_state"],
            ),
            check(
                "the purged Node is gone from the surface and from the tables, and the other Node is untouched",
                "the purged Node answers 404 with 0 rows, the probe Node still answers "
                + str(len(st_expect["sync_entries"]))
                + " entries and keeps its "
                + str(st_expect["purge"]["state_observations"])
                + " rows and its "
                + str(st_expect["purge"]["state_series_state"])
                + " ledger rows",
                json.dumps(
                    {
                        "ledger_read": state["reads"]["after purge"]["ledger"]["status"],
                        "probe_read": {
                            "status": state["reads"]["after purge"]["probe"]["status"],
                            "entries": len(st_entries(st_payload(state["reads"]["after purge"]["probe"]))),
                        },
                        "ledger_rows": st_rows_after["ledger"],
                        "probe_rows": {
                            "observations": st_rows_after["probe"]["observations"],
                            "series_rows": st_rows_after["probe"]["series_rows"],
                            "by_component": st_rows_after["probe"]["by_component"],
                        },
                    }
                ),
                state["reads"]["after purge"]["ledger"]["status"] == 404
                and st_rows_after["ledger"]["observations"] == 0
                and st_rows_after["ledger"]["series_rows"] == 0
                and state["reads"]["after purge"]["probe"]["status"] == 200
                and len(st_entries(st_payload(state["reads"]["after purge"]["probe"]))) == len(st_expect["sync_entries"])
                and st_rows_after["probe"]["observations"] == st_expect["purge"]["state_observations"]
                and st_rows_after["probe"]["series_rows"] == st_expect["purge"]["state_series_state"],
            ),
            check(
                "the state phase measured its own sampling conditions",
                "one Agent, two Nodes, "
                + str(len(st_deliveries))
                + " Reports at a "
                + str(STATE_RHYTHM_SECONDS)
                + " s rhythm on a "
                + str(state["instrument"]["rhythm_seconds"])
                + " s cadence",
                json.dumps(
                    {
                        "instrument": state["instrument"],
                        "storage": state["storage"],
                        "wall_seconds": state["wall_seconds"],
                        "global_state_rows": st_sql["state rows"],
                        "global_state_series": st_sql["state series"],
                    }
                ),
                state["instrument"]["agents"] == 1
                and state["instrument"]["nodes"] == 2
                and state["instrument"]["reports"] == len(st_deliveries)
                and state["instrument"]["database_bytes"] > 0
                and st_sql["state rows"] >= st_expect["purge"]["state_observations"]
                and st_sql["state series"] >= st_expect["purge"]["state_series_state"],
            ),
        ])
        return checks

    # -- orchestration -----------------------------------------------------

    def run(self) -> dict:
        self.run_dir.mkdir(parents=True, exist_ok=True)
        self.state_dir.mkdir(parents=True, exist_ok=True)
        conditions = {
            "generated_at": utc_now(),
            "binary": str(self.binary),
            "binary_bytes": self.binary.stat().st_size if self.binary.is_file() else None,
            "binary_built_at": datetime.fromtimestamp(self.binary.stat().st_mtime, timezone.utc).strftime(CANONICAL)
            if self.binary.is_file()
            else None,
            "hardware": hardware_conditions(),
            "mount": mount_conditions(self.state_dir),
            "seed": self.args.seed,
            "declared_cadence_seconds": self.cadence,
            "declared_window_hours": self.args.hours,
            "rounds": self.rounds,
            "declared_window_seconds": self.window_seconds,
            "planned_window_seconds": max(0, (self.rounds - 1) * self.cadence),
            "cliff_guard_seconds": self.cliff_guard_seconds,
            "server_mode": "development",
            "port": self.port,
        }
        self.build_agent()
        load = self.phase_load()
        restatements = self.phase_restatements()
        release = self.phase_release()
        reads = self.phase_reads()
        # The multi-Node phase runs after every read phase, because the Nodes and
        # rows it adds would otherwise land inside the one-Node counts the earlier
        # checks assert exactly. It runs before the storage phase so that the
        # database counts include the rows it added.
        multi_node = self.phase_multi_node()
        storage = self.phase_storage()
        # The aggregate tier phase (issue #214) runs last: it seeds a backdated
        # month, so it needs the earlier one-Node counts already read, and it takes
        # a page-level storage baseline by stopping the Server, which phase_storage
        # has already done.
        tiers = self.phase_tiers()
        # The Host family phase (issue #215) runs last: it audits the family every
        # Report above already wrote, and the one Report it plants must stay
        # outside the exact counts those earlier phases assert.
        host = self.phase_host(release)
        # The mount coverage phase (issue #216) runs last: it plants Reports of
        # disjoint mount paths and reads the mount list, so it runs after the Host
        # family audit above and keeps its own rows outside the exact counts every
        # earlier phase asserts.
        mount = self.phase_mount_coverage()
        # The recorded state phase (issue #217) runs last: it enrolls its own Agent
        # with two cloned Nodes, plants a low-space pause of its own and purges one
        # of those Nodes, so every count it asserts is scoped to the Nodes it
        # created and no earlier exact count can see its rows.
        state = self.phase_state_history()
        checks = self.evaluate(
            load, restatements, release, reads, multi_node, tiers, storage, host, mount, state
        )
        return {
            "issue": 213,
            "title": "Story 47 baseline: a measured 24 hour raw Node metric history",
            "conditions": conditions,
            "phases": {
                "load": load,
                "restatements": restatements,
                "release": release,
                "reads": reads,
                "multi_node": multi_node,
                "tiers": tiers,
                "host": host,
                "mounts": mount,
                "state": state,
                "storage": storage,
            },
            "checks": checks,
            "not_delivered": [
                "The 24 hour window was compressed in wall time: every observation instant is real, but the Reports"
                " were submitted as fast as the Server accepted them instead of one per declared cadence.",
                "Agent-side collection was not measured: Reports came from the fixture through the real ingestion"
                " path, not from a running platpulse-agent process.",
                "One Agent, one Node and one mount were measured on the load path; the mount family is"
                " measured separately by the issue #216 phase, which plants two Agents and hundreds of mount"
                " paths, but always with a single device behind each path: no multi-disk or network filesystem"
                " deployment was produced.",
                "The fixture reports only process_cpu_percent and process_memory_percent for the one measured"
                " Node: data_directory_percent, peer_inbound_count and peer_outbound_count are carried by the"
                " multi-Node clones below but their one-Node path has integration-test coverage only, not a"
                " measured history here.",
                "The one-Node shape cannot show per-Report drain behaviour under a multi-Node arrival rate;"
                " the multi-Node phase below measures it instead (rows added per Report, rows released by one"
                " cleanup, and the two pure replays that must release nothing).",
                "A declared cadence of 1s, 2s or 3s over the full day exceeds the bounded read limit"
                " (DEFAULT_SAMPLE_LIMIT 5000 per request, MAX_SAMPLE_LIMIT 20000), so such a window is answered"
                " truncated to the newest samples rather than in full.",
                "The Server ran in development mode without TLS and without a reverse proxy, and the build is a"
                " debug build.",
                "The Host family (issue #215) was measured with one Agent: that its series are shared by every"
                " Node of that Agent, and not split per Node, is measured here, but the separation between two"
                " Agents is measured by scripts/capacity-baseline.py and by"
                " crates/platpulse-server/tests/host_metric_history.rs:1115 instead.",
                "No low-space pause was produced by the load phases above: the pause this report measures is"
                " the issue #217 phase's own, and it covers the recorded state surface and the sync height"
                " series on both sides of it. The per-mount accounting of a paused series (two mounts, two"
                " counted losses) is still registered by scripts/capacity-baseline.py and by"
                " crates/platpulse-server/tests/host_metric_history.rs:1295 rather than measured here.",
                "The recorded state phase compresses its own clock: every state instant it plants is real, but"
                " the eleven silent Reports and the"
                " " + str(STATE_ANCHOR_SECONDS) + "s anchor rule were produced without waiting that long in"
                " wall time, and the three paused Reports were submitted back to back inside one protection"
                " interval.",
                "The recorded state surface was measured on one Agent with two cloned Nodes: that the ledger is"
                " per Node is measured here, while the separation across Agents is registered by"
                " scripts/capacity-baseline.py.",
                "The load fixture states no mount, so a maximal Host Report of 264 rows is registered as a"
                " bound (MAX_HOST_ROWS_PER_REPORT) rather than produced: the largest Host Report the load path"
                " submitted carried two mounts. The mount family itself is measured by the issue #216 phase,"
                " whose largest Report carried "
                + str(MOUNT_BULK_PATHS_PER_REPORT + 1)
                + " mounts of the declared "
                + str(MAX_HOST_MOUNTS)
                + " mount bound.",
                "These items must be appended to this same report by a follow-up ticket; nothing here is a"
                " production guarantee.",
            ],
        }


def write_json_report(report: dict, path: Path) -> None:
    path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def write_markdown_report(report: dict, path: Path) -> None:
    conditions = report["conditions"]
    load = report["phases"]["load"]
    reads = report["phases"]["reads"]
    storage = report["phases"]["storage"]
    host = report["phases"]["host"]
    mount = report["phases"]["mounts"]
    state = report["phases"]["state"]
    lines = []
    lines.append("# Issue #213 Story 47 baseline — raw 24 hour Node metric history")
    lines.append("")
    lines.append("Generated " + conditions["generated_at"] + " from " + conditions["binary"] + ".")
    lines.append("")
    lines.append("## Declared conditions")
    lines.append("")
    lines.append("- Hardware: " + str(conditions["hardware"]["cpu_count"]) + " CPUs, " + human_bytes(conditions["hardware"]["memory_total_bytes"]) + " memory, " + str(conditions["hardware"]["cpu_model"]))
    lines.append("- Filesystem: " + str(conditions["mount"]["mount_point"]) + " (" + str(conditions["mount"]["filesystem_type"]) + "), " + human_bytes(conditions["mount"]["available_bytes"]) + " available")
    lines.append("- Declared cadence: " + str(conditions["declared_cadence_seconds"]) + "s over " + str(conditions["declared_window_hours"]) + "h = " + str(conditions["rounds"]) + " rounds, seed " + str(conditions["seed"]))
    lines.append("- Plan: " + str(conditions["planned_window_seconds"]) + "s of observation time at that cadence inside the declared " + str(conditions["declared_window_seconds"]) + "s window, leaving a " + str(conditions["cliff_guard_seconds"]) + "s guard inside the same 24 hour raw retention cliff")
    lines.append("- Server mode: " + conditions["server_mode"] + " on 127.0.0.1:" + str(conditions["port"]))
    lines.append("")
    lines.append("## Steady write path")
    lines.append("")
    lines.append("- Stored observations: " + str(load["stored_rounds"]) + ", paused rounds: " + str(load["paused_rounds"]))
    lines.append("- Write latency p50 " + str(load["write_latency_ms"]["p50_ms"]) + "ms, p95 " + str(load["write_latency_ms"]["p95_ms"]) + "ms, max " + str(load["write_latency_ms"]["max_ms"]) + "ms")
    lines.append("- Dispositions: " + json.dumps(load["dispositions"]))
    lines.append("- Wall time: " + str(load["wall_seconds_before_pause"]) + "s before the pause, " + str(load["wall_seconds_after_pause"]) + "s after it")
    release = report["phases"]["release"]
    lines.append(
        "- Released instant "
        + str(release["released_instant"])
        + ": ledger observationCount "
        + str(release["before"]["observationCount"])
        + " -> "
        + str(release["after"]["observationCount"])
        + ", replayedCount "
        + str(release["before"]["replayedCount"])
        + " -> "
        + str(release["after"]["replayedCount"])
        + ", sampledCount "
        + str(release["before"]["sampledCount"])
        + " -> "
        + str(release["after"]["sampledCount"])
        + ", firstObservedAt "
        + str(release["before"]["firstObservedAt"])
        + " -> "
        + str(release["after"]["firstObservedAt"])
        + ", row stored: "
        + str(release["released_instant_stored"])
    )
    lines.append("")
    lines.append("## Storage")
    lines.append("")
    lines.append("- Database " + human_bytes(storage["database_bytes"]) + " over " + str(storage["page_count"]) + " pages of " + str(storage["page_size"]) + " bytes")
    lines.append("- Rows: " + json.dumps(storage["row_counts"]))
    lines.append("- Bytes per stored optional sample: " + str(storage["bytes_per_optional_sample"]))
    lines.append("")
    lines.append("## Read path")
    lines.append("")
    for label in ("1h", "6h", "24h", "24h_limited"):
        entry = reads[label]
        lines.append("- " + label + ": " + str(entry["items"]) + " items, truncated " + str(entry["truncated"]) + ", " + str(entry["latency_ms"]) + "ms, " + human_bytes(entry["payload_bytes"]) + " payload, coverage " + str(entry["series"]["coverageSeconds"]) + "s, gaps " + str(len(entry["gaps"])))
    default_window = reads["default_window"]
    lines.append("- default (no from, no to): " + str(default_window["window_seconds"]) + "s, " + str(default_window["items"]) + " items, " + str(default_window["latency_ms"]) + "ms, " + human_bytes(default_window["payload_bytes"]) + " payload")
    lines.append("- Never reported (" + reads["never_reported"]["metric"] + "): observed " + str(reads["never_reported"]["series"]["observed"]) + ", " + str(reads["never_reported"]["items"]) + " items")
    lines.append("- Refusals: " + json.dumps(reads["refusals"]))
    lines.append("")
    lines.append("## Multi-Node arrival and the per-Report cleanup budget")
    lines.append("")
    multi_node = report["phases"]["multi_node"]
    part1 = multi_node["part1"]
    drain = multi_node["drain"]
    lines.append(
        "- Burst: "
        + str(part1["accepted_reports"])
        + " of "
        + str(part1["reports"])
        + " Reports of "
        + str(part1["clone_nodes"] + 1)
        + " Nodes each on a "
        + str(part1["cadence_seconds"])
        + "s cadence: p50 "
        + str(part1["latency_ms"]["p50_ms"])
        + "ms, p95 "
        + str(part1["latency_ms"]["p95_ms"])
        + "ms, max "
        + str(part1["latency_ms"]["max_ms"])
        + "ms, "
        + str(part1["wall_seconds"])
        + "s wall"
    )
    lines.append(
        "- Burst rows: "
        + str(part1["planned_stored_rows"])
        + " planned inside the window, "
        + str(part1["stored_rows"])
        + " stored, "
        + str(part1["node_rows"])
        + " Node rows, "
        + str(part1["series_ledger_rows"])
        + " ledger rows; "
        + str(part1["planned_out_of_window_rounds"])
        + " rounds arrived already outside the window"
    )
    lines.append(
        "- Pure replay: stored "
        + str(part1["replay"]["before"]["total"])
        + " -> "
        + str(part1["replay"]["after"]["total"])
        + ", expired "
        + str(part1["replay"]["before"]["expired"])
        + " -> "
        + str(part1["replay"]["after"]["expired"])
    )
    lines.append(
        "- Drain at a "
        + str(drain["pace_seconds"])
        + "s pace: "
        + str(drain["rows_added_per_report"])
        + " rows added and "
        + str(drain["rows_released_per_report"])
        + " released per Report, max "
        + str(drain["max_released_by_one_cleanup"])
        + " by one cleanup (bound under test "
        + str(multi_node["bound_under_test"])
        + ", previous bound "
        + str(multi_node["falsified_bound"])
        + ", backlog floor under it "
        + str(drain["backlog_floor_under_previous_bound"])
        + " rows), p50 "
        + str(drain["latency_ms"]["p50_ms"])
        + "ms, "
        + str(drain["wall_seconds"])
        + "s wall"
    )
    lines.append(
        "- Decisive replay: stored "
        + str(drain["replay"]["before"]["total"])
        + " -> "
        + str(drain["replay"]["after"]["total"])
        + ", expired "
        + str(drain["replay"]["before"]["expired"])
        + " -> "
        + str(drain["replay"]["after"]["expired"])
    )
    lines.append("")
    tiers = report["phases"]["tiers"]
    tier_planned = tiers["planned"]
    tier_walk = tiers["walk"]
    tier_storage = tiers["storage"]
    lines.append("## Aggregate tiers (issue #214)")
    lines.append("")
    lines.append(
        "- Seeded "
        + str(tiers["seeded_reports"])
        + " hourly Reports on a fresh Node over a declared "
        + str(tier_planned["declared_span_seconds"])
        + "s span, newest instant "
        + str(tiers["instants"]["newest_age_seconds"])
        + "s old: write latency p50 "
        + str(tiers["write_latency_ms"]["p50_ms"])
        + "ms, p95 "
        + str(tiers["write_latency_ms"]["p95_ms"])
        + "ms, max "
        + str(tiers["write_latency_ms"]["max_ms"])
        + "ms, "
        + str(tiers["seed_wall_seconds"])
        + "s wall"
    )
    lines.append(
        "- Counted but not stored: node_metric_samples "
        + str(tiers["raw_rows"]["total"])
        + " rows (expired "
        + str(tiers["raw_rows"]["expired"])
        + "), ledger "
        + str(tiers["reads"]["30d"]["observation_count"])
        + " observations, tiers "
        + str(tiers["tier_rows"]["total"])
        + " rows over "
        + str(tiers["tier_rows"]["distinct_buckets"])
        + " (node, metric, bucket_start) coordinates and "
        + str(tiers["tier_rows"]["distinct_series"])
        + " series"
    )
    lines.append(
        "- Buckets per series: "
        + str(tier_planned["one_minute_buckets_per_series"])
        + " one minute ("
        + str(tiers["tier_rows"]["one_minute"])
        + " rows), "
        + str(tier_planned["five_minute_buckets_per_series"])
        + " five minute ("
        + str(tiers["tier_rows"]["five_minute"])
        + " rows)"
    )
    for label in ("30d", "20h", "10d", "4d"):
        entry = tiers["reads"][label]
        lines.append(
            "- Read "
            + label
            + ": "
            + str(entry["items"])
            + " items, grain "
            + str(entry["grain"])
            + ", sources "
            + json.dumps(entry["sources"])
            + ", availability "
            + str(entry["availability"])
            + ", series coverage "
            + str(entry["series_coverage_seconds"])
            + "s, truncated "
            + str(entry["truncated"])
            + ", continuation "
            + str(entry["continuation"])
            + ", gaps "
            + str(entry["gaps"])
            + " ("
            + ",".join(str(kind) for kind in entry["gap_kinds"])
            + ") max "
            + str(entry["gap_max_seconds"])
            + "s, "
            + str(entry["latency_ms"])
            + "ms, "
            + human_bytes(entry["payload_bytes"])
            + " payload"
        )
    inside_horizon = tiers["horizon"]["inside"]
    beyond_horizon = tiers["horizon"]["beyond"]
    lines.append(
        "- Availability over a "
        + str(inside_horizon["history_horizon_days"])
        + " day horizon (raw window "
        + str(inside_horizon["raw_retention_days"])
        + " day): a 30 hour old range answers availability "
        + json.dumps(inside_horizon["availability"])
        + " with "
        + str(inside_horizon["items"])
        + " item (grain "
        + str(inside_horizon["grain"])
        + ", sources "
        + json.dumps(inside_horizon["sources"])
        + "); a range ending before the horizon answers "
        + json.dumps(beyond_horizon["availability"])
        + " with "
        + str(beyond_horizon["items"])
        + " items, its requested start "
        + str(beyond_horizon["requested_from"])
        + " preserved and its effective start clamped to "
        + str(beyond_horizon["effective_from"])
    )
    thirty = tiers["reads"]["30d"]
    lines.append(
        "- 30 day grains: "
        + json.dumps(
            {
                grain: {
                    "points": found["points"],
                    "minValue": found["minValue"],
                    "maxValue": found["maxValue"],
                    "sampleCount": found["sampleCount"],
                    "sources": found["sources"],
                }
                for grain, found in thirty["grains"].items()
            }
        )
    )
    lines.append("- 30 day segments: " + json.dumps(thirty["segments"]))
    lines.append(
        "- Paging walk at limit "
        + str(tier_walk["limit"])
        + ": "
        + str(tier_walk["pages"])
        + " pages, "
        + str(tier_walk["items"])
        + " items, "
        + str(tier_walk["distinct_coordinates"])
        + " distinct coordinates, repeats "
        + str(tier_walk["repeats"])
        + ", missing "
        + str(tier_walk["missing_from_walk"])
        + ", extra "
        + str(tier_walk["extra_in_walk"])
        + ", same set as the single read "
        + str(tier_walk["matches_single_read"])
        + ", strictly descending "
        + str(tier_walk["strictly_descending"])
        + ", last page truncated "
        + str(tier_walk["last_truncated"])
        + " with continuation "
        + str(tier_walk["last_continuation"])
        + ", latency p50 "
        + str(tier_walk["latency_ms"]["p50_ms"])
        + "ms, p95 "
        + str(tier_walk["latency_ms"]["p95_ms"])
        + "ms, max "
        + str(tier_walk["latency_ms"]["max_ms"])
        + "ms, "
        + human_bytes(tier_walk["payload_bytes_total"])
        + " payload total"
    )
    lines.append(
        "- Aggregate storage: "
        + str(tier_storage["rows_added"])
        + " rows added ("
        + str(tier_storage["aggregate_rows_before"])
        + " -> "
        + str(tier_storage["aggregate_rows_after"])
        + "), "
        + str(tier_storage["bytes_per_bucket"])
        + " bytes per bucket, "
        + str(tier_storage["bytes_per_bucket_added"])
        + " bytes per added bucket, "
        + human_bytes(tier_storage["aggregate_bytes_after"])
        + " in node_metric_aggregates plus indexes over "
        + str(tier_storage["aggregate_footprint_after"]["pages"])
        + " pages, database "
        + human_bytes(tier_storage["database_bytes_before"])
        + " -> "
        + human_bytes(tier_storage["database_bytes_after"])
        + " ("
        + str(tier_storage["pages_added"])
        + " pages, "
        + human_bytes(tier_storage["database_bytes_added"])
        + ")"
    )
    lines.append("- Bytes per tier: " + json.dumps(tier_storage["bytes_per_tier"]))
    for plant in tiers["planted"]["plants"]:
        lines.append(
            "- Planted "
            + plant["kind"]
            + " instant "
            + str(plant["age_seconds"])
            + "s old ("
            + str(plant["disposition"])
            + "): tiers "
            + json.dumps(plant["tier_rows_after_own_report"])
            + ", one minute bucket found "
            + str(plant["buckets_after_own_report"]["one_minute"]["found"])
            + ", five minute bucket found "
            + str(plant["buckets_after_own_report"]["five_minute"]["found"])
            + ", released by the later Report "
            + json.dumps(plant["released_by_later_report"])
        )
    later_report = tiers["planted"]["later_report"]
    lines.append(
        "- Later Report ("
        + str(later_report["disposition"])
        + ", observed "
        + later_report["observed_at"]
        + "): its own tiers "
        + json.dumps(later_report["tier_rows"])
    )
    lines.append("")
    lines.append("## Host resource history (issue #215)")
    lines.append("")
    lines.append(
        "- One Agent stored "
        + str(len(host["ledger_after"]))
        + " Host series for "
        + str(host["nodes"])
        + " Nodes ("
        + str(len(HOST_SHARED_SERIES))
        + " shared plus "
        + str(len(host["planted"]["mounts"]) * 2)
        + " for the two planted mounts): "
        + str(host["nodes_with_their_own_series"])
        + " of those Nodes stored Node series of their own, and the raw table carries "
        + ", ".join(host["columns"])
    )
    lines.append(
        "- Before the planted Report the family was "
        + ", ".join(HOST_SHARED_SERIES)
        + " under the empty dimension, "
        + str(host["family_before"][0]["samples"] if host["family_before"] else 0)
        + " samples per series"
    )
    lines.append(
        "- The Agent route and "
        + str(len(host["routes"]["by_node"]))
        + " Node host routes answered the same "
        + str(host["routes"]["reads"]["cpu_percent"]["items"])
        + " cpu_percent points (p50 "
        + str(round(percentile(host["routes"]["latency_ms"], 0.5), 3))
        + "ms over "
        + str(len(host["routes"]["latency_ms"]))
        + " reads, payload "
        + str(host["routes"]["reads"]["cpu_percent"]["payload_bytes"])
        + " bytes)"
    )
    lines.append(
        "- Planted one Report with "
        + str(len(host["planted"]["mounts"]))
        + " mounts at "
        + host["planted"]["observed_at"]
        + " ("
        + str(host["planted"]["disposition"])
        + "): "
        + json.dumps(host["planted"]["mount_reads"])
    )
    lines.append(
        "- Bounds: MAX_HOST_MOUNTS "
        + str(host["bounds"]["max_host_mounts"])
        + ", one maximal Host Report "
        + str(host["bounds"]["server_max_host_rows_per_report"])
        + " rows against cleanup batches "
        + str(host["bounds"]["host_metric_cleanup_batch"])
        + " raw and "
        + str(host["bounds"]["aggregate_cleanup_batch"])
        + " aggregate; forbidden Swap or disk-IO tokens in the family "
        + str(len(host["bounds"]["forbidden_in_family"]))
    )
    lines.append(
        "- dbstat "
        + json.dumps(host["footprint"]["objects"])
        + ", "
        + str(host["footprint"]["bytes_per_sample"])
        + " bytes per Host sample"
    )
    lines.append(
        "- Released boundary: "
        + str(host["rows_at_or_before_the_released_instant"])
        + " Host rows at or before "
        + host["released_instant"]
        + "; route refusals "
        + json.dumps({name: row["code"] for name, row in sorted(host["refusals"].items())})
    )
    lines.append("")
    lines.append("## Storage history per mount path (issue #216)")
    lines.append("")
    lines.append(
        "- Declared: two series per mount path "
        + json.dumps(mount["bounds"]["mount_series"])
        + " on the shared Host family, at most "
        + str(mount["bounds"]["max_host_mounts"])
        + " mounts in one Report, a list bound of "
        + str(mount["bounds"]["coverage_limit"])
        + " paths answered newest first and read "
        + str(mount["bounds"]["coverage_reads"])
        + " times, a silence bound of clamp(cadence, 1, "
        + str(mount["bounds"]["max_observed_cadence_seconds"])
        + ") * "
        + str(mount["bounds"]["gap_cadence_factor"])
        + " floored at "
        + str(mount["bounds"]["min_gap_seconds"])
        + " seconds from the newest "
        + str(mount["bounds"]["cadence_samples"])
        + " Host observations, and the read planned through "
        + mount["bounds"]["coverage_index"]
    )
    lines.append(
        "- Planted: "
        + str(mount["coverage"]["order_size"])
        + " mount paths on the first Agent ("
        + str(mount["planted"]["retired"]["paths"])
        + " retired at "
        + mount["planted"]["retired"]["observed_at"]
        + ", "
        + str(mount["planted"]["middle"]["paths"])
        + " at "
        + mount["planted"]["middle"]["observed_at"]
        + ", "
        + str(mount["planted"]["newest"]["paths"])
        + " at "
        + mount["planted"]["newest"]["observed_at"]
        + ") and "
        + str(len(mount["solo"]["paths"]))
        + " on a second Agent whose one Report cannot measure a cadence, on top of the "
        + str(len(mount["preexisting"]))
        + " mount paths the Host phase above already held"
    )
    lines.append(
        "- Rhythm: three Reports at "
        + str(mount["bounds"]["rhythm_seconds"])
        + "s below the newest instant state the cadence the Agent is keeping now, so the list answers"
        " cadenceSeconds "
        + str(mount["coverage"]["read"]["cadence_seconds"])
        + " with a silence bound of "
        + str(mount["coverage"]["read"]["silence_threshold_seconds"])
        + "s"
    )
    lines.append(
        "- Answer: "
        + str(mount["coverage"]["read"]["mounts"])
        + " mounts of "
        + str(mount["coverage"]["read"]["mount_limit"])
        + ", truncated "
        + str(mount["coverage"]["read"]["truncated"]).lower()
        + ", from "
        + str(mount["coverage"]["order_size"])
        + " ledger paths: first "
        + str(mount["coverage"]["answered"][0] if mount["coverage"]["answered"] else None)
        + ", last "
        + str(mount["coverage"]["answered"][-1] if mount["coverage"]["answered"] else None)
        + ", dropped "
        + str(len(mount["coverage"]["dropped"]))
        + " of a declared ledger of "
        + str(mount["coverage"]["declared_order_size"])
    )
    lines.append(
        "- Dropped by instant: "
        + json.dumps(mount["coverage"]["dropped_by_instant"])
        + " against the declared "
        + json.dumps(mount["coverage"]["expected_dropped"])
        + "; kept by instant "
        + json.dumps(mount["coverage"]["kept_by_instant"])
        + " against "
        + json.dumps(mount["coverage"]["expected_kept"])
    )
    lines.append(
        "- Silence: cadenceSeconds "
        + str(mount["coverage"]["read"]["cadence_seconds"])
        + " with silenceThresholdSeconds "
        + str(mount["coverage"]["read"]["silence_threshold_seconds"])
        + "; the retired path "
        + mount["coverage"]["retired_path"]
        + " is "
        + mount["coverage"]["retired_entry"]["observation_state"]
        + " for "
        + str(mount["coverage"]["retired_entry"]["silent_seconds"])
        + " seconds, the newest path "
        + mount["coverage"]["live_path"]
        + " is "
        + mount["coverage"]["live_entry"]["observation_state"]
        + ", and the second Agent states "
        + json.dumps(mount["solo"]["read"]["states"])
        + " at cadence "
        + str(mount["solo"]["read"]["cadence_seconds"])
    )
    lines.append(
        "- Released boundary: "
        + mount["released"]["entry"]["mount_path"]
        + " at "
        + mount["released"]["observed_at"]
        + " is "
        + mount["released"]["entry"]["observation_state"]
        + " with usedValue "
        + str(mount["released"]["entry"]["used_latest_value"])
        + ", releasedBefore "
        + str(mount["released"]["entry"]["used_released_before"])
        + ", observationCount "
        + str(mount["released"]["entry"]["used_observation_count"])
    )
    lines.append(
        "- A retired path keeps its reading: "
        + str(mount["history"]["items"])
        + " point on "
        + str(mount["history"]["dimension"])
        + " at "
        + json.dumps(mount["history"]["points"])
        + ", the value the mount list still answers"
    )
    lines.append(
        "- Rows: "
        + json.dumps(mount["rows"]["agent"])
        + " for the Agent with several Nodes against "
        + json.dumps(mount["rows"]["solo"])
        + " for the Agent with one Node, on columns "
        + json.dumps(mount["rows"]["columns"])
        + " (no node_id, so mount rows cannot follow the Node count)"
    )
    lines.append(
        "- dbstat: "
        + json.dumps(mount["footprint"]["after"]["objects"])
        + ", "
        + str(mount["footprint"]["mount_index_bytes"])
        + " bytes of it the mount index at "
        + str(mount["footprint"]["bytes_per_mount_series"])
        + " bytes per mount series"
    )
    lines.append(
        "- Query plan: "
        + json.dumps(mount["plan"]["lines"])
        + " under the Server's own binds, forbidden plans "
        + json.dumps(mount["plan"]["forbidden"])
    )
    lines.append(
        "- Read cost: body "
        + json.dumps(mount["coverage"]["read"]["payload_bytes"])
        + " bytes, latency "
        + json.dumps(mount["coverage"]["read"]["latency_ms"])
        + " ms, Cache-Control "
        + str(mount["coverage"]["read"]["cache_control"])
        + ", answered at "
        + str(mount["coverage"]["read"]["answered_at"])
        + "; the phase took "
        + str(mount["wall_seconds"])
        + " seconds"
    )
    lines.append("## Recorded synchronization state history (issue #217)")
    lines.append("")
    lines.append(
        "- Conditions: one Agent ("
        + state["instrument"]["agent_id"]
        + ") enrolled for this phase alone, "
        + str(state["instrument"]["nodes"])
        + " cloned Nodes (probe "
        + state["instrument"]["probe_node_id"]
        + ", ledger "
        + state["instrument"]["ledger_node_id"]
        + ") stated by every Report, "
        + str(state["instrument"]["reports"])
        + " Reports at a "
        + str(state["instrument"]["rhythm_seconds"])
        + "s rhythm over a "
        + str(state["instrument"]["lead_seconds"])
        + "s lead, components "
        + json.dumps(state["instrument"]["components"])
        + ", retained "
        + str(state["instrument"]["retention_days"])
        + " days, anchored after "
        + str(state["instrument"]["anchor_seconds"])
        + "s of unchanged silence, a cleanup batch of "
        + str(state["instrument"]["cleanup_batch"])
        + " rows against a maximal state Report of "
        + str(state["instrument"]["max_state_rows_per_report"])
        + " rows, read "
        + str(state["instrument"]["reads"])
        + " times through "
        + state["instrument"]["read_index"]
        + " and led by "
        + state["instrument"]["ledger_index"]
    )
    lines.append(
        "- Sampling: "
        + str(state["instrument"]["hardware"]["cpu_count"])
        + " CPUs, "
        + human_bytes(state["instrument"]["hardware"]["memory_total_bytes"])
        + " memory, "
        + str(state["instrument"]["mount"]["filesystem_type"])
        + " with "
        + human_bytes(state["instrument"]["mount"]["available_bytes"])
        + " available, "
        + human_bytes(state["instrument"]["database_bytes"])
        + " of database at "
        + state["instrument"]["sampled_at"]
        + "; the phase took "
        + str(state["wall_seconds"])
        + " seconds over "
        + str(len(state["deliveries"]))
        + " Reports"
    )
    lines.append(
        "- Recorded: sync "
        + str(len(state["expect"]["sync_changes"]))
        + " changes + "
        + str(len(state["expect"]["sync_anchors"]))
        + " anchors, consensus "
        + str(len(state["expect"]["consensus_changes"]))
        + " changes + "
        + str(len(state["expect"]["consensus_anchors"]))
        + " anchors, "
        + str(state["expect"]["entry_count"])
        + " counted deliveries per component; cadence sync "
        + str(state["reads"]["sync"]["payload"]["cadenceSeconds"])
        + "s and consensus "
        + str(state["reads"]["consensus"]["payload"]["cadenceSeconds"])
        + "s, coverage sync "
        + str(state["reads"]["sync"]["payload"]["coverageSeconds"])
        + "s and consensus "
        + str(state["reads"]["consensus"]["payload"]["coverageSeconds"])
        + "s, gaps "
        + json.dumps(state["reads"]["sync"]["payload"]["gaps"])
        + " and "
        + json.dumps(state["reads"]["consensus"]["payload"]["gaps"])
    )
    lines.append(
        "- Chain heights: "
        + str(len(state["instrument"]["node_series"]))
        + " series per Node, "
        + str(len(state["expect"]["height_units"]))
        + " samples each, returned as "
        + str(len(state["reads"]["heights"]["probe"]["sync_current_block"]["items"]))
        + " items spaced "
        + str(state["expect"]["height_rhythm_seconds"])
        + "s apart (this route states no cadence) over "
        + str(state["reads"]["heights"]["probe"]["sync_current_block"]["coverage_seconds"])
        + "s of coverage, "
        + json.dumps(state["reads"]["heights"]["probe"]["sync_current_block"]["gaps"])
        + "; stored rows per series "
        + json.dumps(state["rows_after"]["probe"]["heights"])
        + "; the probe Node keeps "
        + str(state["rows_after"]["probe"]["observations"])
        + " state rows over "
        + str(state["rows_after"]["probe"]["series_rows"])
        + " ledger rows "
        + json.dumps(state["rows_after"]["probe"]["by_component"])
    )
    lines.append(
        "- Frozen across the pause: both components held at entry_count "
        + str(state["expect"]["frozen"]["entry_count"])
        + ", last entry "
        + state["expect"]["frozen"]["last_entry_at"]
        + " and last observed "
        + state["expect"]["frozen"]["last_observed_at"]
        + " while "
        + str(len(state["sql"]["skipped series"]))
        + " series counted "
        + str(state["expect"]["pause"]["reports"])
        + " deliveries each as skipped between "
        + state["expect"]["pause"]["from"]
        + " and "
        + state["expect"]["pause"]["to"]
    )
    lines.append(
        "- Read surface: "
        + str(len(state["reads"]["latency_ms"]))
        + " reads at p50 "
        + str(percentile(state["reads"]["latency_ms"], 0.5))
        + "ms and p95 "
        + str(percentile(state["reads"]["latency_ms"], 0.95))
        + "ms; page one carried "
        + str(len(state["reads"]["page one"]["payload"]["entries"]))
        + " entries truncated with the cursor "
        + str(state["reads"]["page one"]["payload"]["continuation"])
        + " and page two carried "
        + str(
            len(state["reads"]["page two"]["payload"]["entries"]) if state["reads"]["page two"] is not None else 0
        )
        + " older entries ending the walk; refusals "
        + json.dumps(
            {
                name: [entry["status"], (entry["payload"].get("error") or {}).get("code")]
                for name, entry in state["reads"]["refusals"].items()
            }
        )
    )
    lines.append(
        "- Plans: "
        + json.dumps(state["plans"])
    )
    lines.append(
        "- Purge: an anonymous purge answered "
        + str(state["purge"]["anonymous"]["status"])
        + " "
        + str(state["purge"]["anonymous"]["code"])
        + ", a session without the CSRF token answered "
        + str(state["purge"]["unguarded"]["status"])
        + " "
        + str(state["purge"]["unguarded"]["code"])
        + ", a mismatched confirmation answered "
        + str(state["purge"]["mismatched"]["status"])
        + " "
        + str(state["purge"]["mismatched"]["code"])
        + ", and the real purge removed "
        + json.dumps(state["purge"]["purged"]["removed"])
        + "; the purged Node then answered "
        + str(state["reads"]["after purge"]["ledger"]["status"])
        + " while the probe Node answered "
        + str(state["reads"]["after purge"]["probe"]["status"])
        + " with "
        + str(len(state["reads"]["after purge"]["probe"]["payload"]["entries"]))
        + " entries"
    )
    lines.append("")
    lines.append("")
    lines.append("## Checks")
    lines.append("")
    for entry in report["checks"]:
        lines.append("- [" + ("ok" if entry["ok"] else "FAILED") + "] " + entry["name"] + ": expected " + str(entry["expected"]) + ", observed " + str(entry["observed"]))
    lines.append("")
    lines.append("## Not delivered")
    lines.append("")
    for item in report["not_delivered"]:
        lines.append("- " + item)
    lines.append("")
    path.write_text("\n".join(lines), encoding="utf-8")


def parse_args(argv: list) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Issue #213 raw metric history baseline")
    parser.add_argument("--output-root", default="target/metric-history-baseline")
    parser.add_argument("--binary", default=str(DEFAULT_BINARY))
    # The declared window is a full day, the same 24 hours the raw retention
    # floor keeps: the policy family default and floor are both one day.
    parser.add_argument("--hours", type=float, default=24.0)
    parser.add_argument("--cadence-seconds", type=int, default=30)
    parser.add_argument("--seed", type=int, default=213)
    parser.add_argument("--port", type=int, default=0)
    return parser.parse_args(argv)


def main(argv: list) -> int:
    args = parse_args(argv)
    run = BaselineRun(args)
    try:
        report = run.run()
    finally:
        run.stop_server()
    json_path = run.run_dir / "baseline.json"
    markdown_path = run.run_dir / "baseline.md"
    write_json_report(report, json_path)
    write_markdown_report(report, markdown_path)
    failed = [entry for entry in report["checks"] if not entry["ok"]]
    print("report:   " + str(json_path))
    print("markdown: " + str(markdown_path))
    print("  checks: " + str(len(report["checks"]) - len(failed)) + "/" + str(len(report["checks"])) + " ok")
    for entry in failed:
        print("  FAILED: " + entry["name"] + " expected " + str(entry["expected"]) + " observed " + str(entry["observed"]))
    return 1 if failed else 0


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except BaselineError as error:
        print("baseline failed: " + str(error), file=sys.stderr)
        sys.exit(2)
