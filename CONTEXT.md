# PlatPulse

PlatPulse monitors PlatON node deployments through distributed Agents and presents their operational state through a central Server and WebUI.

## Language

**Host**:
The machine environment on which one Agent and its monitored PlatON Nodes run. Nodes on another Host belong to another Agent.
_Avoid_: Agent server, Node agent

**Agent**:
A collector instance bound to one Host that connects to one or more PlatON Nodes on that Host and reports each Node's observations independently. One Agent may monitor Nodes from different Networks.
_Avoid_: Node agent, Chain agent

**PlatON Node**:
A monitored logical PlatON node instance with a stable Node ID, one RPC Endpoint at a time, and one Network. Block, transaction, consensus, and other observations belong to this Node rather than to its Agent as a whole.
_Avoid_: RPC target, Endpoint

**Node ID**:
The globally unique internal UUID of a PlatON Node. It is retained when the Node's display name, RPC Endpoint, or owning Agent changes.
_Avoid_: Endpoint URL, Node name, Validator ID

**Node Transfer**:
The explicit reassignment of a PlatON Node from one Agent to another while preserving the Node's identity and history.
_Avoid_: Automatic takeover, New node

**Pending Transfer**:
A temporary Owner-authorized state in which the source Agent remains authoritative until the designated target Agent validly declares the Node.
_Avoid_: Shared ownership, Automatic takeover

**RPC Endpoint**:
The single PubSub-capable IPC, WebSocket, or secure WebSocket address through which an Agent accesses a particular PlatON Node. It is a mutable Node attribute, not a failover member or the Node's identity.
_Avoid_: HTTP polling endpoint, Chain source, Node ID

**RPC Capability**:
A method or transport actually supported by a Node's running build and configuration. Capability is established by probing the Node rather than inferred solely from its version.
_Avoid_: Documented feature, Assumed namespace

**Network**:
The PlatON chain environment to which a Node belongs, such as mainnet or testnet. Network membership belongs to each Node rather than to the Agent.
_Avoid_: Agent network

**Network Identity**:
The observed genesis hash, chain ID, and P2P network ID that distinguish a Network. A configured display name alone is not network identity.
_Avoid_: Network label, Chain ID alone

**Network Registry**:
The Server-managed set of expected Network identities. An Agent declares a Node's configured Network key and observed identity, while the Server validates them without automatically rewriting the registry.
_Avoid_: Free-text network name, Agent-owned network identity

**Network Identity Mismatch**:
A state in which a Node's observed Network Identity differs from its registered Network. Current diagnostic observations may continue, but block history must not merge into the registered Network's history.
_Avoid_: Automatic network migration, RPC error

**Observed Network Head**:
The highest fresh current head observed by eligible Nodes in one registered Network, together with its contributing sources and confidence. It is a deployment-local reference, not a claim about the absolute public network head.
_Avoid_: Canonical network head, Explorer head

**Validator**:
A Network-scoped PlatON validator identity keyed by the registered Network and its validator node identifier. It is independent of PlatPulse Node identity and may exist without a monitored Node.
_Avoid_: PlatPulse Node, Agent validator

**Node Validator Link**:
An explicit time-bounded relationship between a monitored Node and a Validator, including whether the Node is primary, standby, or observer. Consensus membership alone does not create this link.
_Avoid_: Positional mapping, Inferred validator ownership

**Validator Provider**:
A Server-side Network adapter that supplies Validator current data and snapshots, such as the PlatScan browser-server adapter. Provider failure does not change Node health or erase the last successful Validator value.
_Avoid_: Agent collector, Validator authority

**Agent Enrollment**:
The one-time process through which a new Agent establishes its identity with the Server and receives its own revocable credential.
_Avoid_: Agent login, Permanent install token

**Enrollment Token**:
A short-lived, single-use secret that authorizes one Agent Enrollment but cannot submit observations or access human-facing APIs.
_Avoid_: Agent credential, Install password

**Agent Credential**:
A revocable secret bound to one Agent and limited to that Agent's reporting authority.
_Avoid_: Admin token, Enrollment token

**Agent Epoch**:
The Server-controlled generation of an Agent identity. Enrollment, Recovery, or an explicit reset advances the epoch so older state cannot replace newer accepted reports.
_Avoid_: Boot count, Report sequence

**Agent Recovery**:
The Owner-authorized process that restores an existing Agent identity after credential or state loss without creating a duplicate Agent or silently replacing accepted state.
_Avoid_: New enrollment, Automatic reset

**Node Inventory**:
The complete set of PlatON Nodes that an Agent declares from its local configuration. It identifies which Nodes currently belong to that Agent without transferring connection configuration ownership to the Server.
_Avoid_: Server node config, Agent chain sources

**Active Node**:
A PlatON Node present in its Agent's latest valid Node Inventory and eligible for current observation and alert evaluation.
_Avoid_: Online node

**Retired Node**:
A previously Active Node absent from its Agent's latest valid Node Inventory. Its identity and history remain, but live observation alerts no longer apply.
_Avoid_: Deleted node, Offline node

