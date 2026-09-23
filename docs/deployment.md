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
compatible replacement yet. The remaining cargo-audit warnings stay visible in
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


