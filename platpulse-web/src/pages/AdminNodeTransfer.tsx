import { useState, type FormEvent } from 'react'
import { Link, useParams } from 'react-router'
import {
  AdminApiError,
  cancelNodeTransfer,
  createNodeTransfer,
  useAdminDiagnostics,
  useAdminNodeDetail,
  useAdminNodeTransfers,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { StatusBadge, formatObservedAt, livenessLabel } from '../components/StatusBadge'
import type { NodeTransfer } from '../api/generated'

/**
 * PAGE-ADMIN-NODE-TRANSFER (design §4.3, §8.2; issue #37/#46): the
 * two-phase Node Transfer workflow. The Owner pre-authorizes a handover to
 * a target Agent; the source stays authoritative until the target declares
 * the same Node ID in a valid Inventory and the Server validates its
 * Network Identity. Every outcome is typed (pending, completed, cancelled,
 * expired, rejected, conflict, identity_mismatch), visible in a timeline,
 * and never optimistic: successful mutations refetch authoritative REST.
 */

const EXPIRY_OPTIONS = [
  { hours: 24, label: '24 hours' },
  { hours: 48, label: '48 hours' },
  { hours: 72, label: '72 hours' },
  { hours: 168, label: '7 days' },
]

export function transferBadge(status: string): {
  label: string
  tone: 'ok' | 'warning' | 'error' | 'neutral'
} {
  switch (status) {
    case 'pending':
      return { label: 'Pending', tone: 'warning' }
    case 'completed':
      return { label: 'Completed', tone: 'ok' }
    case 'cancelled':
      return { label: 'Cancelled', tone: 'neutral' }
    case 'expired':
      return { label: 'Expired', tone: 'neutral' }
    case 'rejected':
      return { label: 'Rejected', tone: 'error' }
    case 'conflict':
      return { label: 'Conflict', tone: 'error' }
    case 'identity_mismatch':
      return { label: 'Identity mismatch', tone: 'error' }
    default:
      return { label: 'Unknown', tone: 'neutral' }
  }
}

function shortId(id: string): string {
  return id.length > 11 ? `${id.slice(0, 8)}…` : id
}

export default function AdminNodeTransfer() {
  const { nodeId = '' } = useParams()
  const { generation, status } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const nodeQuery = useAdminNodeDetail(generation, nodeId)
  const transfersQuery = useAdminNodeTransfers(generation, nodeId)
  const agentsQuery = useAdminDiagnostics(generation)
  const [created, setCreated] = useState<{
    transfer: NodeTransfer
    requestId: string
    auditEventId: number
  } | null>(null)
  const [notice, setNotice] = useState<string | null>(null)

  const notFound =
    nodeQuery.isError && nodeQuery.error instanceof AdminApiError && nodeQuery.error.code === 'not_found'
  if (notFound) {
    return (
      <section className="page">
        <h1>Node unavailable</h1>
        <p>This Node is no longer available.</p>
      </section>
    )
  }

  const transfers = transfersQuery.data ?? []
  const pending = transfers.find((transfer) => transfer.status === 'pending')
  const node = nodeQuery.data

  return (
    <section className="page">
      <h1>Transfer Node ownership</h1>
      <p className="muted">
        <Link className="text-action" to={`/admin/nodes/${nodeId}`}>
          Back to Node detail
        </Link>{' '}
        · {node ? (node.display_name ?? shortId(node.node_id)) : shortId(nodeId)}
      </p>
      <p className="panel-copy">
        Transfer is two-phase: the source Agent stays authoritative until the target Agent
        declares the same Node ID in its local configuration and the Server validates the
        declared Network Identity. The Server never pushes an RPC Endpoint or command to either
        Agent, and completion never changes the Node ID, Network, history, or visibility.
      </p>

      {!node && nodeQuery.isPending && (
        <p className="panel-state" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading Node state…
        </p>
      )}
      {nodeQuery.isError && !notFound && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Error" tone="error" />{' '}
          {nodeQuery.error instanceof Error ? nodeQuery.error.message : 'Unable to load the Node'}
          <button type="button" className="text-action" onClick={() => void nodeQuery.refetch()}>
            Try again
          </button>
        </p>
      )}
      {!transfersQuery.data && transfersQuery.isPending && (
        <p className="panel-state" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading transfer history…
        </p>
      )}
      {transfersQuery.isError && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Error" tone="error" />{' '}
          {transfersQuery.error instanceof Error
            ? transfersQuery.error.message
            : 'Unable to load transfer history'}
          <button
            type="button"
            className="text-action"
            onClick={() => void transfersQuery.refetch()}
          >
            Try again
          </button>
        </p>
      )}

      {node && (
        <>
          {notice && (
            <p className="form-success" role="status">
              {notice}
            </p>
          )}
          {created && <TransferCreatedPanel created={created} />}
          {pending && (
            <PendingTransferPanel
              transfer={pending}
              nodeId={node.node_id}
              sourceAgentId={node.agent_id}
              csrfToken={csrfToken}
              onCancelled={(message) => {
                // The created receipt is superseded by the authoritative
                // cancellation outcome; never keep a stale pending view.
                setCreated(null)
                setNotice(message)
              }}
            />
          )}
          {!pending && (
            <CreateTransferForm
              nodeId={node.node_id}
              sourceAgentId={node.agent_id}
              agents={agentsQuery.data ?? []}
              agentsError={
                agentsQuery.isError
                  ? agentsQuery.error instanceof Error
                    ? agentsQuery.error.message
                    : 'Unable to load Agents'
                  : null
              }
              csrfToken={csrfToken}
              onCreated={setCreated}
            />
          )}
          <TransferTimeline transfers={transfers} nodeId={node.node_id} />
        </>
      )}
    </section>
  )
}

