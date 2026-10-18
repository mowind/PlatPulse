import { ChevronUp } from 'lucide-react'
import { useEffect, useRef, useState, type FormEvent, type KeyboardEvent } from 'react'
import { Link, useParams } from 'react-router'
import {
  AdminApiError,
  acknowledgeAgentAttention,
  createEnrollmentToken,
  createRecoveryToken,
  removeAgent,
  revokeAgentCredential,
  rotateAgentCredential,
  updateAgentMetadata,
  useAdminAgentAudit,
  useAdminAgentDetail,
  useAdminAgentHostMetricHistory,
  useAdminAgentRemovalImpact,
  useAdminDiagnostics,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { HOST_METRIC_SERIES } from '../metricHistory'
import {
  HostMountPathControl,
  MetricHistoryBody,
  useHostMetricSelection,
  useMetricHistoryWindow,
  type HostMetricSelection,
  type MetricHistoryWindow,
} from './metricHistoryPanel'
import { StorageMountsPanel } from './StorageMountsPanel'
import {
  formatBytesUnknown,
  formatBytesPerSecond,
  formatPercent,
} from '../formatBytes'
import { livenessTone, receiptTimeText } from '../agentDiagnostics'
import {
  agentEvidence,
  diagnosticFindings,
  diagnosticRangeText,
  hasSpoolObservation,
  type EvidenceItem,
  type EvidenceTone,
} from '../agentEvidence'
import { InfoHint } from '../components/InfoHint'
import {
  AdminDetailSections,
  DETAIL_LINK,
  DetailBackLink,
  DetailDimension,
  DetailDimensions,
  DetailDraftProvider,
  DetailNotices,
  draftMarker,
  useDetailDrafts,
  useDetailSection,
  useReportDraft,
  type DetailNotice,
  type DetailSection,
} from '../components/AdminDetailSections'
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
import type {
  AdminAgentRemovalImpact,
  AgentAuditItem,
  AgentCredentialSummary,
  AgentDiagnostic,
  AgentRemovalResponse,
  HostDiagnostic,
} from '../api/generated'
import { AdminIdentifier } from '../components/AdminIdentifier'
import { DetailItem, DetailList } from '../components/DetailList'
import {
  ADMIN_DETAIL_BODY,
  ADMIN_DETAIL_ROW,
  ADMIN_PAGE_TITLE,
  ADMIN_PANEL_SURFACE,
  ADMIN_ROW,
  ADMIN_SECONDARY,
  ADMIN_SECTION_TITLE,
  ADMIN_TD,
  ADMIN_TH,
  AdminPageHeader,
  AdminPanel,
} from '../components/AdminPage'

/**
 * PAGE-ADMIN-AGENTS, PAGE-ADMIN-AGENT-DETAIL, PAGE-ADMIN-ENROLL,
 * PAGE-ADMIN-AGENT-RECOVER, and PAGE-ADMIN-AGENT-ROTATE (design §4.3, §8.2):
 * Owner-only Agent lifecycle operations. Identity, liveness, boot/report
 * state, Inventory, credential state, and diagnostics are separate
 * dimensions (design §14.3). One-time secrets follow PATTERN-SECRET-ONCE:
 * they appear only in the success response, live only in component state,
 * and are gone when the view is left — never in URLs, history, or Audit.
 */

const CARD_SURFACE = ADMIN_PANEL_SURFACE

function shortId(id: string): string {
  return id.length > 14 ? id.slice(0, 8) + '…' + id.slice(-4) : id
}

/** A `0x` + 64 hex digit Inventory hash is unreadable inline; the head and tail
 * are enough to compare two hashes by eye, and the full value stays available
 * on hover. */
function shortHash(hash: string): string {
  return hash.length > 14 ? hash.slice(0, 10) + '…' + hash.slice(-4) : hash
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

function AgentIdentityAccess({
  agentId,
  displayName,
}: {
  agentId: string
  displayName?: string | null
}) {
  const link = (
    <Link
      className="inline-flex min-h-11 min-w-0 max-w-full items-center py-1 leading-snug break-words font-medium underline-offset-4 hover:underline"
      to={'/admin/agents/' + encodeURIComponent(agentId)}
    >
      {displayName ?? shortId(agentId)}
    </Link>
  )
  // A named Agent keeps its name on its own line and the short ID beside the
  // full-ID toggle; an unnamed Agent's link already is the short ID, so the
  // toggle shares that line instead of adding one.
  return (
    <AdminIdentifier
      value={agentId}
      label="Agent ID"
      fullSlot="agent-full-id"
      heading={displayName ? link : undefined}
      lead={
        displayName ? (
          <small
            aria-hidden="true"
            title={agentId}
            className="min-w-0 font-mono text-xs text-muted-foreground [overflow-wrap:anywhere]"
          >
            {shortId(agentId)}
          </small>
        ) : (
          link
        )
      }
    />
  )
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
      <AdminPageHeader
        title="Agents"
        description="Reporting status, receipt time, Node inventory, credentials, and diagnostics are separate dimensions."
        info={{
          label: 'About Agent dimensions',
          content: (
            <p>
              Server reporting status, receipt time, retained Node inventory, credential validity,
              and diagnostics stay separate dimensions. Detailed boot/report state remains on each
              Agent detail page; Agent Offline is not Node Retired.
            </p>
          ),
        }}
        actions={
          <Link className={cn(buttonVariants(), 'shrink-0')} to="/admin/agents/enroll">
            Add Agent
          </Link>
        }
      />
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
        <AdminPanel
          slot="agent-inventory"
          flush
          title="Agent inventory"
          count={
            <span className="text-xs text-muted-foreground tabular-nums">
              {agents.length} Agent{agents.length === 1 ? '' : 's'}
            </span>
          }
        >
          <div className="overflow-x-auto max-md:px-3 max-md:pt-3">
            <table data-stack data-slot="agent-table" className="w-full text-sm">
              <caption className="sr-only">
                Agent, Server reporting status, Server receipt time, retained Node count, and
                current attention apart from recorded diagnostic evidence
              </caption>
              <thead>
                <tr className="border-b border-border/60">
                  <th scope="col" className={ADMIN_TH}>Agent</th>
                  <th scope="col" className={ADMIN_TH}>
                    <span className="inline-flex items-center gap-1">
                      Reporting
                      <InfoHint label="About Reporting">
                        <p>Server liveness from accepted Agent Reports. It is not Node health, clock reliability, or credential validity.</p>
                      </InfoHint>
                    </span>
                  </th>
                  <th scope="col" className={ADMIN_TH}>
                    <span className="inline-flex items-center gap-1">
                      Last received
                      <InfoHint label="About Last received">
                        <p>Server receipt time of the latest accepted Agent Report, not the report sequence or an Agent clock.</p>
                      </InfoHint>
                    </span>
                  </th>
                  <th scope="col" className={ADMIN_TH}>
                    <span className="inline-flex items-center gap-1">
                      Nodes
                      <InfoHint label="About Nodes">
                        <p>Retained Nodes assigned to the Agent: Active and Retired together. Row details split them.</p>
                      </InfoHint>
                    </span>
                  </th>
                  <th scope="col" className={ADMIN_TH}>
                    <span className="inline-flex items-center gap-1">
                      Diagnostics
                      <InfoHint label="About Diagnostics">
                        <p>Current: the Server's own Attention Items and credential state. Recorded: accumulated or retained history, which is not a current fault. Not observed: evidence the Server never received, never zero.</p>
                      </InfoHint>
                    </span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {agents.map((agent) => (
                  <AgentListRow key={agent.agent_id} agent={agent} />
                ))}
              </tbody>
            </table>
          </div>
        </AdminPanel>
      )}
    </section>
  )
}

