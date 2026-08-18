# Release resilience qualification

PlatPulse qualifies packaged Agent and Server artifacts through authenticated AgentReport ingestion. It never writes Current Projections directly. The packaged Agent persist-report command verifies Durable Spool ordering and immutable bytes.

The CI and soak profiles live in release/qualification. Validate them with scripts/test-release-qualification.sh and run scripts/release-qualification.sh with --profile.

The run drives multiple Agents and Nodes through valid and invalid reports, exact retries, body conflicts, stale revisions, REST, SSE, metrics, workers, process restart, transport failure, SQLite busy behavior, realtime reconnect, and packaged Agent Durable Spool checks. Evidence records environment, duration, throughput, latency, failures, resource growth, scenario outcomes, and residual risks without sensitive identifiers or report contents.

PlatPulse exposes no production fault-injection or remote-control API. Controlled worker failure and synthetic partial-receipt injection are NOT_RUN, never PASS. A passing result applies only to the recorded artifact, host, profile, duration, and workload.
