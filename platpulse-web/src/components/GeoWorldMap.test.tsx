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
  nodes?: Array<{ nodeId: string; health: string }>
} = {}): PublicNetwork {
  const { networkKey = 'mainnet', displayName = 'Mainnet', geo = {}, peers = {}, nodes = [] } = overrides
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
    nodes,
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
  it('shows the map and one expand control, and writes nothing onto the map', async () => {
    stubFetch(geometryResponse)
    renderMap()
    await screen.findByRole('img', { name: 'Peer countries map' })
    const map = screen.getByRole('region', { name: 'Peer countries' })

    // The map carries no control at all: the information and status controls,
    // the disclosure they opened, and the expand toggle were all removed.
    expect(map.querySelectorAll('button')).toHaveLength(0)
    expect(screen.queryByRole('dialog')).toBeNull()

    // No heading, no sentence, and no attribution is painted onto the map.
    expect(screen.queryByRole('heading', { name: 'Peer countries' })).toBeNull()
    expect(map.textContent).not.toMatch(/Natural Earth|GeoLite|MaxMind|IPinfo|GeoJS/)
    expect(map.textContent).not.toMatch(/Scope: |Known |Peer observation: |Map resource: /)
    expect(map.textContent).not.toMatch(/records$/)
  })

  it('describes what the markers are and what they are not', async () => {
    stubFetch(geometryResponse)
    const { container } = renderMap()
    await screen.findByRole('img', { name: 'Peer countries map' })
    const description = mapImage(container).querySelector('desc')?.textContent ?? ''
    expect(description).toMatch(/countries have Peer records in scope for All Networks/)
    expect(description).toMatch(/not a Peer location or a Node deployment location/)
  })

  it('paints a fill only for countries in the current Network scope', async () => {
    stubFetch(geometryResponse)
    const mainnet = network({ networkKey: 'mainnet' })
    const testnet = network({
      networkKey: 'testnet',
      displayName: 'Testnet',
      geo: { countries: [{ ...deCountry, staleCount: 0, centroidLat: 60.2, centroidLon: 18.7 }], knownCountryCount: 2, unknownCountryCount: 0, availablePeerCount: 2, unknownWithPublicIpCount: 0 },
    })
    const { container } = renderMap({ networks: [mainnet, testnet], networkFilter: 'testnet' })
    await screen.findByRole('img', { name: 'Peer countries map' })

    await waitFor(() => expect(mapImage(container).querySelectorAll('.home-geo-observed path')).toHaveLength(1))
    expect(screen.getByRole('button', { name: 'Germany · 2 records' })).toBeTruthy()
    expect(screen.queryByRole('button', { name: /Sweden/ })).toBeNull()
  })

  it('never paints a clipped outline that degenerated into a flat line', async () => {
    // Natural Earth clips Antarctica at the southern projection bound, so its
    // ring collapses onto the bottom edge: "M1000 394L0 394...". Painting it
    // draws a stray rule under the whole map.
    const clipped = { code: 'AQ', path: 'M1000 394L0 394L2.6 394L7.6 394Z' }
    stubFetch(() => ({
      ok: true,
      status: 200,
      text: () => Promise.resolve(JSON.stringify({ ...geometry, countries: [geometry.countries[0], clipped] })),
    }))
    const { container } = renderMap()
    await screen.findByRole('img', { name: 'Peer countries map' })

    const painted = [...mapImage(container).querySelectorAll('.home-geo-land path')].map((path) => path.getAttribute('d'))
    expect(painted).toContain(geometry.countries[0].path)
    expect(painted, 'a zero-height outline is not a country shape').not.toContain(clipped.path)
  })

  it('keeps a country reachable when its outline cannot be painted', async () => {
    const outline = { code: 'AQ', path: 'M1000 394L0 394L2.6 394L7.6 394Z' }
    const observed = { countryCode: 'AQ', count: 4, staleCount: 0, centroidLat: -60, centroidLon: 0 }
    stubFetch(() => ({
      ok: true,
      status: 200,
      text: () => Promise.resolve(JSON.stringify({ ...geometry, countries: [geometry.countries[0], outline] })),
    }))
    renderMap({ networks: [network({ geo: { countries: [observed], knownCountryCount: 4, unknownCountryCount: 0, availablePeerCount: 4, unknownWithPublicIpCount: 0 } })] })
    await screen.findByRole('img', { name: 'Peer countries map' })

    const marker = screen.getByRole('button', { name: 'Antarctica · 4 records' })
    fireEvent.click(marker)
    expect(screen.getByRole('tooltip').textContent).toBe('Antarctica · 4 records')
  })

  it('draws a bare dot for a single record and a bounded numbered marker above it', async () => {
    stubFetch(geometryResponse)
    // Both countries carry a Server representative point; the first holds a
    // single record, the second a larger quantity well clear of the first.
    const singleCountry = { ...seCountry, count: 1 }
    const multipleCountry = { ...deCountry, staleCount: 0, count: 3, centroidLat: 40, centroidLon: 0 }
    renderMap({ networks: [network({ geo: { countries: [singleCountry, multipleCountry], knownCountryCount: 4, unknownCountryCount: 0, availablePeerCount: 4, unknownWithPublicIpCount: 0 } })] })
    await screen.findByRole('img', { name: 'Peer countries map' })

    // One record needs no numeral: the marker is Emerald's 8px dot.
    const single = screen.getByRole('button', { name: 'Sweden · 1 records' })
    expect(single.querySelector('.home-geo-marker-label')).toBeNull()
    expect(Number(single.querySelector('.home-geo-marker-dot')?.getAttribute('r')) * 2).toBe(8)

    // A larger quantity keeps a numeral in Emerald's 14px marker.
    const multiple = screen.getByRole('button', { name: 'Germany · 3 records' })
    const radius = Number(multiple.querySelector('.home-geo-marker-dot')?.getAttribute('r'))
    expect(radius * 2).toBe(14)
    expect(multiple.querySelector('.home-geo-marker-label')?.textContent).toBe('3')
  })

  it('abbreviates a large quantity so the numeral always fits its marker', async () => {
    stubFetch(geometryResponse)
    renderMap({ networks: [network({ geo: { countries: [{ ...seCountry, count: 999_500 }], knownCountryCount: 999_500, unknownCountryCount: 0, availablePeerCount: 999_500, unknownWithPublicIpCount: 0 } })] })
    await screen.findByRole('img', { name: 'Peer countries map' })

    const marker = screen.getByRole('button', { name: 'Sweden · 999,500 records' })
    const label = marker.querySelector('.home-geo-marker-label')!.textContent!
    expect(label, 'abbreviation never promotes past four glyphs').toBe('1M')
    expect(label.length).toBeLessThanOrEqual(4)
    const radius = Number(marker.querySelector('.home-geo-marker-dot')!.getAttribute('r'))
    expect(radius).toBeGreaterThanOrEqual(7)
    expect(radius).toBeLessThanOrEqual(11)
    expect(radius * 2, 'disc fits its own glyphs: ' + label).toBeGreaterThanOrEqual(label.length * 4.5 + 3)
  })

  it('shrinks a long numeral instead of letting it spill outside its marker', async () => {
    stubFetch(geometryResponse)
    const long = { ...seCountry, count: 2002 }
    const huge = { ...deCountry, staleCount: 0, count: 999_500, centroidLat: 40, centroidLon: 0 }
    const wide = { ...xkCountry, count: 999, centroidLat: -20, centroidLon: 0 }
    renderMap({ networks: [network({ geo: { countries: [long, huge, wide], knownCountryCount: 2002 + 999_500 + 999, unknownCountryCount: 0, availablePeerCount: 2002 + 999_500 + 999, unknownWithPublicIpCount: 0 } })] })
    await screen.findByRole('img', { name: 'Peer countries map' })

    const fontSizeOf = (name: string) => {
      const label = screen.getByRole('button', { name }).querySelector('.home-geo-marker-label') as SVGTextElement
      return { glyphs: (label.textContent ?? '').length, size: Number.parseFloat(label.style.fontSize) }
    }
    // Emerald's 10px label is kept for short numerals and only reduced when the
    // abbreviation still needs more room than the bounded disc has.
    expect(fontSizeOf('Sweden · 2,002 records'), 'a four-glyph numeral steps down').toEqual({ glyphs: 4, size: 7 })
    expect(fontSizeOf('Germany · 999,500 records'), 'an abbreviation stays at Emerald size').toEqual({ glyphs: 2, size: 10 })
    expect(fontSizeOf('Kosovo · 999 records')).toEqual({ glyphs: 3, size: 8 })
  })

  it('drops the numeral where markers crowd, and keeps every exact count on the tooltip', async () => {
    stubFetch(geometryResponse)
    renderMap({ networks: [network({ geo: { countries: [seCountry, { ...deCountry, staleCount: 0, centroidLat: 60.2, centroidLon: 18.7 }] } })] })
    await screen.findByRole('img', { name: 'Peer countries map' })

    for (const [name, count] of [['Sweden', '3'], ['Germany', '2']] as const) {
      const marker = screen.getByRole('button', { name: name + ' · ' + count + ' records' })
      expect(marker.querySelector('.home-geo-marker-label'), name + ' is crowded').toBeNull()
      fireEvent.keyDown(marker, { key: 'Enter' })
      expect(screen.getByRole('tooltip').textContent).toBe(name + ' · ' + count + ' records')
      fireEvent.keyDown(document, { key: 'Escape' })
      expect(screen.queryByRole('tooltip')).toBeNull()
    }
  })

  it.each(['click', 'Enter', ' '] as const)('opens a named country marker tooltip using %s', async (activation) => {
    stubFetch(geometryResponse)
    renderMap({ networks: [network({ geo: { countries: [seCountry], knownCountryCount: 3, unknownCountryCount: 0, availablePeerCount: 3 } })] })
    await screen.findByRole('img', { name: 'Peer countries map' })
    const marker = screen.getByRole('button', { name: 'Sweden · 3 records' })
    expect(marker.getAttribute('tabindex')).toBe('0')
    expect(screen.queryByRole('tooltip')).toBeNull()
    if (activation === 'click') fireEvent.click(marker)
    else fireEvent.keyDown(marker, { key: activation })
    expect(screen.getByRole('tooltip').textContent).toBe('Sweden · 3 records')
  })

  it('keeps a tapped tooltip open through the synthetic mouse-leave a touch tap emits', async () => {
    stubFetch(geometryResponse)
    const { container } = renderMap({ networks: [network({ geo: { countries: [seCountry], knownCountryCount: 3, unknownCountryCount: 0, availablePeerCount: 3 } })] })
    await screen.findByRole('img', { name: 'Peer countries map' })
    const marker = screen.getByRole('button', { name: 'Sweden · 3 records' })

    fireEvent.click(marker)
    expect(screen.getByRole('tooltip')).toBeTruthy()
    fireEvent.mouseLeave(marker)
    fireEvent.mouseLeave(mapImage(container))
    expect(screen.getByRole('tooltip')).toBeTruthy()
    fireEvent.keyDown(document, { key: 'Escape' })
    expect(screen.queryByRole('tooltip')).toBeNull()
  })

  it('lets a second tap close the marker it opened', async () => {
    stubFetch(geometryResponse)
    renderMap({ networks: [network({ geo: { countries: [seCountry], knownCountryCount: 3, unknownCountryCount: 0, availablePeerCount: 3 } })] })
    await screen.findByRole('img', { name: 'Peer countries map' })
    const marker = screen.getByRole('button', { name: 'Sweden · 3 records' })

    fireEvent.click(marker)
    expect(screen.getByRole('tooltip')).toBeTruthy()
    fireEvent.click(marker)
    expect(screen.queryByRole('tooltip')).toBeNull()
  })

  it('previews a hovered marker and hides the preview when the mouse leaves', async () => {
    stubFetch(geometryResponse)
    renderMap({ networks: [network({ geo: { countries: [seCountry], knownCountryCount: 3, unknownCountryCount: 0, availablePeerCount: 3 } })] })
    await screen.findByRole('img', { name: 'Peer countries map' })
    const marker = screen.getByRole('button', { name: 'Sweden · 3 records' })

    fireEvent.mouseEnter(marker)
    expect(screen.getByRole('tooltip').textContent).toBe('Sweden · 3 records')
    fireEvent.mouseLeave(marker)
    expect(screen.queryByRole('tooltip')).toBeNull()
  })

  it('releases a pin when the pinned country leaves the projection', async () => {
    stubFetch(geometryResponse)
    const { rerender } = renderMap({ networks: [network({ geo: { countries: [seCountry], knownCountryCount: 3, unknownCountryCount: 0, availablePeerCount: 3 } })] })
    await screen.findByRole('img', { name: 'Peer countries map' })
    fireEvent.click(screen.getByRole('button', { name: 'Sweden · 3 records' }))
    expect(screen.getByRole('tooltip')).toBeTruthy()

    rerender(<GeoWorldMap networks={[network({ geo: { countries: [{ ...deCountry, staleCount: 0, centroidLat: 60.2, centroidLon: 18.7 }], knownCountryCount: 2, unknownCountryCount: 0, availablePeerCount: 2 } })]} networkFilter="all" loading={false} hasProjection />)
    expect(screen.queryByRole('tooltip')).toBeNull()

    fireEvent.mouseEnter(screen.getByRole('button', { name: 'Germany · 2 records' }))
    expect(screen.getByRole('tooltip').textContent).toBe('Germany · 2 records')
  })

  it('never requests a basemap while the Owner disabled Geo', async () => {
    const fetchMock = stubFetch(geometryResponse)
    renderMap({ networks: [network({ geo: { state: 'disabled', scope: 'unavailable', countries: null, knownCountryCount: null, unknownCountryCount: null, availablePeerCount: null } })] })

    expect(screen.getByRole('status').textContent).toBe('Peer countries · Disabled by server')
    expect(screen.queryByRole('img', { name: 'Peer countries map' })).toBeNull()
    expect(fetchMock).not.toHaveBeenCalled()
  })

  it('rejects an unusable payload such as the SPA fallback document', async () => {
    stubFetch(() => ({ ok: true, status: 200, text: () => Promise.resolve('<!doctype html><title>PlatPulse</title>') }))
    renderMap()

    // The country data is still whatever the Server said; only the basemap is
    // gone, so both facts are announced together rather than one hiding the other.
    await waitFor(() => expect(screen.getByRole('status').textContent).toBe('Map unavailable · 1 unknown locations'))
    expect(screen.queryByRole('img', { name: 'Peer countries map' })).toBeNull()
  })

  it('keeps loading, unavailable, never-observed, and a real zero distinct and announced', async () => {
    stubFetch(geometryResponse)
    const { rerender } = renderMap({ networks: [], loading: true, hasProjection: false })
    expect(screen.getByRole('status').textContent).toBe('Loading data')
    expect(screen.queryByRole('img', { name: 'Peer countries map' })).toBeNull()

    rerender(<GeoWorldMap networks={[]} networkFilter="all" loading={false} hasProjection={false} />)
    expect(screen.getByRole('status').textContent).toBe('Data unavailable')

    const unobserved = network({ geo: { state: 'unknown', scope: 'unobserved', countries: null, knownCountryCount: null, unknownCountryCount: null, availablePeerCount: null } })
    rerender(<GeoWorldMap networks={[unobserved]} networkFilter="all" loading={false} hasProjection />)
    expect(screen.getByRole('status').textContent).toBe('No observations yet')

    const zero = network({ geo: { countries: [], knownCountryCount: 0, unknownCountryCount: 0, availablePeerCount: 0, unknownWithPublicIpCount: 0 } })
    rerender(<GeoWorldMap networks={[zero]} networkFilter="all" loading={false} hasProjection />)
    await screen.findByRole('img', { name: 'Peer countries map' })
    expect(screen.getByRole('status').textContent).toBe('No data')

    rerender(<GeoWorldMap networks={[]} networkFilter="all" loading={false} hasProjection />)
    expect(screen.getByRole('status').textContent).toBe('No data')
  })

  it('announces stale data and an unknown or varying observation without inventing a zero', async () => {
    stubFetch(geometryResponse)
    const { rerender } = renderMap({ networks: [network({ geo: { state: 'stale', scope: 'partial' }, peers: { state: 'ok', freshness: 'stale' } })] })
    // Wait for the basemap so the notice reports the data state, not that load.
    await screen.findByRole('img', { name: 'Peer countries map' })
    expect(screen.getByRole('status').textContent).toBe('Data stale · 1 unknown locations')

    // No country carries a last-good Stale record here, so the observation
    // dimension is the only thing left to report.
    const clean = { countries: [seCountry], knownCountryCount: 3, unknownCountryCount: 0, availablePeerCount: 3, unknownWithPublicIpCount: 0 }
    rerender(<GeoWorldMap networks={[network({ geo: clean, peers: { state: 'unknown', freshness: 'unknown' } })]} networkFilter="all" loading={false} hasProjection />)
    expect(screen.getByRole('status').textContent).toBe('Observation status unknown')

    const geo = { countries: [seCountry], knownCountryCount: 3, unknownCountryCount: 0, availablePeerCount: 3, unknownWithPublicIpCount: 0 }
    rerender(<GeoWorldMap networks={[network({ geo }), network({ networkKey: 'testnet', displayName: 'Testnet', geo, peers: { state: 'unknown', freshness: 'unknown' } })]} networkFilter="all" loading={false} hasProjection />)
    expect(screen.getByRole('status').textContent).toBe('Observation status varies')
  })

  it('shows the in-scope Peer total as one figure in the corner', async () => {
    stubFetch(geometryResponse)
    // The Server's Peer-record denominator is Known 6 + Unknown 1 for this
    // scope; the figure is that Server number, not a browser-side count.
    renderMap({ networks: [network({ nodes: [{ nodeId: 'n1', health: 'healthy' }] })] })
    await screen.findByRole('img', { name: 'Peer countries map' })
    const map = screen.getByRole('region', { name: 'Peer countries' })
    const counters = map.querySelector('.home-geo-counters')

    expect(counters, 'the corner indicator exists').toBeTruthy()
    expect(counters!.querySelectorAll('.home-geo-counter'), 'one figure only').toHaveLength(1)
    const figure = counters!.querySelector('.home-geo-counter')!
    expect(figure.querySelectorAll('.home-geo-counter-dot')).toHaveLength(1)
    expect(figure.textContent).toContain('Peers: 7')
  })

  it('never invents a corner figure while the projection is unavailable or loading', async () => {
    stubFetch(geometryResponse)
    const { rerender } = renderMap({ networks: [network()], loading: true, hasProjection: false })
    expect(screen.getByRole('region', { name: 'Peer countries' }).querySelector('.home-geo-counters')).toBeNull()

    rerender(<GeoWorldMap networks={[network()]} networkFilter="all" loading={false} hasProjection={false} />)
    expect(screen.getByRole('region', { name: 'Peer countries' }).querySelector('.home-geo-counters')).toBeNull()
  })

  it('never invents a Peer total for a scope without a Peer Snapshot', async () => {
    stubFetch(geometryResponse)
    // A Network that never reported a successful Peer Snapshot has no basis at
    // all, so the Server omits the denominator and no total may be presented as
    // a real zero. (How several Networks' denominators add up is owned by
    // homeGeo.test.ts.)
    renderMap({ networks: [network({ geo: { state: 'current', scope: 'unobserved', countries: null, knownCountryCount: null, unknownCountryCount: null, availablePeerCount: null } })] })
    await screen.findByRole('img', { name: 'Peer countries map' })

    expect(screen.getByRole('region', { name: 'Peer countries' }).querySelector('.home-geo-counters')).toBeNull()
    expect(screen.getByRole('status').textContent).toBe('No observations yet')
  })

  it('shows an authoritative zero when every in-scope Peer Snapshot was empty', async () => {
    stubFetch(geometryResponse)
    renderMap({ networks: [network({ geo: { countries: [], knownCountryCount: 0, unknownCountryCount: 0, availablePeerCount: 0 } })] })
    await screen.findByRole('img', { name: 'Peer countries map' })

    const figure = screen.getByRole('region', { name: 'Peer countries' }).querySelector('.home-geo-counter')
    expect(figure?.textContent).toContain('Peers: 0')
  })

  it('counts Peers in exactly the scope the map covers', async () => {
    stubFetch(geometryResponse)
    const mainnet = network({ networkKey: 'mainnet', nodes: [{ nodeId: 'a', health: 'healthy' }, { nodeId: 'b', health: 'healthy' }] })
    const testnet = network({
      networkKey: 'testnet',
      displayName: 'Testnet',
      nodes: [{ nodeId: 'c', health: 'unhealthy' }],
      geo: { countries: [{ ...deCountry, staleCount: 0 }], knownCountryCount: 2, unknownCountryCount: 0, availablePeerCount: 2, unknownWithPublicIpCount: 0 },
    })
    const countersOf = () => screen.getByRole('region', { name: 'Peer countries' }).querySelector('.home-geo-counters')!.textContent!

    const { rerender } = renderMap({ networks: [mainnet, testnet], networkFilter: 'testnet' })
    await screen.findByRole('img', { name: 'Peer countries map' })
    expect(countersOf(), 'the filtered scope counts only its own Peers').toContain('Peers: 2')

    rerender(<GeoWorldMap networks={[mainnet, testnet]} networkFilter="all" loading={false} hasProjection />)
    expect(countersOf(), 'All Networks adds every scope it covers up').toContain('Peers: 9')
  })

  it('renders the map itself as the only interactive surface', async () => {
    stubFetch(geometryResponse)
    const { container } = renderMap()
    await screen.findByRole('img', { name: 'Peer countries map' })

    // Interaction lives on the map, not on controls beside it: the country
    // fills and the quantity markers are the tab stops.
    const map = screen.getByRole('region', { name: 'Peer countries' })
    expect(map.querySelectorAll('button')).toHaveLength(0)
    const markers = screen.getAllByRole('button', { name: /records$/ })
    expect(markers.length).toBeGreaterThan(0)
    for (const marker of markers) expect(marker.getAttribute('tabindex')).toBe('0')
    expect(container.querySelectorAll('.home-geo-observed path')).toHaveLength(2)
  })

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
    expect(screen.getByRole('status').textContent).toBe('Map unavailable')
    // The fallback paints nothing: no control, no sentence, no dialog.
    expect(screen.queryByRole('button')).toBeNull()
    expect(screen.queryByRole('dialog')).toBeNull()
    consoleError.mockRestore()
  })
})
