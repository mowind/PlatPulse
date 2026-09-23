# ADR 0009: Report Receipt identity is permanent; the receipt body is slimmed after a bounded window

- **Status:** Accepted; implementation pending.
- **Date:** 2026-09-23

`agent_report_receipts` is the Server's only idempotency record for an Agent Report and part of the evidence Agent Removal must keep, yet it grows with every report and has no retention policy (field: 1.58M rows / 4.3 GB, `receipt_body` up to 175 KB of per-Node and per-sample/range detail). Deleting rows would break replay deduplication and erase evidence; keeping whole bodies forever makes every Offline Backup Window scale with total history ([ADR 0008](0008-offline-server-backup.md)). We keep the identity row permanently and slim only the large `receipt_body` after a fixed window.

## Context

- Design §15.9 item 5 explicitly refused a retention policy for this table while the Receipt is the idempotency guard for a retried Report.
- The Server matches a retry by `report_id` and returns the stored `receipt_body`; the `UNIQUE (agent_id, agent_epoch, boot_id, report_sequence)` fence and `report_body_sha256` are what turn a replay into the identical receipt and an identity conflict into `report_identity_conflict`.
- `close_report_id` joins, the latest-rejection Attention Item, and the whole-Inventory rejection evidence all read the identity columns, not the body.
- The Agent's Durable Spool is bounded to 2 MiB / 24 hours and drops unconfirmed reports beyond that, so a legitimate retry cannot normally be older than 24 hours. The coordinated upgrade path (issues #189–#192) deliberately preserves and delivers an immutable backlog, which can be older. Both bounds are far below the window below.

## Decision

- **The identity row is permanent.** `report_id`, `agent_id`, `agent_epoch`, `boot_id`, `report_sequence`, its `UNIQUE` fence, `report_body_sha256`, `disposition`, the rejection/Inventory evidence columns and `received_at` are never deleted. No row retention is added; design §15.9 item 5 stays true.
- **The body is slimmed after a fixed 30-day window.** `receipt_body` is reduced to a compact receipt carrying the top-level disposition and the Inventory disposition; per-Node and per-sample/range detail is cleared. The window is a safety invariant derived from the two retry bounds above, not a capacity policy, and is not operator-configurable.
- **A replay of a slimmed `report_id` returns the compact receipt** when the body hash matches, and keeps the existing conflict response when it does not. A replay never recreates a row and never re-runs ingestion.
- **Slimming runs as a retention family with non-deleting semantics**, executed by the operator-triggered retention run. It touches only rows past the window and never an unconfirmed or in-flight report.
- **Acceptance is measured as reduced backup and scan volume, not a smaller `platpulse.db`.** The update frees pages but does not shrink the file; `VACUUM INTO` writes the compacted copy, so the next Backup Artifact and the offline window get smaller.

## Considered options

- **Row retention by age.** Rejected: it removes the replay guard and the Agent Removal / rejection evidence, and a late retry would be processed as a new Report.
- **Keep whole bodies forever.** Rejected: the offline window and every artifact keep scaling with total history; the field already exceeded 4 GB.
- **Move the body to a side table without clearing it.** Rejected: the same bytes remain and the scan is only relocated.

## Consequences

- A replay older than the window returns a `ReportReceipt` with empty per-Node and per-sample/range arrays but the correct dispositions; the Agent's receipt-application path must tolerate that for such a replay.
- The identity row needed for conflict detection, close-report disposition, Attention evidence and Purge barriers is unaffected.
- design §15.9 item 5 and the Report Receipt glossary entry keep their meaning; the design document records the body-slimming amendment.

## Amendment history

- 2026-09-23: accepted, replacing the unresolved retention question in issue #184.
