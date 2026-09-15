import { useId } from 'react'
import type { PublicPeerInsight } from '../api/generated'
import { componentStateLabel, formatObservedAt, freshnessLabel, StatusBadge } from './StatusBadge'
import { CardX } from './ui/card-x'
import { SURFACE_CARD } from '../lib/surface'
import { cn } from '../lib/utils'

/**
 * Peer state is presented as independent collection, freshness, and value
 * dimensions. The rendered component leads with the redacted counts and only
 * adds dimension detail when the Server has something users need to understand.
 */
export function peerInsightCollectionStatus(insight: PublicPeerInsight | undefined): string {
  return componentStateLabel(insight?.state)
}

export function peerInsightFreshnessStatus(insight: PublicPeerInsight | undefined): string {
  return insight ? freshnessLabel(insight.freshness) : 'Unknown'
}

export function peerInsightValueStatus(insight: PublicPeerInsight | undefined): string {
  if (!insight || insight.peerCount == null) return 'Unknown'
  return insight.peerCount === 0 ? 'Empty' : 'Current'
}

export function peerInsightStatus(insight: PublicPeerInsight | undefined): string {
  const collection = peerInsightCollectionStatus(insight)
  if (collection !== 'Current') return collection

  const freshness = peerInsightFreshnessStatus(insight)
  const value = peerInsightValueStatus(insight)
  if (value === 'Unknown') return 'Unknown'
  if (value === 'Empty' && freshness === 'Current') return 'Empty'
  return freshness
}

function statusTone(status: string): 'ok' | 'warning' | 'error' | 'neutral' {
  switch (status) {
    case 'Current':
    case 'Peer data current':
      return 'ok'
    case 'Stale':
    case 'Unsupported':
      return 'warning'
    case 'Error':
      return 'error'
    default:
      return 'neutral'
  }
}

function count(value: number | null | undefined): string {
  return value == null ? 'Unknown' : value.toLocaleString()
}

function hasUsableValue(valueStatus: string): boolean {
  return valueStatus !== 'Unknown'
}

function showLastSuccessfulSnapshot(collectionStatus: string, freshnessStatus: string, valueStatus: string): boolean {
  return hasUsableValue(valueStatus) && (collectionStatus !== 'Current' || freshnessStatus !== 'Current')
}

function collectionExplanation(status: string, hasValue: boolean, valueStatus: string): string {
  const reason = (() => {
    switch (status) {
      case 'Error':
        return 'Collection failed'
      case 'Disabled':
        return 'Collection disabled; Peer observation is not configured'
      case 'Unsupported':
        return 'Collection unsupported; this Node does not expose a supported Peer snapshot'
      case 'Starting':
        return 'Collection starting; Peer collection has not produced a usable snapshot yet'
      default:
        return 'Collection unknown'
    }
  })()
  const value = hasValue
    ? 'Showing last successful snapshot'
    : 'No successful Peer snapshot is available'
  const empty = valueStatus === 'Empty' ? '; the retained zero is authoritative' : ''
  return `${reason}; ${value}${empty}.`
}

function note(
  collectionStatus: string,
  freshnessStatus: string,
  valueStatus: string,
  hasValue: boolean,
): string {
  if (collectionStatus !== 'Current') {
    const collectionNote = collectionExplanation(collectionStatus, hasValue, valueStatus)
    if (freshnessStatus === 'Stale') return collectionNote + ' Freshness stale; the retained snapshot is past the Server stale boundary.'
    if (freshnessStatus === 'Unknown') return collectionNote + ' Freshness unknown; the Server did not establish that the retained snapshot is current.'
    return collectionNote
  }

  if (freshnessStatus === 'Stale') {
    return hasValue
      ? 'Freshness stale; Showing last successful snapshot.'
      : 'Freshness stale; no successful Peer snapshot is available.'
  }
  if (freshnessStatus === 'Unknown') {
    return hasValue
      ? 'Freshness unknown; the Server did not establish that this Peer snapshot is current.'
      : 'Freshness unknown; no successful Peer snapshot is available.'
  }
  if (valueStatus === 'Empty') return 'The latest successful snapshot was an authoritative empty snapshot (0 Peers).'
  if (!hasValue) return 'Value unknown; no successful Peer snapshot is available yet.'
  return 'Peer counts are from the latest successful snapshot.'
}

function observationDetail(insight: PublicPeerInsight | undefined): string {
  if (!insight) return 'Observation time varies by Node.'

  const details: string[] = []
  if (insight.observedAt) details.push(`Last observed ${formatObservedAt(insight.observedAt)}`)
  if (insight.receivedAt) details.push(`Server received ${formatObservedAt(insight.receivedAt)}`)
  if (insight.freshness === 'stale' && insight.staleSince) {
    details.unshift(`Stale since ${formatObservedAt(insight.staleSince)}`)
  }
  return details.length > 0 ? `${details.join('; ')}.` : 'Observation time varies by Node.'
}

type PeerMetric = {
  label: string
  value: number | null | undefined
}

