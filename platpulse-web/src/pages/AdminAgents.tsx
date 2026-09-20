import { ChevronUp } from 'lucide-react'
import { useEffect, useId, useRef, useState, type FormEvent, type KeyboardEvent, type ReactNode } from 'react'
import { Link, useParams } from 'react-router'
import {
  AdminApiError,
  createEnrollmentToken,
  createRecoveryToken,
  removeAgent,
  revokeAgentCredential,
  rotateAgentCredential,
  updateAgentMetadata,
  useAdminAgentAudit,
  useAdminAgentDetail,
  useAdminAgentRemovalImpact,
  useAdminDiagnostics,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { formatBytesUnknown, formatBytesPerSecond, formatIdentifier, formatPercent } from '../formatBytes'
import { hasSpoolRisk, livenessTone, receiptTimeText } from '../agentDiagnostics'
import {
  StatusBadge,
  formatObservedAt,
  livenessLabel,
} from '../components/StatusBadge'
import { Button, buttonVariants } from '../components/ui/button'
import { CardX } from '../components/ui/card-x'
import { Checkbox, Input, Select, Textarea } from '../components/ui/input'
import { Empty } from '../components/ui/empty'
import { cn } from '../lib/utils'
import { SURFACE_CARD } from '../lib/surface'
import type {
  AdminAgentRemovalImpact,
  AgentAuditItem,
  AgentCredentialSummary,
  AgentDiagnostic,
  AgentRemovalResponse,
  HostDiagnostic,
  NodeDiagnostic,
} from '../api/generated'

/**
 * PAGE-ADMIN-AGENTS, PAGE-ADMIN-AGENT-DETAIL, PAGE-ADMIN-ENROLL,
 * PAGE-ADMIN-AGENT-RECOVER, and PAGE-ADMIN-AGENT-ROTATE (design §4.3, §8.2):
 * Owner-only Agent lifecycle operations. Identity, liveness, boot/report
 * state, Inventory, credential state, and diagnostics are separate
 * dimensions (design §14.3). One-time secrets follow PATTERN-SECRET-ONCE:
 * they appear only in the success response, live only in component state,
 * and are gone when the view is left — never in URLs, history, or Audit.
 */

const CARD_SURFACE = cn('rounded-md border-none', SURFACE_CARD)

function shortId(id: string): string {
  return id.length > 14 ? id.slice(0, 8) + '…' + id.slice(-4) : id
}

/** Credential state maps onto the fixed WebUI vocabulary (`Current`,
 * `Error`) plus the Server-owned domain word `Revoked` (webui.md §2.1,
 * AGENTS.md memory: Server-owned words are shown as sent). `active` is
 * Server-computed; the browser never derives security policy. */
function credentialStatus(credential: AgentCredentialSummary): {
  label: string
  tone: 'ok' | 'warning' | 'error' | 'neutral'
} {
  if (credential.revoked_at) return { label: 'Revoked', tone: 'error' }
  if (!credential.active) return { label: 'Error', tone: 'error' }
  return { label: 'Current', tone: 'ok' }
}

function credentialSummaryText(credentials: AgentCredentialSummary[]): string {
  const active = credentials.filter((credential) => credential.active).length
  const revoked = credentials.filter((credential) => credential.revoked_at).length
  const inactive = credentials.filter((credential) => !credential.active && !credential.revoked_at).length
  if (credentials.length === 0) return 'None issued'
  return String(active) + ' active · ' + revoked + ' revoked · ' + inactive + ' inactive (not revoked) · ' + credentials.length + ' total'
}

/** Emerald detail grid: label above its value, stacked on narrow viewports. */
function DetailList({ children }: { children: ReactNode }) {
  return <dl className="grid grid-cols-1 gap-3 sm:grid-cols-2">{children}</dl>
}

function DetailItem({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="min-w-0">
      <dt className="text-xs font-medium tracking-wider text-muted-foreground">{label}</dt>
      <dd className="mt-0.5 min-w-0 break-words text-sm">{children}</dd>
    </div>
  )
}

/** Emerald dimension label: uppercase micro-caption, colourless. */
function DimensionLabel({ children }: { children: ReactNode }) {
  return (
    <span className="text-[11px] font-medium tracking-wider text-muted-foreground uppercase">
      {children}
    </span>
  )
}

function AgentIdCopyControl({ agentId }: { agentId: string }) {
  const [copyState, setCopyState] = useState<'idle' | 'copied' | 'failed'>('idle')

  async function copyAgentId() {
    if (!navigator.clipboard?.writeText) {
      setCopyState('failed')
      return
    }
    try {
      await navigator.clipboard.writeText(agentId)
      setCopyState('copied')
    } catch {
      setCopyState('failed')
    }
  }

  return (
    <span className="inline-flex flex-wrap items-center gap-2">
      <Button variant="link" size="sm" onClick={() => void copyAgentId()}>
        Copy Agent ID
      </Button>
      {copyState === 'copied' && (
        <span className="text-[11px] text-success" role="status">
          Copied to clipboard.
        </span>
      )}
      {copyState === 'failed' && (
        <span className="text-[11px] text-muted-foreground" role="status">
          Copy unavailable; select the full Agent ID above.
        </span>
      )}
    </span>
  )
}

function AgentIdentityAccess({
  agentId,
  displayName,
}: {
  agentId: string
  displayName?: string | null
}) {
  const [revealed, setRevealed] = useState(false)
  const fullIdPanelId = useId()
  return (
    <div className="flex min-w-0 flex-col items-start gap-1">
      <Link
        className="flex min-h-11 w-full min-w-0 items-center break-all font-medium underline-offset-4 hover:underline"
        to={'/admin/agents/' + encodeURIComponent(agentId)}
      >
        {displayName ?? shortId(agentId)}
      </Link>
      {displayName && (
        <small
          className="min-w-0 break-all text-[11px] text-muted-foreground"
          title={'Full Agent ID: ' + agentId}
        >
          {shortId(agentId)}
        </small>
      )}
      <Button
        variant="link"
        size="sm"
        className="max-w-full whitespace-normal"
        aria-expanded={revealed}
        aria-controls={fullIdPanelId}
        onClick={() => setRevealed((value) => !value)}
      >
        {revealed ? 'Hide full Agent ID' : 'Show full Agent ID'}
      </Button>
      {revealed && (
        <div id={fullIdPanelId} className="flex min-w-0 flex-col items-start gap-2">
          <code data-slot="agent-full-id" className="break-all text-[11px]">{agentId}</code>
          <AgentIdCopyControl agentId={agentId} />
        </div>
      )}
    </div>
  )
}

type DiagnosticFinding = {
  key: string
  label: string
  value: string
}

function hasSpoolObservation(spool: HostDiagnostic): boolean {
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

function diagnosticFindings(host: HostDiagnostic | null | undefined): DiagnosticFinding[] {
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

  host.components.forEach((component) => {
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

function spoolDiagnosticText(spool: HostDiagnostic | null | undefined): string {
  if (!spool) return 'No host observation yet'
  if (!hasSpoolObservation(spool)) return 'Spool not observed yet'
  const parts: string[] = []
  if (spool.spool_queued_reports != null) parts.push('queued reports: ' + spool.spool_queued_reports)
  if (spool.spool_in_flight === true) parts.push('delivery in flight')
  if (spool.spool_in_flight === false) parts.push('delivery idle')
  if (spool.spool_store_fatal === true) parts.push('store fatal: yes')
  if (spool.spool_store_fatal === false) parts.push('store fatal: no')
  if (spool.spool_dropped_sequence_from != null || spool.spool_dropped_sequence_to != null) {
    parts.push('dropped sequence ' + diagnosticRangeText(spool.spool_dropped_sequence_from, spool.spool_dropped_sequence_to))
  }
  if (spool.spool_dropped_height_from != null || spool.spool_dropped_height_to != null) {
    parts.push('dropped height ' + diagnosticRangeText(spool.spool_dropped_height_from, spool.spool_dropped_height_to))
  }
  if (spool.spool_dropped_time_from != null || spool.spool_dropped_time_to != null) {
    parts.push('dropped time ' + diagnosticRangeText(spool.spool_dropped_time_from, spool.spool_dropped_time_to, formatObservedAt))
  }
  if (spool.spool_last_delivery_error) parts.push('last delivery error: ' + spool.spool_last_delivery_error)
  if (spool.spool_store_error) parts.push('store error: ' + spool.spool_store_error)
  if (spool.spool_report_too_large != null) parts.push('report size: ' + (spool.spool_report_too_large ? 'too large' : 'within limit'))
  if (spool.spool_pending_history_gaps != null) parts.push('pending history gaps: ' + spool.spool_pending_history_gaps)
  return parts.length > 0 ? parts.join(' · ') : 'Spool observed; no additional evidence'
}

function diagnosticRangeText<T extends number | string>(
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

function diagnosticMessageText(value: string | null | undefined): string {
  return value == null ? 'Unknown' : value
}

/** PAGE-ADMIN-AGENTS: Agent inventory with independent dimensions. */
export default function AdminAgentsList() {
  const { generation } = useAuth()
  const query = useAdminDiagnostics(generation)
  const agents = query.data ?? []

  return (
    <section className="w-full min-w-0 space-y-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="space-y-1">
          <h1 className="text-2xl font-semibold break-words">Agents</h1>
          <p className="text-sm text-muted-foreground">
            Server reporting status, receipt time, retained Node inventory, credential validity,
            and diagnostics stay separate dimensions. Detailed boot/report state remains on each
            Agent detail page; Agent Offline is not Node Retired.
          </p>
        </div>
        <Link className={cn(buttonVariants(), 'shrink-0')} to="/admin/agents/enroll">
          Add Agent
        </Link>
      </div>
      {!query.data && query.isPending && (
        <p className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading Agent inventory…
        </p>
      )}
      {!query.data && query.isError && (
        <div
          className="flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
          role="alert"
        >
          <StatusBadge status="Error" tone="error" />{' '}
          <span className="min-w-0 break-words">
            {query.error instanceof Error ? query.error.message : 'Unable to load Agents'}
          </span>
          <Button variant="link" size="sm" onClick={() => void query.refetch()}>
            Try again
          </Button>
        </div>
      )}
      {query.data && query.isRefetchError && (
        <div
          className="flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
          role="alert"
        >
          <StatusBadge status="Error" tone="error" /> Failed to refresh; showing the last
          successful Agent values.
        </div>
      )}
      {query.data && agents.length === 0 && (
        <CardX size="medium" className={CARD_SURFACE}>
          <Empty description="No Agents enrolled yet. Add Agent generates a short-lived, single-use Enrollment Token with local instructions; an Agent appears here only after it enrolls successfully.">
            <Link className={cn(buttonVariants())} to="/admin/agents/enroll">
              Add Agent
            </Link>
          </Empty>
        </CardX>
      )}
      {query.data && agents.length > 0 && (
        <CardX
          size="medium"
          className={CARD_SURFACE}
          contentClassName="p-0"
          segmented
          title="Agent inventory"
        >
          <div className="overflow-x-auto">
            <table data-stack data-slot="agent-table" className="w-full text-sm">
              <caption className="sr-only">
                Agent, Server reporting status, receipt time, retained Node inventory, credential validity,
                and diagnostic evidence
              </caption>
              <thead>
                <tr className="border-b">
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Agent</th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Reporting status</th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Last received</th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Node Inventory</th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Credentials</th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Diagnostics</th>
                </tr>
              </thead>
              <tbody>
                {agents.map((agent) => (
                  <AgentListRow key={agent.agent_id} agent={agent} />
                ))}
              </tbody>
            </table>
          </div>
        </CardX>
      )}
    </section>
  )
}

/** Concise recorded-evidence brief for the summary column. It is derived from
 * the same findings the expanded detail renders, so the summary and the detail
 * cannot drift; raw error text still stays off the summary row, and recorded
 * evidence never becomes a current liveness or health verdict. */
function diagnosticBrief(host: HostDiagnostic | null | undefined): string {
  const findings = diagnosticFindings(host)
  const byKey = new Map(findings.map((finding) => [finding.key, finding.value]))
  if (byKey.has('host-not-observed')) return 'No Host observation yet'
  if (byKey.has('spool-not-observed')) return 'Spool not observed yet'
  const parts: string[] = []
  if (byKey.has('queued-reports')) parts.push(byKey.get('queued-reports') + ' queued')
  if (byKey.has('delivery-state')) {
    parts.push(byKey.get('delivery-state') === 'In flight' ? 'delivery in flight' : 'delivery idle')
  }
  if (byKey.has('store-fatal')) {
    parts.push(byKey.get('store-fatal') === 'Yes' ? 'store fatal' : 'store not fatal')
  }
  if (byKey.has('store-error')) parts.push('store error recorded')
  if (byKey.has('delivery-error')) parts.push('delivery error recorded')
  if (byKey.has('dropped-sequence')) parts.push('dropped sequence range recorded')
  if (byKey.has('dropped-height')) parts.push('dropped height range recorded')
  if (byKey.has('dropped-time')) parts.push('dropped time range recorded')
  if (byKey.get('report-size') === 'Too large recorded') parts.push('report too large')
  const pendingGaps = byKey.get('pending-history-gaps')
  if (pendingGaps != null && pendingGaps !== '0') {
    parts.push(pendingGaps + ' pending history gap' + (pendingGaps === '1' ? '' : 's'))
  }
  const componentIssues = findings.filter((finding) => finding.key.startsWith('component-')).length
  if (componentIssues > 0) {
    parts.push(componentIssues + ' Host component issue' + (componentIssues === 1 ? '' : 's'))
  }
  return parts.length > 0 ? parts.join(' · ') : 'Spool observed; no additional evidence'
}

/** PAGE-ADMIN-AGENTS row: one table cell per summary dimension so the six
 * columns stay aligned. The flex-column layout lives on inner wrappers, never
 * on the td itself, because a flex td drops out of the table layout
 * and collapses the remaining columns. */
function AgentListRow({ agent }: { agent: AgentDiagnostic }) {
  const liveness = livenessLabel(agent.liveness)
  const [diagnosticsOpen, setDiagnosticsOpen] = useState(false)
  const detailId = 'agent-diagnostics-' + agent.agent_id
  const collapseOnEscape = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key === 'Escape' && diagnosticsOpen) setDiagnosticsOpen(false)
  }
  return (
    <>
      <tr className="border-b border-border/60 align-top">
        <th scope="row" data-label="Agent" className="min-w-0 px-3 py-3 text-left">
          <AgentIdentityAccess agentId={agent.agent_id} displayName={agent.display_name} />
        </th>
        <td data-label="Reporting status" className="min-w-0 px-3 py-3">
          <div className="flex flex-col gap-1">
            <DimensionLabel>Server liveness</DimensionLabel>
            <StatusBadge status={liveness} tone={livenessTone(agent.liveness)} />
          </div>
        </td>
        <td data-label="Last received" className="min-w-0 px-3 py-3">
          <div className="flex flex-col gap-1">
            <DimensionLabel>Server receipt time</DimensionLabel>
            <span className="text-sm">{receiptTimeText(agent.last_received_at)}</span>
          </div>
        </td>
        <td data-label="Node Inventory" className="min-w-0 px-3 py-3">
          <div className="flex flex-col gap-1">
            <strong className="text-sm">{agent.nodes.length} retained Node{agent.nodes.length === 1 ? '' : 's'}</strong>
            <small className="text-[11px] text-muted-foreground">Active + Retired</small>
          </div>
        </td>
        <td data-label="Credentials" className="min-w-0 px-3 py-3">
          <div className="flex flex-col gap-1">
            <DimensionLabel>Server validity</DimensionLabel>
            <span className="text-sm">{credentialSummaryText(agent.credentials)}</span>
          </div>
        </td>
        <td data-label="Diagnostics" className="flex min-w-0 flex-col px-3 py-3">
          <div className="flex flex-col gap-2">
            <dl className="grid grid-cols-1 gap-1">
              <div>
                <dt className="text-[11px] text-muted-foreground">Recorded gap intervals</dt>
                <dd className="text-sm font-bold leading-none tracking-tight">{agent.sequence_gap_count}</dd>
              </div>
              <div>
                <dt className="text-[11px] text-muted-foreground">Accumulated recorded security events</dt>
                <dd className="text-sm font-bold leading-none tracking-tight">{agent.security_event_count}</dd>
              </div>
            </dl>
            <p className="text-[11px] text-muted-foreground">
              <DimensionLabel>Recorded evidence</DimensionLabel>{' '}
              <span>{diagnosticBrief(agent.host)}</span>
            </p>
            <Button
              variant="link"
              size="sm"
              aria-expanded={diagnosticsOpen}
              aria-controls={detailId}
              onClick={() => setDiagnosticsOpen((value) => !value)}
              onKeyDown={collapseOnEscape}
            >
              {diagnosticsOpen ? 'Hide diagnostics' : 'Show diagnostics'}
            </Button>
          </div>
        </td>
      </tr>
      {diagnosticsOpen && (
        <tr data-slot="detail-row" className="border-b border-border/60 bg-muted/30">
          <td colSpan={6} id={detailId} onKeyDown={collapseOnEscape} className="p-0">
            <div className="space-y-3 p-3">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <strong className="text-sm">Recorded diagnostic evidence</strong>
                <Button variant="link" size="sm" onClick={() => setDiagnosticsOpen(false)}>
                  Collapse diagnostics <ChevronUp size={16} aria-hidden="true" />
                </Button>
              </div>
              <ul className="space-y-1" aria-label="Recorded diagnostic evidence">
                {diagnosticFindings(agent.host).map((finding) => (
                  <li key={finding.key} className="flex flex-wrap items-baseline justify-between gap-2 text-sm">
                    <span className="text-[11px] font-medium text-muted-foreground">{finding.label}</span>
                    <span className="min-w-0 break-words">{finding.value}</span>
                  </li>
                ))}
              </ul>
              <p className="text-[11px] text-muted-foreground">
                Recorded evidence; Server liveness remains separate.
                {agent.host ? ' Host snapshot: ' + formatObservedAt(agent.host.updated_at) : ' No Host snapshot.'}
              </p>
            </div>
          </td>
        </tr>
      )}
    </>
  )
}

/** PAGE-ADMIN-AGENT-DETAIL: identity, credentials, liveness, inventory,
 * diagnostics, and the redacted Audit trail for one Agent. */
export function AdminAgentDetail() {
  const { agentId = '' } = useParams()
  const { generation, status } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const agent = useAdminAgentDetail(generation, agentId)
  const audit = useAdminAgentAudit(generation, agentId)
  const [removed, setRemoved] = useState<AgentRemovalResponse | null>(null)
  const notFound =
    agent.isError && agent.error instanceof AdminApiError && agent.error.code === 'agent_not_found'

  if (removed) {
    return (
      <section className="w-full min-w-0 space-y-4">
        <div className="space-y-1">
          <h1 className="text-lg font-semibold break-words">Agent removed</h1>
          <p className="text-sm text-muted-foreground">
            Agent {shortId(agentId)} was permanently removed at{' '}
            {formatObservedAt(removed.deleted_at)}.
          </p>
        </div>
        <CardX
          size="medium"
          className={CARD_SURFACE}
          header={<h3 className="text-sm font-medium">Removal result</h3>}
        >
          <ul className="list-disc space-y-1 pl-5 text-sm text-muted-foreground">
            <li>{removed.revoked_credential_count} credential(s) were revoked.</li>
            <li>
              {removed.purged_nodes.length} Node(s) were permanently purged (
              {removed.removed.total_owned_rows} Node-owned rows).
            </li>
            <li>
              The remote process was not stopped and local configuration was not changed;
              handle the Host locally.
            </li>
          </ul>
          <div className="mt-3">
            <Link className={cn(buttonVariants(), 'w-fit')} to="/admin/agents">
              All Agents
            </Link>
          </div>
        </CardX>
      </section>
    )
  }

  if (notFound) {
    return (
      <section className="w-full min-w-0 space-y-4">
        <div className="space-y-1">
          <h1 className="text-lg font-semibold">Agent unavailable</h1>
          <p className="text-sm text-muted-foreground">This Agent is no longer available.</p>
        </div>
      </section>
    )
  }
  return (
    <section className="w-full min-w-0 space-y-4">
      <div className="space-y-1">
        <h1 className="text-lg font-semibold break-words">
          {agent.data?.display_name ?? 'Agent ' + shortId(agentId)}
          <span className="mt-0.5 block text-xs font-medium text-muted-foreground break-all">{agentId}</span>
        </h1>
        <p className="text-sm text-muted-foreground">
          <Link
            className="inline-flex min-h-11 min-w-11 items-center font-medium underline-offset-4 hover:underline"
            to="/admin/agents"
          >
            All Agents
          </Link>{' '}
          · identity, liveness, boot/report state, Inventory, credentials, and diagnostics
          stay separate.
        </p>
      </div>
      {!agent.data && (
        <>
          {agent.isPending && (
            <p className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground" role="status">
              <StatusBadge status="Starting" tone="neutral" /> Loading Agent state…
            </p>
          )}
          {agent.isError && (
            <div
              className="flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
              role="alert"
            >
              <StatusBadge status="Error" tone="error" />{' '}
              <span className="min-w-0 break-words">
                {agent.error instanceof Error ? agent.error.message : 'Unable to load the Agent'}
              </span>
              <Button variant="link" size="sm" onClick={() => void agent.refetch()}>
                Try again
              </Button>
            </div>
          )}
        </>
      )}
      {agent.data && (
        <>
          {agent.isRefetchError && (
            <div
              className="flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
              role="alert"
            >
              <StatusBadge status="Error" tone="error" /> Failed to refresh; showing the last
              successful Agent values.
            </div>
          )}
          <AgentDetailSummary agent={agent.data} />
          <section className="space-y-3" aria-labelledby="agent-profile-heading">
            <div className="space-y-1">
              <span className="text-[11px] font-medium text-muted-foreground">01</span>
              <h2 id="agent-metadata-heading" className="text-lg font-semibold">Server-owned metadata</h2>
              <p className="text-sm text-muted-foreground">Server-owned display name and notes. The Agent ID, Host identity, liveness, Epoch, and collection configuration stay read-only.</p>
            </div>
            <MetadataPanel agent={agent.data} onSaved={() => void agent.refetch()} />
          </section>
          <section className="space-y-3" aria-labelledby="agent-overview-heading">
            <div className="space-y-1">
              <span className="text-[11px] font-medium text-muted-foreground">02</span>
              <h2 id="agent-overview-heading" className="text-lg font-semibold">Overview</h2>
              <p className="text-sm text-muted-foreground">Identity and the Agent-declared Node Inventory.</p>
            </div>
            <div className="grid gap-3 lg:grid-cols-2">
              <IdentityPanel agent={agent.data} />
              <InventoryPanel nodes={agent.data.nodes} />
            </div>
          </section>
          <section className="space-y-3" aria-labelledby="agent-runtime-heading">
            <div className="space-y-1">
              <span className="text-[11px] font-medium text-muted-foreground">03</span>
              <h2 id="agent-runtime-heading" className="text-lg font-semibold">Runtime and reporting</h2>
              <p className="text-sm text-muted-foreground">Server liveness remains separate from boot and report lifecycle.</p>
            </div>
            <div className="grid gap-3 lg:grid-cols-2">
              <LivenessPanel agent={agent.data} />
              <BootReportPanel agent={agent.data} />
            </div>
          </section>
          <section className="space-y-3" aria-labelledby="agent-credentials-heading">
            <div className="space-y-1">
              <span className="text-[11px] font-medium text-muted-foreground">04</span>
              <h2 id="agent-credentials-heading" className="text-lg font-semibold">Credentials</h2>
              <p className="text-sm text-muted-foreground">Server-owned credential validity and explicit revocation.</p>
            </div>
            <CredentialsPanel agent={agent.data} onConflictReload={() => void agent.refetch()} />
          </section>
          <section className="space-y-3" aria-labelledby="agent-diagnostics-heading">
            <div className="space-y-1">
              <span className="text-[11px] font-medium text-muted-foreground">05</span>
              <h2 id="agent-diagnostics-heading" className="text-lg font-semibold">Diagnostics</h2>
              <p className="text-sm text-muted-foreground">Recorded evidence is shown without inferring current recovery or failure.</p>
            </div>
            <DiagnosticsPanel agent={agent.data} />
          </section>
          <section className="space-y-3" aria-labelledby="agent-audit-heading">
            <div className="space-y-1">
              <span className="text-[11px] font-medium text-muted-foreground">06</span>
              <h2 id="agent-audit-heading" className="text-lg font-semibold">Audit</h2>
              <p className="text-sm text-muted-foreground">Immutable, redacted lifecycle events for this Agent.</p>
            </div>
            <AuditTrailPanel audit={audit} agentId={agentId} />
          </section>
          <section className="space-y-3" aria-labelledby="agent-removal-heading">
            <div className="space-y-1">
              <span className="text-[11px] font-medium text-muted-foreground">07</span>
              <h2 id="agent-removal-heading" className="text-lg font-semibold">Danger zone</h2>
              <p className="text-sm text-muted-foreground">
                Irreversible Owner disposition. Credential revocation and Agent removal are
                separate actions.
              </p>
            </div>
            <AgentRemovalPanel
              agentId={agent.data.agent_id}
              displayName={agent.data.display_name ?? 'Agent ' + shortId(agent.data.agent_id)}
              csrfToken={csrfToken}
              onRemoved={setRemoved}
            />
          </section>
        </>
      )}
    </section>
  )
}

type AgentSummaryWarning = {
  kind: 'current' | 'recorded' | 'unknown'
  message: string
}

function AgentDetailSummary({ agent }: { agent: AgentDiagnostic }) {
  const liveness = livenessLabel(agent.liveness)
  const warnings = agentSummaryWarnings(agent)
  const activeCredentials = agent.credentials.filter((credential) => credential.active).length
  const bootStatus = agent.boot_status && agent.boot_status !== 'unknown' ? agent.boot_status : 'Unknown'
  return (
    <section
      className={cn('space-y-3 rounded-md border-none p-4', SURFACE_CARD)}
      aria-labelledby="agent-summary-heading"
    >
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0 space-y-1">
          <span className="text-[11px] font-medium tracking-wider text-muted-foreground uppercase">Agent summary</span>
          <h2 id="agent-summary-heading" className="text-lg font-semibold break-all">
            {shortId(agent.agent_id)}
          </h2>
          <p className="flex min-w-0 flex-wrap items-center gap-2">
            <code className="break-all text-[11px]" title={agent.agent_id}>{formatIdentifier(agent.agent_id)}</code>
            <span className="sr-only">Full Agent ID: {agent.agent_id}</span>
            <AgentIdCopyControl agentId={agent.agent_id} />
          </p>
        </div>
        <div className="flex flex-wrap items-start gap-4" aria-label="Current Agent dimensions">
          <div className="flex flex-col gap-1">
            <DimensionLabel>Server liveness</DimensionLabel>
            <StatusBadge status={liveness} tone={livenessTone(agent.liveness)} />
          </div>
          <div className="flex flex-col gap-1">
            <DimensionLabel>Boot status</DimensionLabel>
            <strong className="text-sm">{bootStatus}</strong>
          </div>
          <div className="flex flex-col gap-1">
            <DimensionLabel>Active credentials</DimensionLabel>
            <strong className="text-sm">{activeCredentials} active · {agent.credentials.length} total</strong>
          </div>
        </div>
      </div>
      <dl className="grid grid-cols-2 gap-3 sm:grid-cols-4">
        <div>
          <dt className="text-xs font-medium tracking-wider text-muted-foreground">Agent Epoch</dt>
          <dd className="text-sm font-bold leading-none tracking-tight">{agent.agent_epoch}</dd>
        </div>
        <div>
          <dt className="text-xs font-medium tracking-wider text-muted-foreground">Last received</dt>
          <dd className="text-sm">{receiptTimeText(agent.last_received_at)}</dd>
        </div>
        <div>
          <dt className="text-xs font-medium tracking-wider text-muted-foreground">Report sequence</dt>
          <dd className="text-sm">{agent.last_report_sequence == null ? 'Never received' : '#' + agent.last_report_sequence}</dd>
        </div>
        <div>
          <dt className="text-xs font-medium tracking-wider text-muted-foreground">Declared Nodes</dt>
          <dd className="text-sm font-bold leading-none tracking-tight">{agent.nodes.length}</dd>
        </div>
      </dl>
      {warnings.length > 0 && (
        <div className="space-y-2" role="note" aria-label="Important Agent warnings">
          <h3 className="text-sm font-medium">Important warnings</h3>
          <ul className="space-y-1">
            {warnings.map((warning, index) => (
              <li
                key={warning.kind + '-' + index}
                data-kind={warning.kind}
                className="flex flex-wrap items-baseline gap-2 text-sm"
              >
                <strong
                  className={cn(
                    'text-[11px] font-medium tracking-wider uppercase',
                    warning.kind === 'current'
                      ? 'text-warning'
                      : warning.kind === 'recorded'
                        ? 'text-destructive'
                        : 'text-muted-foreground',
                  )}
                >
                  {warning.kind === 'current' ? 'Current state' : warning.kind === 'recorded' ? 'Recorded history' : 'Unknown'}
                </strong>
                <span className="min-w-0 break-words">{warning.message}</span>
              </li>
            ))}
          </ul>
        </div>
      )}
    </section>
  )
}

function agentSummaryWarnings(agent: AgentDiagnostic): AgentSummaryWarning[] {
  const warnings: AgentSummaryWarning[] = []
  if (agent.liveness === 'offline') {
    warnings.push({ kind: 'current', message: 'Server liveness is Error; the Agent is not reporting now.' })
  } else if (agent.liveness !== 'online') {
    warnings.push({ kind: 'unknown', message: 'Server liveness is Unknown; no current reporting state is known.' })
  }
  if (!agent.boot_status || agent.boot_status === 'unknown') {
    warnings.push({ kind: 'unknown', message: 'Boot status is Unknown.' })
  }
  if (agent.shutdown_forced) {
    warnings.push({ kind: 'recorded', message: 'A forced shutdown was recorded; this is separate from current liveness.' })
  }
  if (agent.shutdown_last_error) {
    warnings.push({ kind: 'recorded', message: 'Shutdown error: ' + agent.shutdown_last_error })
  }
  if (agent.sequence_gap_count > 0) {
    warnings.push({ kind: 'recorded', message: agent.sequence_gap_count + ' report gap' + (agent.sequence_gap_count === 1 ? '' : 's') + ' recorded.' })
  }
  if (agent.security_event_count > 0) {
    warnings.push({ kind: 'recorded', message: agent.security_event_count + ' security event' + (agent.security_event_count === 1 ? '' : 's') + ' recorded.' })
  }
  if (!agent.host) {
    warnings.push({ kind: 'unknown', message: 'Host observation is not available yet; absent diagnostic values remain Unknown.' })
  } else {
    if (hasSpoolRisk(agent.host)) {
      warnings.push({ kind: 'recorded', message: 'Recorded spool evidence includes dropped, oversized, or fatal-storage state; it does not by itself replace Server liveness.' })
    }
    if (agent.host.spool_last_delivery_error || agent.host.spool_store_error || (agent.host.spool_pending_history_gaps ?? 0) > 0) {
      warnings.push({ kind: 'recorded', message: 'Recorded spool delivery or storage errors remain available in Diagnostics.' })
    }
    const componentIssues = agent.host.components.filter((component) => component.state !== 'ok' || component.error_code || component.error_message)
    if (componentIssues.length > 0) {
      warnings.push({ kind: 'recorded', message: componentIssues.length + ' Host component issue' + (componentIssues.length === 1 ? '' : 's') + ' recorded.' })
    }
  }
  return warnings
}

function IdentityPanel({ agent }: { agent: AgentDiagnostic }) {
  return (
    <CardX size="medium" className={CARD_SURFACE} header={<h3 className="text-sm font-medium">Identity</h3>}>
      <DetailList>
        <DetailItem label="Agent ID">
          <code className="break-all text-[11px]">{agent.agent_id}</code>
        </DetailItem>
        <DetailItem label="Agent Epoch">{agent.agent_epoch}</DetailItem>
        <DetailItem label="Capabilities">
          {agent.capabilities.length > 0 ? agent.capabilities.join(', ') : 'Unsupported'}
        </DetailItem>
      </DetailList>
    </CardX>
  )
}

/** PAGE-ADMIN-AGENT-DETAIL, SCN-AGENT-METADATA: Owner-editable Server-owned
 * display name and notes. The stable Agent ID, observed Host identity,
 * Server-derived liveness, Agent Epoch, and local collection configuration
 * are never editable, and no name is inferred from a Node or diagnostic
 * field. Server-backed and authoritative: success refetches; a failed save
 * keeps the form and reports an actionable error. */
function MetadataPanel({ agent, onSaved }: { agent: AgentDiagnostic; onSaved: () => void }) {
  const { status } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const [displayName, setDisplayName] = useState(agent.display_name ?? '')
  const [notes, setNotes] = useState(agent.notes ?? '')
  const [busy, setBusy] = useState(false)
  const [message, setMessage] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  // Re-sync with each authoritative projection (refetch, another Owner's
  // update, or a Server restart). A failed local edit is preserved only
  // until the next authoritative value arrives.
  useEffect(() => {
    setDisplayName(agent.display_name ?? '')
    setNotes(agent.notes ?? '')
  }, [agent.display_name, agent.notes])

  async function save(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (busy) return
    setBusy(true)
    setError(null)
    setMessage(null)
    try {
      await updateAgentMetadata(
        agent.agent_id,
        displayName.trim() === '' ? null : displayName,
        notes.trim() === '' ? null : notes,
        csrfToken,
      )
      setMessage('Saved. The name and notes are Server-owned and visible to every Owner.')
      onSaved()
    } catch (caught) {
      setError(
        caught instanceof AdminApiError && caught.code === 'agent_not_found'
          ? 'This Agent is no longer available.'
          : caught instanceof Error
            ? caught.message
            : 'Unable to save the Agent name and notes',
      )
    } finally {
      setBusy(false)
    }
  }

  return (
    <CardX
      size="medium"
      className={CARD_SURFACE}
      header={<h3 className="text-sm font-medium">Display name and notes</h3>}
    >
      <form className="grid max-w-xl gap-3" onSubmit={save}>
        <div className="flex flex-col gap-1">
          <label
            className="text-xs font-medium tracking-wider text-muted-foreground"
            htmlFor="agent-display-name"
          >
            Display name
          </label>
          <Input
            id="agent-display-name"
            value={displayName}
            maxLength={128}
            placeholder="Unnamed Agent (identified by the stable ID)"
            onChange={(event) => setDisplayName(event.target.value)}
          />
          <p className="text-[11px] text-muted-foreground">
            Optional, at most 128 characters. Clear it to identify the Agent by its stable ID.
          </p>
        </div>
        <div className="flex flex-col gap-1">
          <label
            className="text-xs font-medium tracking-wider text-muted-foreground"
            htmlFor="agent-notes"
          >
            Notes
          </label>
          <Textarea
            id="agent-notes"
            value={notes}
            maxLength={2000}
            rows={4}
            placeholder="Optional operational context"
            onChange={(event) => setNotes(event.target.value)}
          />
          <p className="text-[11px] text-muted-foreground">Optional, at most 2000 characters.</p>
        </div>
        <div className="flex flex-wrap items-center gap-3">
          <Button type="submit" disabled={busy}>
            {busy ? 'Saving…' : 'Save name and notes'}
          </Button>
          <Button
            type="button"
            variant="outline"
            disabled={busy}
            onClick={() => {
              setDisplayName(agent.display_name ?? '')
              setNotes(agent.notes ?? '')
              setError(null)
              setMessage(null)
            }}
          >
            Reset
          </Button>
        </div>
        {message && (
          <p className="text-sm text-success" role="status">
            {message}
          </p>
        )}
        {error && (
          <p className="text-sm text-destructive" role="alert">
            {error}
          </p>
        )}
        <p className="text-[11px] text-muted-foreground">
          Saved through the Owner-only, CSRF-guarded Admin route and recorded in the Audit
          trail as <code>agent_metadata_changed</code>. The Agent ID,
          Host identity, Server liveness, Agent Epoch, and collection configuration are
          read-only and never editable here.
        </p>
      </form>
    </CardX>
  )
}

function LivenessPanel({ agent }: { agent: AgentDiagnostic }) {
  const liveness = livenessLabel(agent.liveness)
  return (
    <CardX size="medium" className={CARD_SURFACE} header={<h3 className="text-sm font-medium">Liveness</h3>}>
      <DetailList>
        <DetailItem label="Server liveness">
          <StatusBadge status={liveness} tone={livenessTone(agent.liveness)} />
        </DetailItem>
        <DetailItem label="Server receipt time">{receiptTimeText(agent.last_received_at)}</DetailItem>
      </DetailList>
    </CardX>
  )
}

function BootReportPanel({ agent }: { agent: AgentDiagnostic }) {
  return (
    <CardX size="medium" className={CARD_SURFACE} header={<h3 className="text-sm font-medium">Boot and report state</h3>}>
      <DetailList>
        <DetailItem label="Boot status">{agent.boot_status}</DetailItem>
        <DetailItem label="Full active boot ID">
          <code className="break-all text-[11px]">{agent.active_boot_id ?? 'Unknown'}</code>
        </DetailItem>
        <DetailItem label="Previous boot ID">
          <code className="break-all text-[11px]">{agent.previous_boot_id ?? 'None'}</code>
        </DetailItem>
        <DetailItem label="Close report ID">
          <code className="break-all text-[11px]">{agent.close_report_id ?? 'None'}</code>
        </DetailItem>
        <DetailItem label="Report sequence">
          {agent.last_report_sequence == null ? 'Never received' : 'sequence #' + agent.last_report_sequence}
        </DetailItem>
        <DetailItem label="Shutdown state">
          {agent.shutdown_state}
          {agent.shutdown_forced ? ' · forced' : ''}
          {agent.shutdown_last_error ? ' · ' + agent.shutdown_last_error : ''}
        </DetailItem>
        <DetailItem label="Shutdown report ID">
          <code className="break-all text-[11px]">{agent.shutdown_report_id ?? 'None'}</code>
        </DetailItem>
        <DetailItem label="Shutdown report sequence">{agent.shutdown_report_sequence ?? 'None'}</DetailItem>
        <DetailItem label="Shutdown started">{formatObservedAt(agent.shutdown_started_at)}</DetailItem>
        <DetailItem label="Shutdown deadline">{formatObservedAt(agent.shutdown_deadline_at)}</DetailItem>
        <DetailItem label="Shutdown finished">{formatObservedAt(agent.shutdown_finished_at)}</DetailItem>
        <DetailItem label="Unresolved shutdown range">
          {agent.shutdown_unresolved_range ? agent.shutdown_unresolved_range.join('–') : 'None'}
        </DetailItem>
        <DetailItem label="Shutdown updated">{formatObservedAt(agent.shutdown_updated_at)}</DetailItem>
      </DetailList>
    </CardX>
  )
}

function InventoryPanel({ nodes }: { nodes: NodeDiagnostic[] }) {
  return (
    <CardX size="medium" className={CARD_SURFACE} header={<h3 className="text-sm font-medium">Inventory</h3>}>
      {nodes.length === 0 && (
        <Empty description="No PlatON Nodes declared by this Agent yet." />
      )}
      {nodes.length > 0 && (
        <ul className="space-y-2">
          {nodes.map((node) => (
            <li key={node.node_id} className="flex flex-wrap items-center gap-3 text-sm">
              <span className="min-w-0">
                <strong className="break-words">{node.display_name ?? node.node_id}</strong>{' '}
                <small className="text-[11px] text-muted-foreground" title={'Full Node ID: ' + node.node_id}>
                  <span aria-hidden="true">{shortId(node.node_id)}</span>
                  <span className="sr-only">Full Node ID: {node.node_id}</span>
                </small>
              </span>
              <span className="text-[11px] text-muted-foreground">{node.network_key}</span>
              <span className="text-[11px]">{node.lifecycle}</span>
              <StatusBadge status={node.visibility} tone="neutral" />
            </li>
          ))}
        </ul>
      )}
    </CardX>
  )
}

/** Credential dimension: ids and lifecycle instants only. Revocation is
 * explicit, immediate, and never optimistic — success refetches the
 * authoritative credential state. */
function CredentialsPanel({
  agent,
  onConflictReload,
}: {
  agent: AgentDiagnostic
  onConflictReload: () => void
}) {
  const { status } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const [confirmingId, setConfirmingId] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [message, setMessage] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  async function revoke(credentialId: string) {
    if (busy) return
    setBusy(true)
    setMessage(null)
    setError(null)
    try {
      const result = await revokeAgentCredential(agent.agent_id, credentialId, csrfToken)
      setMessage('Credential revoked at ' + formatObservedAt(result.revoked_at) + '.')
      setConfirmingId(null)
    } catch (caught) {
      // Typed conflicts reload the authoritative state (PATTERN-CONFLICT-
      // RELOAD): a concurrent operator already revoked this credential, so
      // the panel must show the Server's current credential dimension
      // instead of an optimistic one. Ordinary drafts are not overwritten.
      if (caught instanceof AdminApiError && caught.code === 'credential_already_revoked') {
        onConflictReload()
      }
      setError(caught instanceof Error ? caught.message : 'Unable to revoke the credential')
      setConfirmingId(null)
    } finally {
      setBusy(false)
    }
  }

  return (
    <CardX
      size="medium"
      className={CARD_SURFACE}
      id="credentials"
      header={<h3 className="text-sm font-medium">Credential records</h3>}
    >
      <p className="text-sm text-muted-foreground">
        Only non-sensitive credential ids and lifecycle instants are shown; secrets are
        never stored or displayed again.
      </p>
      {message && (
        <p className="mt-2 text-sm text-success" role="status">
          {message}
        </p>
      )}
      {error && (
        <p className="mt-2 text-sm text-destructive" role="alert">
          {error}
        </p>
      )}
      {agent.credentials.length === 0 && (
        <div className="mt-2">
          <Empty description="No credentials issued yet." />
        </div>
      )}
      {agent.credentials.length > 0 && (
        <ul className="mt-3 space-y-3">
          {agent.credentials.map((credential) => {
            const state = credentialStatus(credential)
            return (
              <li key={credential.credential_id} data-slot="credential-item" className="space-y-1 rounded-sm border border-border/60 p-2">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="break-all text-[11px]">{credential.credential_id}</span>
                  <StatusBadge status={state.label} tone={state.tone} />
                  {credential.revoke_after && credential.active && (
                    <small className="text-[11px] text-muted-foreground">
                      Overlap expires {formatObservedAt(credential.revoke_after)}
                    </small>
                  )}
                </div>
                <small className="block text-[11px] text-muted-foreground">
                  Issued {formatObservedAt(credential.created_at)}
                  {credential.revoked_at ? ' · revoked ' + formatObservedAt(credential.revoked_at) : ''}
                </small>
                {credential.active && (
                  <div className="flex flex-wrap items-center gap-2">
                    {confirmingId === credential.credential_id ? (
                      <>
                        <span className="text-[11px] text-muted-foreground">
                          Revoke now? The Agent stops authenticating immediately.
                        </span>
                        <Button
                          variant="destructive"
                          size="sm"
                          onClick={() => void revoke(credential.credential_id)}
                          disabled={busy}
                        >
                          {busy ? 'Revoking…' : 'Confirm revoke'}
                        </Button>
                        <Button
                          variant="link"
                          size="sm"
                          onClick={() => setConfirmingId(null)}
                          disabled={busy}
                        >
                          Cancel
                        </Button>
                      </>
                    ) : (
                      <Button
                        variant="link"
                        size="sm"
                        onClick={() => setConfirmingId(credential.credential_id)}
                        disabled={busy}
                      >
                        Revoke
                      </Button>
                    )}
                  </div>
                )}
              </li>
            )
          })}
        </ul>
      )}
    </CardX>
  )
}

/** Owner-only Agent Removal (design §15.2, webui.md §15.2, issue #171).
 * Distinct from Credential Revocation: it revokes every credential, purges
 * every owned Node through the Node Purge path, and marks the Agent removed.
 * The confirmation lists the Server-authoritative owned Nodes, blocks on an
 * unhandled Transfer, and reports the authoritative completion. The remote
 * process is never stopped and local configuration is never changed. */
function AgentRemovalPanel({
  agentId,
  displayName,
  csrfToken,
  onRemoved,
}: {
  agentId: string
  displayName: string
  csrfToken: string
  onRemoved: (result: AgentRemovalResponse) => void
}) {
  const { generation } = useAuth()
  const [confirming, setConfirming] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [restoreFocus, setRestoreFocus] = useState(false)
  const triggerRef = useRef<HTMLButtonElement>(null)
  const impact = useAdminAgentRemovalImpact(generation, agentId, confirming)

  // Cancelling the confirmation restores focus to the safe trigger, so a
  // keyboard user is never dropped onto the document body.
  useEffect(() => {
    if (!confirming && restoreFocus) {
      triggerRef.current?.focus()
      setRestoreFocus(false)
    }
  }, [confirming, restoreFocus])

  async function confirm(current: AdminAgentRemovalImpact) {
    if (busy) return
    setBusy(true)
    setError(null)
    try {
      onRemoved(
        await removeAgent(
          agentId,
          agentId,
          current.owned_nodes.map((node) => node.node_id),
          csrfToken,
        ),
      )
    } catch (caught) {
      // Typed conflicts refetch the authoritative scope: a concurrent
      // ownership change or a new Transfer must be seen before reconfirming.
      if (
        caught instanceof AdminApiError &&
        (caught.code === 'ownership_changed' ||
          caught.code === 'pending_transfer' ||
          caught.code === 'agent_not_found')
      ) {
        void impact.refetch()
      }
      setError(caught instanceof Error ? caught.message : 'Unable to remove the Agent')
      setBusy(false)
    }
  }

  return (
    <CardX
      size="medium"
      className={CARD_SURFACE}
      id="removal"
      header={<h3 className="text-sm font-medium">Permanent Agent removal</h3>}
    >
      <p className="text-sm text-muted-foreground">
        Permanently delete this Agent to revoke every credential, remove it from current
        monitoring, and permanently purge every Node it owns (their observations, monitoring
        history, and Node Validator Links). This cannot be undone, and Recovery, Rotation, a
        late report, or a recovery token cannot revive the same identity.
      </p>
      <p className="mt-2 text-sm text-muted-foreground">
        The remote Agent/Node process is not stopped or uninstalled, and local configuration is
        not changed, so you must still handle the Host locally. Shared Agent/Host data,
        independent Validator history, existing Incident evidence, and Audit are preserved.
        This is not the same as revoking a credential, and it is not merely hiding a row.
      </p>
      {!confirming && (
        <div className="mt-3">
          <Button
            ref={triggerRef}
            variant="destructive"
            onClick={() => setConfirming(true)}
          >
            Permanently delete Agent
          </Button>
        </div>
      )}
      {confirming && (
        <div
          className="mt-3 space-y-3 rounded-md border border-destructive/40 bg-destructive/5 p-3"
          role="alertdialog"
          aria-label="Confirm permanent Agent removal"
        >
          <p className="text-sm font-medium">Confirm permanent removal of {displayName}?</p>
          {!impact.data && impact.isPending && (
            <p role="status" className="text-sm text-muted-foreground">
              Measuring the removal scope…
            </p>
          )}
          {impact.isError && (
            <div className="text-sm" role="alert">
              <span className="text-destructive">
                {impact.error instanceof Error
                  ? impact.error.message
                  : 'Unable to measure the removal scope'}
              </span>{' '}
              <Button variant="link" size="sm" onClick={() => void impact.refetch()}>
                Try again
              </Button>
            </div>
          )}
          {impact.data && (
            <>
              {impact.data.pending_transfers.length > 0 && (
                <p className="text-sm text-destructive" role="alert">
                  Removal is blocked by {impact.data.pending_transfers.length} unhandled
                  Transfer(s):{' '}
                  {impact.data.pending_transfers.map((transfer) => transfer.node_id).join(', ')}.
                  Complete, cancel, or let them expire first.
                </p>
              )}
              <p className="text-sm font-medium">
                This Agent owns {impact.data.owned_nodes.length} Node(s):
              </p>
              {impact.data.owned_nodes.length === 0 ? (
                <p className="text-sm text-muted-foreground">
                  No Nodes are owned by this Agent.
                </p>
              ) : (
                <ul className="list-disc space-y-1 pl-5 text-sm text-muted-foreground">
                  {impact.data.owned_nodes.map((node) => (
                    <li key={node.node_id} className="break-all">
                      {node.display_name ?? node.node_id}{' '}
                      <span className="text-[11px]">
                        ({node.network_display_name} · {node.lifecycle})
                      </span>
                    </li>
                  ))}
                </ul>
              )}
              <ul className="list-disc space-y-1 pl-5 text-sm text-muted-foreground">
                <li>
                  {impact.data.active_credential_count} active credential(s) of{' '}
                  {impact.data.credential_count} will be revoked.
                </li>
                <li>
                  {impact.data.counts.total_owned_rows} Node-owned row(s) will be permanently
                  deleted across those Nodes.
                </li>
                <li>The remote process is not stopped; local configuration is not changed.</li>
              </ul>
            </>
          )}
          {error && (
            <p className="text-sm text-destructive" role="alert">
              {error}
            </p>
          )}
          <div className="flex flex-wrap gap-2">
            <Button
              variant="destructive"
              disabled={busy || !impact.data || !impact.data.can_remove}
              onClick={() => impact.data && void confirm(impact.data)}
            >
              {busy ? 'Removing…' : 'Confirm permanent removal'}
            </Button>
            <Button
              variant="outline"
              autoFocus
              disabled={busy}
              onClick={() => {
                setConfirming(false)
                setError(null)
                setRestoreFocus(true)
              }}
            >
              Cancel
            </Button>
          </div>
        </div>
      )}
    </CardX>
  )
}

function DiagnosticsPanel({ agent }: { agent: AgentDiagnostic }) {
  const host = agent.host
  return (
    <CardX
      size="medium"
      className={CARD_SURFACE}
      header={<h3 className="text-sm font-medium">Host and component evidence</h3>}
    >
      <p className="text-sm text-muted-foreground">
        {host
          ? 'Recorded evidence from the latest Host observation; it does not infer current failure or recovery. Server liveness remains independent.'
          : 'No Host observation is available; absent diagnostic fields remain unknown.'}
      </p>
      <div className="mt-3">
        <DetailList>
          <DetailItem label="Clock">
            {agent.clock_status}
            {agent.clock_skew_ms != null ? ' · skew ' + agent.clock_skew_ms + ' ms' : ''}
          </DetailItem>
          <DetailItem label="Recorded gap intervals">{agent.sequence_gap_count}</DetailItem>
          <DetailItem label="Accumulated recorded security events">{agent.security_event_count}</DetailItem>
          {!host && (
            <DetailItem label="Host observation">No host observation yet</DetailItem>
          )}
          {host && (
            <>
              <DetailItem label="Host observation">{formatObservedAt(host.updated_at)}</DetailItem>
              <DetailItem label="Host CPU">{formatPercent(host.cpu_percent)}</DetailItem>
              <DetailItem label="Host memory used / total">
                {formatBytesUnknown(host.memory_used_bytes)} / {formatBytesUnknown(host.memory_total_bytes)}
              </DetailItem>
              <DetailItem label="Host load (1 / 5 / 15)">
                {host.load1 == null ? 'Unknown' : host.load1} / {host.load5 == null ? 'Unknown' : host.load5} / {host.load15 == null ? 'Unknown' : host.load15}
              </DetailItem>
              <DetailItem label="Host network RX / TX">
                {formatBytesPerSecond(host.network_rx_bytes_per_sec)} / {formatBytesPerSecond(host.network_tx_bytes_per_sec)}
              </DetailItem>
              <DetailItem label="Host components">
                {host.components.length === 0 ? 'None observed' : (
                  <ul className="space-y-2">
                    {host.components.map((component) => (
                      <li key={component.component} className="min-w-0">
                        <strong>{component.component}</strong>: {component.state}
                        {component.error_code ? ' · ' + component.error_code : ''}
                        {component.error_message ? ' · ' + component.error_message : ''}
                        <small className="mt-0.5 block text-[11px] text-muted-foreground">
                          attempted {formatObservedAt(component.attempted_at)} · observed {formatObservedAt(component.observed_at)} · received {formatObservedAt(component.received_at)} · state revision {component.state_revision} · value revision {component.value_revision}
                        </small>
                      </li>
                    ))}
                  </ul>
                )}
              </DetailItem>
              <DetailItem label="Spool observation">{hasSpoolObservation(host) ? 'Observed' : 'Not observed yet'}</DetailItem>
              <DetailItem label="Spool evidence">{spoolDiagnosticText(host)}</DetailItem>
              <DetailItem label="Spool queued reports">{host.spool_queued_reports ?? 'Unknown'}</DetailItem>
              <DetailItem label="Spool delivery state">
                {host.spool_in_flight == null ? 'Unknown' : host.spool_in_flight ? 'In flight' : 'Idle'}
              </DetailItem>
              <DetailItem label="Spool bytes / capacity">
                {formatBytesUnknown(host.spool_queued_bytes)} / {formatBytesUnknown(host.spool_capacity_bytes)}
              </DetailItem>
              <DetailItem label="Spool oldest / maximum age">
                {host.spool_oldest_queued_age_ms == null ? 'Unknown' : host.spool_oldest_queued_age_ms + ' ms'} / {host.spool_max_age_seconds == null ? 'Unknown' : host.spool_max_age_seconds + ' s'}
              </DetailItem>
              <DetailItem label="Last spool delivery">
                {host.spool_last_delivery_at == null ? 'Unknown' : formatObservedAt(host.spool_last_delivery_at)}
              </DetailItem>
              <DetailItem label="Last delivery error">{diagnosticMessageText(host.spool_last_delivery_error)}</DetailItem>
              <DetailItem label="Dropped sequence range">
                {diagnosticRangeText(host.spool_dropped_sequence_from, host.spool_dropped_sequence_to)}
              </DetailItem>
              <DetailItem label="Dropped height range">
                {diagnosticRangeText(host.spool_dropped_height_from, host.spool_dropped_height_to)}
              </DetailItem>
              <DetailItem label="Dropped time range">
                {diagnosticRangeText(host.spool_dropped_time_from, host.spool_dropped_time_to, formatObservedAt)}
              </DetailItem>
              <DetailItem label="Pending history gaps">{host.spool_pending_history_gaps ?? 'Unknown'}</DetailItem>
              <DetailItem label="Report size state">
                {host.spool_report_too_large == null ? 'Unknown' : host.spool_report_too_large ? 'Too large' : 'Within limit'}
              </DetailItem>
              <DetailItem label="Spool store fatal">
                {host.spool_store_fatal == null ? 'Unknown' : host.spool_store_fatal ? 'Yes' : 'No'}
              </DetailItem>
              <DetailItem label="Spool store error">{diagnosticMessageText(host.spool_store_error)}</DetailItem>
            </>
          )}
        </DetailList>
      </div>
    </CardX>
  )
}

function AuditTrailPanel({
  audit,
  agentId,
}: {
  audit: ReturnType<typeof useAdminAgentAudit>
  agentId: string
}) {
  const items = audit.data?.items ?? []
  return (
    <CardX size="medium" className={CARD_SURFACE} id="audit" header={<h3 className="text-sm font-medium">Audit trail</h3>}>
      <p className="text-sm text-muted-foreground">
        Redacted immutable events for this Agent. One-time secrets never appear in Audit.
      </p>
      {!audit.data && audit.isPending && (
        <p className="mt-2 flex flex-wrap items-center gap-2 text-sm text-muted-foreground" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading the Audit trail…
        </p>
      )}
      {!audit.data && audit.isError && (
        <div
          className="mt-2 flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
          role="alert"
        >
          <StatusBadge status="Error" tone="error" />{' '}
          <span className="min-w-0 break-words">
            {audit.error instanceof Error ? audit.error.message : 'Unable to load the Audit trail'}
          </span>
          <Button variant="link" size="sm" onClick={() => void audit.refetch()}>
            Try again
          </Button>
        </div>
      )}
      {audit.data && items.length === 0 && (
        <div className="mt-2">
          <Empty description="No Audit events for this Agent yet." />
        </div>
      )}
      {audit.data && items.length > 0 && (
        <ul className="mt-3 space-y-3">
          {items.map((item) => (
            <AuditItemRow key={item.audit_event_id} item={item} agentId={agentId} />
          ))}
        </ul>
      )}
    </CardX>
  )
}

function AuditItemRow({ item, agentId }: { item: AgentAuditItem; agentId: string }) {
  const detail = item.details
  const summary =
    detail && typeof detail === 'object'
      ? Object.entries(detail as Record<string, unknown>)
          .map(([key, value]) => key + ': ' + (Array.isArray(value) ? value.join(', ') : String(value)))
          .join(' · ')
      : ''
  return (
    <li className="space-y-1 rounded-sm border border-border/60 p-2">
      <div className="flex flex-col gap-0.5">
        <strong className="break-words">{item.event_kind}</strong>
        <small className="text-[11px] text-muted-foreground">
          {formatObservedAt(item.created_at)} · {item.actor_username ?? 'local-cli'} · event #
          {item.audit_event_id}
        </small>
      </div>
      {summary && <p className="text-sm text-muted-foreground break-words">{summary}</p>}
      <AuditContextLink agentId={agentId} />
    </li>
  )
}

/** Redacted Audit link and request context for security mutations
 * (issue #44: every security mutation carries an Audit link). Agent-scoped
 * events link to the live Agent audit trail; enrollment tokens link to the
 * global Audit review surface (PAGE-ACCESS-AUDIT, delivered with the
 * People/roles/sessions slice). */
function AuditContextLink({
  agentId,
  requestId,
}: {
  agentId?: string
  requestId?: string
}) {
  return (
    <p className="flex flex-wrap items-center gap-1 text-[11px] text-muted-foreground">
      Recorded in the redacted Audit trail
      {agentId ? (
        <>
          {' · '}
          <Link
            className="inline-flex min-h-11 min-w-11 items-center font-medium text-foreground underline-offset-4 hover:underline"
            to={'/admin/agents/' + agentId + '#audit'}
          >
            view Agent audit
          </Link>
        </>
      ) : (
        <>
          {' · '}
          <Link
            className="inline-flex min-h-11 min-w-11 items-center font-medium text-foreground underline-offset-4 hover:underline"
            to="/admin/access/audit"
          >
            review the redacted Audit log
          </Link>
        </>
      )}
      {requestId ? ' · request ' + requestId : ''}
    </p>
  )
}

/** PATTERN-SECRET-ONCE: the one-time secret exists only in this component's
 * state and disappears when the view unmounts or refreshes. It is never
 * placed in URL/query state, browser history, logs, or Audit bodies. */
function OneTimeSecret({ secret, label }: { secret: string; label: string }) {
  const [copied, setCopied] = useState(false)
  async function copySecret() {
    try {
      await navigator.clipboard.writeText(secret)
      setCopied(true)
    } catch {
      setCopied(false)
    }
  }
  return (
    <div className="space-y-2 rounded-md border border-warning/40 bg-warning/5 p-3" role="status">
      <p>
        <strong>{label}</strong>
      </p>
      <code className="block break-all rounded-sm bg-muted p-2 text-[11px]">{secret}</code>
      <Button type="button" onClick={() => void copySecret()}>
        Copy secret
      </Button>
      {copied && <p className="text-sm text-success">Copied to clipboard.</p>}
      <p className="text-[11px] text-muted-foreground">
        This secret is shown exactly once and cannot be recovered. It never appears in URLs,
        browser history, logs, or the Audit trail. Copy it now and store it with the Agent
        configuration.
      </p>
    </div>
  )
}

function LifetimeField({
  value,
  onChange,
  id,
  label,
}: {
  value: number
  onChange: (hours: number) => void
  id: string
  label: string
}) {
  return (
    <div className="flex flex-col gap-1">
      <label className="text-xs font-medium tracking-wider text-muted-foreground" htmlFor={id}>{label}</label>
      <Select id={id} value={value} onChange={(event) => onChange(Number(event.target.value))}>
        <option value={1}>1 hour</option>
        <option value={6}>6 hours</option>
        <option value={12}>12 hours</option>
        <option value={24}>24 hours (default)</option>
        <option value={72}>72 hours</option>
        <option value={168}>7 days</option>
      </Select>
      <p className="text-[11px] text-muted-foreground">Single use; expires after the selected window.</p>
    </div>
  )
}

/** SCN-AGENT-ENROLL-GUIDANCE: local, safe upstream instructions. The UI
 * installs nothing and runs no remote command; the secret is consumed from
 * the terminal on the Host, never from process arguments or a URL. */
function EnrollmentGuidance() {
  return (
    <CardX
      size="medium"
      className={CARD_SURFACE}
      header={<h2 className="text-lg font-semibold">Local enrollment instructions</h2>}
    >
      <ol className="list-decimal space-y-2 pl-5 text-sm">
        <li>
          Install the <code>platpulse-agent</code> package on the Host. This UI never installs,
          starts, stops, or upgrades anything remotely.
        </li>
        <li>
          Configure <code>server_url</code>, the credential file, the state database, and the
          Host's Nodes in <code>/var/lib/platpulse-agent/agent.toml</code>.
        </li>
        <li>
          On the Host, run
          <code className="mx-1 break-all">platpulse-agent enroll --config /var/lib/platpulse-agent/agent.toml</code>
          and paste the token at the prompt. The token is read from the terminal, never from
          process arguments.
        </li>
        <li>
          Start the Agent service. Successful enrollment immediately adds the Agent to this
          list with its stable Agent ID; a display name and notes can then be saved on the
          Agent detail page.
        </li>
      </ol>
      <p className="mt-3 text-[11px] text-muted-foreground">
        Generating a token does not create an Agent and leaves no offline placeholder. Only a
        successful enrollment adds the Agent to the list. The token is short-lived, single-use,
        and shown exactly once.
      </p>
    </CardX>
  )
}

/** PAGE-ADMIN-ENROLL: create a one-time Enrollment Token for a new Agent. */
export function AdminAgentEnroll() {
  const { status } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const [expiresInHours, setExpiresInHours] = useState(24)
  const [busy, setBusy] = useState(false)
  const [result, setResult] = useState<Awaited<ReturnType<typeof createEnrollmentToken>> | null>(
    null,
  )
  const [error, setError] = useState<string | null>(null)

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setBusy(true)
    setError(null)
    try {
      setResult(await createEnrollmentToken(csrfToken, expiresInHours))
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : 'Unable to create the enrollment token')
    } finally {
      setBusy(false)
    }
  }

  return (
    <section className="w-full min-w-0 space-y-4">
      <div className="space-y-1">
        <h1 className="text-lg font-semibold break-words">Enroll a new Agent</h1>
        <p className="text-sm text-muted-foreground">
          <Link
            className="inline-flex min-h-11 min-w-11 items-center font-medium underline-offset-4 hover:underline"
            to="/admin/agents"
          >
            All Agents
          </Link>{' '}
          · the Server issues a short-lived, single-use Enrollment Token. The Agent exchanges
          it once for a stable identity and an Agent Credential; the token cannot enroll twice.
        </p>
      </div>
      {result ? (
        <CardX size="medium" className={CARD_SURFACE} header={<h2 className="text-lg font-semibold">Enrollment token created</h2>}>
          <OneTimeSecret secret={result.token} label="One-time Enrollment Token" />
          <p className="mt-3 text-sm text-muted-foreground">
            Token id {result.token_id} · expires {formatObservedAt(result.expires_at)} · shown
            exactly once.
          </p>
          <div className="mt-2">
            <AuditContextLink agentId="" requestId={result.request_id} />
          </div>
          <p className="mt-2 text-sm text-muted-foreground">
            Recorded as a redacted <code>enrollment_token_created</code> Audit event.
          </p>
          <Link className={cn(buttonVariants(), 'mt-3 w-fit')} to="/admin/agents">
            Back to Agents
          </Link>
        </CardX>
      ) : (
        <CardX size="medium" className={CARD_SURFACE} header={<h2 className="text-lg font-semibold">Token lifetime</h2>}>
          <form onSubmit={submit} className="grid max-w-xl gap-3">
            <LifetimeField
              id="enroll-lifetime"
              label="Token lifetime"
              value={expiresInHours}
              onChange={setExpiresInHours}
            />
            <Button type="submit" className="w-fit" disabled={busy}>
              Create enrollment token
            </Button>
            {error && (
              <p className="text-sm text-destructive" role="alert">
                {error}
              </p>
            )}
          </form>
        </CardX>
      )}
      <EnrollmentGuidance />
    </section>
  )
}

/** PAGE-ADMIN-AGENT-RECOVER: one-time Recovery Token for an existing Agent.
 * Exchange advances the Agent Epoch and rotates the credential without a
 * duplicate Agent (design §4.5). */
export function AdminAgentRecover() {
  const { agentId = '' } = useParams()
  const { status, generation } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const agent = useAdminAgentDetail(generation, agentId)
  const [expiresInHours, setExpiresInHours] = useState(24)
  const [confirmed, setConfirmed] = useState(false)
  const [busy, setBusy] = useState(false)
  const [result, setResult] = useState<Awaited<ReturnType<typeof createRecoveryToken>> | null>(
    null,
  )
  const [error, setError] = useState<string | null>(null)

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setBusy(true)
    setError(null)
    try {
      setResult(await createRecoveryToken(agentId, csrfToken, expiresInHours))
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : 'Unable to create the recovery token')
    } finally {
      setBusy(false)
    }
  }

  const notFound =
    agent.isError && agent.error instanceof AdminApiError && agent.error.code === 'agent_not_found'

  if (notFound) {
    return (
      <section className="w-full min-w-0 space-y-4">
        <div className="space-y-1">
          <h1 className="text-lg font-semibold">Agent unavailable</h1>
          <p className="text-sm text-muted-foreground">This Agent is no longer available.</p>
        </div>
      </section>
    )
  }

  return (
    <section className="w-full min-w-0 space-y-4">
      <div className="space-y-1">
        <h1 className="text-lg font-semibold break-words">Recover Agent {agentId ? shortId(agentId) : ''}</h1>
        <p className="text-sm text-muted-foreground">
          <Link
            className="inline-flex min-h-11 min-w-11 items-center font-medium underline-offset-4 hover:underline"
            to={'/admin/agents/' + agentId}
          >
            Back to Agent detail
          </Link>
        </p>
      </div>
      {agent.data && (
        <p className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground">
          Current Agent Epoch:{' '}
          <StatusBadge status={String(agent.data.agent_epoch)} tone="neutral" /> · liveness{' '}
          <StatusBadge status={livenessLabel(agent.data.liveness)} tone="neutral" />
        </p>
      )}
      {!agent.data && agent.isPending && (
        <p className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading Agent state…
        </p>
      )}
      {result ? (
        <CardX size="medium" className={CARD_SURFACE} header={<h2 className="text-lg font-semibold">Recovery token created</h2>}>
          <OneTimeSecret secret={result.token} label="One-time Recovery Token" />
          <p className="mt-3 text-sm text-muted-foreground">
            Token id {result.token_id} · expires {formatObservedAt(result.expires_at)}.
          </p>
          <p className="mt-2 text-sm text-warning">
            When the Agent exchanges this token its Epoch advances from {result.agent_epoch} to{' '}
            {result.agent_epoch + 1}, every existing credential is revoked, and a fresh
            credential is issued — the same Agent identity is preserved, never duplicated.
          </p>
          <div className="mt-2">
            <AuditContextLink agentId={agentId} requestId={result.request_id} />
          </div>
          <Link className={cn(buttonVariants(), 'mt-3 w-fit')} to={'/admin/agents/' + agentId}>
            Back to Agent detail
          </Link>
        </CardX>
      ) : (
        <CardX size="medium" className={CARD_SURFACE} header={<h2 className="text-lg font-semibold">Create a recovery token</h2>}>
          <form onSubmit={submit} className="grid max-w-xl gap-3">
            <div className="text-sm text-warning">
              <strong>What recovery does:</strong> advances the Agent Epoch, revokes every
              existing credential, and issues a fresh credential through the one-time token.
              It does not create a duplicate Agent and cannot be undone. Use it when credentials
              are lost or compromised.
            </div>
            <LifetimeField
              id="recover-lifetime"
              label="Token lifetime"
              value={expiresInHours}
              onChange={setExpiresInHours}
            />
            <div>
              <label className="flex min-h-11 items-center gap-2 text-sm" htmlFor="recover-confirm">
                <Checkbox
                  id="recover-confirm"
                  checked={confirmed}
                  onChange={(event) => setConfirmed(event.target.checked)}
                />
                I understand: recovery advances the Agent Epoch and revokes every existing
                credential; it cannot be undone and never creates a duplicate Agent.
              </label>
            </div>
            <Button type="submit" className="w-fit" disabled={busy || !confirmed}>
              Create recovery token
            </Button>
            {error && (
              <p className="text-sm text-destructive" role="alert">
                {error}
              </p>
            )}
          </form>
        </CardX>
      )}
    </section>
  )
}

