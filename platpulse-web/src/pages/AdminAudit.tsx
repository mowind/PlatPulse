import { useEffect, useState, type KeyboardEvent } from 'react'
import { Link } from 'react-router'
import { useAdminAudit, type AuditFilters } from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { StatusBadge, formatObservedAt } from '../components/StatusBadge'
import type { AuditItem } from '../api/generated'

/**
 * PAGE-ACCESS-AUDIT (design §18.2, issue #47): immutable, redacted Audit
 * review with filtering and detail/link navigation. Events are Server-owned
 * and append-only; the WebUI can never mutate, reopen, or delete them.
 * Details are the stored redacted `after` bodies (ids, instants, and counts
 * only) — passwords, tokens, credentials, endpoints, raw peer IPs, and
 * complete request bodies are never written by the Server and therefore
 * never shown here.
 */
export default function AdminAudit() {
  const { generation } = useAuth()
  const [eventKind, setEventKind] = useState('')
  const [targetKind, setTargetKind] = useState('')
  const [before, setBefore] = useState<number | undefined>(undefined)
  const filters: AuditFilters = {
    eventKind: eventKind || undefined,
    targetKind: targetKind || undefined,
    before,
  }
  const query = useAdminAudit(generation, filters)
  const filterKey = `${eventKind}\u0000${targetKind}`
  const [history, setHistory] = useState<{ filterKey: string; items: AuditItem[] }>({
    filterKey,
    items: [],
  })
  const [expanded, setExpanded] = useState<Set<number>>(new Set())
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (!query.data || query.isFetching) return
    setHistory((current) => {
      if (current.filterKey !== filterKey || before == null) {
        return { filterKey, items: query.data.items }
      }
      const known = new Set(current.items.map((item) => item.auditEventId))
      return {
        filterKey,
        items: [...current.items, ...query.data.items.filter((item) => !known.has(item.auditEventId))],
      }
    })
  }, [before, filterKey, query.data, query.isFetching])

  const items = history.filterKey === filterKey ? history.items : []

  function loadOlder() {
    setError(null)
    if (query.data?.nextBefore != null) setBefore(query.data.nextBefore)
  }

  const toggle = (id: number) => {
    setExpanded((current) => {
      const next = new Set(current)
      if (next.has(id)) {
        next.delete(id)
      } else {
        next.add(id)
      }
      return next
    })
  }

  return (
    <section className="page">
      <h1>Audit log</h1>
      <p className="muted">
        Immutable, redacted record of administrative and security mutations.
        Events cannot be edited or deleted from the UI.
      </p>
      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}
      <div className="audit-filters">
        <div className="field">
          <label htmlFor="audit-event-kind">Event kind</label>
          <select
            id="audit-event-kind"
            value={eventKind}
            onChange={(event) => {
              setEventKind(event.target.value)
              setBefore(undefined)
              setError(null)
            }}
          >
            <option value="">All kinds</option>
            {EVENT_KINDS.map((kind) => (
              <option key={kind} value={kind}>
                {kind}
              </option>
            ))}
          </select>
        </div>
        <div className="field">
          <label htmlFor="audit-target-kind">Target</label>
          <select
            id="audit-target-kind"
            value={targetKind}
            onChange={(event) => {
              setTargetKind(event.target.value)
              setBefore(undefined)
              setError(null)
            }}
          >
            <option value="">All targets</option>
            <option value="user">User</option>
            <option value="agent">Agent</option>
            <option value="node">Node</option>
            <option value="network">Network</option>
            <option value="session">Session</option>
            <option value="access">Access</option>
          </select>
        </div>
      </div>
      <article className="panel">
        <div className="panel-heading">
          <h2>Events</h2>
          {query.data && <span className="panel-count">{items.length}</span>}
        </div>
        {!query.data && query.isPending && (
          <p className="panel-state" role="status">
            <StatusBadge status="Starting" tone="neutral" /> Loading the Audit log…
          </p>
        )}
        {!query.data && query.isError && (
          <p className="panel-state" role="alert">
            <StatusBadge status="Error" tone="error" />{' '}
            {query.error instanceof Error ? query.error.message : 'Unable to load the Audit log'}
            <button type="button" className="text-action" onClick={() => void query.refetch()}>
              Try again
            </button>
          </p>
        )}
        {query.data && items.length === 0 && (
          <p className="panel-state">
            <StatusBadge status="Empty" tone="ok" /> No matching Audit events.
          </p>
        )}
        {items.length > 0 && (
          <div className="audit-list audit-table-wrap">
            <table className="audit-table table-static">
              <caption className="sr-only">
                Immutable redacted Audit events with time, event, actor, target, and details
              </caption>
              <thead>
                <tr>
                  <th scope="col">Time</th>
                  <th scope="col">Event</th>
                  <th scope="col">Actor</th>
                  <th scope="col">Target</th>
                  <th scope="col">Details</th>
                </tr>
              </thead>
              <tbody>
                {items.map((item) => (
                  <AuditRow
                    key={item.auditEventId}
                    item={item}
                    expanded={expanded.has(item.auditEventId)}
                    onToggle={() => toggle(item.auditEventId)}
                  />
                ))}
              </tbody>
            </table>
          </div>
        )}
        {query.data?.nextBefore != null && (
          <button type="button" className="text-action" onClick={loadOlder}>
            Load older events
          </button>
        )}
      </article>
    </section>
  )
}