/** Create form (phase 1 of the two-phase workflow): pick the target Agent,
 * the Server-authoritative expiry, and an optional operator reason that is
 * recorded in Audit. High-risk confirmation is explicit and nothing is
 * optimistic. */
function CreateTransferForm({
  nodeId,
  sourceAgentId,
  agents,
  agentsError,
  csrfToken,
  onCreated,
}: {
  nodeId: string
  sourceAgentId: string
  agents: { agent_id: string; liveness: string }[]
  agentsError: string | null
  csrfToken: string
  onCreated: (created: {
    transfer: NodeTransfer
    requestId: string
    auditEventId: number
  }) => void
}) {
  const [target, setTarget] = useState('')
  const [expiryHours, setExpiryHours] = useState(72)
  const [reason, setReason] = useState('')
  const [confirming, setConfirming] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<{
    message: string
    requestId: string | null
    auditReference: string | null
  } | null>(null)

  const candidates = agents.filter((agent) => agent.agent_id !== sourceAgentId)
  const targetKnown = candidates.some((agent) => agent.agent_id === target)

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setError(null)
    setConfirming(true)
  }

  async function confirm() {
    setConfirming(false)
    setBusy(true)
    setError(null)
    try {
      const result = await createNodeTransfer(
        nodeId,
        {
          targetAgentId: target,
          expiresInHours: expiryHours,
          operatorReason: reason.trim() === '' ? undefined : reason.trim(),
        },
        csrfToken,
      )
      onCreated({
        transfer: result.transfer,
        requestId: result.request_id,
        auditEventId: result.audit_event_id,
      })
      setTarget('')
      setReason('')
    } catch (caught) {
      const apiError = caught instanceof AdminApiError ? caught : null
      setError({
        message: caught instanceof Error ? caught.message : 'Unable to start the transfer',
        requestId: apiError?.requestId ?? null,
        auditReference:
          apiError?.fields.find((field) => field.startsWith('audit_event_id:')) ?? null,
      })
    } finally {
      setBusy(false)
    }
  }

  return (
    <article className="panel">
      <div className="panel-heading">
        <h2>Start a transfer</h2>
        <span className="panel-count">Owner only</span>
      </div>
      {agentsError && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Error" tone="error" /> {agentsError} — target Agents cannot be
          listed until the diagnostics load.
        </p>
      )}
      <form className="stack-form" onSubmit={submit}>
        <label>
          Target Agent
          <select
            value={target}
            onChange={(event) => {
              setTarget(event.target.value)
              setConfirming(false)
            }}
            required
          >
            <option value="">Select the Agent that will take over this Node…</option>
            {candidates.map((agent) => (
              <option key={agent.agent_id} value={agent.agent_id}>
                {shortId(agent.agent_id)} · {livenessLabel(agent.liveness)}
              </option>
            ))}
          </select>
        </label>
        {target !== '' && !targetKnown && (
          <p className="form-error" role="alert">
            Choose a registered Agent; the current owner cannot be the target.
          </p>
        )}
        <label>
          Expiry
          <select
            value={expiryHours}
            onChange={(event) => {
              setExpiryHours(Number(event.target.value))
              setConfirming(false)
            }}
          >
            {EXPIRY_OPTIONS.map((option) => (
              <option key={option.hours} value={option.hours}>
                {option.label}
              </option>
            ))}
          </select>
          <small className="muted">
            The Server enforces 1–168 hours; an expired transfer never auto-extends and leaves
            ownership with the source Agent.
          </small>
        </label>
        <label>
          Operator reason <span className="muted">(optional, recorded in Audit)</span>
          <textarea
            value={reason}
            onChange={(event) => {
              setReason(event.target.value)
              setConfirming(false)
            }}
            maxLength={512}
            rows={3}
          />
          <small className="muted">{reason.length}/512</small>
        </label>
        {confirming ? (
          <div className="action-row">
            <span className="muted">
              Create a pending transfer to {shortId(target)}? The source Agent stays
              authoritative until the target declaration is validated.
            </span>
            <button
              type="button"
              className="primary-action"
              onClick={() => void confirm()}
              disabled={busy || !targetKnown}
            >
              {busy ? 'Starting…' : 'Confirm transfer'}
            </button>
            <button type="button" className="secondary-action" onClick={() => setConfirming(false)}>
              Keep editing
            </button>
          </div>
        ) : (
          <div className="action-row">
            <button type="submit" className="primary-action" disabled={!targetKnown}>
              Review transfer
            </button>
          </div>
        )}
      </form>
      {error && (
        <p className="form-error" role="alert">
          {error.message}
          {error.requestId ? (
            <>
              {' '}
              · Request <code>{error.requestId}</code>
            </>
          ) : null}
          {error.auditReference ? (
            <>
              {' '}
              · Audit <code>{error.auditReference}</code>
            </>
          ) : null}
        </p>
      )}
    </article>
  )
}

