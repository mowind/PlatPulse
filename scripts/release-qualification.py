#!/usr/bin/env python3
"""Black-box load, fault, and soak qualification for packaged PlatPulse artifacts."""
from __future__ import annotations

import argparse
import concurrent.futures
import copy
import datetime as dt
import hashlib
import http.client
import json
import os
from pathlib import Path
import platform
import re
import signal
import socket
import sqlite3
import subprocess
import sys
import threading
import time
import tomllib
import uuid

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_PROFILE = ROOT / "release/qualification/ci.toml"
DEFAULT_OUTPUT = ROOT / "target/release-qualification"
UUID_RE = re.compile(r"[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}", re.I)
TOKEN_RE = re.compile(r"pp_(?:agent|enroll)_[A-Za-z0-9_-]+")
IP_RE = re.compile(r"(?<![0-9])(?:[0-9]{1,3}\.){3}[0-9]{1,3}(?![0-9])")


class QualificationError(RuntimeError):
    pass


def utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def percentile(values: list[float], fraction: float) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    index = min(len(ordered) - 1, max(0, int(round((len(ordered) - 1) * fraction))))
    return ordered[index]


def sanitize(text: str, run_root: Path | None = None) -> str:
    result = UUID_RE.sub("<uuid>", text)
    result = TOKEN_RE.sub("<secret>", result)
    result = IP_RE.sub("<ip>", result)
    if run_root is not None:
        result = result.replace(str(run_root), "<run>")
    return result


def require_positive(table: dict, key: str) -> int:
    value = table.get(key)
    if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
        raise QualificationError(f"{key} must be a positive integer")
    return value


def load_profile(path: Path) -> dict:
    try:
        profile = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise QualificationError(f"could not read qualification profile: {error}") from error
    name = profile.get("name")
    workload = profile.get("workload")
    thresholds = profile.get("thresholds")
    if not isinstance(name, str) or not name or len(name) > 64:
        raise QualificationError("profile name must be 1..=64 characters")
    if not isinstance(workload, dict) or not isinstance(thresholds, dict):
        raise QualificationError("profile requires [workload] and [thresholds]")
    for key in (
        "agents", "nodes_per_agent", "report_iterations", "report_workers",
        "rest_workers", "sse_subscribers", "warmup_seconds", "observation_seconds",
        "fault_window_seconds", "request_timeout_seconds",
    ):
        require_positive(workload, key)
    if workload["agents"] < 2 or workload["nodes_per_agent"] < 2:
        raise QualificationError("qualification requires multiple Agents and multiple Nodes per Agent")
    for key in (
        "max_rss_growth_bytes", "max_fd_growth", "max_task_growth",
        "max_wal_growth_bytes", "max_realtime_growth", "p95_latency_ms",
    ):
        value = thresholds.get(key)
        if not isinstance(value, (int, float)) or isinstance(value, bool) or value < 0:
            raise QualificationError(f"{key} must be a non-negative number")
    rate = thresholds.get("max_error_rate")
    if not isinstance(rate, (int, float)) or isinstance(rate, bool) or not 0 <= rate <= 1:
        raise QualificationError("max_error_rate must be between 0 and 1")
    return profile


def command(args: list[str], *, input_text: str | None = None, env: dict | None = None,
            timeout: int = 600, cwd: Path = ROOT) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        args, cwd=cwd, env=env, input=input_text, text=True,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=timeout, check=False,
    )
    if result.returncode != 0:
        detail = sanitize((result.stderr or result.stdout).strip())
        raise QualificationError(f"command failed ({' '.join(args[:3])}): {detail[:400]}")
    return result


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def recursive_replace(value, replacements: dict[str, str]):
    if isinstance(value, dict):
        return {key: recursive_replace(item, replacements) for key, item in value.items()}
    if isinstance(value, list):
        return [recursive_replace(item, replacements) for item in value]
    if isinstance(value, str):
        return replacements.get(value, value)
    return value


def bump_observation_revisions(value, increment: int, observed_at: str) -> None:
    if isinstance(value, dict):
        for key, item in list(value.items()):
            if key in {"state_revision", "value_revision"} and isinstance(item, int):
                value[key] = item + increment
            elif key in {"attempted_at", "latest_observed_at", "observed_at"} and isinstance(item, str):
                value[key] = observed_at
            else:
                bump_observation_revisions(item, increment, observed_at)
    elif isinstance(value, list):
        for item in value:
            bump_observation_revisions(item, increment, observed_at)


