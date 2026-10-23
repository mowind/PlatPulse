# PlatPulse Server deployment

A production release contains one runtime process:

```text
/usr/bin/platpulse-server
/usr/share/platpulse/web/
├── index.html
└── assets/
```

The WebUI is not compiled into the Rust binary. It is served same-origin by
`platpulse-server`; no Vite or separate web server is required at runtime.

## Build a release bundle

From the repository root, build a target-aware release set:

```bash
scripts/build-release.sh \
  --target x86_64-unknown-linux-gnu \
  --output target/release-artifacts
```

The set contains separate Agent and Server archives, package-manager artifacts
when the builders are available, checksums, an SPDX inventory, and dependency
audit evidence. The staged Server tree includes the same-origin WebUI and
deployment examples. The staged Agent tree contains the Agent binary, unit, and
configuration reference. Pepper, TLS private keys, Agent credentials,
notification tokens, SQLite state/sidecars, MMDB files, and other secrets are
never included.

`scripts/package-release.sh` remains a compatibility wrapper for the
release-candidate harness and exposes the historical unpacked Server location.

Install one matching architecture using the distribution package manager, or
extract an archive into an empty staging directory before copying its allowlisted
`usr/` and `etc/` trees:

```bash
sudo dpkg -i platpulse-server-<version>-x86_64.deb
sudo dpkg -i platpulse-agent-<version>-x86_64.deb
# or, on RPM-based systems
sudo rpm -Uvh platpulse-server-<version>-1.x86_64.rpm
sudo rpm -Uvh platpulse-agent-<version>-1.x86_64.rpm
```

Native archives do not create service users. Before enabling the included units,
create the documented `platpulse-server`/`platpulse-agent` users, install the
files with root ownership and packaged modes, and create the private state,
secret, and backup directories with the runtime user's ownership.

## Release-candidate harness

Run the packaged artifact through the external CLI, HTTP, and SSE boundaries with:

```bash
scripts/release-candidate-harness.sh
```

The harness creates a unique temporary run directory under
`target/release-candidate-runs/` (override it with
`PLATPULSE_RC_RUNS_ROOT`), builds the release bundle, initializes real temporary
SQLite state, provisions controlled Owner/Viewer/Network fixtures, enrolls an
Agent, submits the canonical two-Node report, and checks the Report Receipt,
Admin projection, WebUI asset, health endpoints, isolated Prometheus metrics,
and authorized Admin SSE.
Normal completion removes the run directory. A failed run removes credentials,
SQLite files, cookies, headers, and response bodies before preserving the
artifact, configuration, logs, request IDs, and sanitized diagnostics. Exit 2
means the environment is unavailable; exit 1 is a harness failure.

For packaged load, fault, and soak evidence, see
[`docs/release-qualification.md`](release-qualification.md).

To rehearse upgrades and recovery against a packaged Server, run:

```bash
scripts/release-recovery-rehearsal.sh
```

The command creates private schema fixtures under `target/recovery-rehearsal/`,
starts each one so the compiled forward migrations run before serving, and records
sanitized JSON/Markdown evidence. It also verifies the supported backup-artifact
path through the packaged CLI while the Server is stopped; concurrent/live-write
backup is not covered. It then checks checksum and integrity failures,
stopped-Server confirmation, atomic restore and
safety-copy behavior, secret-file preservation, corrupt input refusal, and higher
schema refusal. Use `--self-test` for the fast fixture-generation check.

## Configuration

Copy `crates/platpulse-server/server.example.toml` and set an explicit
`state_dir`, secret paths, and public URL. If `web_root` is omitted, the
Server resolves the installed default `/usr/share/platpulse/web`. A configured
`web_root`, or `serve --web-assets`, takes precedence over that default.

The Server may start when the default or configured WebUI directory is
missing/incomplete, but `/health/ready` reports the `web_assets` component as
not ready with reason `web_assets_missing`. Readiness evaluates eight components:
`storage_ownership`, the storage component named for the backend in use
(`sqlite`, or `postgres` with the PostgreSQL Storage Backend), `owner`,
`web_assets`, `shutdown`, `critical_workers`, `corruption`, and
`inventory_cutover`; any `not_ready` component makes the endpoint return HTTP
503. `/health/live`
only proves that the event loop responds. `init` emits a warning instead of
creating or modifying WebUI files.

```bash
platpulse-server init --config /etc/platpulse/server.toml
platpulse-server owner create \
  --config /etc/platpulse/server.toml \
  --username admin
platpulse-server serve --config /etc/platpulse/server.toml
```

`--web-assets /path/to/web` is an explicit `serve` override for tests,
development, and custom installations. It has higher precedence than the
`web_root` value in `server.toml`, which has higher precedence than the
built-in default.

### Storage Backend

The Server stores its data in a SQLite database file by default. The PostgreSQL
Storage Backend is chosen explicitly by name ([ADR 0013](adr/0013-postgresql-server-storage-backend.md));
it is never inferred from the environment, a URL, or a DSN:

```toml
[storage]
backend = "postgres"

[storage.postgres]
host = "/run/postgresql"   # or a host name / IP address
port = 5432
database = "platpulse"
user = "platpulse"
password_file = "/etc/platpulse/secrets/postgres-password"
sslmode = "disable"        # accepted only over a Unix-domain socket
connect_timeout_secs = 10
ownership_heartbeat_secs = 5
```

Every setting comes from `[storage.postgres]`: the `PG*` libpq variables and
`.pgpass` are ignored, and a `password`, `url`, or `dsn` key is rejected. The
password is read only from `password_file`, which follows the same policy as
`pepper_file` (a same-user 0600 regular file, no symlinked path). It never
appears in a log, an error, or the redacted connection `Debug` output. When the
PostgreSQL backend is selected, `db_path` must be absent; leave `backup_dir`
in place only if the SQLite tooling still uses it.

`sslmode` defaults to `verify-full` and accepts only `verify-ca` as another
value; `disable` must be written out explicitly and is accepted only for a
Unix-domain socket directory, so leaving the key out never downgrades a socket
connection to plaintext by accident. A remote connection is always encrypted, so
`disable` over TCP is refused. `ssl_root_cert_file` supplies the CA certificate and cannot accompany
`disable`. Both timers are 1..=300 s and the heartbeat must be shorter than the
connect timeout.

The PostgreSQL baseline migration lives beside the SQLite ones in
`crates/platpulse-server/migrations-postgres` and is applied automatically after
ownership is taken. Both backends share one logical schema version and CI proves
their tables, columns, constraints, and indexes stay equivalent.

Provisioning the role and database, TLS and certificate trust, the password file,
the single-owner rule, the behaviour after an ownership loss and the operations
that are not available on PostgreSQL are covered in "PostgreSQL Storage
Backend" below. An existing SQLite deployment moves onto PostgreSQL only through
the offline Storage Backend Migration (`storage migrate`, one way); its runbook,
refusal conditions and abort rule are in "Upgrade and rollback".

The current production provider is Telegram. If no channel is configured,
Notification Events are still recorded but no Delivery rows are created; configure
a channel with a path to a secret file and a destination:

```toml
[notifications.telegram]
enabled = true
token_file = "/etc/platpulse/secrets/telegram-token"
chat_id = "123456789"
max_attempts = 5
retry_base_seconds = 60
```

