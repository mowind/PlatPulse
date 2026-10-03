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

    def read(self, from_instant: str, to_instant: str, limit: int, metric: str = "process_cpu_percent"):
        query = "&from=" + from_instant + "&to=" + to_instant + "&limit=" + str(limit)
        return read_history(self.client, self.cookie, self.node_id, metric, query)

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
        status, _, body, _ = admin_get(
            self.client,
            self.cookie,
            history_url(self.node_id, "process_cpu_percent", "&from=" + instant(-30 * 3600) + "&to=" + instant(-29 * 3600)),
        )
        refusals["released_range"] = {
            "status": status,
            "availability": json.loads(body).get("availability"),
            "requestedFrom": json.loads(body).get("requestedFrom"),
            "items": len(json.loads(body).get("items", [])),
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
        self, load: dict, restatements: dict, release: dict, reads: dict, multi_node: dict, storage: dict
    ) -> list:
        full = reads["24h"]
        planned = self.planned_coverage()
        part1 = multi_node["part1"]
        drain = multi_node["drain"]
        ledger_after_load = restatements["before"]
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
                "a range older than the released history is answered as unavailable",
                "unavailable with the requested start preserved and no items",
                json.dumps(reads["refusals"]["released_range"]),
                reads["refusals"]["released_range"]["availability"] == "unavailable"
                and reads["refusals"]["released_range"]["items"] == 0
                and reads["refusals"]["released_range"]["requestedFrom"] is not None,
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
        checks = self.evaluate(load, restatements, release, reads, multi_node, storage)
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
                "storage": storage,
            },
            "checks": checks,
            "not_delivered": [
                "The 24 hour window was compressed in wall time: every observation instant is real, but the Reports"
                " were submitted as fast as the Server accepted them instead of one per declared cadence.",
                "Agent-side collection was not measured: Reports came from the fixture through the real ingestion"
                " path, not from a running platpulse-agent process.",
                "One Agent, one Node and one mount were measured; no multi-disk or network filesystem deployment.",
                "The aggregate tiers (one minute and five minute) belong to issue #214 and were not exercised.",
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
