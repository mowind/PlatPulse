Part of the [PlatPulse Server deployment guide](../deployment.md): Transport modes; Diagnostic logs; Internal operational metrics; Same-origin behavior; Runtime database readiness; Durable Spool capacity and the downtime it covers.

## Transport modes

Production has two explicit transport choices. For direct HTTPS, configure a
certificate chain and private key in `server.toml`:

```toml
listen = "0.0.0.0:8443"
public_base_url = "https://monitor.example.com:8443"
[tls]
cert_chain_file = "/etc/platpulse/tls/fullchain.pem"
private_key_file = "/etc/platpulse/tls/privkey.pem"
```

The private-key path must resolve to a same-user-owned private regular file
with mode `0600`; final symlinks and symlinked ancestors are rejected. The
Server validates and parses both files before binding. A malformed,
unreadable, mismatched, or insecure key fails startup with a redacted
transport diagnostic. PlatPulse does not issue certificates, run ACME, or
reload certificates in-process: replace the files and restart the Server.

Alternatively, terminate HTTPS at a trusted reverse proxy and keep the Server
on a private listener:

```toml
listen = "127.0.0.1:8080"
public_base_url = "https://monitor.example.com"
trusted_proxy_cidrs = ["127.0.0.1/32"]
trusted_proxy_scheme = "https"
```

Forwarded headers are accepted only from a peer in one of the configured CIDRs
and only when the configured scheme is `https`. Conflicting or spoofed
forwarded headers are rejected. A non-loopback production plaintext listener
without either native TLS or this trusted-proxy policy is refused at startup.
Development mode is separate and remains loopback-only HTTP with its
development cookie policy; it cannot be combined with native TLS.

## Diagnostic logs

Both binaries log to **stderr** (the journal under systemd); stdout carries only
machine-readable and CLI result output. Every subcommand accepts:

- `--log-level <error|warn|info|debug|trace>` — default `info`; overrides
  `RUST_LOG`. Without it, `RUST_LOG` uses env-filter syntax, e.g.
  `RUST_LOG=platpulse_server=debug`.
- `--log-format <text|json>` — default `text`
  (`ts LEVEL target message key=value …`); `json` writes one object per line.

Values are redacted at the log egress (credentials, tokens, cookies, URL
userinfo; IP literals on the Server). Every line written while serving a
request carries `request_id`, the same value as the `x-request-id` response
header and the error body's `request_id`:

```sh
journalctl -u platpulse-server | grep 'request_id=<id from x-request-id>'
```

The startup line `listening listen=<addr>` replaces the former stdout line
`listening on <addr>`. Accepted reports and individual requests are `debug`, so
the default level stays quiet in steady state. See
[docs/design/logging.md](../design/logging.md) for the field vocabulary and levels.

## Internal operational metrics

The optional `[metrics]` section exposes only `GET /metrics` on a dedicated
management listener. It is disabled when the section is absent and defaults to
`127.0.0.1:9090` when enabled without an explicit address:

```toml
[metrics]
enabled = true
listen = "127.0.0.1:9090"
```

The metrics router has no Public, Admin, Agent, health, authentication, or SPA
routes and is not included in OpenAPI or the generated browser client. Its
labels are fixed low-cardinality dimensions; Node IDs, Peer IDs, User IDs,
Agent IDs, IP addresses, report IDs, request parameters, credentials, error
strings, and report bodies are never exposed. Non-loopback metrics binds are
refused unless native Rustls TLS or the explicit trusted HTTPS proxy policy is
configured, using the same pre-bind safety checks as the main listener.

The exposition documents these bounded families: HTTP responses by surface and
status class; AgentReport and Report Receipt outcomes; readiness components and
critical-worker heartbeat age; realtime connection and bounded-buffer pressure;
operation and notification-delivery states; SQLite page, freelist, WAL-byte,
and pool pressure; in-flight ingestion; and metrics listener state. A failed
collection is represented by an absent dynamic sample rather than a fabricated
zero.

| Metric | Type | Fixed labels | Semantics |
| --- | --- | --- | --- |
| `platpulse_http_requests_total` | counter | `surface`, `status` | Responses by route group and status class. |
| `platpulse_agent_reports_total` | counter | `outcome` | AgentReport attempts by Receipt disposition, or `unknown`. |
| `platpulse_report_receipts_total` | counter | `outcome` | Report Receipts actually returned by disposition. |
| `platpulse_readiness` | gauge | `component` | Per-component readiness (`1` ready, `0` not ready). |
| `platpulse_ready` | gauge | none | Whether every required readiness component is ready. |
| `platpulse_liveness` | gauge | none | `1` while this process serves the metrics surface. |
| `platpulse_critical_worker_heartbeat_age_seconds` | gauge | none | Critical-worker heartbeat age; absent until first observed. |
| `platpulse_realtime_connections` | gauge | `surface` | Active Public/Admin realtime streams. |
| `platpulse_realtime_buffered_events` | gauge | `surface` | Events held in each bounded realtime buffer. |
| `platpulse_operations` | gauge | `status` | Durable Operation rows by fixed status; absent if unavailable. |
| `platpulse_notification_deliveries` | gauge | `state` | Delivery rows by fixed state; absent if unavailable. |
| `platpulse_sqlite_page_count` | gauge | none | Allocated SQLite pages; absent if unavailable. |
| `platpulse_sqlite_freelist_pages` | gauge | none | SQLite freelist pages; absent if unavailable. |
| `platpulse_sqlite_wal_bytes` | gauge | none | WAL sidecar bytes. |
| `platpulse_sqlite_pool_size` | gauge | none | Pool connection capacity. |
| `platpulse_sqlite_pool_idle` | gauge | none | Idle pool connections. |
| `platpulse_ingestion_in_flight` | gauge | none | AgentReport ingestions currently executing. |
| `platpulse_history_gap_settlements_total` | counter | `path` | History Gap settlement passes on the `report` or `retention` path. |
| `platpulse_history_gap_settlement_slow_total` | counter | `path` | Settlement passes that took 500 ms or longer (each also logs a warning). |
| `platpulse_history_gap_settlement_milliseconds_total` | counter | `path` | Time spent settling History Gaps. |
| `platpulse_history_gap_settlement_examined_total` | counter | `path` | Unresolved `unrecoverable_backfill` gaps examined; tracks touched gaps, not table size. |
| `platpulse_history_gap_settlement_resolved_total` | counter | `path` | History Gaps resolved by settlement. |
| `platpulse_metrics_scrapes_total` | counter | none | Scrapes served by this process. |
| `platpulse_metrics_listener_failures_total` | counter | none | Redacted listener startup/runtime failures. |
| `platpulse_metrics_listener_enabled` | gauge | none | Whether the listener is configured. |
| `platpulse_metrics_listener_ready` | gauge | none | Whether the listener is ready. |

