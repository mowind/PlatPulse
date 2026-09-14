import { useEffect, useMemo, useRef, useState } from 'react'
import type { ECharts } from 'echarts/core'
import type { PublicNetwork } from '../api/generated'
import { homeGeoOverview } from '../homeGeo'
import { WORLD_MAP_NAME, loadWorldGeoJson, regionNameByCode, type WorldGeoJson } from '../worldGeometry'
import { mapChartOption, type MapCountry } from './mapChartOption'
import { Empty } from './ui/empty'
import {
  PEER_COUNTRIES_DISABLED_NOTICE,
  PEER_COUNTRIES_HEADING,
  countryDisplayName,
  formatGeoCount,
} from './geoPresentation'

/**
 * ECharts is loaded on demand rather than imported at module scope: only Home
 * needs the map, and the library is around 530 KiB minified. Keeping it out of
 * the entry chunk means Login, Admin, Network and Node Detail never pay for it.
 */
type EChartsCore = typeof import('echarts/core')
let echartsPromise: Promise<EChartsCore> | null = null
function loadECharts() {
  echartsPromise ??= Promise.all([
    import('echarts/core'),
    import('echarts/charts'),
    import('echarts/components'),
    import('echarts/renderers'),
  ]).then(([core, charts, components, renderers]) => {
    core.use([
      charts.MapChart,
      charts.ScatterChart,
      components.GeoComponent,
      components.TooltipComponent,
      renderers.CanvasRenderer,
    ])
    return core
  })
  return echartsPromise
}

/**
 * Home Peer country map (issue #133), rendered by ECharts exactly as
 * komari-theme-emerald@c2c5e88 NodeEarthMaps.vue renders its world map: the
 * same geometry, the same silent transparent geo coordinate system, the same
 * scatter symbol, the same 8px/14px sizes, white 10px aggregate numerals, and
 * the same tooltip box. The option itself lives in mapChartOption.ts so the
 * encoding can be tested without a canvas.
 *
 * PlatPulse keeps its own data semantics: the map plots Peer records by
 * country from Server-provided country representative points, never node
 * locations and never a Peer address, and stale or unknown data stays visible
 * as such. ECharts paints into a canvas, which carries no per-country element,
 * so the same figures are also exposed as a screen-reader list, the container
 * is a labelled image, and the counters stay real text.
 *
 * The chart instance is created once and updated with setOption, so a data or
 * theme change never destroys and rebuilds the canvas.
 */

type GeoWorldMapProps = {
  networks: PublicNetwork[]
  /** Home Network filter; the map always covers the same scope as the list. */
  networkFilter: string
  loading: boolean
  /** False when the Public Projection itself is unavailable. */
  hasProjection: boolean
}

type MapStatus = 'starting' | 'empty' | 'unknown' | 'disabled' | 'current' | 'stale' | 'error'

/** One registration per page load, shared by every mount of the map. */
let worldMapPromise: Promise<{ geojson: WorldGeoJson; names: Map<string, string> }> | null = null
function ensureWorldMap() {
  if (!worldMapPromise) {
    worldMapPromise = loadECharts()
      .then(async (core) => {
        const geojson = await loadWorldGeoJson()
        core.registerMap(WORLD_MAP_NAME, geojson as unknown as Parameters<typeof core.registerMap>[1])
        return { geojson, names: regionNameByCode(geojson) }
      })
      // A failed load must not poison the cache: the next mount retries.
      .catch((error: unknown) => {
        worldMapPromise = null
        throw error
      })
  }
  return worldMapPromise
}

function prefersReducedMotion() {
  return typeof window !== 'undefined' && typeof window.matchMedia === 'function'
    && window.matchMedia('(prefers-reduced-motion: reduce)').matches
}

/** The theme is applied as the dark class on the root element (theme.ts). */
function useIsDark() {
  const [dark, setDark] = useState(
    () => typeof document !== 'undefined' && document.documentElement.classList.contains('dark'),
  )
  useEffect(() => {
    const root = document.documentElement
    const observer = new MutationObserver(() => setDark(root.classList.contains('dark')))
    observer.observe(root, { attributes: true, attributeFilter: ['class'] })
    return () => observer.disconnect()
  }, [])
  return dark
}

