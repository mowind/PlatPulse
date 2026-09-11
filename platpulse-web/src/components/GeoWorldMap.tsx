import { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react'
import type { PublicNetwork } from '../api/generated'
import { homeGeoOverview } from '../homeGeo'
import { loadWorldGeometry, projectCountryPoint, type WorldGeometry } from '../worldGeometry'
import {
  PEER_COUNTRIES_DISABLED_NOTICE,
  PEER_COUNTRIES_HEADING,
  PEER_COUNTRIES_LIST_LABEL,
  countryDisplayName,
  describeAge,
  formatGeoCount,
  geoPeerRecordBasis,
  geoStateLabel,
  geoUnknownReasons,
  peerObservationLabel,
} from './geoPresentation'
import MapInformation from './MapInformation'

/**
 * Compact Home Peer country map (issue #133). It renders the Server's Public
 * Geo Insight on a transparent, locally hosted world basemap: observed
 * countries take a restrained Emerald fill, markers are Server-provided
 * country representative points, and every count stays readable as text. A
 * missing basemap, a Disabled Geo Provider, or an unavailable projection
 * degrades inside this section only.
 *
 * Three dimensions stay separate on purpose: the Server-owned Geo Insight
 * state, Peer observation freshness, and this basemap resource's own load
 * state. The map never claims a Node deployment location, a unique Peer
 * count, or live Peer presence, and it never fabricates a marker, a
 * representative point, or an unknown country.
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
type MapResourceState = 'starting' | 'current' | 'error' | 'unknown'

/** Which low-noise icon a standing warning uses. The kind is carried next to
 *  the display text so copy edits can never change the icon. */
type NoticeKind = 'loading' | 'empty' | 'notice'

const MAP_RESOURCE_LABEL: Record<MapResourceState, string> = {
  starting: 'Starting',
  current: 'Current',
  error: 'Error',
  unknown: 'Unknown',
}

export default function GeoWorldMap({ networks, networkFilter, loading, hasProjection }: GeoWorldMapProps) {
  const tooltipId = useId()
  const [activeCountryCode, setActiveCountryCode] = useState<string | null>(null)
  // A hover preview may vanish with the pointer, but an explicit activation
  // (tap, click, or keyboard) is pinned: a touch tap is followed by a
  // synthesized mouse-leave burst that must not erase what the user opened.
  const [pinned, setPinned] = useState(false)
  const lastPointerType = useRef<string | null>(null)
  const informationId = useId()
  const informationTrigger = useRef<HTMLButtonElement>(null)
  const statusTrigger = useRef<HTMLButtonElement>(null)
  const [informationSource, setInformationSource] = useState<'information' | 'status'>('information')
  const [informationOpen, setInformationOpen] = useState(false)
  const closeInformation = useCallback(() => setInformationOpen(false), [])
  const canvasId = useId()
  const svgTitleId = useId()
  const svgDescriptionId = useId()
  const [expanded, setExpanded] = useState(false)
  const [attempt, setAttempt] = useState(0)
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
  }, [attempt, needsBasemap])

  const geometryReady = geometry.status === 'ready' && status !== 'disabled'
  const outlineByCode = new Map(geometryReady ? geometry.geometry.countries.map((country) => [country.code, country.path]) : [])
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
  // Only quantities that fit and have room get a numeral; every other marker
  // stays at its true coordinate with its full count in the tooltip/details.
  // A numbered marker stays inside the bounded 14-22px range: one glyph adds
  // one radius step, and long quantities are abbreviated so four glyphs is the
  // maximum. The exact count stays in the tooltip, the accessible name, and the
  // listed country statistics.
  const markerRadius = (count: number) => count === 1 ? 3.5 : Math.min(11, Math.max(7, 2.5 * markerText(count).length + 2))
  const bounds = geometryReady ? plotted.reduce((box, { at }) => {
    const padding = 24 * geometry.geometry.projection.width / Math.max(240, canvasWidth - 48)
    return { left: Math.min(box.left, at.x - padding), top: Math.min(box.top, at.y - padding), right: Math.max(box.right, at.x + padding), bottom: Math.max(box.bottom, at.y + padding) }
  }, { left: -1, top: -1, right: geometry.geometry.projection.width + 1, bottom: geometry.geometry.projection.height + 1 }) : null
  const mapScale = bounds ? canvasWidth / (bounds.right - bounds.left) : 1
  const numbered = new Set(plotted.filter(({ country, at }) => country.count > 1 && !plotted.some((other) =>
    other.country.code !== country.code && Math.hypot(other.at.x - at.x, other.at.y - at.y) * mapScale < markerRadius(country.count) + markerRadius(other.country.count) + 3,
  )).map(({ country }) => country.code))
  const knownCount = overview.knownCountryCount
  const unknownCount = overview.unknownCountryCount
  const countsAvailable = knownCount != null && unknownCount != null
  const basis = geoPeerRecordBasis(overview.availablePeerCount)
  const reasons = geoUnknownReasons(overview)
  const neverObserved = overview.scope === 'unobserved'
  const scopeNotesApply = hasProjection && status !== 'starting' && status !== 'empty' && status !== 'disabled'
  const mapState: MapResourceState = geometryReady
    ? 'current'
    : geometry.status === 'failed'
      ? 'error'
      : geometry.status === 'loading' ? 'starting' : 'unknown'
  // A known country without a quantity marker is different from an unknown
  // location. A missing outline alone does not hide an existing marker.
  const locationsNotShown = geometryReady && overview.countries.some((country) => !country.point)
  const primary: { kind: NoticeKind; text: string } | null = status === 'disabled' ? { kind: 'notice', text: PEER_COUNTRIES_DISABLED_NOTICE }
    : status === 'starting' ? { kind: 'loading', text: 'Loading data' }
    : !hasProjection ? { kind: 'notice', text: 'Data unavailable' }
    : status === 'empty' ? { kind: 'empty', text: 'No data' }
    : neverObserved ? { kind: 'notice', text: 'No observations yet' }
    : geometry.status === 'failed' ? { kind: 'notice', text: 'Map unavailable' }
    : !geometryReady && needsBasemap ? { kind: 'loading', text: 'Loading map' }
    : status === 'error' ? { kind: 'notice', text: 'Data unavailable' }
    : status === 'stale' || overview.peerObservation === 'stale' || overview.countries.some((country) => country.staleCount > 0) ? { kind: 'notice', text: 'Data stale' }
    : overview.availablePeerCount === 0 ? { kind: 'empty', text: 'No data' }
    : !countsAvailable ? { kind: 'notice', text: 'Data unavailable' }
    : locationsNotShown ? { kind: 'notice', text: 'Some locations not shown' }
    : overview.scope !== 'complete' || overview.networksWithBasis < overview.networksInScope ? { kind: 'notice', text: 'Partial data' }
    : overview.peerObservation === 'unknown' ? { kind: 'notice', text: 'Observation status unknown' }
    : overview.peerObservation === 'mixed' ? { kind: 'notice', text: 'Observation status varies' }
    : null
  const unknownNotice = hasProjection && !loading && unknownCount != null && unknownCount > 0
    ? formatGeoCount(unknownCount) + ' unknown locations' : null
  const noticeKind = primary?.kind ?? (unknownNotice ? 'notice' : null)
  const notice = [primary?.text, unknownNotice].filter(Boolean).join(' · ')
  const mapDescription = `${formatGeoCount(overview.countries.length)} ${overview.countries.length === 1 ? 'country has' : 'countries have'} Peer records in scope for ${overview.scopeLabel}. `
    + 'Each marker is a Server-provided country representative point, not a Peer location or a Node deployment location.'

  return (
    <section className="home-geo" aria-label={PEER_COUNTRIES_HEADING} data-state={status} data-scope={overview.scope}>
      <header className="home-geo-heading">
        <div className="home-geo-actions">
          <button ref={informationTrigger} type="button" className="home-geo-icon" title="Map information" aria-label="Map information" aria-haspopup="dialog" aria-expanded={informationOpen} aria-controls={informationId} onClick={() => { setInformationSource('information'); clearActive(); setInformationOpen((current) => !current) }}>
            <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="9" /><circle cx="12" cy="8.1" r="1.2" fill="currentColor" stroke="none" /><path d="M12 11.6v5.2" /></svg>
          </button>
          <button type="button" className="home-geo-icon" title={expanded ? 'Collapse map' : 'Show full map'} data-tooltip={expanded ? 'Collapse map' : 'Show full map'} aria-label={expanded ? 'Collapse map' : 'Show full map'} aria-expanded={expanded} aria-controls={canvasId} disabled={!geometryReady && !expanded} onClick={() => { clearActive(); setExpanded((current) => !current) }}>
            <svg viewBox="0 0 24 24" aria-hidden="true"><path d={expanded ? 'M4 9h5V4M20 9h-5V4M4 15h5v5M20 15h-5v5' : 'M9 4H4v5M15 4h5v5M4 15v5h5M20 15v5h-5'} /></svg>
          </button>
        </div>
      </header>
      {notice && <div className="home-geo-notice">
        <span className="sr-only" role="status">{notice}</span>
        <button ref={statusTrigger} type="button" className="home-geo-icon home-geo-status" aria-label={'Map status: ' + notice} title={notice} aria-haspopup="dialog" aria-expanded={informationOpen && informationSource === 'status'} aria-controls={informationId} onClick={() => { setInformationSource('status'); clearActive(); setInformationOpen(true) }}>
          {/* One quiet status dot, never a second warning glyph: the reason and
              the next step live in the disclosure this control opens. */}
          <svg viewBox="0 0 24 24" aria-hidden="true" data-kind={noticeKind ?? 'notice'}><circle className="home-geo-status-ring" cx="12" cy="12" r="7.5" /><circle className="home-geo-status-core" cx="12" cy="12" r="3" fill="currentColor" stroke="none" /></svg>
        </button>
      </div>}
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
              {geometry.geometry.countries.map((country) => <path key={country.code} d={country.path} />)}
            </g>
            <g className="home-geo-observed" aria-hidden="true">
              {observed.map((country) => <path key={country.code} d={country.path}
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
                  <circle className="home-geo-marker-dot" cx="0" cy="0" r={numbered.has(country.code) ? markerRadius(country.count) : 3.5} />
                  {numbered.has(country.code) && <text className="home-geo-marker-label" x="0" y="0">{markerText(country.count)}</text>}
                </g>
              ))}
            </g>
          </svg>
        ) : null}
        {geometryReady && activeCountry && <div id={tooltipId} className="home-geo-tooltip" role="tooltip" style={{ left: Math.max(90, Math.min(canvasWidth - 90, tooltipAt.x)), top: Math.max(36, tooltipAt.y) }}>{countryLabel(activeCountry.code)}</div>}
      </div>

      {informationOpen && (
        <MapInformation id={informationId} trigger={informationSource === 'status' ? statusTrigger : informationTrigger} fallback={informationTrigger} onClose={closeInformation}>
          {notice && <p>{notice}</p>}
          {status === 'disabled' && <p>An Owner can enable a Geo provider in Admin Settings.</p>}
          {!loading && !hasProjection && <button type="button" onClick={() => window.location.reload()}>Refresh page</button>}
          <p>Data state: {status === 'starting' ? 'Starting' : status === 'empty' ? 'Empty' : geoStateLabel(status)}</p>
          <p className="home-geo-meta">
            <span className="home-geo-scope">Scope: {overview.scopeLabel}</span>
            <span className="home-geo-peer">Peer observation: {peerObservationLabel(overview.peerObservation)}</span>
            <span className="home-geo-basemap">Map resource: {MAP_RESOURCE_LABEL[mapState]}</span>
          </p>

          {status === 'starting' && <p className="home-geo-note">Starting; the Peer country scope is still loading.</p>}
          {status === 'empty' && <p className="home-geo-note">Empty: the Public Projection has no Network to place Peer countries on.</p>}
          {!loading && !hasProjection && (
            <p className="home-geo-note">Country insight is Unknown; the Public Projection is currently unavailable.</p>
          )}
          {scopeNotesApply && neverObserved && (
            <p className="home-geo-note">Country insight is Unknown; no Active Node has reported a successful Peer Snapshot yet.</p>
          )}
          {scopeNotesApply && !neverObserved && !countsAvailable && status !== 'stale' && status !== 'error' && (
            <p className="home-geo-note">Country insight is Unknown; no usable Geo projection is available.</p>
          )}
          {(status === 'stale' || status === 'error') && (
            <p className="home-geo-note">
              {status === 'stale' ? 'Geo database is Stale.' : 'Geo lookup is Error.'}{' '}
              {overview.countries.length > 0
                ? 'Showing the last-good country projection.'
                : 'No usable country projection is currently available.'}
            </p>
          )}
          {(status === 'stale' || status === 'error') && (overview.errorReason || overview.lastGoodAt || overview.databaseAgeSeconds != null || overview.staleSince) && (
            <p className="home-geo-detail">
              {overview.errorReason && <>Reason: {overview.errorReason}. </>}
              {overview.lastGoodAt && <>Last good database load: {overview.lastGoodAt}. </>}
              {overview.databaseAgeSeconds != null && <>Database age: {describeAge(overview.databaseAgeSeconds)}. </>}
              {overview.staleSince && <>Stale since: {overview.staleSince}.</>}
            </p>
          )}
          {scopeNotesApply && overview.scope === 'partial' && (
            <p className="home-geo-note">Partial scope: Active Nodes without a successful Peer Snapshot are not included in these counts.</p>
          )}
          {scopeNotesApply && (overview.scope === 'partial' || overview.networksWithBasis < overview.networksInScope) && (
            <p className="home-geo-note">Counts grow automatically as the remaining Active Nodes report a Peer Snapshot.</p>
          )}
          {scopeNotesApply && locationsNotShown && (
            <p className="home-geo-note">Countries without a representative point keep their exact count in the list above.</p>
          )}
          {scopeNotesApply && neverObserved && (
            <p className="home-geo-note">Counts appear after the first successful Peer Snapshot; nothing on Home has to be reset.</p>
          )}
          {status === 'stale' && (
            <p className="home-geo-note">Last-good results stay visible; the Server refreshes them automatically, and an Owner can force a re-query in Admin Settings.</p>
          )}
          {status === 'error' && (
            <p className="home-geo-note">The Server keeps retrying without a new Agent report; an Owner can force a re-query in Admin Settings.</p>
          )}
          {scopeNotesApply && overview.networksInScope > 1 && overview.networksWithBasis < overview.networksInScope && (
            <p className="home-geo-note">
              {`${formatGeoCount(overview.networksWithBasis)} of ${formatGeoCount(overview.networksInScope)} Networks in scope have a Peer country basis; the others are not included in these counts.`}
            </p>
          )}

          {countsAvailable && (
            <p className="home-geo-counts">
              {'Known ' + formatGeoCount(knownCount) + (unknownCount > 0 ? ' · Unknown ' + formatGeoCount(unknownCount) : '')}
              {basis ? ' · ' + basis : ''}
            </p>
          )}
          {countsAvailable && reasons.length > 0 && <p className="home-geo-unknown-detail">{reasons.join(' · ')}</p>}
          {scopeNotesApply && countsAvailable && overview.countries.length === 0 && (
            <p className="home-geo-note">No country observations are available yet.</p>
          )}

          <p>The four Home statistics cover All Networks. The Network filter changes the Node list and map scope; sorting changes only the list.</p>
          {overview.countries.length > 0 && (
            <ul className="home-geo-countries" aria-label={PEER_COUNTRIES_LIST_LABEL}>
              {overview.countries.map((country) => (
                <li key={country.code}>
                  <span className="home-geo-country-code" aria-hidden="true">{country.code}</span>
                  <span>{countryDisplayName(country.code)}</span>
                  <strong>{formatGeoCount(country.count)}</strong>
                  {country.staleCount > 0 && <small>{formatGeoCount(country.staleCount) + ' last-good Stale'}</small>}
                  {!country.point && <small>no representative point</small>}
                  {geometryReady && !outlineByCode.has(country.code) && <small>no map outline</small>}
                </li>
              ))}
            </ul>
          )}

          {geometry.status === 'failed' && <><p>Map geometry is unavailable; Server country counts are unaffected.</p><button type="button" onClick={() => setAttempt((current) => current + 1)}>Retry map</button></>}
          <p>Crowded markers and abbreviated numerals keep their exact count in the tooltip and in the country list above. Hover, tap, or focus a marker and press Enter to read it.</p>
          <p>Unknown locations have no retained country result. Known countries without a representative point cannot show a quantity marker; a missing outline does not hide a valid marker.</p>
          <h3>Map credits</h3>
          {geometryReady && <p>{geometry.geometry.attribution} <a href="https://www.naturalearthdata.com/">{geometry.geometry.sourceLabel || 'Natural Earth'}</a></p>}
          {overview.attribution && <p>{overview.attribution} <GeoCreditLinks attribution={overview.attribution} /></p>}
        </MapInformation>
      )}
    </section>
  )
}

