# Validator metrics on Home and Node detail

Status: interview decisions confirmed through Q21 and test seams approved. The synthesized implementation specification is published as [GitHub issue #153](https://github.com/mowind/PlatPulse/issues/153), labeled ready-for-agent. The first approved slice — explicit per-Network PlatScan deployment binding and the cumulative Validator block count on both Node views — is implemented by #154. The second slice — gross cumulative Validator rewards, including the delegator allocation, on both Node views with exact-decimal formatting — is implemented by #155. The third slice — the cumulative block production completion rate and PlatScan's own 24-hour production rate on both Node views — is implemented by #156. The fourth slice — the currently effective delegation reward distribution percentage on both Node views — is implemented by #157. The fifth slice — the Network-scoped PlatScan ranking from the shared `aliveStakingList` cohort on both Node views — is implemented by #158. The sixth slice — the current-selection, per-Network deduplicated cumulative-block and gross-reward Home summary with independent coverage metadata — is implemented by #159.

## Confirmed metric definitions

The six metrics belong to the currently linked Validator, not the monitored Node instance. See CONTEXT.md for the canonical definitions: Validator Lifetime Block Count, Validator Lifetime Rewards, Validator Rank, Validator Block Production Completion Rate, PlatScan 24-Hour Block Production Rate, and Effective Delegation Reward Distribution Ratio.

- Cumulative metrics cover the Validator's chain history, not monitoring or link tenure.
- The cumulative completion rate (#156) is computed by the Server from one successful observation's `blockQty` / `expectBlockQty` × 100%, so a fresh numerator is never divided by an older denominator. A known zero denominator is reported `not_applicable` (no scheduled duties), an incomplete pair is `unknown`, and only `ok` carries a value. It is not an exact missed-block rate: whole-round scheduled duties are counted before they elapse (`100/110` → `90.909091`, displayed `90.91%`).
- The UI metric 24小时出块率 (#156) is PlatScan's own `genBlocksRate`, normalized from its optional trailing `%` and shown directly, including a source-reported `0%`. The investigated implementation sums the preceding seven settlement periods excluding the current one and returns `0%` for absent evidence or an upstream error, so the value is labeled PlatScan口径 and never presented as a strict rolling 86,400 seconds or as proof of no production duties. Local failure or a missing field is `unknown`, never a synthesized `0%`.
- Cumulative rewards (UI: 累计收益) include both operator and delegator shares of Validator rewards. Use the PlatScan gross rewardValue meaning, not operator net earnings; exclude principal and ordinary transfers.
- Effective delegation reward distribution (UI: 委托奖励比例, #157) is the currently effective percentage of applicable Validator rewards allocated to delegators, normalized to percentage points so a source `rewardPer` of 20 is 20%, never 0.20% or 2000%. It is distinct from the annualized yield and from the pending `nextRewardPer`, which is never read; a local failure retains the last-good percentage and a success that omits the field stays unknown.
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

## Implemented Home aggregation contract (#159)

- Each Public `PublicNetwork` carries a `validatorSummary` computed entirely by the Server. Home selects which already-computed Network groups to show; it never adds duplicate Node projections and never combines Networks.
- Per metric (blocks and rewards separately) the summary exposes `knownSum`, `expectedCount`, `valuedCount`, `staleCount`, and a `state` of `complete`, `partial`, or `unknown`. Both sums cross the API as decimal strings (or null when unknown). Blocks use integer digits without i64 saturation or JavaScript-number rounding; rewards preserve the source decimal precision. Individual Validator `blockCount` remains an integer. Consumers must not coerce the aggregate `knownSum` to a JavaScript number.
- Membership is the Network's Active Nodes (temporarily offline Active Nodes included, Retired excluded) and the distinct Validators referenced by effective Node Validator Links regardless of primary, standby, or observer role. One Validator with several linked Nodes counts once; the same identifier on another Network counts separately.
- Reward totals use exact string arithmetic (`accumulate_decimal`), so large and fractional values never round-trip through binary floating point. No value means Unknown rather than zero; a legitimate source zero stays a value; partial coverage is a known-values subtotal; contributing last-good values that are no longer current are counted stale. `linkedNodeCount` and `unlinkedNodeCount` are reported separately so an unlinked Node is never a zero-valued Validator.
- Home renders the `ValidatorTotalsSection` for the current filter directly (no hover or expansion required), showing each Network's block and reward totals, per-metric coverage, stale counts, and the unlinked Node count.

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
- Dedicated Server-side aliveStakingList collection is shared per Network for rank, not an implicit detail-failure fallback. Its failure must not erase independent detail metrics. This supersedes the old detail-only boundary, which validator-provider.md now reflects (#158).
- Default refresh remains approximately 60 seconds. Freshness follows the configured refresh interval (default two intervals); display successful fetch time and source cutoff when available. Failures retain last-good data and do not affect Node health.
- Source-limited monetary precision is accepted, not financial settlement accuracy.
- Existing rewardRate sourced from deleAnnualizedRate remains distinct from rewardPer delegation distribution.

## Confirmed ranking and Network boundaries

- Rank uses the Network's complete live staking ALL cohort without a name filter, including candidates, and adopts the upstream rank independently of Home filters. Only a complete successful list fetch can establish unranked; failure or incomplete pagination must not. Retain stale last-good rank when available.
- Server configuration explicitly binds each Network to its corresponding PlatScan deployment. This configuration support is in scope; a shared allowlist is not evidence that one deployment serves multiple Networks.
- Networks without a configured source show not configured. Full completion means all six capabilities work on correctly configured Networks with upstream support, not invented coverage of Networks without a PlatScan service.

## Deployment validation and remaining limitations

- Mainnet response compatibility was checked on 2026-09-17 against the user-designated `https://scan.platon.network/`. The deployment self-identifies as PlatON Mainnet (chain ID 210425); five ALL-cohort pages and one detail response are preserved byte-for-byte with request metadata and SHA-256 hashes. See [the mainnet validation record](platscan-mainnet-validation.md) for source/schema reconciliation and scope.
- The captured responses are replayed through the existing HTTP adapter, refresh, real temporary SQLite and Public API in `platscan_mainnet_capture_reaches_public_api_and_retains_independent_last_good`. No live endpoint is a CI dependency.
- The deployed browser-server revision, complete historical indexing and atomicity of multi-page results remain unknown. A Validator software `version`, HTTP Date, join time or leave time is not a deployment revision or source cutoff. Reset/correction and outage semantics continue to use controlled regression tests; one live capture cannot prove them for all upstream history.
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
- The consolidated scope was confirmed and the older validator-provider.md no-list-call boundary is reconciled: ranking is a dedicated shared-per-Network list, independently stored and displayed (#158).


## 2026-09-17 acceptance follow-up

The three identified follow-up items are implemented:

- Actual mainnet detail, full declared ALL cohort, configuration and provenance are captured in [the deployment validation record](platscan-mainnet-validation.md). The unmodified list/detail bodies pass the existing HTTP adapter → refresh → temporary SQLite → Public API path, including independent rank/detail last-good retention after controlled failures.
- Linked Validator metrics now use two shrinkable columns on Home cards and Node detail. Playwright checks actual three-row/two-column geometry, long identities, full-precision reward values and cell/page overflow, including the 360px detail layout.
- Block totals use exact decimal strings across the Server/API/client/UI rather than saturated i64 values or JavaScript numbers. Public API and rendering regressions cover `i64::MAX + 1`, `i64::MAX + i64::MAX`, `2^53 + 1`, and valid zero; per-Validator counts are unchanged.

Checks actually run for this follow-up:

| Check | Result |
| --- | --- |
| Raw capture SHA-256 verification | All seven recorded response bodies match provenance |
| `cargo fmt --check` | Passed |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed |
| `cargo test --workspace` | Passed, including 444 Server library tests and the mainnet capture replay |
| Server `--print-openapi` compared with committed-path spec | Byte-identical to the updated spec |
| Regenerate browser client | Identical hashes before/after regeneration |
| Web lint, strict typecheck, tests, production build | Passed; 351 tests |
| `linked-validator.spec.ts` + `validator-summary.spec.ts` | 25/25 passed across phone-360-touch, phone-390-touch, tablet-768-touch, desktop-1280, desktop-1440 |
| `git diff --check` | Passed |
| `cargo deny check` / `cargo audit --ignore RUSTSEC-2023-0071 --ignore RUSTSEC-2026-0253` | Failed on existing dependency advisory findings; see below |

Dependency safety is **not green**: the unchanged lockfile contains `rustls 0.23.43`, affected by [RUSTSEC-2026-0285](https://rustsec.org/advisories/RUSTSEC-2026-0285), whose reported fix is >=0.23.45. The tools also report unmaintained `derivative`/`paste` and yanked `chacha20` warnings. No dependency update or advisory suppression was included in this Validator acceptance patch. These failures must not be described as a successful security check. The complete unrelated Playwright suite was not rerun; the five-project Validator acceptance suites above were run.

## Upstream source verification

Research examined official browser-server revision `6daa5ad2e878474869407314a73a219ac49c8b51`. The [2026-09-17 mainnet capture](platscan-mainnet-validation.md) independently establishes response compatibility for that deployment at capture time and supplies the deployment fixtures. Neither source research nor this capture establishes the deployed revision, complete indexing, financial accuracy or future service compatibility.

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
