# ADR 0001: Bound Agent-local receipt lifecycle work

- **Status:** Accepted
- **Date:** 2026-09-02
- **Decision scope:** Agent Store only

## Context

A Server **Report Receipt** is the durable report-level acknowledgement at the
Server trust boundary. Keeping its complete response body indefinitely in the
Agent Store turns that acknowledgement into an unbounded Agent-side archive.
It also invites collection and delivery loops to reload and parse receipt
history at one-second cadence.

The Agent only needs a recent, durable terminal marker to recognise a duplicate
or conflict while it finishes durable delivery. That marker must not weaken the
immutable Agent Report and transactional receipt protocol, and concurrent
Agent workers must not create SQLite ownership or write-decision races.

## Decision

### Receipt lifecycle

The Agent retains an **Applied Receipt Record**, not a Server Report Receipt
archive. It contains only the report identity, immutable report body hash,
disposition, and application time. Records have a 24-hour duplicate/conflict
window. Expiry is an indexed, fixed-size batch so its memory and lock duration
do not grow with historical marker count.

Receipt application validates the complete Server response against the exact
immutable Agent Report before the transaction changes durable state. The
accepted/rejected disposition effects, Applied Receipt Record insertion, Agent
Report removal, and the bounded expiry batch commit as one transaction. A
pending Agent Report and an Applied Receipt Record with the same identity is an
impossible state: the Agent Store fails closed instead of inferring an outcome.

### Startup validation and runtime gates

Startup is the lifecycle boundary for migration, SQLite integrity and required
schema checks, bounded expiry catch-up, and validation of the current Durable
Spool. Workers do not start until those gates pass.

Runtime collection and delivery do not reparse historical Applied Receipt
Records. Their gates inspect the durable fatal marker, queued Agent Reports,
and at most an identity-matched recent marker. Thus current work is bounded by
the current input, queued spool policy, and the fixed cleanup batch rather
than receipt history. The seeded regression covers restart with 100,000 receipt
markers and verifies the empty-spool paths retain the indexed identity lookup.

### Migration failure semantics

Legacy conversion runs through the Agent's transactional SQLx migration path.
It creates the minimal Applied Receipt Record table and application-time index,
retains only the most recent bounded legacy marker fields, removes complete
receipt bodies, and advances the schema. A migration or SQLite error leaves the
Agent Store unopened and prevents worker startup; the migration transaction
must not advance the schema partially. Recovery is an explicit operator
decision rather than an automatic reset.

### Ownership and writes

An Agent runtime acquires a private, exclusive operating-system lock associated
with its configured state database before recovery or worker startup. A second
runtime fails before it can access the same Agent Store; releasing the first
runtime releases the lock.

Purpose-specific SQLite connections share one injected asynchronous write
permit. Every write and every read-dependent write decision retains that permit
from its authoritative read through commit or rollback. Ordinary read-only work
remains concurrent, and RPC, HTTP, report assembly, parsing, and filesystem
operations remain outside the permit.

## Consequences

- Agent-local state stays bounded by retention and fixed cleanup work rather
  than report age.
- Startup remains the deliberate integrity boundary; runtime gates are not a
  substitute for historical validation.
- The Server's Report Receipt ledger, Report Ingestion idempotency, and public
  APIs are unchanged.
- Migration or impossible-state failures stop the Agent safely and require
  investigation rather than data deletion or re-enrollment.
- Release qualification must still measure the deployed Agent and Server on
  the actual state database; deterministic tests do not substitute for an
  operational observation window.
