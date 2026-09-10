import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import type { PublicGeoInsight } from '../api/generated'
import { GeoInsight } from './GeoInsight'

afterEach(cleanup)

/** A complete, Current Geo projection with the retained country basis. */
function geo(overrides: Partial<PublicGeoInsight> = {}): PublicGeoInsight {
  return {
    state: 'current',
    lastGoodAt: null,
    scope: 'complete',
    knownCountryCount: 0,
    unknownCountryCount: 0,
    availablePeerCount: 0,
    unknownWithoutRemoteIpCount: 0,
    unknownWithPublicIpCount: 0,
    countries: [],
    attribution: 'This product includes GeoLite Data created by MaxMind, available from https://www.maxmind.com.',
    ...overrides,
  }
}

describe('GeoInsight', () => {
  it('renders country-only counts and attribution without address fields', () => {
    render(<GeoInsight insight={geo({
      knownCountryCount: 3,
      unknownCountryCount: 1,
      availablePeerCount: 4,
      unknownWithoutRemoteIpCount: 1,
      unknownWithPublicIpCount: 0,
      countries: [{ countryCode: 'US', count: 3, staleCount: 0, centroidLat: 37, centroidLon: -95 }],
    })} />)
    expect(screen.getByRole('heading', { name: 'Peer countries' })).toBeTruthy()
    expect(screen.getByText('Current')).toBeTruthy()
    expect(screen.getByText('US')).toBeTruthy()
    expect(screen.getByText('3')).toBeTruthy()
    expect(screen.getByText(/Known 3 · Unknown 1/)).toBeTruthy()
    expect(screen.getByText(/4 Peer records in scope; counted per Node, not deduplicated by IP/)).toBeTruthy()
    expect(screen.getByText(/This product includes GeoLite Data created by MaxMind/)).toBeTruthy()
    expect(screen.queryByText(/\b\d{1,3}(\.\d{1,3}){3}\b/)).toBeNull()
    expect(screen.queryByText(/peer-[a-z0-9-]/i)).toBeNull()
    expect(screen.queryByText(/static centroid|37, -95/i)).toBeNull()
  })

  it('reports the Server-computed unknown count instead of letting the browser subtract', () => {
    render(<GeoInsight insight={geo({
      knownCountryCount: 2,
      unknownCountryCount: 1,
      availablePeerCount: 3,
      unknownWithoutRemoteIpCount: 0,
      unknownWithPublicIpCount: 1,
      countries: [{ countryCode: 'DE', count: 2, staleCount: 0, centroidLat: 51, centroidLon: 10 }],
    })} />)
    expect(screen.getByText(/Known 2 · Unknown 1/)).toBeTruthy()
    // The one unknown record has a public IP but no retained country result.
    // That reason is a Server-computed field, never a browser subtraction.
    expect(screen.getByText(/1 without a retained country result/)).toBeTruthy()
    expect(screen.queryByText(/without a usable public remote IP/)).toBeNull()
  })

  it('explains a partial scope instead of presenting the counts as complete', () => {
    render(<GeoInsight insight={geo({
      scope: 'partial',
      knownCountryCount: 1,
      unknownCountryCount: 0,
      availablePeerCount: 1,
      countries: [{ countryCode: 'US', count: 1, staleCount: 0, centroidLat: 37, centroidLon: -95 }],
    })} />)
    expect(screen.getByText(/Partial scope/)).toBeTruthy()
    expect(screen.getByText(/Known 1 · Unknown 0/)).toBeTruthy()
  })

  it('keeps never-observed networks Unknown instead of showing a zero count', () => {
    render(<GeoInsight insight={geo({
      scope: 'unobserved',
      knownCountryCount: null,
      unknownCountryCount: null,
      availablePeerCount: null,
      unknownWithoutRemoteIpCount: null,
      unknownWithPublicIpCount: null,
      countries: null,
    })} />)
    expect(screen.getByText(/no Active Node has reported a successful Peer Snapshot/i)).toBeTruthy()
    expect(screen.queryByText(/Known 0/)).toBeNull()
    expect(screen.queryByText('0')).toBeNull()
  })

  it('names Unsupported Peer collection instead of implying a snapshot is merely pending', () => {
    render(<GeoInsight peerState="unsupported" insight={geo({
      scope: 'unobserved',
      knownCountryCount: null,
      unknownCountryCount: null,
      availablePeerCount: null,
      unknownWithoutRemoteIpCount: null,
      unknownWithPublicIpCount: null,
      countries: null,
    })} />)
    expect(screen.getByText(/Peer collection is Unsupported, so no country basis exists/i)).toBeTruthy()
    expect(screen.queryByText(/has reported a successful Peer Snapshot yet/)).toBeNull()
  })

  it('shows an authoritative successful empty snapshot as a zero distribution', () => {
    render(<GeoInsight insight={geo({
      knownCountryCount: 0,
      unknownCountryCount: 0,
      availablePeerCount: 0,
      countries: [],
    })} />)
    expect(screen.getByText(/Known 0 · Unknown 0/)).toBeTruthy()
    expect(screen.getByText(/No country observations are available yet/)).toBeTruthy()
  })

  it('marks retained last-good countries as stale without re-counting them as unknown', () => {
    render(<GeoInsight insight={geo({
      state: 'stale',
      lastGoodAt: '2026-01-01T00:00:00Z',
      databaseAgeSeconds: 2678400,
      staleSince: '2026-01-31T00:00:00Z',
      knownCountryCount: 2,
      unknownCountryCount: 1,
      availablePeerCount: 3,
      countries: [{ countryCode: 'US', count: 2, staleCount: 1, centroidLat: 37, centroidLon: -95 }],
    })} />)
    expect(screen.getByText(/Geo database is Stale/i)).toBeTruthy()
    expect(screen.getByText(/Database age: 31 days/i)).toBeTruthy()
    expect(screen.getByText(/1 retained as last-good Stale/)).toBeTruthy()
    expect(screen.getByText(/Known 2 · Unknown 1/)).toBeTruthy()
  })

  it('keeps a known country that has no representative point in the accessible list', () => {
    render(<GeoInsight insight={geo({
      knownCountryCount: 1,
      countries: [{ countryCode: 'XK', count: 1, staleCount: 0, centroidLat: null, centroidLon: null }],
    })} />)
    expect(screen.getByText('XK')).toBeTruthy()
    expect(screen.getByText('1')).toBeTruthy()
    expect(screen.getByText(/no representative point/i)).toBeTruthy()
  })

  it('keeps Error detail and age visible without inventing a country distribution', () => {
    render(<GeoInsight insight={geo({
      state: 'error',
      lastGoodAt: '2026-01-01T00:00:00Z',
      databaseAgeSeconds: 3600,
      staleSince: null,
      errorReason: 'Geo database is invalid',
      scope: 'unobserved',
      knownCountryCount: null,
      unknownCountryCount: null,
      availablePeerCount: null,
      unknownWithoutRemoteIpCount: null,
      unknownWithPublicIpCount: null,
      countries: null,
      attribution: null,
    })} />)
    expect(screen.getByText(/Geo lookup is Error/i)).toBeTruthy()
    expect(screen.getByText(/Reason: Geo database is invalid/i)).toBeTruthy()
    expect(screen.getByText(/Database age: 1 hour/i)).toBeTruthy()
    expect(screen.queryByText(/Known 0/)).toBeNull()
  })

  it('compresses server-disabled Geo into a neutral single-line notice', () => {
    render(<GeoInsight insight={geo({
      state: 'disabled',
      scope: 'unavailable',
      knownCountryCount: null,
      unknownCountryCount: null,
      availablePeerCount: null,
      unknownWithoutRemoteIpCount: null,
      unknownWithPublicIpCount: null,
      countries: null,
      attribution: null,
    })} />)
    const notice = screen.getByText('Peer countries · Disabled by server', { exact: true })

    expect(notice.tagName).toBe('P')
    expect(notice.getAttribute('role')).toBe('status')
    expect(screen.queryByRole('heading', { name: 'Peer countries' })).toBeNull()
    expect(screen.queryByText('Disabled', { exact: true })).toBeNull()
  })
})
