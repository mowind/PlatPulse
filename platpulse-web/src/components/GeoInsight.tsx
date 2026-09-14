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
import { CardX } from './ui/card-x'
import { SURFACE_CARD } from '../lib/surface'
import { cn } from '../lib/utils'

/** The Server's own Known/Unknown buckets. The browser never derives Unknown
 * by subtracting an independent Peer total. */
function CountryBuckets({ insight }: { insight: PublicGeoInsight | undefined }) {
  const known = insight?.knownCountryCount
  const unknown = insight?.unknownCountryCount
  if (insight == null || known == null || unknown == null) return null
  const basis = geoPeerRecordBasis(insight.availablePeerCount)
  const reasons = geoUnknownReasons(insight)
  return (
    <div className="mt-2 min-w-0">
      <p className="m-0 text-sm tabular-nums">{'Known ' + formatGeoCount(known) + ' · Unknown ' + formatGeoCount(unknown)}</p>
      {basis && <p className="m-0 mt-1 break-words text-[11px] text-muted-foreground">{basis}</p>}
      {reasons.length > 0 && <p className="m-0 mt-1 break-words text-[11px] text-muted-foreground">{reasons.join(' · ')}</p>}
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

  const cardClass = cn('min-w-0 rounded-md border-none', SURFACE_CARD)

  if (state === 'disabled') {
    return (
      <CardX bordered={false} data-slot="geo-insight" data-state={state} className={cardClass}>
        <p className="m-0 break-words text-sm text-muted-foreground" role="status">{PEER_COUNTRIES_DISABLED_NOTICE}</p>
      </CardX>
    )
  }

  // No Active Node has ever reported a successful Peer Snapshot: there is no
  // reliable denominator, so no count may be presented as a real zero.
  const neverObserved = scope === 'unobserved'
  const countriesUnavailable = countries === null && state !== 'stale' && state !== 'error'

  return (
    <CardX bordered={false} data-slot="geo-insight" data-state={state} className={cardClass} contentClassName="flex min-w-0 flex-col gap-2">
      <section className="min-w-0" aria-labelledby={headingId}>
        <div className="flex min-w-0 items-center justify-between gap-3">
          <h3 id={headingId} className="m-0 text-sm font-medium">{PEER_COUNTRIES_HEADING}</h3>
          <StatusBadge status={geoStateLabel(state)} tone={geoStateTone(state)} />
        </div>
        {(state === 'stale' || state === 'error') && (
          <p className="m-0 mt-2 break-words text-sm text-muted-foreground" role="status">
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
          <p className="m-0 mt-2 break-words text-sm text-muted-foreground" role="status">
            {peerState === 'unsupported'
              ? 'Country insight is Unknown; Peer collection is Unsupported, so no country basis exists.'
              : 'Country insight is Unknown; no Active Node has reported a successful Peer Snapshot yet.'}
          </p>
        )}
        {!neverObserved && countriesUnavailable && (
          <p className="m-0 mt-2 break-words text-sm text-muted-foreground">Country insight is Unknown; no usable Geo projection is available.</p>
        )}
        {!neverObserved && <CountryBuckets insight={insight} />}
        {scope === 'partial' && (
          <p className="m-0 mt-2 break-words text-[11px] text-muted-foreground" role="status">
            Partial scope: Active Nodes without a successful Peer Snapshot are not included in these counts.
          </p>
        )}
        {!neverObserved && countries?.length === 0 && !countriesUnavailable && (
          <p className="m-0 mt-2 break-words text-sm text-muted-foreground">No country observations are available yet.</p>
        )}
        {!neverObserved && countries !== null && countries.length > 0 && (
          <ul className="m-0 mt-2 flex list-none flex-col gap-1 p-0" aria-label={PEER_COUNTRIES_LIST_LABEL}>
            {countries.map((country) => (
              <li
                key={country.countryCode}
                className="grid grid-cols-[minmax(0,1fr)_auto] items-baseline gap-x-4 border-b border-border/60 py-1 last:border-b-0"
              >
                <span aria-hidden="true" className="text-sm font-medium">{country.countryCode}</span>
                <span className="sr-only">{countryDisplayName(country.countryCode)}</span>
                <strong className="text-sm font-bold tabular-nums">{formatGeoCount(country.count)}</strong>
                {country.staleCount > 0 && (
                  <small className="col-span-2 break-words text-[11px] text-muted-foreground">
                    {formatGeoCount(country.staleCount) + ' retained as last-good Stale'}
                  </small>
                )}
                {country.centroidLat == null || country.centroidLon == null ? (
                  <small className="col-span-2 break-words text-[11px] text-muted-foreground">No representative point; count remains available.</small>
                ) : null}
              </li>
            ))}
          </ul>
        )}
        {insight?.attribution && <p className="m-0 mt-2 break-words text-[11px] text-muted-foreground">{insight.attribution}</p>}
      </section>
    </CardX>
  )
}

export default GeoInsight
