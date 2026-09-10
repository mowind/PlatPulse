import { useId } from 'react'
import type { PublicGeoInsight } from '../api/generated'
import { StatusBadge } from './StatusBadge'

function label(state: string): string {
  switch (state) {
    case 'current':
      return 'Current'
    case 'stale':
      return 'Stale'
    case 'error':
      return 'Error'
    default:
      return 'Unknown'
  }
}

function tone(state: string): 'ok' | 'warning' | 'error' | 'neutral' {
  switch (state) {
    case 'current':
      return 'ok'
    case 'stale':
      return 'warning'
    case 'error':
      return 'error'
    default:
      return 'neutral'
  }
}

function formatAge(seconds: number): string {
  if (seconds < 60) return `${seconds} second${seconds === 1 ? '' : 's'}`
  const minutes = Math.floor(seconds / 60)
  if (minutes < 60) return `${minutes} minute${minutes === 1 ? '' : 's'}`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours} hour${hours === 1 ? '' : 's'}`
  const days = Math.floor(hours / 24)
  return `${days} day${days === 1 ? '' : 's'}`
}

function formatCount(value: number): string {
  return value.toLocaleString()
}

/** The Server's own Known/Unknown buckets. The browser never derives Unknown
 * by subtracting an independent Peer total. */
function CountryBuckets({ insight }: { insight: PublicGeoInsight | undefined }) {
  const known = insight?.knownCountryCount
  const unknown = insight?.unknownCountryCount
  if (insight == null || known == null || unknown == null) return null
  // Both Unknown reasons are Server-computed; the browser never derives one
  // by subtracting from the other.
  const reasons = [
    [insight.unknownWithoutRemoteIpCount, 'without a usable public remote IP'],
    [insight.unknownWithPublicIpCount, 'without a retained country result'],
  ].flatMap(([value, reason]) => (typeof value === 'number' && value > 0 ? [formatCount(value) + ' ' + reason] : []))
  return (
    <div className="geo-country-buckets">
      <p className="geo-country-totals">{'Known ' + formatCount(known) + ' · Unknown ' + formatCount(unknown)}</p>
      {insight.availablePeerCount != null && insight.availablePeerCount > 0 && (
        <p className="geo-country-basis">
          {formatCount(insight.availablePeerCount) + ' Peer records in scope; counted per Node, not deduplicated by IP.'}
        </p>
      )}
      {reasons.length > 0 && <p className="geo-country-unknown-detail">{reasons.join(' · ')}</p>}
    </div>
  )
}

/** Country-only Home insight. It intentionally has no address, endpoint, or
 * raw MMDB detail and remains honest when Geo is disabled or unavailable.
 * Country counts are Peer records per Node, never IP-deduplicated Peers, and
 * never a claim about where the monitored Nodes are deployed. */
export function GeoInsight({ insight, peerState }: { insight: PublicGeoInsight | undefined; peerState?: string }) {
  const headingId = useId()
  const state = insight?.state ?? 'unknown'
  const scope = insight?.scope ?? 'unavailable'
  const countries = insight?.countries ?? null

  if (state === 'disabled') {
    return <p className="geo-disabled-note" data-state={state} role="status">Peer countries · Disabled by server</p>
  }

  // No Active Node has ever reported a successful Peer Snapshot: there is no
  // reliable denominator, so no count may be presented as a real zero.
  const neverObserved = scope === 'unobserved'
  const countriesUnavailable = countries === null && state !== 'stale' && state !== 'error'

  return (
    <section className="geo-insight" data-state={state} aria-labelledby={headingId}>
      <div className="geo-insight-heading">
        <h3 id={headingId}>Peer countries</h3>
        <StatusBadge status={label(state)} tone={tone(state)} />
      </div>
      {(state === 'stale' || state === 'error') && (
        <p className="panel-state" role="status">
          {state === 'stale'
            ? 'Geo database is Stale.'
            : 'Geo lookup is Error.'}{' '}
          {countries !== null && countries.length > 0
            ? 'Showing the last-good country projection.'
            : 'No usable country projection is currently available.'}
          {insight?.errorReason && <><br />Reason: {insight.errorReason}</>}
          {insight?.lastGoodAt && <><br />Last good database load: {insight.lastGoodAt}</>}
          {insight?.databaseAgeSeconds != null && <><br />Database age: {formatAge(insight.databaseAgeSeconds)}.</>}
          {insight?.staleSince && <><br />Stale since: {insight.staleSince}</>}
        </p>
      )}
      {neverObserved && (
        <p className="panel-state" role="status">
          {peerState === 'unsupported'
            ? 'Country insight is Unknown; Peer collection is Unsupported, so no country basis exists.'
            : 'Country insight is Unknown; no Active Node has reported a successful Peer Snapshot yet.'}
        </p>
      )}
      {!neverObserved && countriesUnavailable && (
        <p className="panel-state">Country insight is Unknown; no usable Geo projection is available.</p>
      )}
      {!neverObserved && <CountryBuckets insight={insight} />}
      {scope === 'partial' && (
        <p className="geo-scope-note" role="status">
          Partial scope: Active Nodes without a successful Peer Snapshot are not included in these counts.
        </p>
      )}
      {!neverObserved && countries?.length === 0 && !countriesUnavailable && (
        <p className="panel-state">No country observations are available yet.</p>
      )}
      {!neverObserved && countries !== null && countries.length > 0 && (
        <ul className="geo-country-list" aria-label="Peer countries by count">
          {countries.map((country) => (
            <li key={country.countryCode}>
              <span>{country.countryCode}</span>
              <strong>{formatCount(country.count)}</strong>
              {country.staleCount > 0 && (
                <small>
                  {formatCount(country.staleCount) + ' retained as last-good Stale'}
                </small>
              )}
              {country.centroidLat == null || country.centroidLon == null ? (
                <small>No representative point; count remains available.</small>
              ) : null}
            </li>
          ))}
        </ul>
      )}
      {insight?.attribution && <p className="geo-attribution">{insight.attribution}</p>}
    </section>
  )
}

export default GeoInsight