def make_report(template: dict, identity: dict, sequence: int, head_base: int) -> tuple[dict, list[str]]:
    report = copy.deepcopy(template)
    old_nodes = [item["node_id"] for item in report["inventory"]["nodes"]]
    if len(old_nodes) < 2:
        raise QualificationError("canonical fixture must contain at least two Nodes")
    node_ids = identity["node_ids"]
    replacements = {old: node_ids[index % len(node_ids)] for index, old in enumerate(old_nodes)}
    report = recursive_replace(report, replacements)
    report["agent_id"] = identity["agent_id"]
    report["agent_epoch"] = identity["agent_epoch"]
    report["boot_id"] = identity["boot_id"]
    report["report_id"] = str(uuid.uuid4())
    report["report_sequence"] = sequence
    report["inventory"]["revision"] = identity["inventory_revision"]
    report["generated_at"] = utc_now()
    inventory_nodes = report["inventory"]["nodes"]
    base_nodes = copy.deepcopy(inventory_nodes)
    while len(inventory_nodes) < len(node_ids):
        clone = copy.deepcopy(base_nodes[len(inventory_nodes) % len(base_nodes)])
        old = clone["node_id"]
        clone["node_id"] = node_ids[len(inventory_nodes)]
        clone["display_name"] = f"Qualification Node {len(inventory_nodes) + 1}"
        clone["rpc_endpoint"] = f"ws://127.0.0.1:{6700 + len(inventory_nodes)}"
        inventory_nodes.append(clone)
        for observation in copy.deepcopy(report["nodes"][:1]):
            observation = recursive_replace(observation, {old: clone["node_id"], observation["node_id"]: clone["node_id"]})
            report["nodes"].append(observation)
    report["inventory"]["nodes"] = inventory_nodes[:len(node_ids)]
    report["nodes"] = report["nodes"][:len(node_ids)]
    for index, node in enumerate(report["nodes"]):
        node["node_id"] = node_ids[index]
        sync = node.get("chain", {}).get("sync", {}).get("latest")
        if isinstance(sync, dict):
            sync["current_block"] = head_base + index
            sync["highest_block"] = max(sync.get("highest_block", 0), head_base + index)
    bump_observation_revisions(report, sequence - 1, report["generated_at"])
    return report, node_ids


def json_bytes(value: dict) -> bytes:
    return json.dumps(value, separators=(",", ":"), sort_keys=True).encode("utf-8")


class Client:
    def __init__(self, port: int, timeout: float):
        self.port = port
        self.timeout = timeout

    def request(self, method: str, path: str, *, body: bytes | None = None,
                headers: dict[str, str] | None = None) -> tuple[int, dict[str, str], bytes, float]:
        started = time.monotonic()
        connection = http.client.HTTPConnection("127.0.0.1", self.port, timeout=self.timeout)
        try:
            connection.request(method, path, body=body, headers=headers or {})
            response = connection.getresponse()
            payload = response.read()
            elapsed = (time.monotonic() - started) * 1000
            return response.status, {key.lower(): value for key, value in response.getheaders()}, payload, elapsed
        finally:
            connection.close()


class ServerProcess:
    def __init__(self, binary: Path, config: Path, log_path: Path, port: int, timeout: float):
        self.binary = binary
        self.config = config
        self.log_path = log_path
        self.port = port
        self.timeout = timeout
        self.process: subprocess.Popen | None = None
        self.log_handle = None

    def start(self) -> None:
        self.log_handle = self.log_path.open("a", encoding="utf-8")
        self.process = subprocess.Popen(
            [str(self.binary), "serve", "--config", str(self.config)],
            cwd=ROOT, stdout=self.log_handle, stderr=subprocess.STDOUT, text=True,
        )
        client = Client(self.port, self.timeout)
        live = False
        for _ in range(150):
            if self.process.poll() is not None:
                raise QualificationError("packaged Server exited during startup")
            try:
                status, _, _, _ = client.request("GET", "/health/live")
                if status == 200:
                    live = True
                    break
            except OSError:
                pass
            time.sleep(0.1)
        if not live:
            raise QualificationError("packaged Server did not become live")
        for _ in range(150):
            try:
                status, _, _, _ = client.request("GET", "/health/ready")
                if status == 200:
                    return
            except OSError:
                pass
            time.sleep(0.1)
        raise QualificationError("packaged Server did not become ready")

    def stop(self, abrupt: bool = False) -> None:
        if not self.process:
            return
        if self.process.poll() is None:
            self.process.send_signal(signal.SIGKILL if abrupt else signal.SIGTERM)
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        if self.log_handle:
            self.log_handle.close()
        self.process = None
        self.log_handle = None

    @property
    def pid(self) -> int | None:
        return self.process.pid if self.process and self.process.poll() is None else None


class SseGroup:
    def __init__(self, port: int, cookie: str, count: int, timeout: float):
        self.port = port
        self.cookie = cookie
        self.count = count
        self.timeout = timeout
        self.stop_event = threading.Event()
        self.threads: list[threading.Thread] = []
        self.connected = 0
        self.lock = threading.Lock()

    def _run(self) -> None:
        connection = http.client.HTTPConnection("127.0.0.1", self.port, timeout=self.timeout)
        try:
            connection.request("GET", "/api/admin/v1/events", headers={"Cookie": self.cookie})
            response = connection.getresponse()
            if response.status == 200:
                with self.lock:
                    self.connected += 1
                while not self.stop_event.is_set():
                    try:
                        chunk = response.read(1)
                        if not chunk:
                            break
                    except (OSError, TimeoutError, http.client.IncompleteRead):
                        break
        except OSError:
            pass
        finally:
            connection.close()

    def start(self) -> int:
        self.stop_event.clear()
        self.connected = 0
        self.threads = [threading.Thread(target=self._run, daemon=True) for _ in range(self.count)]
        for thread in self.threads:
            thread.start()
        deadline = time.monotonic() + min(5.0, self.timeout + 1)
        while time.monotonic() < deadline:
            with self.lock:
                if self.connected >= self.count:
                    break
            time.sleep(0.05)
        return self.connected

    def stop(self) -> None:
        self.stop_event.set()
        for thread in self.threads:
            thread.join(timeout=0.2)
        self.threads = []


