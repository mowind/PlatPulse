import type { HostDiagnostic } from './api/generated'
import { formatObservedAt } from './components/StatusBadge'

/** Durable-spool evidence that must stay prominent wherever Agent summaries
 * are rendered; it never replaces Server liveness or freshness. */
export function hasSpoolRisk(host: HostDiagnostic | null | undefined): boolean {
  return host?.spool_store_fatal === true ||
    host?.spool_dropped_sequence_from != null ||
    host?.spool_dropped_sequence_to != null ||
    host?.spool_dropped_height_from != null ||
    host?.spool_dropped_height_to != null ||
    host?.spool_report_too_large === true
}

/** Server liveness tone (online/offline/unknown), shared by every Agent
 * surface so the three dimensions keep one presentation. */
export function livenessTone(liveness: string): 'ok' | 'error' | 'neutral' {
  return liveness === 'online' ? 'ok' : liveness === 'offline' ? 'error' : 'neutral'
}

/** Server receipt time: an omitted value is Unknown, an explicit null is
 * Never received; the browser never derives liveness from the timestamp. */
export function receiptTimeText(receivedAt: string | null | undefined): string {
  if (receivedAt === undefined) return 'Unknown'
  if (receivedAt === null) return 'Never received'
  return formatObservedAt(receivedAt)
}
