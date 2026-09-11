import { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react'
import type { PublicNetwork } from '../api/generated'
import { homeGeoOverview } from '../homeGeo'
import { hasDrawableArea, loadWorldGeometry, projectCountryPoint, type WorldGeometry } from '../worldGeometry'
import {
  PEER_COUNTRIES_DISABLED_NOTICE,
  PEER_COUNTRIES_HEADING,
  countryDisplayName,
  formatGeoCount,
} from './geoPresentation'

/**
 * Compact Home Peer country map (issue #133). It renders the Server's Public
 * Geo Insight on a transparent, locally hosted world basemap: observed
 * countries take a restrained Emerald fill, and markers are Server-provided
 * country representative points.
 *
 * The map is deliberately bare. It shows the world, the country fills, the
 * quantity markers, and one expand control; every explanatory sentence,
 * attribution line, and status glyph that used to sit here was removed by
 * product decision so the map blends into the page wash. The Server-owned
 * dimension (Geo Insight state) and this basemap resource's own load state are
 * still announced to assistive technology through a screen-reader-only status,
 * so an abnormal state is never silently swallowed.
 *
 * The map never claims a Node deployment location, a unique Peer count, or
 * live Peer presence, and it never fabricates a marker, a representative
 * point, or an unknown country.
 */

type GeoWorldMapProps = {
  networks: PublicNetwork[]
  /** Home Network filter; the map always covers the same scope as the list. */
  networkFilter: string
  loading: boolean
  /** False when the Public Projection itself is unavailable. */
  hasProjection: boolean
}

type GeometryState =
  | { status: 'idle' }
  | { status: 'loading' }
  | { status: 'ready'; geometry: WorldGeometry }
  | { status: 'failed' }

type MapStatus = 'starting' | 'empty' | 'unknown' | 'disabled' | 'current' | 'stale' | 'error'

export default function GeoWorldMap({ networks, networkFilter, loading, hasProjection }: GeoWorldMapProps) {
  const tooltipId = useId()
  const [activeCountryCode, setActiveCountryCode] = useState<string | null>(null)
  // A hover preview may vanish with the pointer, but an explicit activation
  // (tap, click, or keyboard) is pinned: a touch tap is followed by a
  // synthesized mouse-leave burst that must not erase what the user opened.
  const [pinned, setPinned] = useState(false)
  const lastPointerType = useRef<string | null>(null)
  const canvasId = useId()
  const svgTitleId = useId()
  const svgDescriptionId = useId()
  const [expanded, setExpanded] = useState(false)
  const canvas = useRef<HTMLDivElement>(null)
  const [canvasWidth, setCanvasWidth] = useState(600)
  const [tooltipAt, setTooltipAt] = useState({ x: 0, y: 0 })
  /** Close the tooltip and drop any pin. Every dismissal path goes through it. */
  const clearActive = useCallback(() => {
    setActiveCountryCode(null)
    setPinned(false)
  }, [])
  useEffect(() => {
    const element = canvas.current
    if (!element || typeof ResizeObserver === 'undefined') return
    const observer = new ResizeObserver(([entry]) => {
      if (entry.contentRect.width > 0) setCanvasWidth(entry.contentRect.width)
      clearActive()
    })
    observer.observe(element)
    return () => observer.disconnect()
  }, [clearActive])
  useEffect(() => {
    function dismiss(event: Event) {
      if (event.type === 'keydown' && (event as KeyboardEvent).key !== 'Escape') return
      if (event.type === 'pointerdown' && event.target instanceof Node && canvas.current?.contains(event.target)) return
      clearActive()
    }
    document.addEventListener('pointerdown', dismiss)
    document.addEventListener('keydown', dismiss)
    return () => {
      document.removeEventListener('pointerdown', dismiss)
      document.removeEventListener('keydown', dismiss)
    }
  }, [clearActive])
  function showCountry(code: string, target: Element, pin: boolean) {
    const box = target.getBoundingClientRect()
    const parent = canvas.current?.getBoundingClientRect()
    setTooltipAt({ x: box.x + box.width / 2 - (parent?.x ?? 0), y: box.y - (parent?.y ?? 0) - 6 })
    setActiveCountryCode(code)
    setPinned(pin)
  }
  const [geometry, setGeometry] = useState<GeometryState>({ status: 'idle' })

  const overview = useMemo(() => homeGeoOverview(networks, networkFilter), [networks, networkFilter])
  // A Network filter change, a projection change, or any refetch that changes
  // the country list invalidates the current tooltip and releases its pin, so a
  // pinned country can never suppress later hover previews after it is gone.
  useEffect(() => { clearActive() }, [clearActive, networkFilter, loading, hasProjection, overview.countries])
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
  // reload. The request is aborted on unmount so no state is written after
  // the section is gone.
  const needsBasemap = status !== 'disabled' && status !== 'empty' && status !== 'starting' && status !== 'unknown'
  useEffect(() => {
    if (!needsBasemap) return
    const controller = new AbortController()
    setGeometry({ status: 'loading' })
    loadWorldGeometry(controller.signal)
      .then((loaded) => { if (!controller.signal.aborted) setGeometry({ status: 'ready', geometry: loaded }) })
      .catch(() => {
        if (!controller.signal.aborted) setGeometry({ status: 'failed' })
      })
    return () => controller.abort()
  }, [needsBasemap])

  const geometryReady = geometry.status === 'ready' && status !== 'disabled'
  // Only outlines that enclose area are painted; a clipped-away ring would draw
  // a bare line across the map. Counts and markers are unaffected, so nothing
  // is dropped from the data.
  const outlines = geometryReady ? geometry.geometry.countries.filter((country) => hasDrawableArea(country.path)) : []
  const outlineByCode = new Map(outlines.map((country) => [country.code, country.path]))
  const observed = geometryReady
    ? overview.countries.flatMap((country) => {
        const path = outlineByCode.get(country.code)
        return path ? [{ code: country.code, path }] : []
      })
    : []
  const plotted = geometryReady
    ? overview.countries.flatMap((country) => {
        if (!country.point) return []
        return [{ country, at: projectCountryPoint(geometry.geometry.projection, country.point) }]
      })
    : []

  const activeCountry = overview.countries.find((country) => country.code === activeCountryCode)
  const countryLabel = (code: string) => {
    const country = overview.countries.find((item) => item.code === code)
    return country ? countryDisplayName(code) + ' · ' + formatGeoCount(country.count) + ' records' : countryDisplayName(code)
  }
  // Fit the full world and the actual marker extents, rather than reserving
  // a wide chart margin or cropping labels near the projection boundaries.
  // Marker sizes are CSS pixels, independent of the responsive SVG scale.
  // A numbered marker stays inside the bounded 14-22px range, and long
  // quantities are abbreviated so four glyphs is the maximum; the exact count
  // stays in the marker's accessible name and in its tooltip.
  const markerRadius = (count: number) => count === 1 ? 3.5 : Math.min(11, Math.max(7, 2.5 * markerText(count).length + 2))
  const bounds = geometryReady ? plotted.reduce((box, { at }) => {
    const padding = 24 * geometry.geometry.projection.width / Math.max(240, canvasWidth - 48)
    return { left: Math.min(box.left, at.x - padding), top: Math.min(box.top, at.y - padding), right: Math.max(box.right, at.x + padding), bottom: Math.max(box.bottom, at.y + padding) }
  }, { left: -1, top: -1, right: geometry.geometry.projection.width + 1, bottom: geometry.geometry.projection.height + 1 }) : null
  const mapScale = bounds ? canvasWidth / (bounds.right - bounds.left) : 1
  const numbered = new Set(plotted.filter(({ country, at }) => country.count > 1 && !plotted.some((other) =>
    other.country.code !== country.code && Math.hypot(other.at.x - at.x, other.at.y - at.y) * mapScale < markerRadius(country.count) + markerRadius(other.country.count) + 3,
  )).map(({ country }) => country.code))

  const unknownCount = overview.unknownCountryCount
  const countsAvailable = overview.knownCountryCount != null && unknownCount != null
  const neverObserved = overview.scope === 'unobserved'
  // A known country without a quantity marker is different from an unknown
  // location. A missing outline alone does not hide an existing marker.
  const locationsNotShown = geometryReady && overview.countries.some((country) => !country.point)
  const primaryNotice = status === 'disabled' ? PEER_COUNTRIES_DISABLED_NOTICE
    : status === 'starting' ? 'Loading data'
    : !hasProjection ? 'Data unavailable'
    : status === 'empty' ? 'No data'
    : neverObserved ? 'No observations yet'
    : geometry.status === 'failed' ? 'Map unavailable'
    : !geometryReady && needsBasemap ? 'Loading map'
    : status === 'error' ? 'Data unavailable'
    : status === 'stale' || overview.peerObservation === 'stale' || overview.countries.some((country) => country.staleCount > 0) ? 'Data stale'
    : overview.availablePeerCount === 0 ? 'No data'
    : !countsAvailable ? 'Data unavailable'
    : locationsNotShown ? 'Some locations not shown'
    : overview.scope !== 'complete' || overview.networksWithBasis < overview.networksInScope ? 'Partial data'
    : overview.peerObservation === 'unknown' ? 'Observation status unknown'
    : overview.peerObservation === 'mixed' ? 'Observation status varies'
    : null
  const unknownNotice = hasProjection && !loading && unknownCount != null && unknownCount > 0
    ? formatGeoCount(unknownCount) + ' unknown locations' : null
  const notice = [primaryNotice, unknownNotice].filter(Boolean).join(' · ')
  const mapDescription = `${formatGeoCount(overview.countries.length)} ${overview.countries.length === 1 ? 'country has' : 'countries have'} Peer records in scope for ${overview.scopeLabel}. `
    + 'Each marker is a Server-provided country representative point, not a Peer location or a Node deployment location.'

  return (
    <section className="home-geo" aria-label={PEER_COUNTRIES_HEADING} data-state={status} data-scope={overview.scope}>
      <header className="home-geo-heading">
        <div className="home-geo-actions">
          <button type="button" className="home-geo-icon" title={expanded ? 'Collapse map' : 'Show full map'} data-tooltip={expanded ? 'Collapse map' : 'Show full map'} aria-label={expanded ? 'Collapse map' : 'Show full map'} aria-expanded={expanded} aria-controls={canvasId} disabled={!geometryReady && !expanded} onClick={() => { clearActive(); setExpanded((current) => !current) }}>
            <svg viewBox="0 0 24 24" aria-hidden="true"><path d={expanded ? 'M4 9h5V4M20 9h-5V4M4 15h5v5M20 15h-5v5' : 'M9 4H4v5M15 4h5v5M4 15v5h5M20 15v5h-5'} /></svg>
          </button>
        </div>
      </header>
      {/* The only status surface left: an abnormal state stays announced to
          assistive technology without putting a glyph or a sentence on the map. */}
      {notice && <span className="sr-only" role="status">{notice}</span>}
      <div ref={canvas} className="home-geo-canvas" id={canvasId} data-expanded={expanded} style={bounds ? { aspectRatio: `${bounds.right - bounds.left} / ${bounds.bottom - bounds.top}` } : undefined}>
        {geometryReady ? (
          <svg
            className="home-geo-svg"
            viewBox={bounds ? [bounds.left, bounds.top, bounds.right - bounds.left, bounds.bottom - bounds.top].join(' ') : undefined}
            preserveAspectRatio="xMidYMid meet"
            role="img"
            aria-labelledby={svgTitleId}
            aria-describedby={svgDescriptionId}
            onPointerDown={(event) => { lastPointerType.current = event.pointerType }}
            onMouseLeave={() => { if (!pinned) clearActive() }}
            onKeyDown={(event) => { if (event.key === 'Escape') clearActive() }}
            onClick={(event) => { if (event.target === event.currentTarget || (event.target instanceof Element && event.target.closest('.home-geo-land'))) clearActive() }}
          >
            <title id={svgTitleId}>{PEER_COUNTRIES_HEADING} map</title>
            <desc id={svgDescriptionId}>{mapDescription}</desc>
            <g className="home-geo-land" aria-hidden="true">
              {outlines.map((country) => <path key={country.code} d={country.path} />)}
            </g>
            <g className="home-geo-observed" aria-hidden="true">
              {observed.map((country) => <path key={country.code} d={country.path}
                // The active class is shared with the marker, so pointing at or
                // focusing a quantity also lights the country it belongs to.
                className={activeCountryCode === country.code ? 'home-geo-country-active' : undefined}
                onMouseEnter={(event) => { if (!pinned) showCountry(country.code, event.currentTarget, false) }}
                onMouseLeave={() => { if (!pinned) clearActive() }}
                onClick={(event) => showCountry(country.code, event.currentTarget, lastPointerType.current !== 'mouse')}
              />)}
            </g>
            <g className="home-geo-markers">
              {plotted.map(({ country, at }) => (
                <g key={country.code} className="home-geo-marker" transform={'translate(' + at.x + ' ' + at.y + ') scale(' + 1 / mapScale + ')'}
                  role="button" tabIndex={0} aria-label={countryLabel(country.code)} aria-describedby={activeCountryCode === country.code ? tooltipId : undefined}
                  onMouseEnter={(event) => { if (!pinned) showCountry(country.code, event.currentTarget, false) }}
                  onMouseLeave={() => { if (!pinned) clearActive() }}
                  onClick={(event) => {
                    // Re-activating the same marker closes it: a touch tap has no
                    // pointer-leave, so the tap itself must be able to dismiss.
                    if (activeCountryCode === country.code && pinned) clearActive()
                    else showCountry(country.code, event.currentTarget, lastPointerType.current !== 'mouse')
                  }}
                  onBlur={() => clearActive()}
                  onKeyDown={(event) => {
                    if (event.key === 'Enter' || event.key === ' ') {
                      event.preventDefault()
                      showCountry(country.code, event.currentTarget, true)
                    }
                  }}
                >
                  <circle className="home-geo-marker-hit" cx="0" cy="0" r="12" />
                  {/* Wrapped so the hover emphasis can scale the marker about its
                      own centre without touching the placement transform. */}
                  <g className="home-geo-marker-body">
                    <circle className="home-geo-marker-dot" cx="0" cy="0" r={numbered.has(country.code) ? markerRadius(country.count) : 3.5} />
                    {numbered.has(country.code) && <text className="home-geo-marker-label" x="0" y="0">{markerText(country.count)}</text>}
                  </g>
                </g>
              ))}
            </g>
          </svg>
        ) : null}
        {geometryReady && activeCountry && <div id={tooltipId} className="home-geo-tooltip" role="tooltip" style={{ left: Math.max(90, Math.min(canvasWidth - 90, tooltipAt.x)), top: Math.max(36, tooltipAt.y) }}>{countryLabel(activeCountry.code)}</div>}
      </div>
    </section>
  )
}

/** Marker numerals stay at four glyphs or fewer so they always fit the bounded
 *  marker size. Abbreviation is monotonic and never promotes past its own unit:
 *  999,500 would round to "1000k", so it is shown as "1M" instead. The exact
 *  count is never lost: the marker's accessible name and its tooltip carry it. */
function markerText(count: number): string {
  if (count < 10_000) return String(count)
  if (count < 1_000_000) {
    const thousands = Math.round(count / 1_000)
    return thousands < 1_000 ? thousands + 'k' : '1M'
  }
  const millions = Math.round(count / 1_000_000)
  return millions < 1_000 ? millions + 'M' : '999M'
}