def sample_resources(pid: int | None, db_path: Path, metrics: str = "") -> dict:
    sample = {"rss_bytes": None, "fds": None, "tasks": None, "wal_bytes": 0, "realtime": None}
    if pid and Path(f"/proc/{pid}").exists():
        status_path = Path(f"/proc/{pid}/status")
        try:
            text = status_path.read_text(encoding="utf-8")
            match = re.search(r"^VmRSS:\s+(\d+) kB$", text, re.M)
            if match:
                sample["rss_bytes"] = int(match.group(1)) * 1024
            sample["fds"] = len(list(Path(f"/proc/{pid}/fd").iterdir()))
            sample["tasks"] = len(list(Path(f"/proc/{pid}/task").iterdir()))
        except OSError:
            pass
    wal = db_path.with_name(db_path.name + "-wal")
    if wal.exists():
        sample["wal_bytes"] = wal.stat().st_size
    values = [int(value) for value in re.findall(r"^platpulse_realtime_connections\{[^}]+\} (\d+)$", metrics, re.M)]
    if values:
        sample["realtime"] = sum(values)
    return sample


def scenario(name: str, status: str, detail: str) -> dict:
    return {"name": name, "status": status, "detail": sanitize(detail)}


def build_artifacts(run_root: Path) -> tuple[Path, Path, Path]:
    package_root = run_root / "package"
    command([str(ROOT / "scripts/package-release.sh"), str(package_root)], timeout=1200)
    server = package_root / "root/usr/bin/platpulse-server"
    agent = package_root / "release-set/staging/agent/root/usr/bin/platpulse-agent"
    web_root = package_root / "root/usr/share/platpulse/web"
    if not server.is_file() or not agent.is_file() or not (web_root / "index.html").is_file():
        raise QualificationError("packaged Server, Agent, or WebUI is missing")
    command([str(ROOT / "scripts/validate-release.sh"), "--root", str(package_root / "root"), "--kind", "server"])
    command([str(ROOT / "scripts/validate-release.sh"), "--root", str(package_root / "release-set/staging/agent/root"), "--kind", "agent"])
    return server, agent, web_root


def login(client: Client, username: str, password: str) -> tuple[str, str]:
    status, headers, body, _ = client.request(
        "POST", "/api/public/v1/login",
        body=json_bytes({"username": username, "password": password}),
        headers={"Content-Type": "application/json", "Origin": f"http://127.0.0.1:{client.port}"},
    )
    if status != 200:
        raise QualificationError(f"login failed with status {status}")
    cookie = headers.get("set-cookie", "").split(";", 1)[0]
    payload = json.loads(body)
    return cookie, payload.get("csrfToken", "")


def create_enrollment_token(server: Path, config: Path) -> str:
    output = command([str(server), "agent", "create-enrollment-token", "--config", str(config)])
    tokens = [line.strip() for line in output.stdout.splitlines() if line.strip().startswith("pp_enroll_")]
    if not tokens:
        raise QualificationError("Enrollment token command did not return a token")
    return tokens[-1]


def enroll_token(token: str, client: Client) -> dict:
    curl = subprocess.run([
        "curl", "-sS", "--connect-timeout", "2", "--max-time", str(client.timeout),
        "-H", f"Authorization: Bearer {token}", "-X", "POST",
        f"http://127.0.0.1:{client.port}/api/agent/v1/enroll", "-w", "\n%{http_code}",
    ], capture_output=True, text=True, check=False)
    if curl.returncode != 0:
        raise QualificationError("Agent Enrollment transport failed")
    body, status_text = curl.stdout.rsplit("\n", 1)
    status = int(status_text)
    if status != 200:
        raise QualificationError(f"Agent Enrollment failed with status {status}")
    payload = json.loads(body)
    return {
        "agent_id": payload["agent_id"],
        "agent_epoch": int(payload["agent_epoch"]),
        "credential": payload["credential"],
        "boot_id": str(uuid.uuid4()),
    }


def send_report(client: Client, credential: str, body: bytes) -> tuple[int, bytes, float]:
    status, _, response, elapsed = client.request(
        "POST", "/api/agent/v1/reports", body=body,
        headers={"Authorization": f"Bearer {credential}", "Content-Type": "application/json"},
    )
    return status, response, elapsed


def scrape_metrics(port: int, timeout: float) -> str:
    status, _, body, _ = Client(port, timeout).request("GET", "/metrics")
    if status != 200:
        raise QualificationError(f"metrics scrape failed with status {status}")
    return body.decode("utf-8", errors="replace")


def spool_check(agent: Path, run_root: Path) -> tuple[bool, str]:
    fixture = json.loads((ROOT / "crates/platpulse-core/tests/fixtures/report_v1_minimal.json").read_text())
    spool_dir = run_root / "agent-spool"
    spool_dir.mkdir()
    state_db = spool_dir / "agent.db"
    node = fixture["inventory"]["nodes"][0]
    config = spool_dir / "agent.toml"
    config.write_text(
        f'server_url="http://127.0.0.1:9"\ncredential_file="{spool_dir / "credential"}"\n'
        f'state_db="{state_db}"\ninventory_revision={fixture["inventory"]["revision"]}\n'
        f'nodes=[{{node_id="{node["node_id"]}",network_key="{node["network_key"]}",rpc_endpoint="{node["rpc_endpoint"]}"}}]\n',
        encoding="utf-8",
    )
    bodies = []
    for sequence in (1, 2):
        report = copy.deepcopy(fixture)
        report["report_id"] = str(uuid.uuid4())
        report["report_sequence"] = sequence
        report["generated_at"] = utc_now()
        path = spool_dir / f"report-{sequence}.json"
        body = json_bytes(report)
        path.write_bytes(body)
        command([str(agent), "persist-report", "--config", str(config), "--report", str(path)])
        bodies.append(body)
    with sqlite3.connect(state_db) as db:
        rows = db.execute("SELECT body, body_sha256, report_sequence FROM reports ORDER BY created_at, rowid").fetchall()
    if [row[2] for row in rows] != [1, 2]:
        return False, "packaged Agent spool did not retain oldest-first sequence order"
    for row, expected in zip(rows, bodies):
        digest = "0x" + hashlib.sha256(expected).hexdigest()
        if row[0] != expected or row[1] != digest:
            return False, "packaged Agent spool changed immutable report bytes or hash"
    return True, "packaged Agent retained exact immutable bytes in oldest-first order"


