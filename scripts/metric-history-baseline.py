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
#   * crates/platpulse-server/src/retention.rs:175 carries
#     "const NODE_METRIC_CLEANUP_BATCH: i64 = 2048;" and the raw Node sample
#     statement at crates/platpulse-server/src/retention.rs:196-209 deletes at
#     most that many expired rows per call. The bound it replaced was 128.
#   * crates/platpulse-core/src/protocol.rs:33 allows MAX_NODE_OBSERVATIONS =
#     256 Nodes in one Report, and the Server stores at most five series per
#     Node (crates/platpulse-server/src/metric_history.rs:47-53), so one
#     accepted Report can add up to 1280 rows at once.
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
MULTI_NODE_SERIES_PER_NODE = 5
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
NODE_METRIC_CLEANUP_BATCH = 2048
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
#   * crates/platpulse-server/src/retention.rs:224 bounds one tier cleanup at
#     AGGREGATE_CLEANUP_BATCH = 2048 rows; :237 deletes grain_seconds = 60 buckets
#     below the 7 day window and :247 grain_seconds = 300 buckets below the 30 day
#     one, and the pass runs after every accepted Report
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
TIER_SERIES = 2
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
        checks = self.evaluate(load, restatements, release, reads, multi_node, tiers, storage)
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
                "storage": storage,
            },
            "checks": checks,
            "not_delivered": [
                "The 24 hour window was compressed in wall time: every observation instant is real, but the Reports"
                " were submitted as fast as the Server accepted them instead of one per declared cadence.",
                "Agent-side collection was not measured: Reports came from the fixture through the real ingestion"
                " path, not from a running platpulse-agent process.",
                "One Agent, one Node and one mount were measured; no multi-disk or network filesystem deployment.",
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
