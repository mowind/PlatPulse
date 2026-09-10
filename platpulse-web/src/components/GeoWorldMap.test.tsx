import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { PublicNetwork } from '../api/generated'
import GeoMapBoundary from './GeoMapBoundary'
import GeoWorldMap from './GeoWorldMap'

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

/** The labelled map image itself, located by its accessible role rather than
 *  by a production CSS class. */
function mapImage(container: HTMLElement): SVGSVGElement {
  const image = container.querySelector('svg[role="img"]')
  if (!image) throw new Error('the Peer countries map has no image role')
  return image as SVGSVGElement
}

/** A minimal, version-agnostic basemap stand-in: two ISO countries with a
 *  path, so the component's fill, marker, and outline logic is exercised. */
const geometry = {
  asset: 'world-countries-110m',
  version: 1,
  projection: { width: 1000, height: 394, maxLat: 84, minLat: -58 },
  source: 'https://example.invalid/natural-earth.geojson',
  sourceLabel: 'Natural Earth 1:110m Admin 0 Countries (v5.1.2)',
  license: 'Public domain (Natural Earth)',
  attribution: 'Country outlines: Natural Earth 1:110m (public domain).',
  countries: [
    { code: 'SE', path: 'M520 60L560 60L560 100L520 100Z' },
    { code: 'DE', path: 'M500 100L520 100L520 120L500 120Z' },
  ],
}

function stubFetch(handler: () => Promise<unknown> | unknown) {
  const fetchMock = vi.fn(() => Promise.resolve(handler()))
  vi.stubGlobal('fetch', fetchMock)
  return fetchMock
}

function geometryResponse() {
  return { ok: true, status: 200, text: () => Promise.resolve(JSON.stringify(geometry)) }
}

const seCountry = { countryCode: 'SE', count: 3, staleCount: 0, centroidLat: 60.1282, centroidLon: 18.6435 }
const deCountry = { countryCode: 'DE', count: 2, staleCount: 1, centroidLat: null, centroidLon: null }
const xkCountry = { countryCode: 'XK', count: 1, staleCount: 0, centroidLat: null, centroidLon: null }

function network(overrides: {
  networkKey?: string
  displayName?: string
  geo?: Record<string, unknown>
  peers?: Record<string, unknown>
} = {}): PublicNetwork {
  const { networkKey = 'mainnet', displayName = 'Mainnet', geo = {}, peers = {} } = overrides
  return {
    networkKey,
    displayName,
    geo: {
      state: 'current',
      scope: 'complete',
      countries: [seCountry, deCountry, xkCountry],
      knownCountryCount: 6,
      unknownCountryCount: 1,
      unknownWithPublicIpCount: 1,
      unknownWithoutRemoteIpCount: 0,
      availablePeerCount: 7,
      attribution: 'This product includes GeoLite Data created by MaxMind.',
      ...geo,
    },
    peers: { state: 'ok', freshness: 'current', ...peers },
    nodes: [],
    validators: [],
  } as unknown as PublicNetwork
}

function renderMap(props: Partial<Parameters<typeof GeoWorldMap>[0]> = {}) {
  return render(
    <GeoWorldMap
      networks={props.networks ?? [network()]}
      networkFilter={props.networkFilter ?? 'all'}
      loading={props.loading ?? false}
      hasProjection={props.hasProjection ?? true}
    />,
  )
}

