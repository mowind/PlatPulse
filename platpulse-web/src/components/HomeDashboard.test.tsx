import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
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
  validatorSummary: {
    blocks: { knownSum: '100', expectedCount: 1, valuedCount: 1, staleCount: 0, state: 'complete' },
    rewards: { knownSum: '10', expectedCount: 1, valuedCount: 1, staleCount: 0, state: 'complete' },
    eligibleValidatorCount: 1,
    linkedNodeCount: 1,
    unlinkedNodeCount: 1,
  },
} satisfies PublicNetwork

/** The title is the one stretched Node Detail link; controls are siblings. */
const nodeCardLink = (name: string) => screen.getByRole('link', { name: new RegExp(name) })
const cardOf = (link: HTMLElement) => link.closest('article') as HTMLElement

/** A compact Linked Validator parameter value, addressed by its full accessible
 *  label even though the card may render the shorter display name. */
const linkedParamValue = (scope: HTMLElement, label: string) => {
  const full = [...scope.querySelectorAll('[data-full-label]')].find(node => node.textContent === label)
  return full?.closest('[data-slot="metric-row"]')?.querySelector('[data-slot="metric-row-value"]')?.textContent
}

/** The summary card carrying the given label (issue #142). */
const summaryCardOf = (label: string) => {
  const card = screen.getByRole('article', { name: label })
  if (!card) throw new Error(`No summary card for ${label}`)
  return card as HTMLElement
}

/** Its value element, which is the number the label announces. */
const summaryValueOf = (label: string) => {
  const value = summaryCardOf(label).querySelector('[data-slot="summary-value"]')
  if (!value) throw new Error(`No summary value for ${label}`)
  return value
}

afterEach(() => { cleanup(); vi.useRealTimers() })

