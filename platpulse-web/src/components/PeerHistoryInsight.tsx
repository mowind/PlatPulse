import type { AdminPeerHistory, PublicPeerHistory } from '../api/generated'
import { StatusBadge, formatObservedAt } from './StatusBadge'
import { CardX } from './ui/card-x'
import { SURFACE_CARD } from '../lib/surface'
import { cn } from '../lib/utils'

export type PeerHistoryCountry = {
  countryCode: string
  count: number
}

export type PeerHistoryAggregate = {
  bucketStart: string
  lastObservedAt: string
  sampleCount: number
  totalPeers: number
  averagePeers: number | null | undefined
  inboundCount: number
  outboundCount: number
  trustedCount: number
  staticCount: number
  consensusCount: number
  knownCountryCount: number
  unknownCountryCount: number
  countries: PeerHistoryCountry[]
  arrivals: number
  departures: number
  cbftLag: {
    sampleCount: number
    minimum: number | null
    average: number | null
    maximum: number | null
  }
}

export type PeerHistoryData = {
  state: string
  freshness: string
  fiveMinute: PeerHistoryAggregate[]
  hourly: PeerHistoryAggregate[]
}

function value(value: number | null | undefined): string {
  return value == null ? 'Unknown' : value.toLocaleString()
}

function tone(status: string): 'ok' | 'warning' | 'error' | 'neutral' {
  if (status === 'Current' || status === 'current' || status === 'ok') return 'ok'
  if (status === 'Stale' || status === 'stale' || status === 'starting') return 'warning'
  if (status === 'Error' || status === 'error') return 'error'
  return 'neutral'
}

function label(status: string): string {
  if (status === 'ok' || status === 'current') return 'Current'
  if (status === 'stale') return 'Stale'
  if (status === 'error') return 'Error'
  if (status === 'starting') return 'Starting'
  if (status === 'unknown') return 'Unknown'
  if (status === 'disabled') return 'Disabled'
  if (status === 'empty') return 'Empty'
  if (status === 'unsupported') return 'Unsupported'
  return status || 'Unknown'
}

function Summary({ aggregate }: { aggregate: PeerHistoryAggregate | undefined }) {
  const fields: Array<[string, number | null | undefined]> = [
    ['Peers', aggregate?.totalPeers],
    ['Inbound', aggregate?.inboundCount],
    ['Outbound', aggregate?.outboundCount],
    ['Arrivals', aggregate?.arrivals],
    ['Departures', aggregate?.departures],
    ['CBFT lag (avg)', aggregate?.cbftLag.average],
  ]
  return (
    <dl className="my-3 grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-6">
      {fields.map(([name, fieldValue]) => (
        <div className="min-w-0" key={name}>
          <dt className="break-words text-xs font-medium tracking-wider text-muted-foreground">{name}</dt>
          <dd className="m-0 mt-1 break-words text-base font-bold tabular-nums">{value(fieldValue)}</dd>
        </div>
      ))}
    </dl>
  )
}

const TH_CLASS = 'border-b px-2 py-2 text-left text-xs font-medium text-muted-foreground'
const TD_CLASS = 'border-b border-border/60 px-2 py-2 align-top'

