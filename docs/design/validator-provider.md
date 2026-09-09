# Validator Provider contract boundary

PlatPulse treats PlatScan as an optional, Server-side Validator data source.
The browser and Agent never contact it, and PlatScan-specific JSON is reduced
to `ValidatorObservation` before it enters the domain or API projections.

## Network coverage

- When configured, the `[validator_provider]` section of `server.toml` explicitly
  supplies the PlatScan `base_url`, a bounded `networks` allowlist of configured
  Network keys (1..=64), `timeout_seconds`, `refresh_seconds`, and the existing
  analytics `timezone`. Network-list validation checks shape, bounds, and
  uniqueness; the provider configuration also normalizes the base URL and
  validates the IANA timezone. None of these checks query the SQLite Network
  Registry. If the section or its `base_url` is absent, the provider is disabled
  and the network-list validation is skipped.
- During refresh, a registered Network whose key is outside the configured
  allowlist is `Unsupported`: the adapter makes no outbound request and the
  Public projection shows Unknown Activity, never Observing. When a provider
  `base_url` is configured, the allowlist must contain at least one configured
  key; duplicate, blank, control-character, or overlong keys are rejected.
- The base URL must be an absolute HTTP(S) URL without credentials, query
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
  `delegateQty`/`delegatorCount` → `delegator_count`; `epoch` → `epoch`; and
  `blockQty`/`blockCount` → `block_count`. Integer fields accept non-negative
  JSON integers or base-10 integer strings; fractional syntax and malformed
  non-empty values are invalid. Amount/rate fields accept bounded strings or
  JSON numbers and must contain only non-negative decimal syntax. Null or empty
  optional values remain absent; other invalid values degrade the result to
  `Error`.
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
  `timezone = "UTC"`. When a provider `base_url` is configured, refresh is
  clamped to 1–86,400 seconds, timeout to 1–300 seconds, and the Network
  allowlist must be non-empty.
- A successful observation updates the current insight and one row per configured
  Validator + timezone + calendar day/month bucket in the daily/monthly analytics
  tables. If the configured timezone changes, the new timezone forms distinct
  buckets. These Validator analytics buckets are retained as long-term reporting
  state rather than bounded by the provider refresh path. A non-success outcome updates the attempt/outcome
  diagnostic but retains the last-good Activity and optional metrics when they
  exist.
- Public projection semantics are explicit: `empty`/`not_found` becomes
  `observing`/`current`; a successful canonical Activity is `current` or
  `stale` according to Server receipt age; an error with last-good Activity is
  canonical Activity with `stale`; an error with only metric last-good data,
  `unsupported`, or no last-good Activity is `unknown`. Provider state never
  changes Node Health or Server readiness.