**Node Purge**:
An explicit Owner action that permanently removes retained Node data. It is distinct from retiring a Node through local configuration removal.
_Avoid_: Config removal, Automatic cleanup

**Component Observation**:
The status and latest successful value for one independently collected component. A current collection error does not erase the component's last successful value.
_Avoid_: Latest attempt result, Nullable metric

**Agent Report**:
An immutable report containing an Agent's complete current observation view. Retries preserve the same report identity and content; the Server derives its current projection and recent block history from accepted reports.
_Avoid_: Mutable heartbeat, Partial state patch

**Report Receipt**:
The Server's durable idempotency record for one Agent Report, including its content hash and exact acceptance result. The result is report-level — accepted or rejected with a stable rejection code — with no per-Node partial result matrix. It does not retain the complete report body indefinitely.
_Avoid_: Raw report archive, Access log

**Applied Receipt Record**:
A bounded Agent-local terminal marker showing that one Report Receipt was transactionally applied before its Agent Report left the Durable Spool. It retains only the identity and outcome needed to detect a recent duplicate or conflict; it is not Report History, a Receipt archive, or a user-facing audit record.
_Avoid_: Report Receipt, Receipt archive, Agent audit event

**Report Ingestion**:
The Server's atomic acceptance of one authenticated Agent Report, including idempotency, invariants, projection updates, recent block history, invalidation, and its exact Report Receipt.
_Avoid_: HTTP handler update, Partial projection write

**Current Projection**:
A typed Server-side representation of the latest accepted state and each component's last successful value. It is rebuilt only through validated Agent Reports and is distinct from immutable history.
_Avoid_: Agent report cache, Generic JSON metric

**Durable Spool**:
The Agent's bounded persistent queue of immutable reports awaiting Server acknowledgement. On overflow the Agent drops the oldest unconfirmed historical reports, logs a diagnostic, and preserves current-state collection. The Spool is a delivery mechanism, not user-visible history.
_Avoid_: In-memory retry queue, History database

**Agent Store**:
The Agent's local durable module that owns identity sequence, immutable report planning, acknowledgement, and spool limits behind one transactional interface.
_Avoid_: SQLite helper, Collector database

**Head Subscription**:
A per-Node PubSub stream of new block headers used as the trigger for normal block collection. It is not shared across Nodes and is not silently replaced by continuous polling.
_Avoid_: Agent chain subscription, Polling loop

**Block Resolution**:
The retrieval and verification of a subscribed block by its header hash before producing a Block Summary. Transaction hashes may be counted, but complete transaction bodies are not required.
_Avoid_: Fetch by height without hash verification, Full transaction ingestion

**Gap Backfill**:
A bounded point-query recovery of blocks missed across startup, reconnection, or a detected subscription jump. It is distinct from normal subscription ingestion and records a History Gap when the recoverable range is exceeded.
_Avoid_: Continuous polling, Full-chain scan

**Block Summary**:
A per-Node observation of one block containing operational metadata and Block Production Attribution without complete transaction contents.
_Avoid_: Archived block, Transaction record

**History Gap**:
An explicit interval for which time-series samples were lost or intentionally dropped while a later current-state report may still remain authoritative.
_Avoid_: Zero activity, Unknown outage

**Historical High-Water Mark**:
The greatest block height already accepted into a Node's append-only operational history. It does not replace or constrain the Node's current head during resynchronization.
_Avoid_: Current head, Network head

**Resync Episode**:
A period in which a Node rebuilds local chain data and its current head advances below or back toward its Historical High-Water Mark. Existing block and transaction history is retained and replayed heights are not counted again.
_Avoid_: History rollback, Node replacement

**Chain Divergence Observation**:
Evidence that a Node reported a different block hash at a height already recorded. It is retained as an observation rather than overwriting the existing Block Summary.
_Avoid_: Silent overwrite, Duplicate block

**Counter Reset or Correction**:
A state in which a supposedly cumulative value decreases because the upstream source reset or corrected it. The decrease remains explicit and is not clamped to zero or presented as normal growth.
_Avoid_: Zero delta, Negative reward

**Host Observation**:
The host-level operational state collected once by an Agent, including shared CPU, memory, disk, load, and network measurements.
_Avoid_: Node system metrics

**Node Process Observation**:
The process-level operational state of one PlatON Node, such as process availability, resource use, and uptime.
_Avoid_: Host observation

**Node Chain Observation**:
The chain-facing operational state observed from one PlatON Node, including block, transaction, synchronization, consensus, and peer information.
_Avoid_: Agent chain snapshot, Network observation

**Peer Count Observation**:
The current number of connected Peers observed from one PlatON Node, collected as part of its chain observation. A successful collection may be authoritatively zero, while a collection failure preserves the last successful count and its age.
_Avoid_: Peer Snapshot, Peer history, IP-deduplicated count

**Peer Snapshot**:
A complete successful per-Node view of currently connected Peers, keyed by Peer ID. A successful empty snapshot clears the prior set, while collection failure preserves the last successful set and its age.
_Avoid_: Agent-wide peer set, IP-deduplicated peer count

