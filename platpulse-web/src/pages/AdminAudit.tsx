import { useEffect, useRef, useState, type KeyboardEvent, type MouseEvent } from 'react'
import { LoaderCircle } from 'lucide-react'
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
import { auditQueryState } from '../auditQueryState'

/** `#event-13` deep link used by the recorded Operation pages (issue #208). */
function deepLinkEventId(hash: string): number | null {
  const match = /^#event-(\d+)$/.exec(hash)
  return match ? Number.parseInt(match[1], 10) : null
}

/** The merged Audit view for one filter selection: the pages loaded so far,
 *  newest first, and the Server's cursor for the next older page. */
type AuditHistory = {
  filterKey: string
  items: AuditItem[]
  nextBefore: number | null
  /** Whether at least one page for this selection has arrived. */
  loaded: boolean
}

/** What the page can truthfully say about a linked event it is not showing.
 *  It never claims the event is older, missing or deleted: it only knows the
 *  event is not among the loaded results, and why that set may be partial. */
export function auditDeepLinkNotice(state: {
  id: number
  loaded: boolean
  failed: boolean
  count: number
  filtered: boolean
  more: boolean
  olderLoading: boolean
  olderFailed: boolean
}): string {
  const subject = 'Event ' + state.id
  if (!state.loaded) {
    if (state.failed) return subject + ' cannot be looked for: the Audit log failed to load.'
    return 'Looking for event ' + state.id + ' in the Audit log…'
  }
  if (state.count === 0) {
    return state.filtered
      ? subject + ' is not in the current results: no events match the current filters. Changing the filters may bring it into view.'
      : subject + ' is not in the current results: the Audit log returned no events.'
  }
  const scope = state.filtered
    ? state.more
      ? 'It may be excluded by the current filters or not be loaded yet.'
      : 'It may be excluded by the current filters; no further events are available for them.'
    : state.more
      ? 'It may not be loaded yet.'
      : 'No more pages are available to load here.'
  const progress = state.olderLoading
    ? ' Loading older events…'
    : state.olderFailed
      ? ' Loading older events failed; try again below.'
      : state.more
        ? ' Load older events below to keep looking.'
        : ''
  return subject + ' is not in the ' + state.count + ' loaded event' + (state.count === 1 ? '' : 's') + '. ' + scope + progress
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
  // Each cursor is its own query; the page merges the pages it has loaded.
  const query = useAdminAudit(generation, filters)
  const filterKey = `${eventKind}\u0000${targetKind}`
  const [history, setHistory] = useState<AuditHistory>({
    filterKey,
    items: [],
    nextBefore: null,
    loaded: false,
  })
  const [expanded, setExpanded] = useState<Set<number>>(new Set())

  useEffect(() => {
    const page = query.data
    if (!page) return
    setHistory((current) => {
      // The newest page (no cursor) or another filter selection starts over.
      if (current.filterKey !== filterKey || before == null) {
        return { filterKey, items: page.items, nextBefore: page.nextBefore ?? null, loaded: true }
      }
      // An older page appends; an event already listed is never repeated.
      const known = new Set(current.items.map((item) => item.auditEventId))
      const added = page.items.filter((item) => !known.has(item.auditEventId))
      return {
        filterKey,
        items: added.length > 0 ? [...current.items, ...added] : current.items,
        nextBefore: page.nextBefore ?? null,
        loaded: true,
      }
    })
  }, [before, filterKey, query.data])

  // A retry belongs to the exact query it was pressed for, and ends once that
  // query has succeeded.
  const queryIdentity = [generation, filterKey, before ?? ''].join('\u0001')
  const [retryFor, setRetryFor] = useState<string | null>(null)
  if (retryFor !== null && (retryFor !== queryIdentity || (query.isSuccess && !query.isFetching))) setRetryFor(null)

  const settled = history.filterKey === filterKey && history.loaded
  const items = settled ? history.items : []
  const nextBefore = settled ? history.nextBefore : null
  const filtered = eventKind !== '' || targetKind !== ''
  const state = auditQueryState({
    settled,
    appending: before != null,
    hasData: query.data !== undefined,
    isError: query.isError,
    isFetching: query.isFetching,
    retrying: retryFor === queryIdentity,
  })
  const olderLoading = state === 'older-loading'
  const olderFailed = state === 'older-failed'
  const initialFailed = state === 'initial-failed'
  // Without a closing period, so the sentences below never end in "..".
  const errorMessage = (query.error instanceof Error ? query.error.message : 'Unable to load the Audit log').replace(/[.\s]+$/, '')
  const keptEvents =
    items.length === 1 ? 'Showing the event loaded before.' : 'Showing the ' + items.length + ' events loaded before.'

  const location = useLocation()
  const deepLinkId = deepLinkEventId(location.hash)
  const itemsRef = useRef(items)
  itemsRef.current = items
  const deepLinkFound = deepLinkId != null && items.some((item) => item.auditEventId === deepLinkId)
  // A linked event is opened and scrolled to once, when it first appears;
  // a later older page must not pull the reader back to it.
  const revealedDeepLink = useRef<number | null>(null)
  const itemIds = items.map((item) => item.auditEventId).join(',')
  useEffect(() => {
    if (deepLinkId == null) {
      revealedDeepLink.current = null
      return
    }
    if (!itemsRef.current.some((item) => item.auditEventId === deepLinkId)) {
      // Left the loaded results (another filter): reveal it again when it returns.
      revealedDeepLink.current = null
      return
    }
    if (revealedDeepLink.current === deepLinkId) return
    revealedDeepLink.current = deepLinkId
    setExpanded((current) => (current.has(deepLinkId) ? current : new Set(current).add(deepLinkId)))
    const target = document.getElementById('event-' + deepLinkId)
    if (target && typeof target.scrollIntoView === 'function') target.scrollIntoView({ block: 'center' })
  }, [deepLinkId, itemIds])

  const olderHadFocus = useRef(false)
  const endNote = useRef<HTMLParagraphElement>(null)
  // When the last older page arrives, the button it was pressed on goes away;
  // focus moves to the note that says so instead of falling to the page.
  useEffect(() => {
    if (nextBefore == null && olderHadFocus.current) {
      olderHadFocus.current = false
      endNote.current?.focus()
    }
  }, [nextBefore])

  function loadOlder() {
    if (olderLoading) return
    // A failed page is retried with its own cursor, never skipped.
    if (olderFailed) {
      void query.refetch()
      return
    }
    if (nextBefore != null) setBefore(nextBefore)
  }

  // A retry asks for the failed query again (its own filters and cursor) and
  // is ignored while that request is still running, so repeat presses never
  // cancel and restart it.
  const retryHadFocus = useRef(false)
  const tableRef = useRef<HTMLTableElement>(null)
  const emptyRef = useRef<HTMLDivElement>(null)
  function retry(event: MouseEvent<HTMLButtonElement>) {
    if (query.isFetching) return
    retryHadFocus.current = document.activeElement === event.currentTarget
    setRetryFor(queryIdentity)
    void query.refetch()
  }
  // A retry button leaves once its query succeeds; when it held focus, focus
  // moves to the events it brought back instead of falling to the page.
  const retryPending =
    state === 'initial-failed' || state === 'initial-retrying' || state === 'refresh-failed' || state === 'refresh-retrying'
  useEffect(() => {
    // A retry that failed again leaves focus on its button; a later, unrelated
    // arrival must not move it.
    if (state === 'initial-failed' || state === 'refresh-failed') retryHadFocus.current = false
    // A first page that arrived is merged into the list one render later.
    if (retryPending || state === 'initial-loading' || !retryHadFocus.current) return
    retryHadFocus.current = false
    const active = document.activeElement
    if (active === null || active === document.body) (tableRef.current ?? emptyRef.current)?.focus()
  }, [retryPending, state])

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
      <AdminPanel
        slot="audit-panel"
        flush
        title="Events"
        count={
          settled ? (
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
        {deepLinkId != null && !deepLinkFound && (
          <p
            role="status"
            data-slot="audit-deep-link-notice"
            className="border-b border-border/60 px-4 py-2 text-sm text-muted-foreground"
          >
            {auditDeepLinkNotice({
              id: deepLinkId,
              loaded: settled,
              failed: initialFailed,
              count: items.length,
              filtered,
              more: nextBefore != null,
              olderLoading,
              olderFailed,
            })}
          </p>
        )}
        {state === 'initial-loading' && (
          <div role="status" className="flex items-center gap-2 px-4 py-6 text-sm text-muted-foreground">
            <StatusBadge status="Starting" tone="neutral" /> Loading the Audit log…
          </div>
        )}
        {/* The first page failed: one row that keeps its retry button (and its
            focus) in place while the retry runs. */}
        {(state === 'initial-failed' || state === 'initial-retrying') && (
          <div
            data-slot="audit-initial-error"
            className="flex flex-wrap items-center gap-2 px-4 py-6 text-sm"
            aria-busy={state === 'initial-retrying'}
          >
            {/* Keyed apart so a repeated failure mounts a new alert and is
                announced again. */}
            {state === 'initial-failed' ? (
              <p key="failed" role="alert" className="flex min-w-0 flex-wrap items-center gap-2">
                <StatusBadge status="Error" tone="error" />
                <span className="min-w-0 text-destructive [overflow-wrap:anywhere]">{errorMessage}</span>
              </p>
            ) : (
              <p key="retrying" role="status" className="flex min-w-0 flex-wrap items-center gap-2 text-muted-foreground">
                <StatusBadge status="Starting" tone="neutral" /> Loading the Audit log again…
              </p>
            )}
            <RetryButton
              busy={state === 'initial-retrying'}
              busyLabel="Trying again…"
              label="Try again"
              onClick={retry}
            />
          </div>
        )}
        {/* A refresh of the shown query failed: its earlier successful events
            stay listed below, and only this row says so. */}
        {(state === 'refresh-failed' || state === 'refresh-retrying') && (
          <div
            data-slot="audit-refresh-error"
            className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-border/60 px-4 py-2"
            aria-busy={state === 'refresh-retrying'}
          >
            {state === 'refresh-failed' ? (
              <p key="failed" role="alert" className="flex min-w-0 flex-wrap items-center gap-2 text-sm">
                <StatusBadge status="Error" tone="error" />
                <span className="min-w-0 text-destructive [overflow-wrap:anywhere]">
                  {'Refreshing the Audit log failed: ' + errorMessage + '. ' + keptEvents}
                </span>
              </p>
            ) : (
              <p key="retrying" role="status" className="flex min-w-0 flex-wrap items-center gap-2 text-sm text-muted-foreground">
                <StatusBadge status="Starting" tone="neutral" />
                <span className="min-w-0 [overflow-wrap:anywhere]">{'Refreshing the Audit log again. ' + keptEvents}</span>
              </p>
            )}
            <RetryButton
              busy={state === 'refresh-retrying'}
              busyLabel="Refreshing…"
              label="Try refreshing again"
              onClick={retry}
            />
          </div>
        )}
        {settled && items.length === 0 && (
          <div ref={emptyRef} tabIndex={-1} data-slot="audit-empty" className={RESULT_FOCUS}>
            <Empty description="No matching Audit events." />
          </div>
        )}
        {items.length > 0 && (
          <div data-slot="audit-list" className="overflow-x-auto max-md:px-3 max-md:pt-3">
            <table ref={tableRef} tabIndex={-1} data-stack data-slot="audit-table" className={cn('w-full text-sm md:min-w-[44rem]', RESULT_FOCUS)}>
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
        {settled && items.length > 0 && (nextBefore != null || before != null) && (
          <div
            data-slot="audit-older"
            className="flex flex-wrap items-center gap-x-3 gap-y-1 px-4 py-2"
            aria-busy={olderLoading}
          >
            {/* The button keeps its place (and focus) through loading and
                failure; only its label and the message beside it change. */}
            {olderFailed && (
              <p role="alert" data-slot="audit-older-error" className="flex min-w-0 flex-wrap items-center gap-2 text-sm">
                <StatusBadge status="Error" tone="error" />
                <span className="min-w-0 text-destructive [overflow-wrap:anywhere]">
                  {errorMessage +
                    (items.length === 1 ? '. The loaded event stays listed.' : '. The ' + items.length + ' loaded events stay listed.')}
                </span>
              </p>
            )}
            {nextBefore != null ? (
              <Button
                variant={olderFailed ? 'outline' : 'ghost'}
                size="sm"
                data-slot="audit-load-older"
                aria-disabled={olderLoading || undefined}
                className="aria-disabled:cursor-progress aria-disabled:opacity-70"
                onFocus={() => (olderHadFocus.current = true)}
                onBlur={() => (olderHadFocus.current = false)}
                onClick={loadOlder}
              >
                {olderLoading && (
                  <LoaderCircle size={16} className="animate-spin motion-reduce:animate-none" aria-hidden="true" />
                )}
                {olderLoading ? 'Loading older events…' : olderFailed ? 'Try loading older events again' : 'Load older events'}
              </Button>
            ) : (
              <p ref={endNote} tabIndex={-1} data-slot="audit-older-end" className={cn('outline-none', ADMIN_SECONDARY)}>
                No older events for the current filters.
              </p>
            )}
          </div>
        )}
      </AdminPanel>
    </section>
  )
}

/** Where focus lands after a successful retry: a programmatic target that
 *  still shows the keyboard focus ring. */
const RESULT_FOCUS = 'outline-none focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring'

/** A retry control that stays in place, keeps focus and ignores presses
 *  while its request runs. */
function RetryButton({
  busy,
  label,
  busyLabel,
  onClick,
}: {
  busy: boolean
  label: string
  busyLabel: string
  onClick: (event: MouseEvent<HTMLButtonElement>) => void
}) {
  return (
    <Button
      variant="outline"
      size="sm"
      data-slot="audit-retry"
      aria-disabled={busy || undefined}
      className="aria-disabled:cursor-progress aria-disabled:opacity-70"
      onClick={onClick}
    >
      {busy && <LoaderCircle size={16} className="animate-spin motion-reduce:animate-none" aria-hidden="true" />}
      {busy ? busyLabel : label}
    </Button>
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