The token file must be a regular file readable by the `platpulse-server` service
user (mode `0600` is recommended when its owner/group is that service account).
The provider reads and trims it for each send, so controlled token rotation takes
effect without a restart. For a queued Delivery, an unreadable or empty token file
or a disabled/missing channel produces `state = failed` with
`last_error_kind = config`, without calling Telegram or consuming an attempt; a
new Owner test-send action rejects a disabled channel before creating its Event or
Delivery. Provider/API transport failures retry with bounded exponential backoff;
exhaustion becomes `dead_letter`. An Owner can manually retry a `failed`,
`retry_scheduled`, or `dead_letter` Delivery after correcting the configuration.
Delivery destination is masked; the full token and directory path never enter
DTOs, Audit, or logs, while the Channel DTO intentionally exposes only the
secret-file basename as `providerRef`.

## PostgreSQL Storage Backend

PostgreSQL is the second permanent Storage Backend ([ADR 0013](adr/0013-postgresql-server-storage-backend.md));
SQLite stays the default and the backend is selected only by the
`[storage] backend` key. That key, the `[storage.postgres]` section, and the
connection rules are in "Storage Backend" under "Configuration"; this chapter
covers what an Operator provisions and what changes at runtime.

### Provisioning the role and the database

The Server never creates a role, a database, or a schema for you. Create the
database with the byte-order collation the schema needs — a database's locale
cannot be changed afterwards, so it has to be right at creation:

```bash
sudo -u postgres psql -v ON_ERROR_STOP=1 -c "CREATE ROLE platpulse LOGIN" -c \
  "CREATE DATABASE platpulse OWNER platpulse TEMPLATE template0 ENCODING 'UTF8' LC_COLLATE 'C' LC_CTYPE 'C'"
```

- `TEMPLATE template0` with explicit `ENCODING`, `LC_COLLATE` and `LC_CTYPE` is
  required. A database cloned from `template1` inherits the cluster locale, and
  the Server refuses to start when any text column of its schema is not collated
  `"C"`, because its comparisons, indexes and `ORDER BY` semantics must match
  SQLite's byte order. A database created with the wrong locale is dropped and
  recreated, not repaired in place.
- `OWNER platpulse` also makes the role the schema owner the Server requires: the
  `public` schema belongs to `pg_database_owner`, and the startup check accepts
  ownership through role membership. The Server migrates into the connection's
  `current_schema()`, so a dedicated schema needs an explicit
  `ALTER SCHEMA <schema> OWNER TO platpulse` and a role-level `search_path`.
- The role needs a password for any `pg_hba.conf` rule that asks for one, which
  is every TCP connection to a default installation (`scram-sha-256`, or `md5`
  on an older cluster). Set it inside `psql` so the plaintext never reaches the
  shell history or the server log, and put the same secret in `password_file`:

  ```bash
  sudo -u postgres psql -c "\password platpulse"   # prompts twice, never echoes
  ```

  A socket-only deployment whose `pg_hba.conf` rule is `peer` needs no password
  check, because the service user and the role have the same name (`platpulse`);
  `password_file` is still required and still has to hold one non-empty line, but
  PostgreSQL never asks for it there. `trust` accepts a connection without
  checking any credential and is not a recommended rule. Whatever the rule, the
  password comes from `password_file` alone and is never sent anywhere else.

Confirm the three facts the Server checks before it starts:

```bash
sudo -u postgres psql -d platpulse -c "SELECT version()" \
  -c "SELECT datcollate, datctype FROM pg_database WHERE datname = 'platpulse'" \
  -c "SELECT pg_get_userbyid(nspowner) FROM pg_namespace WHERE nspname = 'public'"
```

