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
      {countries === null && (
        <p className="panel-state">
          {state === 'disabled'
            ? 'Country insight is Disabled by the Server.'
            : state === 'stale'
              ? 'Country insight is Stale; no usable country projection is currently available.'
              : state === 'error'
                ? 'Country insight is Error; no usable country projection is currently available.'
                : 'Country insight is Unknown; no usable Geo projection is available.'}
        </p>
      )}
      {countries !== null && countries.length === 0 && (
        <p className="panel-state">
          {state === 'error'
            ? 'Country insight is Error; no usable Geo projection is available.'
            : 'No country observations are available yet.'}
        </p>
      )}
      {countries !== null && countries.length > 0 && (state === 'stale' || state === 'error') && (
        <p className="panel-state" role="status">
          {state === 'stale'
            ? 'Geo database is Stale; showing the last-good country projection.'
            : 'Geo lookup is Error; showing the last-good country projection.'}
          {insight?.lastGoodAt && <><br />Last good database load: {insight.lastGoodAt}</>}
        </p>
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
