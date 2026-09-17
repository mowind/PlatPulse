# Validator Provider contract boundary

PlatPulse treats PlatScan as an optional, Server-side Validator data source.
The browser and Agent never contact it, and PlatScan-specific JSON is reduced
to `ValidatorObservation` before it enters the domain or API projections.

## Network coverage

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
  Public projection shows an explicit unconfigured state, never Unknown
  Activity or Observing. Identical Validator identifiers on different Networks
  are routed to their own deployments and never share stored data (#154).
- Each base URL must be an absolute HTTP(S) URL without credentials, query
  strings, fragments, or an invalid/missing host. Deployments requiring
  authentication place an authenticated reverse proxy in front of the
  configured endpoint rather than adding provider secrets to `server.toml`.

## Request contract

- POST `{base_url}/browser-server/staking/stakingDetails` with
  `Content-Type: application/json`.
- The body contains only `{"nodeId": "<validator_node_id>"}`.
- Before any outbound request, the adapter validates the identifier as `0x`
  followed by exactly 128 hexadecimal characters; anything else is
  `Unsupported` and is never sent.
- The adapter never calls or falls back to `aliveStakingList`.

## Response contract

- The success envelope is
  `{ "code": 0, "errMsg": ..., "data": { "nodeId": ..., "status": ... } }`.
- `code` must be the integer `0`; `data` must be an object; a known Validator
  must return the exact requested `nodeId` and an integer `status`.
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
  `ranks`/`ranking`/`rank` → `rank`; `stakingValue`/`totalValue`/`stake` →
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
