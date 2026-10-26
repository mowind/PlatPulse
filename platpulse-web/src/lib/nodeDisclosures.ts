/**
 * Domain explanations and disclosure copy for Node cards and Node modals.
 *
 * Extracts long domain text out of UI presentation components (`HomeNodeCard.tsx`, etc.)
 * into a typed, semantic module to maintain readability, prevent copy drift,
 * and facilitate reuse across views (issue #277).
 *
 * Terminology note: While 'disclosure' in webui.md and `components/ui/disclosure.tsx`
 * refers to lightweight non-modal collapsible widgets, issue #277 uses 'disclosures'
 * for the domain explanation texts presented in modals. Both *_EXPLANATION and
 * *_DISCLOSURE names are exported for semantic clarity and spec compliance.
 */

export const NODE_ROLE_EXPLANATION =
  'Node role describes the Node’s consensus membership, not its linked Validator’s current staking validity or the freshness of Provider data.'
export const NODE_ROLE_DISCLOSURE = NODE_ROLE_EXPLANATION

export const ACTIVE_NODES_AND_HEALTH_EXPLANATION =
  'Active Nodes are in the latest Agent Inventory, not necessarily online. Healthy reflects successful, fresh RPC, sync and consensus observations. Process errors, a stopped or Unknown process state, or Network Identity Mismatch prevent Healthy; disabled process monitoring does not. Healthy does not mean synchronization is complete; Resyncing is shown independently.'
export const ACTIVE_NODES_AND_HEALTH_DISCLOSURE = ACTIVE_NODES_AND_HEALTH_EXPLANATION

export const HOME_ATTENTION_EXPLANATION =
  "Home Attention counts Active Nodes that are not Healthy, including Unknown. It counts Active Nodes and nothing else: a chain-height difference never enters it, and Observing is not an abnormal state. While nothing is narrowed, these counters and the Peer map read the whole Network selection. Once search, health, or Validator status narrows the list, they read the Active Nodes that matched instead, so a Node's countries count only where that Node is listed — while the two cumulative Validator cells keep the Network selection, and the list still reports how many Nodes matched."
export const HOME_ATTENTION_DISCLOSURE = HOME_ATTENTION_EXPLANATION

export const RESOURCE_METRICS_EXPLANATION =
  "CPU and Memory are the monitored PlatON process's own percentages, not the Host's. Disk is the Node data directory's size as a share of the capacity of the filesystem that holds it, not whole-filesystem fullness. Speed is the Agent Host's own interface throughput, drawn on every Node card that Host reports for, so it is not a rate attributable to this Node alone."
export const RESOURCE_METRICS_DISCLOSURE = RESOURCE_METRICS_EXPLANATION

export const RESYNC_TARGET_EXPLANATION =
  'The last recorded advance toward the Historical High-Water Mark, not the latest report time. The target is the Node’s retained historical height, not the Network Head.'
export const RESYNC_TARGET_DISCLOSURE = RESYNC_TARGET_EXPLANATION

export const RESYNC_PERCENTAGE_UNAVAILABLE_EXPLANATION =
  'No percentage is asserted: a percentage needs a valid current Block Height and a valid, computable Historical High-Water Mark target.'
export const RESYNC_PERCENTAGE_UNAVAILABLE_DISCLOSURE = RESYNC_PERCENTAGE_UNAVAILABLE_EXPLANATION

export const NODE_CARD_EXPLANATIONS = {
  role: NODE_ROLE_EXPLANATION,
  activeAndHealth: ACTIVE_NODES_AND_HEALTH_EXPLANATION,
  attention: HOME_ATTENTION_EXPLANATION,
  resourceMetrics: RESOURCE_METRICS_EXPLANATION,
} as const

export const NODE_CARD_DISCLOSURES = NODE_CARD_EXPLANATIONS
export type NodeCardExplanations = typeof NODE_CARD_EXPLANATIONS
export type NodeCardDisclosures = typeof NODE_CARD_DISCLOSURES

export const RESYNC_EXPLANATIONS = {
  target: RESYNC_TARGET_EXPLANATION,
  percentageUnavailable: RESYNC_PERCENTAGE_UNAVAILABLE_EXPLANATION,
} as const

export const RESYNC_DISCLOSURES = RESYNC_EXPLANATIONS
export type ResyncExplanations = typeof RESYNC_EXPLANATIONS
export type ResyncDisclosures = typeof RESYNC_DISCLOSURES

/** Formats the identity description line with network display name, process uptime, and node role explanation. */
export function formatNodeIdentityDescription(networkDisplayName: string, processUptime: string): string {
  return `Network: ${networkDisplayName} · Uptime ${processUptime}. ${NODE_ROLE_EXPLANATION}`
}

/** Formats the resource metrics explanation paragraph, appending the formatted node data bytes if available. */
export function formatResourceMetricsExplanation(nodeDataBytes?: string): string {
  return nodeDataBytes
    ? `${RESOURCE_METRICS_EXPLANATION} Node data: ${nodeDataBytes}.`
    : RESOURCE_METRICS_EXPLANATION
}
