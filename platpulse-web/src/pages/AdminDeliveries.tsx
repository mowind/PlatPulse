import { useState } from 'react'
import { Link, useParams } from 'react-router'
import {
  AdminApiError,
  type DeliveryFilters,
  type NotificationEventsFilters,
  retryDeliveryEntry,
  useAdminDeliveryDetail,
  useAdminDeliveries,
  useAdminNotificationEvents,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { StatusBadge, formatObservedAt } from '../components/StatusBadge'
import { formatTs } from './AdminAlertRules'
import type { DeliveryRow, EventRow } from '../api/generated'

/**
 * PAGE-ADMIN-DELIVERIES / PAGE-ADMIN-DELIVERY (design §4.4, webui.md
 * §8.3): the notification Outbox. Notification Events (durable business
 * records of Incident transitions or Owner test actions) are shown
 * separately from per-channel/destination Delivery attempts. Delivery
 * state carries bounded retry/backoff, Retry-After, attempt history,
 * provider results, and DeadLetter outcome; manual retry re-arms the same
 * Delivery and never creates a duplicate Event, Incident, or transition.
 * Destinations are redacted by the Server (last four characters only);
 * provider tokens never reach the browser.
 */

/** Server-owned Delivery state words are shown as sent (webui.md §5.4). */
export function deliveryStateBadge(state: string): React.ReactNode {
  switch (state) {
    case 'pending':
      return <StatusBadge status="Pending" tone="neutral" />
    case 'in_flight':
      return <StatusBadge status="Sending" tone="neutral" />
    case 'retry_scheduled':
      return <StatusBadge status="Retry scheduled" tone="warning" />
    case 'succeeded':
      return <StatusBadge status="Delivered" tone="ok" />
    case 'failed':
      return <StatusBadge status="Failed" tone="error" />
    case 'dead_letter':
      return <StatusBadge status="Dead letter" tone="error" />
    case 'suppressed':
      return <StatusBadge status="Suppressed" tone="neutral" />
    default:
      return <StatusBadge status={state} tone="neutral" />
  }
}

function severityLabel(severity: string | undefined): string {
  switch (severity) {
    case 'info':
      return 'Info'
    case 'warning':
      return 'Warning'
    case 'critical':
      return 'Critical'
    default:
      return severity ?? 'Unknown'
  }
}

export default function AdminDeliveriesList() {
  const { generation } = useAuth()
  const [stateFilter, setStateFilter] = useState('')
  const [channelFilter, setChannelFilter] = useState('')
  const [kindFilter, setKindFilter] = useState('')
  const filters: DeliveryFilters = {
    state: stateFilter || undefined,
    channel: channelFilter || undefined,
  }
  const eventsFilters: NotificationEventsFilters = {
    eventKind: kindFilter || undefined,
    limit: 25,
  }
  const query = useAdminDeliveries(generation, filters)
  const eventsQuery = useAdminNotificationEvents(generation, eventsFilters)

  return (
    <section className="page">
      <h1>Deliveries</h1>
      <p className="muted">
        Every Notification Event is durable and produces one Delivery per configured channel.
        Delivery is at-least-once: automatic retries are bounded with exponential backoff and
        provider Retry-After; exhausted Deliveries reach Dead letter and stay retryable by an
        Owner. One failed destination never erases successful Delivery state.
      </p>
      <div className="filter-bar" role="group" aria-label="Delivery filters">
        <label htmlFor="delivery-state-filter">State</label>
        <select
          id="delivery-state-filter"
          value={stateFilter}
          onChange={(event) => setStateFilter(event.target.value)}
        >
          <option value="">All states</option>
          <option value="pending">Pending</option>
          <option value="retry_scheduled">Retry scheduled</option>
          <option value="succeeded">Delivered</option>
          <option value="failed">Failed</option>
          <option value="dead_letter">Dead letter</option>
          <option value="suppressed">Suppressed</option>
        </select>
        <label htmlFor="delivery-channel-filter">Channel</label>
        <select
          id="delivery-channel-filter"
          value={channelFilter}
          onChange={(event) => setChannelFilter(event.target.value)}
        >
          <option value="">All channels</option>
          <option value="telegram">Telegram</option>
        </select>
      </div>

      <h2>Outbox</h2>
      {!query.data && query.isPending && (
        <p className="panel-state" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading Deliveries…
        </p>
      )}
      {!query.data && query.isError && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Error" tone="error" />{' '}
          {query.error instanceof Error ? query.error.message : 'Unable to load Deliveries'}
          <button type="button" className="text-action" onClick={() => void query.refetch()}>
            Try again
          </button>
        </p>
      )}
      {query.data && query.data.items.length === 0 && (
        <p className="panel-state">
          <StatusBadge status="Empty" tone="ok" /> No Deliveries{stateFilter ? ' in this state' : ''}.
        </p>
      )}
      {query.data && query.data.items.length > 0 && (
        <div className="table-wrap">
          <table className="node-table">
            <caption className="sr-only">Notification Deliveries with per-channel state</caption>
            <thead>
              <tr>
                <th scope="col">Delivery</th>
                <th scope="col">Event</th>
                <th scope="col">Channel</th>
                <th scope="col">Destination</th>
                <th scope="col">State</th>
                <th scope="col">Attempts</th>
                <th scope="col">Next attempt</th>
              </tr>
            </thead>
            <tbody>
              {query.data.items.map((delivery) => (
                <DeliveryRow key={delivery.deliveryId} delivery={delivery} />
              ))}
            </tbody>
          </table>
        </div>
      )}

      <h2>Notification Events</h2>
      <p className="muted">
        Events are business records, separate from Delivery attempts. Manual retries never
        duplicate them; test notifications are marked Test and never become business Incidents.
      </p>
      <div className="filter-bar" role="group" aria-label="Event filters">
        <label htmlFor="event-kind-filter">Kind</label>
        <select
          id="event-kind-filter"
          value={kindFilter}
          onChange={(event) => setKindFilter(event.target.value)}
        >
          <option value="">All kinds</option>
          <option value="incident">Incident</option>
          <option value="test">Test</option>
        </select>
      </div>
      {!eventsQuery.data && eventsQuery.isPending && (
        <p className="panel-state" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading Events…
        </p>
      )}
      {!eventsQuery.data && eventsQuery.isError && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Error" tone="error" />{' '}
          {eventsQuery.error instanceof Error
            ? eventsQuery.error.message
            : 'Unable to load Notification Events'}
          <button type="button" className="text-action" onClick={() => void eventsQuery.refetch()}>
            Try again
          </button>
        </p>
      )}
      {eventsQuery.data && eventsQuery.data.items.length === 0 && (
        <p className="panel-state">
          <StatusBadge status="Empty" tone="ok" /> No Notification Events yet.
        </p>
      )}
      {eventsQuery.data && eventsQuery.data.items.length > 0 && (
        <ul className="node-list">
          {eventsQuery.data.items.map((item) => (
            <li key={item.eventId}>
              <span>
                {item.eventKind === 'test' ? (
                  <StatusBadge status="Test" tone="neutral" />
                ) : (
                  <StatusBadge status="Incident" tone="warning" />
                )}{' '}
                <strong>{severityLabel(item.severity)}</strong> — {item.summary}
              </span>
              <small className="muted">
                {formatTs(item.createdAt)}
                {item.deliveries.length === 0
                  ? ' · no configured channel'
                  : ` · ${item.deliveries
                      .map(
                        (delivery) =>
                          `${delivery.channelKind} ${deliveryStateLabel(delivery.state)}`,
                      )
                      .join(', ')}`}
              </small>
            </li>
          ))}
        </ul>
      )}
    </section>
  )
}