describe('GeoWorldMap', () => {
  it('plots Server representative points and keeps every real count readable as text', async () => {
    stubFetch(geometryResponse)
    const { container } = renderMap()

    expect(screen.getByRole('heading', { name: 'Peer countries' })).toBeTruthy()
    expect(await screen.findByText('Current')).toBeTruthy()
    expect(screen.getByText('Scope: All Networks')).toBeTruthy()
    expect(screen.getByText('Peer observation: Current')).toBeTruthy()
    await waitFor(() => expect(screen.getByText('Map resource: Current')).toBeTruthy())
    expect(screen.getByText(/Known 6 · Unknown 1/)).toBeTruthy()
    expect(screen.getByText(/7 Peer records in scope; counted per Node, not deduplicated by IP/)).toBeTruthy()
    expect(screen.getByText(/1 without a retained country result/)).toBeTruthy()

    // Only the country with a Server representative point gets a marker:
    // one circle carrying its real count as the label.
    await waitFor(() => expect(mapImage(container).querySelectorAll('circle')).toHaveLength(1))
    expect(mapImage(container).querySelector('circle')?.nextElementSibling?.textContent).toBe('3')
    // The basemap outlines and the observed-country fills are both drawn:
    // two basemap countries, so two outlines plus two Emerald fills.
    expect(mapImage(container).querySelectorAll('path')).toHaveLength(4)

    // Every country keeps a text statistic, with the reason it is not plotted.
    const list = screen.getByRole('list', { name: 'Peer countries by count' })
    expect(list.textContent).toContain('SE')
    expect(list.textContent).toContain('DE')
    expect(list.textContent).toContain('XK')
    expect(screen.getByText('1 last-good Stale')).toBeTruthy()
    expect(screen.getAllByText('no representative point')).toHaveLength(2)
    expect(screen.getAllByText('no map outline')).toHaveLength(1)

    // The map is an image with a described, non-claiming accessible name.
    const map = screen.getByRole('img', { name: 'Peer countries map' })
    expect(map.getAttribute('aria-describedby')).toBeTruthy()
    expect(screen.getByText(/Each marker is a Server-provided country representative point/)).toBeTruthy()
    // Raw addresses never reach this surface.
    expect(container.textContent).not.toMatch(/\b\d{1,3}(\.\d{1,3}){3}\b/)
  })

  it('degrades locally when the basemap is unavailable and retries on demand', async () => {
    const fetchMock = stubFetch(() => { throw new Error('offline') })
    renderMap()

    expect(await screen.findByText(/Map geometry is Unavailable/)).toBeTruthy()
    // Country counts are Server data and survive a basemap failure.
    expect(screen.getByText(/Known 6 · Unknown 1/)).toBeTruthy()
    expect(screen.getByRole('list', { name: 'Peer countries by count' }).textContent).toContain('SE')
    expect(screen.queryByRole('img', { name: 'Peer countries map' })).toBeNull()

    fetchMock.mockImplementation(() => Promise.resolve(geometryResponse()))
    fireEvent.click(screen.getByRole('button', { name: 'Retry map' }))
    await waitFor(() => expect(screen.getByText('Map resource: Current')).toBeTruthy())
    expect(screen.getByRole('img', { name: 'Peer countries map' })).toBeTruthy()
  })

  it('rejects an unusable payload such as the SPA fallback document', async () => {
    stubFetch(() => ({ ok: true, status: 200, text: () => Promise.resolve('<!doctype html><title>PlatPulse</title>') }))
    renderMap()
    expect(await screen.findByText(/Map geometry is Unavailable/)).toBeTruthy()
  })

  it('never requests a basemap while the Owner disabled Geo', async () => {
    const fetchMock = stubFetch(geometryResponse)
    renderMap({ networks: [network({ geo: { state: 'disabled', scope: 'unavailable', countries: null, knownCountryCount: null, unknownCountryCount: null, availablePeerCount: null } })] })

    expect(screen.getByText('Peer countries · Disabled by server', { exact: true })).toBeTruthy()
    expect(screen.queryByRole('heading', { name: 'Peer countries' })).toBeNull()
    expect(fetchMock).not.toHaveBeenCalled()
  })

  it('follows the Home Network filter and keeps the Peer basis explicit for All Networks', async () => {
    stubFetch(geometryResponse)
    const mainnet = network({ networkKey: 'mainnet', displayName: 'Mainnet' })
    const testnet = network({
      networkKey: 'testnet',
      displayName: 'Testnet',
      geo: { countries: null, knownCountryCount: null, unknownCountryCount: null, availablePeerCount: null, unknownWithPublicIpCount: null, unknownWithoutRemoteIpCount: null, scope: 'unobserved', state: 'unknown' },
    })
    const { rerender } = renderMap({ networks: [mainnet, testnet] })

    expect(await screen.findByText('Scope: All Networks')).toBeTruthy()
    // Only the Network with a denominator contributes counts, so nothing is
    // fabricated for the Network that never observed a Peer Snapshot.
    await waitFor(() => expect(screen.getByText(/Known 6 · Unknown 1/)).toBeTruthy())
    expect(screen.getByText('1 of 2 Networks in scope have a Peer country basis; the others are not included in these counts.')).toBeTruthy()

    rerender(<GeoWorldMap networks={[mainnet, testnet]} networkFilter="testnet" loading={false} hasProjection />)
    expect(screen.getByText('Scope: Testnet')).toBeTruthy()
    expect(screen.getByText(/no Active Node has reported a successful Peer Snapshot yet/)).toBeTruthy()
    expect(screen.queryByText(/Known /)).toBeNull()
  })

  it('keeps Geo state, Peer observation freshness, and basemap state separate', async () => {
    stubFetch(() => { throw new Error('offline') })
    renderMap({
      networks: [network({ geo: { state: 'stale', lastGoodAt: '2026-01-01T00:00:00Z', databaseAgeSeconds: 2678400, staleSince: '2026-01-31T00:00:00Z', scope: 'partial' }, peers: { state: 'ok', freshness: 'stale' } })],
    })

    expect(await screen.findByText(/Geo database is Stale/)).toBeTruthy()
    expect(screen.getByText('Stale')).toBeTruthy()
    expect(screen.getByText('Peer observation: Stale')).toBeTruthy()
    expect(screen.getByText('Map resource: Error')).toBeTruthy()
    expect(screen.getByText(/Database age: 31 days/)).toBeTruthy()
    expect(screen.getByText(/Partial scope/)).toBeTruthy()
    expect(screen.getByText(/Showing the last-good country projection/)).toBeTruthy()
  })

  it('never turns an unavailable or never-observed projection into a zero count', async () => {
    stubFetch(geometryResponse)
    const { rerender } = renderMap({ networks: [], hasProjection: false })
    expect(screen.getByText('Unknown')).toBeTruthy()
    expect(screen.getByText(/the Public Projection is currently unavailable/)).toBeTruthy()
    expect(screen.queryByText(/Known 0/)).toBeNull()

    rerender(<GeoWorldMap
      networks={[network({ geo: { scope: 'unobserved', countries: null, knownCountryCount: null, unknownCountryCount: null, availablePeerCount: null, unknownWithPublicIpCount: null, unknownWithoutRemoteIpCount: null } })]}
      networkFilter="all" loading={false} hasProjection
    />)
    expect(screen.getByText(/no Active Node has reported a successful Peer Snapshot yet/)).toBeTruthy()
    expect(screen.queryByText(/Known 0/)).toBeNull()
  })

  it('keeps Starting and an authoritative empty projection explicit', async () => {
    stubFetch(geometryResponse)
    const { rerender } = renderMap({ networks: [network()], loading: true })
    expect(screen.getByText('Starting')).toBeTruthy()
    expect(screen.getByText(/Starting; the Peer country scope is still loading/)).toBeTruthy()
    expect(screen.getByText('Map resource: Unknown')).toBeTruthy()

    // An authoritative, successful empty country set is a real zero with its
    // basis stated, never Unknown and never a hidden denominator.
    rerender(<GeoWorldMap
      networks={[network({ geo: { countries: [], knownCountryCount: 0, unknownCountryCount: 0, availablePeerCount: 0, unknownWithPublicIpCount: 0, unknownWithoutRemoteIpCount: 0 } })]}
      networkFilter="all" loading={false} hasProjection
    />)
    expect(screen.getByText(/Known 0 · Unknown 0/)).toBeTruthy()
    expect(screen.getByText(/0 Peer records in scope; counted per Node, not deduplicated by IP/)).toBeTruthy()
    expect(screen.getByText(/No country observations are available yet/)).toBeTruthy()

    rerender(<GeoWorldMap networks={[]} networkFilter="all" loading={false} hasProjection />)
    expect(screen.getByText('Empty')).toBeTruthy()
    expect(screen.getByText(/the Public Projection has no Network to place Peer countries on/)).toBeTruthy()
  })

  it('offers a labelled, expanded-state-aware map control', async () => {
    stubFetch(geometryResponse)
    renderMap()
    await screen.findByRole('img', { name: 'Peer countries map' })
    const toggle = screen.getByRole('button', { name: 'Show full map' })

    expect(toggle.getAttribute('aria-expanded')).toBe('false')
    // The control names the region it expands, and that region exists.
    const canvasId = toggle.getAttribute('aria-controls')
    expect(canvasId).toBeTruthy()
    expect(document.getElementById(canvasId as string)).toBeTruthy()

    fireEvent.click(toggle)
    expect(screen.getByRole('button', { name: 'Collapse map' }).getAttribute('aria-expanded')).toBe('true')

    fireEvent.click(screen.getByRole('button', { name: 'Collapse map' }))
    // Collapsing restores the compact map instead of hiding it.
    expect(screen.getByRole('button', { name: 'Show full map' }).getAttribute('aria-expanded')).toBe('false')
    expect(screen.getByRole('img', { name: 'Peer countries map' })).toBeTruthy()
  })
})

describe('GeoMapBoundary', () => {
  it('keeps the rest of Home alive when the map cannot render', () => {
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {})
    function Broken(): never {
      throw new Error('geometry exploded')
    }
    render(
      <div>
        <p>Active Nodes 12</p>
        <GeoMapBoundary><Broken /></GeoMapBoundary>
      </div>,
    )

    expect(screen.getByText('Active Nodes 12')).toBeTruthy()
    expect(screen.getByText(/Peer country map is Unavailable; the Node list below is unaffected/)).toBeTruthy()
    consoleError.mockRestore()
  })
})

