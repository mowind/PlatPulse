# ADR 0014: Catch-up replays oldest-first; current freshness follows observation time

**Status:** Accepted (design interview for [issue #268](https://github.com/mowind/PlatPulse/issues/268)).

## Context

After a 1 h 49 min Server outage, the Durable Spool replayed a two-hour backlog oldest-first (#19). Each replayed Agent Report advanced the Current Projection through stale snapshots, while freshness, Observed Network Head source selection and block-stalled evaluation all keyed on Server receipt time. Nodes that were at chain head were therefore shown as Healthy but "6,000+ blocks behind" for the whole drain. That figure was the Spool Backlog Age converted to blocks, not Node state.

## Decision

- **Delivery order is unchanged.** Catch-up stays strictly oldest-first, with one report in flight. Every report is applied, and nothing is skipped or reordered for the sake of the current view.
- **Current Projection freshness is judged by observation time.** Freshness compares the observation time carried by the report with Server time, corrected by the Agent's measured clock skew. A Node whose current state comes from a replayed report is Stale. A Stale Node is not a source for Observed Network Head, shows no blocks-behind figure, and is not evaluated by head-lag alerts.
- **History is stamped with observation time.** Every history fact derived from an Agent Report — block summaries, Peer presence intervals, Peer aggregates — uses the skew-corrected observation time. Receipt time is kept only for Agent liveness, idempotency and audit. The Clock Unreliable fallback below covers freshness alone: history keeps the observation instant the report carries even when the skew is unmeasured, because re-stamping a drained backlog at receipt time would pile hours of history onto the minutes of catch-up — the compression this ADR removes.
- **Numeric metric samples keep the observation instant uncorrected.** A metric sample is addressed by its observation instant: the raw row's key, the series ledger's high-water mark and the aggregate bucket all read it as one identity. The skew is re-measured in every Report while the Agent carries its last-good observation into later ones, so a correction that drifts by milliseconds changes the identity of a sample the Server already holds and the carry is counted as a second observation (#213). Metric samples therefore store the instant the report carries — still the observation instant, never the receipt instant — and the correction above applies to freshness and to the other history families. Splitting a stable identity from a corrected timeline needs the identity stored next to every sample on both backends; that is a change of its own, tracked as #279.
- **Clock Unreliable falls back to receipt time.** If the skew is unmeasured or the Agent is Clock Unreliable, freshness falls back to receipt time, which is the behaviour before this ADR. This is consistent with Clock Unreliable never invalidating unrelated observations. It accepts that a badly skewed Agent can still look fresh during catch-up.
- **The Server derives Spool Backlog Age at receipt.** Under oldest-first delivery, the report being ingested is the Agent's oldest unacknowledged one. So `received_at − generated_at`, corrected for skew, is the live Spool Backlog Age, and no wire change is needed. When the clock is unreliable, the Server shows the report's own `oldest_queued_age_ms` instead, labelled as of generation time. Admin and Server Doctor show this value (Doctor warns above 5 minutes). Home only shows Stale.

## Considered Options

- **Coalesce or skip superseded reports on the Agent.** Rejected: this drops Block Summaries that must then become History Gaps, which contradicts the Durable Spool's no-silent-loss rule (#20).
- **A separate "latest snapshot first" lane.** Rejected: Report Ingestion rejects any report whose sequence is not newer than the last accepted one (`StaleReport`). Delivering the newest report first would cause the whole backlog to be rejected.
- **Freshness is Unknown under Clock Unreliable.** Rejected: one bad host clock would stop every Node on that Agent from ever being Healthy.

## Consequences

- A long outage now shows as Stale Nodes plus an Admin-visible Spool Backlog Age, not as Node lag. How long it lasts is bounded by catch-up throughput (design §7.2), not by this ADR.