/** Success receipt of a created Transfer (issue #37: successful actions
 * expose the Server receipt/revision, request ID, and Audit reference). It
 * stays visible while the authoritative pending panel takes over after the
 * refetch. */
function TransferCreatedPanel({
  created,
}: {
  created: { transfer: NodeTransfer; requestId: string; auditEventId: number }
}) {
  return (
    <article className="panel">
      <div className="panel-heading">
        <h2>Transfer created</h2>
        <StatusBadge
          status={transferBadge(created.transfer.status).label}
          tone={transferBadge(created.transfer.status).tone}
        />
      </div>
      <p className="form-success" role="status">
        Transfer {shortId(created.transfer.transfer_id)} is pending. The source Agent stays
        authoritative until the target Agent declares this Node ID and the Server validates
        its Network Identity.
      </p>
      <dl className="detail-list">
        <div>
          <dt>Target Agent</dt>
          <dd>{shortId(created.transfer.target_agent_id)}</dd>
        </div>
        <div>
          <dt>Expires</dt>
          <dd>
            {formatObservedAt(created.transfer.expires_at)} (Server-authoritative, never
            auto-extends)
          </dd>
        </div>
        <div>
          <dt>Request</dt>
          <dd>
            <code>{created.requestId}</code>
          </dd>
        </div>
        <div>
          <dt>Audit event</dt>
          <dd>
            <code>#{created.auditEventId}</code>
          </dd>
        </div>
      </dl>
      <p className="panel-copy">
        Configure the target Agent to declare Node ID <code>{created.transfer.node_id}</code> in
        its local TOML and submit a new Inventory. The transfer completes automatically once
        the declared Network Identity matches the Registry.
      </p>
    </article>
  )
}

