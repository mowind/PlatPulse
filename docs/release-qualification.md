# Release resilience qualification

PlatPulse qualifies packaged Agent and Server artifacts through authenticated AgentReport ingestion. It never writes Current Projections directly. The packaged Agent persist-report command verifies Durable Spool ordering and immutable bytes.

The CI and soak profiles live in release/qualification. Validate them with scripts/test-release-qualification.sh and run scripts/release-qualification.sh with --profile.

The run drives multiple Agents and Nodes through valid and invalid reports, exact retries, body conflicts, stale revisions, REST, SSE, metrics, workers, process restart, transport failure, SQLite busy behavior, realtime reconnect, and packaged Agent Durable Spool checks. Evidence records environment, duration, throughput, latency, failures, resource growth, scenario outcomes, and residual risks without sensitive identifiers or report contents.

The packaged security matrix is documented in [Phase 5 security review](security-review.md) and runs as part of CI through `scripts/release-candidate-harness.sh`. It checks route authorization and credential isolation, Session fixation/revocation, exact Origin and CSRF, cookie/TLS/trusted-proxy policy, one-time enrollment, malformed input, API/SPA routing, and sanitized operational output at the packaged Server boundary.

PlatPulse exposes no production fault-injection or remote-control API. Controlled worker failure and synthetic partial-receipt injection are NOT_RUN, never PASS. A passing result applies only to the recorded artifact, host, profile, duration, and workload.

## Migration and recovery rehearsal

Run `scripts/release-recovery-rehearsal.sh` to build the packaged Server and exercise representative schema checkpoints (1, 9, 23, 29, and 35). The rehearsal starts each fixture through the supported Server startup path, verifies forward migration and preservation of projections, Report Receipts, block history, Peer/Geo data, Validator data, Alerts/Notifications, Operations, Audit Events, and human identity/session state, then exercises online backup, checksum failure, stopped-Server restore, secret-file preservation, corrupt input, and higher-schema refusal.

Evidence is written to `target/recovery-rehearsal/recovery-rehearsal.json` and `.md`; the fixture-only seam test is `scripts/release-recovery-rehearsal.sh --self-test`.
