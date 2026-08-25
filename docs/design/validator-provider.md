# Validator Provider contract boundary

PlatPulse treats PlatScan as an optional, Server-side Validator data source.
The browser and Agent never contact it, and PlatScan-specific JSON is reduced
to `ValidatorObservation` before it enters the domain or API projections.

## Network coverage

- The `[validator_provider]` section of `server.toml` explicitly supplies the
  PlatScan `base_url`, a bounded `networks` allowlist of registered Network
  keys (1..=64), `timeout_seconds`, `refresh_seconds`, and the existing
  analytics `timezone`.
- A registered Network outside the allowlist is `Unsupported`: the adapter
  makes no outbound request and the Public projection shows Unknown Activity,
  never Observing.
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
- `405`/`501` are `Unsupported`; malformed envelopes, mismatched identifiers,
  invalid types, unrecognized statuses, oversized bodies, and other
  unsuccessful non-authoritative responses are degraded `Error` outcomes.
- Response bodies are bounded at 64 KiB, JSON is validated at the trust
  boundary, and diagnostics are redacted and bounded before persistence.

The upstream PlatScan envelope is undocumented; these are PlatPulse safety
guarantees, not claims about an upstream schema.
