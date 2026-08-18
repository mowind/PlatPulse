#!/usr/bin/env python3
"""Final Phase 5 release qualification job for packaged PlatPulse releases.

Runs the complete hardening evidence set against the release candidate:
source quality gates (Rust, dependency policy, WebUI, generated artifacts),
the fixed Playwright viewport matrix, package builds and validation, the
release-candidate harness (native TLS, metrics, security matrix), the
migration/backup/restore rehearsal, and the load/fault/soak qualification.

Every check records an explicit status:
  PASS        - the check ran and passed.
  FAIL        - the check ran and failed (release-blocking).
  UNAVAILABLE - the check could not run in this environment (e.g. missing tool).
  NOT_RUN     - the check was intentionally skipped for this invocation.

UNAVAILABLE/NOT_RUN are never reported as a pass. The final report lists them
explicitly, and --require-all turns them into a failure for release gating.
"""
from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import tempfile
import time
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_PROFILE = ROOT / "release/qualification/ci.toml"
DEFAULT_OUTPUT = ROOT / "target/release-qualification/final"
UUID_RE = re.compile(r"[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}", re.I)
TOKEN_RE = re.compile(r"pp_(?:agent|enroll)_[A-Za-z0-9_-]+")
IP_RE = re.compile(r"(?<![0-9])(?:[0-9]{1,3}\.){3}[0-9]{1,3}(?![0-9])")
PRIVATE_KEY_RE = re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----")
FORBIDDEN_PATH_MARKERS = ("geolite", "pepper", "credential", "private-key", "privkey", "token")
FORBIDDEN_SUFFIXES = {".mmdb", ".pem", ".key", ".p12", ".pfx", ".jks", ".p8", ".crt", ".csr"}


class QualificationError(RuntimeError):
    pass


def utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def sanitize(text: str, run_root: Path | None = None) -> str:
    result = UUID_RE.sub("<uuid>", text)
    result = TOKEN_RE.sub("<secret>", result)
    result = IP_RE.sub("<ip>", result)
    if run_root is not None:
        result = result.replace(str(run_root), "<run>")
    return result


def run_command(args: list[str], *, cwd: Path = ROOT, timeout: int = 300,
                env: dict | None = None, check: bool = True) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        args, cwd=cwd, env=env, text=True,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=timeout, check=False,
    )
    if check and result.returncode != 0:
        detail = sanitize((result.stderr or result.stdout).strip())[:400]
        raise QualificationError(f"command failed ({' '.join(args[:3])}): {detail}")
    return result


def workspace_version() -> str:
    output = run_command(["cargo", "metadata", "--no-deps", "--format-version", "1"]).stdout
    packages = json.loads(output)["packages"]
    return next(p["version"] for p in packages if p["name"] == "platpulse-server")


def rust_target() -> str:
    output = run_command(["rustc", "-vV"]).stdout
    return next(line.split(":", 1)[1].strip() for line in output.splitlines() if line.startswith("host:"))


def display_path(path: Path) -> str:
    try:
        return str(path.relative_to(ROOT))
    except ValueError:
        return str(path)


class Check:
    def __init__(self, name: str, phase: str, kind: str = "present"):
        self.name = name
        self.phase = phase
        self.kind = kind
        self.status = "PASS"
        self.detail = ""
        self.evidence: Path | None = None

    def to_dict(self) -> dict:
        return {
            "name": self.name,
            "phase": self.phase,
            "kind": self.kind,
            "status": self.status,
            "detail": self.detail,
            "evidence": str(self.evidence.relative_to(ROOT)) if self.evidence else None,
        }


