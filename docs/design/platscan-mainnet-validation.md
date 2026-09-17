# PlatScan mainnet response validation for #153

## Result and scope

On **2026-09-17 UTC**, bounded, unauthenticated requests to the user-designated mainnet deployment, [scan.platon.network](https://scan.platon.network/), successfully retrieved the complete declared ALL live-staking cohort and one Validator detail selected from that cohort. The recorded responses contain the six requested metric inputs, use the expected object/detail and array/list envelopes, and require no response rewriting. This is actual deployment response evidence for [#153](https://github.com/mowind/PlatPulse/issues/153), not merely synthetic fixtures or upstream source inspection. It does **not** prove complete historical indexing, chain-minimal-unit accounting precision, or compatibility with every deployment/release.

The exact response bodies and per-request provenance are stored under [`crates/platpulse-server/tests/fixtures/platscan-mainnet/`](../../crates/platpulse-server/tests/fixtures/platscan-mainnet/). The [provenance manifest](../../crates/platpulse-server/tests/fixtures/platscan-mainnet/provenance.json) records URL, method, exact request body, HTTP status, local start/completion time, HTTP response Date and headers, byte count, and SHA-256. Each body was written as the UTF-8 HTTP entity body from curl **before any JSON parsing**, without reserialization or an appended newline. HTTP transfer framing is not part of the saved body. File hashes were verified with `sha256sum` after writing. No cookies, credentials, API keys, or mutation endpoints were used.

## Network binding and deployment version

The deployment itself identifies the Network in its public [configuration](https://scan.platon.network/browser-server/config.json): `headerChainName = "PlatON Mainnet"`, wallet `chainId = 210425`, native symbol `LAT`, and `blockExplorerUrl = "https://scan.platon.network"`. The [exact configuration capture](../../crates/platpulse-server/tests/fixtures/platscan-mainnet/config.json) and its request provenance corroborate the user-designated mainnet binding. This is deployment self-identification, **not** an independent observation of the genesis hash or P2P network ID. Bind only the registered mainnet Network intended by the operator to this URL; do not infer testnet support.

No browser-server build hash or release identifier was established. The selected detail returns `version: "1.5.0"`, but the [investigated service implementation][service] derives this from the Validator program version; it is **not the deployed explorer version**. The API definition at the investigated revision labels itself `v1.4.0.3 Build #43aae6736`; that label is also not evidence of the live deployment version. Compatibility evidence here is scoped to the captured endpoint behavior and capture time.

## Requests, bounds, and complete declared cohort

POST requests used `Content-Type: application/json`; the capture used these public endpoints:

- POST [`/browser-server/staking/aliveStakingList`](https://scan.platon.network/browser-server/staking/aliveStakingList), with exactly `{"pageNo":n,"pageSize":50,"queryStatus":"all"}` for `n = 1..5`; no `key`/name filter.
- POST [`/browser-server/staking/stakingDetails`](https://scan.platon.network/browser-server/staking/stakingDetails), with only `nodeId`, selected from rank 1 of page 1.
- GET [`/browser-server/config.json`](https://scan.platon.network/browser-server/config.json), for deployment Network evidence.

The six metric captures ran sequentially from `2026-09-17T09:05:12.934Z` to `2026-09-17T09:05:19.782Z`; their HTTP response Dates range from `09:05:13` to `09:05:19 GMT`. Each request had a 25-second timeout and 65,536-byte response limit, with five list pages as the capture bound. An earlier exploratory page-1 POST confirmed the endpoint and declared cohort size; it is not used as a fixture. The later configuration capture completed at `09:07:10.312Z`. All recorded requests returned HTTP 200; all list/detail envelopes returned integer `code: 0`. See the manifest for exact per-request timings and headers.

| Raw fixture | Rows | Bytes | SHA-256 |
| --- | ---: | ---: | --- |
| `alive-staking-list-page-1.json` | 50 | 29256 | `325c307f4ddfde1d761dcf2dff6fe986ff0709d10f39e2d27b7ccbb97819ce17` |
| `alive-staking-list-page-2.json` | 50 | 29397 | `33656ae56d492b55f5f924c0bd5d3e5303a4f4cb48da4bbd9e09a6620fea5281` |
| `alive-staking-list-page-3.json` | 50 | 29140 | `d8ff9c29bb1194788dc1472129993e2aed9350ab0d4ae11d2ff9ebc6f0007133` |
| `alive-staking-list-page-4.json` | 50 | 29175 | `cded61b6c3b6e11ec9dc52a4154f939a1bd25557ea85d1698aeaf62c48a31d88` |
| `alive-staking-list-page-5.json` | 40 | 22628 | `07c2e67be181dd5de45530956c8fe6d8724bf19f7d48e018997723fe8fb6e157` |
| `staking-details.json` | — | 1415 | `cd51bc943352dfdb087f36ae358e853c03fd844a34abefd54e1662ae6958ff43` |
| `config.json` | — | 3847 | `8d67f0cdde114e30fb024adeea030634f9043c6d84e769851031084acea9dc5f` |

The capture check found **240 rows, 240 distinct Validator identifiers, consecutive global ranks 1–240**, and the same declared `totalCount: 240` on every page. Every identifier matched `0x` plus 128 hexadecimal characters. Each response also declared `totalPages: 5`. No response exceeded the existing 64 KiB adapter bound. This validates the complete **declared** ALL cohort for this capture; sequential pages are not an atomic snapshot, and matching totals/identities cannot prove the absence of undetectable concurrent changes. The source defines ALL/candidate filtering and global offset-based ranks in [StakingService][service], with page metadata exposed directly by [RespPage][page].

## Detail values and six-metric mapping

The selected Validator identifier is:

```text
0xc6c2f9185236d29b3deb0a463b10bf65c88fed993128b422b1f5e1c8fcf7f32e8c8d0a896b3969303c85b4815cf42715c06dfcd33c5b5dc3e78b4159d7f771e2
```

Its [captured detail](../../crates/platpulse-server/tests/fixtures/platscan-mainnet/staking-details.json) returns status `2`, `isInit: false`, and these exact values:

| Metric | Recorded JSON evidence | Interpretation |
| --- | --- | --- |
| Cumulative produced blocks | `blockQty: 1016869` | Validator cumulative count, not monitored Block History |
| Cumulative gross rewards | `rewardValue: "7893196.068697377541"` | Preserve exact decimal LAT text; do not subtract delegator rewards |
| Network rank | Page-1 `ranking: 1` for the same identifier | Adopt the ALL-cohort rank, not a Home-filter rank |
| Historical production rate | Same detail `blockQty: 1016869`, `expectBlockQty: 1018630` | Compute actual / scheduled × 100 from this pair |
| PlatScan 24-hour rate | `genBlocksRate: "100.1493%"` | Normalize to `100.1493` percentage points; **do not clamp to 100** |
| Effective delegation reward share | `rewardPer: "90"` | 90%, not 0.90% or 9000% |

`nextRewardPer` is separately `"90"`; it is not the effective-ratio source. `deleAnnualizedRate` is the string `"3.58"` **without a percent suffix** and retains its independent annualized-yield meaning. `totalDeleReward` is separately `"7084602.159771505952"`; it must not be subtracted from the gross metric. The pinned [detail mapping][service] computes non-initial gross rewards from fee + block + staking reward counters and has special handling for initial Validators. The captured response supplies a real example, not an independent audit of that accounting.

The detail has **no explicit metric source-cutoff timestamp**. `joinTime`, `leaveTime`, and `stakingBlockNum` describe lifecycle/history, not a metric cutoff. HTTP `Date` and local receipt time establish fetch metadata only; do not invent an upstream cutoff from either. The [source 24-hour implementation][rate] uses preceding settlement epochs excluding the current epoch and can return `0%` on missing evidence/error. This capture is nonzero and does not by itself exercise those failure cases. The [amount serializer][amount] emits LAT strings truncated downward to 12 decimal places, consistent with this sample; no missing precision should be reconstructed.

## Reconciliation with the investigated official revision

The comparison uses official browser-server revision **`6daa5ad2e878474869407314a73a219ac49c8b51`**, not a claim that the live server runs that revision.

1. **Detail envelope discrepancy:** the [API definition][api] declares `StakingDetailResult.data` as an array. The [Java controller][controller] returns `BaseResp<StakingDetailsResp>`, and the live response contains a single `data` **object**. The adapter and fixtures should follow the observed object response, not the contradictory array schema.
2. **Version property discrepancy:** the API definition documents capitalized `Version` on detail/list items, while the live service uses lowercase `version`, consistent with Java bean serialization. Neither spelling establishes explorer deployment revision.
3. **List envelope:** the live response has `code`, `errMsg`, `totalCount`, `displayTotalCount`, `totalPages`, and `data: [...]` at the top level, consistent with [RespPage][page]. It is not a nested `data.list` envelope. The API definition omits `displayTotalCount` from this result schema; the parser need not reject that extra field.
4. **Field types/semantics:** live `blockQty` and `expectBlockQty` are integer JSON values; monetary values and `rewardPer` are decimal strings; `genBlocksRate` is a percent-suffixed string. The [detail DTO][detail] and [mapping service][service] support these choices. Detail does not supply a `ranking` field; the separately fetched list is authoritative for rank.

## Acceptance use and remaining limitations

- The implementation integration test `platscan_mainnet_capture_reaches_public_api_and_retains_independent_last_good` in [`validator.rs`](../../crates/platpulse-server/src/validator.rs) replays these exact six list/detail bodies through controlled HTTP → refresh/persistence → real temporary SQLite → Public API. It passed both its focused run and the complete Rust workspace test run on 2026-09-17. It asserts the six metric values, exact reward text, 240-entry cohort, independent detail/rank failure retention, Home summary, and absent source cutoff. The fixture replay does not require network access. See the [implementation validation results](validator-metrics.md#2026-09-17-acceptance-follow-up) for the associated Web and contract checks.
- Preserve existing synthetic tests for malformed/missing fields, zero denominators, source-reported zero, failed/partial pagination, independent detail/rank last-good values, and unconfigured Networks. A successful live sample does not replace those cases.
- This evidence establishes response compatibility at the recorded mainnet deployment/time only. It does not establish a deployed source commit, complete historical indexing, atomic multi-page reads, future availability, quotas/SLA, or testnet compatibility. No genesis/RPC verification was performed.
- The configuration response and exact fixtures are public source data, not trusted instructions. Raw Validator names/descriptions are retained for byte fidelity; tests should not fetch the third-party URLs they contain.

[api]: https://github.com/PlatONnetwork/browser-server/blob/6daa5ad2e878474869407314a73a219ac49c8b51/docs/apidef/browser.yml.json
[service]: https://github.com/PlatONnetwork/browser-server/blob/6daa5ad2e878474869407314a73a219ac49c8b51/scan-api/src/main/java/com/platon/browser/service/StakingService.java
[controller]: https://github.com/PlatONnetwork/browser-server/blob/6daa5ad2e878474869407314a73a219ac49c8b51/scan-api/src/main/java/com/platon/browser/controller/StakingController.java
[detail]: https://github.com/PlatONnetwork/browser-server/blob/6daa5ad2e878474869407314a73a219ac49c8b51/scan-api/src/main/java/com/platon/browser/response/staking/StakingDetailsResp.java
[page]: https://github.com/PlatONnetwork/browser-server/blob/6daa5ad2e878474869407314a73a219ac49c8b51/scan-service/src/main/java/com/platon/browser/response/RespPage.java
[rate]: https://github.com/PlatONnetwork/browser-server/blob/6daa5ad2e878474869407314a73a219ac49c8b51/scan-common/src/main/java/com/platon/browser/bean/NodeSettleStatis.java
[amount]: https://github.com/PlatONnetwork/browser-server/blob/6daa5ad2e878474869407314a73a219ac49c8b51/scan-service/src/main/java/com/platon/browser/config/json/CustomLatSerializer.java
