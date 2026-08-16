import { useId } from 'react'
import type { PublicPeerInsight } from '../api/generated'
import { componentStateLabel, formatObservedAt, freshnessLabel, StatusBadge } from './StatusBadge'

/**
 * Peer state is presented as independent collection, freshness, and value
 * dimensions. The primary helper remains useful to compact callers, but the
 * rendered component does not collapse those dimensions into one badge.
 */
export function peerInsightCollectionStatus(insight: PublicPeerInsight | undefined): string {
  return componentStateLabel(insight?.state)
}

export function peerInsightFreshnessStatus(insight: PublicPeerInsight | undefined): string {
  if (!insight || insight.peerCount == null) return 'Unknown'
  return freshnessLabel(insight.freshness)
}

export function peerInsightValueStatus(insight: PublicPeerInsight | undefined): string {
  if (!insight || insight.peerCount == null) return 'Unknown'
  // A stale or failed empty snapshot is not an authoritative Empty value and
  // must not make the UI render zero as if it were current.
  if (insight.peerCount === 0 && (insight.state !== 'ok' || insight.freshness !== 'current')) {
    return 'Unknown'
  }
  if (insight.peerCount === 0) return 'Empty'
  return 'Current'
}

export function peerInsightStatus(insight: PublicPeerInsight | undefined): string {
  const collection = peerInsightCollectionStatus(insight)
  if (collection !== 'Current') return collection
  const freshness = peerInsightFreshnessStatus(insight)
  const value = peerInsightValueStatus(insight)
  if (value === 'Empty') return 'Empty'
  if (value === 'Unknown') return 'Unknown'
  return freshness
}

function statusTone(status: string): 'ok' | 'warning' | 'error' | 'neutral' {
  switch (status) {
    case 'Current':
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

function count(value: number | null | undefined, insight: PublicPeerInsight | undefined): string {
  if (value == null) return 'Unknown'
  if (value === 0 && peerInsightValueStatus(insight) === 'Unknown') return 'Unknown'
  return value.toLocaleString()
}

function observationDetail(insight: PublicPeerInsight | undefined): string {
  if (!insight?.receivedAt) return insight?.observedAt ? `Last observed ${formatObservedAt(insight.observedAt)}.` : 'Observation time varies by Node.'
  if (insight.freshness === 'stale' && insight.staleSince) {
    return `Stale since ${formatObservedAt(insight.staleSince)}; last accepted ${formatObservedAt(insight.receivedAt)}.`
  }
  return `Last accepted ${formatObservedAt(insight.receivedAt)}.`
}

function note(insight: PublicPeerInsight | undefined, valueStatus: string): string {
  const collection = peerInsightCollectionStatus(insight)
  const freshness = peerInsightFreshnessStatus(insight)
  if (collection === 'Error') {
    return insight?.peerCount != null && valueStatus === 'Current'
      ? 'Collection Error; the last-good snapshot remains visible.'
      : 'Collection Error; no successful Peer snapshot is available.'
  }
  if (collection === 'Unsupported') return 'Collection Unsupported; this Node does not expose a supported Peer snapshot.'
  if (collection === 'Disabled') return 'Collection Disabled; Peer observation is not configured.'
  if (collection === 'Starting') return 'Collection Starting; Peer collection has not produced a usable snapshot yet.'
  if (freshness === 'Stale') return 'Freshness Stale; the last-good snapshot is shown when its value is usable.'
  if (valueStatus === 'Empty') return 'Value Empty; the latest successful snapshot contained no Peers.'
  if (valueStatus === 'Unknown') return 'Value Unknown; no usable successful Peer snapshot is available yet.'
  return 'Peer counts are from the latest successful snapshot.'
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
  const Heading = compact ? 'h3' : 'h2'
  const headingId = useId()

  return (
    <section className={`peer-insight${compact ? ' peer-insight-compact' : ''}`} aria-labelledby={headingId}>
      <div className="peer-insight-heading">
        <Heading id={headingId}>Peer insight</Heading>
        <div className="peer-dimensions" aria-label="Peer observation dimensions">
          <div>
            <span className="peer-dimension-label">Collection</span>
            <StatusBadge status={collectionStatus} tone={statusTone(collectionStatus)} />
          </div>
          <div>
            <span className="peer-dimension-label">Freshness</span>
            <StatusBadge status={freshnessStatus} tone={statusTone(freshnessStatus)} />
          </div>
          <div>
            <span className="peer-dimension-label">Value</span>
            <StatusBadge status={valueStatus} tone={statusTone(valueStatus)} />
          </div>
        </div>
      </div>
      <dl className="peer-summary-list">
        <div>
          <dt>{collectionStatus === 'Error' || freshnessStatus === 'Stale' ? 'Last-good peers' : 'Peers'}</dt>
          <dd>{count(insight?.peerCount, insight)}</dd>
        </div>
        <div>
          <dt>Inbound</dt>
          <dd>{count(insight?.inboundCount, insight)}</dd>
        </div>
        <div>
          <dt>Outbound</dt>
          <dd>{count(insight?.outboundCount, insight)}</dd>
        </div>
        <div>
          <dt>Trusted</dt>
          <dd>{count(insight?.trustedCount, insight)}</dd>
        </div>
        <div>
          <dt>Static</dt>
          <dd>{count(insight?.staticCount, insight)}</dd>
        </div>
        <div>
          <dt>Consensus</dt>
          <dd>{count(insight?.consensusCount, insight)}</dd>
        </div>
      </dl>
      <p className="peer-insight-note">{note(insight, valueStatus)}</p>
      <p className="peer-observation-time">{observationDetail(insight)}</p>
    </section>
  )
}