function deliveryStateLabel(state: string): string {
  switch (state) {
    case 'pending':
      return 'pending'
    case 'in_flight':
      return 'sending'
    case 'retry_scheduled':
      return 'retry scheduled'
    case 'succeeded':
      return 'delivered'
    case 'failed':
      return 'failed'
    case 'dead_letter':
      return 'dead letter'
    case 'suppressed':
      return 'suppressed'
    default:
      return state
  }
}

function DeliveryRow({ delivery }: { delivery: DeliveryRow }) {
  return (
    <tr>
      <td data-label="Delivery">
        <Link to={`/admin/alerts/deliveries/${encodeURIComponent(delivery.deliveryId)}`}>
          {delivery.deliveryId.slice(0, 8)}
        </Link>
      </td>
      <td data-label="Event">
        <Link to={`/admin/alerts/deliveries/${encodeURIComponent(delivery.deliveryId)}`}>
          {delivery.eventId.slice(0, 8)}
        </Link>
      </td>
      <td data-label="Channel">{delivery.channelKind}</td>
      <td data-label="Destination">
        <span title="Redacted destination summary">{delivery.destination}</span>
      </td>
      <td data-label="State">{deliveryStateBadge(delivery.state)}</td>
      <td data-label="Attempts">{delivery.attemptCount}</td>
      <td data-label="Next attempt">{formatTs(delivery.nextAttemptAt)}</td>
    </tr>
  )
}