**Peer Presence Interval**:
A derived connected interval opened or closed only by differences between consecutive successful Peer Snapshots. A Collector error never implies that all Peers disconnected.
_Avoid_: Snapshot history, Disconnect on missing report

**Geo Location Cache**:
The Server's country-only lookup cache keyed by canonical Peer IP. Raw addresses remain sensitive current data, are never part of Public Projection, and are not copied into long-term aggregates.
_Avoid_: Peer history, Browser geolocation

**Geo Database**:
An operator-provided local GeoLite2 Country MMDB read by the Server. PlatPulse does not bundle the database, download it, or hold MaxMind credentials; enabling it also requires MaxMind attribution.
_Avoid_: Embedded asset, PlatPulse-managed download

**Block Production Attribution**:
The evidence describing how an observed block relates to a monitored Node. It keeps Coinbase, Seal Signer Match, and Protocol Proposer distinct rather than collapsing them into one inferred producer flag.
_Avoid_: Miner flag, Validator guess, Default false

**Seal Signer Match**:
A tri-state comparison between a block header's recovered seal-signing key and the monitored Node's recorded P2P key. A match is cryptographic key evidence but does not by itself prove the historical Protocol Proposer.
_Avoid_: Producer proof, Coinbase match

**Protocol Proposer**:
The consensus participant selected to propose a block. It remains unknown unless authoritative consensus evidence identifies it; Coinbase, validator membership, and QC membership are insufficient.
_Avoid_: Miner, Seal signer, Current validator

**Node Observation**:
The combined process and chain observations belonging to one PlatON Node. Observations from different Nodes remain separate and are never merged into one Agent-level chain observation.
_Avoid_: Agent observation

**Node Health Summary**:
A display-oriented severity derived from independent liveness, lifecycle, process, reachability, freshness, synchronization, consensus, and Host pressure dimensions. It never replaces those dimensions, and unknown data is not healthy data.
_Avoid_: Online flag, Authoritative health state

**Clock Unreliable**:
A diagnostic state indicating that an Agent's wall clock differs enough from Server time that cross-host chronology or age calculations cannot be trusted. It does not invalidate unrelated observations or Agent liveness based on Server receipt time.
_Avoid_: Agent offline, Invalid report

**Site Access Mode**:
The Server-wide Home access policy with two states: Public (unauthenticated visitors may read Home projections) or Private (login is required). It applies to the whole site rather than to individual Nodes; every Active Node appears on Home under both states. Changing the mode is an Owner action recorded in Audit and starts a new access generation.
_Avoid_: Per-Node visibility, Guest switch

**Public Projection**:
The fixed, sanitized representation consumed by the Home Dashboard. It excludes connection details, credentials, raw addresses, internal errors, and other administrative data regardless of who views it.
_Avoid_: Filtered admin response, Browser redaction

**Invalidation Event**:
A small versioned SSE notification indicating that a REST resource should be fetched again. It carries no full resource representation and respects the Public or Admin API boundary that produced it.
_Avoid_: State stream, WebSocket payload

**Home Dashboard**:
The read-only WebUI surface that presents Public Projections without exposing administrative controls.
_Avoid_: Public admin, Overview admin

**Admin Dashboard**:
The authenticated WebUI surface for managing PlatPulse and viewing operational details that are not part of the Home Dashboard.
_Avoid_: Home settings, Control terminal

**Attention Item**:
A current Server-derived prompt shown in the Admin Dashboard when an Agent, PlatON Node, Network, or setting needs Owner review. It is reconstructed from authoritative current state, is not a durable Alert Incident, and never replaces the underlying diagnostic dimensions.
_Avoid_: Alert Incident, Notification, Browser-computed warning

**Audit Event**:
An immutable record of an administrative mutation or security-sensitive identity action, including who acted, what changed, and when.
_Avoid_: Debug log, Access log

**Alert Rule**:
A Server-owned typed condition with explicit subject scope, thresholds, duration, recovery, and severity. Agents report facts and never create business Alerts themselves.
_Avoid_: User script, Agent alert

**Alert Incident**:
A durable occurrence opened after an Alert Rule remains firing and resolved only after known recovery conditions hold. Unknown input cannot silently resolve it.
_Avoid_: Notification message, Current health color

**Notification Event**:
A durable consequence of an Incident or scheduled summary transition that creates one or more idempotent delivery rows in the same Server transaction.
_Avoid_: Direct provider call, Alert incident

**Notification Delivery**:
A per-channel, per-destination outbox item delivered with at-least-once semantics, retry state, and dead-letter handling.
_Avoid_: Exactly-once message, In-memory job

**Silence**:
A time-bounded delivery policy that suppresses matching notifications without stopping Alert evaluation or deleting Incidents.
_Avoid_: Alert disable, Incident deletion

**Maintenance Window**:
A time-bounded operational context for an Agent, Node, or Network that suppresses expected delivery while preserving Alert facts and auditability.
_Avoid_: Silence without scope, Health override

**Owner**:
A human principal allowed to access the Admin Dashboard and manage PlatPulse.
_Avoid_: Super viewer, Shared admin

