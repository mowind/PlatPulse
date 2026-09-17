import { useEffect, useState, type KeyboardEvent } from 'react'
import { Link } from 'react-router'
import { useAdminAudit, type AuditFilters } from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { StatusBadge, formatObservedAt } from '../components/StatusBadge'
import { Badge } from '../components/ui/badge'
import { Button } from '../components/ui/button'
import { CardX } from '../components/ui/card-x'
import { DataTooltip } from '../components/ui/data-tooltip'
import { Empty } from '../components/ui/empty'
import { Select } from '../components/ui/input'
import { cn } from '../lib/utils'
import { SURFACE_CARD } from '../lib/surface'
import type { AuditItem } from '../api/generated'

const TH = 'px-3 py-2 text-left text-xs font-medium text-muted-foreground'
const TD = 'px-3 py-2 align-top'

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
    <section className="w-full" data-slot="audit-page">
      <header className="flex flex-col gap-1">
        <h1 className="text-lg font-semibold">Audit log</h1>
        <p className="text-sm text-muted-foreground">
          Immutable, redacted record of administrative and security mutations.
          Events cannot be edited or deleted from the UI.
        </p>
      </header>
      {error && (
        <p
          role="alert"
          className="mt-4 rounded-md border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm text-destructive"
        >
          {error}
        </p>
      )}
      <div className="mt-4 grid gap-3 sm:max-w-xl sm:grid-cols-2" data-slot="audit-filters">
        <div className="flex flex-col gap-1.5">
          <label
            htmlFor="audit-event-kind"
            className="text-xs font-medium tracking-wider text-muted-foreground"
          >
            Event kind
          </label>
          <Select
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
          </Select>
        </div>
        <div className="flex flex-col gap-1.5">
          <label
            htmlFor="audit-target-kind"
            className="text-xs font-medium tracking-wider text-muted-foreground"
          >
            Target
          </label>
          <Select
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
          </Select>
        </div>
      </div>
      <CardX
        size="medium"
        bordered={false}
        segmented
        data-slot="audit-panel"
        className={cn('mt-4 rounded-md', SURFACE_CARD)}
        header={
          <div className="flex w-full items-center gap-2">
            <h2 className="min-w-0 flex-1 truncate text-sm font-medium">Events</h2>
            <DataTooltip
              placement="left"
              content="Detail bodies are the stored redacted after-bodies: ids, instants, and counts only. Events are append-only and can never be edited or deleted from the UI."
            >
              <Button variant="ghost" size="icon-sm" aria-label="About redacted Audit details">
                ?
              </Button>
            </DataTooltip>
            {query.data && (
              <Badge variant="secondary" data-slot="audit-count">
                {items.length}
              </Badge>
            )}
          </div>
        }
      >
        {!query.data && query.isPending && (
          <div role="status" className="flex items-center gap-2 py-6 text-sm text-muted-foreground">
            <StatusBadge status="Starting" tone="neutral" /> Loading the Audit log…
          </div>
        )}
        {!query.data && query.isError && (
          <div role="alert" className="flex flex-wrap items-center gap-2 py-6 text-sm">
            <StatusBadge status="Error" tone="error" />
            <span className="text-destructive">
              {query.error instanceof Error ? query.error.message : 'Unable to load the Audit log'}
            </span>
            <Button variant="ghost" size="sm" onClick={() => void query.refetch()}>
              Try again
            </Button>
          </div>
        )}
        {query.data && items.length === 0 && <Empty description="No matching Audit events." />}
        {items.length > 0 && (
          <div data-slot="audit-list" className="overflow-x-auto">
            <table data-stack data-slot="audit-table" className="w-full min-w-[44rem] text-sm">
              <caption className="sr-only">
                Immutable redacted Audit events with time, event, actor, target, and details
              </caption>
              <thead>
                <tr className="border-b border-border">
                  <th scope="col" className={TH}>
                    Time
                  </th>
                  <th scope="col" className={TH}>
                    Event
                  </th>
                  <th scope="col" className={TH}>
                    Actor
                  </th>
                  <th scope="col" className={TH}>
                    Target
                  </th>
                  <th scope="col" className={TH}>
                    Details
                  </th>
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
          <Button variant="ghost" size="sm" className="mt-3" onClick={loadOlder}>
            Load older events
          </Button>
        )}
      </CardX>
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
      <tr className="border-b border-border/60 align-top">
        <td className={cn(TD, 'whitespace-nowrap')} data-label="Time">
          <time dateTime={item.createdAt}>{formatObservedAt(item.createdAt)}</time>
        </td>
        <th scope="row" className={cn(TD, 'text-left font-medium')} data-label="Event">
          <Badge variant="secondary" className="font-mono">
            {item.eventKind}
          </Badge>
          <small className="mt-0.5 block text-[11px] text-muted-foreground">
            event #{item.auditEventId}
          </small>
        </th>
        <td className={TD} data-label="Actor">
          {item.actorUsername ?? 'local-cli'}
        </td>
        <td className={TD} data-label="Target">
          {target ? (
            <Link
              to={target.to}
              className="inline-flex min-h-11 min-w-11 max-w-full items-center break-words"
            >
              {target.label}
            </Link>
          ) : (
            item.targetId
          )}
          <small className="mt-0.5 block text-[11px] text-muted-foreground">
            {item.targetKind}
          </small>
        </td>
        <td className={TD} data-label="Details">
          <Button
            variant="ghost"
            size="sm"
            data-slot="audit-details-toggle"
            onClick={onToggle}
            onKeyDown={collapseOnEscape}
            aria-expanded={expanded}
            aria-controls={detailsId}
          >
            {expanded ? 'Hide details' : 'Show details'}
          </Button>
        </td>
      </tr>
      {expanded && (
        <tr data-slot="audit-detail-row" className="border-b border-border/60">
          <td colSpan={5} id={detailsId} className="p-0" onKeyDown={collapseOnEscape}>
            <div
              data-slot="audit-details"
              className="p-3"
              role="region"
              aria-label={'Redacted details for Audit event ' + item.auditEventId}
            >
              {item.details == null ? (
                <p className="text-sm text-muted-foreground">
                  No redacted detail was recorded for this event.
                </p>
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
    return <p className="text-sm text-muted-foreground">{JSON.stringify(details)}</p>
  }
  return (
    <dl className="grid gap-1">
      {Object.entries(details as Record<string, unknown>).map(([key, value]) => (
        <div key={key} className="grid gap-0.5 sm:grid-cols-[minmax(8rem,auto)_minmax(0,1fr)] sm:gap-2">
          <dt className="text-xs font-medium tracking-wider text-muted-foreground [overflow-wrap:anywhere]">
            {key}
          </dt>
          <dd className="min-w-0 text-sm [overflow-wrap:anywhere]">
            {typeof value === 'object' ? JSON.stringify(value) : String(value)}
          </dd>
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
