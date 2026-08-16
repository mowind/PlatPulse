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
    case 'disabled':
      return 'Disabled'
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

/** Country-only Home insight. It intentionally has no address, endpoint, or
 * raw MMDB detail and remains honest when Geo is disabled or unavailable. */
export function GeoInsight({ insight }: { insight: PublicGeoInsight | undefined }) {
  const headingId = useId()
  const state = insight?.state ?? 'unknown'
  const countries = insight?.countries ?? null
  return (
    <section className="geo-insight" aria-labelledby={headingId}>
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
      {countries === null && state !== 'stale' && state !== 'error' && (
        <p className="panel-state">
          {state === 'disabled'
            ? 'Country insight is Disabled by the Server.'
            : 'Country insight is Unknown; no usable Geo projection is available.'}
        </p>
      )}
      {countries !== null && countries.length === 0 && state !== 'stale' && state !== 'error' && (
        <p className="panel-state">No country observations are available yet.</p>
      )}
      {countries !== null && countries.length > 0 && (
        <ul className="geo-country-list" aria-label="Peer countries by count">
          {countries.map((country) => (
            <li key={country.countryCode}>
              <span>
                {country.countryCode}
                {country.centroidLat != null && country.centroidLon != null && (
                  <small className="geo-centroid" aria-label={`static centroid ${country.centroidLat}, ${country.centroidLon}`}> (static centroid {country.centroidLat}, {country.centroidLon})</small>
                )}
              </span>
              <strong>{country.count.toLocaleString()}</strong>
            </li>
          ))}
        </ul>
      )}
      {insight?.attribution && <p className="geo-attribution">{insight.attribution}</p>}
    </section>
  )
}

export default GeoInsight