/** Active handover: source stays authoritative, expiry is visible, and the
 * Owner can cancel while pending. */
function PendingTransferPanel({
  transfer,
  nodeId,
  sourceAgentId,
  csrfToken,
  onCancelled,
}: {
  transfer: NodeTransfer
  nodeId: string
  sourceAgentId: string
  csrfToken: string
  onCancelled: (message: string) => void
}) {
  const badge = transferBadge(transfer.status)
  const [confirming, setConfirming] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<{
    message: string
    requestId: string | null
    auditReference: string | null
  } | null>(null)

  async function confirmCancel() {
    setConfirming(false)
    setBusy(true)
    setError(null)
    try {
      const result = await cancelNodeTransfer(transfer.transfer_id, csrfToken)
      onCancelled(
        `Transfer ${shortId(result.transfer.transfer_id)} cancelled · Audit #${result.audit_event_id} · Request ${result.request_id}`,
      )
    } catch (caught) {
      const apiError = caught instanceof AdminApiError ? caught : null
      setError({
        message: caught instanceof Error ? caught.message : 'Unable to cancel the transfer',
        requestId: apiError?.requestId ?? null,
        auditReference:
          apiError?.fields.find((field) => field.startsWith('audit_event_id:')) ?? null,
      })
    } finally {
      setBusy(false)
    }
  }

  return (
    <article className="panel">
      <div className="panel-heading">
        <h2>Pending transfer</h2>
        <StatusBadge status={badge.label} tone={badge.tone} />
      </div>
      <p className="panel-copy" role="status">
        The source Agent remains the only authoritative owner while this transfer is pending.
        Ownership switches atomically only after the target declares the Node ID and the
        Server validates the Network Identity.
      </p>
      <dl className="detail-list">
        <div>
          <dt>Target Agent</dt>
          <dd>{shortId(transfer.target_agent_id)}</dd>
        </div>
        <div>
          <dt>Source Agent</dt>
          <dd>{shortId(sourceAgentId)}</dd>
        </div>
        <div>
          <dt>Created</dt>
          <dd>{formatObservedAt(transfer.created_at)}</dd>
        </div>
        <div>
          <dt>Expires</dt>
          <dd>{formatObservedAt(transfer.expires_at)}</dd>
        </div>
        <div>
          <dt>Operator reason</dt>
          <dd>{transfer.operator_reason ?? 'None'}</dd>
        </div>
      </dl>
      <div className="action-row">
        {confirming ? (
          <>
            <span className="muted">
              Cancel this transfer? Ownership stays with the source Agent and the outcome is
              recorded in Audit.
            </span>
            <button
              type="button"
              className="primary-action"
              onClick={() => void confirmCancel()}
              disabled={busy}
            >
              {busy ? 'Cancelling…' : 'Confirm cancel'}
            </button>
            <button type="button" className="secondary-action" onClick={() => setConfirming(false)}>
              Keep transfer
            </button>
          </>
        ) : (
          <button type="button" className="secondary-action" onClick={() => setConfirming(true)}>
            Cancel transfer
          </button>
        )}
      </div>
      {error && (
        <p className="form-error" role="alert">
          {error.message}
          {error.requestId ? (
            <>
              {' '}
              · Request <code>{error.requestId}</code>
            </>
          ) : null}
          {error.auditReference ? (
            <>
              {' '}
              · Audit <code>{error.auditReference}</code>
            </>
          ) : null}
        </p>
      )}
      <p className="panel-copy">
        Instructions for the operator:{' '}
        <code>{nodeId}</code> must be declared in the target Agent's local configuration;
        the Server never pushes an RPC Endpoint or command to either Agent.
      </p>
    </article>
  )
}

