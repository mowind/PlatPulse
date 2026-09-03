import { cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { BrowserRouter } from 'react-router'
import type { PublicNetwork } from '../api/generated'
import HomeDashboard from './HomeDashboard'

const network = {
  networkKey: 'mainnet',
  displayName: 'Mainnet',
  geo: { state: 'disabled' },
  peers: { state: 'unsupported', freshness: 'unknown' },
  validators: [],
  nodes: [
    {
      nodeId: 'node-a', displayName: 'Alpha', networkKey: 'mainnet', health: 'healthy', healthReason: 'RPC reachable',
      rpcState: 'connected', syncState: 'synced', consensusState: 'ready', processState: 'running', resyncState: 'idle',
      currentHead: 120, latestBlockTransactionCount: 12345, historicalHighWatermark: 120, networkReferenceHead: 120,
      networkReferenceConfidence: 'high', freshness: 'current', resyncProgress: null,
      hostCpuPercent: 91.5, hostMemoryPercent: 82.25, processCpuPercent: 12.5, processMemoryPercent: 45.25, hostStoragePercent: 80, nodeDataDirectorySizeBytes: 12_884_901_888, nodeDataDirectoryCapacityBytes: 51_539_607_552, hostNetworkRxBytesPerSec: 1024, hostNetworkTxBytesPerSec: 2048,
      peers: { state: 'current', freshness: 'current', peerCount: 0 },
      consensus: {
        state: 'ok', freshness: 'current', observedAt: '2026-08-25T00:00:00Z', receivedAt: '2026-08-25T00:00:00Z',
        epoch: 1, viewNumber: 2, validator: true, highestQcBlock: 100, highestLockBlock: 99, highestCommitBlock: 98,
      },
      validator: null,
    },
    {
      nodeId: 'node-b', displayName: 'Beta', networkKey: 'mainnet', health: 'unknown', healthReason: 'Never observed',
      rpcState: 'unknown', syncState: 'unknown', consensusState: 'unknown', processState: 'unknown', resyncState: 'unknown',
      currentHead: null, latestBlockTransactionCount: null, historicalHighWatermark: null, hostCpuPercent: null, networkReferenceHead: null,
      networkReferenceConfidence: 'unknown', freshness: 'unknown', resyncProgress: null,
      peers: { state: 'unknown', freshness: 'unknown', peerCount: null },
      consensus: { state: 'unknown', freshness: 'unknown', validator: null, highestQcBlock: null, highestLockBlock: null, highestCommitBlock: null },
      validator: null,
    },
  ],
} satisfies PublicNetwork

/** Node card links carry the whole card as their accessible name (issue #97). */
const nodeCardLink = (name: string) => screen.getByRole('link', { name: new RegExp(name) })
const cardOf = (link: HTMLElement) => link.closest('article') as HTMLElement

afterEach(cleanup)

describe('Public Home dashboard', () => {
  it('summarizes Server-owned Nodes and preserves authoritative zero peer count', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    expect(screen.getByText('Active Nodes').nextElementSibling?.textContent).toBe('2')
    expect(screen.getByText('Healthy Nodes').nextElementSibling?.textContent).toBe('1')
    expect(screen.getByText('Attention').nextElementSibling?.textContent).toBe('1')
    const alphaCard = cardOf(nodeCardLink('Alpha'))
    expect(within(alphaCard).getByText('PEERS').nextElementSibling?.textContent).toBe('0')
    // A successful zero snapshot stays an authoritative zero, not Unknown.
    expect(within(alphaCard).getByText('Empty; authoritative zero')).toBeTruthy()
  })

  it('shows the first compact metric row as HEAD, TXS, and PEERS with Unknown on absence', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    const alphaCard = cardOf(nodeCardLink('Alpha'))
    expect(within(alphaCard).getByText('HEAD')).toBeTruthy()
    expect(within(alphaCard).getByText('120')).toBeTruthy()
    expect(within(alphaCard).getByText('TXS')).toBeTruthy()
    // Formatted exact match is rendered with locale grouping, never as a raw number.
    expect(within(alphaCard).getByText('12,345')).toBeTruthy()
    expect(within(alphaCard).getByText('PEERS')).toBeTruthy()
    expect(within(alphaCard).getByText('0')).toBeTruthy()
    // No exact Block Summary match is Unknown, not zero (issue #98).
    const betaCard = cardOf(nodeCardLink('Beta'))
    expect(within(betaCard).getByText('HEAD')).toBeTruthy()
    expect(within(betaCard).getByText('TXS')).toBeTruthy()
    expect(within(betaCard).getByText('PEERS')).toBeTruthy()
    expect(within(betaCard).getAllByText('Unknown').length).toBeGreaterThanOrEqual(3)
  })

  it('shows process CPU and memory with Node data and host network rates', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    const alphaCard = cardOf(nodeCardLink('Alpha'))
    const resources = within(alphaCard).getByLabelText('Node process and host network resources')
    const highlights = within(alphaCard).getByLabelText('Node highlights')
    expect(resources.compareDocumentPosition(highlights) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(within(resources).getByText('CPU')).toBeTruthy()
    expect(within(resources).getByText('12.5%')).toBeTruthy()
    expect(within(resources).getByText('45.3%')).toBeTruthy()
    const nodeDataLabel = within(resources).getByText('NODE DATA')
    expect(nodeDataLabel).toBeTruthy()
    expect(within(resources).getByText('12.0 GiB')).toBeTruthy()
    const nodeDataMetric = nodeDataLabel.parentElement as HTMLElement
    expect(nodeDataMetric.classList.contains('dashboard-node-primary-metric-progress')).toBe(true)
    expect(nodeDataMetric.style.getPropertyValue('--metric-progress')).toBe('25%')
    expect(within(resources).queryByText('STORAGE')).toBeNull()
    expect(within(resources).getByText('2.00 KiB/s')).toBeTruthy()
    expect(within(resources).getByText('1.00 KiB/s')).toBeTruthy()
  })
  it('shows Node data as a percentage of its filesystem capacity', () => {
    const withCapacity = {
      ...network,
      nodes: [
        { ...network.nodes[0], nodeDataDirectoryCapacityBytes: 51_539_607_552 },
        network.nodes[1],
      ],
    } as unknown as PublicNetwork
    render(<BrowserRouter><HomeDashboard networks={[withCapacity]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    const alphaCard = cardOf(nodeCardLink('Alpha'))
    const nodeDataMetric = within(alphaCard).getByText('NODE DATA').parentElement as HTMLElement
    expect(nodeDataMetric.classList.contains('dashboard-node-primary-metric-progress')).toBe(true)
    expect(nodeDataMetric.style.getPropertyValue('--metric-progress')).toBe('25%')
    expect(within(nodeDataMetric).getByText('48.0 GiB total · 25.0%')).toBeTruthy()
  })

  it('shows the second compact metric row as QC, LOCKED, COMMITTED, and VALIDATOR', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    const alphaCard = cardOf(nodeCardLink('Alpha'))
    expect(within(alphaCard).getByText('QC')).toBeTruthy()
    expect(within(alphaCard).getByText('100')).toBeTruthy()
    expect(within(alphaCard).getByText('LOCKED')).toBeTruthy()
    expect(within(alphaCard).getByText('99')).toBeTruthy()
    expect(within(alphaCard).getByText('COMMITTED')).toBeTruthy()
    expect(within(alphaCard).getByText('98')).toBeTruthy()
    expect(within(alphaCard).getByText('VALIDATOR')).toBeTruthy()
    // Current successful membership renders True, not a badge or color.
    expect(within(alphaCard).getByText('True')).toBeTruthy()
    expect(within(alphaCard).queryByText('Stale')).toBeNull()

    // Never-observed consensus is Unknown for every metric, never zero/False.
    const betaCard = cardOf(nodeCardLink('Beta'))
    for (const label of ['QC', 'LOCKED', 'COMMITTED', 'VALIDATOR']) {
      expect(within(betaCard).getByText(label)).toBeTruthy()
    }
    expect(within(betaCard).getAllByText('Unknown').length).toBeGreaterThanOrEqual(7)
    expect(within(betaCard).queryByText('False')).toBeNull()
  })

  it('retains last-good consensus values and visibly marks failed or stale collections', () => {
    const staleTrue = {
      ...network.nodes[0],
      nodeId: 'node-stale-true', displayName: 'Stale True',
      consensus: { ...network.nodes[0].consensus!, freshness: 'stale', validator: true, highestQcBlock: 141, highestLockBlock: 140, highestCommitBlock: 139 },
    }
    const failedTrue = {
      ...network.nodes[0],
      nodeId: 'node-failed-true', displayName: 'Failed True',
      consensus: { ...network.nodes[0].consensus!, state: 'error', validator: true, highestQcBlock: 151, highestLockBlock: 150, highestCommitBlock: 149 },
    }
    const failedWithoutLastGood = {
      ...network.nodes[1],
      nodeId: 'node-failed-none', displayName: 'Failed None',
      consensus: { state: 'error', freshness: 'unknown', validator: null, highestQcBlock: null, highestLockBlock: null, highestCommitBlock: null },
    }
    const currentFalse = {
      ...network.nodes[0],
      nodeId: 'node-current-false', displayName: 'Current False',
      consensus: { ...network.nodes[0].consensus!, validator: false, highestQcBlock: 0, highestLockBlock: 0, highestCommitBlock: 0 },
    }
    const staleFalse = {
      ...network.nodes[0],
      nodeId: 'node-stale-false', displayName: 'Stale False',
      consensus: { ...network.nodes[0].consensus!, freshness: 'stale', validator: false, highestQcBlock: 161, highestLockBlock: 160, highestCommitBlock: 159 },
    }
    const unknownFreshness = {
      ...network.nodes[0],
      nodeId: 'node-unknown-freshness', displayName: 'Unknown Freshness',
      consensus: { ...network.nodes[0].consensus!, freshness: 'unknown', validator: true, highestQcBlock: 171, highestLockBlock: 170, highestCommitBlock: 169 },
    }
    render(<BrowserRouter><HomeDashboard
      networks={[{ ...network, nodes: [staleTrue, staleFalse, failedTrue, failedWithoutLastGood, currentFalse, unknownFreshness] }]}
      realtimeStatus="connected" online resetting={false} error={null} loading={false}
    /></BrowserRouter>)

    // A stale successful observation keeps True and every block height, and
    // visibly marks the retained row Stale (text, never color only).
    const staleCard = cardOf(nodeCardLink('Stale True'))
    expect(within(staleCard).getByText('141')).toBeTruthy()
    expect(within(staleCard).getByText('140')).toBeTruthy()
    expect(within(staleCard).getByText('139')).toBeTruthy()
    expect(within(staleCard).getByText('True')).toBeTruthy()
    expect(within(staleCard).getAllByText('Stale')).toHaveLength(4)

    // A failed collection with last-good true keeps the value and is Stale.
    const failedCard = cardOf(nodeCardLink('Failed True'))
    expect(within(failedCard).getByText('151')).toBeTruthy()
    expect(within(failedCard).getByText('150')).toBeTruthy()
    expect(within(failedCard).getByText('149')).toBeTruthy()
    expect(within(failedCard).getByText('True')).toBeTruthy()
    expect(within(failedCard).getAllByText('Stale')).toHaveLength(4)

    // A stale successful non-membership keeps False and marks it Stale.
    const staleFalseCard = cardOf(nodeCardLink('Stale False'))
    expect(within(staleFalseCard).getByText('161')).toBeTruthy()
    expect(within(staleFalseCard).getByText('False')).toBeTruthy()
    expect(within(staleFalseCard).getAllByText('Stale')).toHaveLength(4)

    // A failed collection without a last-good membership is Unknown, never
    // False, and is not dressed up as Stale with no retained value.
    const failedNoneCard = cardOf(nodeCardLink('Failed None'))
    expect(within(failedNoneCard).getAllByText('Unknown').length).toBeGreaterThanOrEqual(4)
    expect(within(failedNoneCard).queryByText('Stale')).toBeNull()
    expect(within(failedNoneCard).queryByText('False')).toBeNull()

    // A current successful non-membership renders False; an observed zero
    // block height is an authoritative zero, never Unknown.
    const falseCard = cardOf(nodeCardLink('Current False'))
    expect(within(falseCard).getByText('False')).toBeTruthy()
    expect(within(falseCard).getAllByText('0').length).toBeGreaterThanOrEqual(4)
    expect(within(falseCard).queryByText('Stale')).toBeNull()

    // Unknown freshness means currency cannot be certified: the retained
    // value must not be presented as current True/False or block heights.
    const unknownFreshnessCard = cardOf(nodeCardLink('Unknown Freshness'))
    expect(within(unknownFreshnessCard).getAllByText('Unknown').length).toBeGreaterThanOrEqual(4)
    expect(within(unknownFreshnessCard).queryByText('True')).toBeNull()
    expect(within(unknownFreshnessCard).queryByText('False')).toBeNull()
    expect(within(unknownFreshnessCard).queryByText('Stale')).toBeNull()
  })

  it('renders link-aware Validator Activity before Node Health as separate text badges', () => {
    const linked = (activity: string, activityState: string) => ({
      validatorId: 'validator-a', validatorNodeId: '0xvalidator', displayName: 'Validator A',
      nodeId: 'node-a', linkRole: 'primary', state: activityState === 'stale' ? 'error' : 'fresh',
      freshness: activityState === 'stale' ? 'fresh' : 'fresh', source: 'fake',
      receivedAt: '2026-08-25T00:00:00Z', counterState: 'normal', activity, activityState,
    })
    const active = {
      ...network.nodes[0], nodeId: 'node-active', displayName: 'Active Node',
      validator: linked('producing', 'current'),
    }
    const observing = {
      ...network.nodes[0], nodeId: 'node-observing', displayName: 'Observing Node',
      validator: linked('observing', 'current'),
    }
    const unknown = {
      ...network.nodes[0], nodeId: 'node-unknown', displayName: 'Unknown Node',
      validator: null,
    }
    const stale = {
      ...network.nodes[0], nodeId: 'node-stale', displayName: 'Stale Node',
      validator: linked('locked', 'stale'),
    }
    render(<BrowserRouter><HomeDashboard
      networks={[{ ...network, nodes: [active, observing, unknown, stale] }]}
      realtimeStatus="connected" online resetting={false} error={null} loading={false}
    /></BrowserRouter>)

    // Activity badge is the first header badge, Node Health the second; both
    // are text badges, never color-only (issue #100).
    const activeCard = cardOf(nodeCardLink('Active Node'))
    const activeBadges = activeCard.querySelectorAll('.status-badge')
    expect(activeBadges).toHaveLength(2)
    expect(activeBadges[0].textContent).toContain('Producing')
    expect(activeBadges[1].textContent).toContain('Healthy')
    expect(within(activeCard).getByText('Producing')).toBeTruthy()

    // Authoritative empty/not-found renders Observing, not a fabricated label.
    const observingCard = cardOf(nodeCardLink('Observing Node'))
    expect(within(observingCard).getByText('Observing')).toBeTruthy()
    expect(observingCard.querySelectorAll('.status-badge')[0]?.textContent).toContain('Observing')

    // No effective explicit Link renders Observing Activity before Health.
    const unknownCard = cardOf(nodeCardLink('Unknown Node'))
    const unknownBadges = unknownCard.querySelectorAll('.status-badge')
    expect(unknownBadges[0].textContent).toContain('Observing')
    expect(unknownBadges[1].textContent).toContain('Healthy')

    // Provider failure with a last-good Activity keeps the label and visibly
    // marks it Stale; the independent Health badge stays Healthy.
    const staleCard = cardOf(nodeCardLink('Stale Node'))
    const staleBadges = staleCard.querySelectorAll('.status-badge')
    expect(staleBadges[0].textContent).toContain('Locked')
    expect(staleBadges[0].textContent).toContain('Stale')
    expect(staleBadges[1].textContent).toContain('Healthy')
    expect(within(staleCard).getByText('Locked (Stale)')).toBeTruthy()
  })

  it('keeps summary cards to marker, title, and number with a compact shell', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    expect(screen.queryByText('Active Nodes on Home')).toBeNull()
    expect(screen.queryByText('Server-owned health')).toBeNull()
    expect(screen.queryByText('Unknown and degraded included')).toBeNull()
    expect(screen.queryByText('Published Network groups')).toBeNull()
    expect(screen.queryByText('PLATPULSE / NETWORK OBSERVATORY')).toBeNull()
    expect(screen.queryByText('A live operational view of every Active PlatON Node.')).toBeNull()
    expect(screen.queryByText('Current', { exact: true })).toBeNull()
    expect(screen.queryByRole('heading', { level: 1, name: 'Home' })).toBeNull()
    const cards = screen.getAllByRole('article').filter((card) => card.className.includes('dashboard-summary-card'))
    expect(cards).toHaveLength(4)
    for (const card of cards) {
      // exactly one dot, one title, one number — no footer text
      expect(card.querySelectorAll('.dashboard-summary-dot')).toHaveLength(1)
      expect(card.querySelectorAll('p')).toHaveLength(1)
      expect(card.querySelectorAll('strong')).toHaveLength(1)
      expect(card.querySelectorAll('small')).toHaveLength(0)
    }
  })

  it('renders one whole-card Node link with the Network name as plain text', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    // One semantic link per Node card, named by its visible card content.
    const alphaLink = nodeCardLink('Alpha')
    expect(alphaLink.getAttribute('href')).toBe('/nodes/node-a')
    const alphaCard = cardOf(alphaLink)
    expect(alphaCard.querySelectorAll('a')).toHaveLength(1)
    // The Network display name is visible text inside the card link, never a
    // nested link (issue #97).
    expect(within(alphaCard).getByText('Mainnet')).toBeTruthy()
    expect(screen.queryByRole('link', { name: 'Mainnet' })).toBeNull()
    // The explicit "View Node Details" affordance is gone.
    expect(screen.queryByText('View Node Details')).toBeNull()
  })

  it('omits routine prose and component status rows on healthy Nodes', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    const alphaCard = cardOf(nodeCardLink('Alpha'))
    expect(within(alphaCard).queryByText('RPC reachable')).toBeNull()
    expect(within(alphaCard).queryByText('Last Observed')).toBeNull()
    expect(within(alphaCard).queryByText('RPC')).toBeNull()
    expect(within(alphaCard).queryByText('Sync')).toBeNull()
    expect(within(alphaCard).queryByText('Consensus')).toBeNull()
    expect(within(alphaCard).queryByText('Process')).toBeNull()
    expect(within(alphaCard).queryByText('No active resync')).toBeNull()
    expect(alphaCard.querySelectorAll('.dashboard-node-diagnostic')).toHaveLength(0)
  })

  it('keeps exactly one short diagnostic line on exceptional Nodes', () => {
    const resyncingNode = {
      ...network.nodes[0],
      nodeId: 'node-c', displayName: 'Gamma', health: 'healthy', resyncState: 'resyncing',
      resyncProgress: 'Backfilling 10,000 blocks', peers: { state: 'current', freshness: 'current', peerCount: 3 },
    }
    render(<BrowserRouter><HomeDashboard networks={[{ ...network, nodes: [network.nodes[1], resyncingNode] }]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    // Unknown Node: the Server-sanitized health reason is the single line.
    const betaCard = cardOf(nodeCardLink('Beta'))
    expect(within(betaCard).getByText('Never observed')).toBeTruthy()
    expect(betaCard.querySelectorAll('.dashboard-node-diagnostic')).toHaveLength(1)
    // The unknown peer observation is never presented as Current (webui.md §5.3).
    expect(within(betaCard).getByText('Unknown observation')).toBeTruthy()
    expect(within(betaCard).queryByText('Current observation')).toBeNull()

    // Healthy Node with an active resync: progress is the single line.
    const gammaCard = cardOf(nodeCardLink('Gamma'))
    expect(within(gammaCard).getByText('Backfilling 10,000 blocks')).toBeTruthy()
    expect(gammaCard.querySelectorAll('.dashboard-node-diagnostic')).toHaveLength(1)
  })

  it('filters by Network and sorts by supported operational fields', () => {
    const secondNetwork = { ...network, networkKey: 'testnet', displayName: 'Testnet', nodes: [{ ...network.nodes[0], nodeId: 'node-c', displayName: 'Gamma', networkKey: 'testnet', currentHead: 900 }] }
    render(<BrowserRouter><HomeDashboard networks={[network, secondNetwork]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    fireEvent.click(screen.getByRole('button', { name: 'Testnet' }))
    expect(nodeCardLink('Gamma')).toBeTruthy()
    expect(screen.queryByRole('link', { name: /Alpha/ })).toBeNull()

    fireEvent.click(screen.getByRole('button', { name: 'All Networks' }))
    fireEvent.change(screen.getByRole('combobox', { name: 'Sort' }), { target: { value: 'head' } })
    const nodeLinks = screen.getAllByRole('link').filter((link) => link.getAttribute('href')?.startsWith('/nodes/'))
    expect(nodeLinks.map((link) => link.getAttribute('href'))).toEqual(['/nodes/node-c', '/nodes/node-a', '/nodes/node-b'])
  })

  it('keeps transport and authorization state explicit', () => {
    render(<BrowserRouter><HomeDashboard networks={[]} realtimeStatus="disconnected" online={false} resetting={false} error={null} loading={false} /></BrowserRouter>)
    expect(screen.getByText('You are offline')).toBeTruthy()
    expect(screen.getByText('No Active Nodes in this view.')).toBeTruthy()
  })

  it('sorts an unhealthy Server Health Summary ahead of healthy Nodes', () => {
    const unhealthyNetwork = {
      ...network,
      nodes: [{ ...network.nodes[0], nodeId: 'node-error', displayName: 'Error Node', health: 'unhealthy', healthReason: 'RPC failed' }, network.nodes[0]],
    }
    render(<BrowserRouter><HomeDashboard networks={[unhealthyNetwork]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)
    const nodeLinks = screen.getAllByRole('link').filter((link) => link.getAttribute('href')?.startsWith('/nodes/'))
    expect(nodeLinks.map((link) => link.getAttribute('href'))).toEqual(['/nodes/node-error', '/nodes/node-a'])
  })

  it('does not render loading as fabricated zero-valued summary data', () => {
    render(<BrowserRouter><HomeDashboard networks={[]} realtimeStatus="connecting" online loading resetting={false} error={null} /></BrowserRouter>)
    expect(screen.getAllByText('—')).toHaveLength(4)
    expect(screen.queryByText('No Active Nodes in this view.')).toBeNull()
  })

  it('preserves an authoritative empty projection when a refresh fails', () => {
    render(<BrowserRouter><HomeDashboard networks={[]} realtimeStatus="connected" online resetting={false} error="Unable to load Active Nodes" hasLastGood loading={false} /></BrowserRouter>)
    expect(screen.getByText('Unable to load Active Nodes')).toBeTruthy()
    expect(screen.getByText('No Active Nodes in this view.')).toBeTruthy()
    expect(screen.getByText('Active Nodes').nextElementSibling?.textContent).toBe('0')
  })
})
