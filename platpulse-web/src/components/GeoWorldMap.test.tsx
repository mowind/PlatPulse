import { act, cleanup, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { PublicNetwork, PublicNode, PublicNodeDetail } from '../api/generated'
import { geoMapStatus, homeGeoOverview, homeNodeSelectionGeoOverview, nodeGeoOverview } from '../homeGeo'
import { NODE_PEER_COUNTRIES_HEADING } from './geoPresentation'

/**
 * GeoWorldMap renders through ECharts, which paints into a canvas with no
 * per-country element. The encoding itself is covered by
 * mapChartOption.test.ts; these tests cover the component's own contract —
 * that it is driven purely by an already-projected overview plus a status,
 * the accessible image, the counters, the screen-reader country list, the
 * states it announces, and the chart lifecycle. ECharts is stubbed so the
 * suite stays canvas-free.
 */
const mocks = vi.hoisted(() => ({
  init: vi.fn(),
  registerMap: vi.fn(),
  use: vi.fn(),
  setOption: vi.fn(),
  resize: vi.fn(),
  dispose: vi.fn(),
  dispatchAction: vi.fn(),
  on: vi.fn(),
}))

vi.mock('echarts/core', () => ({ init: mocks.init, registerMap: mocks.registerMap, use: mocks.use }))
vi.mock('echarts/charts', () => ({ MapChart: {}, ScatterChart: {} }))
vi.mock('echarts/components', () => ({ GeoComponent: {}, TooltipComponent: {} }))
vi.mock('echarts/renderers', () => ({ CanvasRenderer: {} }))

import GeoWorldMap from './GeoWorldMap'

beforeEach(() => {
  mocks.init.mockImplementation(() => ({
    setOption: mocks.setOption,
    resize: mocks.resize,
    dispose: mocks.dispose,
    dispatchAction: mocks.dispatchAction,
    getZr: () => ({ on: mocks.on }),
  }))
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  vi.clearAllMocks()
})

const geojson = {
  type: 'FeatureCollection',
  features: [
    { type: 'Feature', properties: { name: 'Sweden', iso2: 'SE' }, geometry: { type: 'Polygon', coordinates: [] } },
    { type: 'Feature', properties: { name: 'Germany', iso2: 'DE' }, geometry: { type: 'Polygon', coordinates: [] } },
  ],
}

function stubBasemap(payload: unknown = geojson) {
  const fetchMock = vi.fn(() => Promise.resolve({ ok: true, status: 200, json: () => Promise.resolve(payload) }))
  vi.stubGlobal('fetch', fetchMock)
  return fetchMock
}

const seCountry = { countryCode: 'SE', count: 3, staleCount: 0, centroidLat: 60.1282, centroidLon: 18.6435 }
const deCountry = { countryCode: 'DE', count: 2, staleCount: 1, centroidLat: null, centroidLon: null }
const xkCountry = { countryCode: 'XK', count: 1, staleCount: 0, centroidLat: null, centroidLon: null }

function network(overrides: { geo?: Record<string, unknown>; nodes?: Array<{ nodeId: string; health: string }> } = {}): PublicNetwork {
  return {
    networkKey: 'mainnet',
    displayName: 'Mainnet',
    geo: {
      state: 'current',
      scope: 'complete',
      countries: [seCountry, deCountry, xkCountry],
      knownCountryCount: 6,
      unknownCountryCount: 1,
      // Server-shaped: the two reason buckets always sum to the Unknown count.
      unknownWithPublicIpCount: 0,
      unknownWithoutRemoteIpCount: 1,
      availablePeerCount: 7,
      ...overrides.geo,
    },
    peers: { state: 'ok', freshness: 'current' },
    nodes: overrides.nodes ?? [],
    validators: [],
  } as unknown as PublicNetwork
}

/** A Node Detail projection carrying only the fields the map consumes. */
function nodeDetail(overrides: { geo?: Record<string, unknown>; displayName?: string; peers?: Record<string, unknown> } = {}): PublicNodeDetail {
  return {
    nodeId: 'node-1',
    displayName: overrides.displayName ?? 'Validator A',
    peers: { state: 'ok', freshness: 'current', ...overrides.peers },
    geo: {
      state: 'current',
      scope: 'complete',
      countries: [seCountry, deCountry],
      knownCountryCount: 5,
      unknownCountryCount: 1,
      unknownWithPublicIpCount: 0,
      unknownWithoutRemoteIpCount: 1,
      availablePeerCount: 6,
      ...overrides.geo,
    },
  } as unknown as PublicNodeDetail
}

type MapOptions = { networkFilter?: string; loading?: boolean; hasProjection?: boolean }

function propsFor(networks: PublicNetwork[], options: MapOptions = {}) {
  const overview = homeGeoOverview(networks, options.networkFilter ?? 'all')
  return {
    overview,
    status: geoMapStatus(overview, { loading: options.loading ?? false, hasProjection: options.hasProjection ?? true }),
  }
}

function renderMap(networks: PublicNetwork[] = [network()], options: MapOptions = {}) {
  return render(<GeoWorldMap {...propsFor(networks, options)} />)
}

describe('GeoWorldMap', () => {
  it('exposes the map as one labelled image that states what the markers are not', async () => {
    stubBasemap()
    renderMap()
    const image = await screen.findByRole('img')
    const label = image.getAttribute('aria-label') ?? ''
    expect(label).toContain('3 countries have Peer records')
    expect(label).toContain('Server-provided country representative point')
    expect(label).toContain('not a Peer location or a Node deployment location')
  })

  it('keeps the in-scope Peer total and the Unknown bucket in one pointer-inert chip', async () => {
    stubBasemap()
    const { container } = renderMap()
    await screen.findByRole('img')
    const counters = container.querySelector('[data-slot="geo-counters"]')
    expect(counters?.textContent).toContain('7')
    expect(counters?.getAttribute('aria-label'))
      .toBe('7 Peer records in scope for All Networks, 1 with no retained country attribution: 1 without a usable public remote IP')
    expect(counters?.getAttribute('title')).toContain('not unique Peers or Node locations')
    expect(counters?.getAttribute('title')).toContain('1 unknown location: 1 without a usable public remote IP')
    expect(counters?.className).toContain('pointer-events-none')
    const unknown = counters?.querySelector('[data-slot="geo-counter-unknown"]')
    expect(unknown?.textContent).toBe('1 unknown')
    // The second dot states a coverage gap, so it never pulses like the live
    // Peer total and never takes the map's amber.
    const dot = unknown?.querySelector('[data-slot="geo-counter-unknown-dot"]')
    expect(dot).toBeTruthy()
    expect(dot?.className).not.toContain('animate-pulse')
    expect(dot?.className).toContain('bg-muted-foreground')
  })

  it('keeps every country reachable as text, including one with no representative point', async () => {
    stubBasemap()
    const { container } = renderMap()
    await screen.findByRole('img')
    const list = container.querySelector('[data-slot="geo-country-list"]')?.textContent ?? ''
    expect(list).toContain('Sweden')
    expect(list).toContain('3 records')
    expect(list).toContain('2 records, 1 stale')
    expect(list).toContain('no representative point, not plotted')
    expect(list).toContain('1 unknown location: 1 without a usable public remote IP')
  })

  it('creates one chart instance, updates it in place, and disposes on unmount', async () => {
    stubBasemap()
    const view = renderMap()
    await waitFor(() => expect(mocks.setOption).toHaveBeenCalled())
    await waitFor(() => expect(mocks.init).toHaveBeenCalledTimes(1))
    view.unmount()
    expect(mocks.dispose).toHaveBeenCalledTimes(1)
  })

  it('updates in place and resizes without reinitializing', async () => {
    stubBasemap()
    let resize = () => {}
    const disconnect = vi.fn()
    vi.stubGlobal('ResizeObserver', class {
      constructor(callback: () => void) { resize = callback }
      observe() {}
      disconnect = disconnect
    })
    const view = renderMap()
    await waitFor(() => expect(mocks.init).toHaveBeenCalledTimes(1))
    await waitFor(() => expect(mocks.setOption).toHaveBeenCalled())
    // The marker size follows the observed quantity, never the viewport.
    expect(mocks.setOption.mock.lastCall?.[0].series[1].data.map((datum: { symbolSize: number }) => datum.symbolSize)).toEqual([14])
    expect(mocks.setOption.mock.lastCall?.[0].series[0].data).toEqual(expect.arrayContaining([expect.objectContaining({code: 'DE', value: 2})]))
    // The map polygons stay silent; only the scatter marker answers.
    expect(mocks.setOption.mock.lastCall?.[0].series[0].tooltip).toEqual({ show: false })
    act(() => { resize() })
    await waitFor(() => expect(mocks.resize).toHaveBeenCalled())
    view.rerender(<GeoWorldMap {...propsFor([network()], { networkFilter: 'mainnet' })} />)
    expect(mocks.init).toHaveBeenCalledTimes(1)
    const click = mocks.on.mock.calls.find(([name]) => name === 'click')?.[1]
    act(() => click({ target: undefined }))
    expect(mocks.dispatchAction).toHaveBeenCalledWith({type: 'hideTip'})
    view.unmount()
    expect(disconnect).toHaveBeenCalledTimes(1)
    expect(mocks.dispose).toHaveBeenCalledTimes(1)
  })

  it('does not rebuild the map graphic when a Public refetch did not change the plotted countries', async () => {
    stubBasemap()
    const view = renderMap()
    await waitFor(() => expect(mocks.setOption).toHaveBeenCalledTimes(1))

    // An SSE-driven refetch returns a new Network array with equal country
    // values. Re-applying the option would make ECharts re-parse the world
    // geometry and rebuild the graphic for no visible change.
    view.rerender(<GeoWorldMap {...propsFor([network()])} />)
    await waitFor(() => expect(mocks.setOption).toHaveBeenCalledTimes(1))

    // A real country change still reaches the canvas.
    view.rerender(
      <GeoWorldMap {...propsFor([network({ geo: { countries: [{ ...seCountry, count: 9 }, deCountry, xkCountry] } })])} />,
    )
    await waitFor(() => expect(mocks.setOption.mock.calls.length).toBeGreaterThan(1))
  })

  it('never requests a basemap while the Owner disabled Geo', async () => {
    vi.resetModules()
    const fetchMock = vi.fn()
    vi.stubGlobal('fetch', fetchMock)
    const { default: FreshMap } = await import('./GeoWorldMap')
    render(<FreshMap {...propsFor([network({ geo: { state: 'disabled' } })])} />)
    expect(fetchMock).not.toHaveBeenCalled()
  })

  it('never requests a basemap when the projection itself is unavailable', async () => {
    const fetchMock = vi.fn()
    vi.stubGlobal('fetch', fetchMock)
    renderMap([network()], { hasProjection: false })
    expect(fetchMock).not.toHaveBeenCalled()
  })

  it('reports an unusable basemap payload instead of drawing a broken map', async () => {
    vi.resetModules()
    stubBasemap({ type: 'text/html' })
    const { default: FreshMap } = await import('./GeoWorldMap')
    render(<FreshMap {...propsFor([network()])} />)
    expect(await screen.findByText('Map unavailable')).toBeTruthy()
  })

  it('announces stale and never-observed scopes without inventing a zero', async () => {
    stubBasemap()
    const { unmount } = renderMap([network({ geo: { state: 'stale' } })])
    await screen.findByRole('img')
    // Staleness keeps the notice; the Unknown count lives in the chip beside
    // the total instead of becoming a second error-looking line.
    expect(screen.getByRole('status').textContent).toBe('Map data stale')
    const counters = screen.getByRole('note', { name: /^7 Peer records in scope for All Networks/ })
    expect(counters.querySelector('[data-slot="geo-counter"]')?.textContent).toBe('Peers: 7')
    expect(counters.querySelector('[data-slot="geo-counter-unknown"]')?.textContent).toBe('1 unknown')
    unmount()

    stubBasemap()
    renderMap([
      network({ geo: { state: 'current', scope: 'unobserved', countries: [], knownCountryCount: null, unknownCountryCount: null, availablePeerCount: null } }),
    ])
    await screen.findByRole('img')
    expect(document.body.textContent).toContain('No observations yet')
  })

  it('never invents a Peer total while the projection is unavailable or loading', async () => {
    stubBasemap()
    const { container, unmount } = renderMap([network()], { hasProjection: false })
    expect(container.querySelector('[data-slot="geo-counters"]')).toBeNull()
    unmount()
    const loading = renderMap([network()], { loading: true })
    expect(loading.container.querySelector('[data-slot="geo-counters"]')).toBeNull()
  })

  it('shows one figure and no notice while every record has a retained country', async () => {
    stubBasemap()
    const { container } = renderMap([
      network({
        geo: {
          // The same six records, all current: nothing stale and nothing
          // unlocated, so the map stays quiet.
          countries: [seCountry, { ...deCountry, staleCount: 0 }, xkCountry],
          knownCountryCount: 6,
          unknownCountryCount: 0,
          unknownWithPublicIpCount: 0,
          unknownWithoutRemoteIpCount: 0,
          availablePeerCount: 6,
        },
      }),
    ])
    await screen.findByRole('img')
    expect(container.querySelector('[data-slot="geo-counter-unknown"]')).toBeNull()
    expect(container.querySelector('[data-slot="map-status"]')).toBeNull()
    expect(container.querySelector('[data-slot="geo-counters"]')?.getAttribute('aria-label'))
      .toBe('6 Peer records in scope for All Networks')
    expect(container.querySelector('[data-slot="geo-counters"]')?.getAttribute('title'))
      .toBe('6 Peer records in scope for All Networks; not unique Peers or Node locations.')
  })

  it('states the empty map plainly when no country was resolved', async () => {
    stubBasemap()
    const { container } = renderMap([
      network({
        geo: {
          countries: [],
          knownCountryCount: 0,
          unknownCountryCount: 5,
          unknownWithPublicIpCount: 5,
          unknownWithoutRemoteIpCount: 0,
          availablePeerCount: 5,
        },
      }),
    ])
    await screen.findByRole('img')
    expect(screen.getByRole('status').textContent).toBe('No locations to show')
    expect(container.querySelector('[data-slot="geo-counter"]')?.textContent).toBe('Peers: 5')
    expect(container.querySelector('[data-slot="geo-counter-unknown"]')?.textContent).toBe('5 unknown')
    expect(container.querySelector('[data-slot="geo-country-list"]')?.textContent)
      .toContain('5 unknown locations: 5 without a retained country result')
  })

  it('names the basis it stands on and says why a Node selection reads empty', async () => {
    stubBasemap()
    const selected = {
      nodeId: 'node-1',
      displayName: 'Validator A',
      health: 'healthy',
      peerCountries: {
        scope: 'complete',
        countries: [seCountry],
        knownCountryCount: 3,
        unknownCountryCount: 0,
        unknownWithPublicIpCount: 0,
        unknownWithoutRemoteIpCount: 0,
        availablePeerCount: 3,
      },
    } as unknown as PublicNode
    const overview = homeNodeSelectionGeoOverview([selected], [network()], 'All Networks', null)
    const { container } = render(
      <GeoWorldMap
        overview={overview}
        status={geoMapStatus(overview, { loading: false, hasProjection: true })}
        selectionBasis="node-selection"
      />,
    )
    const region = screen.getByRole('region', { name: 'Peer countries' })
    // The basis is stated on the region so a reader can tell a Network reading
    // from a reading of the current list selection.
    expect(region.getAttribute('data-scope-basis')).toBe('node-selection')
    await screen.findByRole('img')
    expect(container.querySelector('[data-slot="geo-counter"]')?.textContent).toBe('Peers: 3')
    expect(container.querySelector('[data-slot="geo-country-list"]')?.textContent).toContain('Sweden: 3 records')
  })

  it('explains an empty Node selection instead of inventing a zero', async () => {
    stubBasemap()
    // A selection with no Node is no reading at all rather than a zero the
    // browser invented, so the empty arm names the selection.
    const nothing = homeNodeSelectionGeoOverview([], [network()], 'All Networks', null)
    const { container } = render(
      <GeoWorldMap
        overview={nothing}
        status={geoMapStatus(nothing, { loading: false, hasProjection: true })}
        selectionBasis="node-selection"
      />,
    )
    expect(screen.getByRole('status').textContent).toBe('No Active Nodes match these filters')
    expect(container.querySelector('[data-slot="geo-counter"]')).toBeNull()
    expect(container.querySelector('[data-slot="geo-counter-unknown"]')).toBeNull()
  })

  it('names the Node unit, drops the Network filter hook, and keeps the same states', async () => {
    stubBasemap()
    const overview = nodeGeoOverview(nodeDetail())
    const { container } = render(
      <GeoWorldMap
        overview={overview}
        status={geoMapStatus(overview, { loading: false, hasProjection: true })}
        heading={NODE_PEER_COUNTRIES_HEADING}
      />,
    )
    const region = screen.getByRole('region', { name: NODE_PEER_COUNTRIES_HEADING })
    expect(region.getAttribute('data-scope')).toBe('complete')
    // A Node scope is not a Home Network selection, so the Home data hook is
    // absent rather than repurposed.
    expect(region.getAttribute('data-network-filter')).toBeNull()
    expect(await screen.findByRole('img', { name: /^Node Peer countries map/ })).toBeTruthy()
    expect(container.querySelector('[data-slot="geo-counters"]')?.getAttribute('aria-label'))
      .toBe('6 Peer records in scope for Validator A, 1 with no retained country attribution: 1 without a usable public remote IP')
  })

  it('keeps the rest of the page alive when the map cannot render', async () => {
    vi.resetModules()
    stubBasemap({ type: 'text/html' })
    const [{ default: FreshMap }, { default: FreshBoundary }] = await Promise.all([
      import('./GeoWorldMap'),
      import('./GeoMapBoundary'),
    ])
    render(
      <FreshBoundary label={NODE_PEER_COUNTRIES_HEADING}>
        <FreshMap {...propsFor([network()])} heading={NODE_PEER_COUNTRIES_HEADING} />
      </FreshBoundary>,
    )
    expect(await screen.findByText('Map unavailable')).toBeTruthy()
    expect(screen.getByRole('region', { name: NODE_PEER_COUNTRIES_HEADING })).toBeTruthy()
  })
})