const EVIDENCE_TONE: Record<EvidenceTone, string> = {
  error: 'font-medium text-destructive',
  warning: 'text-foreground',
  neutral: 'text-muted-foreground',
}

/** PAGE-ADMIN-AGENTS row: an inventory summary. The Diagnostics cell names the
 * first current or unobserved item and the first recorded item, then counts
 * the rest; the row's details show every item, the credential counts, the
 * Active/Retired split and the complete recorded findings. Known-zero
 * counters take no space on the row. */
function AgentListRow({ agent }: { agent: AgentDiagnostic }) {
  const liveness = livenessLabel(agent.liveness)
  const [open, setOpen] = useState(false)
  const toggleRef = useRef<HTMLButtonElement>(null)
  const detailId = 'agent-diagnostics-' + agent.agent_id
  const evidence = agentEvidence(agent)
  const attention = [...evidence.current, ...evidence.unknown]
  const shown = [attention[0], evidence.recorded[0]].filter((item): item is EvidenceItem => Boolean(item))
  // Current and recorded remainders are counted apart, so another current
  // item never reads as more history.
  const moreCurrent = Math.max(0, attention.length - 1)
  const moreRecorded = Math.max(0, evidence.recorded.length - 1)
  const more = [
    moreCurrent > 0 ? '+' + moreCurrent + ' current' : null,
    moreRecorded > 0 ? '+' + moreRecorded + ' recorded' : null,
  ].filter(Boolean).join(' · ')
  const name = agent.display_name ?? shortId(agent.agent_id)
  const collapseOnEscape = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key !== 'Escape' || !open) return
    setOpen(false)
    toggleRef.current?.focus()
  }
  return (
    <>
      <tr className={ADMIN_ROW}>
        <th scope="row" data-label="Agent" className={cn(ADMIN_TD, 'min-w-0 py-1 text-left font-normal')}>
          <AgentIdentityAccess agentId={agent.agent_id} displayName={agent.display_name} />
        </th>
        <td data-label="Reporting" className={cn(ADMIN_TD, 'min-w-0')}>
          <StatusBadge status={liveness} tone={livenessTone(agent.liveness)} />
        </td>
        <td data-label="Last received" className={cn(ADMIN_TD, 'min-w-0')}>
          <span className="tabular-nums">{receiptTimeText(agent.last_received_at)}</span>
        </td>
        <td data-label="Nodes" className={cn(ADMIN_TD, 'min-w-0')}>
          <span className="tabular-nums">{agent.nodes.length} retained</span>
        </td>
        <td data-label="Diagnostics" className={cn(ADMIN_TD, 'min-w-0')}>
          <div className="flex min-w-0 flex-col items-start gap-0.5" data-slot="agent-evidence-summary">
            {shown.length === 0 && <span className="text-muted-foreground">Nothing recorded</span>}
            {attention[0] && (
              <span className={cn('[overflow-wrap:anywhere]', EVIDENCE_TONE[attention[0].tone])} data-group="current">
                {attention[0].text}
              </span>
            )}
            {evidence.recorded[0] && (
              <span className={cn('text-xs [overflow-wrap:anywhere]', EVIDENCE_TONE[evidence.recorded[0].tone])} data-group="recorded">
                <span className="text-muted-foreground">Recorded · </span>
                {evidence.recorded[0].short ?? evidence.recorded[0].text}
              </span>
            )}
            <Button
              ref={toggleRef}
              variant="link"
              size="xs"
              className="min-h-11 px-0 text-xs md:-my-2"
              aria-expanded={open}
              aria-controls={detailId}
              onClick={() => setOpen((value) => !value)}
              onKeyDown={collapseOnEscape}
            >
              {open ? 'Hide details' : more || 'Details'}{' '}
              <span className="sr-only">for {name}</span>
            </Button>
          </div>
        </td>
      </tr>
      {open && (
        <tr data-slot="detail-row" className={ADMIN_DETAIL_ROW}>
          <td colSpan={5} id={detailId} onKeyDown={collapseOnEscape} className="p-0">
            <AgentRowDetails agent={agent} evidence={evidence} onCollapse={() => setOpen(false)} />
          </td>
        </tr>
      )}
    </>
  )
}