function AggregateTable({ title, rows }: { title: string; rows: PeerHistoryAggregate[] }) {
  return (
    <div className="mt-3 min-w-0">
      <h3 className="m-0 mb-2 text-sm font-medium">{title}</h3>
      {rows.length === 0 ? (
        <p className="m-0 text-sm text-muted-foreground">No retained aggregate buckets are available.</p>
      ) : (
        <div className="w-full overflow-x-auto">
          <table className="w-full border-collapse text-sm">
            <caption className="sr-only">{title} Peer aggregate history</caption>
            <thead>
              <tr>
                <th scope="col" className={TH_CLASS}>Bucket</th>
                <th scope="col" className={TH_CLASS}>Samples</th>
                <th scope="col" className={TH_CLASS}>Peers</th>
                <th scope="col" className={TH_CLASS}>In / Out</th>
                <th scope="col" className={TH_CLASS}>Arrivals / Departures</th>
                <th scope="col" className={TH_CLASS}>Countries</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((row) => (
                <tr key={row.bucketStart} className="border-b border-border/60">
                  <th scope="row" className={cn(TD_CLASS, 'font-medium')}>{formatObservedAt(row.bucketStart)}</th>
                  <td data-label="Samples" className={TD_CLASS}>{value(row.sampleCount)}</td>
                  <td data-label="Peers" className={TD_CLASS}>{value(row.averagePeers ?? row.totalPeers)}</td>
                  <td data-label="In / Out" className={TD_CLASS}>{value(row.inboundCount)} / {value(row.outboundCount)}</td>
                  <td data-label="Arrivals / Departures" className={TD_CLASS}>{value(row.arrivals)} / {value(row.departures)}</td>
                  <td data-label="Countries" className={TD_CLASS}>{row.countries.length === 0 ? 'No known countries' : row.countries.map((country) => `${country.countryCode} ${value(country.count)}`).join(', ')} · {value(row.knownCountryCount)} known · {value(row.unknownCountryCount)} unknown</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  )
}

export function PeerHistoryInsight({
  history,
  admin = false,
  error = false,
  loading = false,
}: {
  history: PeerHistoryData | undefined
  admin?: boolean
  error?: boolean
  loading?: boolean
}) {
  const state = loading ? 'Starting' : error ? 'Error' : history ? label(history.state) : 'Unknown'
  const freshness = loading ? 'Unknown' : history ? label(history.freshness) : 'Unknown'
  const latest = history?.fiveMinute[0] ?? history?.hourly[0]
  const latestWindow = history?.fiveMinute.length ? 'five-minute' : 'hourly'
  const hasHistory = Boolean(!loading && history && (history.fiveMinute.length > 0 || history.hourly.length > 0))

  return (
    <CardX
      bordered={false}
      data-slot="peer-history-insight"
      className={cn('mt-3 min-w-0 rounded-md border-none', SURFACE_CARD)}
      contentClassName="flex min-w-0 flex-col gap-2"
    >
      <section className="min-w-0" aria-labelledby={admin ? 'admin-peer-history' : 'public-peer-history'}>
        <div className="flex min-w-0 flex-wrap items-start justify-between gap-3">
          <h2 id={admin ? 'admin-peer-history' : 'public-peer-history'} className="m-0 text-sm font-medium">Peer history</h2>
          <div className="flex min-w-0 flex-wrap items-center gap-3" aria-label="Peer history dimensions">
            <div className="flex items-center gap-2">
              <span className="text-xs font-medium tracking-wider text-muted-foreground">Collection</span>
              <StatusBadge status={state} tone={tone(loading ? 'starting' : error ? 'error' : history?.state ?? 'unknown')} />
            </div>
            <div className="flex items-center gap-2">
              <span className="text-xs font-medium tracking-wider text-muted-foreground">Freshness</span>
              <StatusBadge status={freshness} tone={tone(history?.freshness ?? 'unknown')} />
            </div>
          </div>
        </div>
        {!hasHistory ? (
          <p className="m-0 mt-2 text-sm text-muted-foreground">
            {loading
              ? 'History is Starting; loading retained aggregate buckets…'
              : error
                ? 'History is Error; the aggregate history service could not be reached. Last-good data is unavailable.'
                : 'History is Unknown; no aggregate bucket is available yet. The absence is not rendered as zero.'}
          </p>
        ) : (
          <>
            {error ? <p className="m-0 mt-2 text-sm text-destructive" role="alert">Latest history request failed; showing retained last-good aggregates.</p> : null}
            <Summary aggregate={latest} />
            <p className="m-0 text-sm text-muted-foreground">
              {history?.state === 'empty' ? 'No Peers are currently observed. ' : ''}
              Latest {latestWindow} bucket: {latest ? formatObservedAt(latest.bucketStart) : 'Unknown'} · last observed {latest ? formatObservedAt(latest.lastObservedAt) : 'Unknown'}.
            </p>
            <AggregateTable title="Five-minute history" rows={history?.fiveMinute ?? []} />
            <AggregateTable title="Hourly history" rows={history?.hourly ?? []} />
          </>
        )}
      </section>
    </CardX>
  )
}

function convertPublicRows(rows: PublicPeerHistory['fiveMinute']): PeerHistoryAggregate[] {
  return rows.map((row) => ({
    bucketStart: row.bucketStart,
    lastObservedAt: row.lastObservedAt,
    sampleCount: row.sampleCount,
    totalPeers: row.totalPeers,
    averagePeers: row.averagePeers ?? null,
    inboundCount: row.inboundCount,
    outboundCount: row.outboundCount,
    trustedCount: row.trustedCount,
    staticCount: row.staticCount,
    consensusCount: row.consensusCount,
    knownCountryCount: row.knownCountryCount,
    unknownCountryCount: row.unknownCountryCount,
    countries: row.countries.map((country) => ({ countryCode: country.countryCode, count: country.count })),
    arrivals: row.arrivals,
    departures: row.departures,
    cbftLag: {
      sampleCount: row.cbftLag.sampleCount,
      minimum: row.cbftLag.minimum ?? null,
      average: row.cbftLag.average ?? null,
      maximum: row.cbftLag.maximum ?? null,
    },
  }))
}

export function normalizePublicPeerHistory(history: PublicPeerHistory): PeerHistoryData {
  return {
    state: history.state,
    freshness: history.freshness,
    fiveMinute: convertPublicRows(history.fiveMinute),
    hourly: convertPublicRows(history.hourly),
  }
}

function convertAdminRows(rows: AdminPeerHistory['five_minute']): PeerHistoryAggregate[] {
  return rows.map((row) => ({
    bucketStart: row.bucket_start,
    lastObservedAt: row.last_observed_at,
    sampleCount: row.sample_count,
    totalPeers: row.total_peers,
    averagePeers: row.average_peers ?? null,
    inboundCount: row.inbound_count,
    outboundCount: row.outbound_count,
    trustedCount: row.trusted_count,
    staticCount: row.static_count,
    consensusCount: row.consensus_count,
    knownCountryCount: row.known_country_count,
    unknownCountryCount: row.unknown_country_count,
    countries: row.countries.map((country) => ({ countryCode: country.country_code, count: country.count })),
    arrivals: row.arrivals,
    departures: row.departures,
    cbftLag: {
      sampleCount: row.cbft_lag.sample_count,
      minimum: row.cbft_lag.minimum ?? null,
      average: row.cbft_lag.average ?? null,
      maximum: row.cbft_lag.maximum ?? null,
    },
  }))
}

export function normalizeAdminPeerHistory(history: AdminPeerHistory): PeerHistoryData {
  return {
    state: history.state,
    freshness: history.freshness,
    fiveMinute: convertAdminRows(history.five_minute),
    hourly: convertAdminRows(history.hourly),
  }
}
