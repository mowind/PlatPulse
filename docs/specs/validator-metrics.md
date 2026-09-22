## Problem Statement

PlatPulse users cannot see six Validator metrics together on Home Node cards or Node detail: cumulative produced blocks, cumulative rewards, Network rank, historical block production rate, PlatScan's 24-hour production rate, and effective delegation reward distribution ratio. Home also lacks trustworthy cumulative-block and reward totals for the current selection.

Existing Server Validator Registry, Node Validator Links, Provider refresh, current projections and analytics provide a foundation, but current field aliases do not establish all required meanings. Node and Validator identities differ: a primary, standby and observer can share one Validator, so summing Node values would double-count. Unknown or stale observations, multiple Networks and precision loss can make apparently simple totals misleading.

## Solution

Extend the existing Server-owned Validator pipeline and Public API, then show all six metrics in a dedicated linked-Validator section on Home cards and Node detail. Add Home cumulative-block and cumulative-reward summaries for the current selection, deduplicated by Validator and grouped by Network.

Use PlatScan's gross cumulative reward value, including allocations to delegators. Use its detail response directly for the UI metric “24小时出块率”, explicitly identified as PlatScan's definition rather than a strict rolling 86,400-second window. Preserve source provenance, last-good values, availability and freshness. Deliver all six capabilities end to end on supported, correctly configured Networks; permanent unsupported placeholders do not satisfy completion.

## User Stories

1. As a Home viewer, I want cumulative produced blocks on each Node card, so that I can understand its linked Validator's historical production.
2. As a Home viewer, I want cumulative rewards including delegator allocations, so that I can see the Validator's gross reward total rather than operator net earnings.
3. As a Home viewer, I want a Network-scoped PlatScan rank, so that I can compare a Validator with its actual staking cohort.
4. As a Home viewer, I want cumulative actual-versus-scheduled production, so that I can understand historical production against planned duties.
5. As a Home viewer, I want PlatScan's 24-hour production-rate value, so that I can compare the dashboard with that source.
6. As a Home viewer, I want the effective delegation reward distribution percentage, so that I understand the share allocated to delegators.
7. As a Node detail viewer, I want all six metrics together, so that I do not have to navigate to a separate Network card.
8. As a Home viewer, I want cumulative-block totals for my current filters, so that the summary describes the Nodes I am viewing.
9. As a Home viewer, I want cumulative-reward totals for my current filters, so that the summary changes consistently with the cards.
10. As a viewer of primary and standby Nodes, I want a shared Validator counted once, so that redundant monitoring does not inflate totals.
11. As a viewer filtering to a standby or observer Node, I want its linked Validator included once, so that role alone does not erase relevant metrics.
12. As a multi-Network viewer, I want separate block and reward totals per Network, so that unrelated chains and assets are not combined.
13. As an operator, I want temporarily offline Active Nodes to remain in scope, so that connectivity loss does not silently remove their Validator totals.
14. As a Home viewer, I want Retired Nodes excluded, so that summaries follow the current monitored inventory.
15. As a viewer of an unlinked Node, I want an explicit unlinked state, so that missing association is not mistaken for zero rewards.
16. As a Home viewer, I want unlinked Node counts shown separately, so that I understand the limits of aggregate coverage.
17. As a viewer, I want incomplete aggregates labeled known-values subtotals, so that partial data is not presented as a complete total.
18. As a viewer, I want independent coverage counts for blocks and rewards, so that missing one metric does not hide completeness differences.
19. As a viewer, I want stale contributors identified in totals, so that I can judge their timeliness.
20. As a viewer, I want last-good values retained after source failure, so that useful history does not disappear.
21. As a viewer, I want unknown values to remain unknown, so that collection failure is not represented as zero.
22. As a Node detail viewer, I want source, successful-fetch time and source cutoff when available, so that I can assess provenance and age.
23. As a viewer, I want safe explanations for unavailable values, so that I can distinguish missing data from an unconfigured source without seeing private diagnostics.
24. As a viewer, I want a zero historical expected-block denominator shown as not applicable, so that no duties is not confused with perfect or failed performance.
25. As a viewer, I want the historical rate's whole-round scheduling caveat, so that duties not yet elapsed are not mistaken for missed blocks.
26. As a viewer, I want the PlatScan window and ambiguous-zero caveats, so that source-reported values are not overclaimed as independently verified facts.
27. As a viewer, I want a legitimate source-reported 0% retained, so that the source metric is reproduced without fabricating a different result.
28. As a viewer, I want unranked distinguished from failed or incomplete ranking retrieval, so that an outage does not look like removal from the cohort.
29. As a viewer, I want Home filters not to change Network rank, so that rank keeps a stable meaning.
30. As a Node detail viewer, I want Validator identity and link role visible, so that I know which chain identity the metrics describe.
31. As an operator changing a Node Validator Link, I want values to switch to the new Validator, so that histories of different identities are not spliced together.
32. As a mobile viewer, I want all six metrics visible without hover or expansion, so that essential information is accessible on a narrow touch screen.
33. As a detail viewer, I want all available source precision, so that abbreviated cards do not prevent closer inspection.
34. As a viewer of large monetary totals, I want exact decimal aggregation, so that floating-point rounding does not corrupt totals.
35. As an Owner, I want an explicit PlatScan deployment configured per Network, so that a mainnet service is not accidentally used for testnet.
36. As an Owner, I want an unconfigured Network shown explicitly, so that lack of source coverage is distinguishable from a dead Node.
37. As an operator, I want freshness to follow the configured refresh interval, so that valid slower refresh settings do not immediately look stale.
38. As an operator, I want Provider failures isolated from Node health, so that monitoring the data source does not misdiagnose the Node.
39. As a viewer, I want a falling cumulative counter identified as reset or correction, so that it is not silently rewritten as normal growth.
40. As an API consumer, I want typed metric values and states consistent with the WebUI, so that I can consume the same trustworthy semantics.

