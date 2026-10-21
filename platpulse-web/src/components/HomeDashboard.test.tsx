import { useState } from 'react'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { BrowserRouter, createMemoryRouter, RouterProvider } from 'react-router'
import type { PublicNetwork, PublicNode } from '../api/generated'
import { homeReturnState, homeReturnTab, type HomeReturnState } from '../homeReturn'
import HomeDashboard, { RATE_DIRECTION_CLASSES } from './HomeDashboard'
import type { RewardWalletReading } from './RewardWalletCard'

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
      peerCountries: { scope: 'unavailable', countries: [] },
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
      peerCountries: { scope: 'unavailable', countries: [] },
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

/** A current Reward Wallet Balance (#246), the fixture every Home render reads
 *  unless a test asks for another state. */
const rewardWallet: RewardWalletReading = {
  status: 'ready',
  wallet: {
    address: 'lat1zyctwgvw3ssagn722cneh6sz92y8j3dp8gqj5h',
    balance: '138179.543005858536',
    unit: 'LAT',
    state: 'current',
    lastGoodAt: new Date().toISOString(),
    lastAttemptAt: new Date().toISOString(),
    errorCategory: null,
  },
}

/** The title is the one stretched Node Detail link; controls are siblings. */
const nodeCardLink = (name: string) => screen.getByRole('link', { name: new RegExp(name) })
const cardOf = (link: HTMLElement) => link.closest('article') as HTMLElement

/** A value in the Linked Validator summary, addressed by its full accessible
 *  label even though the compact cell renders the shorter display name. */
const validatorSummaryValue = (scope: HTMLElement, label: string) => {
  const full = [...scope.querySelectorAll('[data-slot="validator-summary-label"] [data-full-label]')].find(node => node.textContent === label)
  return full?.closest('[data-slot="validator-summary-cell"]')?.querySelector('[data-slot="validator-summary-value"]')?.textContent
}

/** The summary card carrying the given label (issue #142). */
const summaryCardOf = (label: string) => {
  // The Balance tile's accessible name carries the exact amount and address
  // after its label (#246); every other tile is named by its label alone.
  const card = screen.getByRole('article', { name: label === 'Balance' ? /^Balance:/ : label })
  if (!card) throw new Error(`No summary card for ${label}`)
  return card as HTMLElement
}

/** Its value element, which is the number the label announces. */
const summaryValueOf = (label: string) => {
  const value = summaryCardOf(label).querySelector('[data-slot="summary-value"]')
  if (!value) throw new Error(`No summary value for ${label}`)
  return value
}

/** Home in its ordinary shell: the projection, the realtime status and the
 *  session states a reader sees, against the URL the test set up beforehand. */
function renderHome(networks: PublicNetwork[], options: { loading?: boolean; rewardWallet?: RewardWalletReading } = {}) {
  return render(
    <BrowserRouter>
      <HomeDashboard
        networks={networks}
        realtimeStatus="connected"
        online
        resetting={false}
        error={null}
        loading={options.loading ?? false}
        rewardWallet={options.rewardWallet ?? rewardWallet}
      />
    </BrowserRouter>,
  )
}

/** Issue #232: the Sort & filter entry holds the three select controls behind
 *  its own toolbar surface, so a test opens that surface before reading or
 *  changing one of them. Opening is guarded, because a second click on the
 *  trigger would close the surface again. A click dispatches no pointerdown,
 *  so the open surface stays open for the rest of the test. */
const openFilterPanel = () => {
  if (!screen.queryByRole('combobox', { name: 'Sort' })) {
    fireEvent.click(screen.getByRole('button', { name: /^Sort & filter/ }))
  }
}

const selectNamed = (name: string) => {
  openFilterPanel()
  return screen.getByRole('combobox', { name }) as HTMLSelectElement
}

/** The compact list read the way a reader reads it: the named table, its own
 *  rows, and the column each public header names. Only the scroller container,
 *  which carries no role, is addressed by its shell marker. */
const listTable = () => screen.queryByRole('table', { name: 'Active Nodes' })
const listRows = () => within(listTable()!).getAllByRole('row')
  .filter((row) => within(row).queryAllByRole('cell').length > 0)
const columnLabels = () => within(listTable()!).getAllByRole('columnheader').map((header) => header.textContent ?? '')
const cellOf = (row: HTMLElement, column: string) => {
  const index = columnLabels().indexOf(column)
  return index < 0 ? undefined : within(row).getAllByRole('cell')[index].textContent
}
const rowOf = (name: string) => listRows().find((row) => cellOf(row, 'Node') === name)
const listedNames = () => listRows().map((row) => cellOf(row, 'Node'))

// Home owns its filter state in the URL, so a test that navigates must not
// leak that query string into the next one.
afterEach(() => {
  cleanup()
  vi.useRealTimers()
  window.history.replaceState({}, '', '/')
})

