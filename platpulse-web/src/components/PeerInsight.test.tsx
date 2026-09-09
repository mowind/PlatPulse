import { cleanup, render, screen, within } from '@testing-library/react'
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

  it('puts primary counts first and reduces a healthy snapshot to one quiet summary', () => {
    render(<PeerInsight insight={current} />)

    const region = screen.getByRole('region', { name: 'Peer insight' })
    const primary = within(region).getByRole('group', { name: 'Primary peer counts' })
    const secondary = within(region).getByRole('group', { name: 'Secondary peer counts' })

    expect(primary.textContent).toMatch(/Peers.*Inbound.*Outbound/)
    expect(secondary.textContent).toMatch(/Trusted.*Static.*Consensus/)
    expect(within(primary).getByText('4')).toBeTruthy()
    expect(within(primary).getByText('1')).toBeTruthy()
    expect(within(primary).getByText('3')).toBeTruthy()
    expect(within(region).getByText('Peer data current')).toBeTruthy()
    expect(within(region).queryByText('Collection', { exact: true })).toBeNull()
    expect(within(region).queryByText('Freshness', { exact: true })).toBeNull()
    expect(within(region).queryByText('Value', { exact: true })).toBeNull()
    expect(within(region).queryByText(/peer[- ]id/i)).toBeNull()
    expect(within(region).queryByText(/remote[- ]ip/i)).toBeNull()
  })

  it('shows a fresh empty snapshot as authoritative zero rather than Unknown', () => {
    render(<PeerInsight insight={{
      ...current,
      peerCount: 0,
      inboundCount: 0,
      outboundCount: 0,
      trustedCount: 0,
      staticCount: 0,
      consensusCount: 0,
    }} />)

    const region = screen.getByRole('region', { name: 'Peer insight' })
    expect(within(region).getByText('Peer data current')).toBeTruthy()
    expect(within(region).getAllByText('0')).toHaveLength(6)
    expect(within(region).getByText(/authoritative empty snapshot/i)).toBeTruthy()
    expect(within(region).queryByText('Unknown', { exact: true })).toBeNull()
  })

  it('shows collection failure and Unknown when no successful snapshot exists', () => {
    render(<PeerInsight insight={{ state: 'error', freshness: 'unknown', peerCount: null }} />)

    const region = screen.getByRole('region', { name: 'Peer insight' })
    expect(within(region).getByText(/Collection failed/)).toBeTruthy()
    expect(within(region).getAllByText('Unknown', { exact: true }).length).toBeGreaterThanOrEqual(2)
    expect(within(region).getByText(/no successful Peer snapshot is available/i)).toBeTruthy()
    expect(within(region).queryByText('0', { exact: true })).toBeNull()
    expect(within(region).queryByText('Peer data current')).toBeNull()
  })

  it('discloses collection failure and stale freshness while retaining the last-good values', () => {
    render(<PeerInsight insight={{
      ...current,
      state: 'error',
      freshness: 'stale',
      staleSince: '2026-08-16T03:02:00Z',
    }} />)

    const region = screen.getByRole('region', { name: 'Peer insight' })
    expect(within(region).getByText(/Collection failed/)).toBeTruthy()
    expect(within(region).getByText('Stale', { exact: true })).toBeTruthy()
    expect(within(region).getByText('Showing last successful snapshot')).toBeTruthy()
    expect(within(region).getByText(/Freshness stale/)).toBeTruthy()
    expect(within(region).getByText('4')).toBeTruthy()
    expect(within(region).getByText(/Stale since 2026-08-16 03:02:00 UTC/i)).toBeTruthy()
    expect(within(region).queryByText('Peer data current')).toBeNull()
  })

  it('retains authoritative zero values for Disabled, Unsupported, and Starting states', () => {
    const retainedEmpty: PublicPeerInsight = {
      ...current,
      peerCount: 0,
      inboundCount: 0,
      outboundCount: 0,
      trustedCount: 0,
      staticCount: 0,
      consensusCount: 0,
    }

    for (const state of ['starting', 'disabled', 'unsupported'] as const) {
      const { unmount } = render(<PeerInsight insight={{ ...retainedEmpty, state }} />)
      const region = screen.getByRole('region', { name: 'Peer insight' })
      expect(within(region).getByText(state[0].toUpperCase() + state.slice(1))).toBeTruthy()
      expect(within(region).getAllByText('0')).toHaveLength(6)
      expect(within(region).getByText('Showing last successful snapshot')).toBeTruthy()
      unmount()
    }
  })

  it('does not promote unknown freshness to current', () => {
    render(<PeerInsight insight={{ ...current, freshness: 'unknown' }} />)

    const region = screen.getByRole('region', { name: 'Peer insight' })
    expect(within(region).getByText('Unknown', { exact: true })).toBeTruthy()
    expect(within(region).getByText(/Freshness unknown/i)).toBeTruthy()
    expect(within(region).queryByText('Peer data current')).toBeNull()
    expect(within(region).getByText('4')).toBeTruthy()
  })

  it('distinguishes Node observation time from Server receipt time', () => {
    const { rerender } = render(<PeerInsight insight={undefined} />)
    let region = screen.getByRole('region', { name: 'Peer insight' })
    expect(within(region).getByText('Observation time varies by Node.')).toBeTruthy()

    rerender(<PeerInsight insight={{ ...current, observedAt: '2026-08-16T03:00:00Z', receivedAt: '2026-08-16T03:01:00Z' }} />)
    region = screen.getByRole('region', { name: 'Peer insight' })
    expect(within(region).getByText(/Last observed 2026-08-16 03:00:00 UTC/i)).toBeTruthy()
    expect(within(region).getByText(/Server received 2026-08-16 03:01:00 UTC/i)).toBeTruthy()
  })
})