## Implementation Decisions

### Scope and existing architecture

- Extend the existing Server Validator Provider, refresh/persistence, Public Projection, configuration and WebUI modules. Do not build a parallel Agent collection pipeline or a standalone indexer.
- The Server remains the trust boundary and the only PlatScan client. Agent and browser do not call PlatScan. Keep public and administrative contracts distinct; public explanations must not leak endpoints, credentials or raw provider diagnostics.
- Validator identity is Network-scoped and independent of Node ID. Use explicit effective Node Validator Links; do not infer ownership from consensus membership or Node role. A single Validator's metrics can appear on several Node cards.
- Preserve existing independent Agent Report, Block History and receipt semantics. Observed chain blocks are not Validator-produced blocks. The existing Agent Store lifecycle ADR is not changed.

### Six-metric contract

| User-facing metric | Authoritative mapping | Meaning |
| --- | --- | --- |
| 累计出块数 | detail blockQty | Validator-identity cumulative produced-block count, not monitoring duration |
| 累计收益 | detail rewardValue | Gross cumulative Validator rewards including operator and delegator shares; excludes principal and ordinary transfers |
| 排名 | aliveStakingList ranking | Same-Network ALL live-staking cohort, including candidates, without name filter |
| 出块率 | same-response blockQty / expectBlockQty × 100% | Cumulative actual / cumulative scheduled blocks; whole-round scheduled duties are accepted |
| 24小时出块率 | detail genBlocksRate | Direct source-defined metric; not a locally reconstructed exact rolling window |
| 委托奖励比例 | detail rewardPer | Current effective percentage allocated to delegators, not nextRewardPer or annualized yield |

- Obtain numerator and denominator for historical completion from the same successful observation. Never combine a fresh numerator with an older denominator. A known zero denominator is not applicable; missing evidence is unknown. Do not interpret the result as an exact historical missed-block rate.
- Accept valid genBlocksRate values with the upstream percentage representation, including source-returned zero; normalize explicitly. Label the metric “PlatScan口径”. Explain that the investigated implementation uses the preceding seven settlement epochs excluding the current one, and can return zero for unavailable evidence or upstream error. This exception is a source-reporting policy, not permission to synthesize zero on local failure.
- Normalize rewardPer as percentage points: source 20 means 20%, not 0.20% or 2000%. Keep existing annualized reward/yield semantics separate; do not repurpose the existing annualized field as delegation distribution.
- Preserve bounded, validated exact decimal amount representations and exact decimal arithmetic for totals; do not aggregate using binary floating point. Use Network native units. Detail preserves available source precision; cards may abbreviate and displayed percentages default to two decimal places. Do not invent precision beyond the source.
- Extend current observations and persistence only as needed for these metrics and their provenance/state. Migrate existing data safely: historical rows missing new fields remain unknown rather than acquiring fabricated values. Preserve established counter-reset/correction semantics and existing analytics compatibility; historical reconstruction of new fields is not required.

### Ranking and configuration