## Same-origin behavior

- `/` and React Router paths such as `/admin` receive `index.html`;
- `/assets/<hashed-file>` receives immutable caching headers;
- `index.html` is served with `Cache-Control: no-cache`;
- unmatched `/api/*` paths remain JSON error responses and never fall through
to the SPA;
- REST, SSE, cookies, and the SPA use the same origin.

## Runtime database readiness

`/health/live` proves only that the HTTP event loop responds. Monitor
`/health/ready` (HTTP 200/503) and the `platpulse_readiness` component metrics
for operational readiness. Schema/Owner queries alone do not establish database
integrity.

Runtime integrity is split by where it can actually work
([ADR 0008](../adr/0008-offline-server-backup.md), issue #194):

- **Start-up** runs one bounded `PRAGMA integrity_check` as a fail-closed gate.
  A corrupt, unreadable, or budget-exhausted database refuses to start with an
  explicit error; it is never silently served.
- **The serving process runs no periodic whole-database scan.** On a
  deployment-sized database such a scan cannot complete inside a useful budget
  on the only SQLite connection, and every attempt would stall ingestion while
  still producing no verdict.
- **Runtime corruption is latched from an observed `SQLITE_CORRUPT`.** When a
  real query on a hot path (report ingestion, readiness, retention, Doctor)
  returns SQLite's corruption code, the `corruption` and `sqlite` readiness
  components report `integrity_check_failed` and the latch never clears; later
  successful queries do not let the Server claim health again. Liveness stays
  independent.
- **Doctor's `quick_check` is bounded.** If it cannot finish inside its budget
  it reports that it did not finish (a warning) instead of a verdict.
- **The authoritative verdict is offline.** Run
  `platpulse-server verify-integrity --config /etc/platpulse/server.toml` with
  the Server stopped (the command takes the exclusive ownership guard), or rely
  on the same bounded check `platpulse-server backup` runs before creating an
  artifact. There is no separate "integrity unavailable" readiness state.

Keep corruption evidence and known-good rollback copies until recovery and
backup verification are complete. Do not repair or replace files underneath a
running Server.

## Durable Spool capacity and the downtime it covers

The Agent's Durable Spool is bounded by bytes (`[spool] max_bytes`, default
512 MiB) and by age (`max_age_seconds`, default 24 hours). Whichever bound binds
first drops the oldest unconfirmed historical reports into History Gaps (design
§7.2, issue #273), so Server downtime — a restart, an Offline Backup Window, or
the stopped Server a Storage Backend Migration needs — is absorbed only while the
byte bound does not bind. Convert one into the other:

    tolerated downtime ≈ max_bytes ÷ (average report size × generation rate)

`generation rate` is one report per `collection_interval_seconds`
(`1 ÷ collection_interval_seconds`). `average report size` is the mean
persisted AgentReport body, so it grows with the Nodes one Agent monitors and
with the peer count per Node. Measure it instead of assuming it: Admin → Agents
→ the Agent's page reports **Spool bytes / capacity** and **Spool queued
reports**, and the queued bytes divided by the queued reports is the current
average report size.

The shipped 512 MiB is sized for a **four-hour** outage at a fast cadence. In the
reference measurement behind it — about 60 peers per Node, a one-second
collection interval — one report is about 33.7 KB and the Agent generates about
0.83 reports/s, so

    536870912 B ÷ (33700 B × 0.83/s) ≈ 19194 s ≈ 5.3 hours

which covers the four hours it targets with margin. Solve the formula the other
way to size a Spool for a longer accepted outage, a shorter interval, or more
Nodes:

    max_bytes ≈ tolerated downtime × average report size × generation rate

For example, an eight-hour outage at that same rate needs
`28800 s × 33700 B × 0.83/s ≈ 805 MB`; set `max_bytes = 1073741824` (1 GiB, the
ceiling) there and restart the Agent, which records the new policy at startup.
A Host whose rate needs more than 1 GiB for the outage it must absorb has two
remaining options: raise `collection_interval_seconds` so it generates fewer,
larger Reports, or accept the History Gap the overflow records. The Spool never
silently loses a report, so an overrun is visible as dropped ranges in the same
Admin panel.

`max_age_seconds` is the independent age bound: a report the Agent cannot
deliver inside that window is dropped even when the byte bound was never
reached, which is what keeps a stalled Spool from becoming an unbounded archive.
The flush reserve is not a separate key — it follows capacity (three quarters
of it), so it can never contradict the bound it is derived from.
