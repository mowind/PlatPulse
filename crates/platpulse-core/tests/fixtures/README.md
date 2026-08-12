# AgentReport v1 wire fixtures

Frozen canonical/historical JSON fixtures for the `platpulse-core` wire
contract. These files are the drift detector for protocol v1:

- every fixture must deserialize with the current types,
- reserializing the parsed struct must produce the same JSON value,
- `AgentReport::validate()` must accept every report fixture,
- `ReportReceipt::validate()` must accept every receipt fixture
  (retryable flags match the code contract, dispositions are consistent
  with entries, and `received_at` never appears in Agent reports).

Validation also enforces: `attempted_at` is required for
`starting`/`ok`/`error` components and `ok` requires the last-good value;
numeric contract bounds (CPU 0..=100, non-negative finite load, `used <=
total`, `current_block <= highest_block`); receipt arrays are required
(authoritative `[]`, never omitted); tagged-enum payloads reject unknown
fields.

Changing the v1 wire contract (renaming fields/enums, changing units or
optionality, removing required fields) breaks these tests by design; such a
change requires a new protocol major and a new fixture set.

## Files

| Fixture | What it freezes |
| --- | --- |
| `report_v1_canonical.json` | Full canonical report: 2 Nodes across 2 Networks, all 13 components in every state (`ok`/`error`/`disabled`/`unsupported`), last-good retention across a failure, 3 Block Summaries (all `seal_signer_match` states, `verified`/`unknown` proposer, `subscription` + `gap_backfill` sources), 1 History Gap. |
| `report_v1_minimal.json` | Historical minimal report: 1 Node, empty authoritative arrays (`agent_capabilities`, `block_summaries`, `history_gaps`), disabled/unsupported components without `attempted_at`, and a block height beyond 2^53 (exact JSON integer). |
| `report_v1_drained.json` | Historical boot transition: `drained_previous` with `previous_boot_id` after a recovery-drain. |
| `receipt_v1_accepted.json` | Full `accepted` receipt: whole-Inventory `accepted`, per-Node `accepted` with component revisions, every sample `accepted`. |
| `receipt_v1_partially_accepted.json` | `partially_accepted` receipt: one Node `rejected` (terminal), one sample `retryable_rejected` (`gap_not_open`), one sample `terminal_rejected` (`network_identity_mismatch`), plus a rotation hint. |
| `receipt_v1_rejected.json` | Envelope-level `rejected` receipt (`unsupported_protocol_version`), no Inventory disposition, no Node/sample entries. |

## Encoding rules exercised here

- `snake_case` field names, stable lowercase enum strings;
- RFC 3339 UTC timestamps, canonical `YYYY-MM-DDTHH:MM:SSZ`
  (fractional seconds and `+00:00` are accepted and normalized);
- block timestamps are Unix milliseconds (`block_timestamp_ms`);
- durations end in `_ms` (`block_interval_ms`, `uptime_ms`, …);
- counts/heights are exact JSON integers (u64);
- `omitted` (allowed for optional fields) vs `null` (never allowed) vs
  `0` (authoritative zero) vs empty array (authoritative empty) are
  distinct.
