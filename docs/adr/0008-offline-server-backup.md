# ADR 0008: Server backups are an offline operation, redacted at write time

- **Status:** Accepted; amended 2026-09-23 — the default deployment ships no automatic backup. Implementation pending.
- **Date:** 2026-09-23

This supersedes the in-process online-backup route that `platpulse-server` implemented — the Server-owned `[backup_schedule]` and the `VACUUM INTO` + snapshot-sanitize path behind the Admin `backup_create` Operation. [Issue #193](https://github.com/mowind/PlatPulse/issues/193) cites that route as "design §20.1", but the current design document no longer carries that section, so this ADR is the authority. The code reduction is a follow-up implementation slice, with the bounded-scan, retention, single-pass and residue-cleanup prerequisites tracked by [#183](https://github.com/mowind/PlatPulse/issues/183), [#184](https://github.com/mowind/PlatPulse/issues/184), [#185](https://github.com/mowind/PlatPulse/issues/185) and [#195](https://github.com/mowind/PlatPulse/issues/195). The amendments below — agreed while re-scoping those issues — delete the packaged automation, record the layout guard and residue handling, and fix the interface with the runtime integrity monitor ([#194](https://github.com/mowind/PlatPulse/issues/194)).

## Context

`platpulse-server` treated "online backup" as a Server capability: `[backup_schedule]` ran `VACUUM INTO` on the owning write connection every 24h — re-arming one hour after every failure — and then scanned the snapshot twice — `sanitize_snapshot` writing redactions back, `validate_snapshot_privacy` re-scanning the whole artifact to prove they held. The 2026-09-23 production deployment (5.9 GB database, 4.3 GB / 1.58M-row `agent_report_receipts`) shows what the route actually bought: after 2026-09-20 not one backup succeeded (every round ended in `backup privacy validation failed`), each failure re-armed hourly, the process held roughly 0.6 cores continuously, resident memory reached 31.5 GB, a SIGKILL left 27 `.part` files totalling 131.6 GB, and the snapshot source already failed `PRAGMA integrity_check`. `VACUUM INTO` rebuilds logical content, so it neither surfaced the physical corruption nor produced a recovery point that could be verified.

The open question is therefore not how to make that loop cheaper but whether the "online" property is worth its cost at all.

## Decision

### Drop the online property

Backup creation (consistent snapshot plus redaction) and restore are **offline Server operations**. The only scenario the online property claimed to serve was a zero-window single-host deployment; the recorded deployment can accept an offline backup window, and a storage-layer snapshot covers the tighter case without the serving process doing the work. The exclusive-ownership guard is held for the whole operation, so the Server is stopped (`platpulse-server backup`, `platpulse-server restore`), or the copy is taken outside the Server by a filesystem or volume snapshot. The serving process does not create backup artifacts — neither on a schedule nor in response to an Admin request; the `backup_create` Operation and its `POST /api/admin/v1/backups` route are removed, while historical `backup_create` Operation rows stay readable. The online property has not produced a successful backup since 2026-09-20 and was paid for continuously; a stopped window is the cheaper and more honest trade.

### No automatic backup in the default deployment (amendment)

The packaged `platpulse-backup.timer` / `platpulse-backup.service` pair is **deleted**, not repurposed. An offline window only removes the resource conflict; it does not make unattended creation safe, because nothing in the package stops and restarts the Server around the command. The default deployment therefore ships **no automatic backup** and creation is an explicit operator action inside an Offline Backup Window.

`[backup_schedule]` and its `required_mount` field are removed from the configuration with the in-process scheduler. The Server configuration rejects unknown fields, so a `server.toml` that still carries the section fails to start with a dedicated error naming the retirement and the fix, and the release notes carry the migration step. A silently ignored section would imply a schedule that no longer exists.

The recorded RPO target is withdrawn. **RPO is operator-defined**; the Server makes no promise about backup age and does not retry, back off, or alert on its own, because it no longer attempts backups. Doctor reports the age of the last successful Backup Artifact and the residue in the backup directory so the operator can see an overdue or failing manual regime.

### Redaction happens at write time, inside the offline process

An artifact is redacted when it is written, not when it is restored or exported. Backups are copied to off-host or otherwise less-protected storage, so an artifact carrying raw Peer public addresses, RPC Endpoints or credentials is a standing exposure: restoring redaction at read time would make every copy, archive and restore step a secret-handling step. Moving the work off the serving path already removes the resource conflict, so artifact safety is not traded away for it.

Validation must not repeat the full redaction cost. The offline creator performs one bounded redaction pass; a separately requested verification still scans the already-redacted artifact read-only, but creation itself does not run a second full pass, so the offline window costs one pass over the data, not two. The independent read-only scan is retained for two trust boundaries: the Admin `backup_verify` Operation and the restore pre-check.

### Recovery target, RPO and RTO

The target is **whole-database recovery**, not projections only. Agent identity, credential digests, Report Receipt idempotency evidence and Purge admission barriers are only coherent as a complete snapshot; a backup that would restore a different Server identity or silently drop receipt/Purge evidence is not a recovery point.

- **Target RPO** is operator-defined (see the amendment above). The Server states the age of the last successful artifact but does not guarantee an interval. A filesystem or volume snapshot may be taken more often and is the recommended complement when a tighter RPO is required.
- **Target RTO** is one Offline Backup Window: restore, start and reach `/health/ready`, plus a data refetch. The window is measured in the per-release isolated-restore rehearsal and must be re-measured after any hardware or path change. Creation is not verification.

### Bounded failure and bounded residue (amended)

No retry loop exists, so a failed attempt cannot become permanent load; the operator sees the command's error directly. Residue is **surfaced, not reclaimed**: a `.part` (and its `.part-journal`) left by a killed or disk-full attempt is removed manually, and Doctor reports the residue count and bytes together with the last successful backup age. The known residual risk is recorded: because there is no pre-flight free-space check, a manual attempt on a full backup filesystem can still leave a half-written artifact. Bounded failure and automatic reclamation were the original requirements; this amendment replaces the automatic reclamation with operator visibility and accepts the risk explicitly.

### Layout guard moves to the offline command (amendment)

The `required_mount` guard is not discarded with `[backup_schedule]`; the optional top-level `backup_required_mount` carries it. The offline `backup` command keeps the same fail-closed layout decision: the backup directory must live under the configured mount, that mount must be a real distinct filesystem, and it must not be the live database filesystem. An absent or unmounted disk must never be treated as a valid destination. Restore is deliberately **not** gated by it: the guard protects where an artifact is written, and a destination-layout mistake must never block recovery.

### A snapshot is not a recovery point without source integrity

Because `VACUUM INTO` rebuilds logical content, a physically corrupt source can still yield a readable artifact. The creating process records the source's integrity result, and a corrupt or unreadable source fails the backup loudly rather than producing an artifact that hides the damage. Physical recovery relies on a filesystem or volume snapshot; this ADR recommends it as the complement to the offline logical backup, not as a replacement for verification.

The authoritative whole-database integrity verdict runs **offline**: as a pre-flight of `platpulse-server backup` and in a dedicated offline verification command. The serving process does not run a periodic whole-database `integrity_check`: on a deployment-sized database it cannot complete inside any useful budget on the single exclusive connection, and each attempt stalls ingestion while still producing no verdict. Runtime corruption is latched from an observed `SQLITE_CORRUPT` error; an inconclusive offline check is reported as exhausted, which is distinct from corruption (see [#194](https://github.com/mowind/PlatPulse/issues/194)).

### Retention is a prerequisite

Whole-database processing is unsustainable while `agent_report_receipts` and comparable tables grow without a retention policy. An offline window bounds the CPU and memory conflict but not the growth; retention is a precondition for routine whole-database backups.

Amended scope: no row retention is added to `agent_report_receipts`. The Report Receipt identity row is permanent, and only the large `receipt_body` is slimmed after a bounded confirmation window — see [ADR 0009](0009-report-receipt-body-slimming.md).

## Consequences

- **No packaged automation.** The shipped `platpulse-backup.timer` / `.service` pair is deleted. Creation is an explicit operator command inside an Offline Backup Window, and the operator owns scheduling stop → backup → start, including restarting the Server when the backup fails. A second local disk is not an off-host backup.
- The Admin backup surface keeps artifact listing/detail, `backup_verify`, and the restored-only-while-stopped Restore request; snapshot creation leaves the serving process and its create route is removed.
- `[backup_schedule]` and its in-process scheduler are removed. A `server.toml` that still contains the section is rejected at startup with a dedicated message, and the release notes carry the migration step.
- Off-host copies, retention and a rehearsed isolated restore remain operator obligations.
- The Agent-side `recover` sidecar backup and the coordinated upgrade checkpoint are separate, bounded, local safety copies; this ADR neither replaces nor weakens them.

## Considered options

- **A — keep online, reduce cost** (SQLite native backup API or WAL increments; summary-only redaction). Rejected: the online loop itself is the field harm, and an incremental or WAL-based snapshot still needs the same redaction pass and the same stopped-source consistency before it is a real recovery point.
- **B — online raw snapshot, redact later** (offline or at restore). Rejected: it puts unredacted Peer public addresses, Endpoints and credentials at rest in every artifact and defers the safety decision to a later, usually less-protected process.
- **C — external process or filesystem snapshot** (chosen, combined with offline CLI creation and write-time redaction). It removes the resource conflict by construction and gives a physically consistent copy for the corruption case.
- **D — projections-only backup**. Rejected: it cannot restore identity, credentials, Receipt idempotency or Purge barriers, so it is not a recovery point.

## Amendment history

- 2026-09-23: accepted, superseding the in-process online-backup route (issue #193).
- 2026-09-23: amended after re-scoping issues #183–#185, #194 and #195. Deleted the packaged timer/service (no automatic backup by default), made the `[backup_schedule]` removal a hard configuration break with a dedicated error, withdrew the RPO target in favour of an operator-defined one, replaced automatic `.part` reclamation with Doctor visibility plus a recorded risk, moved the layout guard to the offline command, moved the authoritative integrity verdict offline, and pointed receipt-body slimming at ADR 0009.
