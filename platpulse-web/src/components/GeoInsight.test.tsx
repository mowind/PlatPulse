import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { GeoInsight } from './GeoInsight'

afterEach(cleanup)

describe('GeoInsight', () => {
  it('renders country-only counts and attribution without address fields', () => {
    render(<GeoInsight insight={{
      state: 'current',
      lastGoodAt: null,
      countries: [{ countryCode: 'US', count: 3, centroidLat: 37, centroidLon: -95 }],
      attribution: 'This product includes GeoLite Data created by MaxMind, available from https://www.maxmind.com.',
    }} />)
    expect(screen.getByRole('heading', { name: 'Peer countries' })).toBeTruthy()
    expect(screen.getByText('Current')).toBeTruthy()
    expect(screen.getByText('US')).toBeTruthy()
    expect(screen.getByText('3')).toBeTruthy()
    expect(screen.getByText(/This product includes GeoLite Data created by MaxMind/)).toBeTruthy()
    expect(screen.queryByText(/remote|address|ip/i)).toBeNull()
  })

  it('describes stale projections without collapsing them into unknown', () => {
    render(<GeoInsight insight={{ state: 'stale', lastGoodAt: '2026-01-01T00:00:00Z', databaseAgeSeconds: 2678400, staleSince: '2026-01-31T00:00:00Z', errorReason: null, countries: null, attribution: null }} />)
    expect(screen.getByText(/Geo database is Stale/i)).toBeTruthy()
    expect(screen.getByText(/Database age: 31 days/i)).toBeTruthy()
    expect(screen.getByText(/Stale since: 2026-01-31T00:00:00Z/i)).toBeTruthy()
  })
  it('explains when a last-good country projection is stale', () => {
    render(<GeoInsight insight={{ state: 'stale', lastGoodAt: '2026-01-01T00:00:00Z', databaseAgeSeconds: 2678400, staleSince: '2026-01-31T00:00:00Z', errorReason: null, countries: [{ countryCode: 'US', count: 1, centroidLat: 37, centroidLon: -95 }], attribution: null }} />)
    expect(screen.getByText(/showing the last-good country projection/i)).toBeTruthy()
    expect(screen.getByText(/Database age: 31 days/i)).toBeTruthy()
    expect(screen.getByText(/Last good database load: 2026-01-01T00:00:00Z/i)).toBeTruthy()
  })

  it('keeps Error detail and age visible without a country projection', () => {
    render(<GeoInsight insight={{ state: 'error', lastGoodAt: '2026-01-01T00:00:00Z', databaseAgeSeconds: 3600, staleSince: null, errorReason: 'Geo database is invalid', countries: null, attribution: null }} />)
    expect(screen.getByText(/Geo lookup is Error/i)).toBeTruthy()
    expect(screen.getByText(/Reason: Geo database is invalid/i)).toBeTruthy()
    expect(screen.getByText(/Database age: 1 hour/i)).toBeTruthy()
  })
  it('is explicit when Geo is disabled', () => {
    render(<GeoInsight insight={{ state: 'disabled', lastGoodAt: null, countries: null, attribution: null }} />)
    expect(screen.getByText('Disabled')).toBeTruthy()
    expect(screen.getByText(/Country insight is Disabled/i)).toBeTruthy()
  })
})