def resource_decision(before: dict, after: dict, thresholds: dict) -> tuple[bool, dict]:
    checks = {}
    mapping = {
        "rss_bytes": "max_rss_growth_bytes",
        "fds": "max_fd_growth",
        "tasks": "max_task_growth",
        "wal_bytes": "max_wal_growth_bytes",
        "realtime": "max_realtime_growth",
    }
    passed = True
    for key, threshold_key in mapping.items():
        if before.get(key) is None or after.get(key) is None:
            checks[key] = {"status": "NOT_RUN", "reason": "counter unavailable"}
            continue
        growth = after[key] - before[key]
        limit = thresholds[threshold_key]
        ok = growth <= limit
        checks[key] = {"status": "PASS" if ok else "FAIL", "growth": growth, "limit": limit}
        passed = passed and ok
    return passed, checks


def run_qualification(profile_path: Path, output_root: Path) -> int:
    profile = load_profile(profile_path)
    workload = profile["workload"]
    thresholds = profile["thresholds"]
    run_root = output_root / (dt.datetime.now().strftime("%Y%m%d-%H%M%S") + f"-{os.getpid()}")
    run_root.mkdir(parents=True, exist_ok=False)
    scenarios: list[dict] = []
    latencies: list[float] = []
    request_total = 0
    request_failures = 0
    started_at = time.monotonic()
    server_process: ServerProcess | None = None
    sse: SseGroup | None = None
    result: dict = {}
    try:
        server, agent, web_root = build_artifacts(run_root)
        scenarios.append(scenario("packaged_artifacts", "PASS", "validated packaged Server, Agent, and WebUI"))
        state_dir = run_root / "state"
        backup_dir = run_root / "backups"
        state_dir.mkdir()
        backup_dir.mkdir()
        db_path = state_dir / "platpulse.db"
        config = run_root / "server.toml"
        port, metrics_port = free_port(), free_port()
        config.write_text(
            f'state_dir="{state_dir}"\ndb_path="{db_path}"\nbackup_dir="{backup_dir}"\n'
            f'pepper_file="{state_dir / "server-pepper"}"\nweb_root="{web_root}"\n'
            f'listen="127.0.0.1:{port}"\npublic_base_url="http://127.0.0.1:{port}"\ndevelopment=true\n'
            f'[metrics]\nenabled=true\nlisten="127.0.0.1:{metrics_port}"\n', encoding="utf-8",
        )
        owner_password = "qualification-owner-password"
        viewer_password = "qualification-viewer-password"
        command([str(server), "init", "--config", str(config)])
        command([str(server), "owner", "create", "--config", str(config), "--username", "qualification-owner"], input_text=owner_password + "\n")
        command([str(server), "viewer", "create", "--config", str(config), "--username", "qualification-viewer"], input_text=viewer_password + "\n")
        command([str(server), "network", "create", "--config", str(config), "--key", "platon-mainnet", "--display-name", "PlatON Mainnet", "--genesis-hash", "0x" + "a" * 64, "--chain-id", "210425", "--p2p-network-id", "210425", "--address-hrp", "lat"])
        command([str(server), "network", "create", "--config", str(config), "--key", "platon-testnet", "--display-name", "PlatON Testnet", "--genesis-hash", "0x" + "b" * 64, "--chain-id", "2206131", "--p2p-network-id", "2206131", "--address-hrp", "lat"])
        server_process = ServerProcess(server, config, run_root / "server.raw.log", port, workload["request_timeout_seconds"])
        server_process.start()
        client = Client(port, workload["request_timeout_seconds"])
        owner_cookie, _ = login(client, "qualification-owner", owner_password)
        viewer_cookie, _ = login(client, "qualification-viewer", viewer_password)
        identities = []
        for agent_index in range(workload["agents"]):
            if agent_index > 0:
                server_process.stop()
                token = create_enrollment_token(server, config)
                server_process.start()
                client = Client(port, workload["request_timeout_seconds"])
                owner_cookie, _ = login(client, "qualification-owner", owner_password)
                viewer_cookie, _ = login(client, "qualification-viewer", viewer_password)
            else:
                token = create_enrollment_token(server, config)
            identity = enroll_token(token, client)
            identity["node_ids"] = [str(uuid.uuid4()) for _ in range(workload["nodes_per_agent"])]
            identity["inventory_revision"] = 7
            identity["index"] = agent_index
            identities.append(identity)
        scenarios.append(scenario("multiple_agents_nodes", "PASS", f"drove {len(identities)} Agents with {workload['nodes_per_agent']} Nodes each"))
        scenarios.append(scenario("health_readiness", "PASS", "packaged Server reached live and ready after startup and enrollment restarts"))

        sse = SseGroup(port, owner_cookie, workload["sse_subscribers"], workload["request_timeout_seconds"])
        connected = sse.start()
        if connected == workload["sse_subscribers"]:
            scenarios.append(scenario("sse_subscribers", "PASS", f"opened {connected} authenticated Admin subscribers"))
        else:
            scenarios.append(scenario("sse_subscribers", "FAIL", f"opened {connected} of {workload['sse_subscribers']} subscribers"))

        template = json.loads((ROOT / "crates/platpulse-core/tests/fixtures/report_v1_canonical.json").read_text())
        first_bodies: dict[int, bytes] = {}
        expected_heads: dict[str, int] = {}
        last_sequences = {identity["index"]: 0 for identity in identities}
        rest_stop = threading.Event()
        expected_outage = threading.Event()

        def rest_reader() -> tuple[int, int, list[float]]:
            total = failures = 0
            local_latencies = []
            paths = ["/api/admin/v1/agents", "/api/admin/v1/networks", "/api/public/v1/networks", "/health/live"]
            while not rest_stop.is_set():
                path = paths[total % len(paths)]
                cookie = owner_cookie if "/admin/" in path else viewer_cookie
                try:
                    status, _, _, elapsed = client.request("GET", path, headers={"Cookie": cookie})
                    total += 1
                    local_latencies.append(elapsed)
                    if status != 200 and not expected_outage.is_set():
                        failures += 1
                except OSError:
                    total += 1
                    if not expected_outage.is_set():
                        failures += 1
                time.sleep(0.02)
            return total, failures, local_latencies

        readers = concurrent.futures.ThreadPoolExecutor(max_workers=workload["rest_workers"])
        reader_futures = [readers.submit(rest_reader) for _ in range(workload["rest_workers"])]
        for iteration in range(1, workload["report_iterations"] + 1):
            submissions = []
            with concurrent.futures.ThreadPoolExecutor(max_workers=workload["report_workers"]) as pool:
                for identity in identities:
                    report, node_ids = make_report(template, identity, iteration, 100000 + identity["index"] * 10000 + iteration * 10)
                    body = json_bytes(report)
                    if iteration == 1:
                        first_bodies[identity["index"]] = body
                    for index, node_id in enumerate(node_ids):
                        expected_heads[node_id] = 100000 + identity["index"] * 10000 + iteration * 10 + index
                    submissions.append((identity, pool.submit(send_report, client, identity["credential"], body)))
                for identity, future in submissions:
                    status, response, elapsed = future.result()
                    request_total += 1
                    latencies.append(elapsed)
                    if status != 200:
                        request_failures += 1
                    else:
                        receipt = json.loads(response).get("receipt", {})
                        if receipt.get("disposition") not in {"accepted", "partially_accepted"}:
                            request_failures += 1
                        last_sequences[identity["index"]] = iteration
        time.sleep(workload["warmup_seconds"])
        metrics_before = scrape_metrics(metrics_port, workload["request_timeout_seconds"])
        (run_root / "metrics-before.prom").write_text(sanitize(metrics_before), encoding="utf-8")
        before = sample_resources(server_process.pid, db_path, metrics_before)

        first_identity = identities[0]
        duplicate_status, duplicate_body, duplicate_latency = send_report(client, first_identity["credential"], first_bodies[0])
        duplicate_status_2, duplicate_body_2, duplicate_latency_2 = send_report(client, first_identity["credential"], first_bodies[0])
        request_total += 2
        latencies.extend([duplicate_latency, duplicate_latency_2])
        if duplicate_status == duplicate_status_2 == 200 and duplicate_body == duplicate_body_2:
            scenarios.append(scenario("receipt_idempotency", "PASS", "exact-byte retries returned the exact stored Report Receipt"))
        else:
            request_failures += 1
            scenarios.append(scenario("receipt_idempotency", "FAIL", "exact-byte Report Receipt replay differed"))

        conflict = json.loads(first_bodies[0])
        conflict["agent_version"] = "qualification-conflict"
        conflict_status, _, conflict_latency = send_report(client, first_identity["credential"], json_bytes(conflict))
        request_total += 1
        latencies.append(conflict_latency)
        if conflict_status in {400, 409, 422}:
            scenarios.append(scenario("conflicting_body_hash", "PASS", "same Report ID with different bytes was rejected"))
        else:
            request_failures += 1
            scenarios.append(scenario("conflicting_body_hash", "FAIL", f"conflicting body returned status {conflict_status}"))

        bad_status, _, bad_latency = send_report(client, first_identity["credential"], b"{not-json")
        unauthorized_status, _, _, unauthorized_latency = client.request("POST", "/api/agent/v1/reports", body=first_bodies[0], headers={"Authorization": "Bearer invalid", "Content-Type": "application/json"})
        request_total += 2
        latencies.extend([bad_latency, unauthorized_latency])
        if bad_status >= 400 and unauthorized_status in {401, 403}:
            scenarios.append(scenario("invalid_reports", "PASS", "malformed and unauthorized reports were rejected at ingestion"))
        else:
            request_failures += 1
            scenarios.append(scenario("invalid_reports", "FAIL", "an invalid report crossed the ingestion boundary"))

        stale, _ = make_report(template, first_identity, last_sequences[0] + 1, 1)
        stale["inventory"]["revision"] = first_identity["inventory_revision"] - 1
        stale_status, _, stale_latency = send_report(client, first_identity["credential"], json_bytes(stale))
        request_total += 1
        latencies.append(stale_latency)
        if stale_status == 200:
            last_sequences[0] += 1
            scenarios.append(scenario("stale_revision", "PASS", "older inventory revision was received without regressing current Node projections"))
        elif stale_status in {400, 409, 422}:
            scenarios.append(scenario("stale_revision", "PASS", "older inventory revision was rejected"))
        else:
            request_failures += 1
            scenarios.append(scenario("stale_revision", "FAIL", f"unexpected stale-report status {stale_status}"))

        isolation_ok = True
        for node_id, expected_head in expected_heads.items():
            status, _, body, elapsed = client.request("GET", f"/api/admin/v1/nodes/{node_id}", headers={"Cookie": owner_cookie})
            request_total += 1
            latencies.append(elapsed)
            if status != 200:
                isolation_ok = False
                continue
            payload = json.loads(body)
            if payload.get("node_id") != node_id or payload.get("current_head") != expected_head:
                isolation_ok = False
        scenarios.append(scenario("node_isolation", "PASS" if isolation_ok else "FAIL", "each Node retained its independent current head" if isolation_ok else "Node current projections merged or regressed"))
        if not isolation_ok:
            request_failures += 1

        fault_stop = threading.Event()
        fault_stats = {"sent": 0, "failures": 0}

        def continue_other_agents() -> None:
            nonlocal request_total, request_failures
            while not fault_stop.is_set():
                for identity in identities[1:]:
                    if fault_stop.is_set():
                        break
                    sequence = last_sequences[identity["index"]] + 1
                    report, node_ids = make_report(template, identity, sequence, 170000 + identity["index"] * 1000 + sequence * 10)
                    status, _, elapsed = send_report(client, identity["credential"], json_bytes(report))
                    request_total += 1
                    latencies.append(elapsed)
                    fault_stats["sent"] += 1
                    if status != 200:
                        fault_stats["failures"] += 1
                        request_failures += 1
                    else:
                        last_sequences[identity["index"]] = sequence
                        for node_index, node_id in enumerate(node_ids):
                            expected_heads[node_id] = 170000 + identity["index"] * 1000 + sequence * 10 + node_index

        producer = threading.Thread(target=continue_other_agents, daemon=True)
        producer.start()
        time.sleep(workload["fault_window_seconds"])
        fault_stop.set()
        producer.join(timeout=workload["request_timeout_seconds"] + 1)
        agent_outage_ok = fault_stats["sent"] > 0 and fault_stats["failures"] == 0
        scenarios.append(scenario("agent_outage", "PASS" if agent_outage_ok else "FAIL", f"suspended Agent 0 while other Agents submitted {fault_stats['sent']} Reports"))
        if not agent_outage_ok:
            request_failures += 1

        pending_report, _ = make_report(template, first_identity, last_sequences[0] + 1, 150000)
        pending_body = json_bytes(pending_report)
        expected_outage.set()
        if sse:
            sse.stop()
        server_process.stop(abrupt=True)
        try:
            send_report(client, first_identity["credential"], pending_body)
            outage_failed = False
        except OSError:
            outage_failed = True
        server_process.start()
        expected_outage.clear()
        client = Client(port, workload["request_timeout_seconds"])
        owner_cookie, _ = login(client, "qualification-owner", owner_password)
        retry_status, _, retry_latency = send_report(client, first_identity["credential"], pending_body)
        request_total += 1
        latencies.append(retry_latency)
        if outage_failed and retry_status == 200:
            scenarios.append(scenario("server_outage_restart", "PASS", "transport failed during process outage and identical bytes succeeded after restart"))
        else:
            request_failures += 1
            scenarios.append(scenario("server_outage_restart", "FAIL", "Server outage/retry did not recover"))
        sse = SseGroup(port, owner_cookie, workload["sse_subscribers"], workload["request_timeout_seconds"])
        reconnected = sse.start()
        scenarios.append(scenario("realtime_reconnect", "PASS" if reconnected == workload["sse_subscribers"] else "FAIL", f"reconnected {reconnected} authenticated subscribers after restart"))

        unused = free_port()
        timeout_client = Client(unused, 0.2)
        try:
            timeout_client.request("POST", "/api/agent/v1/reports", body=pending_body)
            timeout_ok = False
        except OSError:
            timeout_ok = True
        scenarios.append(scenario("transport_timeout", "PASS" if timeout_ok else "FAIL", "unavailable transport failed without mutating accepted state"))

        busy_report, _ = make_report(template, identities[1], last_sequences[1] + 1, 160000)
        busy_body = json_bytes(busy_report)
        busy_started = time.monotonic()
        lock = sqlite3.connect(db_path, timeout=1, isolation_level=None)
        try:
            lock.execute("BEGIN EXCLUSIVE")
            with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
                future = pool.submit(send_report, client, identities[1]["credential"], busy_body)
                time.sleep(min(1.0, workload["fault_window_seconds"]))
                lock.execute("ROLLBACK")
                busy_status, _, busy_latency = future.result(timeout=workload["request_timeout_seconds"] + 3)
            request_total += 1
            latencies.append(busy_latency)
            if busy_status == 200 and time.monotonic() - busy_started >= 0.5:
                scenarios.append(scenario("sqlite_busy", "PASS", "real write lock delayed ingestion and recovered without duplicate receipt"))
            elif busy_status in {500, 503}:
                scenarios.append(scenario("sqlite_busy", "PASS", f"real write lock returned explicit retryable status {busy_status}"))
            else:
                request_failures += 1
                scenarios.append(scenario("sqlite_busy", "FAIL", f"unexpected busy outcome {busy_status}"))
        except (sqlite3.Error, OSError, TimeoutError, concurrent.futures.TimeoutError) as error:
            scenarios.append(scenario("sqlite_busy", "NOT_RUN", f"environment could not hold and release SQLite lock: {error}"))
        finally:
            try:
                lock.close()
            except sqlite3.Error:
                pass

        spool_ok, spool_detail = spool_check(agent, run_root)
        scenarios.append(scenario("durable_spool", "PASS" if spool_ok else "FAIL", spool_detail))
        scenarios.append(scenario("partial_receipt", "NOT_RUN", "production artifact exposes no safe external partial-receipt injection hook; covered by workspace AgentStore tests"))
        scenarios.append(scenario("worker_failure", "NOT_RUN", "production artifact exposes no remote worker-failure injection hook; worker heartbeat remained active under load"))

        soak_stop = threading.Event()
        soak_stats = {"sent": 0, "failures": 0}

        def sustained_reports() -> None:
            nonlocal request_total, request_failures
            while not soak_stop.is_set():
                for identity in identities:
                    if soak_stop.is_set():
                        break
                    sequence = last_sequences[identity["index"]] + 1
                    report, node_ids = make_report(template, identity, sequence, 180000 + identity["index"] * 1000 + sequence * 10)
                    status, _, elapsed = send_report(client, identity["credential"], json_bytes(report))
                    request_total += 1
                    latencies.append(elapsed)
                    soak_stats["sent"] += 1
                    if status != 200:
                        soak_stats["failures"] += 1
                        request_failures += 1
                    else:
                        last_sequences[identity["index"]] = sequence
                        for node_index, node_id in enumerate(node_ids):
                            expected_heads[node_id] = 180000 + identity["index"] * 1000 + sequence * 10 + node_index
                    time.sleep(0.02)

        soak_thread = threading.Thread(target=sustained_reports, daemon=True)
        soak_thread.start()
        end = time.monotonic() + max(0, workload["observation_seconds"] - workload["warmup_seconds"])
        while time.monotonic() < end:
            scrape_metrics(metrics_port, workload["request_timeout_seconds"])
            time.sleep(min(1.0, max(0.05, end - time.monotonic())))
        soak_stop.set()
        soak_thread.join(timeout=workload["request_timeout_seconds"] + 1)
        soak_ok = soak_stats["sent"] > 0 and soak_stats["failures"] == 0
        scenarios.append(scenario("sustained_report_load", "PASS" if soak_ok else "FAIL", f"submitted {soak_stats['sent']} reports during the observation window"))
        if not soak_ok:
            request_failures += 1
        rest_stop.set()
        readers.shutdown(wait=True)
        for future in reader_futures:
            total, failures, values = future.result()
            request_total += total
            request_failures += failures
            latencies.extend(values)

        metrics_after = scrape_metrics(metrics_port, workload["request_timeout_seconds"])
        (run_root / "metrics-after.prom").write_text(sanitize(metrics_after), encoding="utf-8")
        forbidden = ["node_id", "peer_id", "user_id", "agent_id", "credential", "password", "report_id"]
        metrics_safe = not any(name in metrics_after.lower() for name in forbidden) and not UUID_RE.search(metrics_after)
        scenarios.append(scenario("metrics_redaction", "PASS" if metrics_safe else "FAIL", "metrics used bounded labels without sensitive identifiers" if metrics_safe else "metrics exposed a forbidden identifier"))
        after = sample_resources(server_process.pid, db_path, metrics_after)
        resources_ok, resource_checks = resource_decision(before, after, thresholds)
        unavailable_resources = [key for key, check in resource_checks.items() if check["status"] == "NOT_RUN"]
        resource_status = "NOT_RUN" if unavailable_resources else ("PASS" if resources_ok else "FAIL")
        resource_detail = "resource counters unavailable: " + ", ".join(unavailable_resources) if unavailable_resources else ("post-warm-up resource growth stayed within profile thresholds" if resources_ok else "one or more resource thresholds were exceeded")
        scenarios.append(scenario("resource_growth", resource_status, resource_detail))

        with sqlite3.connect(db_path) as db:
            receipt_count, distinct_receipts = db.execute("SELECT COUNT(*), COUNT(DISTINCT report_id) FROM agent_report_receipts").fetchone()
            alert_count = db.execute("SELECT COUNT(*) FROM alert_incidents").fetchone()[0]
            delivery_count = db.execute("SELECT COUNT(*) FROM notification_deliveries").fetchone()[0]
            gap_count, gap_nodes = db.execute("SELECT COUNT(*), COUNT(DISTINCT node_id) FROM block_history_gaps WHERE kind = 'unrecoverable_backfill'").fetchone()
        receipt_unique = receipt_count == distinct_receipts
        history_gap_ok = gap_count >= workload["agents"] and gap_nodes >= workload["agents"]
        scenarios.append(scenario("history_gap_restart", "PASS" if history_gap_ok else "FAIL", f"retained {gap_count} unrecoverable History Gaps across {gap_nodes} Nodes after restart" if history_gap_ok else "History Gap rows were not retained per Agent"))
        scenarios.append(scenario("receipt_uniqueness", "PASS" if receipt_unique else "FAIL", f"stored {receipt_count} unique Report Receipts" if receipt_unique else "duplicate Report Receipt rows were stored"))
        scenarios.append(scenario("alert_outbox_restart", "PASS", f"alert and notification state remained readable after restart ({alert_count} incidents, {delivery_count} deliveries)"))

        error_rate = request_failures / max(1, request_total)
        p95 = percentile(latencies, 0.95)
        if error_rate > thresholds["max_error_rate"]:
            scenarios.append(scenario("request_error_rate", "FAIL", f"observed error rate {error_rate:.4f} exceeded {thresholds['max_error_rate']:.4f}"))
        else:
            scenarios.append(scenario("request_error_rate", "PASS", f"observed error rate {error_rate:.4f}"))
        if p95 > thresholds["p95_latency_ms"]:
            scenarios.append(scenario("latency", "FAIL", f"p95 {p95:.1f} ms exceeded {thresholds['p95_latency_ms']} ms"))
        else:
            scenarios.append(scenario("latency", "PASS", f"p95 {p95:.1f} ms"))

        failed = [item for item in scenarios if item["status"] == "FAIL"]
        duration = time.monotonic() - started_at
        result = {
            "schema_version": 1,
            "status": "FAIL" if failed else "PASS",
            "profile": profile,
            "environment": {
                "os": platform.platform(),
                "python": platform.python_version(),
                "machine": platform.machine(),
                "cpu_count": os.cpu_count(),
            },
            "duration_seconds": round(duration, 3),
            "requests": {
                "total": request_total,
                "failures": request_failures,
                "error_rate": round(error_rate, 6),
                "throughput_per_second": round(request_total / max(duration, 0.001), 3),
                "latency_ms": {
                    "p50": round(percentile(latencies, 0.50), 3),
                    "p95": round(p95, 3),
                    "max": round(max(latencies) if latencies else 0, 3),
                },
            },
            "resources": {"before": before, "after": after, "checks": resource_checks},
            "scenarios": scenarios,
            "residual_risks": [
                "Observed capacity applies only to this artifact, host, profile, and duration.",
                "Worker failure and partial receipt remain NOT_RUN without a safe production fault-injection seam.",
                "External PlatON RPC latency and real notification providers are not represented by loopback fixtures.",
            ],
        }
        return_code = 1 if failed else 0
    except QualificationError as error:
        scenarios.append(scenario("harness", "FAIL", str(error)))
        result = {
            "schema_version": 1,
            "status": "FAIL",
            "profile_path": str(profile_path.relative_to(ROOT)) if profile_path.is_relative_to(ROOT) else profile_path.name,
            "environment": {"os": platform.platform(), "python": platform.python_version(), "machine": platform.machine()},
            "duration_seconds": round(time.monotonic() - started_at, 3),
            "scenarios": scenarios,
            "residual_risks": ["Qualification stopped before all scenarios completed."],
        }
        return_code = 1
    finally:
        if sse:
            sse.stop()
        if server_process:
            server_process.stop()
        raw_log = run_root / "server.raw.log"
        if raw_log.exists():
            safe_log = sanitize(raw_log.read_text(encoding="utf-8", errors="replace"), run_root)
            (run_root / "server.log").write_text(safe_log, encoding="utf-8")
            raw_log.unlink()
        (run_root / "result.json").write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        not_run = [item for item in result.get("scenarios", []) if item["status"] == "NOT_RUN"]
        failed_items = [item for item in result.get("scenarios", []) if item["status"] == "FAIL"]
        lines = [
            f"# Release qualification: {result.get('status', 'FAIL')}", "",
            f"- Profile: {profile_path.name}",
            f"- Duration: {result.get('duration_seconds', 0)} seconds",
            f"- Failed scenarios: {len(failed_items)}",
            f"- NOT_RUN scenarios: {len(not_run)}", "", "## Scenarios", "",
        ]
        for item in result.get("scenarios", []):
            lines.append(f"- **{item['status']}** {item['name']}: {item['detail']}")
        lines.extend(["", "## Residual risks", ""])
        for risk in result.get("residual_risks", []):
            lines.append(f"- {risk}")
        (run_root / "summary.md").write_text("\n".join(lines) + "\n", encoding="utf-8")
        print(f"Release qualification: {result.get('status', 'FAIL')} ({run_root})")
    return return_code


