import { Link, useParams, useSearchParams } from 'react-router'

import { AdminApiError, useAdminNotificationEvent, useAdminNotificationEvents } from '../api/admin'
import type { NotificationEventItem } from '../api/generated'
import { useAuth } from '../auth/AuthContext'
import { StatusBadge, formatObservedAt } from '../components/StatusBadge'
import { Button } from '../components/ui/button'
import { CardX } from '../components/ui/card-x'
import { Empty } from '../components/ui/empty'
import { Select } from '../components/ui/input'
import {
  CARD_SURFACE,
  NotificationSectionNav,
  deliveryStateTone,
  errorMessage,
  shortId,
} from './notificationShared'

const EVENT_KINDS = ['all', 'incident', 'test'] as const

function readKind(search: URLSearchParams): string {
  const value = search.get('kind') ?? 'all'
  return value === 'incident' || value === 'test' ? value : 'all'
}

function readLimit(search: URLSearchParams): number {
  const value = Number.parseInt(search.get('limit') ?? '', 10)
  return Number.isFinite(value) && value > 0 && value <= 100 ? value : 50
}

function eventSubject(event: NotificationEventItem): string {
  if (!event.subjectKind || !event.subjectKey) return 'None'
  return event.subjectKind + ' · ' + event.subjectKey
}

function DeliveriesCell({ event }: { event: NotificationEventItem }) {
  if (event.deliveries.length === 0) return <span className="text-muted-foreground">None</span>
  return (
    <span className="flex flex-wrap gap-1">
      {event.deliveries.map((delivery) => (
        <StatusBadge
          key={delivery.deliveryId}
          status={delivery.state}
          tone={deliveryStateTone(delivery.state)}
        />
      ))}
    </span>
  )
}

