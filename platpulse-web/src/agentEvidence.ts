import type { AgentDiagnostic, HostDiagnostic } from './api/generated'
import { formatBytesUnknown } from './formatBytes'
import { formatObservedAt } from './components/StatusBadge'
import { hasSpoolRisk } from './agentDiagnostics'

/**
 * Agent evidence, organized for the layered Admin summaries (webui.md §8.7).
 * Three groups never merge into one verdict:
 *
 * - current: what the Server says needs an Owner now — its own Agent
 *   Attention Items — and a credential set with no Server-active credential;
 * - unknown: evidence that was never observed, which is never zero or normal;
 * - recorded: accumulated or retained history (gap intervals, security
 *   events, retained spool evidence). Recorded is not a current fault, and a
 *   known zero is not listed at all.
 *
 * Nothing here reads browser time or infers recovery from liveness.
 */
export type EvidenceTone = 'error' | 'warning' | 'neutral'
/** `text` is the complete sentence; `short` is the wording for a line that
 *  already says "Recorded". */
export type EvidenceItem = { key: string; text: string; short?: string; tone: EvidenceTone }
export type AgentEvidence = {
  current: EvidenceItem[]
  unknown: EvidenceItem[]
  recorded: EvidenceItem[]
}

export function hasSpoolObservation(spool: HostDiagnostic): boolean {
  return [
    spool.spool_capacity_bytes,
    spool.spool_dropped_height_from,
    spool.spool_dropped_height_to,
    spool.spool_dropped_sequence_from,
    spool.spool_dropped_sequence_to,
    spool.spool_dropped_time_from,
    spool.spool_dropped_time_to,
    spool.spool_in_flight,
    spool.spool_last_delivery_at,
    spool.spool_last_delivery_error,
    spool.spool_max_age_seconds,
    spool.spool_oldest_queued_age_ms,
    spool.spool_pending_history_gaps,
    spool.spool_queued_bytes,
    spool.spool_queued_reports,
    spool.spool_report_too_large,
    spool.spool_store_error,
    spool.spool_store_fatal,
  ].some((value) => value != null)
}

function plural(count: number, noun: string): string {
  return count + ' ' + noun + (count === 1 ? '' : 's')
}

export function agentEvidence(agent: AgentDiagnostic): AgentEvidence {
  // Partial projections (older fixtures, degraded handlers) omit some lists;
  // an omitted list is not evidence of absence and adds no item.
  const current: EvidenceItem[] = (agent.attention ?? []).map((item) => ({
    key: 'attention-' + item.id,
    text: item.message,
    tone: item.severity === 'critical' ? 'error' : 'warning',
  }))
  // Server-computed validity only; reporting status never implies a valid
  // credential.
  if (agent.credentials && !agent.credentials.some((credential) => credential.active)) {
    current.push({
      key: 'credentials',
      text: agent.credentials.length === 0 ? 'No credential issued' : 'No active credential',
      tone: 'warning',
    })
  }

  const host = agent.host
  // The fatal flag is the state the latest Host observation reports, not a
  // retained message, so it is current rather than history.
  if (host?.spool_store_fatal === true) {
    current.push({ key: 'store-fatal', text: 'Spool store fatal in the latest Host observation', tone: 'error' })
  }

  const unknown: EvidenceItem[] = []
  if (!host) {
    unknown.push({ key: 'host-not-observed', text: 'Host not observed yet', tone: 'neutral' })
  } else if (!hasSpoolObservation(host)) {
    unknown.push({ key: 'spool-not-observed', text: 'Spool not observed yet', tone: 'neutral' })
  }

  const recorded: EvidenceItem[] = []
  if (agent.sequence_gap_count > 0) {
    recorded.push({ key: 'gaps', text: plural(agent.sequence_gap_count, 'gap interval') + ' recorded', short: plural(agent.sequence_gap_count, 'gap interval'), tone: 'neutral' })
  }
  if (agent.security_event_count > 0) {
    recorded.push({
      key: 'security',
      text: plural(agent.security_event_count, 'security event') + ' recorded',
      short: plural(agent.security_event_count, 'security event'),
      tone: 'neutral',
    })
  }
  if (host) {
    const risk = hasSpoolRisk(host)
    if (host.spool_store_error) recorded.push({ key: 'store-error', text: 'Spool store error recorded', short: 'Spool store error', tone: 'error' })
    if (
      host.spool_dropped_sequence_from != null || host.spool_dropped_sequence_to != null ||
      host.spool_dropped_height_from != null || host.spool_dropped_height_to != null ||
      host.spool_dropped_time_from != null || host.spool_dropped_time_to != null
    ) {
      recorded.push({ key: 'dropped', text: 'Discarded reports recorded', short: 'Discarded reports', tone: risk ? 'error' : 'neutral' })
    }
    if (host.spool_report_too_large === true) recorded.push({ key: 'too-large', text: 'Oversized report recorded', short: 'Oversized report', tone: 'error' })
    if (host.spool_last_delivery_error) recorded.push({ key: 'delivery-error', text: 'Delivery error recorded', short: 'Delivery error', tone: 'neutral' })
    if (host.spool_pending_history_gaps != null && host.spool_pending_history_gaps > 0) {
      recorded.push({ key: 'pending-gaps', text: plural(host.spool_pending_history_gaps, 'pending history gap'), tone: 'neutral' })
    }
    const componentIssues = (host.components ?? []).filter(
      (component) => component.state !== 'ok' || component.error_code || component.error_message,
    ).length
    if (componentIssues > 0) {
      recorded.push({ key: 'components', text: plural(componentIssues, 'Host component issue'), tone: 'warning' })
    }
  }
  if (agent.inventory?.last_rejection) {
    recorded.push({ key: 'inventory-rejection', text: 'Inventory rejection recorded', short: 'Inventory rejection', tone: 'warning' })
  }
  if (agent.shutdown_forced) {
    recorded.push({ key: 'forced-shutdown', text: 'Forced shutdown recorded', short: 'Forced shutdown', tone: 'neutral' })
  }
  return { current, unknown, recorded }
}

