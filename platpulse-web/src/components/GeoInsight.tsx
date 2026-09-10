import { useId } from 'react'
import type { PublicGeoInsight } from '../api/generated'
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
} from './geoPresentation'
import { StatusBadge } from './StatusBadge'

/** The Server's own Known/Unknown buckets. The browser never derives Unknown
 * by subtracting an independent Peer total. */
function CountryBuckets({ insight }: { insight: PublicGeoInsight | undefined }) {
  const known = insight?.knownCountryCount
  const unknown = insight?.unknownCountryCount
  if (insight == null || known == null || unknown == null) return null
  const basis = geoPeerRecordBasis(insight.availablePeerCount)
  const reasons = geoUnknownReasons(insight)
  return (
    <div className="geo-country-buckets">
      <p className="geo-country-totals">{'Known ' + formatGeoCount(known) + ' · Unknown ' + formatGeoCount(unknown)}</p>
      {basis && <p className="geo-country-basis">{basis}</p>}
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
    return <p className="geo-disabled-note" data-state={state} role="status">{PEER_COUNTRIES_DISABLED_NOTICE}</p>
  }

  // No Active Node has ever reported a successful Peer Snapshot: there is no
  // reliable denominator, so no count may be presented as a real zero.
  const neverObserved = scope === 'unobserved'
  const countriesUnavailable = countries === null && state !== 'stale' && state !== 'error'

  return (
    <section className="geo-insight" data-state={state} aria-labelledby={headingId}>
      <div className="geo-insight-heading">
        <h3 id={headingId}>{PEER_COUNTRIES_HEADING}</h3>
        <StatusBadge status={geoStateLabel(state)} tone={geoStateTone(state)} />
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
          {insight?.databaseAgeSeconds != null && <><br />Database age: {describeAge(insight.databaseAgeSeconds)}.</>}
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
        <ul className="geo-country-list" aria-label={PEER_COUNTRIES_LIST_LABEL}>
          {countries.map((country) => (
            <li key={country.countryCode}>
              <span aria-hidden="true">{country.countryCode}</span>
              <span className="sr-only">{countryDisplayName(country.countryCode)}</span>
              <strong>{formatGeoCount(country.count)}</strong>
              {country.staleCount > 0 && (
                <small>
                  {formatGeoCount(country.staleCount) + ' retained as last-good Stale'}
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
