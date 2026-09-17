import { cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { BrowserRouter } from 'react-router'
import type { PublicNetwork } from '../api/generated'
import HomeDashboard from './HomeDashboard'

const network = {
  networkKey: 'mainnet',
  displayName: 'Mainnet',
  geo: { state: 'disabled', scope: 'unavailable' },
  peers: { state: 'unsupported', freshness: 'unknown' },
  validators: [],
  nodes: [
    {
      nodeId: 'node-a', displayName: 'Alpha', networkKey: 'mainnet', health: 'healthy', healthReason: 'RPC reachable',
      rpcState: 'connected', syncState: 'synced', consensusState: 'ready', processState: 'running', resyncState: 'idle',
      currentHead: 120, latestBlockTransactionCount: 12345, historicalHighWatermark: 120, networkReferenceHead: 120,
      networkReferenceConfidence: 'high', freshness: 'current', resyncProgress: null,
      hostCpuPercent: 91.5, hostMemoryPercent: 82.25, processCpuPercent: 12.5, processMemoryPercent: 45.25, hostStoragePercent: 80, nodeDataDirectorySizeBytes: 12_884_901_888, nodeDataDirectoryCapacityBytes: 51_539_607_552, hostNetworkRxBytesPerSec: 1024, hostNetworkTxBytesPerSec: 2048,
      peers: { state: 'ok', freshness: 'current', peerCount: 0 },
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

/** The summary card carrying the given label (issue #142). */
const summaryCardOf = (label: string) => {
  const card = screen.getByText(label).closest('[data-slot="summary-card"]')
  if (!card) throw new Error(`No summary card for ${label}`)
  return card as HTMLElement
}

/** Its value element, which is the number the label announces. */
const summaryValueOf = (label: string) => {
  const value = summaryCardOf(label).querySelector('[data-slot="summary-value"]')
  if (!value) throw new Error(`No summary value for ${label}`)
  return value
}

afterEach(cleanup)

describe('Public Home dashboard', () => {
  it('scopes all four counters, map and cards to the selected network', () => {
    const second = { ...network, networkKey: 'testnet', displayName: 'Testnet', nodes: [{ ...network.nodes[0], nodeId: 'gamma', displayName: 'Gamma', networkKey: 'testnet' }] }
    render(<BrowserRouter><HomeDashboard networks={[network, second]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)
    fireEvent.mouseDown(screen.getByRole('tab', { name: 'Testnet' }), { button: 0, ctrlKey: false })
    expect(summaryValueOf('Active Nodes').textContent).toBe('1')
    expect(summaryValueOf('Healthy Nodes').textContent).toBe('1')
    expect(summaryValueOf('Attention').textContent).toBe('0')
    expect(summaryValueOf('Networks').textContent).toBe('1')
    expect(screen.queryByRole('link', { name: /Alpha/ })).toBeNull()
    expect(nodeCardLink('Gamma')).toBeTruthy()
    expect(screen.getByRole('region', { name: 'Peer countries' }).getAttribute('data-network-filter')).toBe('testnet')
  })

  it('summarizes Server-owned Nodes and preserves authoritative zero peer count', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    expect(screen.queryByText('Live updates connected')).toBeNull()
    expect(document.querySelector('[data-realtime-status="connected"]')).toBeTruthy()
    expect(summaryValueOf('Active Nodes').textContent).toBe('2')
    expect(summaryValueOf('Healthy Nodes').textContent).toBe('1')
    expect(summaryValueOf('Attention').textContent).toBe('1')
    const alphaCard = cardOf(nodeCardLink('Alpha'))
    expect(within(alphaCard).getByText('Peers').nextElementSibling?.textContent).toBe('0')
    // A successful zero snapshot stays an authoritative zero, not Unknown.
    expect(within(alphaCard).getByText('Empty; authoritative zero')).toBeTruthy()
  })

  it('keeps retained zero peer values and simultaneous failure and staleness visible', () => {
    const retainedNode = (state: 'starting' | 'disabled' | 'unsupported' | 'error') => ({
      ...network.nodes[0],
      nodeId: `node-${state}`,
      displayName: `Retained ${state}`,
      peers: {
        state,
        freshness: state === 'error' ? 'stale' : 'current',
        peerCount: 0,
      },
    })
    const nodes = [
      retainedNode('starting'),
      retainedNode('disabled'),
      retainedNode('unsupported'),
      retainedNode('error'),
    ]
    render(<BrowserRouter><HomeDashboard networks={[{ ...network, nodes }]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    for (const state of ['starting', 'disabled', 'unsupported', 'error']) {
      const card = cardOf(nodeCardLink(`Retained ${state}`))
      expect(within(card).getByText('0')).toBeTruthy()
      expect(within(card).getByText(/Showing last successful snapshot/)).toBeTruthy()
    }
    const failedCard = cardOf(nodeCardLink('Retained error'))
    expect(within(failedCard).getByText(/Collection failed.*Stale.*Showing last successful snapshot/)).toBeTruthy()
  })

  it('shows the first compact metric row as Head, Txs, and Peers with Unknown on absence', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    const alphaCard = cardOf(nodeCardLink('Alpha'))
    expect(within(alphaCard).getByText('Head')).toBeTruthy()
    expect(within(alphaCard).getByText('120')).toBeTruthy()
    expect(within(alphaCard).getByText('Txs')).toBeTruthy()
    // Formatted exact match is rendered with locale grouping, never as a raw number.
    expect(within(alphaCard).getByText('12,345')).toBeTruthy()
    expect(within(alphaCard).getByText('Peers')).toBeTruthy()
    expect(within(alphaCard).getByText('0')).toBeTruthy()
    for (const legacyLabel of ['HEAD', 'TXS', 'PEERS', 'MEMORY', 'NODE DATA', 'LOCKED', 'COMMITTED', 'VALIDATOR']) {
      expect(within(alphaCard).queryByText(legacyLabel, { exact: true })).toBeNull()
    }
    // No exact Block Summary match is Unknown, not zero (issue #98).
    const betaCard = cardOf(nodeCardLink('Beta'))
    expect(within(betaCard).getByText('Head')).toBeTruthy()
    expect(within(betaCard).getByText('Txs')).toBeTruthy()
    expect(within(betaCard).getByText('Peers')).toBeTruthy()
    expect(within(betaCard).getAllByText('Unknown').length).toBeGreaterThanOrEqual(3)
  })

  it('shows process CPU and memory with Node data and host network rates', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    const alphaCard = cardOf(nodeCardLink('Alpha'))
    const resources = within(alphaCard).getByLabelText('Node process and host network resources')
    const highlights = alphaCard.querySelector('[data-slot="node-business-metrics"]')!
    expect(resources.compareDocumentPosition(highlights) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(within(resources).getByText('CPU')).toBeTruthy()
    expect(within(resources).getByText('12.5%')).toBeTruthy()
    expect(within(resources).getByText('45.3%')).toBeTruthy()
    const nodeDataLabel = within(resources).getByText('Node data')
    expect(nodeDataLabel).toBeTruthy()
    expect(within(resources).getByText('12.0 GiB / 48.0 GiB')).toBeTruthy()
    const nodeDataMetric = nodeDataLabel.parentElement as HTMLElement
    expect(nodeDataMetric.getAttribute('data-slot')).toBe('metric-row')
    expect(nodeDataMetric.parentElement?.className).toBe('md:col-span-2')
    expect(resources.className).toContain('grid-cols-2')
    expect(nodeDataMetric.querySelector('[data-slot="progress-thin"]')).toBeTruthy()
    expect(nodeDataMetric.querySelector('[role="progressbar"]')?.getAttribute('aria-valuenow')).toBe('25')
    expect(within(resources).queryByText('STORAGE')).toBeNull()
    expect(within(resources).getByText('16.4Kbps')).toBeTruthy()
    expect(within(resources).getByLabelText('Upload 16.4Kbps').querySelector('svg')).not.toBeNull()
    expect(within(resources).getByText('8.19Kbps')).toBeTruthy()
  })
  it('places full-width Node uptime immediately below the full-width host network speed row', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)
    const resources = within(cardOf(nodeCardLink('Alpha'))).getByLabelText('Node process and host network resources')
    const speed = within(resources).getByRole('group', { name: 'Host network speed' })
    expect(speed.parentElement).toBe(resources)
    expect(speed.classList.contains('col-span-2')).toBe(true)
    const uptime = within(resources).getByRole('group', { name: 'Node uptime' })
    expect(uptime.parentElement).toBe(resources)
    expect(uptime.classList.contains('col-span-2')).toBe(true)
    expect(uptime.textContent).toBe('Node uptimeUnknown')
    expect(speed.nextElementSibling).toBe(uptime)
    expect(within(speed).getByLabelText('Upload 16.4Kbps')).toBeTruthy()
    expect(within(speed).getByLabelText('Download 8.19Kbps')).toBeTruthy()
    expect(within(speed).queryByText('Node data')).toBeNull()
  })

  it.each([
    [0, '0s'],
    [59_999, '59s'],
    [60_000, '1m'],
    [3_660_000, '1h 1m'],
    [183_600_000, '2d 3h'],
    [null, 'Unknown'],
    [undefined, 'Unknown'],
    [-1, 'Unknown'],
    [NaN, 'Unknown'],
    [Infinity, 'Unknown'],
  ])('shows Node process uptime %s as %s', (processUptimeMs, expected) => {
    const nodes = [{ ...network.nodes[0], processUptimeMs }]
    render(<BrowserRouter><HomeDashboard networks={[{ ...network, nodes }]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)
    expect(within(cardOf(nodeCardLink('Alpha'))).getByText('Node uptime').nextElementSibling?.textContent).toBe(expected)
  })

  it('renders Node data as a percentage with the used / total byte detail under its progress bar', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    const alphaCard = cardOf(nodeCardLink('Alpha'))
    const nodeDataMetric = within(alphaCard).getByText('Node data').parentElement as HTMLElement
    expect(within(nodeDataMetric).getByText('25.0%')).toBeTruthy()
    expect(within(nodeDataMetric).getByText('12.0 GiB / 48.0 GiB')).toBeTruthy()
    expect(nodeDataMetric.querySelector('[role="progressbar"]')?.getAttribute('aria-valuenow')).toBe('25')
  })

  it('lays every Home metric out as one data-item / value row with its detail below', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    const alphaCard = cardOf(nodeCardLink('Alpha'))
    // Resources (CPU, Memory, Node data, Node uptime, Speed), main (Head, Txs,
    // Peers), and consensus (QC, Locked, Committed). Their shared
    // shape is covered by MetricRow.test.tsx.
    expect(alphaCard.querySelectorAll('[data-slot="metric-row"]')).toHaveLength(11)
  })

  it('orders full-width business rows before the wrapping count pair and puts the role in the header', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)
    const card = cardOf(nodeCardLink('Alpha'))
    expect(card.querySelector('[data-slot="metric-triple"]')).toBeNull()
    const metrics = card.querySelector('[data-slot="node-business-metrics"]')!
    expect(Array.from(metrics.querySelectorAll('[data-slot="metric-row-label"]'), el => el.textContent))
      .toEqual(['Head', 'QC', 'Locked', 'Committed', 'Txs', 'Peers'])
    expect(metrics.querySelector('[data-short-label]')).toBeNull()
    expect(metrics.querySelector('[data-slot="validator-role"]')).toBeNull()
    expect(card.querySelector('[data-slot="card-x-header"] [data-slot="validator-role"]')?.textContent).toBe('Validator')
  })

  it.each([
    [0, '0bps'],
    [100, '800bps'],
    [125, '1Kbps'],
    [125_000, '1Mbps'],
    [125_000_000, '1Gbps'],
    [125_000_000_000, '1Tbps'],
    [124_999, '1Mbps'],
    [null, 'Unknown'],
  ])('formats %s bytes/s using an appropriate speed unit', (rate, expected) => {
    const nodes = [{ ...network.nodes[0], hostNetworkTxBytesPerSec: rate, hostNetworkRxBytesPerSec: 100 }]
    render(<BrowserRouter><HomeDashboard networks={[{ ...network, nodes }]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)
    expect(within(cardOf(nodeCardLink('Alpha'))).getByText('Speed').nextElementSibling?.textContent).toBe(`${expected}800bps`)
  })

  it('converts bytes per second to decimal Mbps and preserves missing speeds', () => {
    const nodes = [{ ...network.nodes[0], hostNetworkTxBytesPerSec: 1_250_000, hostNetworkRxBytesPerSec: 2_500_000 }, network.nodes[1]]
    render(<BrowserRouter><HomeDashboard networks={[{ ...network, nodes }]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)
    expect(within(cardOf(nodeCardLink('Alpha'))).getByText('Speed').nextElementSibling?.textContent).toBe('10Mbps20Mbps')
    expect(within(cardOf(nodeCardLink('Beta'))).getByText('Speed').nextElementSibling?.textContent).toBe('UnknownUnknown')
  })

  it('shows the second compact metric row as QC, Locked, Committed, and Validator', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    const alphaCard = cardOf(nodeCardLink('Alpha'))
    expect(within(alphaCard).getByText('QC')).toBeTruthy()
    expect(within(alphaCard).getByText('100')).toBeTruthy()
    expect(within(alphaCard).getByText('Locked')).toBeTruthy()
    expect(within(alphaCard).getByText('99')).toBeTruthy()
    expect(within(alphaCard).getByText('Committed')).toBeTruthy()
    expect(within(alphaCard).getByText('98')).toBeTruthy()
    expect(within(alphaCard).getByText('Validator')).toBeTruthy()
    // Current successful membership renders a neutral role badge.
    expect(within(alphaCard).getByText('Validator')).toBeTruthy()
    expect(within(alphaCard).queryByText('Stale')).toBeNull()

    // Never-observed consensus is Unknown for every metric, never zero/No.
    const betaCard = cardOf(nodeCardLink('Beta'))
    for (const label of ['QC', 'Locked', 'Committed']) {
      expect(within(betaCard).getByText(label)).toBeTruthy()
    }
    expect(within(betaCard).getAllByText('Unknown').length).toBeGreaterThanOrEqual(7)
    expect(within(betaCard).queryByText('Non-validator')).toBeNull()
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

    // A stale successful observation keeps Yes and every block height, and
    // visibly marks the retained row Stale (text, never color only).
    const staleCard = cardOf(nodeCardLink('Stale True'))
    expect(within(staleCard).getByText('141')).toBeTruthy()
    expect(within(staleCard).getByText('140')).toBeTruthy()
    expect(within(staleCard).getByText('139')).toBeTruthy()
    expect(within(staleCard).getByText('Validator')).toBeTruthy()
    expect(within(staleCard).getAllByText('Stale')).toHaveLength(4)

    // A failed collection with last-good true keeps the value and is Stale.
    const failedCard = cardOf(nodeCardLink('Failed True'))
    expect(within(failedCard).getByText('151')).toBeTruthy()
    expect(within(failedCard).getByText('150')).toBeTruthy()
    expect(within(failedCard).getByText('149')).toBeTruthy()
    expect(within(failedCard).getByText('Validator')).toBeTruthy()
    expect(within(failedCard).getAllByText('Stale')).toHaveLength(4)

    // A stale successful non-membership keeps No and marks it Stale.
    const staleFalseCard = cardOf(nodeCardLink('Stale False'))
    expect(within(staleFalseCard).getByText('161')).toBeTruthy()
    expect(within(staleFalseCard).getByText('Non-validator')).toBeTruthy()
    expect(within(staleFalseCard).getAllByText('Stale')).toHaveLength(4)

    // A failed collection without a last-good membership is Unknown, never
    // No, and is not dressed up as Stale with no retained value.
    const failedNoneCard = cardOf(nodeCardLink('Failed None'))
    expect(within(failedNoneCard).getAllByText('Unknown').length).toBeGreaterThanOrEqual(4)
    expect(within(failedNoneCard).queryByText('Stale')).toBeNull()
    expect(within(failedNoneCard).queryByText('Non-validator')).toBeNull()

    // A current successful non-membership renders No; an observed zero
    // block height is an authoritative zero, never Unknown.
    const falseCard = cardOf(nodeCardLink('Current False'))
    expect(within(falseCard).getByText('Non-validator')).toBeTruthy()
    expect(within(falseCard).getAllByText('0').length).toBeGreaterThanOrEqual(4)
    expect(within(falseCard).queryByText('Stale')).toBeNull()

    // Unknown freshness means currency cannot be certified: the retained
    // value must not be presented as current Yes/No or block heights.
    const unknownFreshnessCard = cardOf(nodeCardLink('Unknown Freshness'))
    expect(within(unknownFreshnessCard).getAllByText('Unknown').length).toBeGreaterThanOrEqual(4)
    expect(within(unknownFreshnessCard).queryByText('Validator')).toBeNull()
    expect(within(unknownFreshnessCard).queryByText('Non-validator')).toBeNull()
    expect(within(unknownFreshnessCard).queryByText('Stale')).toBeNull()
  })

  it('keeps the two-state health marker as the header status cue with no activity badge', () => {
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

    // The current SPA renders Node Validator Activity nowhere, so no activity
    // or freshness value can put a badge on a Home card. Node Health stays a
    // two-state marker whose accessible name carries the health word (#141).
    for (const name of ['Active Node', 'Observing Node', 'Unknown Node', 'Stale Node']) {
      const card = cardOf(nodeCardLink(name))
      expect(card.querySelectorAll('.status-badge')).toHaveLength(0)
      expect(within(card).getByRole('img', { name: 'Healthy' })).toBeTruthy()
    }
    expect(within(cardOf(nodeCardLink('Active Node'))).queryByText('Producing')).toBeNull()
    expect(within(cardOf(nodeCardLink('Observing Node'))).queryByText('Observing')).toBeNull()
    expect(within(cardOf(nodeCardLink('Stale Node'))).queryByText('Locked (Stale)')).toBeNull()
  })

  it('marks Healthy green and every other Node grey before the name (issue #141)', () => {
    const unhealthy = { ...network.nodes[0], nodeId: 'node-unhealthy', displayName: 'Unhealthy Node', health: 'unhealthy', healthReason: 'RPC failed' }
    render(<BrowserRouter><HomeDashboard
      networks={[{ ...network, nodes: [network.nodes[0], network.nodes[1], unhealthy] }]}
      realtimeStatus="connected" online resetting={false} error={null} loading={false}
    /></BrowserRouter>)

    const alphaCard = cardOf(nodeCardLink('Alpha'))
    const alphaMarker = within(alphaCard).getByRole('img', { name: 'Healthy' })
    expect(alphaMarker.classList.contains('node-health-marker-healthy')).toBe(true)
    const alphaHeading = within(alphaCard).getByRole('heading', { level: 2, name: 'Alpha' })
    expect(alphaMarker.compareDocumentPosition(alphaHeading) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()

    expect(within(cardOf(nodeCardLink('Beta'))).getByRole('img', { name: 'Unknown' }).classList.contains('node-health-marker-other')).toBe(true)
    expect(within(cardOf(nodeCardLink('Unhealthy Node'))).getByRole('img', { name: 'Unhealthy' }).classList.contains('node-health-marker-other')).toBe(true)

    // No badge remains on the card at all, and no Node Health word is rendered
    // as card text: the marker's accessible name carries it.
    for (const card of [alphaCard, cardOf(nodeCardLink('Beta')), cardOf(nodeCardLink('Unhealthy Node'))]) {
      expect(card.querySelectorAll('.status-badge')).toHaveLength(0)
      expect(within(card).queryByText('Healthy', { exact: true })).toBeNull()
      expect(within(card).queryByText('Unhealthy', { exact: true })).toBeNull()
    }
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
    const cards = document.querySelectorAll('[data-slot="summary-card"]')
    expect(cards).toHaveLength(4)
    for (const card of cards) {
      // exactly one marker icon, one title, one number — no footer text
      expect(card.querySelectorAll('svg')).toHaveLength(1)
      expect(card.querySelectorAll('[data-slot="summary-value"]')).toHaveLength(1)
      expect(card.querySelectorAll('small')).toHaveLength(0)
    }
  })

  it('paints Active Nodes, Healthy Nodes, and Networks with the brand-green marker', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    for (const label of ['Active Nodes', 'Healthy Nodes', 'Networks']) {
      expect(summaryCardOf(label).getAttribute('data-tone')).toBe('green')
    }
  })

  it('reserves the red Attention marker for a non-healthy Active Node', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    // The fixture holds one Healthy and one Unknown Node, so Attention is non-zero.
    expect(summaryCardOf('Attention').getAttribute('data-tone')).toBe('red')
  })

  it('turns the Attention marker green once every Active Node is healthy', () => {
    render(<BrowserRouter><HomeDashboard networks={[{ ...network, nodes: [network.nodes[0]] }]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    expect(summaryCardOf('Attention').getAttribute('data-tone')).toBe('green')
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
      resyncProgress: 'Backfilling 10,000 blocks', peers: { state: 'ok', freshness: 'current', peerCount: 3 },
    }
    render(<BrowserRouter><HomeDashboard networks={[{ ...network, nodes: [network.nodes[1], resyncingNode] }]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    // Unknown Node: the Server-sanitized health reason is the single line.
    const betaCard = cardOf(nodeCardLink('Beta'))
    expect(within(betaCard).getByText('Never observed')).toBeTruthy()
    expect(betaCard.querySelectorAll('[data-slot="node-diagnostic"]')).toHaveLength(1)
    // The unknown peer observation is explicit and never presented as Current.
    expect(within(betaCard).getByText(/No successful Peer snapshot is available/)).toBeTruthy()
    expect(within(betaCard).queryByText('Current observation')).toBeNull()

    // Healthy Node with an active resync: progress is the single line.
    const gammaCard = cardOf(nodeCardLink('Gamma'))
    expect(within(gammaCard).getByText('Backfilling 10,000 blocks')).toBeTruthy()
    expect(within(gammaCard).queryByText('Current observation')).toBeNull()
    expect(gammaCard.querySelectorAll('[data-slot="node-diagnostic"]')).toHaveLength(1)
  })

  it('filters by Network and sorts by supported operational fields', () => {
    const secondNetwork = { ...network, networkKey: 'testnet', displayName: 'Testnet', nodes: [{ ...network.nodes[0], nodeId: 'node-c', displayName: 'Gamma', networkKey: 'testnet', currentHead: 900 }] }
    render(<BrowserRouter><HomeDashboard networks={[network, secondNetwork]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    fireEvent.mouseDown(screen.getByRole('tab', { name: 'Testnet' }))
    expect(nodeCardLink('Gamma')).toBeTruthy()
    expect(screen.queryByRole('link', { name: /Alpha/ })).toBeNull()

    fireEvent.mouseDown(screen.getByRole('tab', { name: 'All Networks' }))
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
    expect(screen.getAllByText('Unknown')).toHaveLength(4)
    expect(screen.queryByText('No Active Nodes in this view.')).toBeNull()
  })

  it('preserves an authoritative empty projection when a refresh fails', () => {
    render(<BrowserRouter><HomeDashboard networks={[]} realtimeStatus="connected" online resetting={false} error="Unable to load Active Nodes" hasLastGood loading={false} /></BrowserRouter>)
    expect(screen.getByText('Unable to load Active Nodes')).toBeTruthy()
    expect(screen.getByText('No Active Nodes in this view.')).toBeTruthy()
    expect(summaryValueOf('Active Nodes').textContent).toBe('0')
  })
})
