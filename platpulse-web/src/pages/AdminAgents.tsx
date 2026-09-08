import { useId, useState, type FormEvent } from 'react'
import { Link, useParams } from 'react-router'
import {
  AdminApiError,
  createEnrollmentToken,
  createRecoveryToken,
  revokeAgentCredential,
  rotateAgentCredential,
  useAdminAgentAudit,
  useAdminAgentDetail,
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
import type {
  AgentAuditItem,
  AgentCredentialSummary,
  AgentDiagnostic,
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

function shortId(id: string): string {
  return id.length > 14 ? `${id.slice(0, 8)}…${id.slice(-4)}` : id
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
  return `${active} active · ${revoked} revoked · ${inactive} inactive (not revoked) · ${credentials.length} total`
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
    <span className="agent-copy-control">
      <button type="button" className="text-action" onClick={() => void copyAgentId()}>
        Copy Agent ID
      </button>
      {copyState === 'copied' && <span className="agent-copy-status" role="status">Copied to clipboard.</span>}
      {copyState === 'failed' && (
        <span className="agent-copy-status" role="status">
          Copy unavailable; select the full Agent ID above.
        </span>
      )}
    </span>
  )
}

function AgentIdentityAccess({ agentId }: { agentId: string }) {
  const [revealed, setRevealed] = useState(false)
  const fullIdPanelId = useId()
  return (
    <div className="agent-identity-access">
      <Link className="agent-link" to={`/admin/agents/${encodeURIComponent(agentId)}`}>
        {shortId(agentId)}
      </Link>
      <button
        type="button"
        className="text-action agent-identity-toggle"
        aria-expanded={revealed}
        aria-controls={fullIdPanelId}
        onClick={() => setRevealed((value) => !value)}
      >
        {revealed ? 'Hide full Agent ID' : 'Show full Agent ID'}
      </button>
      {revealed && (
        <div id={fullIdPanelId} className="agent-identity-details">
          <code className="agent-full-id">{agentId}</code>
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
  return `${from == null ? 'Unknown' : format(from)}–${to == null ? 'Unknown' : format(to)}`
}

function diagnosticSequenceRangeText(from: number | null | undefined, to: number | null | undefined): string {
  if (from == null && to == null) return 'Unknown'
  return `#${from == null ? 'Unknown' : from}–#${to == null ? 'Unknown' : to}`
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
    <section className="page">
      <h1>Agents</h1>
      <p className="muted">
        Server reporting status, receipt time, retained Node inventory, credential validity,
        and diagnostics stay separate dimensions. Detailed boot/report state remains on each
        Agent detail page; Agent Offline is not Node Retired.
      </p>
      {!query.data && query.isPending && (
        <p className="panel-state" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading Agent
          inventory…
        </p>
      )}
      {!query.data && query.isError && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Error" tone="error" />{' '}
          {query.error instanceof Error ? query.error.message : 'Unable to load Agents'}
          <button type="button" className="text-action" onClick={() => void query.refetch()}>
            Try again
          </button>
        </p>
      )}
      {query.data && query.isRefetchError && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Error" tone="error" /> Failed to refresh; showing the last
          successful Agent values.
        </p>
      )}
      {query.data && agents.length === 0 && (
        <p className="panel-state">
          <StatusBadge status="Empty" tone="ok" /> No Agents enrolled yet.
          Enrollment is not available in this Admin surface.
        </p>
      )}
      {query.data && agents.length > 0 && (
        <div className="table-wrap">
          <table className="node-table agent-table">
            <caption className="sr-only">
              Agent, Server reporting status, receipt time, retained Node inventory, credential validity,
              and diagnostic evidence
            </caption>
            <thead>
              <tr>
                <th scope="col">Agent</th>
                <th scope="col">Reporting status</th>
                <th scope="col">Last received</th>
                <th scope="col">Node Inventory</th>
                <th scope="col">Credentials</th>
                <th scope="col">Diagnostics</th>
              </tr>
            </thead>
            <tbody>
              {agents.map((agent) => (
                <AgentListRow key={agent.agent_id} agent={agent} />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  )
}

function AgentListRow({ agent }: { agent: AgentDiagnostic }) {
  const liveness = livenessLabel(agent.liveness)
  return (
    <tr>
      <th scope="row" data-label="Agent">
        <AgentIdentityAccess agentId={agent.agent_id} />
      </th>
      <td data-label="Reporting status" className="agent-summary-status">
        <span className="dimension-label">Server liveness</span>
        <StatusBadge status={liveness} tone={livenessTone(agent.liveness)} />
      </td>
      <td data-label="Last received" className="agent-summary-receipt">
        <span className="dimension-label">Server receipt time</span>
        <span>{receiptTimeText(agent.last_received_at)}</span>
      </td>
      <td data-label="Node Inventory" className="agent-summary-inventory">
        <strong>{agent.nodes.length} retained Node{agent.nodes.length === 1 ? '' : 's'}</strong>
        <small className="muted">Active + Retired</small>
      </td>
      <td data-label="Credentials" className="agent-summary-credentials">
        <span className="dimension-label">Server validity</span>
        <span>{credentialSummaryText(agent.credentials)}</span>
      </td>
      <td data-label="Diagnostics" className="agent-summary-diagnostics">
        <dl className="agent-diagnostic-summary">
          <div>
            <dt>Recorded gap intervals</dt>
            <dd>{agent.sequence_gap_count}</dd>
          </div>
          <div>
            <dt>Accumulated recorded security events</dt>
            <dd>{agent.security_event_count}</dd>
          </div>
        </dl>
        <div className="agent-diagnostic-evidence">
          <span className="dimension-label">Recorded evidence</span>
          <ul className="agent-diagnostic-findings" aria-label="Recorded diagnostic evidence">
            {diagnosticFindings(agent.host).map((finding) => (
              <li key={finding.key}>
                <span className="diagnostic-finding-label">{finding.label}</span>
                <span>{finding.value}</span>
              </li>
            ))}
          </ul>
          <p className="agent-diagnostic-note">
            Recorded evidence; Server liveness remains separate.
            {agent.host ? ` Host snapshot: ${formatObservedAt(agent.host.updated_at)}` : ' No Host snapshot.'}
          </p>
        </div>
      </td>
    </tr>
  )
}

/** PAGE-ADMIN-AGENT-DETAIL: identity, credentials, liveness, inventory,
 * diagnostics, and the redacted Audit trail for one Agent. */
export function AdminAgentDetail() {
  const { agentId = '' } = useParams()
  const { generation } = useAuth()
  const agent = useAdminAgentDetail(generation, agentId)
  const audit = useAdminAgentAudit(generation, agentId)
  const notFound =
    agent.isError && agent.error instanceof AdminApiError && agent.error.code === 'agent_not_found'

  if (notFound) {
    return (
      <section className="page">
        <h1>Agent unavailable</h1>
        <p>This Agent is no longer available.</p>
      </section>
    )
  }
  return (
    <section className="page">
      <h1>
        Agent {shortId(agentId)}
        <span className="heading-muted">{agentId}</span>
      </h1>
      <p className="muted">
        <Link className="text-action" to="/admin/agents">
          All Agents
        </Link>{' '}
        · identity, liveness, boot/report state, Inventory, credentials, and diagnostics
        stay separate.
      </p>
      {!agent.data && (
        <>
          {agent.isPending && (
            <p className="panel-state" role="status">
              <StatusBadge status="Starting" tone="neutral" /> Loading Agent state…
            </p>
          )}
          {agent.isError && (
            <p className="panel-state" role="alert">
              <StatusBadge status="Error" tone="error" />{' '}
              {agent.error instanceof Error ? agent.error.message : 'Unable to load the Agent'}
              <button type="button" className="text-action" onClick={() => void agent.refetch()}>
                Try again
              </button>
            </p>
          )}
        </>
      )}
      {agent.data && (
        <>
          {agent.isRefetchError && (
            <p className="panel-state" role="alert">
              <StatusBadge status="Error" tone="error" /> Failed to refresh; showing the last
              successful Agent values.
            </p>
          )}
          <AgentDetailSummary agent={agent.data} />
          <section className="agent-detail-section" aria-labelledby="agent-overview-heading">
            <div className="agent-detail-section-heading">
              <span className="eyebrow">01</span>
              <h2 id="agent-overview-heading">Overview</h2>
              <p className="muted">Identity and the Agent-declared Node Inventory.</p>
            </div>
            <div className="agent-detail-grid">
              <IdentityPanel agent={agent.data} />
              <InventoryPanel nodes={agent.data.nodes} />
            </div>
          </section>
          <section className="agent-detail-section" aria-labelledby="agent-runtime-heading">
            <div className="agent-detail-section-heading">
              <span className="eyebrow">02</span>
              <h2 id="agent-runtime-heading">Runtime and reporting</h2>
              <p className="muted">Server liveness remains separate from boot and report lifecycle.</p>
            </div>
            <div className="agent-detail-grid">
              <LivenessPanel agent={agent.data} />
              <BootReportPanel agent={agent.data} />
            </div>
          </section>
          <section className="agent-detail-section" aria-labelledby="agent-credentials-heading">
            <div className="agent-detail-section-heading">
              <span className="eyebrow">03</span>
              <h2 id="agent-credentials-heading">Credentials</h2>
              <p className="muted">Server-owned credential validity and explicit revocation.</p>
            </div>
            <CredentialsPanel agent={agent.data} onConflictReload={() => void agent.refetch()} />
          </section>
          <section className="agent-detail-section" aria-labelledby="agent-diagnostics-heading">
            <div className="agent-detail-section-heading">
              <span className="eyebrow">04</span>
              <h2 id="agent-diagnostics-heading">Diagnostics</h2>
              <p className="muted">Recorded evidence is shown without inferring current recovery or failure.</p>
            </div>
            <DiagnosticsPanel agent={agent.data} />
          </section>
          <section className="agent-detail-section" aria-labelledby="agent-audit-heading">
            <div className="agent-detail-section-heading">
              <span className="eyebrow">05</span>
              <h2 id="agent-audit-heading">Audit</h2>
              <p className="muted">Immutable, redacted lifecycle events for this Agent.</p>
            </div>
            <AuditTrailPanel audit={audit} agentId={agentId} />
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
    <section className="agent-summary" aria-labelledby="agent-summary-heading">
      <div className="agent-summary-header">
        <div>
          <span className="eyebrow">Agent summary</span>
          <h2 id="agent-summary-heading">{shortId(agent.agent_id)}</h2>
          <p className="agent-summary-id">
            <code title={agent.agent_id}>{formatIdentifier(agent.agent_id)}</code>
            <span className="sr-only">Full Agent ID: {agent.agent_id}</span>
            <AgentIdCopyControl agentId={agent.agent_id} />
          </p>
        </div>
        <div className="agent-summary-statuses" aria-label="Current Agent dimensions">
          <div>
            <span className="dimension-label">Server liveness</span>
            <StatusBadge status={liveness} tone={livenessTone(agent.liveness)} />
          </div>
          <div>
            <span className="dimension-label">Boot status</span>
            <strong className="agent-summary-plain-status">{bootStatus}</strong>
          </div>
          <div>
            <span className="dimension-label">Active credentials</span>
            <strong>{activeCredentials} active · {agent.credentials.length} total</strong>
          </div>
        </div>
      </div>
      <dl className="agent-summary-facts">
        <div>
          <dt>Agent Epoch</dt>
          <dd>{agent.agent_epoch}</dd>
        </div>
        <div>
          <dt>Last received</dt>
          <dd>{receiptTimeText(agent.last_received_at)}</dd>
        </div>
        <div>
          <dt>Report sequence</dt>
          <dd>{agent.last_report_sequence == null ? 'Never received' : '#' + agent.last_report_sequence}</dd>
        </div>
        <div>
          <dt>Declared Nodes</dt>
          <dd>{agent.nodes.length}</dd>
        </div>
      </dl>
      {warnings.length > 0 && (
        <div className="agent-summary-warnings" role="note" aria-label="Important Agent warnings">
          <h3>Important warnings</h3>
          <ul>
            {warnings.map((warning, index) => (
              <li key={warning.kind + '-' + index} className={'agent-summary-warning-' + warning.kind}>
                <strong>{warning.kind === 'current' ? 'Current state' : warning.kind === 'recorded' ? 'Recorded history' : 'Unknown'}</strong>
                <span>{warning.message}</span>
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
    <article className="panel">
      <h3>Identity</h3>
      <dl className="detail-list">
        <div>
          <dt>Agent ID</dt>
          <dd className="agent-detail-identity">
            <code className="agent-full-id">{agent.agent_id}</code>
          </dd>
        </div>
        <div>
          <dt>Agent Epoch</dt>
          <dd>{agent.agent_epoch}</dd>
        </div>
        <div>
          <dt>Capabilities</dt>
          <dd>
            {agent.capabilities.length > 0 ? agent.capabilities.join(', ') : 'Unsupported'}
          </dd>
        </div>
      </dl>
    </article>
  )
}

function LivenessPanel({ agent }: { agent: AgentDiagnostic }) {
  const liveness = livenessLabel(agent.liveness)
  return (
    <article className="panel">
      <h3>Liveness</h3>
      <dl className="detail-list">
        <div>
          <dt>Server liveness</dt>
          <dd><StatusBadge status={liveness} tone={livenessTone(agent.liveness)} /></dd>
        </div>
        <div>
          <dt>Server receipt time</dt>
          <dd>{receiptTimeText(agent.last_received_at)}</dd>
        </div>
      </dl>
    </article>
  )
}

function BootReportPanel({ agent }: { agent: AgentDiagnostic }) {
  return (
    <article className="panel">
      <h3>Boot and report state</h3>
      <dl className="detail-list">
        <div>
          <dt>Boot status</dt>
          <dd>{agent.boot_status}</dd>
        </div>
        <div>
          <dt>Full active boot ID</dt>
          <dd><code>{agent.active_boot_id ?? 'Unknown'}</code></dd>
        </div>
        <div>
          <dt>Previous boot ID</dt>
          <dd><code>{agent.previous_boot_id ?? 'None'}</code></dd>
        </div>
        <div>
          <dt>Close report ID</dt>
          <dd><code>{agent.close_report_id ?? 'None'}</code></dd>
        </div>
        <div>
          <dt>Report sequence</dt>
          <dd>{agent.last_report_sequence == null ? 'Never received' : `sequence #${agent.last_report_sequence}`}</dd>
        </div>
        <div>
          <dt>Shutdown state</dt>
          <dd>
            {agent.shutdown_state}
            {agent.shutdown_forced ? ' · forced' : ''}
            {agent.shutdown_last_error ? ` · ${agent.shutdown_last_error}` : ''}
          </dd>
        </div>
        <div>
          <dt>Shutdown report ID</dt>
          <dd><code>{agent.shutdown_report_id ?? 'None'}</code></dd>
        </div>
        <div>
          <dt>Shutdown report sequence</dt>
          <dd>{agent.shutdown_report_sequence ?? 'None'}</dd>
        </div>
        <div>
          <dt>Shutdown started</dt>
          <dd>{formatObservedAt(agent.shutdown_started_at)}</dd>
        </div>
        <div>
          <dt>Shutdown deadline</dt>
          <dd>{formatObservedAt(agent.shutdown_deadline_at)}</dd>
        </div>
        <div>
          <dt>Shutdown finished</dt>
          <dd>{formatObservedAt(agent.shutdown_finished_at)}</dd>
        </div>
        <div>
          <dt>Unresolved shutdown range</dt>
          <dd>{agent.shutdown_unresolved_range ? agent.shutdown_unresolved_range.join('–') : 'None'}</dd>
        </div>
        <div>
          <dt>Shutdown updated</dt>
          <dd>{formatObservedAt(agent.shutdown_updated_at)}</dd>
        </div>
      </dl>
    </article>
  )
}

function InventoryPanel({ nodes }: { nodes: NodeDiagnostic[] }) {
  return (
    <article className="panel">
      <h3>Inventory</h3>
      {nodes.length === 0 && (
        <p className="panel-state">
          <StatusBadge status="Empty" tone="ok" /> No PlatON Nodes declared by this Agent yet.
        </p>
      )}
      {nodes.length > 0 && (
        <ul className="inventory-list">
          {nodes.map((node) => (
            <li key={node.node_id} className="inventory-item">
              <span>
                <strong>{node.display_name ?? node.node_id}</strong>{' '}
                <small className="muted" title={`Full Node ID: ${node.node_id}`}>
                  <span aria-hidden="true">{shortId(node.node_id)}</span>
                  <span className="sr-only">Full Node ID: {node.node_id}</span>
                </small>
              </span>
              <span className="muted">{node.network_key}</span>
              <span>{node.lifecycle}</span>
              <StatusBadge status={node.visibility} tone="neutral" />
            </li>
          ))}
        </ul>
      )}
    </article>
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
      setMessage(`Credential revoked at ${formatObservedAt(result.revoked_at)}.`)
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
    <article className="panel" id="credentials">
      <h3>Credential records</h3>
      <p className="muted">
        Only non-sensitive credential ids and lifecycle instants are shown; secrets are
        never stored or displayed again.
      </p>
      {message && (
        <p className="form-success" role="status">
          {message}
        </p>
      )}
      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}
      {agent.credentials.length === 0 && (
        <p className="panel-state">
          <StatusBadge status="Empty" tone="ok" /> No credentials issued yet.
        </p>
      )}
      {agent.credentials.length > 0 && (
        <ul className="credential-list">
          {agent.credentials.map((credential) => {
            const state = credentialStatus(credential)
            return (
              <li key={credential.credential_id} className="credential-item">
                <div className="credential-main">
                  <span className="credential-id">{credential.credential_id}</span>
                  <StatusBadge status={state.label} tone={state.tone} />
                  {credential.revoke_after && credential.active && (
                    <small className="muted">
                      Overlap expires {formatObservedAt(credential.revoke_after)}
                    </small>
                  )}
                </div>
                <small className="muted">
                  Issued {formatObservedAt(credential.created_at)}
                  {credential.revoked_at ? ` · revoked ${formatObservedAt(credential.revoked_at)}` : ''}
                </small>
                {credential.active && (
                  <div className="credential-actions">
                    {confirmingId === credential.credential_id ? (
                      <>
                        <span className="confirm-copy">
                          Revoke now? The Agent stops authenticating immediately.
                        </span>
                        <button
                          type="button"
                          className="danger-action"
                          onClick={() => void revoke(credential.credential_id)}
                          disabled={busy}
                        >
                          {busy ? 'Revoking…' : 'Confirm revoke'}
                        </button>
                        <button
                          type="button"
                          className="text-action"
                          onClick={() => setConfirmingId(null)}
                          disabled={busy}
                        >
                          Cancel
                        </button>
                      </>
                    ) : (
                      <button
                        type="button"
                        className="text-action"
                        onClick={() => setConfirmingId(credential.credential_id)}
                        disabled={busy}
                      >
                        Revoke
                      </button>
                    )}
                  </div>
                )}
              </li>
            )
          })}
        </ul>
      )}
    </article>
  )
}

function DiagnosticsPanel({ agent }: { agent: AgentDiagnostic }) {
  const host = agent.host
  return (
    <article className="panel">
      <h3>Host and component evidence</h3>
      <p className="muted diagnostic-evidence-note">
        {host
          ? 'Recorded evidence from the latest Host observation; it does not infer current failure or recovery. Server liveness remains independent.'
          : 'No Host observation is available; absent diagnostic fields remain unknown.'}
      </p>
      <dl className="detail-list diagnostic-detail-list">
        <div>
          <dt>Clock</dt>
          <dd>
            {agent.clock_status}
            {agent.clock_skew_ms != null ? ` · skew ${agent.clock_skew_ms} ms` : ''}
          </dd>
        </div>
        <div>
          <dt>Recorded gap intervals</dt>
          <dd>{agent.sequence_gap_count}</dd>
        </div>
        <div>
          <dt>Accumulated recorded security events</dt>
          <dd>{agent.security_event_count}</dd>
        </div>
        {!host && (
          <div>
            <dt>Host observation</dt>
            <dd>No host observation yet</dd>
          </div>
        )}
        {host && (
          <>
            <div>
              <dt>Host observation</dt>
              <dd>{formatObservedAt(host.updated_at)}</dd>
            </div>
            <div>
              <dt>Host CPU</dt>
              <dd>{formatPercent(host.cpu_percent)}</dd>
            </div>
            <div>
              <dt>Host memory used / total</dt>
              <dd>{formatBytesUnknown(host.memory_used_bytes)} / {formatBytesUnknown(host.memory_total_bytes)}</dd>
            </div>
            <div>
              <dt>Host load (1 / 5 / 15)</dt>
              <dd>{host.load1 == null ? 'Unknown' : host.load1} / {host.load5 == null ? 'Unknown' : host.load5} / {host.load15 == null ? 'Unknown' : host.load15}</dd>
            </div>
            <div>
              <dt>Host network RX / TX</dt>
              <dd>{formatBytesPerSecond(host.network_rx_bytes_per_sec)} / {formatBytesPerSecond(host.network_tx_bytes_per_sec)}</dd>
            </div>
            <div>
              <dt>Host components</dt>
              <dd>
                {host.components.length === 0 ? 'None observed' : (
                  <ul className="diagnostic-component-list">
                    {host.components.map((component) => (
                      <li key={component.component}>
                        <strong>{component.component}</strong>: {component.state}
                        {component.error_code ? ' · ' + component.error_code : ''}
                        {component.error_message ? ' · ' + component.error_message : ''}
                        <small className="muted diagnostic-component-meta">
                          attempted {formatObservedAt(component.attempted_at)} · observed {formatObservedAt(component.observed_at)} · received {formatObservedAt(component.received_at)} · state revision {component.state_revision} · value revision {component.value_revision}
                        </small>
                      </li>
                    ))}
                  </ul>
                )}
              </dd>
            </div>
            <div>
              <dt>Spool observation</dt>
              <dd>{hasSpoolObservation(host) ? 'Observed' : 'Not observed yet'}</dd>
            </div>
            <div>
              <dt>Spool evidence</dt>
              <dd>{spoolDiagnosticText(host)}</dd>
            </div>
            <div>
              <dt>Spool queued reports</dt>
              <dd>{host.spool_queued_reports ?? 'Unknown'}</dd>
            </div>
            <div>
              <dt>Spool delivery state</dt>
              <dd>{host.spool_in_flight == null ? 'Unknown' : host.spool_in_flight ? 'In flight' : 'Idle'}</dd>
            </div>
            <div>
              <dt>Spool bytes / capacity</dt>
              <dd>{formatBytesUnknown(host.spool_queued_bytes)} / {formatBytesUnknown(host.spool_capacity_bytes)}</dd>
            </div>
            <div>
              <dt>Spool oldest / maximum age</dt>
              <dd>
                {host.spool_oldest_queued_age_ms == null ? 'Unknown' : host.spool_oldest_queued_age_ms + ' ms'} / {host.spool_max_age_seconds == null ? 'Unknown' : host.spool_max_age_seconds + ' s'}
              </dd>
            </div>
            <div>
              <dt>Last spool delivery</dt>
              <dd>{host.spool_last_delivery_at == null ? 'Unknown' : formatObservedAt(host.spool_last_delivery_at)}</dd>
            </div>
            <div>
              <dt>Last delivery error</dt>
              <dd>{diagnosticMessageText(host.spool_last_delivery_error)}</dd>
            </div>
            <div>
              <dt>Dropped sequence range</dt>
              <dd>{diagnosticRangeText(host.spool_dropped_sequence_from, host.spool_dropped_sequence_to)}</dd>
            </div>
            <div>
              <dt>Dropped height range</dt>
              <dd>{diagnosticRangeText(host.spool_dropped_height_from, host.spool_dropped_height_to)}</dd>
            </div>
            <div>
              <dt>Dropped time range</dt>
              <dd>{diagnosticRangeText(host.spool_dropped_time_from, host.spool_dropped_time_to, formatObservedAt)}</dd>
            </div>
            <div>
              <dt>Pending history gaps</dt>
              <dd>{host.spool_pending_history_gaps ?? 'Unknown'}</dd>
            </div>
            <div>
              <dt>Report size state</dt>
              <dd>{host.spool_report_too_large == null ? 'Unknown' : host.spool_report_too_large ? 'Too large' : 'Within limit'}</dd>
            </div>
            <div>
              <dt>Spool store fatal</dt>
              <dd>{host.spool_store_fatal == null ? 'Unknown' : host.spool_store_fatal ? 'Yes' : 'No'}</dd>
            </div>
            <div>
              <dt>Spool store error</dt>
              <dd>{diagnosticMessageText(host.spool_store_error)}</dd>
            </div>
          </>
        )}
      </dl>
    </article>
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
    <article className="panel" id="audit">
      <h3>Audit trail</h3>
      <p className="muted">
        Redacted immutable events for this Agent. One-time secrets never appear in Audit.
      </p>
      {!audit.data && audit.isPending && (
        <p className="panel-state" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading the Audit trail…
        </p>
      )}
      {!audit.data && audit.isError && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Error" tone="error" />{' '}
          {audit.error instanceof Error ? audit.error.message : 'Unable to load the Audit trail'}
          <button type="button" className="text-action" onClick={() => void audit.refetch()}>
            Try again
          </button>
        </p>
      )}
      {audit.data && items.length === 0 && (
        <p className="panel-state">
          <StatusBadge status="Empty" tone="ok" /> No Audit events for this Agent yet.
        </p>
      )}
      {audit.data && items.length > 0 && (
        <ul className="audit-list">
          {items.map((item) => (
            <AuditItemRow key={item.audit_event_id} item={item} agentId={agentId} />
          ))}
        </ul>
      )}
    </article>
  )
}

function AuditItemRow({ item, agentId }: { item: AgentAuditItem; agentId: string }) {
  const detail = item.details
  const summary =
    detail && typeof detail === 'object'
      ? Object.entries(detail as Record<string, unknown>)
          .map(([key, value]) => `${key}: ${Array.isArray(value) ? value.join(', ') : String(value)}`)
          .join(' · ')
      : ''
  return (
    <li className="audit-item">
      <div className="audit-main">
        <strong>{item.event_kind}</strong>
        <small className="muted">
          {formatObservedAt(item.created_at)} · {item.actor_username ?? 'local-cli'} · event #
          {item.audit_event_id}
        </small>
      </div>
      {summary && <p className="audit-details muted">{summary}</p>}
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
    <p className="muted audit-context">
      Recorded in the redacted Audit trail
      {agentId ? (
        <>
          {' · '}
          <Link className="text-action" to={`/admin/agents/${agentId}#audit`}>
            view Agent audit
          </Link>
        </>
      ) : (
        <>
          {' · '}
          <Link className="text-action" to="/admin/access/audit">
            review the redacted Audit log
          </Link>
        </>
      )}
      {requestId ? ` · request ${requestId}` : ''}
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
    <div className="secret-panel" role="status">
      <p>
        <strong>{label}</strong>
      </p>
      <code className="secret-value">{secret}</code>
      <button type="button" className="primary-action" onClick={() => void copySecret()}>
        Copy secret
      </button>
      {copied && <p className="form-success">Copied to clipboard.</p>}
      <p className="secret-warning">
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
    <div className="field">
      <label htmlFor={id}>{label}</label>
      <select id={id} value={value} onChange={(event) => onChange(Number(event.target.value))}>
        <option value={1}>1 hour</option>
        <option value={6}>6 hours</option>
        <option value={12}>12 hours</option>
        <option value={24}>24 hours (default)</option>
        <option value={72}>72 hours</option>
        <option value={168}>7 days</option>
      </select>
      <p className="field-hint">Single use; expires after the selected window.</p>
    </div>
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
    <section className="page">
      <h1>Enroll a new Agent</h1>
      <p className="muted">
        <Link className="text-action" to="/admin/agents">
          All Agents
        </Link>{' '}
        · the Server issues a short-lived, single-use Enrollment Token. The Agent exchanges
        it once for a stable identity and an Agent Credential; the token cannot enroll twice.
      </p>
      {result ? (
        <div className="success-panel">
          <h2>Enrollment token created</h2>
          <OneTimeSecret secret={result.token} label="One-time Enrollment Token" />
          <p className="muted">
            Token id {result.token_id} · expires {formatObservedAt(result.expires_at)} · shown
            exactly once.
          </p>
          <AuditContextLink agentId="" requestId={result.request_id} />
          <p className="muted">
            Recorded as a redacted <code>enrollment_token_created</code> Audit event.
          </p>
          <Link className="primary-action" to="/admin/agents">
            Back to Agents
          </Link>
        </div>
      ) : (
        <form onSubmit={submit} className="single-form">
          <LifetimeField
            id="enroll-lifetime"
            label="Token lifetime"
            value={expiresInHours}
            onChange={setExpiresInHours}
          />
          <button className="primary-action" type="submit" disabled={busy}>
            Create enrollment token
          </button>
          {error && (
            <p className="form-error" role="alert">
              {error}
            </p>
          )}
        </form>
      )}
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
      <section className="page">
        <h1>Agent unavailable</h1>
        <p>This Agent is no longer available.</p>
      </section>
    )
  }

  return (
    <section className="page">
      <h1>Recover Agent {agentId ? shortId(agentId) : ''}</h1>
      <p className="muted">
        <Link className="text-action" to={`/admin/agents/${agentId}`}>
          Back to Agent detail
        </Link>
      </p>
      {agent.data && (
        <p className="panel-state">
          Current Agent Epoch:{' '}
          <StatusBadge status={`${agent.data.agent_epoch}`} tone="neutral" /> · liveness{' '}
          <StatusBadge status={livenessLabel(agent.data.liveness)} tone="neutral" />
        </p>
      )}
      {!agent.data && agent.isPending && (
        <p className="panel-state" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading Agent state…
        </p>
      )}
      {result ? (
        <div className="success-panel">
          <h2>Recovery token created</h2>
          <OneTimeSecret secret={result.token} label="One-time Recovery Token" />
          <p className="muted">
            Token id {result.token_id} · expires {formatObservedAt(result.expires_at)}.
          </p>
          <p className="warning-copy">
            When the Agent exchanges this token its Epoch advances from {result.agent_epoch} to{' '}
            {result.agent_epoch + 1}, every existing credential is revoked, and a fresh
            credential is issued — the same Agent identity is preserved, never duplicated.
          </p>
          <AuditContextLink agentId={agentId} requestId={result.request_id} />
          <Link className="primary-action" to={`/admin/agents/${agentId}`}>
            Back to Agent detail
          </Link>
        </div>
      ) : (
        <form onSubmit={submit} className="single-form">
          <div className="warning-copy">
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
          <div className="field checkbox-field">
            <label htmlFor="recover-confirm">
              <input
                id="recover-confirm"
                type="checkbox"
                checked={confirmed}
                onChange={(event) => setConfirmed(event.target.checked)}
              />
              I understand: recovery advances the Agent Epoch and revokes every existing
              credential; it cannot be undone and never creates a duplicate Agent.
            </label>
          </div>
          <button className="primary-action" type="submit" disabled={busy || !confirmed}>
            Create recovery token
          </button>
          {error && (
            <p className="form-error" role="alert">
              {error}
            </p>
          )}
        </form>
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
      <section className="page">
        <h1>Agent unavailable</h1>
        <p>This Agent is no longer available.</p>
      </section>
    )
  }

  return (
    <section className="page">
      <h1>Rotate credential {agentId ? `for Agent ${shortId(agentId)}` : ''}</h1>
      <p className="muted">
        <Link className="text-action" to={`/admin/agents/${agentId}`}>
          Back to Agent detail
        </Link>
      </p>
      {result ? (
        <div className="success-panel">
          <h2>Credential rotated</h2>
          <OneTimeSecret secret={result.credential} label="New Agent Credential" />
          <p className="muted">
            Credential id {result.credential_id} · overlap {result.overlap_hours} hour
            {result.overlap_hours === 1 ? '' : 's'}
            {result.revoked_previous_ids.length > 0
              ? ' · previous credential(s) revoked immediately'
              : result.overlap_credential_ids.length > 0
                ? ` · previous credential(s) stay valid until ${formatObservedAt(result.revoke_after)}`
                : ' · no previous valid credential remained'}
            {result.revoked_previous_ids.length > 0
              ? ` · revoked: ${result.revoked_previous_ids.join(', ')}`
              : ''}
            {result.overlap_credential_ids.length > 0
              ? ` · overlap: ${result.overlap_credential_ids.join(', ')}`
              : ''}
          </p>
          <p className="warning-copy">
            The Agent Epoch was not changed. Install the new credential on the Agent before
            the overlap expires; the previous credential stops working at that instant.
          </p>
          <AuditContextLink agentId={agentId} requestId={result.request_id} />
          <Link className="primary-action" to={`/admin/agents/${agentId}`}>
            Back to Agent detail
          </Link>
        </div>
      ) : (
        <form onSubmit={submit} className="single-form">
          <div className="warning-copy">
            <strong>What rotation does:</strong> issues a fresh credential and keeps the
            previous one valid through an explicit overlap window (or revokes it immediately
            when chosen). The Agent Epoch is untouched and no duplicate Agent is created.
          </div>
          <div className="field">
            <label htmlFor="rotate-overlap">Overlap window</label>
            <select
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
            </select>
            <p className="field-hint">
              The previous credential stays valid for this long after rotation, then stops
              authenticating automatically.
            </p>
          </div>
          <div className="field checkbox-field">
            <label htmlFor="rotate-revoke-previous">
              <input
                id="rotate-revoke-previous"
                type="checkbox"
                checked={revokePrevious}
                onChange={(event) => setRevokePrevious(event.target.checked)}
              />
              Revoke the previous credential immediately
            </label>
            <p className="field-hint">
              Choose this only when the previous credential is compromised or already
              installed on the replacement configuration. Immediate revocation cannot be
              undone.
            </p>
          </div>
          <div className="field checkbox-field">
            <label htmlFor="rotate-confirm">
              <input
                id="rotate-confirm"
                type="checkbox"
                checked={confirmed}
                onChange={(event) => setConfirmed(event.target.checked)}
              />
              I understand: rotation issues a new credential and the previous one stops
              working at the end of the overlap window (or immediately when revocation is
              selected); the Agent Epoch is untouched.
            </label>
          </div>
          <button className="primary-action" type="submit" disabled={busy || !confirmed}>
            Rotate credential
          </button>
          {error && (
            <p className="form-error" role="alert">
              {error}
            </p>
          )}
        </form>
      )}
    </section>
  )
}
