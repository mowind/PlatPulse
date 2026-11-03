Part of the [PlatPulse Server deployment guide](../deployment.md): Agent Store recovery; systemd services and offline backups.

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
nor restarts the Server. [ADR 0008](../adr/0008-offline-server-backup.md) retires
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

[ADR 0008](../adr/0008-offline-server-backup.md) makes creation and restore
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
- An Offline Backup Window is Server downtime, so it is bounded by the same
  Durable Spool capacity: keep the whole window — creation, verification and
  restore rehearsal — inside the tolerated downtime [Durable Spool capacity and
  the downtime it covers](transport-logs-metrics-and-spool.md#durable-spool-capacity-and-the-downtime-it-covers)
  computes for that Host.
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

- **Use the offline path.** [ADR 0008](../adr/0008-offline-server-backup.md)
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
