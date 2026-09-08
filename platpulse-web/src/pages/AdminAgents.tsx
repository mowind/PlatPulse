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
import { formatBytes } from '../formatBytes'
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

function livenessTone(liveness: string): 'ok' | 'error' | 'neutral' {
  return liveness === 'online' ? 'ok' : liveness === 'offline' ? 'error' : 'neutral'
}

function receiptTimeText(receivedAt: string | null | undefined): string {
  if (receivedAt === undefined) return 'Unknown'
  if (receivedAt === null) return 'Never received'
  return formatObservedAt(receivedAt)
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

function spoolDiagnosticText(spool: HostDiagnostic | null | undefined): string {
  if (!spool) return 'No host observation yet'
  const hasSpoolObservation = [
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
  if (!hasSpoolObservation) return 'Unknown'
  const parts: string[] = []
  if (spool.spool_queued_reports != null) parts.push(`${spool.spool_queued_reports} queued`)
  if (spool.spool_in_flight === true) parts.push('delivery in flight')
  if (spool.spool_in_flight === false) parts.push('delivery idle')
  if (spool.spool_store_fatal === true) parts.push('store fatal')
  if (spool.spool_store_fatal === false) parts.push('store fatal: no')
  if (spool.spool_dropped_sequence_from != null && spool.spool_dropped_sequence_to != null) {
    parts.push(`dropped reports #${spool.spool_dropped_sequence_from}–#${spool.spool_dropped_sequence_to}`)
  }
  if (spool.spool_last_delivery_error) {
    parts.push(`last delivery error: ${spool.spool_last_delivery_error}`)
  }
  if (spool.spool_store_error) parts.push(`store error: ${spool.spool_store_error}`)
  if (spool.spool_report_too_large === true) parts.push('report too large')
  if (spool.spool_pending_history_gaps != null && spool.spool_pending_history_gaps > 0) {
    parts.push(
      `${spool.spool_pending_history_gaps} pending history gap${spool.spool_pending_history_gaps === 1 ? '' : 's'}`,
    )
  }
  return parts.length > 0 ? parts.join(' · ') : 'Observed; no additional spool evidence'
}

function diagnosticRangeText<T extends number | string>(
  from: T | null | undefined,
  to: T | null | undefined,
  format: (value: T) => string = String,
): string {
  if (from == null && to == null) return 'Unknown'
  return `${from == null ? 'Unknown' : format(from)}–${to == null ? 'Unknown' : format(to)}`
}

function diagnosticBytesText(value: number | null | undefined): string {
  return value == null ? 'Unknown' : formatBytes(value)
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
            <dt>Historical gap intervals</dt>
            <dd>{agent.sequence_gap_count}</dd>
          </div>
          <div>
            <dt>Recorded security events</dt>
            <dd>{agent.security_event_count}</dd>
          </div>
          <div>
            <dt>Spool evidence</dt>
            <dd>{spoolDiagnosticText(agent.host)}</dd>
          </div>
        </dl>
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
          <IdentityPanel agent={agent.data} />
          <LivenessPanel agent={agent.data} />
          <BootReportPanel agent={agent.data} />
          <InventoryPanel nodes={agent.data.nodes} />
          <CredentialsPanel agent={agent.data} onConflictReload={() => void agent.refetch()} />
          <DiagnosticsPanel agent={agent.data} />
          <AuditTrailPanel audit={audit} agentId={agentId} />
        </>
      )}
    </section>
  )
}

function IdentityPanel({ agent }: { agent: AgentDiagnostic }) {
  return (
    <article className="panel">
      <h2>Identity</h2>
      <dl className="detail-list">
        <div>
          <dt>Agent ID</dt>
          <dd className="agent-detail-identity">
            <code className="agent-full-id">{agent.agent_id}</code>
            <AgentIdCopyControl agentId={agent.agent_id} />
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
      <h2>Liveness</h2>
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
      <h2>Boot and report state</h2>
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
      <h2>Inventory</h2>
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
  const [message, setMessage] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  async function revoke(credentialId: string) {
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
    }
  }

  return (
    <article className="panel" id="credentials">
      <h2>Credentials</h2>
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
                        >
                          Confirm revoke
                        </button>
                        <button
                          type="button"
                          className="text-action"
                          onClick={() => setConfirmingId(null)}
                        >
                          Cancel
                        </button>
                      </>
                    ) : (
                      <button
                        type="button"
                        className="text-action"
                        onClick={() => setConfirmingId(credential.credential_id)}
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
      <h2>Diagnostics</h2>
      <dl className="detail-list diagnostic-detail-list">
        <div>
          <dt>Clock</dt>
          <dd>
            {agent.clock_status}
            {agent.clock_skew_ms != null ? ` · skew ${agent.clock_skew_ms} ms` : ''}
          </dd>
        </div>
        <div>
          <dt>Historical gap intervals</dt>
          <dd>{agent.sequence_gap_count}</dd>
        </div>
        <div>
          <dt>Recorded security events</dt>
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
              <dt>Host components</dt>
              <dd>
                {host.components.length === 0 ? 'None observed' : (
                  <ul className="diagnostic-component-list">
                    {host.components.map((component) => (
                      <li key={component.component}>
                        <strong>{component.component}</strong>: {component.state}
                        {component.error_code ? ' · ' + component.error_code : ''}
                        {component.error_message ? ' · ' + component.error_message : ''}
                      </li>
                    ))}
                  </ul>
                )}
              </dd>
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
              <dd>{diagnosticBytesText(host.spool_queued_bytes)} / {diagnosticBytesText(host.spool_capacity_bytes)}</dd>
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
      <h2>Audit trail</h2>
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
