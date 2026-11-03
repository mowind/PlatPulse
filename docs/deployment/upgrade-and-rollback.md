Part of the [PlatPulse Server deployment guide](../deployment.md): Upgrade and rollback.

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
offline, one-way command ([ADR 0013](../adr/0013-postgresql-server-storage-backend.md)):
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
runs. A full Durable Spool drops its oldest historical reports into History
Gaps, so the window must stay shorter than the spool's capacity — [Durable
Spool capacity and the downtime it covers](transport-logs-metrics-and-spool.md#durable-spool-capacity-and-the-downtime-it-covers)
converts the two. A retention-heavy deployment should measure the runtime on a
copy of its own data before the real cutover. The measured run recorded in
[release qualification](../release-qualification.md) covers the range of a
near-production deployment: budget the whole window, add the start-and-verify
step, and leave margin for the estimate the command prints as it goes.