function PeerMetricGroup({
  name,
  metrics,
  qualifyFirstValue,
}: {
  name: 'primary' | 'secondary'
  metrics: PeerMetric[]
  qualifyFirstValue: boolean
}) {
  const title = name === 'primary' ? 'Primary peer counts' : 'Secondary peer counts'
  return (
    <div
      className={cn('min-w-0', name === 'secondary' && 'border-t border-dashed border-border/60 pt-3')}
      role="group"
      aria-label={title}
    >
      <h3 className="sr-only">{title}</h3>
      <dl className="m-0 grid grid-cols-3 gap-3">
        {metrics.map((metric, index) => (
          <div className="min-w-0" key={metric.label}>
            <dt className="break-words text-xs font-medium tracking-wider text-muted-foreground">{metric.label}</dt>
            <dd className="m-0 mt-1 grid min-w-0 gap-1">
              <strong
                className={cn(
                  'min-w-0 break-words font-bold leading-none tracking-tight tabular-nums',
                  name === 'primary' ? 'text-lg md:text-xl' : 'text-sm md:text-base',
                )}
              >
                {count(metric.value)}
              </strong>
              {qualifyFirstValue && index === 0 && (
                <small className="max-w-48 break-words text-[11px] text-muted-foreground">Showing last successful snapshot</small>
              )}
            </dd>
          </div>
        ))}
      </dl>
    </div>
  )
}

function PeerStatusDimensions({
  collectionStatus,
  freshnessStatus,
  valueStatus,
}: {
  collectionStatus: string
  freshnessStatus: string
  valueStatus: string
}) {
  return (
    <div className="flex min-w-0 flex-wrap items-center gap-2" aria-label="Peer collection, freshness, and value status">
      {collectionStatus !== 'Current' && (
        <div className="min-w-0" data-dimension="collection">
          <span className="sr-only">Collection </span>
          <StatusBadge status={collectionStatus} tone={statusTone(collectionStatus)} />
        </div>
      )}
      {freshnessStatus !== 'Current' && (
        <div className="min-w-0" data-dimension="freshness">
          <span className="sr-only">Freshness </span>
          <StatusBadge status={freshnessStatus} tone={statusTone(freshnessStatus)} />
        </div>
      )}
      {valueStatus === 'Unknown' && (
        <div className="min-w-0" data-dimension="value">
          <span className="sr-only">Value </span>
          <StatusBadge status="Unknown" tone="neutral" />
        </div>
      )}
    </div>
  )
}

export function PeerInsight({
  insight,
  compact = false,
}: {
  insight: PublicPeerInsight | undefined
  compact?: boolean
}) {
  const collectionStatus = peerInsightCollectionStatus(insight)
  const freshnessStatus = peerInsightFreshnessStatus(insight)
  const valueStatus = peerInsightValueStatus(insight)
  const hasValue = hasUsableValue(valueStatus)
  const isCurrent = collectionStatus === 'Current' && freshnessStatus === 'Current' && hasValue
  const qualifyFirstValue = showLastSuccessfulSnapshot(collectionStatus, freshnessStatus, valueStatus)
  const Heading = compact ? 'h3' : 'h2'
  const headingId = useId()
  const primaryMetrics: PeerMetric[] = [
    { label: 'Peers', value: insight?.peerCount },
    { label: 'Inbound', value: insight?.inboundCount },
    { label: 'Outbound', value: insight?.outboundCount },
  ]
  const secondaryMetrics: PeerMetric[] = [
    { label: 'Trusted', value: insight?.trustedCount },
    { label: 'Static', value: insight?.staticCount },
    { label: 'Consensus', value: insight?.consensusCount },
  ]

  return (
    <CardX
      bordered={false}
      data-slot="peer-insight"
      className={cn('min-w-0 rounded-md border-none', SURFACE_CARD)}
      contentClassName="!p-0"
    >
      <section className={cn('min-w-0', compact ? 'p-3' : 'p-4')} aria-labelledby={headingId}>
        <div className="flex min-w-0 flex-wrap items-start gap-3">
          <Heading id={headingId} className="m-0 text-sm font-medium">Peer insight</Heading>
        </div>
        <div className="mt-3 grid min-w-0 gap-4">
          <PeerMetricGroup name="primary" metrics={primaryMetrics} qualifyFirstValue={qualifyFirstValue} />
          <PeerMetricGroup name="secondary" metrics={secondaryMetrics} qualifyFirstValue={false} />
        </div>
        <div className="mt-3 flex min-w-0 items-center" aria-label="Peer data status">
          {isCurrent ? (
            <StatusBadge status="Peer data current" tone="ok" />
          ) : (
            <PeerStatusDimensions
              collectionStatus={collectionStatus}
              freshnessStatus={freshnessStatus}
              valueStatus={valueStatus}
            />
          )}
        </div>
        <p className="m-0 mt-3 break-words text-sm text-muted-foreground">{note(collectionStatus, freshnessStatus, valueStatus, hasValue)}</p>
        <p className="m-0 mt-1 break-words text-[11px] text-muted-foreground">{observationDetail(insight)}</p>
      </section>
    </CardX>
  )
}
