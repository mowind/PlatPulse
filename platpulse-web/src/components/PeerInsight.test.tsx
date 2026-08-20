import { render, screen, cleanup } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import type { PublicPeerInsight } from '../api/generated'
import { PeerInsight, peerInsightStatus } from './PeerInsight'

const current: PublicPeerInsight = {
  state: 'ok',
  freshness: 'current',
  observedAt: '2026-08-16T03:00:00Z',
  receivedAt: '2026-08-16T03:00:00Z',
  peerCount: 4,
  inboundCount: 1,
  outboundCount: 3,
  trustedCount: 2,
  staticCount: 1,
  consensusCount: 2,
}

afterEach(cleanup)

describe('PeerInsight', () => {
  it('keeps collection, freshness, and value dimensions distinct', () => {
    expect(peerInsightStatus(current)).toBe('Current')
    expect(peerInsightStatus({ ...current, freshness: 'stale' })).toBe('Stale')
    expect(peerInsightStatus({ ...current, state: 'error' })).toBe('Error')
    expect(peerInsightStatus({ ...current, state: 'unsupported' })).toBe('Unsupported')
    expect(peerInsightStatus({ ...current, peerCount: 0, inboundCount: 0, outboundCount: 0 })).toBe('Empty')
    expect(peerInsightStatus({ state: 'ok', freshness: 'unknown', peerCount: null })).toBe('Unknown')
    expect(peerInsightStatus(undefined)).toBe('Unknown')
  })

  it('renders bounded summaries with accessible status text and does not expose peer identities', () => {
    render(<PeerInsight insight={current} />)
    expect(screen.getByRole('heading', { name: 'Peer insight' })).toBeTruthy()
    expect(screen.getAllByText('Current')).toHaveLength(3)
    expect(screen.getByText('Inbound')).toBeTruthy()
    expect(screen.getByText('Outbound')).toBeTruthy()
    expect(screen.getByText('Trusted')).toBeTruthy()
    expect(screen.getByText('Consensus')).toBeTruthy()
    expect(screen.getByText('4')).toBeTruthy()
    expect(screen.queryByText(/peer[- ]id/i)).toBeNull()
    expect(screen.queryByText(/remote[- ]ip/i)).toBeNull()
  })

  it('shows Unknown instead of zero when no successful snapshot exists', () => {
    render(<PeerInsight insight={{ state: 'error', freshness: 'unknown', peerCount: null }} />)
    expect(screen.getByText('Error')).toBeTruthy()
    expect(screen.getAllByText('Unknown').length).toBeGreaterThanOrEqual(8)
    expect(screen.getByText(/no successful Peer snapshot is available/i)).toBeTruthy()
  })

  it('renders a successful empty snapshot as Empty with authoritative zero counts', () => {
    const empty = { ...current, peerCount: 0, inboundCount: 0, outboundCount: 0, trustedCount: 0, staticCount: 0, consensusCount: 0 }
    render(<PeerInsight insight={empty} />)
    expect(screen.getAllByText('Empty')).toHaveLength(1)
    expect(screen.getAllByText('0')).toHaveLength(6)
    expect(screen.getByText(/latest successful snapshot contained no Peers/i)).toBeTruthy()
  })

  it('keeps last-good non-empty values visible with Error and Unsupported context', () => {
    const { rerender } = render(<PeerInsight insight={{ ...current, state: 'error' }} />)
    expect(screen.getByText('Error')).toBeTruthy()
    expect(screen.getByText('Last-good peers')).toBeTruthy()
    expect(screen.getByText(/last-good snapshot remains visible/i)).toBeTruthy()
    expect(screen.getByText('4')).toBeTruthy()

    rerender(<PeerInsight insight={{ ...current, state: 'unsupported' }} />)
    expect(screen.getByText('Unsupported')).toBeTruthy()
    expect(screen.getByText(/does not expose a supported Peer snapshot/i)).toBeTruthy()
    expect(screen.getByText('4')).toBeTruthy()
  })

  it('keeps non-empty last-good values visible while freshness is Stale', () => {
    render(<PeerInsight insight={{ ...current, freshness: 'stale', staleSince: '2026-08-16T03:02:00Z' }} />)
    expect(screen.getByText('Stale')).toBeTruthy()
    expect(screen.getByText('Last-good peers')).toBeTruthy()
    expect(screen.getByText('4')).toBeTruthy()
    expect(screen.getByText(/last-good snapshot is shown/i)).toBeTruthy()
  })

  it('keeps an authoritative last-good zero visible while stale', () => {
    render(<PeerInsight insight={{
      state: 'ok',
      freshness: 'stale',
      observedAt: '2026-08-16T00:00:00Z',
      receivedAt: '2026-08-16T00:00:00Z',
      staleSince: '2026-08-16T00:02:00Z',
      peerCount: 0,
      inboundCount: 0,
      outboundCount: 0,
      trustedCount: 0,
      staticCount: 0,
      consensusCount: 0,
    }} />)
    expect(screen.getByText('Stale')).toBeTruthy()
    expect(screen.getAllByText('0')).toHaveLength(6)
    expect(screen.getByText(/last-good snapshot is shown/i)).toBeTruthy()
    expect(screen.getByText(/Stale since 2026-08-16 00:02:00 UTC/i)).toBeTruthy()
  })
  it('does not render retained empty values for non-current collection states', () => {
    const retainedEmpty: PublicPeerInsight = {
      ...current,
      peerCount: 0,
      inboundCount: 0,
      outboundCount: 0,
      trustedCount: 0,
      staticCount: 0,
      consensusCount: 0,
    }
    const { rerender } = render(<PeerInsight insight={{ ...retainedEmpty, state: 'starting' }} />)
    for (const state of ['starting', 'disabled', 'unsupported'] as const) {
      rerender(<PeerInsight insight={{ ...retainedEmpty, state }} />)
      expect(screen.queryByText('0')).toBeNull()
      expect(screen.getAllByText('Unknown').length).toBeGreaterThanOrEqual(6)
    }
  })
})
