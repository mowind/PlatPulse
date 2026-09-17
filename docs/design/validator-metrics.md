# Validator metrics on Home and Node detail

Status: interview decisions confirmed through Q21 and test seams approved. The synthesized implementation specification is published as [GitHub issue #153](https://github.com/mowind/PlatPulse/issues/153), labeled ready-for-agent. The first approved slice — explicit per-Network PlatScan deployment binding and the cumulative Validator block count on both Node views — is implemented by #154. The remaining metrics are tracked by #155–#159.

## Confirmed metric definitions

The six metrics belong to the currently linked Validator, not the monitored Node instance. See CONTEXT.md for the canonical definitions: Validator Lifetime Block Count, Validator Lifetime Rewards, Validator Rank, Validator Block Production Completion Rate, PlatScan 24-Hour Block Production Rate, and Effective Delegation Reward Distribution Ratio.

- Cumulative metrics cover the Validator's chain history, not monitoring or link tenure.
- Cumulative rewards (UI: 累计收益) include both operator and delegator shares of Validator rewards. Use the PlatScan gross rewardValue meaning, not operator net earnings; exclude principal and ordinary transfers.
- Rank follows PlatScan within the same Network.
- Historical completion is same-response blockQty / expectBlockQty × 100%, labeled cumulative actual / cumulative scheduled blocks. Whole-round scheduling is accepted; it can transiently lower the rate before duties elapse and is not an exact missed-block rate.
- The UI metric 24小时出块率 is taken directly from PlatScan stakingDetails.genBlocksRate using upstream semantics, not a locally reconstructed strict rolling-24-hour window. This explicitly supersedes the earlier strict rolling-window requirement.
- Delegation distribution means the currently effective share allocated to delegators, not APY, operator commission, or a pending ratio.

## Confirmed Home aggregation

- New aggregate metrics follow the current Home filters and include linked Active Nodes. Temporarily offline Active Nodes remain eligible; Retired Nodes do not. Label the scope as the current selection.
- Deduplicate by Network plus Validator identifier regardless of primary, standby, or observer role. A Validator counts once if any eligible Node has an effective link.
- Group both cumulative block counts and earnings by Network. Do not combine Networks or perform fiat conversion.
- Aggregate available last-good values. An incomplete aggregate is a known-values subtotal, not a complete total. Show independent coverage and stale counts for each metric, for example 9/10 Validators with values, including one stale.
- Show unknown when no values are available. Show unlinked Node counts separately; do not treat them as zero-value Validators.
- These are metrics of linked Validators, not a claim of asset ownership. Aggregates can decrease when filters, effective links, or Active membership change.

## Confirmed Node presentation and edge cases

- Both Home Node cards and Node detail expose all six metrics. Mobile must not require hover or expanding a section to see them; use a dedicated linked-Validator section with a two-column mobile layout.
- Display Validator identity and link role. On a link change from A to B, display B's cumulative metrics; never splice A and B histories.
- Unlinked Nodes show unlinked, not zero. Unknown metrics remain unknown. Retain last-good values after failures and label staleness and update time. Provider failure must not change Node health.
- With complete evidence and zero expected blocks, completion is not applicable / no production duties, not 0% or 100%. Missing evidence is insufficient data.
- The source-defined 24小时出块率 is displayed directly when the detail request succeeds and genBlocksRate is valid, including source-returned 0%. Mark it PlatScan口径 and explain the preceding-seven-settlement-period window and ambiguous zero (which can reflect absent evidence or upstream errors). This is a source-reported value, not verified zero performance. Local failures/missing fields never synthesize zero, and upstream 0% never establishes no production duties.
- Node detail shows provenance, update time, and reasons for unavailable or degraded data.
- Earnings use the Network native unit; cards may abbreviate. Detail exposes all available source precision, without reconstructing digits absent from the source. Source-limited precision is accepted: exact decimal processing, abbreviated cards, and available source precision in detail; default displayed percentages use two decimal places.

## Confirmed delivery and Provider scope

- All six metrics must be implemented end to end; permanent unsupported placeholders do not satisfy completion. Legitimate unlinked, missing, failed, or not-applicable outcomes remain explicit.
- The user approves dedicated Server-side aliveStakingList collection shared per Network for rank, not an implicit detail-failure fallback. Its failure must not erase independent detail metrics. This supersedes the old detail-only boundary in validator-provider.md; that contract will be reconciled when the final design is confirmed.
- Default refresh remains approximately 60 seconds. Freshness follows the configured refresh interval (default two intervals); display successful fetch time and source cutoff when available. Failures retain last-good data and do not affect Node health.
- Source-limited monetary precision is accepted, not financial settlement accuracy.
- Existing rewardRate sourced from deleAnnualizedRate remains distinct from rewardPer delegation distribution.

## Confirmed ranking and Network boundaries

- Rank uses the Network's complete live staking ALL cohort without a name filter, including candidates, and adopts the upstream rank independently of Home filters. Only a complete successful list fetch can establish unranked; failure or incomplete pagination must not. Retain stale last-good rank when available.
- Server configuration explicitly binds each Network to its corresponding PlatScan deployment. This configuration support is in scope; a shared allowlist is not evidence that one deployment serves multiple Networks.
- Networks without a configured source show not configured. Full completion means all six capabilities work on correctly configured Networks with upstream support, not invented coverage of Networks without a PlatScan service.

## Verification remaining before implementation acceptance

- Validate per-Network Provider deployment mapping, upstream version, fixtures, field units, historical coverage, resets/corrections, and paging consistency. Upstream source analysis alone does not prove deployment compatibility.
- Prior research blockers for operator net earnings and strict rolling24h below are retained as evidence for rejected mappings; those two requirements have now been superseded.

## Acceptance checklist

- Server exposes all six validated metrics and their availability/freshness/provenance; preserves existing annualized-yield semantics separately from the new delegation ratio.
- Server owns aggregate membership, deduplication, Network separation, exact decimal arithmetic, and per-metric coverage/staleness. API results follow the same selection as the Home cards, not a browser-only sum of duplicate Node metrics.
- Home adds cumulative blocks and gross cumulative rewards for the current selection per Network. Node cards and Node detail render all six metrics, Validator identity/role, and the confirmed source explanations.
- Validate direct rewardValue mapping, rewardPer percentage scaling, optional trailing-percent genBlocksRate parsing, historical zero denominator, missing/invalid fields, and source-reported zero distinct from locally unknown. Preserve amount precision without floating-point summation.
- Validate duplicate primary/standby/observer links, Node link changes, offline Active versus Retired Nodes, unlinked Nodes, filters, partial aggregates, source failures, stale refresh, counter resets/corrections, list paging failures and genuine unranked outcomes.
- Validate separate Network sources and unconfigured Network behavior; no cross-Network amount or block aggregation.
- Generate the API client from updated OpenAPI and run relevant Rust/Web tests and fixed responsive projects (360/390/768/1280 widths). Document checks actually run and any environmental blockers.
- No permanent unsupported placeholders for the six requested capabilities on supported, correctly configured deployments. Transient failures and genuine missing/not-applicable states remain visible rather than being replaced by zero.
- Before feature implementation, obtain final user confirmation of this consolidated scope and reconcile the older validator-provider.md contract, including its no-list-call boundary.


## Upstream source verification

Research examined official browser-server revision `6daa5ad2e878474869407314a73a219ac49c8b51`. These findings do not establish the deployed PlatScan revision, complete indexing, or live response compatibility. Deployment fixtures remain necessary.

- `blockQty` and `expectBlockQty` map to node-identity cumulative counters retained across re-staking. Expected blocks are scheduled in whole consensus rounds, so their ratio is not necessarily an elapsed-slot completion rate during an unfinished round. Indexer history completeness remains unverified.
- `rewardValue` is gross fee + block + staking rewards, with special treatment for initial nodes. `rewardValue - totalDeleReward` is only a candidate: consistent settlement boundaries, initial-node handling, history, and precision remain unverified. No verified drop-in lifetime operator earnings mapping was found.
- `rewardPer` is the current delegator distribution percentage, while `nextRewardPer` is pending. Internal 2000 basis points becomes API decimal percentage 20, meaning 20%, not 0.20% or 2000%. `deleAnnualizedRate` is unrelated APY.
- `genBlocksRate` sums the preceding seven settlement epochs, excludes the current one, and can return 0% for errors or absent evidence. No verified drop-in strict rolling-24-hour source was found; it needs timestamped actual and expected production history with full coverage.
- Ranking comes from `aliveStakingList`, not the detail DTO. The list orders by version descending, total stake descending, staking block ascending, and staking transaction index ascending. Cohort selection and multi-page consistency need explicit treatment; Home filters must not redefine ranking.
- Amount serialization converts von to LAT strings and truncates down to 12 decimal places. Source precision is not chain-minimal-unit accounting precision.
- Requests contain nodeId but no Network selector. Correct Network-to-provider deployment mapping must be validated; an allowlist alone does not establish source chain identity.

Pinned primary references:

- [Detail mapping and ranking](https://github.com/PlatONnetwork/browser-server/blob/6daa5ad2e878474869407314a73a219ac49c8b51/scan-api/src/main/java/com/platon/browser/service/StakingService.java)
- [Settlement-window production rate](https://github.com/PlatONnetwork/browser-server/blob/6daa5ad2e878474869407314a73a219ac49c8b51/scan-common/src/main/java/com/platon/browser/bean/NodeSettleStatis.java)
- [Amount serializer](https://github.com/PlatONnetwork/browser-server/blob/6daa5ad2e878474869407314a73a219ac49c8b51/scan-service/src/main/java/com/platon/browser/config/json/CustomLatSerializer.java)
- [Cumulative counters on re-staking](https://github.com/PlatONnetwork/browser-server/blob/6daa5ad2e878474869407314a73a219ac49c8b51/scan-common/src/main/resources/custommapper/StakeCreateMapper.xml)
- [Expected block accumulation](https://github.com/PlatONnetwork/browser-server/blob/6daa5ad2e878474869407314a73a219ac49c8b51/scan-common/src/main/resources/custommapper/EpochConsensusMapper.xml)
- [Protocol reward allocation](https://github.com/PlatONnetwork/PlatON-Go/blob/9037f54ec5cb848e2617ce031d8fe8717986a631/x/plugin/reward_plugin.go#L398-L485)

Protocol cross-check confirms delegation proportions apply to eligible block/staking rewards, not transaction fees, with per-event eligibility and rounding. Applying today's proportion to lifetime gross rewards is invalid.