- Add dedicated Server-side aliveStakingList collection shared per Network, not repeated per monitored Node and not a silent fallback after detail failure. This explicitly replaces the previous detail-only/no-list-call constraint for ranking acquisition.
- Adopt upstream ALL-cohort ranks without recomputing them over monitored Nodes or Home filters. The researched ordering is version descending, total stake descending, staking block ascending, staking transaction index ascending.
- A complete successful list retrieval is required to establish that a Validator is unranked. Failed, interrupted or detectably inconsistent pagination must not clear last-good ranks or create false unranked states. Bound list fetching and validate response shape; do not claim atomicity the upstream does not provide.
- Detail and ranking retrieval have independent success/failure and freshness: failed ranks do not erase successful detail values and vice versa.
- Explicitly bind each Network to the correct PlatScan deployment. The detail request identifies a Validator but has no Network selector; an allowlist is not proof that one deployment serves several Networks. Unconfigured Networks have an explicit not-configured outcome.
- Retain approximately 60-second default refresh. Determine stale status relative to configured intervals, defaulting to two intervals, and mark failed collection with retained values as degraded/stale. Expose last successful fetch time and upstream cutoff if provided. Never fabricate the latter from receipt time.

### Home aggregation and Public API

- The Server owns eligible-set selection, Validator deduplication, Network separation, aggregate arithmetic and coverage metadata. Extend the Public API and generated client so summary selection matches the existing Home filters; do not independently sum duplicated Node projections in the browser.
- Include Active Nodes in the selected scope even if temporarily offline; exclude Retired Nodes. Aggregate the distinct Validators referenced by effective links regardless of primary, standby or observer role.
- Partition by Network and deduplicate by Network plus Validator identifier. Do not combine mainnet/testnet amounts or block totals. A filter leaving only a standby Node still includes its Validator once.
- For each of blocks and rewards separately, expose known-value sum, eligible distinct-Validator count, count with values and stale-contributor count. Use available last-good values. Incomplete coverage must be labeled as a known-values subtotal; when no values exist, show unknown rather than zero. Report unlinked Node counts separately.
- Current-selection totals are not an ownership ledger and are not guaranteed monotonic: changing filters, retirement or effective links can reduce them. If a Node changes from Validator A to B, use B's full cumulative value, never A+B.
- Extend typed OpenAPI/Public DTOs and regenerate the TypeScript client. Keep source-reported, unknown, stale, unranked, unlinked, unconfigured and not-applicable outcomes distinguishable rather than overloading a nullable number.

### Web presentation

- Add current-selection, per-Network cumulative-block and gross-reward summaries on Home. This does not require redesigning unrelated existing health summaries.
- Home Node cards and Node detail display the six metrics in a linked-Validator section, with Validator identity and role. Mobile uses a two-column arrangement and never hides required metrics behind hover or expansion.
- Detail provides source-limited full values, update metadata, sanitized state explanations and rate caveats. Distinguish fetch success from independently verified upstream computation, particularly for source-reported 0%.
- Preserve last-good values after failure and clearly mark age/state. Never clear other valid metrics because a separate ranking fetch failed. Keep Provider state independent of Node health and Server readiness.
- Reconcile the existing Provider contract and design documentation with these decisions; do not leave the former ban on list calls as the active contract.

## Testing Decisions

The user explicitly approved these test seams before publication. Prefer externally observable behavior and existing high-level integration infrastructure over new interfaces or tests of private helpers.

1. **Primary seam: controlled PlatScan HTTP responses → existing refresh/persistence with real temporary SQLite → Public API responses.** Extend the existing request-recording mock PlatScan service, Validator refresh/last-good tests and Public Projection/link-scope tests. Use realistic detail and paginated ranking fixtures. Assert API values, status, filtering and externally relevant HTTP contracts, not private call graphs, table layout or helper names. No repository abstraction or global dependency container is needed.
2. **UI seam: existing React Testing Library and Playwright infrastructure.** Extend existing ValidatorInsight exact-value/freshness/error tests and public API tests; verify Home summaries, Node cards, Node detail and navigation through user-visible content. Reuse current mobile navigation and no-horizontal-overflow conventions. Avoid making live public PlatScan availability a deterministic test dependency.
3. **Normal-path coverage:** all six metrics; gross rewards include delegator shares; exact decimal sum of large and fractional values; rewardPer unit scaling; percentage-string normalization; unchanged annualized-yield meaning; same-observation historical rate; source precision retained.
4. **Identity and aggregation coverage:** multiple Node roles sharing one Validator; standby-only filtered selection; same identifier across different Networks; filter changes; offline Active inclusion; Retired exclusion; unlinked counts; link switch from A to B; distinct block/reward coverage and stale counts.
5. **Failure/state coverage:** missing and malformed optional fields; local request failure; never observed versus zero; last-good preservation; authoritative historical denominator zero; genBlocksRate source-reported zero versus local unknown; unconfigured Network; counter reset/correction; custom refresh periods; provider timestamps absent; public diagnostic redaction.
6. **Ranking coverage:** exact ALL cohort and no name filter; ranking shared by Network; full-list absence yields unranked; partial/failed pagination does not; retained stale rank on failure; independence of rank and detail updates; no false guarantee of an atomic multi-page snapshot.
7. **Numeric examples:** cumulative 100 actual / 110 scheduled yields approximately 90.91% with scheduling caveat; source rewardPer 20 renders 20%; a source genBlocksRate 0% remains source-reported while a failed request without last-good data remains unknown; duplicate linked Nodes do not double the cumulative totals.
8. **Responsive acceptance:** run the fixed phone-360-touch, phone-390-touch, tablet-768-touch and desktop-1280 projects. All six metrics remain directly discoverable without hover/expansion, long identities and amounts do not create horizontal overflow, and full-detail values remain available.
9. **Contract/regression checks:** OpenAPI generation and client consistency; relevant Rust formatting, lint and workspace tests; Web lint, strict typecheck, tests and production build; existing Public/Admin separation and Node-health behavior. Report exactly what ran and any environment constraints. Any schema changes include migration coverage preserving existing last-good data.
10. **Source validation:** fixtures must cite the investigated official source revision and reconcile any schema/implementation discrepancy. Before claiming deployment compatibility, validate an actual supported deployment response or an explicitly versioned supported contract; document the evidence and any remaining live-environment limitation. Controlled tests are not proof of complete upstream historical indexing.