/** Marker numerals stay at four glyphs or fewer so they always fit the bounded
 *  marker size. Abbreviation is monotonic and never promotes past its own unit:
 *  999,500 would round to "1000k", so it is shown as "1M" instead. The exact
 *  count is never lost: the marker's accessible name, the tooltip, and the
 *  country list all carry it in full. */
function markerText(count: number): string {
  if (count < 10_000) return String(count)
  if (count < 1_000_000) {
    const thousands = Math.round(count / 1_000)
    return thousands < 1_000 ? thousands + 'k' : '1M'
  }
  const millions = Math.round(count / 1_000_000)
  return millions < 1_000 ? millions + 'M' : '999M'
}

/** Keep the Server's complete attribution above; these are only its linked
 * credits, never a substitute attribution for a different Provider. */
function GeoCreditLinks({ attribution }: { attribution: string }) {
  const sources = [
    { name: 'GeoJS', label: 'GeoJS', url: 'https://get.geojs.io' },
    { name: 'MaxMind', label: 'GeoLite by MaxMind', url: 'https://www.maxmind.com' },
    { name: 'IPinfo', label: 'Powered by IPinfo', url: 'https://ipinfo.io' },
  ].filter((source) => attribution.includes(source.name))
  return sources.length > 0 ? sources.map((source, index) => <span key={source.name}>{index > 0 && ' · '}<a href={source.url}>{source.label}</a></span>) : <span>{attribution}</span>
}