/** PAGE-ADMIN-DELIVERY: destination redaction, attempts, retry. */
export function AdminDeliveryDetail() {
  const { deliveryId = '' } = useParams()
  const { generation } = useAuth()
  const status = useAuth().status
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const query = useAdminDeliveryDetail(generation, deliveryId)
  const [retrying, setRetrying] = useState(false)
  const [retryError, setRetryError] = useState<string | null>(null)
  const [retryResult, setRetryResult] = useState<string | null>(null)

  const onRetry = async () => {
    if (!window.confirm('Re-arm this Delivery for a new attempt? Retry never creates a duplicate Event.')) {
      return
    }
    setRetrying(true)
    setRetryError(null)
    setRetryResult(null)
    try {
      const response = await retryDeliveryEntry(deliveryId, csrfToken)
      setRetryResult(`Retry queued — attempt ${response.attemptCount + 1} will run shortly.`)
    } catch (retryFailure) {
      const message =
        retryFailure instanceof AdminApiError ? retryFailure.message : 'Unable to retry the Delivery.'
      if (retryFailure instanceof AdminApiError && retryFailure.code === 'delivery_already_queued') {
        setRetryError('This Delivery is already queued or in flight — a parallel retry was refused by the Server.')
      } else if (
        retryFailure instanceof AdminApiError &&
        retryFailure.code === 'delivery_not_retryable'
      ) {
        setRetryError('This Delivery cannot be retried in its current state.')
      } else {
        setRetryError(message)
      }
    } finally {
      setRetrying(false)
    }
  }

  return (
    <section className="page">
      <p>
        <Link to="/admin/alerts/deliveries" className="text-action">
          ← Deliveries
        </Link>
      </p>
      <h1>Delivery {deliveryId.slice(0, 8)}</h1>
      {!query.data && query.isPending && (
        <p className="panel-state" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading the Delivery…
        </p>
      )}
      {!query.data && query.isError && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Error" tone="error" />{' '}
          {query.error instanceof Error ? query.error.message : 'Unable to load the Delivery'}
        </p>
      )}
      {query.data && (
        <>
          <dl className="detail-list">
            <dt>State</dt>
            <dd>{deliveryStateBadge(query.data.state)}</dd>
            <dt>Channel</dt>
            <dd>{query.data.channelKind}</dd>
            <dt>Destination</dt>
            <dd>
              <span title="Redacted destination summary">{query.data.destination}</span>{' '}
              <small className="muted">(redacted)</small>
            </dd>
            <dt>Attempts</dt>
            <dd>{query.data.attemptCount}</dd>
            <dt>Next attempt</dt>
            <dd>
              {query.data.state === 'retry_scheduled' && query.data.nextAttemptAt
                ? formatTs(query.data.nextAttemptAt)
                : '—'}
            </dd>
            <dt>Last attempt</dt>
            <dd>{formatObservedAt(query.data.lastAttemptAt)}</dd>
            <dt>Last result</dt>
            <dd>{query.data.lastResult ?? '—'}</dd>
            {query.data.retryAfterSeconds != null && (
              <>
                <dt>Retry-After</dt>
                <dd>{query.data.retryAfterSeconds} s (provider)</dd>
              </>
            )}
          </dl>

          <h2>Notification Event</h2>
          <EventSummary event={query.data.event} />

          <h2>Attempts</h2>
          {query.data.attempts.length === 0 && (
            <p className="panel-state">
              <StatusBadge status="Empty" tone="ok" /> No attempt recorded yet.
            </p>
          )}
          {query.data.attempts.length > 0 && (
            <div className="table-wrap">
              <table className="node-table">
                <caption className="sr-only">Delivery attempt history</caption>
                <thead>
                  <tr>
                    <th scope="col">#</th>
                    <th scope="col">Attempted</th>
                    <th scope="col">Outcome</th>
                    <th scope="col">Provider result</th>
                    <th scope="col">Retry-After</th>
                  </tr>
                </thead>
                <tbody>
                  {query.data.attempts.map((attempt) => (
                    <tr key={attempt.attemptId}>
                      <td data-label="#">{attempt.attemptNumber}</td>
                      <td data-label="Attempted">{formatObservedAt(attempt.attemptedAt)}</td>
                      <td data-label="Outcome">
                        {attempt.outcome === 'succeeded' ? (
                          <StatusBadge status="Delivered" tone="ok" />
                        ) : (
                          <StatusBadge status="Failed" tone="error" />
                        )}
                      </td>
                      <td data-label="Provider result">{attempt.providerResult}</td>
                      <td data-label="Retry-After">
                        {attempt.retryAfterSeconds != null ? `${attempt.retryAfterSeconds} s` : '—'}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}

          <div className="field">
            <button
              type="button"
              className="button-primary"
              onClick={() => void onRetry()}
              disabled={
                retrying ||
                query.data.state === 'pending' ||
                query.data.state === 'in_flight' ||
                query.data.state === 'succeeded' ||
                query.data.state === 'suppressed'
              }
            >
              {retrying ? 'Retrying…' : 'Retry delivery'}
            </button>
            {query.data.state === 'pending' || query.data.state === 'in_flight' ? (
              <p className="muted">A retry is already queued — the Server refuses duplicates.</p>
            ) : null}
            {query.data.state === 'suppressed' ? (
              <p className="muted">
                This Delivery was suppressed by a Silence or Maintenance Window at Event creation;
                it is not retryable.
              </p>
            ) : null}
            {retryError && (
              <p className="field-error" role="alert">
                {retryError}
              </p>
            )}
            {retryResult && (
              <p className="form-ok" role="status">
                {retryResult}
              </p>
            )}
          </div>
          <p className="muted">
            Manual retry creates a new Delivery attempt on the same row — never a new Notification
            Event, Incident, or business transition. All retries are audited.
          </p>
        </>
      )}
    </section>
  )
}

function EventSummary({ event }: { event: EventRow }) {
  return (
    <dl className="detail-list">
      <dt>Event</dt>
      <dd>
        {event.eventId}{' '}
        {event.eventKind === 'test' ? (
          <StatusBadge status="Test" tone="neutral" />
        ) : (
          <StatusBadge status="Incident" tone="warning" />
        )}
      </dd>
      <dt>Summary</dt>
      <dd>{event.summary}</dd>
      <dt>Severity</dt>
      <dd>{severityLabel(event.severity)}</dd>
      {event.ruleKey && (
        <>
          <dt>Rule</dt>
          <dd>{event.ruleKey}</dd>
        </>
      )}
      {event.subjectKind && event.subjectKey && (
        <>
          <dt>Subject</dt>
          <dd>
            {event.subjectKind} · {event.subjectKey}
          </dd>
        </>
      )}
      {event.incidentId && (
        <>
          <dt>Incident</dt>
          <dd>
            <Link to={`/admin/alerts/incidents/${encodeURIComponent(event.incidentId)}`}>
              {event.incidentId.slice(0, 8)}
            </Link>
          </dd>
        </>
      )}
      <dt>Created</dt>
      <dd>{formatTs(event.createdAt)}</dd>
    </dl>
  )
}
