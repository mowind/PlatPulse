import { cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { MemoryRouter } from 'react-router'
import { HomeNodeCard } from './HomeNodeCard'
import type { PublicNetwork, PublicNode } from '../api/generated'
import { homeReturnState } from '../homeReturn'
import { formatNodeDataBytes } from '../formatBytes'
import { formatDuration } from '../formatDuration'
import {
  NODE_CARD_DISCLOSURES,
  RESYNC_DISCLOSURES,
  formatNodeIdentityDescription,
  formatResourceMetricsExplanation,
} from '../lib/nodeDisclosures'

afterEach(cleanup)

const mockNode: PublicNode = {
  nodeId: 'node-1',
  displayName: 'Validator Node 1',
  networkKey: 'testnet',
  health: 'healthy',
  healthReason: 'RPC reachable',
  rpcState: 'connected',
  syncState: 'synced',
  consensusState: 'ready',
  processState: 'running',
  resyncState: 'idle',
  processUptimeMs: 3600000,
  currentHead: 12345,
  latestBlockTransactionCount: 10,
  historicalHighWatermark: 12345,
  networkReferenceHead: 12345,
  networkReferenceConfidence: 'high',
  freshness: 'current',
  resyncProgress: null,
  hostCpuPercent: 50,
  hostMemoryPercent: 50,
  processCpuPercent: 10,
  processMemoryPercent: 20,
  hostStoragePercent: 60,
  nodeDataDirectorySizeBytes: 1000000000,
  nodeDataDirectoryCapacityBytes: 2000000000,
  hostNetworkRxBytesPerSec: 100,
  hostNetworkTxBytesPerSec: 200,
  peers: { state: 'ok', freshness: 'current', peerCount: 5 },
  peerCountries: { scope: 'unavailable', countries: [] },
  consensus: {
    state: 'ok',
    freshness: 'current',
    observedAt: '2026-10-25T00:00:00Z',
    receivedAt: '2026-10-25T00:00:00Z',
    epoch: 1,
    viewNumber: 1,
    validator: true,
    highestQcBlock: 90,
    highestLockBlock: 89,
    highestCommitBlock: 88,
  },
  validator: null,
}

const mockNetwork: PublicNetwork = {
  networkKey: 'testnet',
  displayName: 'PlatON Testnet',
  geo: { state: 'disabled', scope: 'unavailable' },
  peers: { state: 'unsupported', freshness: 'unknown' },
  validators: [],
  validatorSummary: {
    blocks: { knownSum: '100', expectedCount: 1, valuedCount: 1, staleCount: 0, state: 'complete' },
    rewards: { knownSum: '10', expectedCount: 1, valuedCount: 1, staleCount: 0, state: 'complete' },
    eligibleValidatorCount: 1,
    linkedNodeCount: 1,
    unlinkedNodeCount: 1,
  },
  nodes: [mockNode],
}

const mockResyncingNode: PublicNode = {
  ...mockNode,
  resyncState: 'resyncing',
  currentHead: null,
  historicalHighWatermark: null,
}

const linkState = homeReturnState('node-1', new URLSearchParams(''), 0, 1, 'nodes')

describe('HomeNodeCard Disclosures and Dialog', () => {
  it('opens identity details dialog with exact domain disclosure copy', async () => {
    render(
      <MemoryRouter>
        <HomeNodeCard network={mockNetwork} node={mockNode} linkState={linkState} />
      </MemoryRouter>,
    )

    const trigger = screen.getByRole('button', { name: 'Node identity details' })
    fireEvent.click(trigger)

    const dialog = await screen.findByRole('dialog')

    // Title
    expect(within(dialog).getByText('Validator Node 1')).toBeTruthy()

    // Dialog Description with identity info and role explanation
    expect(
      within(dialog).getByText(
        formatNodeIdentityDescription(mockNetwork.displayName, formatDuration(mockNode.processUptimeMs)),
      ),
    ).toBeTruthy()

    // Active & Health disclosure
    expect(within(dialog).getByText(NODE_CARD_DISCLOSURES.activeAndHealth)).toBeTruthy()

    // Home Attention disclosure
    expect(within(dialog).getByText(NODE_CARD_DISCLOSURES.attention)).toBeTruthy()

    // Resource metrics disclosure with node data bytes
    const nodeDataBytes = formatNodeDataBytes(mockNode.nodeDataDirectorySizeBytes, mockNode.nodeDataDirectoryCapacityBytes)
    expect(
      within(dialog).getByText(formatResourceMetricsExplanation(nodeDataBytes)),
    ).toBeTruthy()
  })

  it('opens resync dialog with exact resync disclosures', async () => {
    render(
      <MemoryRouter>
        <HomeNodeCard
          network={mockNetwork}
          node={mockResyncingNode}
          linkState={linkState}
        />
      </MemoryRouter>,
    )

    const resyncCue = screen.getByRole('button', { name: /Resync state:/ })
    fireEvent.click(resyncCue)

    const dialog = await screen.findByRole('dialog')
    expect(within(dialog).getByText('Resync progress')).toBeTruthy()
    expect(within(dialog).getByText(RESYNC_DISCLOSURES.target)).toBeTruthy()
    expect(within(dialog).getByText(RESYNC_DISCLOSURES.percentageUnavailable)).toBeTruthy()
  })
})
