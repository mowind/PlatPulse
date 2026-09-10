import { useEffect, useId, useMemo, useState } from 'react'
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
  geoStateTone,
  geoUnknownReasons,
  peerObservationLabel,
} from './geoPresentation'
import { StatusBadge } from './StatusBadge'

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

const MAP_RESOURCE_LABEL: Record<MapResourceState, string> = {
  starting: 'Starting',
  current: 'Current',
  error: 'Error',
  unknown: 'Unknown',
}

export default function GeoWorldMap({ networks, networkFilter, loading, hasProjection }: GeoWorldMapProps) {
  const titleId = useId()
  const canvasId = useId()
  const svgTitleId = useId()
  const svgDescriptionId = useId()
  const [expanded, setExpanded] = useState(false)
  const [attempt, setAttempt] = useState(0)
  const [geometry, setGeometry] = useState<GeometryState>({ status: 'idle' })

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
  // reload. The request is aborted on unmount so no state is written after
  // the section is gone.
  const needsBasemap = status !== 'disabled' && status !== 'empty' && status !== 'starting' && status !== 'unknown'
  useEffect(() => {
    if (!needsBasemap) return
    const controller = new AbortController()
    setGeometry({ status: 'loading' })
    loadWorldGeometry(controller.signal)
      .then((loaded) => setGeometry({ status: 'ready', geometry: loaded }))
      .catch(() => {
        if (!controller.signal.aborted) setGeometry({ status: 'failed' })
      })
    return () => controller.abort()
  }, [attempt, needsBasemap])

  if (status === 'disabled') {
    return (
      <section className="home-geo home-geo-disabled" aria-label={PEER_COUNTRIES_HEADING}>
        <p className="geo-disabled-note" role="status">{PEER_COUNTRIES_DISABLED_NOTICE}</p>
      </section>
    )
  }

  const geometryReady = geometry.status === 'ready'
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

  const knownCount = overview.knownCountryCount
  const unknownCount = overview.unknownCountryCount
  const countsAvailable = knownCount != null && unknownCount != null
  const basis = geoPeerRecordBasis(overview.availablePeerCount)
  const reasons = geoUnknownReasons(overview)
  const neverObserved = overview.scope === 'unobserved'
  const scopeNotesApply = hasProjection && status !== 'starting' && status !== 'empty'
  const mapState: MapResourceState = geometryReady
    ? 'current'
    : geometry.status === 'failed'
      ? 'error'
      : geometry.status === 'loading' ? 'starting' : 'unknown'
  // The badge uses the fixed status vocabulary: Starting/Empty for this
  // surface's own loading states, otherwise the Server-owned Geo state.
  const badgeLabel = status === 'starting' ? 'Starting' : status === 'empty' ? 'Empty' : geoStateLabel(status)
  const badgeTone = status === 'starting' || status === 'empty' ? 'neutral' : geoStateTone(status)
  const mapDescription = `${formatGeoCount(overview.countries.length)} ${overview.countries.length === 1 ? 'country has' : 'countries have'} Peer records in scope for ${overview.scopeLabel}. `
    + 'Each marker is a Server-provided country representative point, not a Peer location or a Node deployment location.'

  return (
    <section className="home-geo" aria-labelledby={titleId} data-state={status} data-scope={overview.scope}>
      <header className="home-geo-heading">
        <h2 id={titleId}>{PEER_COUNTRIES_HEADING}</h2>
        <StatusBadge status={badgeLabel} tone={badgeTone} />
      </header>
      <p className="home-geo-meta">
        <span className="home-geo-scope">Scope: {overview.scopeLabel}</span>
        <span className="home-geo-peer">Peer observation: {peerObservationLabel(overview.peerObservation)}</span>
        <span className="home-geo-basemap">Map resource: {MAP_RESOURCE_LABEL[mapState]}</span>
      </p>

      {status === 'starting' && <p className="home-geo-note" role="status">Starting; the Peer country scope is still loading.</p>}
      {status === 'empty' && <p className="home-geo-note" role="status">Empty: the Public Projection has no Network to place Peer countries on.</p>}
      {!loading && !hasProjection && (
        <p className="home-geo-note" role="status">Country insight is Unknown; the Public Projection is currently unavailable.</p>
      )}
      {scopeNotesApply && neverObserved && (
        <p className="home-geo-note" role="status">Country insight is Unknown; no Active Node has reported a successful Peer Snapshot yet.</p>
      )}
      {scopeNotesApply && !neverObserved && !countsAvailable && status !== 'stale' && status !== 'error' && (
        <p className="home-geo-note" role="status">Country insight is Unknown; no usable Geo projection is available.</p>
      )}
      {(status === 'stale' || status === 'error') && (
        <p className="home-geo-note" role="status">
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
        <p className="home-geo-note" role="status">Partial scope: Active Nodes without a successful Peer Snapshot are not included in these counts.</p>
      )}
      {scopeNotesApply && overview.networksInScope > 1 && overview.networksWithBasis < overview.networksInScope && (
        <p className="home-geo-note" role="status">
          {`${formatGeoCount(overview.networksWithBasis)} of ${formatGeoCount(overview.networksInScope)} Networks in scope have a Peer country basis; the others are not included in these counts.`}
        </p>
      )}

      {countsAvailable && (
        <p className="home-geo-counts">
          {'Known ' + formatGeoCount(knownCount) + ' · Unknown ' + formatGeoCount(unknownCount)}
          {basis ? ' · ' + basis : ''}
        </p>
      )}
      {countsAvailable && reasons.length > 0 && <p className="home-geo-unknown-detail">{reasons.join(' · ')}</p>}
      {scopeNotesApply && countsAvailable && overview.countries.length === 0 && (
        <p className="home-geo-note" role="status">No country observations are available yet.</p>
      )}

      <div className="home-geo-canvas" id={canvasId} data-expanded={expanded}>
        {geometryReady ? (
          <svg
            className="home-geo-svg"
            viewBox={'0 0 ' + geometry.geometry.projection.width + ' ' + geometry.geometry.projection.height}
            preserveAspectRatio="xMidYMid meet"
            role="img"
            aria-labelledby={svgTitleId}
            aria-describedby={svgDescriptionId}
          >
            <title id={svgTitleId}>{PEER_COUNTRIES_HEADING} map</title>
            <desc id={svgDescriptionId}>{mapDescription}</desc>
            <g className="home-geo-land" aria-hidden="true">
              {geometry.geometry.countries.map((country) => <path key={country.code} d={country.path} />)}
            </g>
            <g className="home-geo-observed" aria-hidden="true">
              {observed.map((country) => <path key={country.code} d={country.path} />)}
            </g>
            <g className="home-geo-markers" aria-hidden="true">
              {plotted.map(({ country, at }) => (
                <g key={country.code} className="home-geo-marker" transform={'translate(' + at.x.toFixed(1) + ' ' + at.y.toFixed(1) + ')'}>
                  <circle className="home-geo-marker-dot" cx="0" cy="0" />
                  <text className="home-geo-marker-label" x="0" y="0">{formatGeoCount(country.count)}</text>
                </g>
              ))}
            </g>
          </svg>
        ) : geometry.status === 'failed' ? (
          <div className="home-geo-canvas-state" role="status">
            <p>Map geometry is Unavailable; the country counts below are unaffected.</p>
            <button type="button" className="home-geo-retry" onClick={() => setAttempt((current) => current + 1)}>Retry map</button>
          </div>
        ) : (
          <p className="home-geo-canvas-state" role="status">Map geometry is Starting…</p>
        )}
      </div>

      <div className="home-geo-footer">
        <button
          type="button"
          className="home-geo-toggle"
          aria-expanded={expanded}
          aria-controls={canvasId}
          onClick={() => setExpanded((current) => !current)}
        >
          {expanded ? 'Collapse map' : 'Show full map'}
        </button>
        {overview.countries.length > 0 && (
          <ul className="home-geo-countries" aria-label={PEER_COUNTRIES_LIST_LABEL}>
            {overview.countries.map((country) => (
              <li key={country.code}>
                <span className="home-geo-country-code" aria-hidden="true">{country.code}</span>
                <span className="sr-only">{countryDisplayName(country.code)}</span>
                <strong>{formatGeoCount(country.count)}</strong>
                {country.staleCount > 0 && <small>{formatGeoCount(country.staleCount) + ' last-good Stale'}</small>}
                {!country.point && <small>no representative point</small>}
                {geometryReady && !outlineByCode.has(country.code) && <small>no map outline</small>}
              </li>
            ))}
          </ul>
        )}
      </div>
      {(geometryReady || overview.attribution) && (
        <p className="home-geo-credit">
          {[geometryReady ? geometry.geometry.attribution : '', overview.attribution ?? ''].filter(Boolean).join(' · ')}
          {geometryReady && geometry.geometry.sourceLabel && <span className="sr-only"> Basemap source: {geometry.geometry.sourceLabel}.</span>}
        </p>
      )}
    </section>
  )
}
