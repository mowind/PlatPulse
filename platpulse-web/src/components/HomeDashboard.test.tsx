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
      currentHead: 120, transactionCountAtCurrentHead: 12345, historicalHighWatermark: 120, hostCpuPercent: 14.5, networkReferenceHead: 120,
      networkReferenceConfidence: 'high', freshness: 'current', resyncProgress: null,
      peers: { state: 'current', freshness: 'current', peerCount: 0 }, validator: null,
    },
    {
      nodeId: 'node-b', displayName: 'Beta', networkKey: 'mainnet', health: 'unknown', healthReason: 'Never observed',
      rpcState: 'unknown', syncState: 'unknown', consensusState: 'unknown', processState: 'unknown', resyncState: 'unknown',
      currentHead: null, transactionCountAtCurrentHead: null, historicalHighWatermark: null, hostCpuPercent: null, networkReferenceHead: null,
      networkReferenceConfidence: 'unknown', freshness: 'unknown', resyncProgress: null,
      peers: { state: 'unknown', freshness: 'unknown', peerCount: null }, validator: null,
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

    expect(screen.getByText('Published Nodes').nextElementSibling?.textContent).toBe('2')
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

  it('keeps summary cards to marker, title, and number with a compact shell', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    expect(screen.queryByText('Active Nodes on Home')).toBeNull()
    expect(screen.queryByText('Server-owned health')).toBeNull()
    expect(screen.queryByText('Unknown and degraded included')).toBeNull()
    expect(screen.queryByText('Published Network groups')).toBeNull()
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
    expect(screen.getByText('No published Nodes in this view.')).toBeTruthy()
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
    expect(screen.queryByText('No published Nodes in this view.')).toBeNull()
  })

  it('preserves an authoritative empty projection when a refresh fails', () => {
    render(<BrowserRouter><HomeDashboard networks={[]} realtimeStatus="connected" online resetting={false} error="Unable to load published Nodes" hasLastGood loading={false} /></BrowserRouter>)
    expect(screen.getByText('Unable to load published Nodes')).toBeTruthy()
    expect(screen.getByText('No published Nodes in this view.')).toBeTruthy()
    expect(screen.getByText('Published Nodes').nextElementSibling?.textContent).toBe('0')
  })
})
