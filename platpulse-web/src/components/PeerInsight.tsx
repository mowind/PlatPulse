import type { PublicPeerInsight } from '../api/generated'
import { componentStateLabel, freshnessLabel } from './StatusBadge'

/**
 * Peer observation state is presented as independent collection, freshness,
 * and value dimensions. Consumers render the redacted counts and add
 * dimension detail only when the Server has something users need to understand.
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
