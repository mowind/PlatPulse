import { describe, expect, it } from 'vitest'
import {
  ACTIVE_NODES_AND_HEALTH_DISCLOSURE,
  ACTIVE_NODES_AND_HEALTH_EXPLANATION,
  HOME_ATTENTION_DISCLOSURE,
  HOME_ATTENTION_EXPLANATION,
  NODE_CARD_DISCLOSURES,
  NODE_CARD_EXPLANATIONS,
  NODE_ROLE_DISCLOSURE,
  NODE_ROLE_EXPLANATION,
  RESYNC_DISCLOSURES,
  RESYNC_EXPLANATIONS,
  RESYNC_PERCENTAGE_UNAVAILABLE_DISCLOSURE,
  RESYNC_PERCENTAGE_UNAVAILABLE_EXPLANATION,
  RESYNC_TARGET_DISCLOSURE,
  RESYNC_TARGET_EXPLANATION,
  RESOURCE_METRICS_DISCLOSURE,
  RESOURCE_METRICS_EXPLANATION,
  formatNodeIdentityDescription,
  formatResourceMetricsExplanation,
} from './nodeDisclosures'

describe('nodeDisclosures', () => {
  it('exports verbatim domain explanation constants', () => {
    expect(NODE_ROLE_EXPLANATION).toBe(
      'Node role describes the Node’s consensus membership, not its linked Validator’s current staking validity or the freshness of Provider data.',
    )
    expect(NODE_ROLE_DISCLOSURE).toBe(NODE_ROLE_EXPLANATION)

    expect(ACTIVE_NODES_AND_HEALTH_EXPLANATION).toBe(
      'Active Nodes are in the latest Agent Inventory, not necessarily online. Healthy reflects successful, fresh RPC, sync and consensus observations. Process errors, a stopped or Unknown process state, or Network Identity Mismatch prevent Healthy; disabled process monitoring does not. Healthy does not mean synchronization is complete; Resyncing is shown independently.',
    )
    expect(ACTIVE_NODES_AND_HEALTH_DISCLOSURE).toBe(ACTIVE_NODES_AND_HEALTH_EXPLANATION)

    expect(HOME_ATTENTION_EXPLANATION).toBe(
      "Home Attention counts Active Nodes that are not Healthy, including Unknown. It counts Active Nodes and nothing else: a chain-height difference never enters it, and Observing is not an abnormal state. While nothing is narrowed, these counters and the Peer map read the whole Network selection. Once search, health, or Validator status narrows the list, they read the Active Nodes that matched instead, so a Node's countries count only where that Node is listed — while the two cumulative Validator cells keep the Network selection, and the list still reports how many Nodes matched.",
    )
    expect(HOME_ATTENTION_DISCLOSURE).toBe(HOME_ATTENTION_EXPLANATION)

    expect(RESOURCE_METRICS_EXPLANATION).toBe(
      "CPU and Memory are the monitored PlatON process's own percentages, not the Host's. Disk is the Node data directory's size as a share of the capacity of the filesystem that holds it, not whole-filesystem fullness. Speed is the Agent Host's own interface throughput, drawn on every Node card that Host reports for, so it is not a rate attributable to this Node alone.",
    )
    expect(RESOURCE_METRICS_DISCLOSURE).toBe(RESOURCE_METRICS_EXPLANATION)

    expect(RESYNC_TARGET_EXPLANATION).toBe(
      'The last recorded advance toward the Historical High-Water Mark, not the latest report time. The target is the Node’s retained historical height, not the Network Head.',
    )
    expect(RESYNC_TARGET_DISCLOSURE).toBe(RESYNC_TARGET_EXPLANATION)

    expect(RESYNC_PERCENTAGE_UNAVAILABLE_EXPLANATION).toBe(
      'No percentage is asserted: a percentage needs a valid current Block Height and a valid, computable Historical High-Water Mark target.',
    )
    expect(RESYNC_PERCENTAGE_UNAVAILABLE_DISCLOSURE).toBe(RESYNC_PERCENTAGE_UNAVAILABLE_EXPLANATION)
  })

  it('provides structured disclosure objects', () => {
    expect(NODE_CARD_EXPLANATIONS.role).toBe(NODE_ROLE_EXPLANATION)
    expect(NODE_CARD_EXPLANATIONS.activeAndHealth).toBe(ACTIVE_NODES_AND_HEALTH_EXPLANATION)
    expect(NODE_CARD_EXPLANATIONS.attention).toBe(HOME_ATTENTION_EXPLANATION)
    expect(NODE_CARD_EXPLANATIONS.resourceMetrics).toBe(RESOURCE_METRICS_EXPLANATION)

    expect(NODE_CARD_DISCLOSURES).toBe(NODE_CARD_EXPLANATIONS)

    expect(RESYNC_EXPLANATIONS.target).toBe(RESYNC_TARGET_EXPLANATION)
    expect(RESYNC_EXPLANATIONS.percentageUnavailable).toBe(RESYNC_PERCENTAGE_UNAVAILABLE_EXPLANATION)

    expect(RESYNC_DISCLOSURES).toBe(RESYNC_EXPLANATIONS)
  })

  it('formats node identity description with network and uptime prefix', () => {
    const formatted = formatNodeIdentityDescription('PlatON Mainnet', '14d 6h')
    expect(formatted).toBe(
      'Network: PlatON Mainnet · Uptime 14d 6h. Node role describes the Node’s consensus membership, not its linked Validator’s current staking validity or the freshness of Provider data.',
    )
  })

  it('formats resource metrics explanation with optional node data bytes', () => {
    const withoutData = formatResourceMetricsExplanation()
    expect(withoutData).toBe(RESOURCE_METRICS_EXPLANATION)

    const withUndefined = formatResourceMetricsExplanation(undefined)
    expect(withUndefined).toBe(RESOURCE_METRICS_EXPLANATION)

    const withEmpty = formatResourceMetricsExplanation('')
    expect(withEmpty).toBe(RESOURCE_METRICS_EXPLANATION)

    const withData = formatResourceMetricsExplanation('12.3 GB / 100 GB')
    expect(withData).toBe(
      RESOURCE_METRICS_EXPLANATION + ' Node data: 12.3 GB / 100 GB.',
    )
  })
})
