# ADR 0004: Owner removal is a permanent admission boundary, not retirement

**Status:** Accepted; implementation pending.

Local Node Inventory must remain Agent-owned, but an Owner must be able to remove an invalid deployment even when it is still declared or its Host will never report again. We choose explicit Agent Removal and permanent Node Purge with retained deletion identities over retirement-only cleanup, recoverable hiding, or an ordinary row delete that the next valid Inventory could undo. The Owner accepts loss of Node monitoring history and must use a new Node ID to monitor the deployment again; Server admission changes do not remotely edit configuration or stop processes.

## Scope and consequences

- Agent Removal revokes all credentials and purges owned Nodes after pending Transfers have been handled. The UI discloses the affected Nodes and irreversible consequences; stable identity must not be silently restored through reporting or credential recovery.
- Node Purge removes Node observations, monitoring history, and Node Validator Links, including when the Node is Active. Minimal deletion identity and necessary Audit remain; continuing Inventory, late writes, restarts, and report retries cannot reconstruct it or prevent other valid Nodes from reporting.
- Retired still means absent from the latest valid local Inventory and keeps history. Purge is not a new liveness value, automatic offline cleanup, a visibility switch, or remote control.
- Independent Validator history and existing Alert Incident evidence are not Node-owned disposable data. Preserve them even when the last linked Node is purged; distinguish removal of a subject from known recovery. The subject leaves current attention/evaluation, outstanding unsent notifications are cancelled, and no synthetic recovery notification is created. Already delivered messages cannot be recalled.
- Ordinary retention protections are unchanged. Explicit Owner Purge is a separate authorization to erase Node monitoring data, not permission to erase shared data, Incident evidence, receipt idempotency, or audit accountability. Acknowledging Attention is also separate: it changes shared presentation of one occurrence, not health, evidence, Incident state, or notification policy.
- The immutable Agent Report and transactional receipt application in [ADR 0001](0001-agent-store-receipt-lifecycle.md) remain intact. Ingestion must distinguish purged-Node admission from invalid whole-Inventory structure and preserve exact existing receipts without replaying removed projections.

This irreversible boundary is deliberate: allowing a deleted identity to reappear makes the Owner's removal meaningless; erasing every reference would destroy shared Validator data and evidence of past security/operational actions. API shapes, physical deletion ordering, and retryable operation mechanics remain implementation work, not claims made by this ADR.

The complete accepted, not-yet-implemented contract and verification scenarios are in [main design §15](../design/platpulse.md#accepted-management-target); interactions are in [WebUI §15](../design/webui.md#accepted-management-ui-target). No runtime or data change is performed by this documentation decision.