describe('Public Home dashboard', () => {
  it('scopes all six overview counters, map and cards to the selected network', () => {
    const second = { ...network, networkKey: 'testnet', displayName: 'Testnet', nodes: [{ ...network.nodes[0], nodeId: 'gamma', displayName: 'Gamma', networkKey: 'testnet' }] }
    render(<BrowserRouter><HomeDashboard networks={[network, second]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)
    fireEvent.mouseDown(screen.getByRole('tab', { name: 'Testnet' }), { button: 0, ctrlKey: false })
    expect(summaryValueOf('Active Nodes').textContent).toBe('1')
    expect(summaryValueOf('Healthy Nodes').textContent).toBe('1')
    expect(summaryValueOf('Attention').textContent).toBe('0')
    expect(summaryValueOf('Networks').textContent).toBe('1')
    expect(summaryValueOf('Cumulative blocks').textContent).toBe('100')
    expect(summaryValueOf('Cumulative rewards').textContent).toBe('10')
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
    expect(nodeDataMetric.parentElement?.classList.contains('col-span-2')).toBe(true)
    expect(resources.className).toContain('grid-cols-2')
    expect(nodeDataMetric.querySelector('[data-slot="progress-thin"]')).toBeTruthy()
    expect(nodeDataMetric.querySelector('[role="progressbar"]')?.getAttribute('aria-valuenow')).toBe('25')
    expect(within(resources).queryByText('STORAGE')).toBeNull()
    expect(within(resources).getByText('16.4Kbps')).toBeTruthy()
    expect(within(resources).getByLabelText('Upload 16.4Kbps').querySelector('svg')).not.toBeNull()
    expect(within(resources).getByText('8.19Kbps')).toBeTruthy()
  })
  it('places Network and uptime in the second identity line while host speed stays full width', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)
    const card = cardOf(nodeCardLink('Alpha'))
    const resources = within(card).getByLabelText('Node process and host network resources')
    const speed = within(resources).getByRole('group', { name: 'Host network speed' })
    expect(speed.parentElement).toBe(resources)
    expect(speed.classList.contains('col-span-2')).toBe(true)
    const uptime = within(card).getByText('Uptime Unknown')
    const identity = uptime.closest('p')
    expect(identity?.textContent).toBe('Mainnet · Uptime Unknown')
    expect(resources.contains(uptime)).toBe(false)
    expect(identity).not.toBeNull()
    expect(identity!.compareDocumentPosition(resources) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(within(resources).queryByRole('group', { name: 'Node uptime' })).toBeNull()
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
    expect(within(cardOf(nodeCardLink('Alpha'))).getByText(`Uptime ${expected}`).textContent).toBe(`Uptime ${expected}`)
  })

  it('renders Node data as a percentage with the used / total byte detail under its progress bar', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    const alphaCard = cardOf(nodeCardLink('Alpha'))
    const nodeDataMetric = within(alphaCard).getByText('Node data').parentElement as HTMLElement
    expect(within(nodeDataMetric).getByText('25.0%')).toBeTruthy()
    expect(within(nodeDataMetric).getByText('12.0 GiB / 48.0 GiB')).toBeTruthy()
    expect(nodeDataMetric.querySelector('[role="progressbar"]')?.getAttribute('aria-valuenow')).toBe('25')
  })

  it('uses compact key-value rows for every resource, chain height and count', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    const alphaCard = cardOf(nodeCardLink('Alpha'))
    // Uptime is identity metadata, not a resource row. Resources (4), chain
    // heights (4), and Txs/Peers (2) retain their independent observations, and
    // every ordinary parameter uses the same left-label/right-value line.
    expect(alphaCard.querySelectorAll('[data-slot="metric-row"]')).toHaveLength(10)
    expect(alphaCard.querySelectorAll('[data-slot="metric-row"][data-layout="compact"]')).toHaveLength(10)
    expect(alphaCard.querySelectorAll('[data-slot="metric-row"][data-layout="inline"]')).toHaveLength(0)
  })

  it('orders Head/QC/Locked/Committed as compact rows before Txs and Peers and keeps the PlatScan status in the header', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)
    const card = cardOf(nodeCardLink('Alpha'))
    expect(card.querySelector('[data-slot="metric-triple"]')).toBeNull()
    const metrics = card.querySelector('[data-slot="node-business-metrics"]')!
    expect(Array.from(metrics.querySelectorAll('[data-slot="metric-row-label"]'), el => el.textContent))
      .toEqual(['Head', 'QC', 'Locked', 'Committed', 'Txs', 'Peers'])
    // The two-column/full-width switch is a card-width container rule keyed on
    // the group's data-wide marker: no hard-coded class, no viewport breakpoint.
    expect(metrics.getAttribute('data-wide')).toBeNull()
    for (const label of ['Head', 'QC', 'Locked', 'Committed']) {
      expect(within(metrics as HTMLElement).getByText(label).parentElement?.getAttribute('data-layout')).toBe('compact')
    }
    const counts = metrics.querySelector('[data-slot="node-counts"]')!
    expect(Array.from(counts.querySelectorAll('[data-slot="metric-row-label"]'), el => el.textContent)).toEqual(['Txs', 'Peers'])
    expect(counts.querySelectorAll('[data-layout="compact"]')).toHaveLength(2)
    expect(metrics.querySelector('[data-short-label]')).toBeNull()
    // The PlatScan status belongs to the identity row, never to the metric grid.
    expect(metrics.querySelector('[data-slot="validator-activity"]')).toBeNull()
    // Node alpha has no effective Node Validator Link, so its Activity is the
    // unified Observing state rather than a named production status.
    expect(card.querySelector('[data-slot="card-x-header"] [data-slot="validator-activity"]')?.getAttribute('data-activity')).toBe('observing')
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
    // Consensus membership no longer drives the header chip: the PlatScan
    // Activity badge is an independent dimension.
    expect(within(alphaCard).getByLabelText(/^PlatScan status: Observing/)).toBeTruthy()
    expect(within(alphaCard).queryByText('Stale')).toBeNull()

    // Never-observed consensus is Unknown for every metric, never zero/No.
    const betaCard = cardOf(nodeCardLink('Beta'))
    for (const label of ['QC', 'Locked', 'Committed']) {
      expect(within(betaCard).getByText(label)).toBeTruthy()
    }
    expect(within(betaCard).getAllByText('Unknown').length).toBeGreaterThanOrEqual(7)
    expect(within(betaCard).getByLabelText(/^PlatScan status: Observing/)).toBeTruthy()
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
    // QC/Locked/Committed carry the retained-row Stale text; the Activity badge
    // marks its own staleness with a mark plus an accessible name, not a word.
    expect(within(staleCard).getAllByText('Stale')).toHaveLength(3)

    // A failed collection with last-good true keeps the value and is Stale.
    const failedCard = cardOf(nodeCardLink('Failed True'))
    expect(within(failedCard).getByText('151')).toBeTruthy()
    expect(within(failedCard).getByText('150')).toBeTruthy()
    expect(within(failedCard).getByText('149')).toBeTruthy()
    expect(within(failedCard).getAllByText('Stale')).toHaveLength(3)

    // A stale successful non-membership keeps No and marks it Stale.
    const staleFalseCard = cardOf(nodeCardLink('Stale False'))
    expect(within(staleFalseCard).getByText('161')).toBeTruthy()
    expect(within(staleFalseCard).getAllByText('Stale')).toHaveLength(3)

    // A failed collection without a last-good membership is Unknown, never
    // No, and is not dressed up as Stale with no retained value.
    const failedNoneCard = cardOf(nodeCardLink('Failed None'))
    expect(within(failedNoneCard).getAllByText('Unknown').length).toBeGreaterThanOrEqual(4)
    expect(within(failedNoneCard).queryByText('Stale')).toBeNull()

    // A current successful non-membership renders No; an observed zero
    // block height is an authoritative zero, never Unknown.
    const falseCard = cardOf(nodeCardLink('Current False'))
    expect(within(falseCard).getAllByText('0').length).toBeGreaterThanOrEqual(4)
    expect(within(falseCard).queryByText('Stale')).toBeNull()

    // Unknown freshness means currency cannot be certified: the retained
    // value must not be presented as current Yes/No or block heights.
    const unknownFreshnessCard = cardOf(nodeCardLink('Unknown Freshness'))
    expect(within(unknownFreshnessCard).getAllByText('Unknown').length).toBeGreaterThanOrEqual(3)
    expect(within(unknownFreshnessCard).queryByText('Stale')).toBeNull()
  })

  it('shows the PlatScan Activity badge beside the independent Node Health marker', () => {
    const linked = (activity: string, activityState: string) => ({
      validatorId: 'validator-a', validatorNodeId: '0xvalidator', displayName: 'Validator A',
      nodeId: 'node-a', state: activityState === 'stale' ? 'error' : 'fresh',
      freshness: 'fresh', source: 'platscan',
      receivedAt: '2026-08-25T00:00:00Z', rankState: 'unknown', rankFreshness: 'unknown', blockRateState: 'unknown', counterState: 'normal', activity, activityState,
      currentValidatorStatus: 'validator', currentValidatorStatusState: 'current', currentValidatorStatusQualifier: null,
    })
    // Producing is deliberately seeded test data: the captured PlatScan detail
    // responses have not presented status 3, so only labelled data reaches it.
    const cases = [
      { name: 'Active Node', validator: linked('active', 'current'), activity: 'active', label: 'Active' },
      { name: 'Producing Node', validator: linked('producing', 'current'), activity: 'producing', label: 'Producing' },
      { name: 'Verifying Node', validator: linked('verifying', 'current'), activity: 'verifying', label: 'Verifying' },
      { name: 'Observing Node', validator: linked('observing', 'current'), activity: 'observing', label: 'Observing' },
      { name: 'Unlinked Node', validator: null, activity: 'observing', label: 'Observing' },
    ]
    const nodes = cases.map(({ name, validator }, index) => ({
      ...network.nodes[0], nodeId: `node-activity-${index}`, displayName: name, validator,
    }))
    render(<BrowserRouter><HomeDashboard
      networks={[{ ...network, nodes }]}
      realtimeStatus="connected" online resetting={false} error={null} loading={false}
    /></BrowserRouter>)

    // The status is the Node's own Validator Activity projection; the
    // two-state Node Health marker beside the name stays an independent cue.
    for (const { name, activity, label } of cases) {
      const card = cardOf(nodeCardLink(name))
      const badge = card.querySelector('[data-slot="validator-activity"]') as HTMLElement
      expect(badge.getAttribute('data-activity')).toBe(activity)
      expect(badge.querySelector('[data-slot="validator-activity-label"]')?.textContent).toBe(label)
      expect(within(card).getByRole('img', { name: 'Healthy' })).toBeTruthy()
    }
    // The local Node role is no longer repeated in the card header.
    expect(document.querySelector('[data-slot="validator-role"]')).toBeNull()
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

    // The health marker is the only header status cue besides the independent
    // PlatScan Activity badge; CONTEXT.md's rule holds: the reason for an
    // abnormal Node Health state stays visible as text while a Healthy Node has
    // no diagnostic placeholder.
    for (const card of [alphaCard, cardOf(nodeCardLink('Beta')), cardOf(nodeCardLink('Unhealthy Node'))]) {
      expect(card.querySelectorAll('[data-slot="status-badge"]')).toHaveLength(0)
      expect(card.querySelectorAll('[data-slot="node-diagnostic"]')).toHaveLength(card === alphaCard ? 0 : 1)
    }
    expect(within(alphaCard).queryByText('RPC reachable')).toBeNull()
    expect(within(cardOf(nodeCardLink('Beta'))).getByText('Never observed')).toBeTruthy()
    expect(within(cardOf(nodeCardLink('Unhealthy Node'))).getByText('RPC failed')).toBeTruthy()
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
    expect(cards).toHaveLength(6)
    for (const card of cards) {
      // All six use one marker in the same header position and one value; on
      // the cumulative cards that marker is the Breakdown control itself.
      expect(card.querySelectorAll('svg[data-icon]')).toHaveLength(1)
      expect(card.querySelectorAll('[data-slot="summary-value"]')).toHaveLength(1)
      expect(card.querySelectorAll('small')).toHaveLength(0)
    }
  })

  it('orders all six overview cards together and removes the standalone Validator summary', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)
    const summary = screen.getByLabelText('Home summary')
    const cards = within(summary).getAllByRole('article')
    expect(cards.map(card => card.getAttribute('aria-label'))).toEqual([
      'Active Nodes', 'Healthy Nodes', 'Cumulative blocks', 'Attention', 'Networks', 'Cumulative rewards',
    ])
    expect(cards.map(card => card.querySelector('[data-slot="summary-value"]')?.textContent)).toEqual(['2', '1', '100', '1', '1', '10'])
    expect(summary.className).toContain('grid-cols-2')
    expect(summary.className).toContain('sm:grid-cols-3')
    expect(summary.className).toContain('auto-rows-fr')
    expect(screen.queryByRole('region', { name: 'Current-selection Validator summary' })).toBeNull()
    expect(document.querySelector('[data-slot="validator-totals"]')).toBeNull()
    expect(document.querySelector('[data-slot="validator-summary-network"]')).toBeNull()
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(within(summary).getByRole('button', { name: 'Cumulative blocks breakdown and exact values' })).toBeTruthy()
    expect(within(summary).getByRole('button', { name: 'Cumulative rewards breakdown and exact values' })).toBeTruthy()
  })

  it('opens full long Node identity without navigating and restores focus on close', async () => {
    const name = 'Long PlatON Node identity with an intentionally descriptive deployment name '.repeat(3).trim()
    const networkName = 'Network with a deliberately long public display name'
    const nodes = [{ ...network.nodes[0], displayName: name, processUptimeMs: 183_600_000 }]
    render(<BrowserRouter><HomeDashboard networks={[{ ...network, displayName: networkName, nodes }]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)
    const title = screen.getByRole('link', { name })
    expect(title.textContent).toBe(name)
    expect(title.getAttribute('title')).toBe(name)
    const card = cardOf(title)
    const trigger = within(card).getByRole('button', { name: 'Node identity details' })
    expect(trigger.closest('a')).toBeNull()
    expect(trigger.className).toContain('relative z-10')
    const before = window.location.href
    fireEvent.click(trigger)
    const dialog = screen.getByRole('dialog', { name })
    expect(window.location.href).toBe(before)
    expect(within(dialog).getByRole('heading', { name }).textContent).toBe(name)
    expect(within(dialog).getByText(/Network:/).textContent).toContain('Network: ' + networkName + ' · Uptime 2d 3h.')
    expect(within(dialog).getByText(/Node role describes/).textContent).toContain('not its linked Validator’s current staking validity or the freshness of Provider data')
    fireEvent.click(within(dialog).getByRole('button', { name: 'Close' }))
    await waitFor(() => expect(document.activeElement).toBe(trigger))
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(window.location.href).toBe(before)
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

  it('renders one stretched title link with Network and controls outside the anchor', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    // One semantic link per Node card, named by the Node title alone.
    const alphaLink = nodeCardLink('Alpha')
    expect(alphaLink.getAttribute('href')).toBe('/nodes/node-a')
    const alphaCard = cardOf(alphaLink)
    expect(alphaCard.querySelectorAll('a')).toHaveLength(1)
    expect(alphaLink.textContent).toBe('Alpha')
    expect(alphaLink.closest('h2')).not.toBeNull()
    expect(alphaLink.className).toContain('after:inset-0')
    expect(alphaLink.querySelector('button')).toBeNull()
    expect(within(alphaCard).getByRole('button', { name: 'Node identity details' }).closest('a')).toBeNull()
    // Network metadata is plain text outside the title's stretched link.
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

  it('keeps health diagnostics separate from structured resync progress', () => {
    const resyncingNode = {
      ...network.nodes[0],
      nodeId: 'node-c', displayName: 'Gamma', health: 'healthy', resyncState: 'resyncing',
      currentHead: 6_920_136, historicalHighWatermark: 159_311_799,
      resyncLastProgressAt: '2026-08-25T00:00:00Z',
      resyncProgress: '6920136/159311799 (last progress 2026-08-25T00:00:00Z)', peers: { state: 'ok', freshness: 'current', peerCount: 3 },
    }
    render(<BrowserRouter><HomeDashboard networks={[{ ...network, nodes: [network.nodes[1], resyncingNode] }]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    // Unknown Node: the Server-sanitized health reason stays reachable from the
    // single short status chip instead of a full diagnostic line.
    const betaCard = cardOf(nodeCardLink('Beta'))
    expect(within(betaCard).getByText('Never observed')).toBeTruthy()
    expect(betaCard.querySelectorAll('[data-slot="node-diagnostic"]')).toHaveLength(1)
    // The unknown peer observation is explicit and never presented as Current.
    expect(within(betaCard).getByText(/No successful Peer snapshot is available/)).toBeTruthy()
    expect(within(betaCard).queryByText('Current observation')).toBeNull()

    // Gamma keeps the live Resyncing percentage directly visible in the same
    // identity status row, without the removed three-line sync block.
    const gammaCard = cardOf(nodeCardLink('Gamma'))
    const progress = within(gammaCard).getByRole('button', { name: 'Resync progress: 4.34% toward the Historical High-Water Mark' })
    expect(within(progress).getByText('Resyncing')).toBeTruthy()
    expect(within(progress).getByText('4.34%')).toBeTruthy()
    expect(within(gammaCard).queryByText('6,920,136 / 159,311,799')).toBeNull()
    expect(within(gammaCard).getByRole('img', { name: 'Healthy' })).toBeTruthy()
    expect(summaryValueOf('Healthy Nodes').textContent).toBe('1')
    expect(summaryValueOf('Attention').textContent).toBe('1')
    fireEvent.click(progress)
    const dialog = screen.getByRole('dialog', { name: 'Resync progress' })
    expect(within(dialog).getByText('6,920,136 / 159,311,799')).toBeTruthy()
    expect(within(dialog).getByText(/Last progress/)).toBeTruthy()
    expect(dialog.textContent).toContain('not the latest report time')
  })

  it.each([
    { currentHead: 0, historicalHighWatermark: 100, height: '0 / 100', percent: '0.00%' },
    { currentHead: 0, historicalHighWatermark: 0, height: '0 / 0', percent: 'Unknown' },
    { currentHead: 50, historicalHighWatermark: null, height: '50 / Unknown', percent: 'Unknown' },
    { currentHead: 50, historicalHighWatermark: undefined, height: '50 / Unknown', percent: 'Unknown' },
    { currentHead: null, historicalHighWatermark: 100, height: 'Unknown / 100', percent: 'Unknown' },
    { currentHead: undefined, historicalHighWatermark: 100, height: 'Unknown / 100', percent: 'Unknown' },
    { currentHead: NaN, historicalHighWatermark: 100, height: 'Unknown / 100', percent: 'Unknown' },
    { currentHead: Infinity, historicalHighWatermark: 100, height: 'Unknown / 100', percent: 'Unknown' },
    { currentHead: 50, historicalHighWatermark: Infinity, height: '50 / Unknown', percent: 'Unknown' },
    { currentHead: -1, historicalHighWatermark: 100, height: 'Unknown / 100', percent: 'Unknown' },
    { currentHead: 50, historicalHighWatermark: -1, height: '50 / Unknown', percent: 'Unknown' },
    { currentHead: 0.5, historicalHighWatermark: 100, height: 'Unknown / 100', percent: 'Unknown' },
    { currentHead: 100, historicalHighWatermark: 100, height: '100 / 100', percent: '100.00%' },
  ])('renders resync raw heights safely: $height ($percent)', ({ currentHead, historicalHighWatermark, height, percent }) => {
    const node = { ...network.nodes[0], resyncState: 'resyncing', currentHead, historicalHighWatermark }
    render(<BrowserRouter><HomeDashboard networks={[{ ...network, nodes: [node] }]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)
    const progress = screen.getByRole('button', { name: /Resync progress: .* toward the Historical High-Water Mark/ })
    expect(progress.textContent).toContain(percent)
    expect(progress.textContent).not.toMatch(/NaN|Infinity/)
    fireEvent.click(progress)
    expect(within(screen.getByRole('dialog', { name: 'Resync progress' })).getByText(height)).toBeTruthy()
  })

  it.each([null, undefined, '', 'invalid-time', '2026-02-30T00:00:00Z', '2026-08-25T00:00:00', '2026-08-25T24:00:00Z'])('keeps missing or invalid progress time Unknown: %s', timestamp => {
    const node = { ...network.nodes[0], resyncState: 'resyncing', resyncLastProgressAt: timestamp,
      lastReportAt: new Date().toISOString(), processUptimeMs: 0,
      resyncProgress: '120/120 (last progress 2026-08-25T00:00:00Z)' }
    render(<BrowserRouter><HomeDashboard networks={[{ ...network, nodes: [node] }]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)
    const progress = screen.getByRole('button', { name: /Resync progress: .* toward the Historical High-Water Mark/ })
    expect(progress.textContent).not.toMatch(/\bnow\b|\bago\b|2026|ETA/)
    fireEvent.click(progress)
    const dialog = screen.getByRole('dialog', { name: 'Resync progress' })
    expect(within(dialog).getByText('Last progress: Unknown')).toBeTruthy()
    expect(dialog.textContent).not.toMatch(/\bnow\b|\bago\b|2026|ETA/)
  })

  it('keeps resync independent of unhealthy diagnostics and trusts the completed Server state', () => {
    const node = { ...network.nodes[0], resyncState: 'resyncing', health: 'unhealthy', healthReason: 'RPC observation failed',
      currentHead: 100, historicalHighWatermark: 100,
      consensus: { ...network.nodes[0].consensus, highestQcBlock: 0, highestLockBlock: 0, highestCommitBlock: 0 } }
    const view = (resyncState: string) => <BrowserRouter><HomeDashboard networks={[{ ...network, nodes: [{ ...node, resyncState }] }]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>
    const { rerender } = render(view('resyncing'))
    expect(screen.getByRole('button', { name: /Resync progress:/ })).toBeTruthy()
    expect(screen.getByText('RPC observation failed')).toBeTruthy()
    for (const label of ['QC', 'Locked', 'Committed']) {
      expect(screen.getByText(label).nextElementSibling?.textContent).toBe('0')
    }
    expect(summaryValueOf('Healthy Nodes').textContent).toBe('0')
    expect(summaryValueOf('Attention').textContent).toBe('1')
    rerender(view('normal'))
    expect(screen.queryByRole('button', { name: /Resync progress:/ })).toBeNull()
    expect(screen.getByText('RPC observation failed')).toBeTruthy()
  })

  it('explains health and Home Attention without equating health with completed synchronization', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)
    fireEvent.click(within(cardOf(nodeCardLink('Alpha'))).getByRole('button', { name: 'Node identity details' }))
    const dialog = screen.getByRole('dialog', { name: 'Alpha' })
    expect(dialog.textContent).toContain('Healthy does not mean synchronization is complete')
    expect(dialog.textContent).toContain('Attention counts Active Nodes that are not Healthy, including Unknown')
  })

  it('updates the real progress age without new reports and exposes its full UTC time', () => {
    vi.useFakeTimers()
    vi.setSystemTime(new Date('2026-08-25T00:02:00Z'))
    const node = { ...network.nodes[0], resyncState: 'resyncing', resyncLastProgressAt: '2026-08-25T02:00:00.123456+02:00' }
    render(<BrowserRouter><HomeDashboard networks={[{ ...network, nodes: [node] }]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)
    fireEvent.click(screen.getByRole('button', { name: /Resync progress:/ }))
    const dialog = screen.getByRole('dialog', { name: 'Resync progress' })
    expect(within(dialog).getByText(/2 minutes ago/)).toBeTruthy()
    act(() => { vi.advanceTimersByTime(60_000) })
    expect(within(dialog).getByText(/3 minutes ago/)).toBeTruthy()
    expect(within(dialog).getByText(/25 Aug 2026, 00:00:00 UTC/)).toBeTruthy()
    expect(within(dialog).getByText('2026-08-25T02:00:00.123456+02:00')).toBeTruthy()
    expect(dialog.textContent).toContain('not the latest report time')
  })

  it('uses the real current time when a new progress observation arrives between clock ticks', () => {
    vi.useFakeTimers()
    vi.setSystemTime(new Date('2026-08-25T00:02:00Z'))
    const view = (timestamp: string) => <BrowserRouter><HomeDashboard networks={[{ ...network, nodes: [{ ...network.nodes[0], resyncState: 'resyncing', resyncLastProgressAt: timestamp }] }]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>
    const { rerender } = render(view('2026-08-25T00:00:00Z'))
    vi.setSystemTime(new Date('2026-08-25T00:02:10Z'))
    rerender(view('2026-08-25T00:02:09Z'))
    fireEvent.click(screen.getByRole('button', { name: /Resync progress:/ }))
    expect(within(screen.getByRole('dialog', { name: 'Resync progress' })).getByText(/1 second ago/)).toBeTruthy()
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
    expect(screen.getAllByText('Unknown')).toHaveLength(6)
    expect(screen.queryByText('No Active Nodes in this view.')).toBeNull()
  })

  it('preserves an authoritative empty projection when a refresh fails', () => {
    render(<BrowserRouter><HomeDashboard networks={[]} realtimeStatus="connected" online resetting={false} error="Unable to load Active Nodes" hasLastGood loading={false} /></BrowserRouter>)
    expect(screen.getByText('Unable to load Active Nodes')).toBeTruthy()
    expect(screen.getByText('No Active Nodes in this view.')).toBeTruthy()
    expect(summaryValueOf('Active Nodes').textContent).toBe('0')
  })

  it('shows the linked Validator cumulative block count, rates, and status on the Home card', () => {
    const linked = {
      validatorId: 'validator-linked', validatorNodeId: '0xlinked', displayName: 'Validator A',
      nodeId: 'node-linked', state: 'fresh', freshness: 'fresh', source: 'platscan',
      providerTimestamp: '2026-08-25T00:00:00Z', receivedAt: '2026-08-25T00:00:05Z',
      blockCount: 123456, expectedBlockCount: 110, blockRate: '90.909091', blockRateState: 'ok',
      rank: 5, rankState: 'ranked', rankFreshness: 'fresh', rankCohortSize: 300,
      genBlocksRate: '0', delegationRewardPercentage: '20', counterState: 'normal', activity: 'producing', activityState: 'current',
      currentValidatorStatus: 'validator', currentValidatorStatusState: 'current', currentValidatorStatusQualifier: null,
    }
    const linkedNode = { ...network.nodes[0], nodeId: 'node-linked', displayName: 'Calico', validator: linked }
    const unlinkedNode = { ...network.nodes[0], nodeId: 'node-unlinked', displayName: 'Domino', validator: null }
    render(<BrowserRouter><HomeDashboard networks={[{ ...network, nodes: [linkedNode, unlinkedNode] }]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    const card = cardOf(nodeCardLink('Calico'))
    // The removed association area leaves no heading, identity, identifier,
    // copy control, Details row or long explanation on the Home card.
    expect(within(card).queryByText('Linked Validator')).toBeNull()
    expect(within(card).queryByText('Validator A')).toBeNull()
    expect(within(card).queryByRole('button', { name: 'Copy full Validator identifier' })).toBeNull()
    expect(within(card).queryByRole('button', { name: 'Open Validator details' })).toBeNull()
    expect(within(card).queryByText('Data: Current')).toBeNull()
    const linkedSection = card.querySelector('[data-slot="linked-validator"]') as HTMLElement
    expect(linkedSection).not.toBeNull()
    expect(linkedSection.getAttribute('aria-label')).toBe('Linked Validator')
    // The six metrics stay directly visible without hover or expansion, and a
    // source 0% stays a value.
    expect(within(card).getByText('Cumulative blocks')).toBeTruthy()
    expect(within(card).getByText('123,456')).toBeTruthy()
    expect(linkedParamValue(card, 'Production rate')).toBe('90.91%')
    expect(linkedParamValue(card, 'PlatScan 24h rate')).toBe('0.00%')
    expect(linkedParamValue(card, 'Delegation reward share')).toBe('20.00%')
    const titleLink = nodeCardLink('Calico')
    expect(titleLink.querySelector('button, input, textarea, a')).toBeNull()
    const control = within(card).getByRole('button', { name: 'Node identity details' })
    expect(control.closest('a')).toBeNull()
    expect(control.className).toContain('relative z-10')

    const unlinkedCard = cardOf(nodeCardLink('Domino'))
    expect(within(unlinkedCard).getByText('Validator identity has not been observed yet.')).toBeTruthy()
    expect(within(unlinkedCard).queryByText('Cumulative blocks')).toBeNull()
    expect(within(unlinkedCard).queryByText('Linked Validator')).toBeNull()
  })
})