class FinalQualification:
    def __init__(self, output: Path, profile: Path, require_all: bool):
        self.output = output
        self.profile = profile
        self.require_all = require_all
        self.checks: list[Check] = []
        self.residual_risks: list[str] = []
        self.security_dispositions: list[dict] = []
        self.version = "<unknown>"
        self.target = "<unknown>"

    def check(self, name: str, phase: str, kind: str = "present") -> Check:
        item = Check(name, phase, kind)
        self.checks.append(item)
        return item

    def save_log(self, check: Check, text: str) -> None:
        path = self.output / f"{check.name}.log"
        path.write_text(sanitize(text, self.output), encoding="utf-8")
        check.evidence = path

    def run_present(self, check: Check, args: list[str], *, cwd: Path = ROOT,
                    timeout: int = 300, env: dict | None = None) -> None:
        try:
            result = run_command(args, cwd=cwd, timeout=timeout, env=env, check=False)
            self.save_log(check, result.stdout + result.stderr)
            if result.returncode != 0:
                check.status = "FAIL"
                check.detail = f"exited {result.returncode}; see {check.name}.log"
            else:
                check.detail = "passed"
        except subprocess.TimeoutExpired:
            check.status = "FAIL"
            check.detail = f"timed out after {timeout}s"

    def run_unavailable(self, check: Check, tool: str, reason: str) -> None:
        check.status = "UNAVAILABLE"
        check.detail = f"{tool}: {reason}"

    # ------------------------------------------------------------------ gates

    def phase_rust(self) -> None:
        if shutil.which("cargo") is None:
            for name in ("rust-fmt", "rust-clippy", "rust-tests"):
                self.run_unavailable(self.check(name, "rust-gates"), "cargo", "not installed")
            return
        self.run_present(self.check("rust-fmt", "rust-gates"), ["cargo", "fmt", "--check"], timeout=300)
        self.run_present(self.check("rust-clippy", "rust-gates"),
                         ["cargo", "clippy", "--all-targets", "--all-features", "--", "-D", "warnings"], timeout=1800)
        self.run_present(self.check("rust-tests", "rust-gates"), ["cargo", "test", "--workspace"], timeout=2400)

    def phase_dependency_policy(self) -> None:
        if shutil.which("cargo-deny") is None:
            self.run_unavailable(self.check("cargo-deny", "dependency-policy"), "cargo-deny", "not installed")
        else:
            self.run_present(self.check("cargo-deny", "dependency-policy"), ["cargo", "deny", "check"], timeout=600)
        if shutil.which("cargo-audit") is None:
            self.run_unavailable(self.check("cargo-audit", "dependency-policy"), "cargo-audit", "not installed")
        else:
            self.run_present(self.check("cargo-audit", "dependency-policy"),
                             ["cargo", "audit", "--ignore", "RUSTSEC-2023-0071"], timeout=600)
        if shutil.which("npm") is None:
            self.run_unavailable(self.check("npm-audit", "dependency-policy"), "npm", "not installed")
        else:
            self.run_present(self.check("npm-audit", "dependency-policy"),
                             ["npm", "audit", "--audit-level=critical"], cwd=ROOT / "platpulse-web", timeout=600)

    def phase_web(self) -> None:
        web = ROOT / "platpulse-web"
        if shutil.which("npm") is None:
            for name in ("web-lint", "web-typecheck", "web-unit", "web-build"):
                self.run_unavailable(self.check(name, "web-gates"), "npm", "not installed")
            return
        self.run_present(self.check("web-lint", "web-gates"), ["npm", "run", "lint"], cwd=web, timeout=600)
        self.run_present(self.check("web-typecheck", "web-gates"), ["npm", "run", "typecheck"], cwd=web, timeout=600)
        self.run_present(self.check("web-unit", "web-gates"), ["npm", "test"], cwd=web, timeout=900)
        self.run_present(self.check("web-build", "web-gates"), ["npm", "run", "build"], cwd=web, timeout=900)

    def phase_freshness(self) -> None:
        spec = ROOT / "docs/openapi/openapi.json"
        with tempfile.TemporaryDirectory(prefix="platpulse-openapi-") as tmp:
            candidate = Path(tmp) / "openapi.json"
            try:
                result = run_command(
                    ["cargo", "run", "-p", "platpulse-server", "--quiet", "--", "--print-openapi"],
                    timeout=1800, check=False)
                candidate.write_text(result.stdout, encoding="utf-8")
                check = self.check("openapi-freshness", "generated-artifacts")
                if result.returncode != 0:
                    check.status = "FAIL"
                    check.detail = "Server refused to print the OpenAPI document"
                elif candidate.read_bytes() == spec.read_bytes():
                    check.detail = "committed spec matches the Server routes"
                else:
                    check.status = "FAIL"
                    check.detail = "docs/openapi/openapi.json is stale; regenerate and commit"
            except (OSError, subprocess.TimeoutExpired) as error:
                check = self.check("openapi-freshness", "generated-artifacts")
                check.status = "FAIL"
                check.detail = f"could not regenerate the OpenAPI document: {error}"
        web = ROOT / "platpulse-web"
        check = self.check("browser-client-freshness", "generated-artifacts")
        if shutil.which("npm") is None:
            self.run_unavailable(check, "npm", "not installed")
            return
        try:
            generated = web / "src/api/generated"
            with tempfile.TemporaryDirectory(prefix="platpulse-generated-client-") as tmp:
                snapshot = Path(tmp) / "generated"
                shutil.copytree(generated, snapshot)
                try:
                    run_command(["npm", "run", "generate:api"], cwd=web, timeout=600)
                    diff = run_command(
                        ["git", "status", "--porcelain", "--", "platpulse-web/src/api/generated"],
                        check=False)
                    self.save_log(check, diff.stdout + diff.stderr)
                    if (diff.stdout + diff.stderr).strip():
                        check.status = "FAIL"
                        check.detail = "generated browser client is stale; regenerate and commit"
                    else:
                        check.detail = "generated browser client matches the committed spec"
                finally:
                    shutil.rmtree(generated)
                    shutil.copytree(snapshot, generated)
        except (OSError, subprocess.TimeoutExpired) as error:
            check.status = "FAIL"
            check.detail = f"could not regenerate the browser client: {error}"

    def phase_playwright(self) -> None:
        check = self.check("playwright-fixed-viewports", "browser-matrix")
        web = ROOT / "platpulse-web"
        if shutil.which("npx") is None:
            self.run_unavailable(check, "npx", "not installed")
            return
        cache = Path(os.environ.get("PLAYWRIGHT_BROWSERS_PATH", str(Path.home() / ".cache/ms-playwright")))
        chromium_roots = list(cache.glob("chromium-*")) if cache.exists() else []
        chromium = [
            path for root in chromium_roots for path in root.rglob("*")
            if path.is_file() and path.name in {"chrome", "chrome-headless-shell"}
        ]
        if not chromium and not os.environ.get("PLAYWRIGHT_SKIP_BROWSER_CHECK"):
            self.run_unavailable(check, "Playwright chromium", "not installed (npx playwright install chromium)")
            return
        env = {**os.environ, "CI": "1"}
        self.run_present(check, ["npm", "run", "e2e"], cwd=web, timeout=1800, env=env)

    # ------------------------------------------------------------- artifacts

    def phase_package(self) -> None:
        check = self.check("package-build", "artifacts")
        package_root = self.output / "package"
        if shutil.which("cargo") is None or shutil.which("npm") is None:
            self.run_unavailable(check, "cargo/npm", "package build requires both toolchains")
            return
        try:
            result = run_command([str(ROOT / "scripts/package-release.sh"), str(package_root)], timeout=3600)
            self.save_log(check, result.stdout + result.stderr)
            check.detail = "packaged Server, Agent, and WebUI release set built"
        except (OSError, subprocess.TimeoutExpired, QualificationError) as error:
            check.status = "FAIL"
            check.detail = sanitize(str(error))[:400]
            return
        set_dir = package_root / "release-set"
        server_root = package_root / "root"
        agent_root = set_dir / "staging/agent/root"
        server_archives = list((ROOT / "target").glob("platpulse-server-*.tar.gz"))
        archive = max(server_archives, key=lambda path: path.stat().st_mtime) if server_archives else None
        agent_archive = next(set_dir.glob("platpulse-agent-*.tar.gz"), None)
        validate = self.check("package-validate", "artifacts")
        errors: list[str] = []
        for label, args in (
            ("server root", ["scripts/validate-release.sh", "--root", str(server_root), "--kind", "server"]),
            ("agent root", ["scripts/validate-release.sh", "--root", str(agent_root), "--kind", "agent"]),
        ):
            try:
                result = run_command(args, cwd=ROOT, timeout=300, check=False)
                if result.returncode != 0:
                    errors.append(f"{label}: {sanitize(result.stdout + result.stderr)[:200]}")
            except (OSError, subprocess.TimeoutExpired) as error:
                errors.append(f"{label}: {error}")
        if archive is not None:
            try:
                result = run_command(["scripts/validate-release.sh", "--archive", str(archive), "--kind", "server"],
                                     cwd=ROOT, timeout=300, check=False)
                if result.returncode != 0:
                    errors.append(f"server archive: {sanitize(result.stdout + result.stderr)[:200]}")
            except (OSError, subprocess.TimeoutExpired) as error:
                errors.append(f"server archive: {error}")
        else:
            errors.append("server archive: packaged Server archive is missing")
        if agent_archive is not None:
            try:
                result = run_command(["scripts/validate-release.sh", "--archive", str(agent_archive), "--kind", "agent"],
                                     cwd=ROOT, timeout=300, check=False)
                if result.returncode != 0:
                    errors.append(f"agent archive: {sanitize(result.stdout + result.stderr)[:200]}")
            except (OSError, subprocess.TimeoutExpired) as error:
                errors.append(f"agent archive: {error}")
        if errors:
            validate.status = "FAIL"
            validate.detail = "; ".join(errors)[:400]
        else:
            validate.detail = "validated packaged layout, modes, units, and allowed members"
        sums = self.check("package-checksums", "artifacts")
        sums_file = set_dir / "SHA256SUMS"
        if not sums_file.is_file():
            sums.status = "FAIL"
            sums.detail = "release set has no SHA256SUMS"
        else:
            try:
                result = run_command(["sha256sum", "-c", "SHA256SUMS"], cwd=set_dir, timeout=300, check=False)
                self.save_log(sums, result.stdout + result.stderr)
                if result.returncode != 0:
                    sums.status = "FAIL"
                    sums.detail = "SHA256SUMS verification failed"
                else:
                    sums.detail = "release set checksums verified"
            except (OSError, subprocess.TimeoutExpired) as error:
                sums.status = "FAIL"
                sums.detail = f"could not verify checksums: {error}"

    def phase_forbidden_scan(self) -> None:
        """Reject secrets and licensed GeoLite data inside packaged artifacts.

        Path-level checks are enforced by scripts/validate-release.sh; this adds
        a content-level scan. Documentation that legitimately states PlatPulse
        does not distribute GeoLite or MaxMind credentials is not a finding.
        """
        check = self.check("forbidden-secrets-and-geolite", "artifacts")
        package_root = self.output / "package"
        set_dir = package_root / "release-set"
        scan_roots = []
        for candidate in (package_root / "root", set_dir / "staging/agent/root", set_dir):
            if candidate.is_dir():
                scan_roots.append(candidate)
        if not scan_roots:
            self.run_unavailable(check, "package", "no packaged tree to scan")
            return
        findings: list[str] = []
        text_extensions = {".toml", ".md", ".yml", ".yaml", ".service", ".timer", ".sh", ".txt", ".json", ".example"}
        for root in scan_roots:
            for path in root.rglob("*"):
                if not path.is_file():
                    continue
                relative = str(path.relative_to(ROOT))
                lower_name = path.name.lower()
                if path.suffix.lower() in FORBIDDEN_SUFFIXES or any(marker in lower_name for marker in FORBIDDEN_PATH_MARKERS):
                    findings.append(f"{relative} is a forbidden secret/GeoLite member")
                if path.suffix.lower() in text_extensions and path.stat().st_size < 4 * 1024 * 1024:
                    try:
                        text = path.read_text(encoding="utf-8", errors="replace")
                    except OSError:
                        continue
                    if PRIVATE_KEY_RE.search(text):
                        findings.append(f"{relative} contains private-key material")
                    if TOKEN_RE.search(text):
                        findings.append(f"{relative} contains a live credential token")
        native_dir = ROOT / "target/release-artifacts"
        if native_dir.is_dir():
            for artifact in native_dir.iterdir():
                if artifact.suffix in {".deb", ".rpm", ".gz"} and artifact.is_file():
                    try:
                        payload = artifact.read_bytes()
                    except OSError as error:
                        findings.append(f"{artifact.name} could not be scanned: {error}")
                        continue
                    for marker in (b"pp_agent_", b"pp_enroll_", b"PRIVATE KEY", b".mmdb", b"GeoLite"):
                        if marker in payload:
                            findings.append(f"{artifact.name} contains forbidden marker {marker.decode(errors='replace')}")

        try:
            research = ROOT / "docs/research/geolite2-country-acquisition.md"
            if not research.is_file() or "Commercial Redistribution License" not in research.read_text(encoding="utf-8"):
                findings.append("no documented GeoLite licensing statement for operator-provided MMDB")
        except OSError:
            findings.append("could not read the GeoLite licensing research document")
        if findings:
            check.status = "FAIL"
            check.detail = "; ".join(dict.fromkeys(findings))[:400]
        else:
            check.detail = "no forbidden secrets or licensed GeoLite data in packaged artifacts"

    def phase_security_dispositions(self) -> None:
        check = self.check("security-matrix-dispositions", "security")
        path = ROOT / "release/qualification/security.toml"
        try:
            data = tomllib.loads(path.read_text(encoding="utf-8"))
            self.security_dispositions = [
                {
                    "id": item["id"],
                    "owner": item["owner"],
                    "blocking": item["blocking"],
                    "disposition": item["disposition"],
                    "evidence": item.get("evidence"),
                    "risk": item.get("risk"),
                }
                for item in data.get("criterion", [])
            ]
            blocking_failures = [
                item["id"] for item in self.security_dispositions
                if item["blocking"] and item["disposition"] != "pass"
            ]
            if blocking_failures:
                check.status = "FAIL"
                check.detail = "blocking security criteria are not pass: " + ", ".join(blocking_failures)
            else:
                check.detail = "all blocking security criteria are pass"
        except (OSError, tomllib.TOMLDecodeError, KeyError) as error:
            check.status = "FAIL"
            check.detail = f"security matrix could not be evaluated: {error}"

    def phase_native_package_checks(self) -> None:
        for name, command, reason in (
            ("packaging-policy", "scripts/test-release-packaging.sh", ""),
            ("native-package-inspection", "scripts/test-release-packages.sh", "native .deb/.rpm artifacts are produced by release CI"),
            ("docker-context-inspection", "scripts/test-docker-context.sh", "Docker is not installed"),
        ):
            check = self.check(name, "package-policy")
            if name == "native-package-inspection" and not (ROOT / "target/release-artifacts").is_dir():
                self.run_unavailable(check, command, reason)
                continue
            if name == "docker-context-inspection" and shutil.which("docker") is None:
                self.run_unavailable(check, "docker", reason)
                continue
            args = [str(ROOT / command)]
            if name == "native-package-inspection":
                args.append(str(ROOT / "target/release-artifacts"))
            result = run_command(args, timeout=1800, check=False)
            self.save_log(check, result.stdout + result.stderr)
            if result.returncode == 2:
                self.run_unavailable(check, command, "required package tooling is unavailable")
            elif result.returncode != 0:
                check.status = "FAIL"
                check.detail = f"exited {result.returncode}"
            else:
                check.detail = "passed"

    def phase_provenance(self) -> None:
        check = self.check("unsigned-supply-chain-claims", "artifacts")
        docs = [ROOT / "docs/deployment.md", ROOT / "docs/release-qualification.md",
                ROOT / "docs/security-review.md", ROOT / "README.md"]
        positive: list[str] = []
        disclaimer_ok = False
        for doc in docs:
            if not doc.is_file():
                continue
            for number, line in enumerate(doc.read_text(encoding="utf-8", errors="replace").splitlines(), 1):
                lower = line.lower()
                if "verified supply chain" in lower or "artifact sign" in lower:
                    if any(neg in lower for neg in ("not", "must not", "unsigned", "do not", "no signature")):
                        disclaimer_ok = True
                    else:
                        positive.append(f"{doc.name}:{number}")
        release_workflow = ROOT / ".github/workflows/release.yml"
        if release_workflow.is_file():
            for number, line in enumerate(release_workflow.read_text(encoding="utf-8").splitlines(), 1):
                lower = line.lower()
                if "verified supply chain" in lower:
                    if "not" in lower:
                        disclaimer_ok = True
                    else:
                        positive.append(f"release.yml:{number}")
        if positive:
            check.status = "FAIL"
            check.detail = "unsigned artifacts are described as a verified supply chain: " + ", ".join(positive)
        elif not disclaimer_ok:
            check.status = "FAIL"
            check.detail = "no explicit statement that unsigned artifacts are not a verified supply chain"
        else:
            check.detail = "unsigned artifacts are not described as a verified supply chain"
        sbom = self.check("sbom-evidence", "artifacts")
        package_root = self.output / "package"
        sbom_candidates = []
        for root in (package_root, ROOT / "target/release-artifacts"):
            if root.exists():
                sbom_candidates.extend(root.rglob("*.spdx.json"))
        if sbom_candidates:
            sbom.detail = f"SPDX SBOM recorded: {display_path(sbom_candidates[0])}"
        else:
            self.run_unavailable(sbom, "syft", "no SPDX SBOM was produced")

    # ---------------------------------------------------------- packaged runs

    def phase_recovery(self) -> None:
        check = self.check("recovery-rehearsal", "packaged")
        recovery = self.check("recovery-rehearsal-evidence", "packaged", "evidence")
        package = self.output / "package"
        recovery_out = self.output / "recovery-rehearsal"
        args = [str(ROOT / "scripts/release-recovery-rehearsal.sh"), "--output", str(recovery_out)]
        server_archives = list((ROOT / "target").glob("platpulse-server-*.tar.gz"))
        archive = max(server_archives, key=lambda path: path.stat().st_mtime) if server_archives else None
        if archive is None:
            check.status = "FAIL"
            check.detail = "packaged Server archive is missing; recovery cannot qualify a different build"
            recovery.status = "FAIL"
            recovery.detail = "packaged Server archive is missing"
            return
        extracted = self.output / "extracted-server"
        extracted.mkdir(exist_ok=True)
        extraction = run_command(["tar", "-xzf", str(archive), "-C", str(extracted)], timeout=300, check=False)
        server_binary = extracted / "usr/bin/platpulse-server"
        if extraction.returncode != 0 or not server_binary.is_file():
            check.status = "FAIL"
            check.detail = "could not extract the packaged Server archive for recovery"
            recovery.status = "FAIL"
            recovery.detail = "packaged Server binary is missing"
            return
        args += ["--server", str(server_binary), "--skip-package"]
        try:
            result = run_command(args, timeout=3600, check=False)
            json_path = recovery_out / "recovery-rehearsal.json"
            md_path = recovery_out / "recovery-rehearsal.md"
            self.save_log(check, result.stdout + result.stderr)
            if result.returncode != 0:
                check.status = "FAIL"
                check.detail = f"migration/backup/restore rehearsal exited {result.returncode}"
            else:
                check.detail = "migration, backup, and restore rehearsal passed"
            if json_path.exists():
                recovery.evidence = json_path
                recovery.detail = recovery_out.name
            if md_path.exists():
                check.evidence = md_path
            missing = [str(path.name) for path in (json_path, md_path) if not path.exists()]
            if missing:
                recovery.status = "FAIL"
                recovery.detail = "missing recovery evidence: " + ", ".join(missing)
                check.status = "FAIL"
                check.detail = "recovery rehearsal did not produce complete evidence"
        except (OSError, subprocess.TimeoutExpired) as error:
            check.status = "FAIL"
            check.detail = f"rehearsal could not run: {error}"

    def phase_harness(self) -> None:
        check = self.check("release-candidate-harness", "packaged")
        try:
            result = run_command([str(ROOT / "scripts/release-candidate-harness.sh")], timeout=3600, check=False)
            self.save_log(check, result.stdout + result.stderr)
            if result.returncode == 2:
                self.run_unavailable(check, "release candidate harness", "environment cannot run packaged harness")
            elif result.returncode != 0:
                check.status = "FAIL"
                check.detail = f"release-candidate harness exited {result.returncode}"
            else:
                check.detail = "native TLS, metrics, and security matrix passed (external boundary)"
        except (OSError, subprocess.TimeoutExpired) as error:
            check.status = "FAIL"
            check.detail = f"harness could not run: {error}"

    def phase_qualification(self) -> None:
        check = self.check("qualification-profile", "packaged")
        qual_out = self.output / "qualification"
        try:
            result = run_command([
                str(ROOT / "scripts/release-qualification.sh"),
                "--profile", str(self.profile),
                "--output-root", str(qual_out),
            ], timeout=5400, check=False)
            self.save_log(check, result.stdout + result.stderr)
            if result.returncode != 0:
                check.status = "FAIL"
                check.detail = f"qualification profile exited {result.returncode}"
            else:
                check.detail = f"qualification profile {self.profile.name} passed"
        except (OSError, subprocess.TimeoutExpired) as error:
            check.status = "FAIL"
            check.detail = f"qualification could not run: {error}"
        run_dirs = sorted(qual_out.glob("20*"), reverse=True) if qual_out.exists() else []
        result_json = run_dirs[0] / "result.json" if run_dirs else None
        if result_json is not None and result_json.exists():
                try:
                    data = json.loads(result_json.read_text(encoding="utf-8"))
                    not_run = [s["name"] for s in data.get("scenarios", []) if s["status"] == "NOT_RUN"]
                    self.residual_risks.extend(
                        f"qualification: {risk}" for risk in data.get("residual_risks", [])
                    )
                    detail = data.get("status", "?")
                    if detail == "FAIL":
                        check.status = "FAIL"
                    elif detail != "PASS":
                        check.status = "UNAVAILABLE"
                    evidence = self.check("qualification-result", "packaged", "evidence")
                    evidence.evidence = result_json
                    evidence.status = check.status
                    evidence.detail = f"{detail}; NOT_RUN scenarios: {', '.join(not_run)}" if not_run else f"{detail}; all scenarios present"
                except (OSError, json.JSONDecodeError) as error:
                    check.status = "FAIL"
                    check.detail = f"qualification result is unreadable: {error}"
        else:
            check.status = "FAIL"
            check.detail = "qualification result.json is missing"
            evidence = self.check("qualification-result", "packaged", "evidence")
            evidence.status = "FAIL"
            evidence.detail = "qualification result.json is missing"

    def collect_residual_risks(self) -> None:
        security = ROOT / "release/qualification/security.toml"
        if security.is_file():
            try:
                data = tomllib.loads(security.read_text(encoding="utf-8"))
                for criterion in data.get("criterion", []):
                    disposition = criterion.get("disposition")
                    if disposition in {"not_run", "partial"}:
                        risk = criterion.get("risk") or criterion.get("evidence") or ""
                        self.residual_risks.append(
                            f"security/{criterion.get('id', 'unknown')} ({disposition}): {risk}"
                        )
            except (OSError, tomllib.TOMLDecodeError):
                pass
        known = [
            "Observed capacity applies only to the recorded artifact, host, profile, and duration.",
            "External PlatON RPC latency and real notification providers are not represented by loopback fixtures.",
            "Package-manager install smoke (deb/rpm), multi-architecture artifacts, and the releasable SPDX SBOM run in the release CI native-artifacts job (syft required).",
            "Worker failure and partial receipt remain NOT_RUN without a safe production fault-injection seam.",
        ]
        for item in known:
            if item not in self.residual_risks:
                self.residual_risks.append(item)

    # --------------------------------------------------------------- assembly

    def assembly(self, started: float) -> dict:
        unavailable = [c.to_dict() for c in self.checks if c.status in {"UNAVAILABLE", "NOT_RUN"}]
        failed = [c for c in self.checks if c.status == "FAIL"]
        passed = [c for c in self.checks if c.status == "PASS"]
        if self.require_all:
            total = "FAIL" if (failed or unavailable) else "PASS"
        else:
            if failed:
                total = "FAIL"
            elif unavailable and not passed:
                total = "FAIL"
            elif unavailable:
                total = "PARTIAL"
            else:
                total = "PASS"
        report = {
            "schema_version": 1,
            "status": total,
            "started_at": dt.datetime.fromtimestamp(started, dt.timezone.utc).replace(microsecond=0).isoformat(),
            "finished_at": utc_now(),
            "version": self.version,
            "target": self.target,
            "profile": self.profile.name,
            "environment": {
                "os": platform.platform(),
                "python": platform.python_version(),
                "machine": platform.machine(),
                "cpu_count": os.cpu_count(),
            },
            "checks": [c.to_dict() for c in self.checks],
            "counts": {"pass": len(passed), "fail": len(failed), "unavailable": len(unavailable)},
            "artifacts": {
                "package_dir": display_path(self.output / "package"),
                "checksums": display_path(self.output / "package/release-set/SHA256SUMS") if (self.output / "package/release-set/SHA256SUMS").is_file() else None,
                "native_artifacts_dir": display_path(ROOT / "target/release-artifacts") if (ROOT / "target/release-artifacts").is_dir() else None,
                "sbom": [display_path(path) for path in (self.output / "package").rglob("*.spdx.json")] if (self.output / "package").is_dir() else [],
                "audit_results": display_path(ROOT / "target/release-artifacts/audit-results.txt") if (ROOT / "target/release-artifacts/audit-results.txt").is_file() else None,
                "package_results": display_path(ROOT / "target/release-artifacts/package-results.txt") if (ROOT / "target/release-artifacts/package-results.txt").is_file() else None,
            },
            "audit_results": {item.name: item.status for item in self.checks if item.phase == "dependency-policy"},
            "test_results": {
                "profile": self.profile.name,
                "qualification_root": display_path(self.output / "qualification"),
                "recovery_root": display_path(self.output / "recovery-rehearsal"),
                "harness_evidence": display_path(self.output / "release-candidate-harness.log"),
            },
            "security_dispositions": self.security_dispositions,
            "unavailable_checks": unavailable,
            "residual_risks": list(dict.fromkeys(self.residual_risks)),
        }
        return report

    def run(self) -> int:
        self.output.mkdir(parents=True, exist_ok=True)
        try:
            self.version = workspace_version() if shutil.which("cargo") else "<unknown>"
        except (QualificationError, OSError, json.JSONDecodeError):
            self.version = "<unknown>"
        try:
            self.target = rust_target() if shutil.which("rustc") else "<unknown>"
        except (QualificationError, OSError):
            self.target = "<unknown>"
        started = time.time()
        self.phase_rust()
        self.phase_dependency_policy()
        self.phase_web()
        self.phase_freshness()
        self.phase_playwright()
        self.phase_package()
        self.phase_forbidden_scan()
        self.phase_security_dispositions()
        self.phase_native_package_checks()
        self.phase_provenance()
        self.phase_recovery()
        self.phase_harness()
        self.phase_qualification()
        self.collect_residual_risks()
        report = self.assembly(started)
        (self.output / "final-qualification.json").write_text(
            json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        lines = [
            f"# Final release qualification: {report['status']}", "",
            f"- Version: {report['version']}  Target: {report['target']}",
            f"- Profile: {report['profile']}",
            f"- Passed: {report['counts']['pass']}  Failed: {report['counts']['fail']}  "
            f"Unavailable/NOT_RUN: {report['counts']['unavailable']}",
            "", "## Checks", "",
        ]
        for item in report["checks"]:
            lines.append(f"- **{item['status']}** {item['name']} ({item['phase']}): {item['detail']}")
        lines += ["", "## Residual risks", ""]
        lines += [f"- {risk}" for risk in report["residual_risks"]]
        (self.output / "final-qualification.md").write_text("\n".join(lines) + "\n", encoding="utf-8")
        note = " (unavailable checks listed above are not passes)" if report["status"] == "PARTIAL" else ""
        print(f"Final release qualification: {report['status']}{note} ({self.output})")
        return 0 if report["status"] in {"PASS", "PARTIAL"} else 1


def self_test() -> None:
    sample = "agent 0195f2a1-0001-4001-8001-000000000001 pp_agent_secret 127.0.0.1"
    cleaned = sanitize(sample)
    assert "0195" not in cleaned and "pp_agent" not in cleaned and "127.0.0.1" not in cleaned
    assert PRIVATE_KEY_RE.search("-----BEGIN PRIVATE KEY-----\nabc") is not None
    assert TOKEN_RE.search("pp_agent_abc_123") is not None
    with tempfile.TemporaryDirectory(prefix="platpulse-final-") as tmp:
        output = Path(tmp)
        runner = FinalQualification(output, DEFAULT_PROFILE, require_all=False)
        passed = runner.check("ok-check", "self-test")
        passed.detail = "passed"
        runner.run_unavailable(runner.check("unavailable-check", "self-test"), "tool", "missing")
        report = runner.assembly(time.time())
        assert report["status"] == "PARTIAL", report["status"]
        assert report["counts"]["unavailable"] == 1
        assert report["checks"][1]["status"] == "UNAVAILABLE"
        assert report["status"] != "PASS"  # unavailable must never yield PASS
        runner.require_all = True
        report = runner.assembly(time.time())
        assert report["status"] == "FAIL"
        runner2 = FinalQualification(output, DEFAULT_PROFILE, require_all=False)
        failed = runner2.check("fail-check", "self-test")
        failed.status = "FAIL"
        runner2.check("pass-check", "self-test").detail = "passed"
        report = runner2.assembly(time.time())
        assert report["status"] == "FAIL"
    print("Final release qualification self-test: PASS")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", type=Path, default=DEFAULT_PROFILE)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--require-all", action="store_true",
                        help="fail when any check is UNAVAILABLE or NOT_RUN (release gating)")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    try:
        if args.self_test:
            self_test()
            return 0
        profile = args.profile.resolve()
        if not profile.is_file():
            print(f"final release qualification: profile not found: {profile}", file=sys.stderr)
            return 2
        runner = FinalQualification(args.output.resolve(), profile, require_all=args.require_all)
        return runner.run()
    except QualificationError as error:
        print(f"final release qualification: FAIL ({error})", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
