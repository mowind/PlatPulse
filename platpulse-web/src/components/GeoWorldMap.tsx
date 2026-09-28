import { useEffect, useMemo, useRef, useState } from 'react'
import type { ECharts } from 'echarts/core'
import type { PublicNetwork } from '../api/generated'
import { homeGeoOverview } from '../homeGeo'
import { WORLD_MAP_NAME, loadWorldGeoJson, regionNameByCode, type WorldGeoJson } from '../worldGeometry'
import { mapChartOption, type MapCountry } from './mapChartOption'
import {
  PEER_COUNTRIES_DISABLED_NOTICE,
  PEER_COUNTRIES_HEADING,
  countryDisplayName,
  formatGeoCount,
  geoUnknownLocationLabel,
  geoUnknownReasons,
} from './geoPresentation'
import { cn } from '../lib/utils'

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
 * Home Peer country map (issue #133), rendered by ECharts following
 * komari-theme-emerald@c2c5e88 NodeEarthMaps.vue: the same geometry, the same
 * silent transparent geo coordinate system, the same scatter symbol, the same
 * 8px/14px sizes, white 10px aggregate numerals, and the same tooltip box. The
 * option itself lives in mapChartOption.ts so the encoding can be tested
 * without a canvas.
 *
 * The layer sets only left/top/width, exactly as upstream does, so the world
 * fills the track width and derives its own height. The desktop track is
 * upstream's fixed 22rem (352px) band, which is slightly taller than the world
 * at Home's 1280px ceiling: the world therefore fits without cropping instead
 * of overflowing a shorter proportional track. Below xl the track stays
 * proportional (2:1), so phones and tablets keep the compact map the mobile
 * acceptance measured.
 *
 * PlatPulse keeps its own data semantics: the map plots Peer records by
 * country from Server-provided country representative points, never node
 * locations and never a Peer address, and stale or unknown data stays visible
 * as such. The corner chip keeps the Server's own Peer-record denominator and,
 * beside it, the Unknown country bucket the Server reports — a coverage gap,
 * never an offline, health or connection claim. That count left the exceptional
 * notice, which is now kept for the map with nothing to plot. ECharts paints
 * into a canvas, which carries no per-country element, so the same figures are
 * also exposed as a screen-reader list, the container is a labelled image, and
 * the counters stay real text.
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
    const dismissOutside = (event: PointerEvent) => {
      if (event.target instanceof Node && !element.contains(event.target)) {
        instance?.dispatchAction({ type: 'hideTip' })
      }
    }
    document.addEventListener('pointerdown', dismissOutside)
    loadECharts()
      .then((core) => {
        if (disposed || !container.current) return
        instance = core.init(container.current)
        chart.current = instance
        // ZRender receives blank-canvas taps too; ECharts series clicks do not.
        instance.getZr().on('click', (event) => {
          if (!event.target) instance?.dispatchAction({ type: 'hideTip' })
        })
        if (optionRef.current) instance.setOption(optionRef.current)
        observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(() => {
          instance?.dispatchAction({ type: 'hideTip' })
          instance?.resize()
        })
        observer?.observe(container.current)
        if (!observer) instance.resize()
      })
      .catch(() => { if (!disposed) setFailed(true) })
    return () => {
      disposed = true
      document.removeEventListener('pointerdown', dismissOutside)
      observer?.disconnect()
      instance?.dispose()
      chart.current = null
    }
  }, [world])

  const countries = useMemo<MapCountry[]>(
    () => overview.countries.map((country) => ({ code: country.code, point: country.point, count: country.count, staleCount: country.staleCount })),
    [overview.countries],
  )

  // Every Public refetch returns a new Network array, so `countries` changes
  // identity even when the Server's country projection did not. ECharts
  // re-parses the world geometry and rebuilds the map graphic on every
  // setOption, so applying an option whose plotted data is unchanged burns CPU
  // for no visible change. Compare the plotted values and only re-apply when
  // the map actually changes (or the chart instance was rebuilt for a new
  // basemap/theme).
  const appliedSignature = useRef<string | null>(null)

  useEffect(() => {
    if (!world) return
    const signature = JSON.stringify(countries) + '|' + (dark ? 'dark' : 'light')
    if (appliedSignature.current === signature) return
    appliedSignature.current = signature
    const option = mapChartOption({
      mapName: WORLD_MAP_NAME,
      regionNameByCode: world.names,
      countries,
      labelFor: countryDisplayName,
      dark,
      reducedMotion: prefersReducedMotion(),
    })
    optionRef.current = option
    chart.current?.dispatchAction({ type: 'hideTip' })
    chart.current?.setOption(option)
  }, [world, countries, dark])

  const scopedPeerCount = hasProjection && !loading ? overview.availablePeerCount : null
  const unknownCount = overview.unknownCountryCount
  // The second corner figure exists only beside a real denominator. The Server
  // computes available = known + unknown and gates all three on the same basis,
  // so a non-zero Unknown bucket always has a chip to sit in and no fallback
  // figure has to be invented here.
  const shownUnknownCount =
    scopedPeerCount !== null && unknownCount != null && unknownCount > 0 ? unknownCount : null
  const unknownReasons = shownUnknownCount === null ? [] : geoUnknownReasons(overview)
  const countersLabel = scopedPeerCount === null
    ? null
    : formatGeoCount(scopedPeerCount) + ' Peer records in scope for ' + overview.scopeLabel +
      (shownUnknownCount === null
        ? ''
        : ', ' + formatGeoCount(shownUnknownCount) + ' with no retained country attribution')
  const countersTitle = scopedPeerCount === null
    ? null
    : 'Peer records in scope for ' + overview.scopeLabel + '; not unique Peers or Node locations.' +
      (shownUnknownCount === null
        ? ''
        : ' ' + geoUnknownLocationLabel(shownUnknownCount) +
          (unknownReasons.length > 0 ? ': ' + unknownReasons.join('; ') : '') + '.')
  const countsAvailable = overview.knownCountryCount != null && unknownCount != null
  const neverObserved = overview.scope === 'unobserved'
  const primaryNotice = failed && needsBasemap ? 'Map unavailable'
    : status === 'disabled' ? PEER_COUNTRIES_DISABLED_NOTICE
    : status === 'starting' ? 'Loading data'
    : !hasProjection ? 'Data unavailable'
    : status === 'empty' ? 'No data'
    : neverObserved ? 'No observations yet'
    : status === 'error' ? 'Data unavailable'
    : status === 'unknown' ? 'Data unknown'
    : status === 'stale' || overview.peerObservation === 'stale' || overview.countries.some((country) => country.staleCount > 0) ? 'Map data stale'
    : overview.availablePeerCount === 0 ? 'No data'
    : !countsAvailable ? 'Data unavailable'
    : overview.scope !== 'complete' || overview.networksWithBasis < overview.networksInScope ? 'Partial data'
    : overview.peerObservation === 'unknown' ? 'Observation status unknown'
    : overview.peerObservation === 'mixed' ? 'Observation status varies'
    : null
  // The Unknown count is a coverage figure, not an exception, so it lives in
  // the corner chip. The one notice it can still raise is the map with nothing
  // to plot at all: no resolved country, only unlocated records.
  const noLocationsNotice =
    primaryNotice === null && overview.knownCountryCount === 0 && shownUnknownCount !== null
      ? 'No locations to show' : null
  const notice = primaryNotice ?? noLocationsNotice
  const mapDescription =
    formatGeoCount(overview.countries.length) +
    (overview.countries.length === 1 ? ' country has ' : ' countries have ') +
    'Peer records in scope for ' + overview.scopeLabel +
    '. Each marker is a Server-provided country representative point, not a Peer location or a Node deployment location.'

  return (
    <section aria-label={PEER_COUNTRIES_HEADING} data-state={status} data-network-filter={networkFilter} data-scope={overview.scope} className="relative h-full">
      {/* Normal stays quiet; exceptions are visible without moving the canvas. */}
      {notice && (
        <span
          data-slot="map-status"
          role="status"
          className={cn(
            'absolute top-0 left-0 z-10 rounded-md bg-background/90 px-2 py-1 text-xs text-muted-foreground',
            // The corner chip reserves the complementary width, so the two can
            // never overlap however long either text grows.
            scopedPeerCount !== null
              ? 'max-w-[45%] md:max-w-[55%]'
              : 'max-w-[calc(100%-88px)] md:max-w-[80%]',
          )}
        >
          {notice}
        </span>
      )}
      {scopedPeerCount !== null && (
        <p
          data-slot="geo-counters"
          role="note"
          aria-label={countersLabel ?? undefined}
          title={countersTitle ?? undefined}
          className="pointer-events-none md:pointer-events-auto absolute top-0 right-0 z-2 flex max-w-[calc(55%-8px)] flex-wrap items-center justify-end gap-2 rounded bg-background/60 px-2 py-0.5 text-[10px] text-muted-foreground backdrop-blur-lg md:max-w-[calc(45%-8px)]"
        >
          <span data-slot="geo-counter" className="flex items-center gap-1">
            <span data-slot="geo-counter-dot" className="inline-block size-1.5 animate-pulse rounded-full bg-emerald-600" aria-hidden="true" />
            <span>Peers: </span>
            {formatGeoCount(scopedPeerCount)}
          </span>
          {/* Static and neutral: the Unknown bucket is a coverage gap, never the
              amber this map already uses for last-good country results. */}
          {shownUnknownCount !== null && (
            <span data-slot="geo-counter-unknown" className="flex items-center gap-1">
              <span data-slot="geo-counter-unknown-dot" className="inline-block size-1.5 rounded-full bg-muted-foreground" aria-hidden="true" />
              {formatGeoCount(shownUnknownCount)} unknown
            </span>
          )}
        </p>
      )}
      {failed ? (
        <div className="h-full" aria-hidden="true" />
      ) : (
        // The layer fills the track; the desktop band is tall enough for the
        // world's own aspect ratio, so nothing is cropped there.
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
          its count, whether some of those records are stale, and — when the
          Unknown bucket is non-zero — its count and the Server's reasons. */}
      <ul className="sr-only" data-slot="geo-country-list">
        {overview.countries.map((country) => (
          <li key={country.code}>
            {countryDisplayName(country.code) + ': ' + formatGeoCount(country.count) + ' records' +
              (country.staleCount > 0 ? ', ' + formatGeoCount(country.staleCount) + ' stale' : '') +
              (country.point ? '' : ' (no representative point, not plotted)')}
          </li>
        ))}
        {shownUnknownCount !== null && (
          <li data-slot="geo-unknown-list-item">
            {geoUnknownLocationLabel(shownUnknownCount) +
              (unknownReasons.length > 0 ? ': ' + unknownReasons.join('; ') : '')}
          </li>
        )}
      </ul>
    </section>
  )
}