describe('Public Home dashboard', () => {
  it('scopes all six overview counters, map and cards to the selected network', () => {
    const second = { ...network, networkKey: 'testnet', displayName: 'Testnet', nodes: [{ ...network.nodes[0], nodeId: 'gamma', displayName: 'Gamma', networkKey: 'testnet' }] }
    render(<BrowserRouter><HomeDashboard networks={[network, second]} realtimeStatus="connected" online resetting={false} error={null} loading={false} rewardWallet={rewardWallet} /></BrowserRouter>)
    expect(summaryValueOf('Balance').textContent).toBe('138,179.54 LAT')
    fireEvent.mouseDown(screen.getByRole('tab', { name: 'Testnet' }), { button: 0, ctrlKey: false })
    expect(summaryValueOf('Active Nodes').textContent).toBe('1')
    expect(summaryValueOf('Healthy Nodes').textContent).toBe('1')
    expect(summaryValueOf('Attention').textContent).toBe('0')
    // The Reward Wallet Balance follows no Network selection.
    expect(summaryValueOf('Balance').textContent).toBe('138,179.54 LAT')
    expect(screen.queryByRole('article', { name: 'Networks' })).toBeNull()
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
    // The count reads on the secondary identity line with the one cue an
    // authoritative empty snapshot needs; it is never a bare zero or Unknown.
    expect(within(alphaCard).getByText('Peers 0')).toBeTruthy()
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
      expect(within(card).getByText('Peers 0')).toBeTruthy()
      expect(within(card).getByText(/^last good \(/)).toBeTruthy()
    }
    const failedCard = cardOf(nodeCardLink('Retained error'))
    expect(within(failedCard).getByText('last good (collection error, freshness stale)')).toBeTruthy()
    // The card states the cue in short form; the identity entry keeps the whole
    // collection/freshness/value sentence, so compacting it loses nothing.
    fireEvent.click(within(failedCard).getByRole('button', { name: 'Node identity details' }))
    const dialog = screen.getByRole('dialog', { name: 'Retained error' })
    expect(dialog.textContent).toContain('Collection failed')
    expect(dialog.textContent).toContain('Showing last successful snapshot')
  })

  it('keeps the Home chain block to the current Block Height and reads Peers on the second line', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    const alphaCard = cardOf(nodeCardLink('Alpha'))
    const chain = alphaCard.querySelector('[data-slot="node-business-metrics"]') as HTMLElement
    expect(Array.from(chain.querySelectorAll('[data-slot="metric-row-label"]'), el => el.textContent)).toEqual(['Head'])
    expect(within(chain).getByText('120')).toBeTruthy()
    // Peers is metadata now, not a chain row: the same count reads on the
    // secondary identity line and never inside the chain block.
    expect(within(alphaCard).getByText('Peers 0')).toBeTruthy()
    expect(chain.contains(within(alphaCard).getByText('Peers 0'))).toBe(false)
    // QC, Locked, Committed and Txs stay Node Detail statements: the card neither
    // shows them nor restates a raw transaction count.
    for (const label of ['QC', 'Locked', 'Committed', 'Txs']) {
      expect(within(alphaCard).queryByText(label, { exact: true })).toBeNull()
    }
    expect(within(alphaCard).queryByText('12,345')).toBeNull()
    for (const legacyLabel of ['HEAD', 'TXS', 'PEERS', 'MEMORY', 'NODE DATA', 'LOCKED', 'COMMITTED', 'VALIDATOR']) {
      expect(within(alphaCard).queryByText(legacyLabel, { exact: true })).toBeNull()
    }
    // A never-observed height stays Unknown, never zero (issue #98), and an
    // unattested Peer count says why rather than implying a zero set.
    const betaCard = cardOf(nodeCardLink('Beta'))
    expect(within(betaCard).getByText('Head')).toBeTruthy()
    expect(within(betaCard).getByText('Peers Unknown')).toBeTruthy()
    expect(within(betaCard).getByText('no successful snapshot')).toBeTruthy()
    expect(within(betaCard).getAllByText('Unknown').length).toBeGreaterThanOrEqual(4)
  })

  it('reads CPU, Memory and Disk as three resource columns from their own fields', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    const alphaCard = cardOf(nodeCardLink('Alpha'))
    const resources = within(alphaCard).getByLabelText('Node process and host network resources')
    const highlights = alphaCard.querySelector('[data-slot="node-business-metrics"]')!
    expect(resources.compareDocumentPosition(highlights) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    const cells = [...resources.querySelectorAll('[data-slot="resource-cell"]')]
    expect(cells.map(cell => cell.getAttribute('data-metric'))).toEqual(['process-cpu', 'process-memory', 'node-data'])
    // The longest label is shortened so its percentage keeps sharing the label's
    // line at the card's own minimum track; the full process name stays the
    // label's title and the bar's accessible name (#237).
    expect(cells.map(cell => cell.querySelector('[data-slot="resource-label"]')?.textContent)).toEqual(['CPU', 'Mem', 'Disk'])
    expect(cells.map(cell => cell.querySelector('[data-slot="resource-label"]')?.getAttribute('title')))
      .toEqual(['Process CPU', 'Process memory', 'Disk: Node data directory size as a share of the filesystem capacity holding it, not whole-filesystem fullness'])
    expect(cells.map(cell => cell.querySelector('[data-slot="resource-value"]')?.textContent)).toEqual(['12.5%', '45.3%', '25.0%'])
    // Every column keeps one thin Emerald bar underneath its label/value line.
    expect(within(resources).getAllByRole('progressbar').map(bar => bar.getAttribute('aria-valuenow')))
      .toEqual(['13', '45', '25'])
    // The Disk bar names the Node data directory it measures, not whole-disk
    // fullness, and the Host storage percentage (80) is a different observation
    // that never becomes the Disk reading.
    expect(within(resources).getByRole('progressbar', { name: /^Disk: Node data directory size as a share of the filesystem capacity/ })).toBeTruthy()
    expect(within(resources).queryByText('80.0%')).toBeNull()
    expect(within(resources).queryByText('STORAGE')).toBeNull()
    expect(within(resources).getByText('16.4Kbps')).toBeTruthy()
    expect(within(resources).getByLabelText('Upload 16.4Kbps').querySelector('svg')).not.toBeNull()
    expect(within(resources).getByText('8.19Kbps')).toBeTruthy()
    // The used / capacity bytes remain answerable from the identity entry.
    fireEvent.click(within(alphaCard).getByRole('button', { name: 'Node identity details' }))
    const dialog = screen.getByRole('dialog', { name: 'Alpha' })
    expect(dialog.textContent).toContain('12.0 GiB / 48.0 GiB')
    expect(dialog.textContent).toContain('not whole-filesystem fullness')
    expect(dialog.textContent).toContain("process's own percentages")
  })
  it('keeps Network, uptime and Peers on one secondary line while host speed stays full width', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)
    const card = cardOf(nodeCardLink('Alpha'))
    const resources = within(card).getByLabelText('Node process and host network resources')
    const speed = within(resources).getByRole('group', { name: 'Host network speed' })
    expect(speed.parentElement).toBe(resources)
    // The shared Host line spans all three resource columns and carries no
    // resource of its own.
    expect(speed.classList.contains('col-span-3')).toBe(true)
    const meta = card.querySelector('[data-slot="node-identity-meta"]') as HTMLElement
    expect(meta.textContent).toContain('Mainnet')
    expect(meta.textContent).toContain('Uptime Unknown')
    expect(meta.textContent).toContain('Peers 0')
    expect(resources.contains(meta)).toBe(false)
    expect(meta.compareDocumentPosition(resources) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(within(resources).queryByRole('group', { name: 'Node uptime' })).toBeNull()
    expect(within(speed).getByLabelText('Upload 16.4Kbps')).toBeTruthy()
    expect(within(speed).getByLabelText('Download 8.19Kbps')).toBeTruthy()
    expect(within(speed).queryByText('Disk')).toBeNull()
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

  it('keeps a real zero at zero and an uncomputable Disk ratio Unknown', () => {
    const zero = { ...network.nodes[0], nodeId: 'node-zero', displayName: 'Zero data', processCpuPercent: 0, processMemoryPercent: 0, nodeDataDirectorySizeBytes: 0, nodeDataDirectoryCapacityBytes: 51_539_607_552 }
    const missing = { ...network.nodes[0], nodeId: 'node-missing', displayName: 'Missing data', processCpuPercent: null, processMemoryPercent: null, nodeDataDirectorySizeBytes: null, nodeDataDirectoryCapacityBytes: 51_539_607_552 }
    const noCapacity = { ...network.nodes[0], nodeId: 'node-nocapacity', displayName: 'No capacity', nodeDataDirectorySizeBytes: 12_884_901_888, nodeDataDirectoryCapacityBytes: 0 }
    render(<BrowserRouter><HomeDashboard networks={[{ ...network, nodes: [zero, missing, noCapacity] }]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    const diskValue = (name: string) => cardOf(nodeCardLink(name))
      .querySelector('[data-slot="resource-cell"][data-metric="node-data"] [data-slot="resource-value"]')?.textContent
    expect(diskValue('Zero data')).toBe('0.0%')
    expect(diskValue('Missing data')).toBe('Unknown')
    expect(diskValue('No capacity')).toBe('Unknown')
    // An unknown ratio draws the track alone: no value is published for it and
    // no zero-width fill pretends the directory is empty.
    const bars = within(cardOf(nodeCardLink('Missing data'))).getAllByRole('progressbar')
    expect(bars.map(bar => bar.getAttribute('aria-valuenow'))).toEqual([null, null, null])
    expect(within(cardOf(nodeCardLink('Zero data'))).getAllByRole('progressbar').map(bar => bar.getAttribute('aria-valuenow')))
      .toEqual(['0', '0', '0'])
  })

  it('keeps one compact key-value line for the chain height and the shared network row', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    const alphaCard = cardOf(nodeCardLink('Alpha'))
    // Uptime and Peers are identity metadata, not metric rows, and the three
    // resource columns are their own anatomy (#237): the compact key-value line
    // is what is left for the chain height and the shared Host network row.
    // The shared Host network row leads its region; the chain block follows.
    const rows = [...alphaCard.querySelectorAll('[data-slot="metric-row"]')]
    expect(rows.map(row => row.querySelector('[data-slot="metric-row-label"]')?.textContent)).toEqual(['Speed', 'Head'])
    expect(rows.every(row => row.getAttribute('data-layout') === 'compact')).toBe(true)
    expect(alphaCard.querySelectorAll('[data-slot="metric-row"][data-layout="inline"]')).toHaveLength(0)
    expect(alphaCard.querySelectorAll('[data-slot="node-counts"]')).toHaveLength(0)
    expect(alphaCard.querySelectorAll('[data-slot="metric-row"] [data-short-label]')).toHaveLength(0)
  })

  it('keeps the PlatScan Activity in the header and out of the metric grid', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)
    const card = cardOf(nodeCardLink('Alpha'))
    expect(card.querySelector('[data-slot="metric-triple"]')).toBeNull()
    const metrics = card.querySelector('[data-slot="node-business-metrics"]')!
    expect(Array.from(metrics.querySelectorAll('[data-slot="metric-row-label"]'), el => el.textContent)).toEqual(['Head'])
    // The PlatScan status belongs to the identity row, never to the metric grid.
    expect(metrics.querySelector('[data-slot="validator-activity"]')).toBeNull()
    expect(card.querySelector('[data-slot="card-x-header"] [data-slot="validator-activity"]')?.getAttribute('data-activity')).toBe('observing')
    expect(within(card).getByLabelText(/^PlatScan status: Observing/)).toBeTruthy()
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

  it('applies dark-mode-compatible contrast colors to network upload and download speed labels', () => {
    const nodes = [{ ...network.nodes[0], hostNetworkTxBytesPerSec: 1_250_000, hostNetworkRxBytesPerSec: 2_500_000 }]
    render(<BrowserRouter><HomeDashboard networks={[{ ...network, nodes }]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)
    const alphaCard = cardOf(nodeCardLink('Alpha'))
    const uploadEl = within(alphaCard).getByLabelText('Upload 10Mbps')
    const downloadEl = within(alphaCard).getByLabelText('Download 20Mbps')
    for (const token of RATE_DIRECTION_CLASSES.upload.split(' ')) {
      expect(uploadEl.classList.contains(token)).toBe(true)
    }
    for (const token of RATE_DIRECTION_CLASSES.download.split(' ')) {
      expect(downloadEl.classList.contains(token)).toBe(true)
    }
  })

  it('never restates the consensus block heights on the card', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    const alphaCard = cardOf(nodeCardLink('Alpha'))
    // The Node observed QC 100 / Locked 99 / Committed 98 and a raw transaction
    // count; Home reports none of them now (#237), while the same public fields
    // stay readable on Node Detail.
    for (const value of ['100', '99', '98']) {
      expect(within(alphaCard).queryByText(value, { exact: true })).toBeNull()
    }
    // Consensus membership never drives the header chip: the PlatScan Activity
    // badge is an independent dimension.
    expect(within(alphaCard).getByLabelText(/^PlatScan status: Observing/)).toBeTruthy()
    expect(within(alphaCard).queryByText('Stale')).toBeNull()

    // Never-observed consensus is Unknown for every metric, never zero/No.
    const betaCard = cardOf(nodeCardLink('Beta'))
    expect(within(betaCard).getAllByText('Unknown').length).toBeGreaterThanOrEqual(4)
    expect(within(betaCard).getByLabelText(/^PlatScan status: Observing/)).toBeTruthy()
  })

  it('keeps the last-good Block Height while the consensus rows move to Node Detail', () => {
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
    render(<BrowserRouter><HomeDashboard
      networks={[{ ...network, nodes: [staleTrue, failedTrue, failedWithoutLastGood] }]}
      realtimeStatus="connected" online resetting={false} error={null} loading={false}
    /></BrowserRouter>)

    // The Node-level last-good Block Height stays on the card whatever the
    // consensus observation did: it is the Node's own published value, and Home
    // no longer restates QC, Locked, Committed or their retained Stale marker
    // (#237) — Node Detail keeps those rows and their last-good annotation.
    for (const name of ['Stale True', 'Failed True']) {
      const card = cardOf(nodeCardLink(name))
      expect(within(card).getByText('Head')).toBeTruthy()
      expect(within(card).getByText('120')).toBeTruthy()
      expect(within(card).queryByText('Stale')).toBeNull()
    }
    for (const name of ['Stale True', 'Failed True', 'Failed None']) {
      const card = cardOf(nodeCardLink(name))
      for (const label of ['QC', 'Locked', 'Committed']) {
        expect(within(card).queryByText(label, { exact: true })).toBeNull()
      }
      for (const value of ['141', '140', '139', '151', '150', '149']) {
        expect(within(card).queryByText(value, { exact: true })).toBeNull()
      }
    }
    // A Node whose height was never observed keeps Unknown, never a zero.
    expect(within(cardOf(nodeCardLink('Failed None'))).getByText('Head')).toBeTruthy()
    expect(within(cardOf(nodeCardLink('Failed None'))).getAllByText('Unknown').length).toBeGreaterThanOrEqual(4)
  })


  /** One Node whose Block Height comparison the Server's evidence either
   *  supports or does not (#239). Every case starts from a current, healthy
   *  observation, so it only has to state what it is about. */
  const comparableNode = (overrides: Partial<PublicNode> = {}): PublicNode => ({
    ...network.nodes[0],
    nodeId: 'node-head',
    displayName: 'Head Node',
    health: 'healthy',
    rpcState: 'ok',
    freshness: '2026-08-25T00:00:00Z',
    networkReferenceConfidence: 'high',
    ...overrides,
  })
  const renderHeadCard = (node: PublicNode) => render(
    <BrowserRouter><HomeDashboard networks={[{ ...network, nodes: [node] }]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>,
  )
  const headChain = () => cardOf(nodeCardLink('Head Node')).querySelector('[data-slot="node-business-metrics"]') as HTMLElement

  it.each<{ height: number; reference: number; label: string; tone: string; toneClass: string }>([
    { height: 120, reference: 120, label: 'At observed head', tone: 'success', toneClass: 'text-success' },
    { height: 119, reference: 120, label: 'Behind 1 block', tone: 'warning', toneClass: 'text-warning-foreground dark:text-warning' },
    { height: 110, reference: 120, label: 'Behind 10 blocks', tone: 'warning', toneClass: 'text-warning-foreground dark:text-warning' },
    { height: 109, reference: 120, label: 'Behind 11 blocks', tone: 'error', toneClass: 'text-destructive' },
    { height: 125, reference: 120, label: 'Ahead 5 blocks', tone: 'ahead', toneClass: 'text-muted-foreground' },
  ])('reads the Block Height as $label only while the Server evidence supports it (#239)', ({ height, reference, label, tone, toneClass }) => {
    renderHeadCard(comparableNode({ currentHead: height, networkReferenceHead: reference }))

    // The absolute height keeps its own value element and the comparison is a
    // separate short hint beside it, so a comparison never replaces the reading.
    const chain = headChain()
    expect(chain.querySelector('[data-slot="metric-row-label"]')?.textContent).toBe('Head')
    expect(chain.querySelector('[data-slot="metric-row-value"]')?.textContent).toBe(height.toLocaleString())
    const hint = chain.querySelector('[data-slot="node-head-comparison"]') as HTMLElement
    expect(hint.textContent).toBe(label)
    expect(hint.getAttribute('data-tone')).toBe(tone)
    // The display tone is the detail page's own scale, reused rather than a new
    // health or Attention rule.
    expect(hint.className).toContain(toneClass)
    // The reference is named in full where the comparison is explained, and the
    // deployment-local reference is never called a public or canonical head.
    expect(hint.getAttribute('title')).toContain('Observed Network Head')
    expect(hint.getAttribute('title')).toContain('not the public chain head')
    // A comparison needs the Node's own confirmed observation, so the retained
    // mark can never appear beside one.
    expect(chain.querySelector('[data-slot="node-head-retained"]')).toBeNull()
    expect(chain.textContent).not.toMatch(/canonical|explorer|synced/i)
  })

  it.each<{ name: string; change: Partial<PublicNode>; height: string; retained: string | null }>([
    { name: 'a Node health the Server has not confirmed', change: { health: 'unhealthy' }, height: '120', retained: 'Current Node Head not confirmed by Server' },
    { name: 'an Unknown Server health', change: { health: 'unknown' }, height: '120', retained: 'Current Node Head not confirmed by Server' },
    { name: 'a failed RPC collection', change: { rpcState: 'error' }, height: '120', retained: 'Node RPC collection error' },
    { name: 'an unknown RPC collection', change: { rpcState: 'unknown' }, height: '120', retained: 'Node RPC collection unknown' },
    { name: 'a missing observation time', change: { freshness: null }, height: '120', retained: 'Node Head observation time unknown' },
    { name: 'an invalid observation time', change: { freshness: 'not-a-time' }, height: '120', retained: 'Node Head observation time unknown' },
    { name: 'a state token where a time belongs', change: { freshness: 'current' }, height: '120', retained: 'Node Head observation time unknown' },
    // Only this Node's own missing evidence makes the height a retained reading:
    // when the gap is the reference's, the height stays a confirmed current one.
    { name: 'a low-confidence reference', change: { networkReferenceConfidence: 'low' }, height: '120', retained: null },
    { name: 'an unknown-confidence reference', change: { networkReferenceConfidence: 'unknown' }, height: '120', retained: null },
    { name: 'a missing reference height', change: { networkReferenceHead: null }, height: '120', retained: null },
    { name: 'an unsafe reference height', change: { networkReferenceHead: 120.5 }, height: '120', retained: null },
    { name: 'a negative reference height', change: { networkReferenceHead: -1 }, height: '120', retained: null },
    { name: 'a missing Node height', change: { currentHead: null }, height: 'Unknown', retained: null },
    // A height that cannot be a Block Height — negative or fractional — is not a
    // reading this card restates: like a height that was never observed it stays
    // explicitly Unknown rather than a value a reader could trust, and it
    // supports no comparison either (design §11.2, #239).
    { name: 'a negative Node height', change: { currentHead: -1 }, height: 'Unknown', retained: null },
    { name: 'a fractional Node height', change: { currentHead: 120.5 }, height: 'Unknown', retained: null },
  ])('asserts no comparison conclusion for $name (#239)', ({ change, height, retained }) => {
    renderHeadCard(comparableNode(change))

    // No comparison is drawn and no sync conclusion is written, while the
    // absolute value keeps its own honest reading: a never-observed or impossible
    // height stays Unknown and is never a fabricated zero.
    const chain = headChain()
    expect(chain.querySelector('[data-slot="node-head-comparison"]')).toBeNull()
    expect(chain.querySelector('[data-slot="metric-row-value"]')?.textContent).toBe(height)
    // A value the Server still holds but is not confirming as current says so,
    // instead of reading as an ordinary current height (#239, #236 story 26).
    const mark = chain.querySelector('[data-slot="node-head-retained"]')
    if (retained === null) expect(mark).toBeNull()
    else {
      expect(mark?.textContent).toBe('Last-good')
      expect(mark?.getAttribute('title')).toContain(retained)
    }
    expect(chain.textContent).not.toMatch(/Ahead|Behind|At observed head|synced/i)
  })

  it('names the retained Block Height and that no comparison comes from it (#239)', () => {
    renderHeadCard(comparableNode({ rpcState: 'error' }))
    fireEvent.click(within(cardOf(nodeCardLink('Head Node'))).getByRole('button', { name: 'Node identity details' }))
    const dialog = screen.getByRole('dialog', { name: 'Head Node' })

    expect(dialog.textContent).toContain('the last Block Height the Server retained for this Node')
    expect(dialog.textContent).toContain('marked Last-good because Node RPC collection error')
    expect(dialog.textContent).toContain('No comparison is asserted from a retained height')
    expect(dialog.textContent).toContain('Observed Network Head')
  })

  it('keeps the comparison, its reference and its display boundary in the card explanation (#239)', () => {
    renderHeadCard(comparableNode({ currentHead: 118, networkReferenceHead: 120 }))
    fireEvent.click(within(cardOf(nodeCardLink('Head Node'))).getByRole('button', { name: 'Node identity details' }))
    const dialog = screen.getByRole('dialog', { name: 'Head Node' })

    expect(dialog.textContent).toContain('Behind 2 blocks')
    expect(dialog.textContent).toContain('Observed Network Head')
    expect(dialog.textContent).toContain('not the public chain head')
    expect(dialog.textContent).toContain('does not claim the Node is synchronized')
    expect(dialog.textContent).toContain('never a health, alert or Attention rule')
    expect(dialog.textContent).not.toMatch(/canonical|explorer/i)
  })

  it('names why no comparison is asserted while the comparison cannot be supported (#239)', () => {
    renderHeadCard(comparableNode({ networkReferenceConfidence: 'low' }))
    fireEvent.click(within(cardOf(nodeCardLink('Head Node'))).getByRole('button', { name: 'Node identity details' }))
    const dialog = screen.getByRole('dialog', { name: 'Head Node' })

    expect(dialog.textContent).toContain('No comparison is asserted here: Observed Network Head confidence low')
    expect(dialog.textContent).toContain('Observed Network Head')
    // No comparison label is written anywhere in the explanation.
    expect(dialog.textContent).not.toContain('Shown here')
    expect(dialog.textContent).not.toMatch(/At observed head/i)
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
      { name: 'Candidate Node', validator: linked('candidate', 'current'), activity: 'candidate', label: 'Candidate' },
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

  it('marks Healthy green, Unhealthy red, and Unknown amber before the name (issue #141, round 2)', () => {
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

    // Three distinct tones, not two: a confirmed failed observation takes the
    // error tone, while missing, stale or never-observed evidence is a warning.
    // Neither is ever painted as Offline.
    const unknownMarker = within(cardOf(nodeCardLink('Beta'))).getByRole('img', { name: 'Unknown' })
    expect(unknownMarker.classList.contains('node-health-marker-unknown')).toBe(true)
    expect(unknownMarker.getAttribute('data-tone')).toBe('unknown')
    expect(unknownMarker.classList.contains('bg-warning')).toBe(true)
    const unhealthyMarker = within(cardOf(nodeCardLink('Unhealthy Node'))).getByRole('img', { name: 'Unhealthy' })
    expect(unhealthyMarker.classList.contains('node-health-marker-unhealthy')).toBe(true)
    expect(unhealthyMarker.getAttribute('data-tone')).toBe('unhealthy')
    expect(document.querySelector('.node-health-marker-other')).toBeNull()
    expect(screen.queryByText(/Offline/)).toBeNull()

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
    // The diagnostic line carries the same verdict as the marker and keeps the
    // Server's own sentence as its title, so a short line hides no reason.
    const unknownDiagnostic = cardOf(nodeCardLink('Beta')).querySelector('[data-slot="node-diagnostic"]')!
    expect(unknownDiagnostic.getAttribute('data-tone')).toBe('warning')
    expect(unknownDiagnostic.getAttribute('title')).toBe('Never observed')
    const unhealthyDiagnostic = cardOf(nodeCardLink('Unhealthy Node')).querySelector('[data-slot="node-diagnostic"]')!
    expect(unhealthyDiagnostic.getAttribute('data-tone')).toBe('destructive')
    expect(unhealthyDiagnostic.getAttribute('title')).toBe('RPC failed')
  })

  it('shortens a long stale-observation reason to one line and keeps the full one reachable', () => {
    // The Centaur case: the Server's stale/unknown verdict arrives as one long
    // sentence that used to crowd the card header in red, as if the evidence
    // that was missing were a confirmed failure.
    const reason = 'one or more observations are stale or unknown'
    const centaur = { ...network.nodes[1], nodeId: 'node-centaur', displayName: 'Centaur', health: 'unknown', healthReason: reason }
    render(<BrowserRouter><HomeDashboard
      networks={[{ ...network, nodes: [centaur] }]}
      realtimeStatus="connected" online resetting={false} error={null} loading={false}
    /></BrowserRouter>)

    const card = cardOf(nodeCardLink('Centaur'))
    const diagnostic = card.querySelector('[data-slot="node-diagnostic"]')!
    expect(diagnostic.textContent).toBe('Observations stale / unknown')
    expect(diagnostic.getAttribute('data-tone')).toBe('warning')
    expect(diagnostic.className).toContain('text-warning-foreground')
    expect(diagnostic.className).not.toContain('text-destructive')
    expect(diagnostic.getAttribute('title')).toBe(reason)
    expect(card.getAttribute('data-tone')).toBe('warn')
    // The identity entry keeps the Server's full reason, so nothing is lost.
    fireEvent.click(within(card).getByRole('button', { name: 'Node identity details' }))
    const dialog = screen.getByRole('dialog')
    expect(within(dialog).getByText('Current health diagnostic: ' + reason)).toBeTruthy()
    expect(within(dialog).queryByText('Current health diagnostic: Observations stale / unknown')).toBeNull()
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
      // #240: the trailing marker or control is the one shared slot in every
      // header, so title, marker and value line up across all six tiles by
      // construction instead of per-tile offsets (the geometry is measured in
      // the browser acceptance).
      const header = card.querySelector('[data-slot="card-x-content"] > div:first-child')!
      const marker = card.querySelector('[data-slot="summary-marker"]')
      expect(marker).not.toBeNull()
      expect(marker!.querySelectorAll('svg[data-icon]')).toHaveLength(1)
      expect(header.lastElementChild).toBe(marker)
      expect(card.getAttribute('aria-label')?.startsWith(header.firstElementChild?.textContent ?? '\u0000')).toBe(true)
    }
  })

  it('orders all six overview cards together and removes the standalone Validator summary', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} rewardWallet={rewardWallet} /></BrowserRouter>)
    const summary = screen.getByLabelText('Home summary')
    const cards = within(summary).getAllByRole('article')
    // #246: DOM order is the visual order — two rows of three from 640px
    // (Active/Healthy/Attention, Balance/blocks/rewards) and three rows of two
    // below it (Active/Healthy, Attention/Balance, blocks/rewards).
    expect(cards.map(card => card.querySelector('[data-slot="card-x-content"] > div:first-child > span')?.textContent)).toEqual([
      'Active Nodes', 'Healthy Nodes', 'Attention', 'Balance', 'Cumulative blocks', 'Cumulative rewards',
    ])
    expect(cards.map(card => card.querySelector('[data-slot="summary-value"]')?.textContent)).toEqual(['2', '1', '1', '138,179.54 LAT', '100', '10'])
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

  it('states what Attention counts and what Attention First leads with, without widening either', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)
    const attention = summaryCardOf('Attention')
    // The tile names the two states it counts inside the caption slot every card
    // already reserves, and the full reading travels as the tile's title. The
    // plain count tiles stay wordless, so the band keeps its density (round 2).
    expect(attention.querySelector('[data-slot="summary-caption"]')?.textContent).toBe('Unhealthy or Unknown')
    expect(attention.querySelector('[data-slot="summary-reading"]')?.getAttribute('title')).toBe('Home Attention counts Active Nodes the Server has not confirmed Healthy: Unhealthy or Unknown. A chain-height difference never counts, and Observing is not an abnormal state.')
    // The explanation is a tooltip, never copy the compact tile has to render.
    expect(attention.querySelector('[data-slot="card-x-header"]')).toBeNull()
    expect(screen.queryByText(/^Home Attention counts Active Nodes/)).toBeNull()
    const summary = screen.getByLabelText('Home summary')
    expect([...summary.querySelectorAll('[data-slot="summary-caption"]')].filter((caption) => caption.textContent === 'Unhealthy or Unknown')).toHaveLength(1)
    // The two plain count tiles stay wordless: the caption slot they already
    // reserve is empty, so the band's height and density are unchanged.
    expect(['Active Nodes', 'Healthy Nodes'].map((label) => summaryCardOf(label).querySelector('[data-slot="summary-caption"]')?.textContent)).toEqual(['', ''])
    // Attention First is the one order whose name does not say what it leads
    // with, so its explanation is announced with the control that selects it,
    // and withdrawn again when another order is chosen.
    const sort = selectNamed('Sort')
    expect(sort.getAttribute('aria-describedby')).toBeNull()
    fireEvent.change(sort, { target: { value: 'attention' } })
    const hintId = sort.getAttribute('aria-describedby')
    expect(hintId).toBeTruthy()
    expect(document.getElementById(hintId as string)?.textContent).toBe('Attention First leads with the Active Nodes the Server has not confirmed Healthy, Unhealthy before Unknown, and lists the Healthy ones last.')
    fireEvent.change(sort, { target: { value: 'health' } })
    expect(sort.getAttribute('aria-describedby')).toBeNull()
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

  it('paints Active Nodes, Healthy Nodes, and Balance with the brand-green marker', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} rewardWallet={rewardWallet} /></BrowserRouter>)

    for (const label of ['Active Nodes', 'Healthy Nodes', 'Balance']) {
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
    // The unattested Peer count is explicit and never presented as Current.
    expect(within(betaCard).getByText('Peers Unknown')).toBeTruthy()
    expect(within(betaCard).getByText('no successful snapshot')).toBeTruthy()
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

  /** A percentage is asserted only while a valid current height and a valid,
   *  computable Historical High-Water Mark target both exist (#239): an Unknown
   *  percentage is not a progress area, so the cue keeps the Server's state word
   *  and the retained target/height stay in its own explanation. */
  const resyncHeights: Array<{
    currentHead: number | null | undefined
    historicalHighWatermark: number | null | undefined
    height: string
    percent: string | null
    /** True where two decimals would print a finished 100.00% for a Node the
     *  Server still reports as Resyncing, so the cue states ≈100% instead (#246). */
    approximate?: boolean
  }> = [
    { currentHead: 0, historicalHighWatermark: 100, height: '0 / 100', percent: '0.00%' },
    { currentHead: 100, historicalHighWatermark: 100, height: '100 / 100', percent: '100.00%' },
    { currentHead: 99_999_999, historicalHighWatermark: 100_000_000, height: '99,999,999 / 100,000,000', percent: '≈100%', approximate: true },
    { currentHead: 99_990_000, historicalHighWatermark: 100_000_000, height: '99,990,000 / 100,000,000', percent: '99.99%' },
    { currentHead: 100_000_001, historicalHighWatermark: 100_000_000, height: '100,000,001 / 100,000,000', percent: '100.00%' },
    { currentHead: 0, historicalHighWatermark: 0, height: '0 / 0', percent: null },
    { currentHead: 50, historicalHighWatermark: null, height: '50 / Unknown', percent: null },
    { currentHead: 50, historicalHighWatermark: undefined, height: '50 / Unknown', percent: null },
    { currentHead: null, historicalHighWatermark: 100, height: 'Unknown / 100', percent: null },
    { currentHead: undefined, historicalHighWatermark: 100, height: 'Unknown / 100', percent: null },
    { currentHead: NaN, historicalHighWatermark: 100, height: 'Unknown / 100', percent: null },
    { currentHead: Infinity, historicalHighWatermark: 100, height: 'Unknown / 100', percent: null },
    { currentHead: 50, historicalHighWatermark: Infinity, height: '50 / Unknown', percent: null },
    { currentHead: -1, historicalHighWatermark: 100, height: 'Unknown / 100', percent: null },
    { currentHead: 50, historicalHighWatermark: -1, height: '50 / Unknown', percent: null },
    { currentHead: 0.5, historicalHighWatermark: 100, height: 'Unknown / 100', percent: null },
  ]
  it.each(resyncHeights)('renders resync heights safely: $height ($percent)', ({ currentHead, historicalHighWatermark, height, percent, approximate }) => {
    const node = { ...network.nodes[0], resyncState: 'resyncing', currentHead, historicalHighWatermark }
    render(<BrowserRouter><HomeDashboard networks={[{ ...network, nodes: [node] }]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)
    const card = cardOf(nodeCardLink('Alpha'))
    const progress = within(card).getByRole('button', { name: /Resync/ })
    expect(progress.textContent).not.toMatch(/NaN|Infinity|Unknown/)
    if (percent === null) {
      // The state word is the whole cue: no percentage area, and no claim about
      // progress anywhere on the card.
      expect(progress.textContent).toBe('Resyncing')
      expect(card.textContent).not.toContain('Resyncing Unknown')
      expect(progress.getAttribute('aria-label')).not.toContain('%')
    } else {
      expect(progress.textContent).toBe('Resyncing' + percent)
      // The rounded share is the ordinary statement; a share that is short of the
      // target while two decimals would print a finished 100.00% is stated as an
      // approximation instead, so the chip never reads as a sync the Server has
      // not reported done (#246).
      expect(progress.getAttribute('aria-label')).toBe(approximate
        ? 'Resync progress: approximately 100% toward the Historical High-Water Mark, short of the target; the share is stated in the dialog'
        : 'Resync progress: ' + percent + ' toward the Historical High-Water Mark')
    }
    fireEvent.click(progress)
    const dialogElement = screen.getByRole('dialog', { name: 'Resync progress' })
    const dialog = within(dialogElement)
    expect(dialog.getByText(height)).toBeTruthy()
    // The dialog states the figure the cue declined to print - the closest reading
    // this card prints, never one dressed up as the exact share - and only while
    // the cue is the approximate one.
    if (approximate) {
      expect(dialogElement.textContent).toContain('the closest reading this card prints is 99.999999%')
      expect(dialogElement.textContent).toContain('two decimal places print 100.00%')
    } else {
      expect(dialogElement.textContent).not.toContain('two decimal places print 100.00%')
    }
    if (percent === null) expect(dialog.getByText(/No percentage is asserted/)).toBeTruthy()
    else expect(dialog.queryByText(/No percentage is asserted/)).toBeNull()
  })

  it('keeps a Resyncing Node short of its target readable as short, never as a finished 100% (#246)', () => {
    const nearTarget = {
      ...network.nodes[0],
      resyncState: 'resyncing',
      currentHead: 999_999_999,
      historicalHighWatermark: 1_000_000_000,
      resyncLastProgressAt: '2026-08-25T00:00:00Z',
    }
    render(<BrowserRouter><HomeDashboard networks={[{ ...network, nodes: [nearTarget] }]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)
    const card = cardOf(nodeCardLink('Alpha'))
    const cue = within(card).getByRole('button', { name: /Resync progress/ })

    // 99.9999999% rounds to 100.00% at two places, and a chip reading 100.00%
    // beside the word Resyncing would claim a sync the Server has not reported as
    // done. The cue keeps the Server's state and marks the share as approximate.
    expect(cue.textContent).toBe('Resyncing≈100%')
    expect(cue.getAttribute('aria-label')).not.toContain('100.00%')

    fireEvent.click(cue)
    const dialog = screen.getByRole('dialog', { name: 'Resync progress' })
    // Both figures stay answerable: the rounded one the chip declined to print,
    // the closest reading of the share that explains why, and the untouched
    // retained heights.
    expect(dialog.textContent).toContain('999,999,999 / 1,000,000,000')
    expect(dialog.textContent).toContain('the closest reading this card prints is 99.9999999%')
    expect(dialog.textContent).toContain('two decimal places print 100.00%')
    expect(dialog.textContent).toContain('≈100%')
    expect(dialog.textContent).not.toContain('No percentage is asserted')
    // The state stays the Server's: nothing here decides the Resync has finished.
    expect(within(card).getByText('Resyncing')).toBeTruthy()
  })

  it('keeps a Node at its exact target Resyncing while the Server still reports it (#246)', () => {
    const atTarget = {
      ...network.nodes[0],
      resyncState: 'resyncing',
      currentHead: 1_000_000_000,
      historicalHighWatermark: 1_000_000_000,
      resyncLastProgressAt: '2026-08-25T00:00:00Z',
    }
    render(<BrowserRouter><HomeDashboard networks={[{ ...network, nodes: [atTarget] }]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)
    const cue = within(cardOf(nodeCardLink('Alpha'))).getByRole('button', { name: 'Resync progress: 100.00% toward the Historical High-Water Mark' })

    // An exact 100.00% is shown as recorded rather than replaced by an
    // approximation, and it does not end the Resync the Server reports: no
    // completion is inferred from the number.
    expect(cue.textContent).toBe('Resyncing100.00%')
    fireEvent.click(cue)
    const dialog = screen.getByRole('dialog', { name: 'Resync progress' })
    expect(dialog.textContent).toContain('1,000,000,000 / 1,000,000,000')
    expect(dialog.textContent).not.toContain('two decimal places print 100.00%')
    expect(dialog.textContent).not.toContain('No percentage is asserted')
  })

  it('states a share too close to the target to print, rather than a number reading 100% (#246)', () => {
    const beyondAnyDecimal = {
      ...network.nodes[0],
      resyncState: 'resyncing',
      currentHead: 9_007_199_254_740_990,
      historicalHighWatermark: 9_007_199_254_740_991,
      resyncLastProgressAt: '2026-08-25T00:00:00Z',
    }
    render(<BrowserRouter><HomeDashboard networks={[{ ...network, nodes: [beyondAnyDecimal] }]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)
    const card = cardOf(nodeCardLink('Alpha'))
    const cue = within(card).getByRole('button', { name: /Resync progress: approximately 100%/ })

    // This share sits below its target and above every reading the card prints, so
    // there is no number to set beside the word Resyncing: the cue marks the
    // approximation, and the dialog answers in words rather than claiming a target
    // the Server has not reported as reached.
    expect(cue.textContent).toBe('Resyncing≈100%')

    fireEvent.click(cue)
    const dialog = screen.getByRole('dialog', { name: 'Resync progress' })
    expect(dialog.textContent).toContain('9,007,199,254,740,990 / 9,007,199,254,740,991')
    expect(dialog.textContent).toContain('less than any decimal this card prints')
    expect(dialog.textContent).toContain('two decimal places print 100.00%')
    expect(dialog.textContent).not.toContain("the closest reading this card prints is 100")
    expect(dialog.textContent).not.toContain('No percentage is asserted')
    expect(within(card).getByText('Resyncing')).toBeTruthy()
  })

  it('keeps the Resyncing state independent of an unavailable comparison and of its health diagnostic (#239)', () => {
    renderHeadCard(comparableNode({
      health: 'unhealthy', healthReason: 'RPC observation failed',
      resyncState: 'resyncing', currentHead: 50, historicalHighWatermark: 100,
      resyncLastProgressAt: '2026-08-25T00:00:00Z',
    }))
    const card = cardOf(nodeCardLink('Head Node'))

    // The health diagnostic stays readable, the comparison is not asserted while
    // the Server has not confirmed the Node's health, and the Server-owned
    // Resyncing state keeps its own real percentage: none of the three hides
    // another, and the retained target is not the Observed Network Head.
    expect(within(card).getByText('RPC observation failed')).toBeTruthy()
    expect(card.querySelector('[data-slot="node-head-comparison"]')).toBeNull()
    expect(card.querySelector('[data-slot="node-resync-cue"]')?.textContent).toBe('Resyncing50.00%')
    expect(card.textContent).not.toContain('Resyncing Unknown')
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
    // An observed zero block height is an authoritative zero, never Unknown, and
    // the chain block keeps only the current Block Height (#237).
    expect(within(cardOf(nodeCardLink('Alpha'))).getByText('Head').nextElementSibling?.textContent).toBe('100')
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
    fireEvent.change(selectNamed('Sort'), { target: { value: 'head' } })
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
    const summary = screen.getByLabelText('Home summary')
    // Five tiles wait on the Network projection; the Balance waits on its own
    // query and states that it is loading, never a zero.
    expect(within(summary).getAllByText('Unknown')).toHaveLength(5)
    expect(summaryValueOf('Balance').textContent).toBe('—')
    expect(summaryCardOf('Balance').querySelector('[data-slot="summary-caption"]')?.textContent).toBe('Loading…')
    expect(screen.queryByText('No Active Nodes in this view.')).toBeNull()
  })

  it('keeps the Balance on its own query when the Network projection is loading or failed', () => {
    const { unmount } = render(<BrowserRouter><HomeDashboard networks={[]} realtimeStatus="connecting" online loading resetting={false} error={null} rewardWallet={rewardWallet} /></BrowserRouter>)
    expect(summaryValueOf('Active Nodes').textContent).toBe('Unknown')
    expect(summaryValueOf('Balance').textContent).toBe('138,179.54 LAT')
    unmount()
    render(<BrowserRouter><HomeDashboard networks={[]} realtimeStatus="connected" online resetting={false} error="Unable to load Active Nodes" hasLastGood={false} loading={false} rewardWallet={rewardWallet} /></BrowserRouter>)
    expect(screen.getByText('Unable to load Active Nodes')).toBeTruthy()
    expect(summaryValueOf('Active Nodes').textContent).toBe('Unknown')
    expect(summaryValueOf('Balance').textContent).toBe('138,179.54 LAT')
  })

  it('preserves an authoritative empty projection when a refresh fails', () => {
    render(<BrowserRouter><HomeDashboard networks={[]} realtimeStatus="connected" online resetting={false} error="Unable to load Active Nodes" hasLastGood loading={false} /></BrowserRouter>)
    expect(screen.getByText('Unable to load Active Nodes')).toBeTruthy()
    expect(screen.getByText('No Active Nodes in this view.')).toBeTruthy()
    expect(summaryValueOf('Active Nodes').textContent).toBe('0')
  })

  it('shows only the three Validator summary metrics a confirmed status allows', () => {
    const linked = {
      validatorId: 'validator-linked', validatorNodeId: '0xlinked', displayName: 'Validator A',
      nodeId: 'node-linked', state: 'fresh', freshness: 'fresh', source: 'platscan',
      providerTimestamp: '2026-08-25T00:00:00Z', receivedAt: '2026-08-25T00:00:05Z',
      blockCount: 123456, expectedBlockCount: 110, blockRate: '90.909091', blockRateState: 'ok',
      rank: 5, rankState: 'ranked', rankFreshness: 'fresh', rankCohortSize: 300,
      rewardAmount: '1234.123456789012',
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
    expect(linkedSection.getAttribute('data-identity')).toBe('validator')
    // Only the three key summaries stay, directly visible without hover or
    // expansion: the Network rank, the lifetime block count and the gross
    // lifetime rewards (#238).
    expect(validatorSummaryValue(card, 'Network rank')).toBe('#5')
    expect(validatorSummaryValue(card, 'Cumulative blocks')).toBe('123,456')
    expect(validatorSummaryValue(card, 'Cumulative rewards')).toBe('1.23K')
    // Production rate, the PlatScan 24h rate and the delegation reward share
    // left the Home card and stay on Node Detail.
    expect(within(card).queryByText('Production rate')).toBeNull()
    expect(within(card).queryByText('PlatScan 24h rate')).toBeNull()
    expect(within(card).queryByText('Delegation reward share')).toBeNull()
    const titleLink = nodeCardLink('Calico')
    expect(titleLink.querySelector('button, input, textarea, a')).toBeNull()
    const control = within(card).getByRole('button', { name: 'Node identity details' })
    expect(control.closest('a')).toBeNull()
    expect(control.className).toContain('relative z-10')

    const unlinkedCard = cardOf(nodeCardLink('Domino'))
    expect(within(unlinkedCard).getByText('Validator unknown')).toBeTruthy()
    expect(within(unlinkedCard).getByText('Identity has not been observed yet.')).toBeTruthy()
    expect(within(unlinkedCard).queryByText('Cumulative blocks')).toBeNull()
    expect(within(unlinkedCard).queryByText('Linked Validator')).toBeNull()
  })
})

/**
 * Issue #241: the Home toolbar's search is the row's own control in two shapes.
 * The row holds both — a compact field, and the accessibly named entry that
 * opens that same field — and the toolbar's own width decides which one a
 * reader gets, in CSS, so jsdom paints neither and the browser acceptance owns
 * the switch itself. A unit test therefore names the slot it reads; both slots
 * read the one query, and neither holds a word the other lost.
 */
const searchSlots = () => ({
  inline: document.querySelector('[data-slot="home-search-inline"]'),
  collapsed: document.querySelector('[data-slot="home-search-collapsed"]'),
})
const inlineField = () =>
  within(searchSlots().inline as HTMLElement).getByRole('searchbox', { name: 'Search Active Nodes' }) as HTMLInputElement
const collapsedField = () =>
  within(searchSlots().collapsed as HTMLElement).getByRole('searchbox', { name: 'Search Active Nodes' }) as HTMLInputElement

/** Issue #222: Home keeps its filter state in an ordinary navigation URL. The
 *  URL restores what it can, reports what it cannot honour, and is rewritten
 *  only when the reader acts. */
describe('Public Home filter URLs', () => {
  const secondNetwork = {
    ...network,
    networkKey: 'testnet',
    displayName: 'Testnet',
    nodes: [{ ...network.nodes[0], nodeId: 'node-c', displayName: 'Gamma', networkKey: 'testnet', currentHead: 900 }],
  }

  // queryAll: an empty grid is a result this suite asserts, not an error.
  const nodeHrefs = () => screen.queryAllByRole('link')
    .map((link) => link.getAttribute('href'))
    .filter((href): href is string => href !== null && href.startsWith('/nodes/'))
  // Issue #241: the reader's field is the row's own control, so these URL cases
  // read and type into it directly rather than opening the entry that used to
  // hold the only field there was.
  const searchbox = () => inlineField()
  const filterNotice = () => document.querySelector('[data-slot="home-filter-notice"]')

  it('restores the Network, search, and sorting an ordinary Home URL carries', () => {
    window.history.replaceState({}, '', '/?network=testnet&q=Gamma&sort=name')
    renderHome([network, secondNetwork])

    expect(screen.getByRole('tab', { name: 'Testnet' }).getAttribute('aria-selected')).toBe('true')
    expect(nodeHrefs()).toEqual(['/nodes/node-c'])
    expect(summaryValueOf('Active Nodes').textContent).toBe('1')
    expect(searchbox().value).toBe('Gamma')
    expect(selectNamed('Sort').value).toBe('name')
    expect(filterNotice()).toBeNull()
  })

  it('narrows the list by search and reads the three counters over that selection', async () => {
    renderHome([network, secondNetwork])
    // The overview counters are the one place Home states how many Nodes it
    // holds: the list itself carries no count line.
    expect(summaryValueOf('Active Nodes').textContent).toBe('3')
    expect(document.querySelector('[data-scope-basis]')?.getAttribute('data-scope-basis')).toBe('network')
    // Nothing narrows the list yet, so the cumulative cells name the plain scope.
    expect(screen.getByRole('article', { name: 'Cumulative blocks' }).querySelector('[data-slot="summary-caption"]')?.textContent).toMatch(/^2 Networks · /)

    fireEvent.change(searchbox(), { target: { value: 'Alpha' } })

    await waitFor(() => expect(nodeHrefs()).toEqual(['/nodes/node-a']))
    expect(window.location.search).toBe('?q=Alpha')
    // Issue #233: the three attributable counters and the map read the list
    // selection the filters left, while the two cumulative cells keep the
    // Network selection and say so on the card.
    expect(summaryValueOf('Active Nodes').textContent).toBe('1')
    // The Balance is not part of the list selection.
    expect(summaryValueOf('Balance').textContent).toBe('138,179.54 LAT')
    expect(document.querySelector('[data-scope-basis]')?.getAttribute('data-scope-basis')).toBe('node-selection')
    // #245: the note is one scope word on the face, not a sentence in the caption.
    expect(screen.getByRole('article', { name: 'Cumulative blocks' }).querySelector('[data-slot="summary-caption"]')?.textContent).toMatch(/^All of 2 Networks · /)
    expect(screen.getByRole('article', { name: 'Cumulative rewards' }).querySelector('[data-slot="summary-caption"]')?.textContent).toMatch(/^All of 2 Networks · /)
    expect(document.body.textContent).not.toContain('Whole Network selection')
  })

  it('searches the public Node name, the Node ID, and the Network name', async () => {
    renderHome([network, secondNetwork])

    fireEvent.change(searchbox(), { target: { value: 'alpha' } })
    await waitFor(() => expect(nodeHrefs()).toEqual(['/nodes/node-a']))

    fireEvent.change(searchbox(), { target: { value: 'node-b' } })
    await waitFor(() => expect(nodeHrefs()).toEqual(['/nodes/node-b']))

    fireEvent.change(searchbox(), { target: { value: 'Testnet' } })
    await waitFor(() => expect(nodeHrefs()).toEqual(['/nodes/node-c']))
  })

  it('takes a search one character at a time and commits the whole text', async () => {
    renderHome([network, secondNetwork])
    const box = searchbox()

    // A keyboard appends to whatever the box already holds, one character at a
    // time, and every character commits its own URL. A value read straight back
    // out of the address bar would drop the characters typed in the meantime.
    for (const character of 'Gamma') {
      fireEvent.change(box, { target: { value: `${box.value}${character}` } })
    }

    expect(box.value).toBe('Gamma')
    await waitFor(() => expect(window.location.search).toBe('?q=Gamma'))
    expect(nodeHrefs()).toEqual(['/nodes/node-c'])
  })

  it('combines the search with the health filter and reads an empty selection as empty', () => {
    window.history.replaceState({}, '', '/?q=Alpha&health=unknown')
    renderHome([network])

    expect(nodeHrefs()).toEqual([])
    expect(screen.getByText('No Active Nodes match these filters.')).toBeTruthy()
    expect(document.body.textContent).toContain('widen the filters')
    // Issue #233: a selection that holds no Node is an empty reading, not a
    // zero — the three counters that follow the list state no figure at all
    // instead of inventing a country count for the Nodes they did not find.
    expect(summaryValueOf('Active Nodes').textContent).toBe('No match')
    expect(summaryValueOf('Healthy Nodes').textContent).toBe('No match')
    expect(summaryValueOf('Attention').textContent).toBe('No match')
    // The Balance is the Reward Wallet's own figure: it still reads its value.
    expect(summaryValueOf('Balance').textContent).toBe('138,179.54 LAT')
    expect(document.querySelector('[data-scope-basis]')?.getAttribute('data-scope-basis')).toBe('node-selection')
    // The two cumulative cells do not follow the list, so they keep the
    // Network selection's figures and say so on the card.
    expect(screen.getByRole('article', { name: 'Cumulative blocks' }).querySelector('[data-slot="summary-caption"]')?.textContent).toMatch(/^All of Mainnet · /)
    // The map says the selection holds no Node; it never draws an empty world
    // and never reports a failure.
    expect(document.body.textContent).toContain('No Active Nodes match these filters')
  })

  it('reports the URL values this deployment cannot honour instead of obeying them', () => {
    window.history.replaceState({}, '', '/?network=gone&health=critical&sort=size')
    renderHome([network])

    const notice = filterNotice()
    expect(notice?.textContent).toContain('Network "gone"')
    expect(notice?.textContent).toContain('health "critical"')
    expect(notice?.textContent).toContain('sort "size"')
    expect(notice?.textContent).toContain('fell back to their defaults')
    // Everything the link could honour still applies, and the link reaches the
    // reader exactly as it arrived.
    expect(screen.getByRole('tab', { name: 'All Networks' }).getAttribute('aria-selected')).toBe('true')
    expect(summaryValueOf('Active Nodes').textContent).toBe('2')
    expect(window.location.search).toBe('?network=gone&health=critical&sort=size')

    // Acting on a filter is what rewrites the link, without the refused values.
    fireEvent.change(selectNamed('Sort'), { target: { value: 'name' } })
    expect(window.location.search).toBe('?sort=name')
    expect(filterNotice()).toBeNull()
  })

  it('keeps a Network selection provisional until the projection can judge it', () => {
    window.history.replaceState({}, '', '/?network=testnet')
    const view = renderHome([], { loading: true })

    expect(filterNotice()).toBeNull()
    // Nothing is listed until the projection can judge the Network selection.
    expect(nodeHrefs()).toEqual([])

    view.rerender(
      <BrowserRouter>
        <HomeDashboard networks={[network, secondNetwork]} realtimeStatus="connected" online resetting={false} error={null} loading={false} />
      </BrowserRouter>,
    )

    expect(screen.getByRole('tab', { name: 'Testnet' }).getAttribute('aria-selected')).toBe('true')
    expect(nodeHrefs()).toEqual(['/nodes/node-c'])
    expect(filterNotice()).toBeNull()
  })

  it('writes the reader choices back into an ordinary Home URL', async () => {
    renderHome([network, secondNetwork])

    fireEvent.mouseDown(screen.getByRole('tab', { name: 'Testnet' }))
    expect(window.location.search).toBe('?network=testnet')

    fireEvent.change(searchbox(), { target: { value: 'Gamma' } })
    await waitFor(() => expect(window.location.search).toBe('?network=testnet&q=Gamma'))

    fireEvent.change(selectNamed('Health filter'), { target: { value: 'healthy' } })
    expect(window.location.search).toBe('?network=testnet&q=Gamma&health=healthy')

    fireEvent.change(selectNamed('Validator status filter'), { target: { value: 'unknown' } })
    expect(window.location.search).toBe('?network=testnet&q=Gamma&health=healthy&validator=unknown')
    expect(nodeHrefs()).toEqual(['/nodes/node-c'])
  })

  it('immediately flushes pending debounced search query when switching another filter', () => {
    renderHome([network, secondNetwork])

    fireEvent.mouseDown(screen.getByRole('tab', { name: 'Testnet' }))
    expect(window.location.search).toBe('?network=testnet')

    // Type query into searchbox within debounce window
    fireEvent.change(searchbox(), { target: { value: 'Gamma' } })
    // Immediately switch Health filter without waiting for debounce timer
    fireEvent.change(selectNamed('Health filter'), { target: { value: 'healthy' } })

    // Must immediately flush the draft query along with the health filter
    expect(window.location.search).toBe('?network=testnet&q=Gamma&health=healthy')
    expect(nodeHrefs()).toEqual(['/nodes/node-c'])
  })

  it('debounces search query before committing to URL and filter pipeline', async () => {
    window.history.replaceState({}, '', '/')
    vi.useFakeTimers()
    try {
      renderHome([network, secondNetwork])
      fireEvent.change(searchbox(), { target: { value: 'Alpha' } })

      // Immediate draft text reflected in input
      expect(searchbox().value).toBe('Alpha')
      // URL has not updated yet before debounce
      expect(window.location.search).toBe('')

      // Advance less than debounce duration (150ms)
      vi.advanceTimersByTime(100)
      expect(window.location.search).toBe('')

      // Advance past debounce duration
      vi.advanceTimersByTime(60)
      expect(window.location.search).toBe('?q=Alpha')
    } finally {
      vi.useRealTimers()
    }
    await waitFor(() => expect(nodeHrefs()).toEqual(['/nodes/node-a']))
  })

  it('filters by the Server Validator status and keeps unknown separate from not a Validator', () => {
    const linked = (status: string) => ({
      validatorId: 'validator-filter', validatorNodeId: '0xfilter', displayName: 'Validator F',
      nodeId: 'node-v', state: 'fresh', freshness: 'fresh', source: 'platscan',
      receivedAt: '2026-08-25T00:00:00Z', rankState: 'unknown', rankFreshness: 'unknown',
      blockRateState: 'unknown', counterState: 'normal', activity: 'active', activityState: 'current',
      currentValidatorStatus: status, currentValidatorStatusState: 'current', currentValidatorStatusQualifier: null,
    })
    const validatorNode = { ...network.nodes[0], nodeId: 'node-is-validator', displayName: 'Ivan', validator: linked('validator') }
    const notValidatorNode = { ...network.nodes[0], nodeId: 'node-not-validator', displayName: 'Nadia', validator: linked('not_validator') }
    const unknownNode = { ...network.nodes[0], nodeId: 'node-unknown-validator', displayName: 'Uma', validator: null }
    renderHome([{ ...network, nodes: [validatorNode, notValidatorNode, unknownNode] }])

    fireEvent.change(selectNamed('Validator status filter'), { target: { value: 'validator' } })
    expect(nodeHrefs()).toEqual(['/nodes/node-is-validator'])

    fireEvent.change(selectNamed('Validator status filter'), { target: { value: 'not_validator' } })
    expect(nodeHrefs()).toEqual(['/nodes/node-not-validator'])

    fireEvent.change(selectNamed('Validator status filter'), { target: { value: 'unknown' } })
    expect(nodeHrefs()).toEqual(['/nodes/node-unknown-validator'])
  })

})

/** Issue #223: the same Home list can be read as cards or as one compact table
 *  with the seven public columns, and that choice travels in the ordinary Home
 *  URL like every other filter on the surface. */
describe('Public Home card and list views', () => {
  const secondNetwork = {
    ...network,
    networkKey: 'testnet',
    displayName: 'Testnet',
    nodes: [{
      ...network.nodes[0],
      nodeId: 'node-c',
      displayName: 'Gamma',
      networkKey: 'testnet',
      currentHead: 900,
      processMemoryPercent: 90,
    }],
  }

  // A Node whose Peer count was observed once and then retained: the collection
  // failed and the snapshot went stale (story 6, design §11.2).
  const retainedNetwork = {
    ...network,
    networkKey: 'retained',
    displayName: 'Retained',
    nodes: [{
      ...network.nodes[0],
      nodeId: 'node-d',
      displayName: 'Delta',
      networkKey: 'retained',
      peers: { state: 'error', freshness: 'stale', peerCount: 2 },
    }],
  }

  // Which view is mounted has no role of its own, so the two containers are
  // told apart by their shell markers; everything inside the list is read
  // through the table roles above.
  const cardGrid = () => document.querySelector('[data-slot="node-grid"]')
  const viewTab = (name: string) => screen.getByRole('tab', { name })

  it('keeps cards by default and offers the seven public list columns instead', () => {
    renderHome([network])
    expect(cardGrid()).toBeTruthy()
    expect(listTable()).toBeNull()

    fireEvent.mouseDown(viewTab('List'))

    expect(window.location.search).toBe('?view=list')
    expect(cardGrid()).toBeNull()
    expect(columnLabels()).toEqual([
      'Node', 'Network', 'Health', 'Current Head', 'Peers', 'Process CPU', 'Process memory',
    ])
    // The default health sort puts the health the Server reports as Unknown
    // last, exactly like the metric keys (#223, story 74).
    expect(listedNames()).toEqual(['Alpha', 'Beta'])
    // QC, Locked and Committed stay card-only columns.
    expect(listTable()!.textContent).not.toContain('Locked')

    fireEvent.mouseDown(viewTab('Cards'))

    expect(cardGrid()).toBeTruthy()
    expect(listTable()).toBeNull()
    expect(window.location.search).toBe('')
  })

  it('keeps an authoritative zero and a never-observed metric apart in one column', () => {
    renderHome([network])
    fireEvent.mouseDown(viewTab('List'))
    const alpha = rowOf('Alpha')!
    const beta = rowOf('Beta')!

    expect(cellOf(alpha, 'Peers')).toBe('0')
    expect(cellOf(alpha, 'Process CPU')).toBe('12.5%')
    expect(cellOf(alpha, 'Process memory')).toBe('45.3%')
    expect(cellOf(beta, 'Peers')).toBe('Unknown')
    expect(cellOf(beta, 'Process CPU')).toBe('Unknown')
    expect(cellOf(beta, 'Process memory')).toBe('Unknown')
    // The chain header and the health word are public Server values, not zeros.
    expect(cellOf(alpha, 'Current Head')).toBe('120')
    expect(cellOf(beta, 'Current Head')).toBe('Unknown')
    expect(cellOf(alpha, 'Health')).toBe('Healthy')
    expect(cellOf(beta, 'Health')).toBe('Unknown')
    expect(cellOf(alpha, 'Network')).toBe('Mainnet')
  })

  it('qualifies a retained Peer count instead of letting the list read it as current', () => {
    window.history.replaceState({}, '', '/?view=list&sort=name')
    renderHome([network, retainedNetwork])

    // Two peers is the last good snapshot rather than a fresh reading, so the
    // cell keeps the value and says whose snapshot it is.
    const delta = rowOf('Delta')!
    expect(cellOf(delta, 'Peers')).toContain('2')
    expect(cellOf(delta, 'Peers')).toContain('last good')
    expect(cellOf(delta, 'Peers')).toContain('collection error')
    expect(cellOf(delta, 'Peers')).toContain('freshness stale')
    // A current authoritative zero and a never-observed snapshot carry no cue,
    // and a retained count never turns into one.
    expect(cellOf(rowOf('Alpha')!, 'Peers')).toBe('0')
    expect(cellOf(rowOf('Beta')!, 'Peers')).toBe('Unknown')
  })

  it('restores the list from an ordinary Home URL and reports a view it cannot honour', () => {
    window.history.replaceState({}, '', '/?view=list')
    renderHome([network])
    expect(listTable()).toBeTruthy()
    expect(cardGrid()).toBeNull()

    cleanup()
    window.history.replaceState({}, '', '/?view=grid')
    renderHome([network])

    expect(cardGrid()).toBeTruthy()
    expect(listTable()).toBeNull()
    const notice = document.querySelector('[data-slot="home-filter-notice"]')!.textContent ?? ''
    expect(notice).toContain('view "grid"')
    expect(notice).toContain('fell back to its default')
  })

  it('sorts the list with the two metric keys, Unknown last and a stable Node tie', () => {
    window.history.replaceState({}, '', '/?view=list&sort=peers')
    renderHome([network, secondNetwork])
    expect([...selectNamed('Sort').options].map((option) => option.value)).toEqual([
      'health', 'name', 'head', 'peers', 'process_cpu', 'process_memory', 'disk', 'uptime', 'attention',
    ])
    // Alpha and Gamma both report an authoritative 0 against Beta's never
    // observed Peer snapshot: the tie keeps the Node identity order and the
    // Unknown value stays last.
    expect(listedNames()).toEqual(['Alpha', 'Gamma', 'Beta'])

    fireEvent.change(selectNamed('Sort'), { target: { value: 'process_memory' } })

    expect(window.location.search).toBe('?sort=process_memory&view=list')
    expect(listedNames()).toEqual(['Gamma', 'Alpha', 'Beta'])
  })

  it('sorts by Disk, Uptime and Attention First through the one sort entry, in both views', () => {
    // Every Node's Host storage percentage disagrees with its own Node data
    // ratio, and each key ranks these three Nodes differently, so no two of them
    // can be satisfied by one reading of the same field (#242).
    const busy = {
      ...network.nodes[0],
      nodeId: 'node-busy',
      displayName: 'Busy',
      health: 'healthy',
      nodeDataDirectorySizeBytes: 90,
      nodeDataDirectoryCapacityBytes: 100,
      hostStoragePercent: 5,
      processUptimeMs: 1_000,
    }
    const empty = {
      ...network.nodes[0],
      nodeId: 'node-empty',
      displayName: 'Empty',
      health: 'unknown',
      nodeDataDirectorySizeBytes: 0,
      nodeDataDirectoryCapacityBytes: 100,
      hostStoragePercent: 95,
      processUptimeMs: 356_400_000,
    }
    const gone = {
      ...network.nodes[1],
      nodeId: 'node-gone',
      displayName: 'Gone',
      health: 'unhealthy',
      nodeDataDirectorySizeBytes: null,
      nodeDataDirectoryCapacityBytes: 100,
      processUptimeMs: 0,
    }
    window.history.replaceState({}, '', '/?sort=disk')
    renderHome([{ ...network, nodes: [empty, gone, busy] }])

    // The entry offers the three new keys under the six delivered ones, with no
    // direction control of its own.
    expect([...selectNamed('Sort').options].map((option) => option.textContent)).toEqual([
      'Health', 'Name', 'Current Head', 'Peers', 'Process CPU', 'Process memory', 'Disk', 'Uptime', 'Attention First',
    ])
    // The grid holds one link per card and nothing else, so the card order is
    // read through the links' own role and name.
    const cardNames = () => within(cardGrid() as HTMLElement).getAllByRole('link').map((link) => link.getAttribute('aria-label'))
    // Disk reads the Node data ratio: the 90% directory leads, the real zero
    // follows, and the ratio the Server never attested sorts last.
    expect(cardNames()).toEqual(['Busy', 'Empty', 'Gone'])
    const diskValue = (name: string) => cardOf(nodeCardLink(name))
      .querySelector('[data-slot="resource-cell"][data-metric="node-data"] [data-slot="resource-value"]')?.textContent
    expect(diskValue('Busy')).toBe('90.0%')
    expect(diskValue('Empty')).toBe('0.0%')
    expect(diskValue('Gone')).toBe('Unknown')

    // Uptime is the process uptime, and it is a different ranking: the long-lived
    // process leads, a freshly restarted one stays a zero, and an unobserved
    // process is last.
    fireEvent.change(selectNamed('Sort'), { target: { value: 'uptime' } })
    expect(window.location.search).toBe('?sort=uptime')
    expect(cardNames()).toEqual(['Empty', 'Busy', 'Gone'])

    // Attention First is its own order over the same Server health value:
    // Unhealthy, then Unknown, then Healthy — the reverse of the delivered
    // Health key's grouping, and no health is turned into another.
    fireEvent.change(selectNamed('Sort'), { target: { value: 'attention' } })
    expect(window.location.search).toBe('?sort=attention')
    expect(cardNames()).toEqual(['Gone', 'Empty', 'Busy'])
    fireEvent.change(selectNamed('Sort'), { target: { value: 'health' } })
    expect(cardNames()).toEqual(['Gone', 'Busy', 'Empty'])

    // The list reads the same matching collection and the same order.
    fireEvent.change(selectNamed('Sort'), { target: { value: 'attention' } })
    fireEvent.mouseDown(viewTab('List'))
    expect(window.location.search).toBe('?sort=attention&view=list')
    expect(listedNames()).toEqual(['Gone', 'Empty', 'Busy'])
  })
})

/** Issue #232: Home keeps one row of toolbar entries — the Network pills, one
 *  Sort & filter entry, the card/list choice, and the search control — and every
 *  control behind the Sort & filter entry stays out of the row, and out of the
 *  tab walk, until a reader opens the surface that holds it. Issue #241 gave
 *  that search control its own two shapes: the row's compact field, and the
 *  entry that opens the same field for a toolbar without the width for one. */
describe('Public Home toolbar entries', () => {
  const secondNetwork = {
    ...network,
    networkKey: 'testnet',
    displayName: 'Testnet',
    nodes: [{ ...network.nodes[0], nodeId: 'node-c', displayName: 'Gamma', networkKey: 'testnet', currentHead: 900 }],
  }
  const filterEntry = /^Sort & filter$/
  const trigger = (name: string | RegExp) => screen.getByRole('button', { name })
  const badge = () => document.querySelector('[data-slot="home-filter-count"]')
  const surface = (label: string) => screen.queryByRole('dialog', { name: label })

  it('keeps the controls of a closed entry out of the row and out of the tab walk', () => {
    renderHome([network])

    // A control the reader cannot see is not in the reading order either: the
    // closed entry holds no field and no surface, while the row's own field is
    // the row's own control rather than something behind that entry (#241).
    expect(screen.queryAllByRole('combobox')).toEqual([])
    expect(within(searchSlots().collapsed as HTMLElement).queryByRole('searchbox')).toBeNull()
    expect(surface('Sort and filters')).toBeNull()
    expect(surface('Search Active Nodes')).toBeNull()
    expect(inlineField().getAttribute('data-slot')).toBe('home-search-field')

    // What the row does keep: the Network pills, the two entries, and the view.
    expect(screen.getByRole('tablist', { name: 'Network filter' })).toBeTruthy()
    expect(screen.getByRole('tablist', { name: 'View' })).toBeTruthy()
    expect(trigger(filterEntry).getAttribute('data-slot')).toBe('home-filter-trigger')
    expect(trigger('Search Active Nodes').getAttribute('data-slot')).toBe('home-search-trigger')
    // The count is part of the row, not of the surface: the row states it while
    // the entry is closed, and it draws the chip only while there is a count to
    // state — a chip reading zero is a chip that says nothing.
    expect(badge()).toBeNull()
    expect(document.getElementById('home-status-filter-count')?.textContent).toBe('0 of 2 status filters on')

    fireEvent.click(trigger(filterEntry))
    const opened = surface('Sort and filters') as HTMLElement
    expect(opened).toBeTruthy()
    // The three controls belong to the surface, so the surface is what this reads
    // them through rather than the whole page.
    const surfaceQueries = within(opened)
    expect(surfaceQueries.getByRole('combobox', { name: 'Sort' })).toBeTruthy()
    expect(surfaceQueries.getByRole('combobox', { name: 'Health filter' })).toBeTruthy()
    expect(surfaceQueries.getByRole('combobox', { name: 'Validator status filter' })).toBeTruthy()
    expect(trigger(filterEntry).getAttribute('aria-expanded')).toBe('true')
    // Nothing to clear while both filters are the default, so the surface draws
    // no footer at all: no sentence about a filter that is not on, and no action
    // drawn disabled for a reader to wonder about (#246).
    expect(surfaceQueries.queryByRole('button', { name: 'Clear filters' })).toBeNull()
    expect(opened.querySelector('[data-slot="home-filter-footer"]')).toBeNull()
  })

  it('stacks the three controls as one-line rows over one shared label column (#246)', () => {
    renderHome([network])
    fireEvent.click(trigger(filterEntry))
    const dialog = surface('Sort and filters') as HTMLElement

    // Three rows rather than three stacked labels: every control shares its line
    // with the word that names it, so the surface is about half the height it was
    // and the three selects line up on one left edge.
    const rows = Array.from(dialog.querySelectorAll('label'))
    expect(rows.map(row => row.querySelector('span')?.textContent)).toEqual(['Sort by', 'Health', 'Validator'])
    rows.forEach(row => {
      expect(row.className).toContain('items-center')
      expect(row.querySelector('span')?.className).toContain('w-[5.5rem]')
      // The compact layout does not shrink the control itself: the 44px target
      // and the accessible name the row already had both stay.
      expect(row.querySelector('select')?.className).toContain('min-h-11 flex-1')
    })

    // One low-contrast rule separates the order from the two filters, and it sits
    // between them rather than above or below the group.
    const separator = dialog.querySelector('[data-slot="home-filter-separator"]')
    const selects = Array.from(dialog.querySelectorAll('select'))
    expect(selects).toHaveLength(3)
    expect(separator).toBeTruthy()
    expect(selects[0].compareDocumentPosition(separator as Node) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(selects[1].compareDocumentPosition(separator as Node) & Node.DOCUMENT_POSITION_PRECEDING).toBeTruthy()

    // The surface stays the medium band's own 288px, and the names a reader
    // reached it by did not follow the layout.
    expect(dialog.className).toContain('w-72')
    const surfaceQueries = within(dialog)
    expect(surfaceQueries.getByRole('combobox', { name: 'Sort' })).toBeTruthy()
    expect(surfaceQueries.getByRole('combobox', { name: 'Health filter' })).toBeTruthy()
    expect(surfaceQueries.getByRole('combobox', { name: 'Validator status filter' })).toBeTruthy()
  })

  it('states the order in force only while it is not the default one (#246)', () => {
    renderHome([network])
    fireEvent.click(trigger(filterEntry))

    // The default order is the Health order the card has always led with, so the
    // entry states nothing about it: its one fixed-width track keeps the entry's
    // own name, no second description is added to the control a reader reaches
    // without opening anything, and the width that track occupies is the same
    // width every other state occupies (#246).
    const sortTrack = () => document.querySelector('[data-slot="home-filter-sort"]')
    expect(sortTrack()?.textContent?.trim()).toBe('Sort & filter')
    expect(document.querySelector('[data-slot="home-sort-state"]')).toBeNull()
    expect(trigger(filterEntry).getAttribute('aria-describedby')).toBe('home-status-filter-count')

    fireEvent.change(selectNamed('Sort'), { target: { value: 'process_cpu' } })
    // The chip names the order where the row has the width for a word, and the
    // description names it with the direction it sorts in at every width, so the
    // word is never the only way to learn which order is in force.
    expect(document.querySelector('[data-slot="home-filter-sort"]')?.textContent?.replace(/\s+/g, ' ')).toBe('Sort: CPU')
    expect(document.getElementById('home-sort-state')?.textContent)
      .toBe('Sort is Process CPU: highest process CPU first, never-observed Nodes last.')
    expect(trigger(filterEntry).getAttribute('aria-describedby')).toBe('home-status-filter-count home-sort-state')
    // The track holds one fixed width instead of the width of its own word, so
    // changing from one order to another moves nothing (the entry states its name
    // in the very same track), and the surface hangs from the entry's end edge —
    // the edge the toolbar pins — so the entry stating an order cannot slide its
    // own surface sideways (#246).
    expect(sortTrack()?.className).toContain('w-[5.5rem]')
    expect(sortTrack()?.className).toContain('shrink-0')
    expect((surface('Sort and filters') as HTMLElement).className).toContain('right-0')
    // A sort choice is not a status filter: the badge counts the two filters only.
    expect(badge()).toBeNull()
    expect(surface('Sort and filters')).toBeTruthy()

    // Back to the default and the extra state leaves with it, while the surface
    // the reader is working in stays open.
    fireEvent.change(selectNamed('Sort'), { target: { value: 'health' } })
    expect(sortTrack()?.textContent?.trim()).toBe('Sort & filter')
    expect(document.querySelector('[data-slot="home-sort-state"]')).toBeNull()
    expect(trigger(filterEntry).getAttribute('aria-describedby')).toBe('home-status-filter-count')
    expect(surface('Sort and filters')).toBeTruthy()

    // Anchoring belongs to the control, not to the surface: the search control
    // states nothing inside itself, so its surface keeps hanging from the edge a
    // reader reads from.
    fireEvent.click(trigger('Search Active Nodes'))
    expect(surface('Search Active Nodes')).toBeTruthy()
    expect((surface('Search Active Nodes') as HTMLElement).className).not.toContain('right-0')
  })

  it('gives the row two ends: the Network group and one right-hand group of three controls', () => {
    renderHome([network])

    const row = document.querySelector('[data-slot="home-toolbar"]') as HTMLElement
    const group = row.querySelector('[data-slot="home-toolbar-actions"]') as HTMLElement
    // The row is two groups rather than four controls with three of them pushed
    // to the right by their own margins: the Network pills take the left-hand
    // end, and the search control, the Sort & filter entry and the card/list
    // choice share one right-hand group, in the order a reader reaches them.
    // Each group names itself, so the reading is of the row as it is built
    // rather than of a name this test supplies for a missing one.
    expect(Array.from(row.children).map(child => child.getAttribute('data-slot')))
      .toEqual(['network-group', 'home-toolbar-actions'])
    // The search slot and the view choice carry their own slots; the entry in
    // between is the Sort & filter wrapper, which holds the trigger and the
    // surface the row states its count beside.
    expect(Array.from(group.children).map(child => child.getAttribute('data-slot')))
      .toEqual(['home-search-slot', 'sort-filter-entry', 'tabs'])
    expect(group.querySelector('[data-slot="home-filter-trigger"]')).toBeTruthy()
    expect(group.querySelector('[aria-label="View"]')).toBeTruthy()
  })

  it('opens one entry at a time and closes the open one from its own trigger', () => {
    renderHome([network])

    fireEvent.click(trigger(filterEntry))
    fireEvent.click(trigger('Search Active Nodes'))

    expect(surface('Sort and filters')).toBeNull()
    expect(surface('Search Active Nodes')).toBeTruthy()

    fireEvent.click(trigger('Search Active Nodes'))
    expect(surface('Search Active Nodes')).toBeNull()
  })

  it('counts only the status filters that differ from the default', () => {
    renderHome([network])

    fireEvent.click(trigger(filterEntry))
    // Zero is not a count this chip states: with both filters at their default
    // the entry draws no chip at all, and the count still reaches a reader that
    // cannot see one through the entry's own description.
    expect(badge()).toBeNull()
    expect(document.getElementById('home-status-filter-count')?.textContent).toBe('0 of 2 status filters on')

    fireEvent.change(selectNamed('Health filter'), { target: { value: 'healthy' } })
    expect(badge()?.textContent).toBe('1')
    // The entry still reads exactly "Sort & filter"; the count reaches a
    // screen reader through the description instead of the name.
    expect(trigger(filterEntry).getAttribute('aria-describedby')).toBe('home-status-filter-count')
    expect(document.getElementById('home-status-filter-count')?.textContent).toBe('1 of 2 status filters on')

    fireEvent.change(selectNamed('Validator status filter'), { target: { value: 'unknown' } })
    expect(badge()?.textContent).toBe('2')

    // Choosing a value is not a reason to close the surface the reader is in.
    expect(surface('Sort and filters')).toBeTruthy()

    fireEvent.change(selectNamed('Health filter'), { target: { value: 'all' } })
    fireEvent.change(selectNamed('Validator status filter'), { target: { value: 'all' } })
    expect(badge()).toBeNull()
    expect(trigger(filterEntry).getAttribute('aria-describedby')).toBe('home-status-filter-count')
    expect(document.getElementById('home-status-filter-count')?.textContent).toBe('0 of 2 status filters on')
  })

  it('clears only the two status filters and leaves the rest of the reading alone', () => {
    window.history.replaceState({}, '', '/?network=testnet&q=Gamma&sort=name&health=unhealthy&validator=unknown&view=list')
    renderHome([network, secondNetwork])

    fireEvent.click(trigger(filterEntry))
    // The footer exists exactly while there is something to clear: the sentence,
    // the action and the count all arrive together, and none of them is drawn
    // disabled for a reader to wonder about (#246).
    expect(document.querySelector('[data-slot="home-filter-footer"]')?.textContent).toContain('2 of 2 status filters on')
    expect(badge()?.textContent).toBe('2')
    fireEvent.click(screen.getByRole('button', { name: 'Clear filters' }))

    expect(window.location.search).toBe('?network=testnet&q=Gamma&sort=name&view=list')
    expect(selectNamed('Health filter').value).toBe('all')
    expect(selectNamed('Validator status filter').value).toBe('all')
    expect(badge()).toBeNull()
    expect(surface('Sort and filters')).toBeTruthy()
  })

  it('keeps the search entry marked while a search is in force and returns focus when it closes', async () => {
    window.history.replaceState({}, '', '/?q=Gamma')
    renderHome([network])
    const searchEntry = trigger('Search Active Nodes')

    // The marker is on the entry itself, so a narrowed list is never mistaken
    // for an empty deployment while the field is out of sight — drawn beside the
    // icon rather than carried by the accent colour alone, and named for a
    // reader who sees neither.
    expect(searchEntry.getAttribute('data-search-state')).toBe('on')
    expect(document.querySelector('[data-slot="home-search-active"]')).toBeTruthy()
    expect(searchEntry.getAttribute('aria-describedby')).toBe('home-search-state')
    expect(document.getElementById('home-search-state')?.textContent).toBe('Search is on: Gamma')

    fireEvent.click(searchEntry)
    const box = collapsedField()
    await waitFor(() => expect(document.activeElement).toBe(box))
    expect(box.value).toBe('Gamma')

    // Escape closes the surface without touching the word: the query is the
    // reader's filter, and only Clear search removes it (#241).
    fireEvent.keyDown(document, { key: 'Escape' })
    expect(within(searchSlots().collapsed as HTMLElement).queryByRole('searchbox')).toBeNull()
    expect(document.activeElement).toBe(searchEntry)
    expect(window.location.search).toBe('?q=Gamma')

    // Reopening hands the reader back the text they had, and the marker stays.
    fireEvent.click(searchEntry)
    expect(collapsedField().value).toBe('Gamma')
    expect(searchEntry.getAttribute('data-search-state')).toBe('on')
    expect(document.querySelector('[data-slot="home-search-active"]')).toBeTruthy()
  })

  it('clears the search from inside its own entry and keeps the Network selection', () => {
    window.history.replaceState({}, '', '/?network=testnet&q=Gamma')
    renderHome([network, secondNetwork])
    fireEvent.click(trigger('Search Active Nodes'))

    fireEvent.click(within(searchSlots().collapsed as HTMLElement).getByRole('button', { name: 'Clear search' }))

    expect(window.location.search).toBe('?network=testnet')
    expect(collapsedField().value).toBe('')
    expect(trigger('Search Active Nodes').getAttribute('data-search-state')).toBe('off')
    expect(document.querySelector('[data-slot="home-search-active"]')).toBeNull()
    expect(trigger('Search Active Nodes').getAttribute('aria-describedby')).toBeNull()
  })

  it('reads one query from the row field and from the entry that opens it', async () => {
    window.history.replaceState({}, '', '/?network=testnet&q=Gamma')
    renderHome([network, secondNetwork])

    // The row's own field is the same labelled control the entry opens, and it
    // states the flag in force on itself: a narrowed list is never mistaken for
    // an empty deployment while the reader has the field in their hand.
    const field = inlineField()
    expect(field.getAttribute('aria-label')).toBe('Search Active Nodes')
    expect(field.value).toBe('Gamma')
    expect(field.getAttribute('data-search-state')).toBe('on')
    expect(field.getAttribute('aria-describedby')).toBe('home-search-state')
    expect(document.getElementById('home-search-state')?.textContent).toBe('Search is on: Gamma')

    // Typing in the row's field is what the entry's field then reads: one
    // query, one filter, one URL — whichever shape the reader was given.
    fireEvent.change(field, { target: { value: 'Alpha' } })
    await waitFor(() => expect(window.location.search).toBe('?network=testnet&q=Alpha'))
    fireEvent.click(trigger('Search Active Nodes'))
    expect(collapsedField().value).toBe('Alpha')
    expect(inlineField().value).toBe('Alpha')

    // Closing the entry leaves both shapes on the same word and the same filter.
    fireEvent.keyDown(document, { key: 'Escape' })
    expect(within(searchSlots().collapsed as HTMLElement).queryByRole('searchbox')).toBeNull()
    expect(inlineField().value).toBe('Alpha')
    expect(inlineField().getAttribute('data-search-state')).toBe('on')
    expect(window.location.search).toBe('?network=testnet&q=Alpha')

    // And the same holds the other way round: the entry writes the query the
    // row's field reads, with nothing lost on the way.
    fireEvent.click(trigger('Search Active Nodes'))
    fireEvent.change(collapsedField(), { target: { value: 'Beta' } })
    expect(inlineField().value).toBe('Beta')
    await waitFor(() => expect(window.location.search).toBe('?network=testnet&q=Beta'))
    fireEvent.keyDown(document, { key: 'Escape' })
    expect(inlineField().value).toBe('Beta')
    expect(window.location.search).toBe('?network=testnet&q=Beta')
  })

  it("keeps the reader's word when Escape is pressed in the row's own field", () => {
    window.history.replaceState({}, '', '/?q=Gamma')
    renderHome([network])

    // Escape hands nothing back here: there is no surface to close, and the
    // reader's search is not something a retreat key gets to clear.
    fireEvent.keyDown(inlineField(), { key: 'Escape' })

    expect(inlineField().value).toBe('Gamma')
    expect(window.location.search).toBe('?q=Gamma')
    expect(inlineField().getAttribute('data-search-state')).toBe('on')
  })

  it("clears only the query from the row's own field and keeps the Network selection", () => {
    window.history.replaceState({}, '', '/?network=testnet&q=Gamma')
    renderHome([network, secondNetwork])

    fireEvent.click(within(searchSlots().inline as HTMLElement).getByRole('button', { name: 'Clear search' }))

    expect(window.location.search).toBe('?network=testnet')
    expect(inlineField().value).toBe('')
    expect(within(searchSlots().collapsed as HTMLElement).queryByRole('searchbox')).toBeNull()
    expect(inlineField().getAttribute('data-search-state')).toBe('off')
    expect(inlineField().getAttribute('aria-describedby')).toBeNull()
  })

  /** Issue #241: which of the two shapes the toolbar has room for is CSS's
   *  decision on the slot's own width, but a reader who is inside the search
   *  control when a resize replaces that shape must not lose the keyboard, so
   *  the component measures the same room and hands the focus on. jsdom lays
   *  nothing out and has no ResizeObserver, so a test states the slot's width and
   *  drives the observation by hand: the room, the flip, and the reader's place
   *  are the whole of what is simulated. */
  it("hands the reader's place to the search shape that replaces the one they were in", async () => {
    class FakeResizeObserver {
      static callbacks: Array<() => void> = []
      constructor(callback: () => void) {
        FakeResizeObserver.callbacks.push(callback)
      }
      observe() {}
      disconnect() {}
    }
    vi.stubGlobal('ResizeObserver', FakeResizeObserver)
    const setRoom = (width: number) => {
      const slot = document.querySelector('[data-slot="home-search-slot"]') as HTMLElement
      Object.defineProperty(slot, 'clientWidth', { value: width, configurable: true })
      FakeResizeObserver.callbacks.forEach((callback) => callback())
    }

    window.history.replaceState({}, '', '/')
    renderHome([network])
    const entry = trigger('Search Active Nodes')
    fireEvent.click(entry)
    const field = collapsedField()
    fireEvent.change(field, { target: { value: 'Gamma' } })
    await waitFor(() => expect(document.activeElement).toBe(field))

    // The toolbar grows room for the field while the reader is in the entry's
    // surface. The surface is not left open behind a shape that is no longer
    // displayed; the field that took over takes the reader's place, and the word
    // and the filter it stands for are exactly as the reader left them.
    await act(async () => setRoom(400))
    const rowField = inlineField()
    await waitFor(() => expect(document.activeElement).toBe(rowField))
    expect(rowField.value).toBe('Gamma')
    expect(surface('Search Active Nodes')).toBeNull()
    await waitFor(() => expect(window.location.search).toBe('?q=Gamma'))

    // And back the other way: the entry speaks for the field that went out of
    // sight and holds the reader's place, without the word being touched.
    await act(async () => setRoom(100))
    await waitFor(() => expect(document.activeElement).toBe(entry))
    expect(inlineField().value).toBe('Gamma')
    expect(window.location.search).toBe('?q=Gamma')
    vi.unstubAllGlobals()
  })
})

/** Home arriving from Node Detail: the departure a Node link recorded travels in
 *  the history entry's own state, which is what react-router reads back. */
function renderReturnArrival(networks: PublicNetwork[], search: string, departure: HomeReturnState | null) {
  const router = createMemoryRouter(
    [{
      path: '/',
      element: (
        <HomeDashboard
          networks={networks}
          realtimeStatus="connected"
          online
          resetting={false}
          error={null}
          loading={false}
        />
      ),
    }],
    { initialEntries: [{ pathname: '/', search, state: departure }] },
  )
  const view = render(<RouterProvider router={router} />)
  return { router, view }
}

const returnNotice = () => document.querySelector('[data-slot="home-return-notice"]')

/** A departure as a Node link writes it. Only the document that wrote it reads
 *  it back, so the token has to be this document's own. */
function departureState(nodeId: string, search: string, index: number, count: number): HomeReturnState {
  return homeReturnState(nodeId, new URLSearchParams(search), index, count, homeReturnTab())
}

/** The same arrival, in a Home whose projection a refetch replaces a moment
 *  later: what the reader is owed has to survive that second reading. */
function renderRefetchedArrival(
  search: string,
  departure: HomeReturnState | null,
  options: { networks?: PublicNetwork[]; refreshing?: boolean } = {},
) {
  const router = createMemoryRouter(
    [
      {
        path: '/',
        element: (
          <RefetchedHome
            initialNetworks={options.networks ?? [network]}
            initialRefreshing={options.refreshing ?? false}
          />
        ),
      },
    ],
    { initialEntries: [{ pathname: '/', search, state: departure }] },
  )
  render(<RouterProvider router={router} />)
}

/** Two Nodes of the same Network, with the Peer counts that decide their order. */
function peersNetwork(alphaPeers: number, betaPeers: number): PublicNetwork {
  const withPeers = (nodeId: string, peerCount: number): PublicNode => ({
    ...network.nodes.find((node) => node.nodeId === nodeId)!,
    peers: { state: 'ok', freshness: 'current', peerCount },
  })
  return { ...network, nodes: [withPeers('node-a', alphaPeers), withPeers('node-b', betaPeers)] }
}

let refetch: ((networks: PublicNetwork[]) => void) | null = null
let settleRefetch: ((refreshing: boolean) => void) | null = null

/** The arrival in a Home that is still refetching: whatever it puts on screen
 *  first is the reading the reader is owed a place in only provisionally. */
function RefetchedHome({
  initialNetworks,
  initialRefreshing,
}: {
  initialNetworks: PublicNetwork[]
  initialRefreshing: boolean
}) {
  const [networks, setNetworks] = useState<PublicNetwork[]>(initialNetworks)
  const [refreshing, setRefreshing] = useState(initialRefreshing)
  refetch = setNetworks
  settleRefetch = setRefreshing
  return (
    <HomeDashboard
      networks={networks}
      realtimeStatus="connected"
      online
      resetting={false}
      error={null}
      loading={false}
      refreshing={refreshing}
    />
  )
}

describe('Public Home same-tab return', () => {
  const revealed = vi.fn()
  beforeEach(() => {
    revealed.mockClear()
    // jsdom has no scrolling: the reveal is observed as the contract it makes.
    Object.defineProperty(Element.prototype, 'scrollIntoView', {
      configurable: true,
      writable: true,
      value: revealed,
    })
  })

  it('lands on the Node, in the reading it was left from, with no place in the URL', async () => {
    const { router } = renderReturnArrival(
      [network],
      '?view=list&sort=name',
      departureState('node-b', 'view=list&sort=name', 1, 2),
    )

    expect(listTable()).toBeTruthy()
    const returned = rowOf('Beta')!.querySelector('[data-slot="node-link"]')
    await waitFor(() => expect(document.activeElement).toBe(returned))
    expect(revealed).toHaveBeenCalledWith({ block: 'center' })
    expect(returnNotice()).toBeNull()
    // A place is never a URL parameter, and the departure is consumed on
    // arrival so this entry can never be inherited a second time.
    expect(router.state.location.search).toBe('?view=list&sort=name')
    await waitFor(() => expect(router.state.location.state).toBeNull())
  })

  it('follows the Node ID rather than a place the refreshed list has moved past', async () => {
    // The reader left from place 1 of 2, which the refreshed order now gives to
    // Alpha; the Node they left from still decides where the return lands.
    renderReturnArrival(
      [network],
      '?view=list&sort=name',
      departureState('node-b', 'view=list&sort=name', 0, 2),
    )

    const returned = rowOf('Beta')!.querySelector('[data-slot="node-link"]')
    await waitFor(() => expect(document.activeElement).toBe(returned))
  })

  it('names the Node the restored filters hide and shows the nearest match instead', async () => {
    renderReturnArrival(
      [network],
      '?view=list&q=Alpha',
      departureState('node-b', 'view=list&q=Alpha', 1, 2),
    )

    await waitFor(() => expect(returnNotice()?.textContent).toContain('Beta is no longer shown by these filters'))
    expect(returnNotice()?.textContent).toContain('Showing the nearest matching Active Node instead.')
    // A return into a narrowed list never leaves the reader on blank space: the
    // rows that are still listed are shown, and the nearest one is revealed.
    expect(listedNames()).toEqual(['Alpha'])
    expect(document.activeElement).toBe(rowOf('Alpha')!.querySelector('[data-slot="node-link"]'))
  })

  it('lands again, with its notice, when the refetch settles without the Node', async () => {
    // Home paints the projection it already held, and the authoritative refetch
    // can settle a moment later without the Node the reader left from: the
    // reader is not left beside a row that is gone.
    renderRefetchedArrival('?view=list&sort=name', departureState('node-b', 'view=list&sort=name', 1, 2))
    await waitFor(() =>
      expect(document.activeElement).toBe(rowOf('Beta')!.querySelector('[data-slot="node-link"]')),
    )
    expect(returnNotice()).toBeNull()

    const without = network.nodes.filter((entry) => entry.nodeId !== 'node-b')
    await act(async () => refetch!([{ ...network, nodes: without }]))

    await waitFor(() => expect(returnNotice()?.textContent).toContain('node-b is no longer in this view'))
    expect(returnNotice()?.textContent).toContain('Showing the nearest Active Node instead.')
    expect(listedNames()).toEqual(['Alpha'])
    expect(document.activeElement).toBe(rowOf('Alpha')!.querySelector('[data-slot="node-link"]'))
  })

  it('places the reader again when the reading that settles has moved their Node', async () => {
    // Home paints the projection it already held while the authoritative
    // refetch is still in flight, and that reading can settle with the same
    // Nodes in another order. Beta leads on Peers to begin with, so the reader
    // returns to the first of the two rows.
    const cached = peersNetwork(1, 5)
    const settled = peersNetwork(5, 1)
    renderRefetchedArrival(
      '?view=list&sort=peers',
      departureState('node-b', 'view=list&sort=peers', 0, 2),
      { networks: [cached], refreshing: true },
    )
    await waitFor(() => expect(listedNames()).toEqual(['Beta', 'Alpha']))
    expect(document.activeElement).toBe(rowOf('Beta')!.querySelector('[data-slot="node-link"]'))
    expect(revealed).toHaveBeenCalledTimes(1)
    expect(returnNotice()).toBeNull()

    await act(async () => {
      refetch!([settled])
      settleRefetch!(false)
    })

    // Beta is last now: the reader is placed on it again, with no notice to say
    // so, because the place they were given is not where their Node is.
    await waitFor(() => expect(listedNames()).toEqual(['Alpha', 'Beta']))
    await waitFor(() => expect(revealed).toHaveBeenCalledTimes(2))
    expect(returnNotice()).toBeNull()
    expect(document.activeElement).toBe(rowOf('Beta')!.querySelector('[data-slot="node-link"]'))

    // Once the reading has settled the arrival is done: a live reorder the
    // reader is watching never moves them around.
    await act(async () => refetch!([cached]))
    await waitFor(() => expect(listedNames()).toEqual(['Beta', 'Alpha']))
    expect(revealed).toHaveBeenCalledTimes(2)
    expect(document.activeElement).toBe(rowOf('Beta')!.querySelector('[data-slot="node-link"]'))
  })

  it('reports a Node this Home cannot show by the ID it was left from', async () => {
    renderReturnArrival(
      [network],
      '?view=list',
      departureState('node-gone', 'view=list', 1, 2),
    )

    await waitFor(() => expect(returnNotice()?.textContent).toContain('node-gone is no longer in this view'))
    expect(document.activeElement).toBe(rowOf('Beta')!.querySelector('[data-slot="node-link"]'))
  })

  it('drops the notice as soon as the reader asks for a different reading', async () => {
    renderReturnArrival(
      [network],
      '?view=list&q=Alpha',
      departureState('node-b', 'view=list&q=Alpha', 1, 2),
    )
    await waitFor(() => expect(returnNotice()).toBeTruthy())

    fireEvent.change(selectNamed('Health filter'), { target: { value: 'healthy' } })

    expect(returnNotice()).toBeNull()
  })

  it('inherits nothing on a fresh Home', () => {
    renderReturnArrival([network], '?view=list', null)

    expect(returnNotice()).toBeNull()
    expect(revealed).not.toHaveBeenCalled()
    expect(document.activeElement).toBe(document.body)
  })
})