const GROUP_TITLE = 'text-xs font-semibold text-foreground'

function AgentRowDetails({
  agent,
  evidence,
  onCollapse,
}: {
  agent: AgentDiagnostic
  evidence: ReturnType<typeof agentEvidence>
  onCollapse: () => void
}) {
  const active = agent.nodes.filter((node) => node.lifecycle === 'active').length
  const retired = agent.nodes.filter((node) => node.lifecycle === 'retired').length
  const other = agent.nodes.length - active - retired
  const attention = [...evidence.current, ...evidence.unknown]
  return (
    <div className={ADMIN_DETAIL_BODY}>
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className={ADMIN_SECTION_TITLE}>Agent details</h3>
        <Button variant="link" size="sm" className="-my-1.5" onClick={onCollapse}>
          Collapse details <ChevronUp size={16} aria-hidden="true" />
        </Button>
      </div>
      <div className="grid min-w-0 gap-x-8 gap-y-4 sm:grid-cols-2 xl:grid-cols-3">
        <section aria-label="Current and not observed" className="min-w-0 space-y-1.5">
          <h4 className={GROUP_TITLE}>Current and not observed</h4>
          {attention.length === 0 ? (
            <p className={ADMIN_SECONDARY}>No current Server attention item; Host and spool observed.</p>
          ) : (
            <ul className="space-y-1">
              {attention.map((item) => (
                <li key={item.key} className={cn('[overflow-wrap:anywhere]', EVIDENCE_TONE[item.tone])}>{item.text}</li>
              ))}
            </ul>
          )}
        </section>
        <section aria-label="Nodes and credentials" className="min-w-0 space-y-1.5">
          <h4 className={GROUP_TITLE}>Nodes</h4>
          <p className="tabular-nums">
            {agent.nodes.length} retained · {active} Active · {retired} Retired
            {other > 0 ? ' · ' + other + ' Unknown lifecycle' : ''}
          </p>
          <h4 className={cn(GROUP_TITLE, 'pt-1')}>Credentials</h4>
          <p className="tabular-nums">{credentialSummaryText(agent.credentials)}</p>
          <p className={ADMIN_SECONDARY}>
            Server validity only; reporting status does not imply a valid credential.{' '}
            <Link
              className="inline-flex min-h-11 items-center font-medium text-foreground underline-offset-4 hover:underline"
              to={'/admin/agents/' + encodeURIComponent(agent.agent_id) + '?section=manage#credentials'}
            >
              Manage credentials
            </Link>
          </p>
        </section>
        <section aria-label="Recorded evidence" className="min-w-0 space-y-1.5 sm:col-span-2 xl:col-span-1">
          <h4 className={GROUP_TITLE}>Recorded evidence</h4>
          <ul
            className="grid grid-cols-1 gap-y-1"
            aria-label="Recorded diagnostic evidence"
          >
            <li className="flex min-w-0 flex-col">
              <span className={cn('font-medium', ADMIN_SECONDARY)}>Recorded gap intervals</span>
              <span className="tabular-nums">{agent.sequence_gap_count}</span>
            </li>
            <li className="flex min-w-0 flex-col">
              <span className={cn('font-medium', ADMIN_SECONDARY)}>Accumulated recorded security events</span>
              <span className="tabular-nums">{agent.security_event_count}</span>
            </li>
            {diagnosticFindings(agent.host).map((finding) => (
              <li key={finding.key} className="flex min-w-0 flex-col">
                <span className={cn('font-medium', ADMIN_SECONDARY)}>{finding.label}</span>
                <span className="min-w-0 [overflow-wrap:anywhere]">{finding.value}</span>
              </li>
            ))}
          </ul>
          <p className={ADMIN_SECONDARY}>
            Recorded evidence; Server liveness remains separate.
            {agent.host ? ' Host snapshot: ' + formatObservedAt(agent.host.updated_at) : ' No Host snapshot.'}
          </p>
        </section>
      </div>
    </div>
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
  const [section, setSection, openSection] = useDetailSection(AGENT_DETAIL_ANCHORS)
  const drafts = useDetailDrafts()
  const notices = agent.data ? agentDetailNotices(agent.data) : []
  // The tab counts current and unobserved findings only; recorded history is
  // in the section but is not a current fault.
  const diagnosticCount = notices.filter(
    (notice) => notice.section === 'diagnostics' && notice.status !== 'Recorded history',
  ).length

  if (removed) {
    return (
      <section className="w-full min-w-0 space-y-4">
        <div className="space-y-1">
          <h1 className={ADMIN_PAGE_TITLE}>Agent removed</h1>
          <p className="text-sm text-muted-foreground">
            Agent {shortId(agentId)} was permanently removed at{' '}
            {formatObservedAt(removed.deleted_at)}.
          </p>
        </div>
        <CardX
          size="medium"
          className={CARD_SURFACE}
          header={<h2 className={ADMIN_SECTION_TITLE}>Removal result</h2>}
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
          <h1 className={ADMIN_PAGE_TITLE}>Agent unavailable</h1>
          <p className="text-sm text-muted-foreground">This Agent is no longer available.</p>
        </div>
      </section>
    )
  }
  const data = agent.data
  return (
    <section className="w-full min-w-0 space-y-4" data-slot="admin-agent-detail">
      {data ? (
        <AgentDetailHeader
          agent={data}
          csrfToken={csrfToken}
          onAcknowledged={() => void agent.refetch()}
          notices={notices}
          section={section}
          onSection={openSection}
        />
      ) : (
        <div className="space-y-1">
          <DetailBackLink to="/admin/agents">All Agents</DetailBackLink>
          <h1 className={ADMIN_PAGE_TITLE}>{'Agent ' + shortId(agentId)}</h1>
        </div>
      )}
      {!data && (
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
      {data && (
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
          <DetailDraftProvider value={drafts.report}>
            <AdminDetailSections
              label="Agent detail sections"
              value={section}
              onValueChange={setSection}
              sections={[
                {
                  key: 'overview',
                  label: 'Overview',
                  content: (
                    <>
                      <AgentRuntimeSummaryPanel agent={data} onSection={openSection} />
                      <InventoryPanel agent={data} />
                    </>
                  ),
                },
                {
                  key: 'history',
                  label: 'History',
                  content: (
                    <>
                      {/* The shared Host series and its mount paths, kept as one
                          named region apart from the Audit trail. */}
                      <section aria-label="Host resource history" className="grid min-w-0 grid-cols-[minmax(0,1fr)] gap-4">
                        <AgentHostHistorySection agent={data} />
                      </section>
                      <AuditTrailPanel audit={audit} agentId={agentId} />
                    </>
                  ),
                },
                {
                  key: 'diagnostics',
                  label: 'Diagnostics',
                  marker: diagnosticCount > 0 ? diagnosticCount : undefined,
                  markerLabel:
                    diagnosticCount > 0
                      ? diagnosticCount + ' recorded or unobserved item' + (diagnosticCount === 1 ? '' : 's')
                      : undefined,
                  content: (
                    <>
                      <div className="grid grid-cols-[minmax(0,1fr)] gap-4 lg:grid-cols-[repeat(2,minmax(0,1fr))]">
                        <IdentityPanel agent={data} />
                        <BootReportPanel agent={data} />
                      </div>
                      <DiagnosticsPanel agent={data} />
                    </>
                  ),
                },
                {
                  key: 'manage',
                  label: 'Manage',
                  ...draftMarker(drafts.pending),
                  content: (
                    <>
                      <MetadataPanel agent={data} onSaved={() => void agent.refetch()} />
                      <CredentialsPanel agent={data} onConflictReload={() => void agent.refetch()} />
                      <AgentRemovalPanel
                        agentId={data.agent_id}
                        displayName={data.display_name ?? 'Agent ' + shortId(data.agent_id)}
                        csrfToken={csrfToken}
                        onRemoved={setRemoved}
                      />
                    </>
                  ),
                },
              ]}
            />
          </DetailDraftProvider>
        </>
      )}
    </section>
  )
}

/** Older in-page links (`#audit`, `#credentials`, `#removal`) open the
 *  section that now holds their target. */
const AGENT_DETAIL_ANCHORS = {
  audit: 'history',
  credentials: 'manage',
  removal: 'manage',
} as const

/** Current, unobserved and recorded findings shown above every section,
 *  each with the section that holds its complete evidence. The Server's own
 *  Attention Items have their own list with acknowledgment. */
function agentDetailNotices(agent: AgentDiagnostic): DetailNotice[] {
  const notices: DetailNotice[] = []
  for (const item of agentEvidence(agent).current) {
    if (item.key === 'credentials') {
      notices.push({ key: item.key, tone: 'warning', status: 'Current state', message: item.text + '.', section: 'manage', sectionLabel: 'Manage' })
    } else if (item.key === 'store-fatal') {
      notices.push({ key: item.key, tone: 'error', status: 'Current state', message: item.text + '.', section: 'diagnostics', sectionLabel: 'Diagnostics' })
    }
  }
  agentSummaryWarnings(agent).forEach((warning, index) => {
    notices.push({
      key: warning.kind + '-' + index,
      tone: warning.kind === 'current' ? 'warning' : 'neutral',
      status: warning.kind === 'current' ? 'Current state' : warning.kind === 'recorded' ? 'Recorded history' : 'Unknown',
      message: warning.message,
      section: 'diagnostics',
      sectionLabel: 'Diagnostics',
    })
  })
  return notices
}

type AgentSummaryWarning = {
  kind: 'current' | 'recorded' | 'unknown'
  message: string
}

/** The always-visible Agent header: identity, the independent Server-owned
 *  dimensions, the Server's own Attention Items with their acknowledgment
 *  (issue #172), and the structural warnings. The displayed boundaries come
 *  from the Server, so an acknowledged occurrence stays suppressed while raw
 *  evidence remains in Diagnostics; a stale boundary is reported as a conflict
 *  and the authoritative list is refetched instead of hiding a newer one. */
function AgentDetailHeader({
  agent,
  csrfToken,
  onAcknowledged,
  notices,
  section,
  onSection,
}: {
  agent: AgentDiagnostic
  csrfToken: string
  onAcknowledged: () => void
  notices: DetailNotice[]
  section: DetailSection
  onSection: (next: DetailSection) => void
}) {
  const liveness = livenessLabel(agent.liveness)
  const activeCredentials = agent.credentials.filter((credential) => credential.active).length
  const bootStatus = agent.boot_status && agent.boot_status !== 'unknown' ? agent.boot_status : 'Unknown'
  const [pending, setPending] = useState<string | null>(null)
  const [feedback, setFeedback] = useState<{ tone: 'error' | 'conflict'; message: string } | null>(null)
  const acknowledge = async (items: AgentDiagnostic['attention']) => {
    if (items.length === 0) return
    setPending(items.length === 1 ? items[0].id : '__bulk__')
    setFeedback(null)
    try {
      const result = await acknowledgeAgentAttention(
        agent.agent_id,
        items.map((item) => ({ kind: item.kind, evidence_key: item.evidence_key })),
        csrfToken,
      )
      if (result.skipped.length > 0) {
        setFeedback({
          tone: 'conflict',
          message:
            'Some items changed while you were reviewing them. The authoritative attention list was refreshed; review the current evidence before acknowledging again.',
        })
      }
      onAcknowledged()
    } catch (error) {
      setFeedback({
        tone: 'error',
        message: error instanceof Error ? error.message : 'Unable to acknowledge the attention items',
      })
    } finally {
      setPending(null)
    }
  }
  return (
    <header className="grid min-w-0 gap-3" data-slot="agent-detail-header">
      <DetailBackLink to="/admin/agents">All Agents</DetailBackLink>
      <div className="flex min-w-0 flex-wrap items-start justify-between gap-x-4 gap-y-2">
        <div className="min-w-0 flex-[1_1_18rem] space-y-1">
          <h1 className={ADMIN_PAGE_TITLE}>{agent.display_name ?? 'Agent ' + shortId(agent.agent_id)}</h1>
          <AdminIdentifier value={agent.agent_id} label="Agent ID" fullSlot="agent-full-id" />
        </div>
        <nav aria-label="Agent links" className="flex flex-wrap items-center gap-x-3 text-sm">
          <Link
            className={DETAIL_LINK}
            to={`/admin/alerts/incidents?subject=agent&subject_key=${encodeURIComponent(agent.agent_id)}`}
          >
            Incidents for this Agent
          </Link>
        </nav>
      </div>
      <DetailDimensions label="Current Agent dimensions">
        <DetailDimension label="Server liveness">
          <StatusBadge status={liveness} tone={livenessTone(agent.liveness)} />
        </DetailDimension>
        <DetailDimension label="Last received">{receiptTimeText(agent.last_received_at)}</DetailDimension>
        <DetailDimension label="Boot status">{bootStatus}</DetailDimension>
        <DetailDimension label="Nodes">
          <span className="tabular-nums">{agent.nodes.length} retained</span>
        </DetailDimension>
        <DetailDimension label="Active credentials">
          <span className="tabular-nums">{activeCredentials} active · {agent.credentials.length} total</span>
        </DetailDimension>
      </DetailDimensions>
      {agent.attention.length > 0 && (
        <div className="space-y-2" role="note" aria-label="Agent attention items">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <h2 className={ADMIN_SECTION_TITLE}>Attention</h2>
            <Button
              variant="outline"
              size="sm"
              className="min-h-11"
              disabled={pending !== null}
              onClick={() => void acknowledge(agent.attention)}
            >
              {pending === '__bulk__' ? 'Acknowledging…' : 'Acknowledge current Agent items'}
            </Button>
          </div>
          <ul className="space-y-1">
            {agent.attention.map((item) => (
              <li
                key={item.id}
                data-kind={item.kind}
                className="flex flex-wrap items-center gap-2 text-sm"
              >
                <StatusBadge
                  status={item.severity === 'critical' ? 'Critical' : 'Warning'}
                  tone={item.severity === 'critical' ? 'error' : 'warning'}
                />
                <span className="min-w-0 break-words">{item.message}</span>
                <Button
                  variant="outline"
                  size="sm"
                  className="min-h-11 shrink-0"
                  disabled={pending !== null}
                  aria-label={`Acknowledge ${item.kind}`}
                  onClick={() => void acknowledge([item])}
                >
                  {pending === item.id ? 'Acknowledging…' : 'Acknowledge'}
                </Button>
              </li>
            ))}
          </ul>
        </div>
      )}
      {feedback && (
        <div
          role={feedback.tone === 'error' ? 'alert' : 'status'}
          className={cn(
            'rounded-md border p-3 text-sm',
            feedback.tone === 'error'
              ? 'border-destructive/40 bg-destructive/5'
              : 'border-warning/40 bg-warning/5',
          )}
        >
          {feedback.message}
        </div>
      )}
      <DetailNotices
        label="Important Agent warnings"
        notices={notices}
        current={section}
        onOpen={onSection}
      />
    </header>
  )
}

/** Overview: reporting and the necessary running summary. Boot IDs, the
 *  complete Spool, clock and component evidence stay in Diagnostics. */
function AgentRuntimeSummaryPanel({
  agent,
  onSection,
}: {
  agent: AgentDiagnostic
  onSection: (next: DetailSection) => void
}) {
  const host = agent.host
  const evidence = agentEvidence(agent)
  return (
    <CardX size="medium" className={CARD_SURFACE} header={<h2 className={ADMIN_SECTION_TITLE}>Reporting and runtime</h2>}>
      {/* Liveness, receipt time and boot status are in the header above. */}
      <DetailList>
        <DetailItem label="Report sequence">
          {agent.last_report_sequence == null ? 'Never received' : '#' + agent.last_report_sequence}
        </DetailItem>
        <DetailItem label="Host snapshot">
          {host ? formatObservedAt(host.updated_at) : 'No Host observation yet'}
        </DetailItem>
        <DetailItem label="Host CPU / memory">
          {host
            ? formatPercent(host.cpu_percent) + ' · ' + formatBytesUnknown(host.memory_used_bytes) + ' / ' + formatBytesUnknown(host.memory_total_bytes)
            : 'Unknown'}
        </DetailItem>
        <DetailItem label="Credentials">
          {credentialSummaryText(agent.credentials)}{' '}
          <Button variant="link" size="sm" className="h-auto min-h-11 px-0" onClick={() => onSection('manage')}>
            Manage credentials
          </Button>
        </DetailItem>
        <DetailItem label="Recorded evidence">
          {evidence.recorded.length === 0
            ? 'Nothing recorded'
            : evidence.recorded.map((item) => item.text).join(' · ')}{' '}
          <Button variant="link" size="sm" className="h-auto min-h-11 px-0" onClick={() => onSection('diagnostics')}>
            Open Diagnostics
          </Button>
        </DetailItem>
      </DetailList>
    </CardX>
  )
}

/** Non-attention structural warnings only. Predicates the Server owns as
 * acknowledgeable Attention Items (offline, report gaps, security events,
 * incomplete shutdown, spool fatal/overflow) are never rebuilt here from
 * cumulative counters, and recorded spool/component evidence stays in
 * Diagnostics, so an acknowledged occurrence cannot be re-promoted as a
 * fresh prompt (issue #172, webui.md §15.3). */
function agentSummaryWarnings(agent: AgentDiagnostic): AgentSummaryWarning[] {
  const warnings: AgentSummaryWarning[] = []
  if (agent.liveness !== 'online' && agent.liveness !== 'offline') {
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
  if (!agent.host) {
    warnings.push({ kind: 'unknown', message: 'Host observation is not available yet; absent diagnostic values remain Unknown.' })
  }
  return warnings
}

function IdentityPanel({ agent }: { agent: AgentDiagnostic }) {
  return (
    <CardX size="medium" className={CARD_SURFACE} header={<h2 className={ADMIN_SECTION_TITLE}>Identity</h2>}>
      <DetailList>
        <DetailItem label="Agent ID">
          <code className="break-all text-xs">{agent.agent_id}</code>
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
  useReportDraft(
    'agent-metadata',
    displayName !== (agent.display_name ?? '') || notes !== (agent.notes ?? ''),
  )

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
      header={<h2 className={ADMIN_SECTION_TITLE}>Display name and notes</h2>}
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
          <p className="text-xs text-muted-foreground">
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
          <p className="text-xs text-muted-foreground">Optional, at most 2000 characters.</p>
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
        <p className="text-xs text-muted-foreground">
          Saved through the Owner-only, CSRF-guarded Admin route and recorded in the Audit
          trail as <code>agent_metadata_changed</code>. The Agent ID,
          Host identity, Server liveness, Agent Epoch, and collection configuration are
          read-only and never editable here.
        </p>
      </form>
    </CardX>
  )
}

function BootReportPanel({ agent }: { agent: AgentDiagnostic }) {
  return (
    <CardX size="medium" className={CARD_SURFACE} header={<h2 className={ADMIN_SECTION_TITLE}>Boot and report state</h2>}>
      <DetailList>
        <DetailItem label="Boot status">{agent.boot_status}</DetailItem>
        <DetailItem label="Full active boot ID">
          <code className="break-all text-xs">{agent.active_boot_id ?? 'Unknown'}</code>
        </DetailItem>
        <DetailItem label="Previous boot ID">
          <code className="break-all text-xs">{agent.previous_boot_id ?? 'None'}</code>
        </DetailItem>
        <DetailItem label="Close report ID">
          <code className="break-all text-xs">{agent.close_report_id ?? 'None'}</code>
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
          <code className="break-all text-xs">{agent.shutdown_report_id ?? 'None'}</code>
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

/** The Server-owned label for how the accepted Inventory revision was
 * produced. v2 (Server-managed) declarations carry no Agent revision, so the
 * page must never present one as if the Agent had supplied it (issue #188). */
function revisionSourceLabel(declaration: string): string {
  return declaration === 'server_managed' ? 'Server-assigned revision' : 'Accepted revision'
}

function acceptedHashLabel(declaration: string): string {
  return declaration === 'server_managed'
    ? 'Accepted declaration fingerprint'
    : 'Accepted Inventory content hash'
}

/** Inventory dimension: the Agent-wide Node Inventory declaration the Server
 * currently accepts, and the Nodes it declares. The accepted revision and
 * fingerprint are shown next to the Nodes they describe instead of being
 * inferred from the per-Node revisions; an Agent whose declaration was never
 * accepted stays explicitly Unknown rather than showing revision 0 (issue #188). */
function InventoryPanel({ agent }: { agent: AgentDiagnostic }) {
  const nodes = agent.nodes
  const { accepted_revision, accepted_sha256, accepted_declaration } = agent.inventory
  return (
    <CardX size="medium" className={CARD_SURFACE} header={<h2 className={ADMIN_SECTION_TITLE}>Inventory</h2>}>
      <DetailList>
        <DetailItem label="Accepted declaration">
          {accepted_revision == null ? (
            <span>Unknown · no declaration accepted yet</span>
          ) : (
            <>
              {revisionSourceLabel(accepted_declaration)} {accepted_revision}{' '}
              <small
                className="text-xs text-muted-foreground"
                title={accepted_sha256 ? acceptedHashLabel(accepted_declaration) + ': ' + accepted_sha256 : undefined}
              >
                <span aria-hidden={accepted_sha256 ? true : undefined}>
                  {accepted_sha256 ? shortHash(accepted_sha256) : 'content hash not recorded'}
                </span>
                {accepted_sha256 && (
                  <span className="sr-only">{acceptedHashLabel(accepted_declaration) + ': ' + accepted_sha256}</span>
                )}
              </small>
            </>
          )}
        </DetailItem>
      </DetailList>
      {nodes.length === 0 && (
        <div className="mt-3">
          <Empty description="No PlatON Nodes declared by this Agent yet." />
        </div>
      )}
      {nodes.length > 0 && (
        <ul className="mt-3 space-y-2">
          {nodes.map((node) => (
            <li key={node.node_id} className="flex flex-wrap items-center gap-3 text-sm">
              <span className="min-w-0">
                <strong className="break-words">{node.display_name ?? node.node_id}</strong>{' '}
                <small className="text-xs text-muted-foreground" title={'Full Node ID: ' + node.node_id}>
                  <span aria-hidden="true">{shortId(node.node_id)}</span>
                  <span className="sr-only">Full Node ID: {node.node_id}</span>
                </small>
              </span>
              <span className="text-xs text-muted-foreground">{node.network_key}</span>
              <span className="text-xs">{node.lifecycle}</span>
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
  useReportDraft('credential-revoke', confirmingId !== null)
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
      header={<h2 className={ADMIN_SECTION_TITLE}>Credential records</h2>}
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
                  <span className="break-all text-xs">{credential.credential_id}</span>
                  <StatusBadge status={state.label} tone={state.tone} />
                  {credential.revoke_after && credential.active && (
                    <small className="text-xs text-muted-foreground">
                      Overlap expires {formatObservedAt(credential.revoke_after)}
                    </small>
                  )}
                </div>
                <small className="block text-xs text-muted-foreground">
                  Issued {formatObservedAt(credential.created_at)}
                  {credential.revoked_at ? ' · revoked ' + formatObservedAt(credential.revoked_at) : ''}
                </small>
                {credential.active && (
                  <div className="flex flex-wrap items-center gap-2">
                    {confirmingId === credential.credential_id ? (
                      <>
                        <span className="text-xs text-muted-foreground">
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
  useReportDraft('agent-removal', confirming)

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
      className={cn(CARD_SURFACE, 'ring-1 ring-destructive/25')}
      id="removal"
      data-slot="danger-zone"
      header={<h2 className={ADMIN_SECTION_TITLE}>Danger zone</h2>}
    >
      <h3 className="text-sm font-medium">Permanent Agent removal</h3>
      <p className="mt-1 text-sm text-muted-foreground">
        Irreversible Owner disposition, separate from revoking one credential.
      </p>
      <p className="mt-2 text-sm text-muted-foreground">
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
                      <span className="text-xs">
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

/**
 * The Host series of this Agent (issue #215, design §11.5): the machine's CPU, physical memory,
 * load averages, network rate and per-mount storage. The Agent observes its Host once and the
 * Server stores that observation once, so every Node this Agent reports reads the same series
 * instead of a copy of it, and the Agent is named on the card so a shared reading is never
 * presented as one Node's own. A storage series exists once per mount path, so the Operator names
 * the path the Agent reported and a changed path is a different series rather than a continued one.
 */
const AGENT_HOST_INTRO =
  "Stored observations of the machine this Agent observes once for every Node it serves. The series belongs to the Agent, not to a Node: the same evidence is read here and on the page of each Node the Agent reports, and no single Node's Purge removes it."

/**
 * The Agent's Host history and the mount paths it is read by (issues #215 and
 * #216, design §11.6). The selection lives here rather than inside one
 * card, so choosing a mount path from the list reads that exact path's series on
 * the chart instead of asking the Operator to type a path that the list already
 * knows. A storage series is named by the mount path and nothing else: the list
 * is evidence, never a device claim, and the chart never continues a series
 * across a path change.
 */
function AgentHostHistorySection({ agent }: { agent: AgentDiagnostic }) {
  const history = useMetricHistoryWindow(24)
  const selection = useHostMetricSelection(history)
  const readSeries = (mountPath: string) => {
    selection.onMountPath(mountPath)
    // A mount path names a storage series and nothing else, so picking one from
    // the list also selects the series it names: the chart switches to that
    // path's used bytes, and the capacity side of the same path is one metric
    // choice away. A storage metric already selected keeps its side.
    if (!selection.needsMount) selection.onMetric('disk_used_bytes')
  }
  return (
    <>
      <StorageMountsPanel agentId={agent.agent_id} onReadSeries={readSeries} />
      <HostMetricHistoryPanel agent={agent} history={history} selection={selection} />
    </>
  )
}

function HostMetricHistoryPanel({
  agent,
  history,
  selection,
}: {
  agent: AgentDiagnostic
  history: MetricHistoryWindow
  selection: HostMetricSelection
}) {
  const { generation } = useAuth()
  const query = useAdminAgentHostMetricHistory(
    generation,
    agent.agent_id,
    selection.metric,
    selection.dimension,
    history.range.from,
    history.range.to,
    history.olderThan ?? undefined,
  )
  const retained = agent.nodes.length
  return (
    <MetricHistoryBody
      title="Host resource history"
      subject="Host"
      surface="agent-host"
      intro={AGENT_HOST_INTRO}
      definitions={HOST_METRIC_SERIES}
      selection={selection}
      extraControl={
        selection.needsMount ? (
          <HostMountPathControl
            value={selection.mountPath}
            onChange={selection.onMountPath}
          />
        ) : undefined
      }
      note={
        <p
          className="mt-2 text-xs text-muted-foreground"
          data-slot="host-metric-history-owner"
        >
          The Agent <span className="break-all font-mono">{agent.agent_id}</span> reports
          this Host once and the Server stores it once, so the same series answers here
          and on each of the {retained} retained Node{retained === 1 ? '' : 's'} it
          reports; a Node's own Process series stays separate.
        </p>
      }
      promptWhenUnasked="Enter the mount path the Agent reported, or choose one from the list above, to read one storage series."
      view={history}
      answer={query}
      unasked={!selection.asked}
    />
  )
}

function DiagnosticsPanel({ agent }: { agent: AgentDiagnostic }) {
  const host = agent.host
  return (
    <CardX
      size="medium"
      className={CARD_SURFACE}
      header={<h2 className={ADMIN_SECTION_TITLE}>Host and component evidence</h2>}
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
          <DetailItem label="Inventory rejection evidence">
            {agent.inventory.last_rejection ? (
              <div className="space-y-0.5">
                <div>
                  {agent.inventory.last_rejection.code}
                  {agent.inventory.last_rejection.declaration === 'server_managed'
                    ? ' · server-managed declaration'
                    : ''}
                </div>
                <small className="block text-xs text-muted-foreground">
                  {agent.inventory.last_rejection.declaration === 'server_managed'
                    ? 'Declared fingerprint ' +
                      (agent.inventory.last_rejection.reported_sha256
                        ? shortHash(agent.inventory.last_rejection.reported_sha256)
                        : 'not recorded')
                    : 'Declared revision ' +
                      (agent.inventory.last_rejection.reported_revision ?? 'Unknown') +
                      ' · ' +
                      (agent.inventory.last_rejection.reported_sha256
                        ? shortHash(agent.inventory.last_rejection.reported_sha256)
                        : 'content hash not recorded')}
                </small>
                <small className="block text-xs text-muted-foreground">
                  {revisionSourceLabel(agent.inventory.accepted_declaration)}{' '}
                  {agent.inventory.accepted_revision ?? 'Unknown'} ·{' '}
                  {agent.inventory.accepted_sha256
                    ? shortHash(agent.inventory.accepted_sha256)
                    : 'content hash not recorded'}
                </small>
                <small className="block text-xs text-muted-foreground">
                  Received {formatObservedAt(agent.inventory.last_rejection.received_at)}
                </small>
              </div>
            ) : (
              'No Inventory rejection recorded'
            )}
          </DetailItem>
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
                        <small className="mt-0.5 block text-xs text-muted-foreground">
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
    <CardX size="medium" className={CARD_SURFACE} id="audit" header={<h2 className={ADMIN_SECTION_TITLE}>Audit trail</h2>}>
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
        <small className="text-xs text-muted-foreground">
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
    <p className="flex flex-wrap items-center gap-1 text-xs text-muted-foreground">
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
      <code className="block break-all rounded-sm bg-muted p-2 text-xs">{secret}</code>
      <Button type="button" onClick={() => void copySecret()}>
        Copy secret
      </Button>
      {copied && <p className="text-sm text-success">Copied to clipboard.</p>}
      <p className="text-xs text-muted-foreground">
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
      <p className="text-xs text-muted-foreground">Single use; expires after the selected window.</p>
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
      header={<h2 className={ADMIN_SECTION_TITLE}>Local enrollment instructions</h2>}
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
      <p className="mt-3 text-xs text-muted-foreground">
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
        <h1 className={ADMIN_PAGE_TITLE}>Enroll a new Agent</h1>
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
        <CardX size="medium" className={CARD_SURFACE} header={<h2 className={ADMIN_SECTION_TITLE}>Enrollment token created</h2>}>
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
        <CardX size="medium" className={CARD_SURFACE} header={<h2 className={ADMIN_SECTION_TITLE}>Token lifetime</h2>}>
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
          <h1 className={ADMIN_PAGE_TITLE}>Agent unavailable</h1>
          <p className="text-sm text-muted-foreground">This Agent is no longer available.</p>
        </div>
      </section>
    )
  }

  return (
    <section className="w-full min-w-0 space-y-4">
      <div className="space-y-1">
        <h1 className={ADMIN_PAGE_TITLE}>Recover Agent {agentId ? shortId(agentId) : ''}</h1>
        <p className="text-sm text-muted-foreground">
          <Link
            className="inline-flex min-h-11 min-w-11 items-center font-medium underline-offset-4 hover:underline"
            to={'/admin/agents/' + agentId + '?section=manage#credentials'}
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
        <CardX size="medium" className={CARD_SURFACE} header={<h2 className={ADMIN_SECTION_TITLE}>Recovery token created</h2>}>
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
          <Link className={cn(buttonVariants(), 'mt-3 w-fit')} to={'/admin/agents/' + agentId + '?section=manage#credentials'}>
            Back to Agent detail
          </Link>
        </CardX>
      ) : (
        <CardX size="medium" className={CARD_SURFACE} header={<h2 className={ADMIN_SECTION_TITLE}>Create a recovery token</h2>}>
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
          <h1 className={ADMIN_PAGE_TITLE}>Agent unavailable</h1>
          <p className="text-sm text-muted-foreground">This Agent is no longer available.</p>
        </div>
      </section>
    )
  }

  return (
    <section className="w-full min-w-0 space-y-4">
      <div className="space-y-1">
        <h1 className={ADMIN_PAGE_TITLE}>Rotate credential {agentId ? 'for Agent ' + shortId(agentId) : ''}</h1>
        <p className="text-sm text-muted-foreground">
          <Link
            className="inline-flex min-h-11 min-w-11 items-center font-medium underline-offset-4 hover:underline"
            to={'/admin/agents/' + agentId + '?section=manage#credentials'}
          >
            Back to Agent detail
          </Link>
        </p>
      </div>
      {result ? (
        <CardX size="medium" className={CARD_SURFACE} header={<h2 className={ADMIN_SECTION_TITLE}>Credential rotated</h2>}>
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
          <Link className={cn(buttonVariants(), 'mt-3 w-fit')} to={'/admin/agents/' + agentId + '?section=manage#credentials'}>
            Back to Agent detail
          </Link>
        </CardX>
      ) : (
        <CardX size="medium" className={CARD_SURFACE} header={<h2 className={ADMIN_SECTION_TITLE}>Rotate the Agent credential</h2>}>
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
              <p className="text-xs text-muted-foreground">
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
              <p className="text-xs text-muted-foreground">
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
