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
  it('keeps a current map quiet and opens the complete explanation on demand', async () => {
    stubFetch(geometryResponse)
    renderMap({ networks: [network({ geo: { countries: [seCountry], knownCountryCount: 3, unknownCountryCount: 0, availablePeerCount: 3, unknownWithPublicIpCount: 0 } })] })
    await screen.findByRole('img', { name: 'Peer countries map' })
    expect(screen.getByText('· 3 records')).toBeTruthy()
    expect(screen.queryByText('Current')).toBeNull()
    expect(screen.queryByText('Scope: All Networks')).toBeNull()
    expect(screen.queryByRole('list', { name: 'Peer countries by count' })).toBeNull()
    expect(screen.queryByRole('status')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Map information' }))
    expect(screen.getByRole('dialog', { name: 'Map information' })).toBeTruthy()
    expect(screen.getByText('Scope: All Networks')).toBeTruthy()
    expect(screen.getByText(/3 Peer records in scope; counted per Node, not deduplicated by IP/)).toBeTruthy()
    expect(screen.getByRole('list', { name: 'Peer countries by count' }).textContent).toContain('Sweden')
    expect(screen.getByRole('heading', { name: 'Map credits' })).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Close map information' }))
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it('dismisses map information with Escape and restores trigger focus', async () => {
    stubFetch(geometryResponse)
    renderMap()
    await screen.findByRole('img', { name: 'Peer countries map' })
    const trigger = screen.getByRole('button', { name: 'Map information' })
    trigger.focus()
    fireEvent.click(trigger)
    const dialog = screen.getByRole('dialog', { name: 'Map information' })
    expect(dialog.contains(document.activeElement)).toBe(true)
    fireEvent.keyDown(document.activeElement ?? dialog, { key: 'Escape' })
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(document.activeElement).toBe(trigger)
  })

  it('does not warn for a missing outline when its quantity has a valid marker', async () => {
    stubFetch(geometryResponse)
    const locatedKosovo = { ...xkCountry, centroidLat: 42.6, centroidLon: 20.9 }
    const { rerender } = renderMap({ networks: [network({ geo: { countries: [locatedKosovo], knownCountryCount: 1, unknownCountryCount: 0, availablePeerCount: 1, unknownWithPublicIpCount: 0 } })] })
    await screen.findByRole('img', { name: 'Peer countries map' })
    expect(screen.queryByRole('status')).toBeNull()
    rerender(<GeoWorldMap networks={[network({ geo: { countries: [locatedKosovo], knownCountryCount: 1, unknownCountryCount: 2, availablePeerCount: 3, unknownWithPublicIpCount: 2 } })]} networkFilter="all" loading={false} hasProjection />)
    expect(screen.getAllByRole('status')).toHaveLength(1)
    expect(screen.getByRole('status').textContent).toBe('2 unknown locations')
    expect(screen.queryByText(/Some locations not shown/)).toBeNull()
  })

  it('combines missing plotted quantities and unknown locations into one compact status', async () => {
    stubFetch(geometryResponse)
    renderMap({ networks: [network({ geo: { countries: [seCountry, { ...deCountry, staleCount: 0 }], knownCountryCount: 5, unknownCountryCount: 1, availablePeerCount: 6 } })] })
    await screen.findByRole('img', { name: 'Peer countries map' })

    // Germany has an outline but no representative point: its quantity is
    // still not shown. That is separate from the one unknown-country record.
    const statuses = screen.getAllByRole('status')
    expect(statuses).toHaveLength(1)
    expect(statuses[0].textContent).toContain('Some locations not shown')
    expect(statuses[0].textContent).toContain('1 unknown locations')
    expect(statuses[0].textContent).not.toContain('3 unknown locations')
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(screen.getByText('· 6 records')).toBeTruthy()

    fireEvent.click(screen.getByRole('button', { name: 'Map information' }))
    expect(screen.getByRole('list', { name: 'Peer countries by count' }).textContent).toContain('Germany')
    expect(screen.getByText(/Known 5 · Unknown 1/)).toBeTruthy()
  })

  it.each(['click', 'Enter', ' '] as const)('opens a named country marker tooltip using %s', async (activation) => {
    stubFetch(geometryResponse)
    renderMap({ networks: [network({ geo: { countries: [seCountry], knownCountryCount: 3, unknownCountryCount: 0, availablePeerCount: 3 } })] })
    await screen.findByRole('img', { name: 'Peer countries map' })
    const marker = screen.getByRole('button', { name: 'Sweden · 3 records' })
    expect(marker.tagName.toLowerCase()).toBe('g')
    expect(marker.getAttribute('tabindex')).toBe('0')
    expect(screen.queryByRole('tooltip')).toBeNull()
    if (activation === 'click') fireEvent.click(marker)
    else fireEvent.keyDown(marker, { key: activation })
    expect(screen.getByRole('tooltip').textContent).toBe('Sweden · 3 records')
  })

  it('plots Server representative points and keeps every real count readable as text', async () => {
    stubFetch(geometryResponse)
    const { container } = renderMap()

    expect(screen.getByRole('heading', { name: 'Peer countries' })).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Map information' }))
    expect(screen.getByText('Data state: Current')).toBeTruthy()
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

    expect(await screen.findByText(/^Map unavailable/)).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Map information' }))
    // Country counts are Server data and survive a basemap failure.
    expect(screen.getByText(/Known 6 · Unknown 1/)).toBeTruthy()
    expect(screen.getByRole('list', { name: 'Peer countries by count' }).textContent).toContain('SE')
    expect(screen.queryByRole('img', { name: 'Peer countries map' })).toBeNull()

    fetchMock.mockImplementation(() => Promise.resolve(geometryResponse()))
    fireEvent.click(screen.getByRole('button', { name: 'Retry map' }))
    // Retry is outside the disclosure, so its click dismisses the panel.
    fireEvent.click(screen.getByRole('button', { name: 'Map information' }))
    await waitFor(() => expect(screen.getByText('Map resource: Current')).toBeTruthy())
    expect(screen.getByRole('img', { name: 'Peer countries map' })).toBeTruthy()
  })

  it('rejects an unusable payload such as the SPA fallback document', async () => {
    stubFetch(() => ({ ok: true, status: 200, text: () => Promise.resolve('<!doctype html><title>PlatPulse</title>') }))
    renderMap()
    expect(await screen.findByText(/^Map unavailable/)).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Map information' }))
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

    fireEvent.click(screen.getByRole('button', { name: 'Map information' }))
    expect(screen.getByText('Scope: All Networks')).toBeTruthy()
    // Only the Network with a denominator contributes counts, so nothing is
    // fabricated for the Network that never observed a Peer Snapshot.
    await waitFor(() => expect(screen.getByText(/Known 6 · Unknown 1/)).toBeTruthy())
    expect(screen.getByText('1 of 2 Networks in scope have a Peer country basis; the others are not included in these counts.')).toBeTruthy()

    rerender(<GeoWorldMap networks={[mainnet, testnet]} networkFilter="testnet" loading={false} hasProjection />)
    expect(screen.getByText('Scope: Testnet')).toBeTruthy()
    expect(screen.getByText(/no Active Node has reported a successful Peer Snapshot yet/)).toBeTruthy()
    expect(screen.queryByText(/^Known \d/)).toBeNull()
  })

  it('keeps Geo state, Peer observation freshness, and basemap state separate', async () => {
    stubFetch(() => { throw new Error('offline') })
    renderMap({
      networks: [network({ geo: { state: 'stale', lastGoodAt: '2026-01-01T00:00:00Z', databaseAgeSeconds: 2678400, staleSince: '2026-01-31T00:00:00Z', scope: 'partial' }, peers: { state: 'ok', freshness: 'stale' } })],
    })

    fireEvent.click(screen.getByRole('button', { name: 'Map information' }))
    expect(await screen.findByText(/Geo database is Stale/)).toBeTruthy()
    expect(screen.getByText('Data state: Stale')).toBeTruthy()
    expect(screen.getByText('Peer observation: Stale')).toBeTruthy()
    await waitFor(() => expect(screen.getByText('Map resource: Error')).toBeTruthy())
    expect(screen.getByText(/Database age: 31 days/)).toBeTruthy()
    expect(screen.getByText(/Partial scope/)).toBeTruthy()
    expect(screen.getByText(/Showing the last-good country projection/)).toBeTruthy()
  })

  it('never turns an unavailable or never-observed projection into a zero count', async () => {
    stubFetch(geometryResponse)
    const { rerender } = renderMap({ networks: [], hasProjection: false })
    fireEvent.click(screen.getByRole('button', { name: 'Map information' }))
    expect(screen.getByText('Data state: Unknown')).toBeTruthy()
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
    fireEvent.click(screen.getByRole('button', { name: 'Map information' }))
    expect(screen.getByText('Data state: Starting')).toBeTruthy()
    expect(screen.getByText(/Starting; the Peer country scope is still loading/)).toBeTruthy()
    expect(screen.getByText('Map resource: Unknown')).toBeTruthy()

    // An authoritative, successful empty country set is a real zero with its
    // basis stated, never Unknown and never a hidden denominator.
    rerender(<GeoWorldMap
      networks={[network({ geo: { countries: [], knownCountryCount: 0, unknownCountryCount: 0, availablePeerCount: 0, unknownWithPublicIpCount: 0, unknownWithoutRemoteIpCount: 0 } })]}
      networkFilter="all" loading={false} hasProjection
    />)
    expect(screen.getByText(/^Known 0\b/)).toBeTruthy()
    expect(screen.queryByText(/Unknown 0/)).toBeNull()
    expect(screen.getByText(/0 Peer records in scope; counted per Node, not deduplicated by IP/)).toBeTruthy()
    expect(screen.getByText(/No country observations are available yet/)).toBeTruthy()

    rerender(<GeoWorldMap networks={[]} networkFilter="all" loading={false} hasProjection />)
    expect(screen.getByText('Data state: Empty')).toBeTruthy()
    expect(screen.getByText(/the Public Projection has no Network to place Peer countries on/)).toBeTruthy()
  })

  it.each([
    ['error', 'Data unavailable'],
    ['stale', 'Data stale'],
  ])('retains an authoritative zero without hiding %s behind No data', async (state, hint) => {
    stubFetch(geometryResponse)
    renderMap({ networks: [network({ geo: { state, countries: [], knownCountryCount: 0, unknownCountryCount: 0, availablePeerCount: 0, unknownWithPublicIpCount: 0, unknownWithoutRemoteIpCount: 0 } })] })
    await screen.findByRole('img', { name: 'Peer countries map' })
    expect(screen.getAllByRole('status')).toHaveLength(1)
    expect(screen.getByRole('status').textContent).toBe(hint)
    expect(screen.getByText('· 0 records')).toBeTruthy()
    expect(screen.queryByText('No data')).toBeNull()
    expect(screen.queryByText(/0 unknown locations/)).toBeNull()
  })

  it('keeps loading, unavailable, never-observed and authoritative zero compact and distinct', async () => {
    stubFetch(geometryResponse)
    const { rerender } = renderMap({ networks: [], loading: true, hasProjection: false })
    expect(screen.getAllByRole('status')).toHaveLength(1)
    expect(screen.getByRole('status').textContent).toBe('Loading data')
    expect(screen.queryByText('· 0 records')).toBeNull()

    rerender(<GeoWorldMap networks={[]} networkFilter="all" loading={false} hasProjection={false} />)
    expect(screen.getAllByRole('status')).toHaveLength(1)
    expect(screen.getByRole('status').textContent).toBe('Data unavailable')
    expect(screen.queryByText('· 0 records')).toBeNull()

    const unobserved = network({ geo: { state: 'unknown', scope: 'unobserved', countries: null, knownCountryCount: null, unknownCountryCount: null, availablePeerCount: null } })
    rerender(<GeoWorldMap networks={[unobserved]} networkFilter="all" loading={false} hasProjection />)
    expect(screen.getAllByRole('status')).toHaveLength(1)
    expect(screen.getByRole('status').textContent).toBe('No observations yet')
    expect(screen.queryByText('· 0 records')).toBeNull()

    const zero = network({ geo: { countries: [], knownCountryCount: 0, unknownCountryCount: 0, availablePeerCount: 0, unknownWithPublicIpCount: 0 } })
    rerender(<GeoWorldMap networks={[zero]} networkFilter="all" loading={false} hasProjection />)
    await screen.findByRole('img', { name: 'Peer countries map' })
    expect(screen.getAllByRole('status')).toHaveLength(1)
    expect(screen.getByRole('status').textContent).toBe('No data')
    expect(screen.getByText('· 0 records')).toBeTruthy()
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it.each([
    { name: 'partial country basis', geo: { scope: 'partial' }, peers: {}, hint: 'Partial data' },
    { name: 'unknown Peer observation', geo: {}, peers: { state: 'unknown', freshness: 'unknown' }, hint: 'Observation status unknown' },
  ])('exposes one compact hint for $name', async ({ geo, peers, hint }) => {
    stubFetch(geometryResponse)
    renderMap({ networks: [network({ geo: { countries: [seCountry], knownCountryCount: 3, unknownCountryCount: 0, availablePeerCount: 3, ...geo }, peers })] })
    await screen.findByRole('img', { name: 'Peer countries map' })
    expect(screen.getAllByRole('status')).toHaveLength(1)
    expect(screen.getByRole('status').textContent).toBe(hint)
  })

  it('does not collapse varying Peer observations across Networks into current', async () => {
    stubFetch(geometryResponse)
    const geo = { countries: [seCountry], knownCountryCount: 3, unknownCountryCount: 0, availablePeerCount: 3, unknownWithPublicIpCount: 0 }
    renderMap({ networks: [network({ geo }), network({ networkKey: 'testnet', displayName: 'Testnet', geo, peers: { state: 'unknown', freshness: 'unknown' } })] })
    await screen.findByRole('img', { name: 'Peer countries map' })
    expect(screen.getAllByRole('status')).toHaveLength(1)
    expect(screen.getByRole('status').textContent).toBe('Observation status varies')
    expect(screen.getByText('· 6 records')).toBeTruthy()
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