export default function AdminNotificationEvents() {
  const { generation } = useAuth()
  const [search, setSearch] = useSearchParams()
  const kind = readKind(search)
  const limit = readLimit(search)
  const before = search.get('before') ?? ''
  const events = useAdminNotificationEvents(generation, {
    eventKind: kind === 'all' ? undefined : kind,
    before: before.length > 0 ? before : undefined,
    limit,
  })

  function setParam(key: string, value: string | null) {
    const next = new URLSearchParams(search)
    if (value === null) next.delete(key)
    else next.set(key, value)
    if (key !== 'before') next.delete('before')
    setSearch(next, { replace: false })
  }

  const nextBefore = events.data?.nextBefore ?? null

  return (
    <div className="space-y-6">
      <div className="space-y-3">
        <h1 className="text-xl font-semibold tracking-tight">Notification Events</h1>
        <p className="text-sm text-muted-foreground">
          Every event the Server recorded, including the suppression decision and the Delivery rows it produced. A
          suppressed delivery is terminal and never retryable.
        </p>
        <NotificationSectionNav />
      </div>

      <div className="flex flex-wrap items-end gap-2">
        <label className="text-xs font-medium tracking-wider text-muted-foreground" htmlFor="event-kind-filter">
          Event kind
          <Select
            id="event-kind-filter"
            className="mt-1 min-h-11"
            value={kind}
            onChange={(event) => setParam('kind', event.target.value === 'all' ? null : event.target.value)}
          >
            {EVENT_KINDS.map((value) => (
              <option key={value} value={value}>
                {value}
              </option>
            ))}
          </Select>
        </label>
        <label className="text-xs font-medium tracking-wider text-muted-foreground" htmlFor="event-limit-filter">
          Page size
          <Select
            id="event-limit-filter"
            className="mt-1 min-h-11"
            value={String(limit)}
            onChange={(event) => setParam('limit', event.target.value)}
          >
            {[25, 50, 100].map((value) => (
              <option key={value} value={value}>
                {value}
              </option>
            ))}
          </Select>
        </label>
        {before.length > 0 && (
          <Button variant="outline" size="sm" className="min-h-11" onClick={() => setParam('before', null)}>
            Back to newest
          </Button>
        )}
        <Button
          variant="outline"
          size="sm"
          className="min-h-11"
          disabled={!nextBefore}
          onClick={() => setParam('before', nextBefore)}
        >
          Older events
        </Button>
      </div>

      {!events.data && events.isPending && (
        <p role="status" className="text-sm">
          <StatusBadge status="Starting" tone="neutral" /> Loading the notification events…
        </p>
      )}
      {!events.data && events.isError && (
        <div role="alert" className="space-y-2 text-sm">
          <p>{errorMessage(events.error, 'Unable to load the notification events')}</p>
          <Button variant="link" size="sm" onClick={() => void events.refetch()}>
            Try again
          </Button>
        </div>
      )}
      {events.data && events.isRefetchError && (
        <p role="alert" className="text-sm">
          Failed to refresh; showing the last successful notification events.
        </p>
      )}
      {events.data && events.data.items.length === 0 && (
        <CardX size="medium" className={CARD_SURFACE}>
          <Empty description="No notification events match the current filters. Events appear here when the Server records an incident notification or a controlled test." />
        </CardX>
      )}
      {events.data && events.data.items.length > 0 && (
        <CardX
          size="medium"
          className={CARD_SURFACE}
          contentClassName="p-0"
          segmented
          title={'Notification events · ' + String(events.data.items.length)}
        >
          <div className="overflow-x-auto">
            <table data-stack data-slot="notification-events-table" className="w-full text-sm">
              <caption className="sr-only">Notification events</caption>
              <thead>
                <tr className="border-b">
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Created
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Kind
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Severity
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Subject
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Summary
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Deliveries
                  </th>
                </tr>
              </thead>
              <tbody>
                {events.data.items.map((event) => (
                  <tr key={event.eventId} className="border-b border-border/60 align-top">
                    <th scope="row" data-label="Created" className="min-w-0 px-3 py-3 text-left font-normal">
                      <Link className="inline-flex min-h-11 items-center underline underline-offset-4" to={'/admin/notifications/events/' + event.eventId}>
                        {shortId(event.eventId)}
                      </Link>
                      <span className="block text-xs text-muted-foreground">
                        {formatObservedAt(event.createdAt)}
                      </span>
                    </th>
                    <td data-label="Kind" className="min-w-0 px-3 py-3">
                      {event.eventKind}
                    </td>
                    <td data-label="Severity" className="min-w-0 px-3 py-3">
                      <StatusBadge
                        status={event.severity}
                        tone={event.severity === 'critical' ? 'error' : undefined}
                      />
                    </td>
                    <td data-label="Subject" className="min-w-0 px-3 py-3">
                      {eventSubject(event)}
                    </td>
                    <td data-label="Summary" className="min-w-0 px-3 py-3 break-words">
                      {event.summary}
                    </td>
                    <td data-label="Deliveries" className="min-w-0 px-3 py-3">
                      <DeliveriesCell event={event} />
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </CardX>
      )}
    </div>
  )
}

export function AdminNotificationEventDetailPage() {
  const { eventId = '' } = useParams()
  const { generation } = useAuth()
  const query = useAdminNotificationEvent(generation, eventId)
  const notFound =
    query.error instanceof AdminApiError &&
    (query.error.code === 'notification_event_not_found' || query.error.code === 'not_found')

  if (notFound) {
    return (
      <div className="space-y-6">
        <h1 className="text-xl font-semibold tracking-tight">Notification Event not found</h1>
        <p className="text-sm text-muted-foreground">
          The Server has no notification event with this id. It may have been pruned, or the link is stale.
        </p>
        <Link
          className="inline-flex min-h-11 items-center text-sm font-medium underline-offset-4 hover:underline"
          to="/admin/notifications/events">
          Back to notification events
        </Link>
      </div>
    )
  }

  const event = query.data

  return (
    <div className="space-y-6">
      <div className="space-y-3">
        <h1 className="text-xl font-semibold tracking-tight">
          Notification Event {shortId(eventId)}
        </h1>
        <Link
          className="inline-flex min-h-11 items-center text-sm font-medium underline-offset-4 hover:underline"
          to="/admin/notifications/events">
          Back to notification events
        </Link>
      </div>

      {!event && query.isPending && (
        <p role="status" className="text-sm">
          <StatusBadge status="Starting" tone="neutral" /> Loading the notification event…
        </p>
      )}
      {!event && query.isError && (
        <div role="alert" className="space-y-2 text-sm">
          <p>{errorMessage(query.error, 'Unable to load the notification event')}</p>
          <Button variant="link" size="sm" onClick={() => void query.refetch()}>
            Try again
          </Button>
        </div>
      )}
      {event && (
        <>
          {query.isRefetchError && (
            <p role="alert" className="text-sm">
              Failed to refresh; showing the last successful notification event.
            </p>
          )}
          <CardX size="medium" className={CARD_SURFACE} title="Event">
            <dl className="grid grid-cols-1 gap-3 sm:grid-cols-2">
              <div className="min-w-0">
                <dt className="text-xs font-medium tracking-wider text-muted-foreground">Event id</dt>
                <dd className="mt-0.5 min-w-0 break-words font-mono text-sm">{event.eventId}</dd>
              </div>
              <div className="min-w-0">
                <dt className="text-xs font-medium tracking-wider text-muted-foreground">Recorded at</dt>
                <dd className="mt-0.5 min-w-0 break-words text-sm">{formatObservedAt(event.createdAt)}</dd>
              </div>
              <div className="min-w-0">
                <dt className="text-xs font-medium tracking-wider text-muted-foreground">Kind</dt>
                <dd className="mt-0.5 min-w-0 break-words text-sm">{event.eventKind}</dd>
              </div>
              <div className="min-w-0">
                <dt className="text-xs font-medium tracking-wider text-muted-foreground">Severity</dt>
                <dd className="mt-0.5 min-w-0 break-words text-sm">{event.severity}</dd>
              </div>
              <div className="min-w-0">
                <dt className="text-xs font-medium tracking-wider text-muted-foreground">Incident</dt>
                <dd className="mt-0.5 min-w-0 break-words text-sm">
                  {event.incidentId ? (
                    <Link className="underline" to={'/admin/alerts/incidents/' + event.incidentId}>
                      {shortId(event.incidentId)}
                    </Link>
                  ) : (
                    'None'
                  )}
                </dd>
              </div>
              <div className="min-w-0">
                <dt className="text-xs font-medium tracking-wider text-muted-foreground">Rule key</dt>
                <dd className="mt-0.5 min-w-0 break-words text-sm">{event.ruleKey ?? 'None'}</dd>
              </div>
              <div className="min-w-0">
                <dt className="text-xs font-medium tracking-wider text-muted-foreground">Subject</dt>
                <dd className="mt-0.5 min-w-0 break-words text-sm">
                  {event.subjectKind && event.subjectKey
                    ? event.subjectKind + ' · ' + event.subjectKey
                    : 'None'}
                </dd>
              </div>
              <div className="min-w-0 sm:col-span-2">
                <dt className="text-xs font-medium tracking-wider text-muted-foreground">Summary</dt>
                <dd className="mt-0.5 min-w-0 break-words text-sm">{event.summary}</dd>
              </div>
            </dl>
          </CardX>

          <CardX
            size="medium"
            className={CARD_SURFACE}
            contentClassName="p-0"
            segmented
            title={'Deliveries · ' + String(event.deliveries.length)}
          >
            {event.deliveries.length === 0 ? (
              <Empty description="This event produced no Delivery rows. A channel may be unconfigured, or the delivery was suppressed before any handoff." />
            ) : (
              <div className="overflow-x-auto">
                <table data-stack data-slot="notification-event-deliveries-table" className="w-full text-sm">
                  <caption className="sr-only">Deliveries for this notification event</caption>
                  <thead>
                    <tr className="border-b">
                      <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                        Delivery
                      </th>
                      <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                        Channel
                      </th>
                      <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                        Destination
                      </th>
                      <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                        State
                      </th>
                      <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                        Attempts
                      </th>
                      <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                        Last result
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {event.deliveries.map((delivery) => (
                      <tr key={delivery.deliveryId} className="border-b border-border/60 align-top">
                        <th scope="row" data-label="Delivery" className="min-w-0 px-3 py-3 text-left font-normal">
                          <Link className="inline-flex min-h-11 items-center underline underline-offset-4" to={'/admin/notifications/deliveries/' + delivery.deliveryId}>
                            {shortId(delivery.deliveryId)}
                          </Link>
                        </th>
                        <td data-label="Channel" className="min-w-0 px-3 py-3">
                          {delivery.channelKind}
                        </td>
                        <td data-label="Destination" className="min-w-0 px-3 py-3 font-mono text-xs">
                          {delivery.destination}
                        </td>
                        <td data-label="State" className="min-w-0 px-3 py-3">
                          <StatusBadge status={delivery.state} tone={deliveryStateTone(delivery.state)} />
                        </td>
                        <td data-label="Attempts" className="min-w-0 px-3 py-3">
                          {String(delivery.attemptCount)}
                        </td>
                        <td data-label="Last result" className="min-w-0 px-3 py-3 break-words">
                          {delivery.lastResult ?? 'None'}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </CardX>
        </>
      )}
    </div>
  )
}