export default function GeoWorldMap({ networks, networkFilter, loading, hasProjection }: GeoWorldMapProps) {
  const container = useRef<HTMLDivElement>(null)
  const chart = useRef<ECharts | null>(null)
  const optionRef = useRef<ReturnType<typeof mapChartOption> | null>(null)
  const [world, setWorld] = useState<{ geojson: WorldGeoJson; names: Map<string, string> } | null>(null)
  const [failed, setFailed] = useState(false)
  const dark = useIsDark()

  const overview = useMemo(() => homeGeoOverview(networks, networkFilter), [networks, networkFilter])
  const status: MapStatus = loading
    ? 'starting'
    : !hasProjection
      ? 'unknown'
      : networks.length === 0
        ? 'empty'
        : overview.state

  // The basemap is only fetched when a map can actually be drawn: a Disabled
  // Geo Provider, a Starting projection, and a scope without a country basis
  // never trigger the request, and enabling Geo later loads the map without a
  // reload.
  const needsBasemap = status !== 'disabled' && status !== 'empty' && status !== 'starting' && status !== 'unknown'
  useEffect(() => {
    if (!needsBasemap) return
    let live = true
    ensureWorldMap()
      .then((loaded) => { if (live) setWorld(loaded) })
      .catch(() => { if (live) setFailed(true) })
    return () => { live = false }
  }, [needsBasemap])

  // The instance is created once per mounted basemap and only ever updated
  // afterwards, so a data or theme change never rebuilds the canvas.
  useEffect(() => {
    const element = container.current
    if (!element || !world) return
    let disposed = false
    let instance: ECharts | null = null
    let observer: ResizeObserver | null = null
    loadECharts()
      .then((core) => {
        if (disposed || !container.current) return
        instance = core.init(container.current)
        chart.current = instance
        if (optionRef.current) instance.setOption(optionRef.current)
        observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(() => instance?.resize())
        observer?.observe(container.current)
        if (!observer) instance.resize()
      })
      .catch(() => { if (!disposed) setFailed(true) })
    return () => {
      disposed = true
      observer?.disconnect()
      instance?.dispose()
      chart.current = null
    }
  }, [world])

  const countries = useMemo<MapCountry[]>(
    () => overview.countries.flatMap((country) => country.point
      ? [{ code: country.code, point: country.point, count: country.count, staleCount: country.staleCount }]
      : []),
    [overview.countries],
  )

  useEffect(() => {
    if (!world) return
    const option = mapChartOption({
      mapName: WORLD_MAP_NAME,
      regionNameByCode: world.names,
      countries,
      labelFor: countryDisplayName,
      dark,
      reducedMotion: prefersReducedMotion(),
    })
    optionRef.current = option
    chart.current?.setOption(option)
  }, [world, countries, dark])

  const scopedPeerCount = hasProjection && !loading ? overview.availablePeerCount : null
  const unknownCount = overview.unknownCountryCount
  const countsAvailable = overview.knownCountryCount != null && unknownCount != null
  const neverObserved = overview.scope === 'unobserved'
  const primaryNotice = status === 'disabled' ? PEER_COUNTRIES_DISABLED_NOTICE
    : status === 'starting' ? 'Loading data'
    : !hasProjection ? 'Data unavailable'
    : status === 'empty' ? 'No data'
    : neverObserved ? 'No observations yet'
    : failed ? 'Map unavailable'
    : status === 'error' ? 'Data unavailable'
    : status === 'stale' || overview.peerObservation === 'stale' || overview.countries.some((country) => country.staleCount > 0) ? 'Data stale'
    : overview.availablePeerCount === 0 ? 'No data'
    : !countsAvailable ? 'Data unavailable'
    : overview.scope !== 'complete' || overview.networksWithBasis < overview.networksInScope ? 'Partial data'
    : overview.peerObservation === 'unknown' ? 'Observation status unknown'
    : overview.peerObservation === 'mixed' ? 'Observation status varies'
    : null
  const unknownNotice = hasProjection && !loading && unknownCount != null && unknownCount > 0
    ? formatGeoCount(unknownCount) + ' unknown locations' : null
  const notice = [primaryNotice, unknownNotice].filter(Boolean).join(' · ')
  const mapDescription =
    formatGeoCount(overview.countries.length) +
    (overview.countries.length === 1 ? ' country has ' : ' countries have ') +
    'Peer records in scope for ' + overview.scopeLabel +
    '. Each marker is a Server-provided country representative point, not a Peer location or a Node deployment location.'

  return (
    <section aria-label={PEER_COUNTRIES_HEADING} data-state={status} data-scope={overview.scope} className="relative h-full">
      {/* An abnormal state stays announced to assistive technology without
          putting a glyph or a sentence on the map. */}
      {notice && <span className="sr-only" role="status">{notice}</span>}
      {scopedPeerCount !== null && (
        <p
          data-slot="geo-counters"
          className="pointer-events-none absolute top-0 right-0 z-2 flex items-center gap-2 rounded bg-background/60 px-2 py-0.5 text-[10px] text-muted-foreground backdrop-blur-lg"
        >
          <span data-slot="geo-counter" className="flex items-center gap-1">
            <span data-slot="geo-counter-dot" className="inline-block size-1.5 animate-pulse rounded-full bg-emerald-600" aria-hidden="true" />
            <span className="sr-only">Peers: </span>
            {formatGeoCount(scopedPeerCount)}
          </span>
        </p>
      )}
      {failed ? (
        <Empty description="Map unavailable" className="h-full" />
      ) : (
        <div
          ref={container}
          role="img"
          aria-label={PEER_COUNTRIES_HEADING + ' map. ' + mapDescription}
          data-slot="geo-chart"
          className="h-full w-full"
        />
      )}
      {/* The canvas has no per-country element, so the same figures stay
          available as text: screen-reader users get every observed country,
          its count, and whether some of those records are stale. */}
      <ul className="sr-only" data-slot="geo-country-list">
        {overview.countries.map((country) => (
          <li key={country.code}>
            {countryDisplayName(country.code) + ': ' + formatGeoCount(country.count) + ' records' +
              (country.staleCount > 0 ? ', ' + formatGeoCount(country.staleCount) + ' stale' : '') +
              (country.point ? '' : ' (no representative point, not plotted)')}
          </li>
        ))}
      </ul>
    </section>
  )
}