export type DiagnosticFinding = { key: string; label: string; value: string }

export function diagnosticRangeText<T extends number | string>(
  from: T | null | undefined,
  to: T | null | undefined,
  format: (value: T) => string = String,
): string {
  if (from == null && to == null) return 'Unknown'
  return (from == null ? 'Unknown' : format(from)) + '–' + (to == null ? 'Unknown' : format(to))
}

function diagnosticSequenceRangeText(from: number | null | undefined, to: number | null | undefined): string {
  if (from == null && to == null) return 'Unknown'
  return '#' + (from == null ? 'Unknown' : from) + '–#' + (to == null ? 'Unknown' : to)
}

/** Every recorded Host/spool finding, complete, for the expanded evidence. */
export function diagnosticFindings(host: HostDiagnostic | null | undefined): DiagnosticFinding[] {
  if (!host) {
    return [{ key: 'host-not-observed', label: 'Host observation', value: 'Not observed yet' }]
  }

  const findings: DiagnosticFinding[] = []
  if (!hasSpoolObservation(host)) {
    findings.push({ key: 'spool-not-observed', label: 'Spool observation', value: 'Not observed yet' })
  } else {
    findings.push({ key: 'spool-observed', label: 'Spool observation', value: 'Observed' })
    if (host.spool_queued_reports != null) {
      findings.push({ key: 'queued-reports', label: 'Queued reports', value: String(host.spool_queued_reports) })
    }
    if (host.spool_queued_bytes != null && host.spool_queued_reports == null) {
      findings.push({ key: 'queued-bytes', label: 'Queued bytes', value: formatBytesUnknown(host.spool_queued_bytes) })
    }
    if (host.spool_in_flight != null) {
      findings.push({ key: 'delivery-state', label: 'Delivery state', value: host.spool_in_flight ? 'In flight' : 'Idle' })
    }
    if (host.spool_store_fatal != null) {
      findings.push({ key: 'store-fatal', label: 'Store fatal', value: host.spool_store_fatal ? 'Yes' : 'No' })
    }
    if (host.spool_dropped_sequence_from != null || host.spool_dropped_sequence_to != null) {
      findings.push({
        key: 'dropped-sequence',
        label: 'Dropped sequence range',
        value: diagnosticSequenceRangeText(host.spool_dropped_sequence_from, host.spool_dropped_sequence_to) + ' recorded',
      })
    }
    if (host.spool_dropped_height_from != null || host.spool_dropped_height_to != null) {
      findings.push({
        key: 'dropped-height',
        label: 'Dropped height range',
        value: diagnosticRangeText(host.spool_dropped_height_from, host.spool_dropped_height_to) + ' recorded',
      })
    }
    if (host.spool_dropped_time_from != null || host.spool_dropped_time_to != null) {
      findings.push({
        key: 'dropped-time',
        label: 'Dropped time range',
        value: diagnosticRangeText(host.spool_dropped_time_from, host.spool_dropped_time_to, formatObservedAt) + ' recorded',
      })
    }
    if (host.spool_last_delivery_at) {
      findings.push({ key: 'last-delivery', label: 'Last delivery', value: formatObservedAt(host.spool_last_delivery_at) })
    }
    if (host.spool_last_delivery_error) {
      // The raw error text stays on the Agent detail; a list row says it exists.
      findings.push({ key: 'delivery-error', label: 'Delivery error', value: 'Recorded' })
    }
    if (host.spool_store_error) {
      findings.push({ key: 'store-error', label: 'Store error', value: 'Recorded' })
    }
    if (host.spool_report_too_large != null) {
      findings.push({
        key: 'report-size',
        label: 'Report size',
        value: host.spool_report_too_large ? 'Too large recorded' : 'Within limit recorded',
      })
    }
    if (host.spool_pending_history_gaps != null) {
      findings.push({ key: 'pending-history-gaps', label: 'Pending history gaps', value: String(host.spool_pending_history_gaps) })
    }
  }

  ;(host.components ?? []).forEach((component) => {
    if (component.state !== 'ok' || component.error_code || component.error_message) {
      findings.push({
        key: 'component-' + component.component,
        label: 'Host component',
        value: component.component + ': ' + component.state,
      })
    }
  })
  return findings
}
