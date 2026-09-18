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
not ready with reason `web_assets_missing`. Readiness evaluates six components:
`sqlite`, `owner`, `web_assets`, `shutdown`, `critical_workers`, and `corruption`;
any `not_ready` component makes the endpoint return HTTP 503. `/health/live`
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

After the startup integrity check, a Server-owned monitor runs
`PRAGMA integrity_check(1)` immediately and every six hours. The `(1)` limits
the number of reported errors, not the tables examined. Scans use the owning
SQLite connection, never an external connection to a live EXCLUSIVE database.
They are not triggered by public health requests. The cadence is deliberately
low: the scan holds the only SQLite connection for its whole duration, so a
deployment-sized database (measured: ~2.5 s for 1.16 GB) would otherwise stall
ingestion on every tick.

- Confirmed corruption is latched: the `corruption` and `sqlite` components
  report `integrity_check_failed`; later successful queries do not clear it.
  Recovery is an explicit operator procedure, followed by a restart that must
  pass startup validation. Liveness remains independent.
- A failed/interrupted scan or an integrity result older than six minutes makes
  `corruption` report `integrity_check_unavailable`, not healthy and not a
  diagnosis of corruption. The watchdog uses monotonic time.
- Detection is periodic, not instantaneous. The SQLite VM scan has a 60-second
  cooperative execution budget and observes shutdown. The sole connection can
  still delay ingestion for the scan duration; native progress callbacks cannot
  preempt a blocked kernel filesystem operation.
- Before rollout, validate scan duration and ingestion latency on a
  representative isolated database. A database that cannot complete within the
  budget will remain not-ready until a check can complete; do not interpret that
  as permission to delete data or disable integrity monitoring. Raise the budget
  in `http/health.rs` only after measuring, and never widen it silently.

Keep corruption evidence and known-good rollback copies until recovery and
backup verification are complete. Do not repair or replace files underneath a
running Server.

## systemd services and backup timer

Install the binary and WebUI tree using the package manager or release archive.
The checked-in units under `release/systemd/` run Server and Agent as separate
dedicated users, apply a strict filesystem sandbox, and leave service enabling
to the operator. Copy the example configuration, create same-user-owned secret
files with mode `0600`, initialize the Server, then enable the selected unit.

**Do not enable the shipped backup timer against a running Server.** Its
independent CLI must only open the database after the Server is stopped; it is
not an online-backup scheduler. The CLI ownership-guard audit is tracked in
[issue #160](https://github.com/mowind/PlatPulse/issues/160), following #137.
For online backups, use the authenticated Admin backup Operation inside the
owning Server process, or the Server-owned schedule below. A stopped-Server
schedule must serialize stop/backup/start and guarantee restart even when
backup fails; the packaged timer does not do that orchestration. Home-based
user services also need their own paths and permissions rather than blindly
copying the system units.

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

### Server-owned online backup schedule

`[backup_schedule]` makes the running Server create and verify its own daily
backup on the owning SQLite connection. It stores no Owner password or machine
token and never opens a second connection to a live database:

```toml
backup_dir = "/data/platpulse-backups"

[backup_schedule]
required_mount = "/data"
interval_hours = 24   # optional, default 24, minimum 1
```

- The first attempt runs two minutes after startup; it is deliberately not
  immediate so a restart never races ingestion with a `VACUUM`.
- A success is recorded only after checksum, read-only integrity, schema, and
  privacy verification pass. A failed auto-verification keeps the artifact and
  records the error; the schedule retries within an hour.
- `required_mount` fails closed: the backup directory must live under that
  mount point, on that mount device, and on a filesystem distinct from the
  live database. An absent or unmounted disk produces no artifact instead of
  writing beside the database.
- Old artifacts are never pruned automatically; retention and off-host copies
  remain operator policy.
- Every schedule result is durable in `server_settings` keys prefixed
  `backup_schedule_`, so a restart does not duplicate a recent success.

Place `backup_dir` on a distinct disk where possible, require the expected mount
before running a scheduled backup, and keep the directory private (`0700`).
Do not allow a missing mount to silently fall back to the database disk.
Creation is not verification: verify checksum/integrity/schema, rehearse an
isolated restore, monitor last successful backup and capacity, and retain
protected configuration/secret recovery material separately. A second local
disk is not an off-host backup.

The optional `platpulse-backup.timer` invokes `platpulse-server backup --config
/etc/platpulse/server.toml`. That command uses the same sanitized `VACUUM INTO`,
redaction, fsync, atomic-rename, and metadata path as the Admin backup Operation;
it writes restrictive artifacts to the configured `backup_dir` (the example uses
`/var/backups/platpulse`), separate from Server state. If `db_path` or `backup_dir` is changed, add the same paths to a systemd drop-in
for `ReadWritePaths`. Restore remains an explicit,
stopped-Server operation using the documented `platpulse-server restore` flow;
never restore by copying a live database or its WAL/SHM sidecars.

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
scripts/deploy-local.sh --check-only    # report whether the live Server is stale
```

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
systemd units, the Caddy and Compose examples, the optional MaxMind `geoipupdate` example, and the
backup timer/service. Agent archives include the Agent unit and configuration
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
compatible replacement yet. The remaining cargo-audit warnings stay visible in
the evidence for release signoff. Checksums and SBOMs are
integrity and inventory metadata only;
they are not artifact signatures. Until an artifact-signing workflow is added,
unsigned artifacts must not be described as a verified supply chain.

The checked-in deployment assets are:

- `release/systemd/` — Server, Agent, backup service, and backup timer;
- `release/examples/Caddyfile` — trusted reverse-proxy example;
- `release/compose/server.compose.yml` and `release/compose/server.toml` — non-root Server Compose example and matching container configuration;
- `release/geo/geoipupdate.compose.yml` — optional Geo sidecar example.

## Upgrade and rollback

Before upgrading, stop the Server, take a backup with the packaged `backup` command, and copy the backup plus the pepper and TLS secret files to protected storage. Verify the release `SHA256SUMS` file and keep the previous binary/archive available. Start the new Server and confirm `/health/live`, `/health/ready`, migrations, and the Admin audit surface before returning traffic.

If readiness or a post-upgrade smoke check fails, stop the new process, restore the previous matching Server/Agent artifacts, and start the previous version against the unchanged state directory. Do not delete or downgrade the database in place: a schema migration is forward-only. If the new version has already migrated the database, restore the pre-upgrade backup into a fresh state directory, restore the pepper and secret files with their private permissions, and validate the restored instance before switching the service back. Keep the failed release logs and recovery-rehearsal evidence for incident review.