/** PAGE-ADMIN-AGENT-ROTATE: credential rotation with an explicit overlap
 * window and optional old-credential revocation (design §12.6). Distinct
 * from recovery: the Agent Epoch is untouched. */
export function AdminAgentRotate() {
  const { agentId = '' } = useParams()
  const { status, generation } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const agent = useAdminAgentDetail(generation, agentId)
  const [overlapHours, setOverlapHours] = useState(24)
  const [revokePrevious, setRevokePrevious] = useState(false)
  const [confirmed, setConfirmed] = useState(false)
  const [busy, setBusy] = useState(false)
  const [result, setResult] = useState<Awaited<ReturnType<typeof rotateAgentCredential>> | null>(
    null,
  )
  const [error, setError] = useState<string | null>(null)

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setBusy(true)
    setError(null)
    try {
      setResult(await rotateAgentCredential(agentId, csrfToken, overlapHours, revokePrevious))
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : 'Unable to rotate the credential')
    } finally {
      setBusy(false)
    }
  }

  const notFound =
    agent.isError && agent.error instanceof AdminApiError && agent.error.code === 'agent_not_found'

  if (notFound) {
    return (
      <section className="w-full min-w-0 space-y-4">
        <div className="space-y-1">
          <h1 className="text-lg font-semibold">Agent unavailable</h1>
          <p className="text-sm text-muted-foreground">This Agent is no longer available.</p>
        </div>
      </section>
    )
  }

  return (
    <section className="w-full min-w-0 space-y-4">
      <div className="space-y-1">
        <h1 className="text-lg font-semibold break-words">Rotate credential {agentId ? 'for Agent ' + shortId(agentId) : ''}</h1>
        <p className="text-sm text-muted-foreground">
          <Link
            className="inline-flex min-h-11 min-w-11 items-center font-medium underline-offset-4 hover:underline"
            to={'/admin/agents/' + agentId}
          >
            Back to Agent detail
          </Link>
        </p>
      </div>
      {result ? (
        <CardX size="medium" className={CARD_SURFACE} header={<h2 className="text-lg font-semibold">Credential rotated</h2>}>
          <OneTimeSecret secret={result.credential} label="New Agent Credential" />
          <p className="mt-3 text-sm text-muted-foreground">
            Credential id {result.credential_id} · overlap {result.overlap_hours} hour
            {result.overlap_hours === 1 ? '' : 's'}
            {result.revoked_previous_ids.length > 0
              ? ' · previous credential(s) revoked immediately'
              : result.overlap_credential_ids.length > 0
                ? ' · previous credential(s) stay valid until ' + formatObservedAt(result.revoke_after)
                : ' · no previous valid credential remained'}
            {result.revoked_previous_ids.length > 0
              ? ' · revoked: ' + result.revoked_previous_ids.join(', ')
              : ''}
            {result.overlap_credential_ids.length > 0
              ? ' · overlap: ' + result.overlap_credential_ids.join(', ')
              : ''}
          </p>
          <p className="mt-2 text-sm text-warning">
            The Agent Epoch was not changed. Install the new credential on the Agent before
            the overlap expires; the previous credential stops working at that instant.
          </p>
          <div className="mt-2">
            <AuditContextLink agentId={agentId} requestId={result.request_id} />
          </div>
          <Link className={cn(buttonVariants(), 'mt-3 w-fit')} to={'/admin/agents/' + agentId}>
            Back to Agent detail
          </Link>
        </CardX>
      ) : (
        <CardX size="medium" className={CARD_SURFACE} header={<h2 className="text-lg font-semibold">Rotate the Agent credential</h2>}>
          <form onSubmit={submit} className="grid max-w-xl gap-3">
            <div className="text-sm text-warning">
              <strong>What rotation does:</strong> issues a fresh credential and keeps the
              previous one valid through an explicit overlap window (or revokes it immediately
              when chosen). The Agent Epoch is untouched and no duplicate Agent is created.
            </div>
            <div className="flex flex-col gap-1">
              <label className="text-xs font-medium tracking-wider text-muted-foreground" htmlFor="rotate-overlap">Overlap window</label>
              <Select
                id="rotate-overlap"
                value={overlapHours}
                onChange={(event) => setOverlapHours(Number(event.target.value))}
              >
                <option value={1}>1 hour</option>
                <option value={6}>6 hours</option>
                <option value={12}>12 hours</option>
                <option value={24}>24 hours (default)</option>
                <option value={72}>72 hours</option>
                <option value={168}>7 days</option>
              </Select>
              <p className="text-[11px] text-muted-foreground">
                The previous credential stays valid for this long after rotation, then stops
                authenticating automatically.
              </p>
            </div>
            <div>
              <label className="flex min-h-11 items-center gap-2 text-sm" htmlFor="rotate-revoke-previous">
                <Checkbox
                  id="rotate-revoke-previous"
                  checked={revokePrevious}
                  onChange={(event) => setRevokePrevious(event.target.checked)}
                />
                Revoke the previous credential immediately
              </label>
              <p className="text-[11px] text-muted-foreground">
                Choose this only when the previous credential is compromised or already
                installed on the replacement configuration. Immediate revocation cannot be
                undone.
              </p>
            </div>
            <div>
              <label className="flex min-h-11 items-center gap-2 text-sm" htmlFor="rotate-confirm">
                <Checkbox
                  id="rotate-confirm"
                  checked={confirmed}
                  onChange={(event) => setConfirmed(event.target.checked)}
                />
                I understand: rotation issues a new credential and the previous one stops
                working at the end of the overlap window (or immediately when revocation is
                selected); the Agent Epoch is untouched.
              </label>
            </div>
            <Button type="submit" className="w-fit" disabled={busy || !confirmed}>
              Rotate credential
            </Button>
            {error && (
              <p className="text-sm text-destructive" role="alert">
                {error}
              </p>
            )}
          </form>
        </CardX>
      )}
    </section>
  )
}
