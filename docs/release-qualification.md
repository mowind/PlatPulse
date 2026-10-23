# Release resilience qualification

PlatPulse qualifies packaged Agent and Server artifacts through authenticated AgentReport ingestion. It never writes Current Projections directly. The packaged Agent persist-report command verifies Durable Spool ordering and immutable bytes.

The CI and soak profiles live in release/qualification. Validate them with scripts/test-release-qualification.sh and run scripts/release-qualification.sh with --profile.

The run drives multiple Agents and Nodes through valid and invalid reports, exact retries, body conflicts, stale revisions, REST, SSE, metrics, workers, process restart, transport failure, SQLite busy behavior, realtime reconnect, and packaged Agent Durable Spool checks. Evidence records environment, duration, throughput, latency, failures, resource growth, scenario outcomes, and residual risks without sensitive identifiers or report contents.

The packaged security matrix is documented in [Phase 5 security review](security-review.md) and runs as part of CI through `scripts/release-candidate-harness.sh`. It checks route authorization and credential isolation, Session fixation/revocation, exact Origin and CSRF, cookie/TLS/trusted-proxy policy, one-time enrollment, malformed input, API/SPA routing, and sanitized operational output at the packaged Server boundary.

PlatPulse exposes no production fault-injection or remote-control API. Controlled worker failure and synthetic partial-receipt injection are NOT_RUN, never PASS. A passing result applies only to the recorded artifact, host, profile, duration, and workload.

## Migration and recovery rehearsal

Run `scripts/release-recovery-rehearsal.sh` to build the packaged Server and exercise representative schema checkpoints. The script derives the final checkpoint from the latest file in `crates/platpulse-server/migrations` (currently schema 76; migration `0076_storage_migration.sql`) and combines it with the fixed historical checkpoints (1, 9, 23, 29, 35, 36). The rehearsal starts each fixture through the supported Server startup path, verifies forward migration and preservation of representative projections, Report Receipts, block history, Peer/Geo rows, and the one-time Validator model cutover (#174) — a fixture already at the cutover schema keeps its seeded Validator identity and snapshot/ranking rows, while every older fixture has its manual Validator generation (identity, snapshot, ranking/counter history and daily/monthly aggregates) removed and the `validator_model_migration` marker recorded — plus Site Access settings, selected Alert/Notification/Operation rows, Audit Events, and representative human identity/session presence and Agent-epoch checks. It then creates a backup artifact through the packaged CLI while the Server is stopped, and exercises checksum failure, stopped-Server restore, secret-file preservation, corrupt input, and higher-schema refusal; concurrent/live-write backup is not covered by this rehearsal.

Evidence is written to `target/recovery-rehearsal/recovery-rehearsal.json` and `.md`; the fixture-only seam test is `scripts/release-recovery-rehearsal.sh --self-test`.

### Storage Backend Migration measurement (#257)

The Storage Backend Migration is offline and one-way, so its cost is a downtime
input: the window has to hold the copy, the verification that re-reads what was
copied, and the final commit. A measurement on a synthetic SQLite fixture built
to look like a near-production deployment — 5.58 GiB, 13,105,798 rows in 82
tables, schema 76 — copied in 6.7 minutes (`13105798 of 13105798 row(s) copied
in 6.7min`) and the whole command took 11 minutes 13 seconds of wall clock
against a local PostgreSQL 18 over a Unix-domain socket. The work after the copy
dominated: verifying every table re-read 4.8 million `node_metric_samples` rows
in a single 107.7-second query, and the final commit took 215.6 seconds. The
target database measured 7328 MB afterwards, about 1.2 times the source file.
Those numbers describe that fixture, host, storage and database, not a budget for
another deployment: a remote database, slower disks or a larger schema change the
total, so measure a copy of the deployment's own data before the real switch.

The rollback is rehearsed the same way. A Storage Backend Migration of a 35-row
fixture followed by `platpulse-server storage migrate abort` removed the 35 rows it
had written across all 82 tables, deleted the marker, and the SQLite deployment
served `/health/ready` again. A target that has accepted a write since the baseline
is refused instead: starting the Server on the migrated database wrote rows of its
own, and the abort named the mismatch ("no longer holds the state recorded when it
was migrated …; now 82 table(s), 48 row(s), digest …") rather than emptying a
database that had moved on. So did an abort while a Server was still running
("another Server owns this PostgreSQL database"). The forward path was exercised
after the refusal too: the same configuration started again, reported its
`postgres` readiness component ready, and stopped cleanly on SIGTERM, so a healthy
migrated deployment needs no repair tool. The SQLite file is a fallback point, not
a rollback target.

## Final release qualification job

Run `scripts/final-release-qualification.sh --profile release/qualification/ci.toml --allow-known-not-run --require-all` for the final Phase 5 gate (use `soak.toml` for the scheduled soak profile). The explicit `--allow-known-not-run` exception keeps the documented `partial_receipt`, `worker_failure`, `agent_outage`, `transport_timeout`, and `alert_outbox_restart` seams visible as `INCOMPLETE` residual risks (the offline harness drives no notification provider, so Alert/Notification outbox durability is only asserted when the profile actually produces incidents or deliveries); every other unavailable check still blocks sign-off. The GitHub workflow provisions Rust format/lint components, `cargo-deny`, `cargo-audit`, npm dependencies, and the fixed Playwright Chromium browser before invoking the same runner.

The runner executes Rust formatting, Clippy with warnings denied, workspace tests, dependency audits, WebUI lint/typecheck/unit/build, OpenAPI and generated browser-client freshness, the four fixed Playwright projects (`phone-360-touch`, `phone-390-touch`, `tablet-768-touch`, and `desktop-1280`) with the configured single worker, package validation, native package/Docker-context checks when available, the release-candidate harness, recovery rehearsal, and the load/fault/soak profile. It also scans packaged artifacts for forbidden secrets and GeoLite data and verifies that unsigned artifacts are not described as a verified supply chain.

The consolidated evidence is `target/release-qualification/final/final-qualification.json` and `.md`. It records version, target, checksums, SBOM and audit/package evidence, profile results, environment, security-matrix dispositions, unavailable checks, and residual risks. Every check is `PASS`, `FAIL`, `INCOMPLETE`, `UNAVAILABLE`, or `NOT_RUN`; `INCOMPLETE` is used when a profile contains deliberate non-runnable scenarios, and `UNAVAILABLE`/`NOT_RUN` never becomes `PASS`. The final report retains incomplete scenarios and residual risks instead of silently converting them to pass. Without `--require-all`, an otherwise successful run is reported as `PARTIAL`; the workflow preserves that explicit status so deliberate environment-dependent checks cannot be mistaken for a pass. Use `--require-all` when the operator wants missing gates to block sign-off.
