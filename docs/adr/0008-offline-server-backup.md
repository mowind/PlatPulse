# ADR 0008: Server backups are an offline operation, redacted at write time

- **Status:** Accepted; implementation pending.
- **Date:** 2026-09-23

This supersedes the in-process online-backup route that `platpulse-server` implemented — the Server-owned `[backup_schedule]` and the `VACUUM INTO` + snapshot-sanitize path behind the Admin `backup_create` Operation. [Issue #193](https://github.com/mowind/PlatPulse/issues/193) cites that route as "design §20.1", but the current design document no longer carries that section, so this ADR is the authority. The code reduction is a follow-up implementation slice, with the bounded-scan, retention, single-pass and residue-cleanup prerequisites tracked by [#183](https://github.com/mowind/PlatPulse/issues/183), [#184](https://github.com/mowind/PlatPulse/issues/184), [#185](https://github.com/mowind/PlatPulse/issues/185) and [#195](https://github.com/mowind/PlatPulse/issues/195).

## Context

`platpulse-server` treated "online backup" as a Server capability: `[backup_schedule]` ran `VACUUM INTO` on the owning write connection every 24h — re-arming one hour after every failure — and then scanned the snapshot twice — `sanitize_snapshot` writing redactions back, `validate_snapshot_privacy` re-scanning the whole artifact to prove they held. The 2026-09-23 production deployment (5.9 GB database, 4.3 GB / 1.58M-row `agent_report_receipts`) shows what the route actually bought: after 2026-09-20 not one backup succeeded (every round ended in `backup privacy validation failed`), each failure re-armed hourly, the process held roughly 0.6 cores continuously, resident memory reached 31.5 GB, a SIGKILL left 27 `.part` files totalling 131.6 GB, and the snapshot source already failed `PRAGMA integrity_check`. `VACUUM INTO` rebuilds logical content, so it neither surfaced the physical corruption nor produced a recovery point that could be verified.

The open question is therefore not how to make that loop cheaper but whether the "online" property is worth its cost at all.

## Decision

### Drop the online property

Backup creation (consistent snapshot plus redaction) and restore are **offline Server operations**. The only scenario the online property claimed to serve was a zero-window single-host deployment; the recorded deployment can accept a maintenance window, and a storage-layer snapshot covers the tighter case without the serving process doing the work. The exclusive-ownership guard is held for the whole operation, so the Server is stopped (`platpulse-server backup`, `platpulse-server restore`), or the copy is taken outside the Server by a filesystem or volume snapshot. The serving process does not create backup artifacts — neither on a schedule nor in response to an Admin request. A maintenance window or a storage-layer snapshot is an accepted deployment prerequisite. The online property has not produced a successful backup since 2026-09-20 and was paid for continuously; a stopped window is the cheaper and more honest trade.

### Redaction happens at write time, inside the offline process

An artifact is redacted when it is written, not when it is restored or exported. Backups are copied to off-host or otherwise less-protected storage, so an artifact carrying raw Peer public addresses, RPC Endpoints or credentials is a standing exposure: restoring redaction at read time would make every copy, archive and restore step a secret-handling step. Moving the work off the serving path already removes the resource conflict, so artifact safety is not traded away for it.

Validation must not repeat the full redaction cost. The offline creator performs one bounded redaction pass; the read-only validation step checks the already-redacted artifact directly rather than re-running the whole redaction scan, so the offline window costs one pass over the data, not two.

### Recovery target, RPO and RTO

The target is **whole-database recovery**, not projections only. Agent identity, credential digests, Report Receipt idempotency evidence and Purge admission barriers are only coherent as a complete snapshot; a backup that would restore a different Server identity or silently drop receipt/Purge evidence is not a recovery point.

- **Target RPO** is at most the configured backup interval plus one bounded retry — 24 hours under the default daily schedule. A filesystem or volume snapshot may be taken more often and is the recommended complement when a tighter RPO is required.
- **Target RTO** is one maintenance window: restore, start and reach `/health/ready`, plus a data refetch. The window is measured in the per-release isolated-restore rehearsal and must be re-measured after any hardware or path change. Creation is not verification.

### Bounded failure and bounded residue

A failed attempt must leave nothing behind and must not become permanent load. Any `.part` file is reclaimed at the start of the next attempt and at startup; a failure backs off and surfaces as an operator-visible alert (last success, last attempt, last error) instead of re-arming every hour indefinitely; and restore refuses while the Server is running, in every mode.

### A snapshot is not a recovery point without source integrity

Because `VACUUM INTO` rebuilds logical content, a physically corrupt source can still yield a readable artifact. The creating process records the source's integrity result, and a corrupt or unreadable source fails the backup loudly rather than producing an artifact that hides the damage. Physical recovery relies on a filesystem or volume snapshot; this ADR recommends it as the complement to the offline logical backup, not as a replacement for verification.

### Retention is a prerequisite

Whole-database processing is unsustainable while `agent_report_receipts` and comparable tables grow without a retention policy. An offline window bounds the CPU and memory conflict but not the growth; retention is a precondition for routine whole-database backups.

## Consequences

- The shipped `platpulse-backup.timer`/`.service` pair becomes the canonical automation path and must be wrapped in a stop → backup → start orchestration that always restarts the Server, including when the backup fails.
- The Admin backup surface keeps artifact listing/detail, verification and the restored-only-while-stopped Restore request; snapshot creation leaves the serving process.
- `[backup_schedule]` and its `required_mount` layout guard are removed together with the in-process scheduler. A layout guard may move to the offline command; it must not imply that an absent or unmounted disk is a safe destination.
- Off-host copies, retention and a rehearsed isolated restore remain operator obligations. A second local disk is not an off-host backup.
- The Agent-side `recover` sidecar backup and the coordinated upgrade checkpoint are separate, bounded, local safety copies; this ADR neither replaces nor weakens them.

## Considered options

- **A — keep online, reduce cost** (SQLite native backup API or WAL increments; summary-only redaction). Rejected: the online loop itself is the field harm, and an incremental or WAL-based snapshot still needs the same redaction pass and the same stopped-source consistency before it is a real recovery point.
- **B — online raw snapshot, redact later** (offline or at restore). Rejected: it puts unredacted Peer public addresses, Endpoints and credentials at rest in every artifact and defers the safety decision to a later, usually less-protected process.
- **C — external process or filesystem snapshot** (chosen, combined with offline CLI creation and write-time redaction). It removes the resource conflict by construction and gives a physically consistent copy for the corruption case.
- **D — projections-only backup**. Rejected: it cannot restore identity, credentials, Receipt idempotency or Purge barriers, so it is not a recovery point.
