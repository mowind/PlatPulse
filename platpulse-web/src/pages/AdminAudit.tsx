import { useEffect, useRef, useState, type KeyboardEvent } from 'react'
import { Link, useLocation } from 'react-router'
import { useAdminAudit, type AuditFilters } from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import {
  ADMIN_DETAIL_BODY,
  ADMIN_DETAIL_ROW,
  ADMIN_ROW,
  ADMIN_SECONDARY,
  ADMIN_TD,
  ADMIN_TH,
  AdminPageHeader,
  AdminPanel,
  AdminToolbar,
  AdminToolbarField,
} from '../components/AdminPage'
import { StatusBadge, formatObservedAt } from '../components/StatusBadge'
import { Badge } from '../components/ui/badge'
import { Button } from '../components/ui/button'
import { Empty } from '../components/ui/empty'
import { Select } from '../components/ui/input'
import { cn } from '../lib/utils'
import type { AuditItem } from '../api/generated'

/** `#event-13` deep link used by the recorded Operation pages (issue #208). */
function deepLinkEventId(hash: string): number | null {
  const match = /^#event-(\d+)$/.exec(hash)
  return match ? Number.parseInt(match[1], 10) : null
}

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
  const location = useLocation()
  const deepLinkId = deepLinkEventId(location.hash)
  const [deepLinkMissing, setDeepLinkMissing] = useState(false)
  const itemsRef = useRef(items)
  itemsRef.current = items

  // A recorded Operation links to its Audit event with `#event-<id>`. The
  // Audit log is loaded one page at a time, so a linked event is expanded and
  // scrolled to when it is inside the loaded window, and otherwise the Owner
  // is told plainly that the window does not reach it (issue #208).
  const itemIds = items.map((item) => item.auditEventId).join(',')
  useEffect(() => {
    if (deepLinkId == null || itemsRef.current.length === 0) return
    if (itemsRef.current.some((item) => item.auditEventId === deepLinkId)) {
      setDeepLinkMissing(false)
      setExpanded((current) => (current.has(deepLinkId) ? current : new Set(current).add(deepLinkId)))
      document.getElementById('event-' + deepLinkId)?.scrollIntoView({ block: 'center' })
      return
    }
    setDeepLinkMissing(true)
  }, [deepLinkId, itemIds])

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
    <section className="w-full min-w-0 space-y-4" data-slot="audit-page">
      <AdminPageHeader
        title="Audit log"
        description="Immutable, redacted record of administrative and security mutations."
        info={{
          label: 'About redacted Audit details',
          content: (
            <>
              <p>Events are append-only and cannot be edited or deleted from the UI.</p>
              <p>
                Detail bodies are the stored redacted after-bodies: ids, instants, and counts only.
              </p>
            </>
          ),
        }}
      />
      {error && (
        <p
          role="alert"
          className="rounded-md border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm text-destructive"
        >
          {error}
        </p>
      )}
      <AdminPanel
        slot="audit-panel"
        flush
        title="Events"
        count={
          query.data ? (
            <Badge variant="secondary" data-slot="audit-count" className="tabular-nums">
              {items.length} loaded
            </Badge>
          ) : undefined
        }
      >
        <div className="border-b border-border/60 px-4 py-3">
          <AdminToolbar label="Audit filters" className="sm:flex-nowrap">
            <div data-slot="audit-filters" className="contents">
              <AdminToolbarField label="Event kind" htmlFor="audit-event-kind" size="md">
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
              </AdminToolbarField>
              <AdminToolbarField label="Target" htmlFor="audit-target-kind" size="sm">
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
              </AdminToolbarField>
            </div>
          </AdminToolbar>
        </div>
        {deepLinkMissing && deepLinkId != null && (
          <p role="status" className="border-b border-border/60 px-4 py-2 text-sm text-muted-foreground">
            Event {deepLinkId} is older than the events loaded here. Load older events until it appears; the
            Audit log is append-only and never trimmed by this page.
          </p>
        )}
        {!query.data && query.isPending && (
          <div role="status" className="flex items-center gap-2 px-4 py-6 text-sm text-muted-foreground">
            <StatusBadge status="Starting" tone="neutral" /> Loading the Audit log…
          </div>
        )}
        {!query.data && query.isError && (
          <div role="alert" className="flex flex-wrap items-center gap-2 px-4 py-6 text-sm">
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
          <div data-slot="audit-list" className="overflow-x-auto max-md:px-3 max-md:pt-3">
            <table data-stack data-slot="audit-table" className="w-full text-sm md:min-w-[44rem]">
              <caption className="sr-only">
                Immutable redacted Audit events with time, event, actor, target, and details
              </caption>
              <thead>
                <tr className="border-b border-border/60">
                  <th scope="col" className={ADMIN_TH}>
                    Time
                  </th>
                  <th scope="col" className={ADMIN_TH}>
                    Event
                  </th>
                  <th scope="col" className={ADMIN_TH}>
                    Actor
                  </th>
                  <th scope="col" className={ADMIN_TH}>
                    Target
                  </th>
                  <th scope="col" className={ADMIN_TH}>
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
          <div className="px-4 py-2">
            <Button variant="ghost" size="sm" onClick={loadOlder}>
              Load older events
            </Button>
          </div>
        )}
      </AdminPanel>
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
      <tr id={'event-' + item.auditEventId} className={ADMIN_ROW}>
        <td className={cn(ADMIN_TD, 'tabular-nums md:whitespace-nowrap')} data-label="Time">
          <time dateTime={item.createdAt}>{formatObservedAt(item.createdAt)}</time>
        </td>
        <th scope="row" className={cn(ADMIN_TD, 'text-left font-normal')} data-label="Event">
          <Badge variant="secondary" className="font-mono">
            {item.eventKind}
          </Badge>
          <small className={cn('mt-0.5 block tabular-nums', ADMIN_SECONDARY)}>
            event #{item.auditEventId}
          </small>
        </th>
        <td className={ADMIN_TD} data-label="Actor">
          {item.actorUsername ?? 'local-cli'}
        </td>
        <td className={ADMIN_TD} data-label="Target">
          {target ? (
            <Link
              to={target.to}
              className="inline-flex min-h-11 md:-my-2.5 min-w-11 max-w-full items-center underline-offset-4 [overflow-wrap:anywhere] hover:underline"
            >
              {target.label}
            </Link>
          ) : (
            item.targetId
          )}
          <small className={cn('block', ADMIN_SECONDARY)}>
            {item.targetKind}
          </small>
        </td>
        <td className={ADMIN_TD} data-label="Details">
          <Button
            variant="ghost"
            size="sm"
            className="md:-my-1.5 md:-ml-3"
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
        <tr data-slot="audit-detail-row" className={ADMIN_DETAIL_ROW}>
          <td colSpan={5} id={detailsId} className="p-0" onKeyDown={collapseOnEscape}>
            <div
              data-slot="audit-details"
              className={ADMIN_DETAIL_BODY}
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
          <dt className="text-xs font-medium text-muted-foreground [overflow-wrap:anywhere]">
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