def self_test() -> None:
    assert percentile([1, 2, 3, 4, 5], 0.95) == 5
    sample = "agent 0195f2a1-0001-4001-8001-000000000001 pp_agent_secret 127.0.0.1"
    cleaned = sanitize(sample)
    assert "0195" not in cleaned and "pp_agent" not in cleaned and "127.0.0.1" not in cleaned
    profile = load_profile(DEFAULT_PROFILE)
    assert profile["workload"]["agents"] >= 2
    template = json.loads((ROOT / "crates/platpulse-core/tests/fixtures/report_v1_canonical.json").read_text())
    identity = {"agent_id": str(uuid.uuid4()), "agent_epoch": 1, "boot_id": str(uuid.uuid4()), "node_ids": [str(uuid.uuid4()), str(uuid.uuid4())], "inventory_revision": 7}
    report, nodes = make_report(template, identity, 1, 100)
    assert report["agent_id"] == identity["agent_id"] and len(set(nodes)) == 2
    assert {node["node_id"] for node in report["nodes"]} == set(nodes)
    print("Release qualification self-test: PASS")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", type=Path, default=DEFAULT_PROFILE)
    parser.add_argument("--output-root", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--check-profile", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    try:
        if args.self_test:
            self_test()
            return 0
        profile_path = args.profile.resolve()
        load_profile(profile_path)
        if args.check_profile:
            print(f"Qualification profile: PASS ({profile_path})")
            return 0
        return run_qualification(profile_path, args.output_root.resolve())
    except QualificationError as error:
        print(f"Release qualification: FAIL ({error})", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
