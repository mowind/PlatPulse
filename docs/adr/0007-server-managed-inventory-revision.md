# ADR 0007: Server assigns Inventory Revisions without owning Agent configuration

**Status:** Accepted; partially implemented. The v2 protocol major, Server-side allocation, v2 Receipt, Agent confirmation record, and the retry/out-of-order/transaction-failure semantics (issues #186 and #187) are implemented; the coordinated migration, offline preparation and cutover tooling described below remain unimplemented and do not authorize a production switch. Approved through the Q1–Q12 design interview for [issue #182](https://github.com/mowind/PlatPulse/issues/182).

## Context

The v1 Agent manually supplies a monotonically increasing Inventory revision. Forgetting to increase it after changing declared content prevents acceptance; the guard and diagnostics delivered by issue #181 expose this failure but do not remove the operator burden. Moving allocation to the Server changes the wire contract, not merely a configuration default. Frozen v1 types reject unknown fields, and its Inventory hash includes both revision and Node array order.

The Server is the acceptance boundary, while the Agent remains the owner of local Node Inventory and connection configuration. We choose Server-assigned revisions, existing Report ordering, and a coordinated offline protocol cutover rather than a second declaration CAS protocol or rolling dual-major ingestion. The resulting preparation requirements and restricted downgrade policy are deliberate costs of keeping one allocation authority.

## Decision

### Meaning and authority

- An Inventory Revision identifies an accepted declaration transition within one Agent identity. Consecutive equivalent declarations retain it; A → B → A advances twice. Purge is an admission change, not an Agent declaration change.
- The Server allocates only after whole-Inventory validation and the existing Epoch, Boot and Report Sequence fences permit the declaration. Network arrival order alone is not authority. No additional CAS or permission round-trip is introduced.
- A versioned canonical declaration fingerprint excludes revision, sorts Nodes by Node ID and retains all declared fields, including bootstrap display name, Network key, RPC Endpoint and process selector. Agent-only settings are excluded. Do not invent URL/path/string equivalences. The fingerprint is distinct from the immutable Report body hash; v1 hashing stays frozen.
- The revision belongs to the Agent identity, not a protocol version, Boot or Epoch. Restart and Agent Recovery do not reset it. A genuinely new Agent identity has an independent sequence. Uninitialized state has no accepted revision; first acceptance assigns 1. Use checked increments within the positive signed 64-bit storage range; exhaustion rejects a required increment without resetting or wrapping.

### Receipt and confirmation

The new protocol major (v2) removes Agent-supplied Inventory revision and returns the accepted declaration's revision and fingerprint in the Inventory outcome of its Receipt. Unchanged content returns the existing pair; rejected whole Inventory has no accepted pair for that declaration. Top-level partial acceptance is not a substitute for the Inventory outcome. Allocation, fingerprint, applicable projection changes and exact Receipt commit atomically. A rollback allocates nothing durably; replay returns the original Receipt, never the latest pair.

The Agent's bounded Inventory Declaration Record becomes confirmation state, not a declaration guard or revision allocator. Receipt application validates the exact immutable Report and its corresponding declaration fingerprint before any durable effects. A higher confirmed revision advances the record; equal revision and fingerprint is idempotent; a delayed lower revision may complete its Report acknowledgement without moving the record backward. Equal revision with a different fingerprint fails closed and retains evidence. Receipt effects, confirmation update and Report removal remain one transaction under [ADR 0001](0001-agent-store-receipt-lifecycle.md). Missing confirmation or delayed delivery never prevents creating new Reports.

### Purge and lifecycle

[ADR 0004](0004-owner-removal-and-node-purge.md) remains authoritative: fingerprint the complete declaration, including barred Node IDs, and separately re-evaluate admission even when content is unchanged. Purged Nodes cannot reappear; valid siblings continue. A valid Inventory with every Node barred can still have an accepted/unchanged Inventory outcome and per-Node rejection. Do not rewrite receipts, remove deduplication evidence, remotely change configuration or turn Purge into retirement. The latest accepted complete declaration still governs retirement subject to permanent admission boundaries.

### Coordinated migration, not rolling compatibility

1. Under v1, stop ordinary Report generation, drain queued immutable Reports, and successfully apply the final Closing Receipt. Empty Spool alone is insufficient. Rejected closure blocks cutover.
2. Capture the complete declaration used by that Closing and verify its original revision-inclusive, order-sensitive v1 hash and revision against the Server's last accepted values **before** canonicalization. Current configuration alone, Server Node projections and old hashes cannot reconstruct this evidence. Missing or mismatched evidence blocks migration.
3. Quiesce relevant writers and external-effect workers and take a coordinated checkpoint of Server/Agent state and old configuration after closure. The preparation bridge, snapshot capture and offline validation are future implementation work, not existing operational commands. Do not rewrite pending reports, discard backlog or re-enroll to bypass the gates.
4. Preserve the old accepted revision and convert both the Server baseline and Agent confirmation fingerprint using the verified declaration. Preserve Boot linkage, the next Boot's sequence state and pending DrainedPrevious transition; the first v2 Report completes the existing transition rather than resetting identity. An Agent with no accepted Inventory remains uninitialized, not an invented accepted empty declaration.
5. Remove local inventory_revision configuration for v2; its presence yields an actionable migration error, not silent ignoring. Retain old configuration in the rollback checkpoint. Validate offline before resuming business writes.

The existing agents.last_inventory_revision is the preserved numerical baseline, not an Agent-controlled value after cutover. The old inventory_sha256 is not a convertible digest: the new canonical fingerprint is computed from verified declaration evidence, with its interpretation explicitly versioned. Agent and Server conversion must agree, or equal-revision checks would falsely report a conflict. Physical columns, migration numbers and operator command shapes belong to the implementation specification; they must meet this contract.

A coordinated checkpoint can restore the old system only before business writes resume. Offline validation must not start normal collectors, ingestion, administrative mutations or external-effect workers. After writes resume, promise forward repair, not lossless downgrade; restoring an older backup is a separate disaster-recovery action and may undo important post-cutover state, including Purge barriers.

### Narrow v1 replay exception

After cutover, retain a replay-only v1 path under normal authentication, Agent ownership, input size and safety checks. An existing retained report identity with identical body bytes/hash returns its original v1 Receipt; different bytes conflict. An unseen v1 Report is unsupported: it cannot create an acceptance Receipt, advance a Boot, allocate a revision or update projections. Never convert saved receipts to v2. Existing retention bounds still apply. This is not mixed-version ingestion, automatic downgrade or a substitute for pre-upgrade drainage.

## Alternatives and consequences

- **Agent-managed automatic numbering or declaration CAS:** rejected in favor of Server allocation and existing report-order fences; acknowledgement latency must not gate local collection.
- **Content-addressed revisions:** rejected because returning to earlier content is a new accepted transition, not reuse of its historical revision.
- **Rolling dual-major ingestion or new Agent fallback to old Server:** rejected; coordinated downtime is accepted instead, with preparation failures stopping upgrade.
- **Additive v1 Receipt field:** rejected because frozen strict v1 is not safely extensible this way.
- **Rebuild from Node projections or silently establish an unverified first-v2 baseline:** rejected; neither proves equality with the last accepted declaration.
- **Delete all v1 handling:** rejected only to the extent necessary to retain authenticated exact replay of existing receipts. No new v1 ingestion is retained.

The preparation snapshot is bounded migration evidence, not a new permanent Report/Inventory archive. Existing report immutability, receipt retention, last-good observations, Node-scoped data, Network validation and Agent-local configuration ownership remain intact. This decision authorizes no runtime, schema, OpenAPI or configuration change by itself.

The detailed acceptance matrix and current-versus-target boundary are in [main design §15.10](../design/platpulse.md#server-managed-inventory-revision-target). Follow-up implementation must supply versioned wire/canonicalization fixtures, failure tests and the verified preparation tooling before claiming this capability is available.
