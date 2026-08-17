# Validator Provider contract boundary

PlatPulse treats an Explorer as an optional, Server-side data source. The browser and Agent never contact it, and Explorer-specific JSON is reduced to `ValidatorObservation` before it enters the domain or API projections.

## Verified source constraints

- The PlatON protocol implementation is published by the PlatON Network organization in [`PlatON-Go`](https://github.com/PlatONnetwork/PlatON-Go). This repository is the primary protocol source used to avoid inventing Agent-side validator fields.
- The adapter does **not** claim that PlatON-Go is an Explorer HTTP API contract. The configured Explorer URL and `/api/v1/networks/{network}/validators/{validator}` path are an explicit deployment adapter convention. A deployment must provide a compatible endpoint; an incompatible endpoint is represented as `Unsupported` or `Error`, never as fabricated data.
- PlatON-Go is distributed under the license in its repository [`LICENSE`](https://github.com/PlatONnetwork/PlatON-Go/blob/master/LICENSE). PlatPulse links to the source and does not copy Explorer code or payloads into the repository.

No official public Explorer authentication or rate-limit contract was available to rely on for this adapter. Therefore:

- the adapter supports no implicit credentials and rejects base URLs containing URL credentials, query strings, or fragments;
- deployments requiring authentication must place an authenticated reverse proxy in front of the configured endpoint rather than adding provider secrets to `server.toml`;
- no rate-limit guarantee is assumed. Requests are serialized by the Server refresh cycle, use an explicit timeout, and refresh cadence is operator-configured;
- response bodies are bounded at 64 KiB, JSON is validated at the trust boundary, and diagnostics are redacted and bounded before persistence.

The adapter recognizes HTTP `404` as `NotFound`, `501`/`405` as `Unsupported`, bounded JSON arrays used as authoritative empty results as `AuthoritativeEmpty`, and malformed, oversized, invalid, or other unsuccessful responses as degraded `Error` outcomes. Exact decimal values remain strings; ranks, counts, epochs, and block metrics are bounded non-negative integers. These are PlatPulse safety guarantees, not claims about an upstream Explorer's undocumented schema.