## Out of Scope

- Operator net lifetime earnings, subtraction of delegator allocations, or applying today's delegation percentage retroactively to cumulative gross rewards.
- A strict rolling 86,400-second production metric, a new chain-history indexer, timestamped duty reconstruction or replacing the selected source metric with a local proxy.
- Deriving produced-block counts from monitored Block History, Coinbase or signer matches.
- Cross-Network totals, fiat conversion, financial settlement accuracy or invented precision beyond PlatScan.
- Agent changes for economic metrics, direct browser access to PlatScan, remote control, automatic Validator ownership inference or new Node identities during transfer.
- Ranking only monitored Nodes, changing rank according to Home filters, or making pending delegation ratios the current effective metric.
- New historical charts, backfilling all historical analytics for new fields, or unrelated Home/Admin redesign.
- Guaranteed metrics for Networks with no configured compatible PlatScan service. Such Networks must report their actual configuration/availability state.
- Shipping permanent unsupported placeholders in place of implementing any of the six capabilities on supported configured Networks.

## Further Notes

- This spec supersedes the earlier interview choices of operator-only earnings and strict rolling-24-hour completion. The final accepted choices are gross cumulative rewards and direct PlatScan 24-hour-rate semantics. All 21 interview decisions and the proposed test seams were confirmed before publication.
- Full completion includes Server data acquisition, persistence/projection, Public API/client, Home summaries, both Node presentations, configuration, documentation and tests. Legitimate unavailable/unlinked/unranked/not-applicable states remain necessary even when the feature is complete.
- Existing code already provides the Validator foundation; this is an extension, not a greenfield rewrite. Earlier repository wording describing the whole project as pre-implementation is not the implementation baseline.
- Official browser-server revision **6daa5ad2e878474869407314a73a219ac49c8b51** supplied the investigated mappings. This verifies source behavior, not the revision, completeness, atomicity, SLA or quotas of any deployed service. Its API definition and Java response shape have discrepancies, so neither should be accepted blindly.
- In the investigated implementation, node-identity cumulative counters survive re-staking, gross rewards have special treatment for initial nodes, and amount serialization truncates LAT values downward to 12 decimal places. Present this as source-limited monitoring data, not verified full-chain accounting.
- Primary evidence: [detail mapping and ranking](https://github.com/PlatONnetwork/browser-server/blob/6daa5ad2e878474869407314a73a219ac49c8b51/scan-api/src/main/java/com/platon/browser/service/StakingService.java), [source production window](https://github.com/PlatONnetwork/browser-server/blob/6daa5ad2e878474869407314a73a219ac49c8b51/scan-common/src/main/java/com/platon/browser/bean/NodeSettleStatis.java), [amount precision](https://github.com/PlatONnetwork/browser-server/blob/6daa5ad2e878474869407314a73a219ac49c8b51/scan-service/src/main/java/com/platon/browser/config/json/CustomLatSerializer.java), [re-staking counters](https://github.com/PlatONnetwork/browser-server/blob/6daa5ad2e878474869407314a73a219ac49c8b51/scan-common/src/main/resources/custommapper/StakeCreateMapper.xml).
- The existing Agent Store receipt-lifecycle ADR was reviewed and is unaffected. No new ADR is required merely to publish this feature spec.
