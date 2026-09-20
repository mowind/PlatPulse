# Validator metrics on Home and Node detail

Historical metric-delivery baseline: the earlier metrics interview decisions were confirmed through its Q21 and test seams were approved. The synthesized implementation specification is published as [GitHub issue #153](https://github.com/mowind/PlatPulse/issues/153), labeled ready-for-agent. The first approved slice — explicit per-Network PlatScan deployment binding and the cumulative Validator block count on both Node views — is implemented by #154. The second slice — gross cumulative Validator rewards, including the delegator allocation, on both Node views with exact-decimal formatting — is implemented by #155. The third slice — the cumulative block production completion rate and PlatScan's own 24-hour production rate on both Node views — is implemented by #156. The fourth slice — the currently effective delegation reward distribution percentage on both Node views — is implemented by #157. The fifth slice — the Network-scoped PlatScan ranking from the shared `aliveStakingList` cohort on both Node views — is implemented by #158. The sixth slice — the current-selection, per-Network deduplicated cumulative-block and gross-reward Home summary with independent coverage metadata — is implemented by #159.

## Status of the new identity design

Automatic Validator identification and the associated history cutover are
**accepted, NOT IMPLEMENTED**. The target is defined by
[the main design, section 15](platpulse.md#accepted-management-target),
[ADR 0005: automatic Validator identity](../adr/0005-automatic-validator-identity.md)
and the [accepted Provider identity contract](validator-provider.md#accepted-target-automatic-validator-identity-not-implemented).
This document retains the historically implemented #154–#159 metric behavior
and deployment-validation results; those results do not establish that the new
identity, classification or migration paths exist. No implementation, migration,
live query or test rerun is claimed by this documentation synchronization.

## Accepted identity and history scope (not implemented)

- Automatically identify the Validator using the Node's validated Network and
  observed full 64-byte P2P public key, not its PlatPulse UUID, shortened
  fingerprint, display name or IP. The Server queries the Network-specific
  PlatScan deployment; the Agent and WebUI do not query it.
- No manual registration or binding is required from the user, and there are no
  primary/standby/observer roles in the target. Missing or invalid identity and
  an unavailable Network-specific provider have explicit reasons; they do not
  establish Not Validator or a zero-valued metric.
- Current Validator Status means currently valid staking identity, not current
  consensus selection. Confirmed candidates/active/producing identities count;
  locked/exiting identities count only while staking validity is confirmed and
  their special state remains visible. Completed exit or authoritative absence
  is Not Validator; verifying or inconclusive evidence is Unknown. Lookup
  failures retain established last-good values as stale, not fresh negatives.
- **Technical verification recorded ([#168](../research/platscan-current-validator-status-evidence.md)):**
  the precise PlatScan validity predicates — including locked/exiting validity,
  authoritative absence, and the exclusions for HTTP 404, transport failure and
  ranking absence — are established from browser-server and PlatON-Go source
  evidence plus a captured mainnet deployment. The historical Activity
  normalization and mainnet capture below still do not prove those predicates;
  the target must not guess validity from status labels, ranking absence or a
  transport error.
- An observed key change closes the old association interval and identifies the
  new Validator. Node monitoring history stays intact; never merge the two
  Validators' cumulative metrics or histories. Old manual links are not a
  fallback during automatic identification or outages.
- The one-time migration removes old manual links and Validator snapshots,
  ranking/counter history, and daily/monthly aggregates, **not Node monitoring
  history**. Reset incompatible current baselines/classifications with that
  cutover, retain existing Incident/Audit evidence, and do not turn the cleanup
  into a fake counter-reset/rank-change alert or recovery. New observations
  establish new baselines; analytics must not recreate old periods from stale
  caches. The source's chain-history totals may still be nonzero after local
  history is cleared.
- Ordinary Node Purge removes that Node's monitoring data and links, **not
  independent Validator history**, even for the last associated Node. Other
  Nodes using the same Validator remain unaffected. This is distinct from the
  explicitly accepted one-time migration.

## Metric definitions retained by the accepted target

The six metrics belong to the automatically identified Validator, not the monitored Node instance. See [the domain glossary](../../CONTEXT.md) for the canonical definitions: Validator Lifetime Block Count, Validator Lifetime Rewards, Validator Rank, Validator Block Production Completion Rate, PlatScan 24-Hour Block Production Rate, and Effective Delegation Reward Distribution Ratio.

- Cumulative metrics cover the Validator's chain history, not monitoring or link tenure.
- The cumulative completion rate (#156) is computed by the Server from one successful observation's `blockQty` / `expectBlockQty` × 100%, so a fresh numerator is never divided by an older denominator. A known zero denominator is reported `not_applicable` (no scheduled duties), an incomplete pair is `unknown`, and only `ok` carries a value. It is not an exact missed-block rate: whole-round scheduled duties are counted before they elapse (`100/110` → `90.909091`, displayed `90.91%`).
- The UI metric 24小时出块率 (#156) is PlatScan's own `genBlocksRate`, normalized from its optional trailing `%` and shown directly, including a source-reported `0%`. The investigated implementation sums the preceding seven settlement periods excluding the current one and returns `0%` for absent evidence or an upstream error, so the value is labeled PlatScan口径 and never presented as a strict rolling 86,400 seconds or as proof of no production duties. Local failure or a missing field is `unknown`, never a synthesized `0%`.
- Cumulative rewards (UI: 累计收益) include both operator and delegator shares of Validator rewards. Use the PlatScan gross rewardValue meaning, not operator net earnings; exclude principal and ordinary transfers.
- Effective delegation reward distribution (UI: 委托奖励比例, #157) is the currently effective percentage of applicable Validator rewards allocated to delegators, normalized to percentage points so a source `rewardPer` of 20 is 20%, never 0.20% or 2000%. It is distinct from the annualized yield and from the pending `nextRewardPer`, which is never read; a local failure retains the last-good percentage and a success that omits the field stays unknown.
- Rank follows PlatScan within the same Network.
- Historical completion is same-response blockQty / expectBlockQty × 100%, labeled cumulative actual / cumulative scheduled blocks. Whole-round scheduling is accepted; it can transiently lower the rate before duties elapse and is not an exact missed-block rate.
- The UI metric 24小时出块率 is taken directly from PlatScan stakingDetails.genBlocksRate using upstream semantics, not a locally reconstructed strict rolling-24-hour window. This explicitly supersedes the earlier strict rolling-window requirement.
- Delegation distribution means the currently effective share allocated to delegators, not APY, operator commission, or a pending ratio.

## Accepted Home aggregation target

- Aggregate metrics follow the current Home filters and include automatically linked Active Nodes. Temporarily offline Active Nodes remain eligible; Retired and purged Nodes do not. Label the scope as the current selection.
- Deduplicate by Network plus Validator identifier through automatic Node Validator Links, without roles. A Validator counts once if any eligible Node has an effective association.
- Group both cumulative block counts and earnings by Network. Do not combine Networks or perform fiat conversion.
- Aggregate available last-good values. An incomplete aggregate is a known-values subtotal, not a complete total. Show independent coverage and stale counts for each metric, for example 9/10 Validators with values, including one stale.
- Show unknown when no values are available. Show Nodes without an established automatic association separately; do not treat them as zero-value Validators or ask the user to bind them manually.
- These are metrics of linked Validators, not a claim of asset ownership. Aggregates can decrease when filters, effective links, or Active membership change.

## Implemented Home aggregation baseline (#159; explicit-Link model)

- Each Public `PublicNetwork` carries a `validatorSummary` computed entirely by the Server. Home selects which already-computed Network groups to show; it never adds duplicate Node projections and never combines Networks.
- Per metric (blocks and rewards separately) the summary exposes `knownSum`, `expectedCount`, `valuedCount`, `staleCount`, and a `state` of `complete`, `partial`, or `unknown`. Both sums cross the API as decimal strings (or null when unknown). Blocks use integer digits without i64 saturation or JavaScript-number rounding; rewards preserve the source decimal precision. Individual Validator `blockCount` remains an integer. Consumers must not coerce the aggregate `knownSum` to a JavaScript number.
- Membership is the Network's Active Nodes (temporarily offline Active Nodes included, Retired excluded) and the distinct Validators referenced by effective Node Validator Links regardless of primary, standby, or observer role. One Validator with several linked Nodes counts once; the same identifier on another Network counts separately.
- Reward totals use exact string arithmetic (`accumulate_decimal`), so large and fractional values never round-trip through binary floating point. No value means Unknown rather than zero; a legitimate source zero stays a value; partial coverage is a known-values subtotal; contributing last-good values that are no longer current are counted stale. `linkedNodeCount` and `unlinkedNodeCount` are reported separately so an unlinked Node is never a zero-valued Validator.
- Home renders the `ValidatorTotalsSection` for the current filter directly (no hover or expansion required), showing each Network's block and reward totals, per-metric coverage, stale counts, and the unlinked Node count.

## Accepted Node presentation target and edge cases

- Both Home Node cards and Node detail expose all six metrics. Mobile must not require hover or expanding a section to see them; use a dedicated linked-Validator section with a two-column mobile layout.
- Display automatically identified Validator identity and Current Validator Status, not a manual link control or role. On an observed identity change from A to B, display B's metrics only after B is identified; otherwise show the pending/unknown reason rather than falling back to A. Never splice A and B histories.
- Nodes without an established automatic association show its absence/reason, not zero or a manual-binding prompt. Unknown metrics remain unknown. Retain last-good values for the same established identity after failures and label staleness and update time; neither lookup failure nor historical identity alone proves current valid stake. Provider failure must not change Node health.
- With complete evidence and zero expected blocks, completion is not applicable / no production duties, not 0% or 100%. Missing evidence is insufficient data.
- The source-defined 24小时出块率 is displayed directly when the detail request succeeds and genBlocksRate is valid, including source-returned 0%. Mark it PlatScan口径 and explain the preceding-seven-settlement-period window and ambiguous zero (which can reflect absent evidence or upstream errors). This is a source-reported value, not verified zero performance. Local failures/missing fields never synthesize zero, and upstream 0% never establishes no production duties.
- Node detail shows provenance, update time, and reasons for unavailable or degraded data.
- Earnings use the Network native unit; cards may abbreviate. Detail exposes all available source precision, without reconstructing digits absent from the source. Source-limited precision is accepted: exact decimal processing, abbreviated cards, and available source precision in detail; default displayed percentages use two decimal places.

## Metric delivery and Provider scope retained by the target

- Preserve all six end-to-end metric capabilities while changing identity discovery; permanent unsupported placeholders do not satisfy completion. Legitimate unidentified, missing, failed, or not-applicable outcomes remain explicit.
- Dedicated Server-side aliveStakingList collection is shared per Network for rank, not an implicit detail-failure fallback. Its failure must not erase independent detail metrics. This supersedes the old detail-only boundary, which validator-provider.md now reflects (#158).
- Default refresh remains approximately 60 seconds. Freshness follows the configured refresh interval (default two intervals); display successful fetch time and source cutoff when available. Failures retain last-good data and do not affect Node health.
- Source-limited monetary precision is accepted, not financial settlement accuracy.
- Existing rewardRate sourced from deleAnnualizedRate remains distinct from rewardPer delegation distribution.

## Ranking and Network boundaries retained by the target

- Rank uses the Network's complete live staking ALL cohort without a name filter, including candidates, and adopts the upstream rank independently of Home filters. Only a complete successful list fetch can establish unranked; failure or incomplete pagination must not. Retain stale last-good rank when available.
- Server configuration explicitly binds each Network to its corresponding PlatScan deployment. This configuration support is in scope; a shared allowlist is not evidence that one deployment serves multiple Networks.
- Networks without a configured source show not configured. Full completion means all six capabilities work on correctly configured Networks with upstream support, not invented coverage of Networks without a PlatScan service.

## Historical deployment validation and remaining limitations

- Mainnet response compatibility was checked on 2026-09-17 against the user-designated `https://scan.platon.network/`. The deployment self-identifies as PlatON Mainnet (chain ID 210425); five ALL-cohort pages and one detail response are preserved byte-for-byte with request metadata and SHA-256 hashes. See [the mainnet validation record](platscan-mainnet-validation.md) for source/schema reconciliation and scope.
- The captured responses are replayed through the existing HTTP adapter, refresh, real temporary SQLite and Public API in `platscan_mainnet_capture_reaches_public_api_and_retains_independent_last_good`. No live endpoint is a CI dependency.
- The deployed browser-server revision, complete historical indexing and atomicity of multi-page results remain unknown. A Validator software `version`, HTTP Date, join time or leave time is not a deployment revision or source cutoff. Reset/correction and outage semantics continue to use controlled regression tests; one live capture cannot prove them for all upstream history.
- Prior research blockers for operator net earnings and strict rolling24h below are retained as evidence for rejected mappings; those two requirements have now been superseded.

## Acceptance checklist for the new target (not yet run)

- Server exposes all six validated metrics and their availability/freshness/provenance; preserves existing annualized-yield semantics separately from the new delegation ratio.
- Server owns aggregate membership, deduplication, Network separation, exact decimal arithmetic, and per-metric coverage/staleness. API results follow the same selection as the Home cards, not a browser-only sum of duplicate Node metrics.
- Preserve cumulative blocks and gross cumulative rewards for the current Home selection per Network. Node cards and Node detail render all six metrics, automatically identified Validator identity, Current Validator Status, and the confirmed source explanations; no manual binding or role remains in the target.
- Validate direct rewardValue mapping, rewardPer percentage scaling, optional trailing-percent genBlocksRate parsing, historical zero denominator, missing/invalid fields, and source-reported zero distinct from locally unknown. Preserve amount precision without floating-point summation.
- Validate multiple Nodes automatically matching one Validator, full-key and Network validation, missing identity, key changes, offline Active versus Retired/purged Nodes, unidentified Nodes, filters, partial aggregates, source failures, stale refresh, genuine counter resets/corrections, list paging failures and genuine unranked outcomes. Cover confirmed-valid locked/exiting status, completed exit, and verifying/inconclusive Unknown with primary-evidence-backed classification fixtures.
- Validate separate Network sources and unconfigured Network behavior; no cross-Network identity association, amount or block aggregation. Old manual links must never serve as an automatic-discovery fallback.
- Validate the one-time history cutover: remove only the accepted old Validator data and links, reset incompatible current baselines/classifications, retain Node monitoring history and Incident/Audit evidence, reject stale pre-cutover writes/rebuild inputs, and produce no invented historical samples, reset/change alerts or recovery events. Ordinary Node Purge must preserve independent Validator history, including when no associated Node remains.
- Generate the API client from updated OpenAPI and run relevant Rust/Web tests and fixed responsive projects (360/390/768/1280 widths). Document checks actually run and any environmental blockers.
- No permanent unsupported placeholders for the six requested capabilities on supported, correctly configured deployments. Transient failures and genuine missing/not-applicable states remain visible rather than being replaced by zero.
- The consolidated scope was confirmed and the older validator-provider.md no-list-call boundary is reconciled: ranking is a dedicated shared-per-Network list, independently stored and displayed (#158).


## 2026-09-17 acceptance follow-up

The following three items were implemented in the historical metrics follow-up. These results are retained as evidence; they are not a claim that the newly accepted automatic-identity target has been implemented or tested:

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
