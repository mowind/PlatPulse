import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { PeerHistoryInsight, type PeerHistoryData } from './PeerHistoryInsight'

afterEach(cleanup)

const history: PeerHistoryData = {
  state: 'ok',
  freshness: 'current',
  fiveMinute: [{
    bucketStart: '2026-08-12T10:05:00Z',
    lastObservedAt: '2026-08-12T10:09:00Z',
    sampleCount: 2,
    totalPeers: 10,
    averagePeers: 5,
    inboundCount: 4,
    outboundCount: 6,
    trustedCount: 3,
    staticCount: 2,
    consensusCount: 1,
    knownCountryCount: 7,
    unknownCountryCount: 3,
    countries: [{ countryCode: 'US', count: 7 }],
    arrivals: 2,
    departures: 1,
    cbftLag: { sampleCount: 2, minimum: 0, average: 1.5, maximum: 3 },
  }],
  hourly: [],
}

describe('PeerHistoryInsight', () => {
  it('renders bounded aggregate summaries without peer identifiers or addresses', () => {
    render(<PeerHistoryInsight history={history} />)

    expect(screen.getByRole('heading', { name: 'Peer history' })).toBeTruthy()
    expect(screen.getAllByText('Current')).toHaveLength(2)
    expect(screen.getByText('5')).toBeTruthy()
    expect(screen.getByText('Five-minute history')).toBeTruthy()
    expect(screen.getByText(/US\s+7/)).toBeTruthy()
    expect(screen.queryByText(/peer-id|remote|address|ip/i)).toBeNull()
  })

  it('keeps retained aggregates visible while marking a failed refresh as Error', () => {
    render(<PeerHistoryInsight history={history} error />)

    expect(screen.getByText('Error')).toBeTruthy()
    expect(screen.getByText(/showing retained last-good aggregates/i)).toBeTruthy()
    expect(screen.getByText(/US\s+7/)).toBeTruthy()
  })

  it('renders Starting while the independent history request is pending', () => {
    render(<PeerHistoryInsight history={undefined} loading />)

    expect(screen.getByText('Starting')).toBeTruthy()
    expect(screen.getByText('Unknown')).toBeTruthy()
    expect(screen.getByText(/loading retained aggregate buckets/i)).toBeTruthy()
    expect(screen.queryByText('0')).toBeNull()
  })

  it('keeps missing history explicit instead of displaying zero values', () => {
    render(<PeerHistoryInsight history={{ state: 'unknown', freshness: 'unknown', fiveMinute: [], hourly: [] }} />)

    expect(screen.getAllByText('Unknown')).toHaveLength(2)
    expect(screen.getByText(/no aggregate bucket is available yet/i)).toBeTruthy()
    expect(screen.queryByText('0')).toBeNull()
  })
})
