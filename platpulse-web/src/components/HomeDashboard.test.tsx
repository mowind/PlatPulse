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
      currentHead: 120, historicalHighWatermark: 120, hostCpuPercent: 14.5, networkReferenceHead: 120,
      networkReferenceConfidence: 'high', freshness: 'current', resyncProgress: null,
      peers: { state: 'current', freshness: 'current', peerCount: 0 }, validator: null,
    },
    {
      nodeId: 'node-b', displayName: 'Beta', networkKey: 'mainnet', health: 'unknown', healthReason: 'Never observed',
      rpcState: 'unknown', syncState: 'unknown', consensusState: 'unknown', processState: 'unknown', resyncState: 'unknown',
      currentHead: null, historicalHighWatermark: null, hostCpuPercent: null, networkReferenceHead: null,
      networkReferenceConfidence: 'unknown', freshness: 'unknown', resyncProgress: null,
      peers: { state: 'unknown', freshness: 'unknown', peerCount: null }, validator: null,
    },
  ],
} satisfies PublicNetwork

afterEach(cleanup)

describe('Public Home dashboard', () => {
  it('summarizes Server-owned Nodes and preserves authoritative zero peer count', () => {
    render(<BrowserRouter><HomeDashboard networks={[network]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    expect(screen.getByText('Published Nodes').nextElementSibling?.textContent).toBe('2')
    expect(screen.getByText('Healthy Nodes').nextElementSibling?.textContent).toBe('1')
    expect(screen.getByText('Attention').nextElementSibling?.textContent).toBe('1')
    const alphaCard = screen.getByRole('link', { name: 'Alpha' }).closest('article')
    expect(within(alphaCard!).getByText('Peer Count').nextElementSibling?.textContent).toBe('0')
    expect(screen.getByText('Current')).toBeTruthy()
  })

  it('filters by Network and sorts by supported operational fields', () => {
    const secondNetwork = { ...network, networkKey: 'testnet', displayName: 'Testnet', nodes: [{ ...network.nodes[0], nodeId: 'node-c', displayName: 'Gamma', networkKey: 'testnet', currentHead: 900 }] }
    render(<BrowserRouter><HomeDashboard networks={[network, secondNetwork]} realtimeStatus="connected" online resetting={false} error={null} loading={false} /></BrowserRouter>)

    fireEvent.click(screen.getByRole('button', { name: 'Testnet' }))
    expect(screen.getByRole('link', { name: 'Gamma' })).toBeTruthy()
    expect(screen.queryByRole('link', { name: 'Alpha' })).toBeNull()

    fireEvent.click(screen.getByRole('button', { name: 'All Networks' }))
    fireEvent.change(screen.getByRole('combobox', { name: 'Sort' }), { target: { value: 'head' } })
    const links = screen.getAllByRole('link').filter((link) => ['Alpha', 'Beta', 'Gamma'].includes(link.textContent ?? ''))
    expect(links.map((link) => link.textContent)).toEqual(['Gamma', 'Alpha', 'Beta'])
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
    const nodeLinks = screen.getAllByRole('link').filter((link) => ['Error Node', 'Alpha'].includes(link.textContent ?? ''))
    expect(nodeLinks.map((link) => link.textContent)).toEqual(['Error Node', 'Alpha'])
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