/** Typed, auditable timeline of every Transfer outcome for this Node. A
 * Network Identity Mismatch is a blocking diagnostic: no ownership switch,
 * no history merge, source remains authoritative. */
function TransferTimeline({
  transfers,
  nodeId,
}: {
  transfers: NodeTransfer[]
  nodeId: string
}) {
  return (
    <article className="panel">
      <div className="panel-heading">
        <h2>Transfer history</h2>
        <span className="panel-count">
          {transfers.length === 0 ? 'No transfers yet' : `${transfers.length} outcome${transfers.length === 1 ? '' : 's'}`}
        </span>
      </div>
      {transfers.length === 0 ? (
        <p className="panel-state">
          <StatusBadge status="Empty" tone="ok" /> This Node has never been transferred.
        </p>
      ) : (
        <ol className="transfer-timeline">
          {transfers.map((transfer) => (
            <TransferTimelineEntry key={transfer.transfer_id} transfer={transfer} nodeId={nodeId} />
          ))}
        </ol>
      )}
    </article>
  )
}

function TransferTimelineEntry({ transfer, nodeId }: { transfer: NodeTransfer; nodeId: string }) {
  const badge = transferBadge(transfer.status)
  const identityMismatch = transfer.status === 'identity_mismatch'
  const conflict = transfer.status === 'conflict'
  return (
    <li className="transfer-entry">
      <div className="transfer-entry-heading">
        <StatusBadge status={badge.label} tone={badge.tone} />
        <span className="muted">
          {shortId(transfer.transfer_id)} · {shortId(transfer.source_agent_id)} →{' '}
          {shortId(transfer.target_agent_id)}
        </span>
      </div>
      <dl className="detail-list">
        <div>
          <dt>Created</dt>
          <dd>{formatObservedAt(transfer.created_at)}</dd>
        </div>
        <div>
          <dt>Expires</dt>
          <dd>{formatObservedAt(transfer.expires_at)}</dd>
        </div>
        {transfer.completed_at && (
          <div>
            <dt>Completed</dt>
            <dd>{formatObservedAt(transfer.completed_at)}</dd>
          </div>
        )}
        {transfer.cancelled_at && (
          <div>
            <dt>Cancelled</dt>
            <dd>{formatObservedAt(transfer.cancelled_at)}</dd>
          </div>
        )}
        {transfer.operator_reason && (
          <div>
            <dt>Operator reason</dt>
            <dd>{transfer.operator_reason}</dd>
          </div>
        )}
        {transfer.rejection_reason && (
          <div>
            <dt>Rejection</dt>
            <dd>
              <code>{transfer.rejection_code ?? 'rejected'}</code> — {transfer.rejection_reason}
            </dd>
          </div>
        )}
        {transfer.mismatched_fields.length > 0 && (
          <div>
            <dt>Mismatched fields</dt>
            <dd>{transfer.mismatched_fields.join(', ')}</dd>
          </div>
        )}
      </dl>
      {identityMismatch && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Identity mismatch" tone="error" /> Blocking diagnostic: the
          target-declared Network Identity contradicts the registered Network, so ownership
          never switched and no new block history was merged into the registered Network
          history. Verify the Node was re-pointed to the correct chain before retrying.
        </p>
      )}
      {conflict && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Conflict" tone="error" /> This attempt was refused because a
          transfer for Node <code>{shortId(nodeId)}</code> was already pending. Source ownership
          is unchanged.
        </p>
      )}
      {transfer.status === 'completed' && (
        <p className="panel-state">
          <StatusBadge status="Completed" tone="ok" /> Ownership switched atomically; the Node
          ID, Network, history, and visibility are unchanged.
        </p>
      )}
    </li>
  )
}