`version()` must be 15 or newer; an older server is refused with "PostgreSQL
server version … is older than the supported minimum 15". `datcollate` and
`datctype` must both be `C`. The schema owner must be the Server role, directly
or through membership; otherwise startup names the remedy ("role "…" does not own
the target schema "…" (owner: "…"); make the Server role the schema owner, for
example: ALTER SCHEMA public OWNER TO platpulse"). The Owner-only Doctor
(`POST /api/admin/v1/doctor` queues a run, `GET` returns the latest report)
reports the same four checks —
`postgres_connection`, `postgres_server_version`, `postgres_collation` and
`postgres_ownership` — on every run, so a server upgrade or a changed ownership
stays visible.

The PostgreSQL baseline migration set in
`crates/platpulse-server/migrations-postgres` is applied by the Server itself on
its first start, and by `storage migrate` when an existing SQLite deployment
moves over — there is no `init` on this backend.

### TLS, certificate trust, and the password file

A local database reached through a Unix-domain socket is the only plaintext
case: `sslmode = "disable"` is accepted only with a socket directory in `host`,
and refused over TCP ("`sslmode = "disable"` is refused over TCP: a remote
PostgreSQL connection is always encrypted (ADR 0013)"). A socket deployment has
to write that key out. The mode is applied to the connection whatever the
transport, and PostgreSQL does not speak TLS on a socket, so the default
`verify-full` fails at startup with "error occurred while attempting to
establish a TLS connection: server does not support TLS".

```toml
[storage.postgres]
host = "db.internal.example"
sslmode = "verify-full"
ssl_root_cert_file = "/etc/platpulse/secrets/postgres-ca.pem"
```

`verify-full` verifies the certificate chain and the server name. `verify-ca`
verifies only the chain, so any certificate that CA ever issued is accepted under
any name; use it only when that CA signs for this one database. There is exactly
one configured CA and no client certificate setting. Without
`ssl_root_cert_file` the chain is checked against the public WebPKI roots the
Server is built with, not against the host's `/etc/ssl/certs`, so an internal or
self-signed CA must be named explicitly. The libpq variables that would override those choices make
the Server refuse to start instead of connecting differently from the file —
`PGSSLCERT`, `PGSSLKEY`, `PGOPTIONS`, and `PGSSLROOTCERT` when no
`ssl_root_cert_file` is configured: "the libpq environment variable(s) … would
change how the Server connects to PostgreSQL; unset them (the Server takes every
PostgreSQL setting from `[storage.postgres]` only)". The remaining `PG*`
variables and `.pgpass` are never consulted. [ADR 0013](adr/0013-postgresql-server-storage-backend.md)
puts that as libpq's environment being ignored; the Server enforces it literally by
building its own connection options (never `PgConnectOptions::new()`) and refusing
the variables it cannot shadow instead of connecting differently from the file.

The password comes only from `password_file`, which requires an absolute path
("`password_file` is required: the password is never read from `server.toml`,
the environment, or the command line") and follows the same sensitive-file policy
as `pepper_file`: a regular file, owned by the service user, no group or world
access (mode `0600`), no symlink in the path, at most 4096 bytes, exactly one
line. Anything else refuses startup, and an empty file is refused too because SQLx
would otherwise fall back to `.pgpass`. The value is read when the connection is
opened, so a rotation takes effect with the next Server start; it never appears in
a log, an error, or the redacted connection `Debug` output.

### One Server per database, and after an ownership loss

An ownership lock, not a configuration convention, keeps two Servers apart: the
running Server holds a session-level advisory lock on a dedicated ownership
connection, and a second Server pointed at the same database refuses to start
("another Server owns this PostgreSQL database: the session advisory lock is
held; only one Server may serve a database at a time"). The work pool keeps a
single connection and never reconnects after ownership is lost; a transaction
interrupted that way rolls back and Ingestion answers with a retryable 503.

While it serves, the Server proves ownership with a heartbeat on that connection
every `ownership_heartbeat_secs`, bounded by `connect_timeout_secs`. A failure —
the connection closed, or a network partition detected by the next heartbeat — is
final for that process: it logs "PostgreSQL ownership connection lost; the Server
stops without reacquiring ownership", stops writing, reports the
`storage_ownership` readiness component as not ready (so `/health/ready` answers
503), drains for up to 10 s, and exits non-zero with "Storage Backend ownership
lost (…, checkpointed=…)". It never takes the lock again in the running process,
so a former owner cannot write beside a new one.

The shipped unit therefore restarts it: `Restart=on-failure` with
`RestartSec=5s`. Each attempt exits immediately while the other session holds the
lock, and the cycle repeats every five seconds. That loop is a recovery retry, not
failover:

- It is not high availability. Never run a second Server, on another host or as a
  standby unit, against the same database: the loser only produces refused work,
  and the loop keeps producing it.
- Treat the loop as an incident, not as a finished recovery. Find the holder
  (`SELECT pid, application_name, state FROM pg_stat_activity;` and
  `SELECT pid, granted FROM pg_locks WHERE locktype = 'advisory';`), stop the
  unit that should not be running, and confirm the remaining Server serves
  `/health/ready`. A former owner whose work was refused lost nothing durable:
  Agents retry from their Durable Spool.
- To keep the unit from retrying indefinitely, bound it with `StartLimitIntervalSec=`
  and `StartLimitBurst=` so systemd parks it in `failed` instead of restarting.
- Do not remove the lock by hand (`pg_terminate_backend`) while a Server is
  writing: terminate the process that lost ownership instead and let its
  connection close.

### Operations that do not apply on PostgreSQL

The SQLite file tooling refuses this backend by design, and the PostgreSQL
tooling that owns the database replaces it:

| Operation | On PostgreSQL |
| --- | --- |
| `init` | Refused: "…operates on the Server SQLite database file and is not available on the PostgreSQL Storage Backend (ADR 0013)…". Nothing is missing — the baseline is applied on first start, or by `storage migrate`. |
| `backup`, `restore`, `verify-integrity`, `checkpoint create` | Refused with the same message. Use `pg_dump`/`pg_restore`, `pg_basebackup` and `pg_verifybackup`, and rehearse the restore yourself: no packaged backup timer or unit exists for this backend. |
| `cutover status`, `cutover resume` | Refused. `storage migrate` refuses a source whose Inventory conversion is still `awaiting_resume`, so the cutover is completed on SQLite before the move. |
| `[capacity]` | Refused when the configuration loads: "the database disk belongs to the database operator, so the Server neither measures nor protects it (ADR 0013)". Delete the section. The filesystem gauges have no filesystem to measure and
publish no sample; `platpulse_capacity_paused` is still published and reads `0`. |
| `doctor` backup checks | Backup age, backup residue and the SQLite integrity check are reported `not_applicable`; the four `postgres_*` checks represent this backend. |
| `/health/ready` and `platpulse_readiness` | Report the backend-named component `postgres`; the storage component this deployment does not use is omitted from both surfaces and never blocks readiness. |
| Operational metrics | `platpulse_storage_database_bytes` comes from `pg_database_size`; `platpulse_sqlite_page_count`, `platpulse_sqlite_freelist_pages` and `platpulse_sqlite_wal_bytes` have no sample (an unknown is never published as zero), and the pool gauges describe the single work connection. |

### Storage Snapshots on PostgreSQL are unsanitized host data

A PostgreSQL dump, a WAL archive or PITR chain, and a managed-instance snapshot
are all Storage Snapshots: complete copies of the Server's state taken outside the
Server process. They are the supported recovery path on this backend, and they are
never sanitized. A Storage Snapshot holds what the database holds — raw Peer
addresses and RPC Endpoints, credential digests, Owner password hashes, Report
Receipt bodies, and Audit before/after JSON — so it needs the same protection as
the database itself:

- encrypt it at rest and restrict it to the operator's backup path and backup
  role; an unencrypted dump on shared storage is a credential disclosure, not
  just a data copy;
- treat every copy of it as in scope for the same handling, including snapshots a
  managed service takes and stores where you cannot delete them;
- the Server cannot see, delete, rotate, or verify these copies, and neither
  `doctor` nor `verify-integrity` says anything about them: retention and
  destruction belong to the database operator's policy;
- restoring one is a disaster-recovery action, never a rollback (see "Upgrade and
  rollback"): a restored dump would also rewind Agent Receipts and Purge
  barriers.

Moving a SQLite deployment onto this backend is one offline command; its
prerequisites, operator sequence, abort rule and downtime budget are in "Storage
Backend Migration (issue #257)" under "Upgrade and rollback", and the measured
runtime is recorded in [release qualification](release-qualification.md).

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
[docs/design/logging.md](design/logging.md) for the field vocabulary and levels.

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
([ADR 0008](adr/0008-offline-server-backup.md), issue #194):

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

## Agent Store recovery

`platpulse-agent run` drains the previous Boot before it starts new
collection. A leftover Closing report that no longer belongs to the current
`agent_state` -- a superseded Boot, an advanced `report_sequence`, or a stale
Agent Epoch -- is quarantined automatically, so a self-healing Store no longer
stalls startup in a restart loop. Only a receipt whose full
`(agent_epoch, boot_id, report_sequence)` compare-and-swap key still matches
stays fatal.

If the automatic drain cannot proceed, use the offline recovery command rather
than editing the Agent database by hand:

```bash
# The command refuses while a running Agent holds the runtime lock.
sudo systemctl stop platpulse-agent
platpulse-agent recover --config /var/lib/platpulse-agent/agent.toml
# Review the stale Closing reports it reports, then quarantine them:
platpulse-agent recover --config /var/lib/platpulse-agent/agent.toml --drop-stale-closing
sudo systemctl start platpulse-agent
```

`recover` always writes a consistent `VACUUM INTO` backup beside the state
database before it changes anything; a run without `--drop-stale-closing` only
backs up and diagnoses. Quarantining a `reports` row cascades to
`report_sample_assignments`, returning those samples to the re-assignable pool
(issue #163).

## systemd services and offline backups

Install the binary and WebUI tree using the package manager or release archive.
The checked-in units under `release/systemd/` run Server and Agent as separate
dedicated users, apply a strict filesystem sandbox, and leave service enabling
to the operator. Copy the example configuration, create same-user-owned secret
files with mode `0600`, initialize the Server, then enable the selected unit.

**A backup job must not run against a running Server.** Every
offline Server CLI command that opens the database (`init`, `owner create`,
`viewer create`, `network create`, `agent create-enrollment-token`,
`backup`, and `restore`) now takes the same exclusive ownership guard the
serving process holds, *before* SQLite opens. In a non-development deployment
the command fails with an explicit stopped-Server requirement instead of
opening -- or modifying -- a database the Server owns (issue #160, following
#137). Stopping the Server releases the guard, and a refused or failed command
releases it as it exits. Development mode is the deliberate exception for these
offline maintenance commands: local tooling and the browser e2e harness attach
to a running dev Server, so they do not take the guard there. `serve` always
holds the guard, and `restore` is always a stopped-Server operation, in every
mode.

That guard makes the independent CLI safe, not scheduled-safe: it neither stops
nor restarts the Server. [ADR 0008](adr/0008-offline-server-backup.md) retires
the in-process online backup and **the package ships no backup timer**: creating
a Backup Artifact is an explicit operator action inside an Offline Backup
Window, and the operator owns the stop → backup → start orchestration,
including starting the Server again when the backup fails. Apply the same rule
to user-level systemd (`systemctl --user`) units and cron jobs: use the same
state, backup, and secret paths as the service, run with `UMask=0077`, keep the
backup directory private, and wrap the command so the Server is started again
on every failure path. Do not copy the system units' users or paths blindly
into a user service.

### Node process selectors and supervisor authorization

A Node's optional `[nodes.process]` selector is the only source of process
identity; the Agent never guesses it from a name, command line, or RPC port.
Without a selector the process component stays Disabled while RPC and chain
collection continue. Three forms are supported:

```toml
[nodes.process]
kind = "systemd_unit"
unit = "platon-validator.service"
```

```toml
[nodes.process]
kind = "pid_file"
path = "/run/platon-validator.pid"
```

```toml
[nodes.process]
kind = "supervisor"
program = "platon-validator"           # "group:process" when numprocs > 1
```

The `supervisor` form runs `supervisorctl pid <program>` as the Agent user, so
that account must be able to execute `supervisorctl` and connect to the
supervisor control socket (typically `/var/run/supervisor.sock` or
`/run/supervisor/supervisor.sock`). Grant access through supervisor's own
`[unix_http_server]` `chmod`/`chown` settings instead of broad filesystem
permissions. If the socket lives in a directory hidden by the unit's sandbox
(`PrivateTmp=true`), point `[supervisorctl] serverurl` at a visible path and
add that path to the unit's readable paths.

Missing `supervisorctl`, a non-zero exit, a non-numeric PID, and a `0` PID are
distinct typed collection errors: the component keeps its last-good value with
an explicit error and is never rendered as `0` or Healthy.

### Offline backups (no packaged automation)

[ADR 0008](adr/0008-offline-server-backup.md) makes creation and restore
offline, stopped-Server operations and **deletes the packaged
`platpulse-backup.timer` / `.service` pair**. The default deployment has no
automatic backup: `platpulse-server backup` is an explicit operator command run
inside an Offline Backup Window, and the operator owns its schedule.

`[backup_schedule]` is removed with the in-process scheduler. A `server.toml`
that still contains the section is rejected at startup with a dedicated error;
delete the section before upgrading. `backup_dir` remains, and the optional
`backup_required_mount` re-arms the layout guard:

```toml
backup_dir = "/data/platpulse-backups"
backup_required_mount = "/data"   # optional; enforced when set
```

- `platpulse-server backup` holds the exclusive ownership guard and refuses
  while a Server owns the database, so it can only run with the Server stopped.
- When `backup_required_mount` is set, creation fails closed unless the backup
  directory lives under that mount, the mount is a distinct real filesystem,
  and the directory is not on the live database filesystem. An absent or
  unmounted disk produces no artifact instead of writing beside the database.
  Restore is deliberately not gated by it: a destination-layout mistake must
  never block recovery.
- Creation runs one bounded redaction pass. Verifying an existing artifact is a
  separate read-only step (Admin `backup_verify`) and still scans it
  independently.
- A killed or disk-full attempt can leave a `platpulse-*.db.part` (and a
  `.part-journal`) behind. Nothing reclaims it automatically: remove it
  manually, and confirm no backup is running before deleting anything. Doctor
  reports the residue count and the age of the last successful artifact.
- Retained artifacts are never pruned automatically; retention and off-host
  copies remain operator policy.
- Place `backup_dir` on a distinct disk where possible, keep the directory
  private (`0700`), and never let a missing mount fall back to the database
  disk. A second local disk is not an off-host backup.
- The `report_receipt_body` retention family slims receipt bodies older than a
  fixed 30 days — never the identity rows — so a whole-database backup stops
  scaling with total receipt history. It runs only when a retention run is
  triggered, so schedule one periodically rather than only when reclaiming
  disk.

The offline command writes restrictive artifacts to the configured `backup_dir`
(the example uses `/var/backups/platpulse`), separate from Server state. If
`db_path` or `backup_dir` is changed, add the same paths to a systemd drop-in
for `ReadWritePaths`. Restore remains an explicit, stopped-Server operation
using the documented `platpulse-server restore` flow; never restore by copying
a live database or its WAL/SHM sidecars.

### Safe daily backups, verification, and restore rehearsal

- **Use the offline path.** [ADR 0008](adr/0008-offline-server-backup.md)
  retires the in-process online schedule and the packaged timer, so create
  and verify only in an Offline Backup Window with the Server stopped, or
  from a filesystem/volume snapshot.
- **You own the schedule; orchestrate the stop.** No packaged timer exists.
  The `platpulse-server backup` command refuses while a running Server owns
  the database, so a job that fires against a live Server fails closed
  instead of opening the file. Write a wrapper that stops the Server, runs
  the backup, and always starts the Server again, even when the backup fails:

  ```bash
  #!/bin/sh
  set -u
  systemctl stop platpulse-server
  status=1
  platpulse-server backup --config /etc/platpulse/server.toml && status=0
  systemctl start platpulse-server
  exit "$status"
  ```

  Use the same pattern with `systemctl --user` and the user service names for a
  home deployment, and with `cron` only if the wrapper restarts the Server on
  every failure path. Never schedule the raw `platpulse-server backup` command.
- **Verification is not creation.** A successful `backup` writes a sanitized,
  fsync'd, atomically renamed artifact plus a registry manifest; that does not
  prove the artifact restores. Creation performs one bounded redaction pass
  and records the source integrity result. To verify an artifact afterwards,
  use the Admin `backup_verify` Operation or verify it in the same Offline
  Backup Window: compare the artifact's SHA-256 with the `backup_artifacts`
  manifest, run a read-only integrity check, and confirm the recorded schema
  is not newer than the running binary.
- **Rehearse an isolated restore.** At least once per release, and after any
  hardware or path change, copy the state directory and the artifact to an
  isolated scratch location, point `backup_dir` in the copied configuration at
  the copied artifacts, and run `platpulse-server restore --config
  <scratch>/server.toml --artifact-id <id> --yes` against the copy. Then start a
  scratch Server on a loopback port and confirm `/health/live`,
  `/health/ready`, and a data refetch. The copied `backup_artifacts` registry
  supplies the artifact identity, and the production state is never opened.
  Retain corruption evidence and known-good rollback copies until recovery and
  verification are complete.

For local development, use an explicit `development = true` configuration and
loopback `listen`; do not reuse the development cookie policy in production.
Non-loopback plaintext listeners remain refused until TLS or an explicitly
trusted HTTPS reverse proxy is configured.

## Redeploying a running source checkout

A Server that runs the binaries directly out of `target/release` reads
`dist/index.html` once at startup and then serves the hashed assets beside it
from disk. Rebuilding the WebUI replaces those hashed files, so a Server left
running keeps handing out an `index.html` that references assets which no
longer exist and the browser renders a blank page. Rebuilding the WebUI without
restarting the Server therefore breaks the WebUI even though every artifact on
disk is correct.

`scripts/deploy-local.sh` rebuilds the Agent/Server binaries and the WebUI, then
restarts the user-level services and proves the live Server resolves every asset
the current build references:

```bash
scripts/deploy-local.sh                 # rebuild, restart if anything changed
scripts/deploy-local.sh --skip-tests    # skip WebUI lint/typecheck/unit tests
scripts/deploy-local.sh --timeout 300   # allow a slower /health/ready budget
scripts/deploy-local.sh --check-only    # report whether the live Server is stale
```

The readiness budget defaults to 180 seconds because a cold start replays the
migrations and opens the projections before the listener binds, which takes over
a minute on a populated database; a tighter budget reports a failed restart while
the services are already healthy. Lower it with `--timeout` only when the state
directory is known to be small.

The restart decision is not limited to "did this run change something": the
script also probes the running Server and restarts it when the live `index.html`
disagrees with `dist/index.html`, or when any asset it references is missing.
That self-heals the stale state above regardless of what rebuilt the WebUI. Run
`--check-only` in monitoring or before a manual rebuild; it exits non-zero when
the live Server is stale. The script never modifies SQLite state, the Owner
account, the enrollment credential, the pepper, or the service configuration.

## Supported release set

The supported release builder is:

```bash
scripts/build-release.sh --target x86_64-unknown-linux-gnu --output target/release-artifacts
```

It produces versioned Server and Agent Linux `x86_64` archives. The release CI runs
that command for both `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`
(the package labels are Debian `amd64`/`arm64` and RPM `x86_64`/`aarch64`). The
artifacts use the Ubuntu 24.04 build baseline and therefore require glibc 2.39 or
newer. They are tested on Ubuntu 24.04 and Fedora 41; Debian 12 and RHEL 9-class
systems are not supported by these GNU-linked packages. The target toolchain and
linker must be installed before requesting an architecture build. `dpkg-deb` and
`rpmbuild` outputs are generated when those builders are available; the build
reports an explicit unavailable status otherwise.

Native archives and packages include the repository `LICENSE` in their package-specific
documentation directories. Each Server archive includes the same-origin WebUI, non-root
systemd units, the Caddy and Compose examples, and the optional MaxMind `geoipupdate` example.
Agent archives include the Agent unit and configuration
reference. Packages install dedicated `platpulse-server` and `platpulse-agent`
system users, create their private state directories plus the Server backup and
`/etc/platpulse/secrets` directories with runtime-user ownership and mode `0700`,
and never enable a service automatically. DEB and RPM scriptlets declare their
`adduser`/`shadow-utils`, `coreutils`, `libc`, and `systemd` runtime dependencies.
Release builds export `SOURCE_DATE_EPOCH` through tar, package, and SBOM generation;
Syft is required for a releasable dependency-aware SPDX SBOM. Fixture and harness
runs may explicitly emit only a non-releasable SBOM-skipped marker.

## OCI deployment and mount model

The OCI build definition is `release/oci/server.Dockerfile`. Its Node, Rust, and
Debian base images are pinned by manifest digest, and it uses no mutable apt
repository resolution. Update those pins only as a reviewed release-input change.
It runs as fixed UID and GID `10001`, declares separate volumes for SQLite state, backup artifacts, secret
files, and optional Geo data. WebUI assets remain in the versioned image layer by
default; operators may explicitly bind-mount a replacement WebUI tree, and the image
does not create an anonymous WebUI volume that could survive an image upgrade. It does
not contain live state or credentials. The
Compose example is `release/compose/server.compose.yml`; copy the accompanying
`release/compose/server.toml` beside it as `server.toml`. It binds inside the
container on `0.0.0.0`, trusts only the pinned Compose subnet for the host HTTPS
reverse proxy, and mounts the Geo database below `/var/lib/platpulse/geo`. Mount
the prepared config and secret directory read-only. Secret files must be regular files owned
by UID `10001` with mode `0600`; bind mounts do not relax the Server's no-symlink
or same-user ownership checks. The image's WebUI is used unless an operator
deliberately mounts a replacement `/usr/share/platpulse/web` tree.

The Geo sidecar example in `release/geo/geoipupdate.compose.yml` uses the official
MaxMind image and operator-provided secrets. PlatPulse does not distribute a
GeoLite database, MaxMind credentials, or a downloader configuration as a
runtime secret. Review MaxMind licensing and provide the resulting MMDB through
the read-only Geo mount.

## Release validation and metadata

`scripts/validate-release.sh` rejects missing executables/WebUI assets, unexpected
or non-regular members, empty directory additions, symlinks, unsafe archive paths,
secret names, SQLite sidecars, MMDB data, Agent state, non-canonical file/directory
modes, and root-running service units. The release
candidate harness runs the unpacked Server archive through the external CLI,
HTTP, WebUI, metrics, AgentReport, and SSE boundaries. Package-manager install
smoke tests run in the release CI's disposable package environments.

Every release set contains `SHA256SUMS`, an SPDX inventory, and recorded Rust/npm
audit evidence. Normal release builds fail when cargo-deny, cargo-audit, or npm audit
is unavailable or exits non-zero; only fixture/harness builds use the explicit audit
skip. `RUSTSEC-2023-0071` is ignored because `rsa` is lockfile-only behind disabled
SQLx features (`cargo tree -i rsa` is empty). `RUSTSEC-2026-0253` is ignored because
the current `lru` release has no fixed upgrade; `RUSTSEC-2024-0436` (`paste`) is
ignored only by cargo-deny because Alloy's proc-macro graph has no maintained
compatible replacement yet. `RUSTSEC-2026-0381` (`webpki-roots` 0.26.11, the
bundled root store Alloy's WebSocket transport pulls through
tokio-tungstenite) is ignored by both gates: the advisory patches only the 1.0
line, every published tungstenite release through 0.30 still requires `^0.26`,
and the workspace's own 1.0 root store is on the patched release. See the
residual-risk entry in the security review. The remaining cargo-audit warnings stay visible in
the evidence for release signoff. Checksums and SBOMs are
integrity and inventory metadata only;
they are not artifact signatures. Until an artifact-signing workflow is added,
unsigned artifacts must not be described as a verified supply chain.

The checked-in deployment assets are:

- `release/systemd/` — Server and Agent units;
- `release/examples/Caddyfile` — trusted reverse-proxy example;
- `release/compose/server.compose.yml` and `release/compose/server.toml` — non-root Server Compose example and matching container configuration;
- `release/geo/geoipupdate.compose.yml` — optional Geo sidecar example.

## Upgrade and rollback

Before upgrading, stop the Server, take a backup with the packaged `backup` command, and copy the backup plus the pepper and TLS secret files to protected storage. Verify the release `SHA256SUMS` file and keep the previous binary/archive available. Start the new Server and confirm `/health/live`, `/health/ready`, migrations, and the Admin audit surface before returning traffic.

If readiness or a post-upgrade smoke check fails, stop the new process, restore the previous matching Server/Agent artifacts, and start the previous version against the unchanged state directory. Do not delete or downgrade the database in place: a schema migration is forward-only. If the new version has already migrated the database, restore the pre-upgrade backup into a fresh state directory, restore the pepper and secret files with their private permissions, and validate the restored instance before switching the service back. Keep the failed release logs and recovery-rehearsal evidence for incident review.

A Node build upgrade is a separate operation from a Server upgrade: the Agent stamps each Block Summary with the identity its own Node reports, so a Node whose build changes the identity it answers with keeps producing rejected samples. When a Node's binary or network configuration changes, follow "Node Network Identity at onboarding and after a Node build upgrade" at the end of this document.

### Coordinated Inventory v2 cutover (issue #192)

The Server-managed Inventory Revision protocol (v2) replaces the Agent-supplied inventory_revision. Migrating an existing frozen-v1 deployment is a coordinated offline operation with one explicit switch gate; it is not a rolling upgrade, and new and old protocols must never run mixed.

Not switched yet (still frozen v1):

- inventory_migration is absent: nothing to do; the deployment keeps the v1 baseline.
- inventory_migration is present but inventory_cutover is not: the database is a converted deployment that has not been authorized. The Server serves read-only diagnostics only, refuses ingestion, Admin mutations and every external-effect worker, and /health/ready reports the inventory_cutover component with reason cutover_not_resumed. Do not point the previous binary at this database: a schema migration is forward-only.

Switched (v2 active):

- inventory_cutover records state = resumed. Ordinary Reports must use /api/agent/v2/reports; the frozen v1 route returns the original retained Receipt only for an identical report identity and byte hash. An unknown v1 Report is refused with report_not_replayable and causes no write. Restoring the coordinated checkpoint is no longer a software rollback.
- The frozen v1 route still enforces normal authentication, Agent ownership and input-size limits; replay never bypasses Agent Removal.

Operator sequence (each step runs with the Server stopped; never point the old binary at the converted database):

1. platpulse-agent prepare-upgrade, then platpulse-agent checkpoint create: stop ordinary collection and drain the immutable backlog through the final Closing (issues #189/#190).
2. platpulse-server checkpoint create --config server.toml --agent-checkpoint AGENT_DIR --output CHECKPOINT: preserve the exact Server/Agent state and re-verify the preparation.
3. platpulse-server checkpoint convert --checkpoint CHECKPOINT --output CONVERTED: write the converted deployment; this does not authorize a switch.
4. platpulse-server checkpoint verify-conversion --checkpoint CHECKPOINT --converted CONVERTED: prove both halves offline without starting a worker.
5. platpulse-server cutover status --config CONVERTED/server/server.toml: must report awaiting_resume.
6. To abort before the switch, run platpulse-server cutover rollback --checkpoint CHECKPOINT --restore-dir RESTORE --converted CONVERTED and start the old binaries against the restored state. The command refuses once the cutover has resumed.
7. platpulse-server cutover resume --config CONVERTED/server/server.toml --checkpoint CHECKPOINT --converted CONVERTED: pass the gate. It re-verifies the checkpoint and the conversion, refuses a changed or inconsistent participant, and records the durable cutover marker.
8. Start the new Server with the converted server.toml and the Agent with the converted agent.toml. Confirm /health/ready, that the first v2 Report completes the existing DrainedPrevious transition without changing the preserved revision, and that the Admin Inventory diagnosis reports protocol v2.

Failure diagnosis: cutover status distinguishes not_converted, awaiting_resume and resumed. A missing transcript, a mismatched hash or revision, a changed Boot/Closing identity or a failed offline verification leaves the deployment at awaiting_resume and refuses the switch; it never silently adopts a new baseline. After the switch, repair forward: do not restore an older backup as a routine rollback, because that may discard post-cutover declarations, Purge barriers and other writes.

### Storage Backend Migration (issue #257)

Moving an existing SQLite deployment onto the PostgreSQL Storage Backend is one
offline, one-way command ([ADR 0013](adr/0013-postgresql-server-storage-backend.md)):
`platpulse-server storage migrate` copies the whole database — Agent identity,
credential digests, Report Receipts and Purge barriers included, unsanitized —
into an empty PostgreSQL database. There is no PostgreSQL → SQLite direction:
once the PostgreSQL Server has accepted a report, Agents have applied the
Receipts, emptied their Durable Spool and advanced their sequence and Inventory
Declaration Records, so replaying the old file backwards would break ordering,
Receipt and Purge-barrier evidence.

The copy streams every table out of the stopped SQLite deployment and inserts it
inside one target transaction, then reads each table back and compares per-table
row counts and content digests against the source. The transaction commits only
when every check matches; any failure rolls the target back to empty, so a
corrected cause can simply be retried. The command prints per-table progress with
an estimate, and after the commit it advances every identity sequence past the
migrated maximum, so the first insert after the migration cannot collide.

Before starting, all of the following must hold:

- the Server is stopped. The command claims both sides itself — the SQLite
  deployment's ownership guard then `EXCLUSIVE` locking, and the PostgreSQL
  target's session advisory lock — and refuses if either is held;
- the source passes `integrity_check`, matches this binary's schema version, and
  is not an Inventory conversion still waiting for its cutover resume
  (`cutover status` reports `awaiting_resume`);
- the target database exists, is owned by the configured role with a `"C"`
  collation, and is **empty**: no tables at all, or only the ones this command
  created on an earlier attempt. The command applies the PostgreSQL baseline
  itself. A database that ever served is refused, because merging two histories
  into one target is not a migration.

Operator sequence:

1. Stop the Server, then take the packaged backup of the SQLite deployment
   (`platpulse-server backup`) and copy it, the pepper and the TLS secret files
   to protected storage. The source file is never modified, so it stays the
   fallback point.
2. Create the empty target database and the role that owns it, for example:

```bash
sudo -u postgres psql -v ON_ERROR_STOP=1 -c "CREATE ROLE platpulse LOGIN" -c \
  "CREATE DATABASE platpulse OWNER platpulse TEMPLATE template0 ENCODING 'UTF8' LC_COLLATE 'C' LC_CTYPE 'C'"
```

3. Write the target `server.toml`: `[storage] backend = "postgres"` with the
   `[storage.postgres]` section, the same `pepper_file`, `backup_dir` and TLS
   settings, and no `db_path` (see "Storage Backend" above).
4. Run the migration with one `server.toml` on each side:

```bash
sudo -u platpulse platpulse-server storage migrate \
  --source /etc/platpulse/server.toml \
  --target /etc/platpulse/postgres.toml
```

5. Start the Server with the target configuration and confirm `/health/ready`,
   the Admin dashboard and a fresh AgentReport before returning traffic.

A successful migration leaves the SQLite file untouched and writes a sibling
marker next to it (`/var/lib/platpulse/server.db.migrated`), which records the
target, the time, the table/row counts and the content digest. While that marker
exists, the SQLite backend refuses to start and any command that opens that
database file refuses — it is the fallback point, not a serving database. The
marker is taken before the first row is copied, so a run that is interrupted
leaves an in-progress marker that fences the file the same way; its failure also
rolls the target back, which leaves the command retryable.

Rollback before the switch: `platpulse-server storage migrate abort` undoes the
migration while the PostgreSQL target has accepted no write since the baseline
recorded at commit — no report, no Admin mutation, no background write. The two
`server.toml` paths describe the Storage Backend Migration itself, so they are given once,
before the `abort` mode:

```bash
sudo -u platpulse platpulse-server storage migrate \
  --source /etc/platpulse/server.toml \
  --target /etc/platpulse/postgres.toml \
  abort
```

It empties the target, removes the marker, and the SQLite deployment may serve
again. The command compares two witnesses against the baseline: the content
digest of every table, and the identity sequences, which advance outside the
transaction that consumes them so a write that rolled back still leaves a
trace. An interrupted run is aborted the same way — it left no baseline, so the
command refuses unless the target holds nothing but the reference rows its own
migrations insert, because rows it did not copy are not its to delete. A write
that leaves every table byte-identical and consumes no identity (a no-op
`UPDATE`, or one a later statement undid) is not evidence these two witnesses
can see, so the abort proceeds: it stored nothing, and the SQLite file it hands
back has lost nothing. Once any
business write has landed on PostgreSQL, abort is refused (it names the write
it found) and the fix is forward on PostgreSQL — restore into a
fresh state only as a disaster-recovery action, exactly like the Inventory
cutover rule above. Turning an abort down is deliberate: a PostgreSQL Server that
already advanced Agent Receipts and Purge barriers cannot be replaced by the
older file without losing that evidence.

Downtime: the window is offline only, so Agents keep spooling while the copy
runs. A full Durable Spool drops its oldest historical reports into History Gaps,
so the window must stay shorter than the spool's capacity; a retention-heavy
deployment should measure the runtime on a copy of its own data before the real
cutover. The measured run recorded in
[release qualification](release-qualification.md) covers the range of a
near-production deployment: budget the whole window, add the start-and-verify
step, and leave margin for the estimate the command prints as it goes.

## Node Network Identity at onboarding and after a Node build upgrade

A Network's identity is its Registry tuple: genesis hash, chain ID, P2P network ID, and address HRP. The Agent stamps every Block Summary with the identity tuple its own Node reports, and the Server compares that stamp with the Registry row for the Node's Network key. A sample whose stamp differs is terminally rejected with `network_identity_mismatch` ("Block network identity does not match the registered Network"); the rejected interval is a History Gap that is never merged, re-labelled, or backfilled.

Confirm a Node's own identity before adding it to a Network, and again after any Node build upgrade:

- `eth_chainId` must answer the chain ID registered for that Network — never a value borrowed from another chain. On PlatON mainnet that is 210425 (`0x335f9`), and a pre-PIP-7 build answers the legacy `100` instead; PlatON's own testnet is a different chain (210429, legacy `104`). Take the expected values from this Network's Registry row, not from a generalised example.
- `admin_nodeInfo` must report the same PIP-7 chain ID, while the legacy `chainId` field may still report the pre-PIP-7 value.
- `platon version` must be at or after commit `45c3e086d`, the PIP-7 chain-ID fix. Builds without it (for example `v1.5.1-unstable-34e50429-20251023`) still answer `eth_chainId` with `100`.

Symptoms of a mismatching Node: Admin → Home raises the critical Network Identity Mismatch attention item (`node_identity_mismatch`); Admin → Networks and the Node page show the Node as Mismatched with the contradicting field names and the observed tuple beside the Registry tuple; every Block Summary from that Node is rejected, so the Blocks and Txs of its heights stay unknown. The mismatch is repaired on the Node side, never in the Registry:

1. Correct the Node: upgrade its binary to a build containing the chain-ID fix, or correct its network configuration, and restart it.
2. Confirm the Node now answers the Registry tuple — repeat the checks above — and let the Agent's collection run at least once afterwards (`collection_interval_seconds`), so a Report carrying the new identity is persisted, before you touch the Agent. A restart taken while the Agent still holds the pre-upgrade identity is not harmless: its Block workers stamp samples from the identity in the most recent persisted Report (`load_last_report` in `crates/platpulse-agent/src/collector.rs`), those workers start before the Report collection loop (`crates/platpulse-agent/src/cli.rs`), and a graceful stop persists last-good observations without probing the Node (`crates/platpulse-agent/src/shutdown.rs`). Without a successful post-upgrade collection the restarted Agent can keep the stale stamp for a whole subscription and extend the permanent gap.
3. Restart that Host's Agent (`sudo systemctl restart platpulse-agent`, or the matching `systemctl --user` unit), then verify the outcome instead of assuming it: the Node must leave the Mismatched state, the `node_identity_mismatch` alert must clear, and its new heights must stop arriving as rejected samples. If samples are still rejected, wait one `collection_interval_seconds` so a fresh Report is persisted, then restart the Agent once more.
4. Do not edit the Registry tuple to match a Node (Admin → Networks → Edit tuple, or `PUT /api/admin/v1/networks/{network_key}`). The observed tuple is the Node's own claim; matching it would contradict every other Node on the same Network, and it reclaims no already-rejected Block History.

### When the observation matches but the Block history is still rejected

A second shape of the same failure is the one that reads as a contradiction (issue #235): the Node's observed identity **matches** the Registry, Admin → Home shows the `node_identity_mismatch` attention item as resolved, and yet every Block Summary from that Node is still terminally rejected, so its Blocks and Txs stay Unknown and new History Gaps keep arriving. Nothing is wrong with the Node or the Registry — the **Agent's Block Identity Stamp** is stale.

The Agent takes that stamp from the most recent persisted Report when its Block Worker subscribes to the Node (`load_last_report` in `crates/platpulse-agent/src/collector.rs`) and reuses it for the whole subscription. A Node rebuild, reconfiguration, or binary swap does not necessarily fail the Agent's own RPC connection, so that subscription — and with it the stamp — can outlive the identity change; the Server, which only ever compares the stamp it is given, keeps refusing samples that carry the old tuple. The stamp is refreshed when the Agent re-subscribes or restarts, never by anything the Server or the Registry does.

Where the Server says so: Admin → the Node's detail page → Network identity renders a **History rejected** diagnosis that states the refusal in the Server's own words — every differing field with its stamped and registered value, which side was found stale (the Agent's stamp, or a genuine cross-Network conflict because the current observation contradicts the Registry as well), the refused height range, and the most recent refusal time. A stamp that carries no address HRP at all reads as `not reported`, never as a value the Registry might hold. That diagnosis is built only from the Server's own refusal rows (`block_history_gaps` where `kind = 'server_rejected'` and `authored_by = 'server'`); a History Gap an Agent reports about itself is stored as the Agent's own evidence and can never present itself as the Server's diagnosis. The same reason text is stored on the History Gap row, so it survives in a snapshot and in the audit trail, and it is what the Node's Receipt reports as the refusal, so the two surfaces always name the same cause. Each sample is judged on the stamp it was collected with, so a Report spooled across a subscription boundary reports each sample's own differing fields. A diagnosis that names the current observation as contradicting too is not a stale stamp: repair it with the mismatch steps above.

1. Restart that Host's Agent (`sudo systemctl restart platpulse-agent`, or the matching `systemctl --user` unit). That is the only repair: the Node is already correct.
2. Verify the outcome instead of assuming it: the Node's new heights must stop arriving as rejected samples and it must start producing accepted Block Summaries again, so its Blocks and Txs stop being Unknown. The History rejected diagnosis clears itself once the Server accepts a Block Summary again — it names the newest refusal only while that refusal is later than the newest accepted summary. That acceptance evidence is kept on the Node's durable history state (`block_history_state.last_accepted_summary_at`), so the diagnosis cannot come back for a Node that already recovered once raw Block Summaries age out of retention.
3. Accept the loss: the already-refused interval stays a permanent History Gap. Those heights are never merged, re-labelled, or backfilled, and the Registry tuple is still never edited to match a stamped sample (step 4 above).

### When a Node answers millisecond timestamps

A third shape of "the Block history is rejected" never reaches the Server at all: the Agent cannot assemble a Report, exits non-zero, and its unit restarts it in a loop. The journal shows the same fatal line about a second after every start:

```text
Agent report collection failed: Collection("invalid persisted Agent identity: field block.block_timestamp_ms is outside its contract range")
```

The value that line names is a **millisecond** timestamp read as seconds (issue #243). PlatON native headers carry `header.Time` in milliseconds, and the `eth_*` aliases the Agent uses (`eth_getBlockByNumber`, `eth_subscribe`) divide it by 1000 only when the Node runs with `--http.ethcompatible`. A Node started without that flag hands the Agent a raw millisecond value such as `1792034712000`, which an Agent older than this fix multiplied by 1000 again and stored as `1792034712000000` — far beyond the AgentReport v1 contract limit of `4102444800000` ms (2100-01-01T00:00:00Z). Every Report assembled from that stored sample is refused, so collection can never succeed and the restart loop repeats. Current Agents refuse such a sample at the persistence boundary with an actionable message that names the limit and the flag, skip an already-stored one when loading samples, and sweep the Store once at startup, so the loop cannot start. The unit is read from the value itself: `100000000000` or more is already milliseconds (that magnitude read as seconds would be the year 5138), anything smaller is seconds.

Run every PlatON Node with `--http.ethcompatible`, and confirm the flag and the unit before you touch the Agent:

```bash
# The flag must be among the Node's own arguments.
ps -o args= -C platon | tr ' ' '\n' | grep -x -- '--http.ethcompatible'

# The timestamp must be seconds: 10 hex digits, for example 0x6ad04a70
# (1792035440), never a millisecond value such as 0x1a13d97a9c0 (1792034712000).
curl -s -X POST -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"eth_getBlockByNumber","params":["latest",false]}' \
  http://127.0.0.1:6789 | head -c 400
```

Then repair in this order:

1. Restart the Node with `--http.ethcompatible` (or with the flag its Network's Nodes are expected to carry), and confirm the timestamp above is now seconds.
2. Restart that Host's Agent (`sudo systemctl restart platpulse-agent`, or the matching `systemctl --user` unit). On startup it sweeps its own Agent Store, removes every Block Summary whose chain timestamp is outside the contract, and declares what it removed as a History Gap. It logs one line per affected Node, followed by the number of ranges it declared:

```text
Purged 3 Block Summary sample(s) outside the AgentReport v1 contract (0195f2a1-0014-4014-8014-000000000014: 3), declared as 1 History Gap range(s). Check that every Node runs with --http.ethcompatible.
```

   The sweep is also the repair for a Store poisoned before the Agent learned to refuse such a sample, so a hand-written `DELETE` is only needed on an Agent build older than this fix. The sweep covers the Nodes this Agent declares in its Inventory: rows of a Node that is no longer declared are left for a later start, because a History Gap naming an undeclared Node is refused by the contract. Each Node's consecutive removed heights become one `unrecoverable_backfill` History Gap per contiguous range in its `history_gaps` (a run longer than the 1,000,000-height contract limit is split so the declaration itself stays inside the contract), reported to the Server so those heights are never presented as continuous history. **Nothing refills them automatically:** the Agent resumes recovery at the height recorded for that Node and never schedules a lower one again, and the Server discards a replay of heights below its own high-water mark. Treat the declared gap as the permanent record it is, and fetch those Blocks out of band from the Node while it still retains them if they matter.
3. Verify the outcome instead of assuming it: the Agent stays up (`systemctl status platpulse-agent`) and its next Reports are accepted instead of failing validation. Read-only inspection of the Agent Store (the configured `state_db`, packaged default `/var/lib/platpulse-agent/agent.db`) needs the Agent stopped, as the Store is written under WAL with exclusive locking:

```bash
STATE_DB=/var/lib/platpulse-agent/agent.db   # the Agent config key state_db, Agent stopped
sqlite3 "$STATE_DB" \
  "SELECT node_id, COUNT(*) AS samples, MIN(block_timestamp_ms), MAX(block_timestamp_ms)
     FROM block_summaries
    WHERE block_timestamp_ms > 4102444800000
    GROUP BY node_id;"
```

   A row that still shows up after a restart of the current Agent means the row was written after the sweep (check the Agent version), or the Agent is not reading the `state_db` you queried.

Read-only verification. The packaged units keep the Server database at `/var/lib/platpulse/platpulse.db` (see the configured `db_path`). Query a **consistent snapshot**, never the live file: either the artifact written by the packaged `platpulse-server backup` command in an [Offline Backup Window](#offline-backups-no-packaged-automation) (the Server stopped, as that command requires), or a copy of `db_path` taken while the Server is stopped. The live database is written under WAL with exclusive locking, so opening it directly under the `immutable` URI bypasses that synchronisation — it can miss not-yet-checkpointed WAL content, and under concurrent writes it can even fail with `database disk image is malformed`. A live `immutable` read is a best-effort eyeball, never evidence:

```bash
# Packaged `backup` artifact, or a copy of db_path taken while the Server was stopped.
SNAPSHOT=/var/backups/platpulse/<backup-artifact>

sqlite3 "$SNAPSHOT" \
  "SELECT n.display_name, o.network_chain_id, o.network_p2p_network_id,
          o.network_address_hrp, o.network_genesis_hash
     FROM nodes n
     JOIN current_node_chain_observations o ON o.node_id = n.node_id
    WHERE n.network_key = 'REPLACE_WITH_NETWORK_KEY';"

sqlite3 "$SNAPSHOT" \
  "SELECT network_key, chain_id, p2p_network_id, address_hrp, genesis_hash FROM networks;"

# The refusals one Node's stamp produced, and the Server's own newest reason for them.
sqlite3 "$SNAPSHOT" \
  "SELECT kind, MIN(from_height) AS first_height, MAX(to_height) AS last_height,
          COUNT(*) AS gap_rows, MAX(created_at) AS latest
     FROM block_history_gaps
    WHERE node_id = 'REPLACE_WITH_NODE_ID'
    GROUP BY kind;"

sqlite3 "$SNAPSHOT" \
  "SELECT reason FROM block_history_gaps
    WHERE node_id = 'REPLACE_WITH_NODE_ID' AND kind = 'server_rejected'
      AND authored_by = 'server'
    ORDER BY created_at DESC, from_height DESC LIMIT 1;"
```

A Node's `display_name` is assigned when the Node is first accepted; a later Inventory declaration does not rename it, so rename the Node through the Admin Node metadata action (`PUT /api/admin/v1/nodes/{node_id}/metadata`).



