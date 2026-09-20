# Validator Provider contract boundary

PlatPulse treats PlatScan as an optional, Server-side Validator data source.
The browser and Agent never contact it, and PlatScan-specific JSON is reduced
to `ValidatorObservation` before it enters the domain or API projections.

## Status and authority

The transport, normalization, persistence and deployment-capture sections below
record the **implemented baseline**. That baseline uses an Owner-populated
Validator Registry and optional, explicit Node Validator Links; it does not
automatically discover Validators from observed Nodes. Historical API and live
capture observations remain evidence of that baseline, not proof of the new
identity contract.

The following automatic-identity design is **accepted, NOT IMPLEMENTED**. Its
cross-feature authority is [the main design, section 15](platpulse.md#accepted-management-target)
and [ADR 0005: automatic Validator identity](../adr/0005-automatic-validator-identity.md).
This documentation update neither implements nor executes a migration.

## Accepted target: automatic Validator identity (not implemented)

### Discovery and identity correspondence

- A Node does not require manual Validator registration, binding or a
  primary/standby/observer role. The Server automatically identifies a Node
  Validator Link from the Node's observed full P2P public key and validated
  Network. The independent Validator Registry remains a data identity, not a
  user-maintained prerequisite for discovery.
- The lookup identifier is the full 64-byte P2P public key represented as
  0x plus 128 hexadecimal characters. The Server revalidates the observed
  identity, including a full key carried in an observed enode, before using it.
  A PlatPulse Node UUID, shortened key fingerprint, display name, IP address or
  old manual binding is not a substitute. The Agent reports facts; neither the
  Agent nor WebUI queries PlatScan.
- Each lookup uses that Node's Network-specific deployment binding. Missing or
  mismatched Network evidence, a missing/invalid full key, or an unconfigured
  provider cannot establish a negative Validator identity. Expose the relevant
  unavailable/unconfigured reason; never search another Network or infer a
  match from the Node name. Provider availability and Current Validator Status
  are separate dimensions.
- Automatic correspondence does not prove ownership, current consensus
  membership or historical block production. A key change ends the old
  association interval and requires identification of the new Validator;
  preserve the Node's monitoring history without joining the old and new
  Validators' cumulative counters, rewards or time series.
- No old manual link is a fallback while automatic identification is pending or
  failed. Metrics with no established association remain unavailable, not zero.
  A failed refresh for an already established identity retains its last-good
  values with explicit staleness rather than claiming fresh identity evidence.

### Current Validator Status versus consensus eligibility

The target asks whether the Node's chain identity has **currently valid staking
identity**, not whether it is selected for the current consensus round or has
any historical PlatScan entry:

| Evidence | Target Current Validator Status |
| --- | --- |
| Confirmed candidate, active or producing identity with valid stake | Validator |
| Locked or exiting identity whose staking validity is independently confirmed to remain effective | Validator, with locked/exiting state shown explicitly; not a claim of normal participation |
| Confirmed completed exit or authoritative absence of current staking identity | Not Validator |
| Verifying, insufficient or conflicting evidence that cannot establish validity | Unknown |
| Lookup failure or incomplete evidence | Last-good status marked stale when available; otherwise Unknown, never a default negative |

**Primary-source verification recorded ([#168][status-evidence]):** the baseline
integer-to-Activity mapping below is not by itself a verified mapping to current
staking validity. The required validity predicates — locked/exiting validity,
completed exit, authoritative absence, and the exclusions for HTTP 404,
transport failure and ranking absence — are now established from browser-server
and PlatON-Go source evidence plus a captured mainnet deployment. The source
shows the `verifying` code is a candidate in a consensus round; because
[CONTEXT.md](../../CONTEXT.md), the table above and ADR 0005 define verification
in progress as Unknown, that code stays **Unknown** here, and the
source-vs-target disagreement is flagged in the evidence note rather than
silently reclassified. A bare transport failure, ranking absence or the legacy
HTTP 404 normalization must not silently become new proof of staking absence.
Preserve the independently evidenced ranking contract; unranked is not
synonymous with Not Validator. This documentation update did not run a live
capture beyond the recorded fixtures.

[status-evidence]: ../research/platscan-current-validator-status-evidence.md

### One-time migration and ordinary Node Purge

The accepted migration removes old manual links and old Validator snapshots,
ranking/counter history, and daily/monthly aggregates. It **does not remove Node
monitoring history** (including Block Summaries, Peer history and Node counters)
or existing Alert Incident/Audit evidence.

- Reset incompatible cached baselines and change/reset classifications together
  with the deleted Validator history. Old classification inputs, pending refresh
  results or analytics rebuilds must not recreate pre-migration rows or make an
  empty history look like a new counter reset, ranking change or recovered
  Incident. New observations establish new baselines; missing data is not zero.
- Accumulate local Validator history again after cutover, without reconstructing
  removed periods or using old manual links during the transition. Fresh
  PlatScan lifetime totals can still be nonzero: clearing local time series does
  not reset chain-history rewards or blocks. Existing Incident and Audit
  evidence remains retained, even when its old source history is removed.
- Daily/monthly analytics must restart from eligible post-cutover observations,
  not stale cached pre-cutover inputs. A partial new reporting period is not
  fabricated complete historical coverage. Runtime writers and historical
  readers must agree on the cutover before a future migration can be executed.
- Ordinary Node Purge is deliberately narrower: it deletes that Node's links
  and monitoring data, **not independent Validator history**, even when the
  purged Node was the final associated Node. Other Nodes using the same
  Validator keep their data; a selection summary simply excludes the purged
  Node and continues to deduplicate remaining associations.

The old Registry/explicit-Link refresh and existing migration descriptions
below remain baseline facts until this target is implemented. They do not
claim that automatic discovery, historical cleanup or new alert-suppression
behavior already exists.

## Network coverage (implemented baseline)

- When configured, the `[validator_provider]` section of `server.toml` binds
  each registered Network key to its own PlatScan deployment base URL in a
  bounded `networks` table (1..=64 entries), alongside `timeout_seconds`,
  `refresh_seconds`, and the existing analytics `timezone`. Binding validation
  checks key shape, bounds, and non-empty URLs; the provider configuration also
  normalizes each base URL and validates the IANA timezone. None of these checks
  query the SQLite Network Registry. If the section is absent, the provider is
  disabled; if it is present without any Network binding, configuration
  resolution fails.
- The detail request carries no Network selector, so an allowlist shared by
  several Networks cannot prove which chain one deployment serves. Each Network
  is therefore bound to a distinct deployment; a registered Network without a
  binding is `not_configured`: the adapter makes no outbound request and the
  Public projection's top-level `state` explicitly remains `not_configured`,
  rather than collapsing the reason into an ordinary unknown/observing result.
  Its separate legacy `activity` and `activity_state` fields are `unknown`; this
  distinction is not a current-staking-validity classification. Identical
  Validator identifiers on different Networks are routed to their own
  deployments and never share stored data (#154).
- Each base URL must be an absolute HTTP(S) URL without credentials, query
  strings, fragments, or an invalid/missing host. Deployments requiring
  authentication place an authenticated reverse proxy in front of the
  configured endpoint rather than adding provider secrets to `server.toml`.

## Verified deployment response contract

The mainnet deployment at `https://scan.platon.network/` was checked on
2026-09-17: its configuration self-identifies as PlatON Mainnet (chain ID
210425), and the captured detail and five-page ALL ranking responses pass
HTTP → refresh → SQLite → Public API regression coverage. See
[mainnet validation](platscan-mainnet-validation.md) and the raw fixtures in
`crates/platpulse-server/tests/fixtures/platscan-mainnet/` for requests, hashes,
source-revision reconciliation and limitations. The deployed browser-server
revision and completeness of its historical indexing are not known. Other
Networks still require their own explicit compatible deployment binding.

The detail capture reports `genBlocksRate: "100.1493%"`; this source-defined
percentage is retained even above 100, unlike `rewardPer`, which is bounded
to 0..=100. Its reward and annualized-yield values are decimal strings. It
contains no source cutoff: HTTP Date and Validator join/leave times must not
be substituted for one. The live detail `data` is an object (matching the
Java controller), despite the investigated API definition describing an
array. Tests replay the raw responses offline; CI does not contact PlatScan.

## Request contract

- POST `{base_url}/browser-server/staking/stakingDetails` with
  `Content-Type: application/json`.
- The body contains only `{"nodeId": "<validator_node_id>"}`.
- Before any outbound request, the adapter validates the identifier as `0x`
  followed by exactly 128 hexadecimal characters; anything else is
  `Unsupported` and is never sent.
- Detail and ranking are independent collections. The adapter never treats
  the ranking list as a fallback for a failed detail request, and the detail
  request never carries a rank: the investigated detail response has no
  ranking alias, so ranking is acquired only from the dedicated
  `aliveStakingList` endpoint below (#158).

## Ranking request contract

- POST `{base_url}/browser-server/staking/aliveStakingList` with
  `Content-Type: application/json` and the body
  `{"pageNo": <n>, "pageSize": 50, "queryStatus": "all"}`.
- The request never sets the upstream `key` name filter, so the cohort is the
  Network's complete live-staking ALL set including candidates. The adapter
  adopts the upstream 1-based `ranking` verbatim and never recomputes a rank
  over the monitored Node set, a Home filter, or another Network.
- Paging is bounded (50 rows per page, at most 50 pages, at most 2500 cohort
  entries). Every page's `totalCount` must match the first page; the global
  position is recomputed from the requested page and row order; a duplicate
  Validator, a page-local rank, a short page, or a declared total that is
  never reached is a detectable inconsistency and degrades the whole fetch to
  `Error`. A complete, internally consistent list is the only outcome that
  can establish an unranked Validator; PlatPulse does not claim that upstream
  paging is an atomic snapshot.
- `404`/`405`/`501` are `Unsupported`; other 4xx/5xx responses, transport
  failures, malformed envelopes, and bodies over 64 KiB are `Error`.

## Response contract

- The success envelope is
  `{ "code": 0, "errMsg": ..., "data": { "nodeId": ..., "status": ... } }`.
- `code` must be the integer `0`; `data` must be an object; a known Validator
  must return the exact requested `nodeId` and an integer `status`.
- The ranking response is the upstream paginated page returned directly:
  `{ "code": 0, "errMsg": ..., "totalCount": ..., "data": [ { "nodeId": ...,
  "ranking": ... } ] }`. `code` must be integer `0`, `totalCount` must be a
  non-negative integer, and every row must carry a valid 130-character
  `0x`-hex `nodeId` and the next in-sequence global rank (#158).
- Status mapping: 1 Candidate and 2 Active map to `active`, 3 Producing maps
  to `producing`, 4 Exiting maps to `exiting`, 5 Exited maps to `exited`,
  6 Verifying maps to `verifying`, and 7 Locked maps to `locked`.
- The strictly validated empty form (empty `data.nodeId` and status `0`) is
  `AuthoritativeEmpty`; HTTP `404` is `NotFound`. Both are authoritative
  no-live-Validator outcomes for an effective Node Validator Link.
- `405`/`501` are `Unsupported`; all other 4xx/5xx responses are classified
  as `NotFound` or degraded `Error` before the response body is buffered. For an
  accepted response, malformed envelopes, mismatched identifiers, invalid types,
  unrecognized statuses, or a body over 64 KiB are degraded `Error` outcomes.
- Optional metrics are normalized into the Server-owned observation when present:
  `stakingValue`/`totalValue`/`stake` →
  `stake_amount`; `rewardValue`/`reward` → `reward_amount`;
  `deleAnnualizedRate`/`rewardRate` → `reward_rate`;
  `delegateQty`/`delegatorCount` → `delegator_count`; `epoch` → `epoch`;
  `blockQty`/`blockCount` → `block_count`; `expectBlockQty`/`expectedBlockQty`
  → `expected_block_count`; `genBlocksRate`/`generatedBlocksRate` →
  `gen_blocks_rate` (#156); and `rewardPer` → `delegation_reward_percentage`
  (#157). Integer fields accept non-negative
  JSON integers or base-10 integer strings; fractional syntax and malformed
  non-empty values are invalid. A `genBlocksRate` value may be a bounded
  decimal string with an optional trailing `%` or a non-negative JSON number;
  it is normalized to percentage points without the sign, and a stray `%`,
  malformed text, or exponent notation is invalid rather than a fabricated `0`.
  Amount/rate fields accept bounded strings or
  integral JSON numbers and must contain only non-negative decimal syntax. A
  fractional or exponent JSON number is rejected because serde_json has already
  converted it to binary floating point, so its exact source digits are
  unrecoverable; the demonstrated source emits decimal strings. Null or empty
  optional values remain absent; other invalid values degrade the result to
  `Error`.
- `reward_amount` is the gross cumulative Validator reward over chain history,
  including the operator and delegator allocations. The adapter never subtracts
  `totalDeleReward` and never derives a historical net reward from the current
  `rewardPer`/`nextRewardPer`; `rewardPer` is read only as the delegation
  distribution percentage and `nextRewardPer` is never read. The investigated
  source serializes LAT values truncated downward to at most 12 decimal places,
  and those exact digits cross the trust boundary as a bounded decimal string
  that is never parsed into binary floating point (#155).
- `expected_block_count` is the same observation's cumulative scheduled-block
  denominator for the Server-computed completion rate (#156). Both inputs come
  from one successful observation, so a fresh numerator is never divided by an
  older denominator; a known zero denominator is `not_applicable` and an
  incomplete pair is `unknown`, never a synthesized `0%`. The rate is not an
  exact missed-block rate because whole-round duties are counted before they
  elapse.
- `gen_blocks_rate` is projected directly as PlatScan's own 24-hour rate. The
  investigated implementation sums the preceding seven settlement periods
  excluding the current one and can return `0%` for absent evidence or an
  upstream error; the Public contract therefore labels it PlatScan口径 rather
  than a strict rolling 86,400-second window, and retains a source `0`
  independently of the locally computed completion rate.
- `delegation_reward_percentage` is the currently effective delegation reward
  distribution percentage from the detail `rewardPer` (#157). The upstream
  detail response has already scaled its internal basis points, so a source
  `20` is stored and projected as 20 percentage points, never 0.20% or 2000%.
  The adapter bounds it to `0..=100`: an out-of-range, negative, or malformed
  value degrades the observation to `Error` rather than being clamped or
  reinterpreted. It is distinct from the annualized `reward_rate`
  (`deleAnnualizedRate`) and from the pending `nextRewardPer`, which is never
  read. A local failure retains the last-good percentage, and a later success
  that omits the field is Unknown (#157).
- The 64 KiB response limit is checked after `Response.bytes()` has read the
  response. It is a post-buffer validation bound, not a streaming memory cap.
  JSON is validated at the trust boundary, and diagnostics are redacted and
  bounded before persistence.

The upstream PlatScan envelope is undocumented; these are PlatPulse safety
guarantees, not claims about an upstream schema.

## Refresh and persistence

- The refresh worker iterates the persisted Server Validator Registry, not only
  currently effective Node Validator Links. A Validator can therefore be
  refreshed even when it temporarily has no active link.
- Configuration defaults are `refresh_seconds = 60`, `timeout_seconds = 10`, and
  `timezone = "UTC"`. When provider Network bindings are configured, refresh is
  clamped to 1–86,400 seconds, timeout to 1–300 seconds, and at least one
  Network binding is required.
- A last-good observation is fresh while it is no older than two configured
  refresh intervals (120s at the default 60s refresh). Freshness therefore
  follows a valid slower refresh setting instead of a fixed constant, and the
  Public projection exposes the last successful fetch time plus the upstream
  cutoff when the source provides one — never a fabricated timestamp.
- A successful observation updates the current insight and one row per configured
  Validator + timezone + calendar day/month bucket in the daily/monthly analytics
  tables. If the configured timezone changes, the new timezone forms distinct
  buckets. These Validator analytics buckets are retained as long-term reporting
  state rather than bounded by the provider refresh path. A non-success outcome updates the attempt/outcome
  diagnostic but retains the last-good Activity and optional metrics when they
  exist.
- Non-success outcomes retain every last-good metric: a `not_configured`,
  `unsupported`, `error`, `empty`, or `not_found` outcome updates the attempt
  outcome but never clears a retained cumulative block count, scheduled-block
  denominator, gross cumulative reward, PlatScan 24-hour rate, or other
  last-good values, and never fabricates a zero. A later successful observation
  that omits a field replaces that field with Unknown rather than retaining a
  value from a different observation. The cumulative
  `block_count` is the Validator's chain-history counter, not blocks observed by
  the monitored Node; `reward_amount` is the gross lifetime reward, not the
  operator's net earnings.
- Public projection semantics are explicit: `empty`/`not_found` becomes
  `observing`/`current`; a successful canonical Activity is `current` or
  `stale` according to Server receipt age; an error with last-good Activity is
  canonical Activity with `stale`; an error with only metric last-good data,
  `unsupported`, `not_configured`, or no last-good Activity is `unknown`.
  Provider state never changes Node Health or Server readiness.
- Migration 0044 widens the stored outcome CHECK to accept `not_configured`
  and copies every existing row verbatim, so pre-existing last-good values
  survive without fabrication.
- Migration 0045 adds the nullable `expected_block_count` and
  `gen_blocks_rate` columns. Historical rows keep NULL, which the Public
  projection reports as `unknown`; no value is backfilled with a fabricated
  rate.
- Migration 0046 adds the nullable `delegation_reward_percentage` column.
  Historical rows keep NULL and the Public projection reports `unknown`; no
  ratio is backfilled.
- Migration 0047 adds the nullable ranking outcome, diagnostic, attempt time,
  last-success time, and cohort-size columns and clears the pre-existing
  `rank` value: the investigated detail response has no authoritative ranking
  alias, so an upgraded row stays Unknown instead of exposing an untrusted
  position. No rank is backfilled. The `rank` column then holds the last-good
  ranking value: a complete successful list may clear it (authoritative
  unranked), while any failure or incomplete fetch retains it. Ranking has its
  own last-success time, so detail and ranking freshness are independent and
  neither failure erases the other's successful values (#158).
- The detail-derived daily/monthly Validator analytics `rank` is not a ranking
  source and is no longer populated: it was never an authoritative detail
  field, and reconstructing historical analytics rank from the dedicated list
  is intentionally out of scope for this slice. The current insight's dedicated
  ranking state is the only authoritative rank.