/** Known event kinds for the filter. The listing itself always shows the
 * Server's exact `event_kind` strings, including kinds not listed here. */
const EVENT_KINDS = [
  'owner_created',
  'viewer_created',
  'user_role_changed',
  'user_disabled',
  'user_enabled',
  'password_reset',
  'session_created',
  'session_revoked',
  'sessions_revoked',
  'login_failed',
  'guest_access_changed',
  'logout',
  'network_created',
  'network_updated',
  'agent_enrolled',
  'agent_recovered',
  'agent_credential_rotated',
  'agent_credential_revoked',
  'enrollment_token_created',
  'recovery_token_created',
  'node_visibility_changed',
  'node_metadata_changed',
  'node_transfer_created',
  'node_transfer_cancelled',
]

/** PAGE-ACCESS-AUDIT row: the five summary columns stay in one table row;
 * the redacted detail opens in its own full-width row below the record, the
 * same disclosure pattern as the Nodes inventory. Long values therefore wrap
 * inside the detail row instead of widening or crushing the action column. */
function AuditRow({
  item,
  expanded,
  onToggle,
}: {
  item: AuditItem
  expanded: boolean
  onToggle: () => void
}) {
  const target = targetLink(item)
  const detailsId = `audit-details-${item.auditEventId}`
  const collapseOnEscape = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key === 'Escape' && expanded) onToggle()
  }
  return (
    <>
      <tr>
        <td data-label="Time">
          <time dateTime={item.createdAt}>{formatObservedAt(item.createdAt)}</time>
        </td>
        <th scope="row" data-label="Event">
          <span className="audit-event-kind">{item.eventKind}</span>
          <small className="muted">event #{item.auditEventId}</small>
        </th>
        <td data-label="Actor">{item.actorUsername ?? 'local-cli'}</td>
        <td data-label="Target">
          {target ? <Link to={target.to}>{target.label}</Link> : item.targetId}
          <small className="muted">{item.targetKind}</small>
        </td>
        <td data-label="Details">
          <button
            type="button"
            className="text-action audit-details-toggle"
            onClick={onToggle}
            onKeyDown={collapseOnEscape}
            aria-expanded={expanded}
            aria-controls={detailsId}
          >
            {expanded ? 'Hide details' : 'Show details'}
          </button>
        </td>
      </tr>
      {expanded && (
        <tr className="node-detail-row">
          <td colSpan={5} id={detailsId} onKeyDown={collapseOnEscape}>
            <div
              className="audit-details"
              role="region"
              aria-label={'Redacted details for Audit event ' + item.auditEventId}
            >
              {item.details == null ? (
                <p className="muted">No redacted detail was recorded for this event.</p>
              ) : (
                <RedactedDetails details={item.details} />
              )}
            </div>
          </td>
        </tr>
      )}
    </>
  )
}

/** Details are redacted by construction; render them as a flat key/value
 * list so a screen reader can navigate them without a wide table. */
function RedactedDetails({ details }: { details: unknown }) {
  if (typeof details !== 'object' || details === null || Array.isArray(details)) {
    return <p className="muted">{JSON.stringify(details)}</p>
  }
  return (
    <dl className="detail-list">
      {Object.entries(details as Record<string, unknown>).map(([key, value]) => (
        <div key={key}>
          <dt>{key}</dt>
          <dd>{typeof value === 'object' ? JSON.stringify(value) : String(value)}</dd>
        </div>
      ))}
    </dl>
  )
}

/** Link an Audit target to its Admin page when one exists; unknown or
 * unlinkable targets stay plain redacted text. */
function targetLink(item: AuditItem): { to: string; label: string } | null {
  switch (item.targetKind) {
    case 'agent':
      return { to: `/admin/agents/${encodeURIComponent(item.targetId)}`, label: item.targetId }
    case 'user':
      return { to: '/admin/access/people', label: item.targetId }
    case 'session':
      return { to: '/admin/access/sessions', label: item.targetId }
    case 'node':
      return { to: `/admin/nodes/${encodeURIComponent(item.targetId)}`, label: item.targetId }
    case 'network':
      return { to: `/admin/networks/${encodeURIComponent(item.targetId)}`, label: item.targetId }
    default:
      return null
  }
}
