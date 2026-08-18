# Release resilience qualification

PlatPulse qualifies packaged Agent and Server artifacts through authenticated AgentReport ingestion. It never writes Current Projections directly. The packaged Agent persist-report command verifies Durable Spool ordering and immutable bytes.

The CI and soak profiles live in release/qualification. Validate them with scripts/test-release-qualification.sh and run scripts/release-qualification.sh with --profile.

The run drives multiple Agents and Nodes through valid and invalid reports, exact retries, body conflicts, stale revisions, REST, SSE, metrics, workers, process restart, transport failure, SQLite busy behavior, realtime reconnect, and packaged Agent Durable Spool checks. Evidence records environment, duration, throughput, latency, failures, resource growth, scenario outcomes, and residual risks without sensitive identifiers or report contents.

The packaged security matrix is documented in [Phase 5 security review](security-review.md) and runs as part of CI through `scripts/release-candidate-harness.sh`. It checks route authorization and credential isolation, Session fixation/revocation, exact Origin and CSRF, cookie/TLS/trusted-proxy policy, one-time enrollment, malformed input, API/SPA routing, and sanitized operational output at the packaged Server boundary.

PlatPulse exposes no production fault-injection or remote-control API. Controlled worker failure and synthetic partial-receipt injection are NOT_RUN, never PASS. A passing result applies only to the recorded artifact, host, profile, duration, and workload.

## Migration and recovery rehearsal

Run `scripts/release-recovery-rehearsal.sh` to build the packaged Server and exercise representative schema checkpoints (1, 9, 23, 29, and 35). The rehearsal starts each fixture through the supported Server startup path, verifies forward migration and preservation of projections, Report Receipts, block history, Peer/Geo data, Validator data, Alerts/Notifications, Operations, Audit Events, and human identity/session state, then exercises online backup, checksum failure, stopped-Server restore, secret-file preservation, corrupt input, and higher-schema refusal.

Evidence is written to `target/recovery-rehearsal/recovery-rehearsal.json` and `.md`; the fixture-only seam test is `scripts/release-recovery-rehearsal.sh --self-test`.

## Final release qualification job

Run `scripts/final-release-qualification.sh --profile release/qualification/ci.toml --allow-known-not-run --require-all` for the final Phase 5 gate (use `soak.toml` for the scheduled soak profile). The explicit `--allow-known-not-run` exception keeps the documented `partial_receipt`, `worker_failure`, `agent_outage`, and `transport_timeout` seams visible as `INCOMPLETE` residual risks; every other unavailable check still blocks sign-off. The GitHub workflow provisions Rust format/lint components, `cargo-deny`, `cargo-audit`, npm dependencies, and the fixed Playwright Chromium browser before invoking the same runner.

The runner executes Rust formatting, Clippy with warnings denied, workspace tests, dependency audits, WebUI lint/typecheck/unit/build, OpenAPI and generated browser-client freshness, the four fixed Playwright projects (`phone-360-touch`, `phone-390-touch`, `tablet-768-touch`, and `desktop-1280`) with the configured single worker, package validation, native package/Docker-context checks when available, the release-candidate harness, recovery rehearsal, and the load/fault/soak profile. It also scans packaged artifacts for forbidden secrets and GeoLite data and verifies that unsigned artifacts are not described as a verified supply chain.

The consolidated evidence is `target/release-qualification/final/final-qualification.json` and `.md`. It records version, target, checksums, SBOM and audit/package evidence, profile results, environment, security-matrix dispositions, unavailable checks, and residual risks. Every check is `PASS`, `FAIL`, `INCOMPLETE`, `UNAVAILABLE`, or `NOT_RUN`; `INCOMPLETE` is used when a profile contains deliberate non-runnable scenarios, and `UNAVAILABLE`/`NOT_RUN` never becomes `PASS`. The final report retains incomplete scenarios and residual risks instead of silently converting them to pass. Without `--require-all`, an otherwise successful run is reported as `PARTIAL`; the workflow preserves that explicit status so deliberate environment-dependent checks cannot be mistaken for a pass. Use `--require-all` when the operator wants missing gates to block sign-off.
