# ADR 0012: Public Node Detail Metric Window

**Status:** Accepted (design interview 2026-10-20). Delivery is split into two issues: #247 (the Metric Window for the four sample-backed charts) and #248 (the block-series aggregate tiers) — both **implemented**. Supersedes deviation 29 (h) of the Emerald register (docs/visual-migration/emerald/README.md) for the range switcher only; every other part of deviation 29 stands.

## Context

The Public Node Detail six-chart deck answers exactly the latest 60 seconds (`PUBLIC_NODE_METRIC_WINDOW_SECONDS`, crates/platpulse-server/src/http/public.rs). Deviation 29 (h) refused Emerald's 1/6/24/720h switcher because "the deck's window is the Server's own 60 seconds, so a front-end range selector would advertise a window no data supports".

That reason no longer holds. Since issue #214 every counted observation is accumulated into the 1-minute (7-day) and 5-minute (30-day) aggregate tiers at ingestion, in the same receipt transaction as the raw write, and the raw tier keeps 24 hours by default. Process CPU, Process memory, Peer inbound/outbound (Node series) and Host network rx/tx (Host series) therefore have real evidence for every window up to 7 days. The two block charts do not: Block interval and Transactions / block are derived from `block_summaries`, which has no aggregate tier and holds roughly 600,000 rows per Node over 7 days.

## Decision

- **A Metric Window is part of the Public Projection** (`CONTEXT.md`): the same answer for every Home reader, Guests included when Site Access Mode is Public. It is not restricted to signed-in readers.
- **Fixed presets, Server-owned grain.** The Public metrics route accepts only an enum — `live`, `1h`, `6h`, `24h`, `7d` — never an arbitrary `from`/`to`. The Server fixes the tier per preset: Live and 1h from raw samples, 6h from 1-minute buckets, 24h and 7d from 5-minute buckets. No grain is derived at read time. Live is the default and keeps today's behaviour exactly. There is no 30-day preset: thirty days is an Admin investigation, not a Home reading.
- **A bucket is drawn by its maximum.** Buckets store count, min, max and last but no sum, so no mean exists; drawing `last` would hide spikes. The line goes through `max_value`; the tooltip states min, max, sample count and the bucket's interval, and the newest, still-accumulating bucket is marked `Open bucket`. **Amended by #248 for the block tiers only:** a block bucket does store a sum — `transaction_total`, every transaction its blocks carried — so a Transactions / block bucket states that total beside its count of blocks; it is still drawn by its maximum (the most transactions one block carried), because a mean would still smooth away the busy block, and the sample tiers keep no sum.
- **Block charts are delivered in a second phase** (#248, delivered: migration 0075 `node_block_aggregates`, `crates/platpulse-server/src/block_aggregates.rs`) through new ingestion-time block aggregate tiers (1m/5m: block count, interval min/max, transaction total) that obey Block history dedup — a Replay never accumulates, only `OpenRecoverableGap` backfill may — and are registered in the retention catalog, with no backfill of earlier history. Until #248 shipped, the two block cards stated, in the place of the plot, that they showed the Live window only; they now plot every window.
- **The Public and Admin surfaces stay separate.** The window is a Public DTO on the Public route group, not a reuse of the Admin metric-history endpoint; the Admin Metric history panel is unchanged.

## Considered options

- **Only signed-in readers get long windows.** Rejected: a Public Projection is fixed and sanitized regardless of who views it; these series carry no sensitive data.
- **Arbitrary `from`/`to` on the Public route.** Rejected: an anonymous request could choose its own cost; a fixed enum bounds every answer (at most about 2,016 points per series) without a new rate limit.
- **Read-time bucketing of `block_summaries`.** Rejected: it contradicts the rule that nothing is derived at read time, and it scans hundreds of thousands of rows per anonymous request.
- **Drawing `last` or a mean per bucket.** Rejected: no mean exists, and `last` hides exactly the spikes a monitor exists to show.
- **Showing only the latest N blocks for long windows.** Rejected: the axis would claim a window the series does not cover.

## Consequences

- The Public API now exposes seven days of aggregated Node and Host series. Narrowing or withdrawing that later is a breaking change to a Public route.
- The Public answer depends on the aggregate tiers' retention floors (1-minute tier 7 days, 5-minute tier 30 days) and on raw retention for Live/1h; a window older than a tier's evidence, including stretches from before #214, is answered as unobserved, never as zero.
- The block tiers place a block in the bucket of its own `observed_at`, the instant the Live chart plots it at, so an `OpenRecoverableGap` backfill lands where it was observed (the backfill time), not at its chain timestamp: a stretch the Node did not observe stays a gap on every window. A block's interval is credited once, by whichever of the block and its parent arrived second, into the block's own bucket while that bucket still exists. A bucket is drawn by its maximum on both block charts — the longest interval, and the most transactions one block carried — and a Transactions / block bucket additionally states its transaction total.
- Silence inside a long window is judged by the existing Admin long-history continuity rule (`MIN_GAP_SECONDS` plus each bucket's `max_gap_seconds`), not a second rule.
