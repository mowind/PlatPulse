import { cleanup, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { PublicNetwork } from '../api/generated'

/**
 * GeoWorldMap renders through ECharts, which paints into a canvas with no
 * per-country element. The encoding itself is covered by
 * mapChartOption.test.ts; these tests cover the component's own contract —
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
      availablePeerCount: 7,
      ...overrides.geo,
    },
    peers: { state: 'ok', freshness: 'current' },
    nodes: overrides.nodes ?? [],
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
  it('exposes the map as one labelled image that states what the markers are not', async () => {
    stubBasemap()
    renderMap()
    const image = await screen.findByRole('img')
    const label = image.getAttribute('aria-label') ?? ''
    expect(label).toContain('3 countries have Peer records')
    expect(label).toContain('Server-provided country representative point')
    expect(label).toContain('not a Peer location or a Node deployment location')
  })

  it('writes nothing onto the map but one in-scope Peer total', async () => {
    stubBasemap()
    const { container } = renderMap()
    await screen.findByRole('img')
    const counters = container.querySelector('[data-slot="geo-counters"]')
    expect(counters?.textContent).toContain('7')
    expect(counters?.className).toContain('pointer-events-none')
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
  })

  it('creates one chart instance, updates it in place, and disposes on unmount', async () => {
    stubBasemap()
    const view = renderMap()
    await waitFor(() => expect(mocks.setOption).toHaveBeenCalled())
    await waitFor(() => expect(mocks.init).toHaveBeenCalledTimes(1))
    view.unmount()
    expect(mocks.dispose).toHaveBeenCalledTimes(1)
  })

  it('never requests a basemap while the Owner disabled Geo', async () => {
    vi.resetModules()
    const fetchMock = vi.fn()
    vi.stubGlobal('fetch', fetchMock)
    const { default: FreshMap } = await import('./GeoWorldMap')
    render(
      <FreshMap networks={[network({ geo: { state: 'disabled' } })]} networkFilter="all" loading={false} hasProjection />,
    )
    expect(fetchMock).not.toHaveBeenCalled()
  })

  it('reports an unusable basemap payload instead of drawing a broken map', async () => {
    vi.resetModules()
    stubBasemap({ type: 'text/html' })
    const { default: FreshMap } = await import('./GeoWorldMap')
    render(<FreshMap networks={[network()]} networkFilter="all" loading={false} hasProjection />)
    expect(await screen.findByText('Map unavailable')).toBeTruthy()
  })

  it('announces stale and never-observed scopes without inventing a zero', async () => {
    stubBasemap()
    const { unmount } = renderMap({ networks: [network({ geo: { state: 'stale' } })] })
    await screen.findByRole('img')
    expect(document.body.textContent).toContain('Data stale')
    unmount()

    stubBasemap()
    renderMap({
      networks: [network({ geo: { state: 'current', scope: 'unobserved', countries: [], knownCountryCount: null, unknownCountryCount: null, availablePeerCount: null } })],
    })
    await screen.findByRole('img')
    expect(document.body.textContent).toContain('No observations yet')
  })

  it('never invents a Peer total while the projection is unavailable or loading', async () => {
    stubBasemap()
    const { container, unmount } = renderMap({ hasProjection: false })
    expect(container.querySelector('[data-slot="geo-counters"]')).toBeNull()
    unmount()
    const loading = renderMap({ loading: true })
    expect(loading.container.querySelector('[data-slot="geo-counters"]')).toBeNull()
  })

  it('keeps the rest of Home alive when the map cannot render', async () => {
    vi.resetModules()
    stubBasemap({ type: 'text/html' })
    const [{ default: FreshMap }, { default: FreshBoundary }] = await Promise.all([
      import('./GeoWorldMap'),
      import('./GeoMapBoundary'),
    ])
    render(
      <FreshBoundary>
        <FreshMap networks={[network()]} networkFilter="all" loading={false} hasProjection />
      </FreshBoundary>,
    )
    expect(await screen.findByText('Map unavailable')).toBeTruthy()
  })
})
