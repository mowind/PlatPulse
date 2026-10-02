import { useState } from 'react'
import { Link, useParams, useSearchParams } from 'react-router'

import {
  AdminApiError,
  newNotificationRequestId,
  retryDeliveryEntry,
  useAdminDeliveryDetail,
  useAdminDeliveries,
} from '../api/admin'
import type { AttemptRow, DeliveryRow } from '../api/generated'
import { useAuth } from '../auth/AuthContext'
import { StatusBadge, formatObservedAt } from '../components/StatusBadge'
import { Button } from '../components/ui/button'
import { CardX } from '../components/ui/card-x'
import { Empty } from '../components/ui/empty'
import { Select } from '../components/ui/input'
import {
  CARD_SURFACE,
  DetailItem,
  DetailList,
  FormFeedbackNote,
  INDETERMINATE_OUTCOME,
  NotificationRequestPanel,
  NotificationSectionNav,
  deliveryIsRetryable,
  deliveryStateTone,
  errorMessage,
  indeterminateOutcome,
  shortId,
  type FormFeedback,
} from './notificationShared'

/** The Server's fixed delivery-state vocabulary (design §17.4). */
const DELIVERY_STATES = [
  'pending',
  'in_flight',
  'retry_scheduled',
  'succeeded',
  'failed',
  'dead_letter',
  'suppressed',
  'cancelled',
] as const

function readState(search: URLSearchParams): string {
  const value = search.get('state') ?? 'all'
  return (DELIVERY_STATES as readonly string[]).includes(value) ? value : 'all'
}

function readChannel(search: URLSearchParams): string {
  const value = search.get('channel') ?? 'all'
  return value === 'telegram' ? value : 'all'
}

function readLimit(search: URLSearchParams): number {
  const value = Number.parseInt(search.get('limit') ?? '', 10)
  return Number.isFinite(value) && value > 0 && value <= 100 ? value : 50
}

function AttemptsTable({ attempts }: { attempts: AttemptRow[] }) {
  if (attempts.length === 0) {
    return <Empty description="No provider attempt has been recorded for this delivery yet." />
  }
  return (
    <div className="overflow-x-auto">
      <table data-stack data-slot="notification-attempts-table" className="w-full text-sm">
        <caption className="sr-only">Delivery attempts</caption>
        <thead>
          <tr className="border-b">
            <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
              Attempt
            </th>
            <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
              Attempted at
            </th>
            <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
              Outcome
            </th>
            <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
              Provider result
            </th>
            <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
              Error kind
            </th>
            <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
              Retry after
            </th>
          </tr>
        </thead>
        <tbody>
          {attempts.map((attempt) => (
            <tr key={attempt.attemptId} className="border-b border-border/60 align-top">
              <th scope="row" data-label="Attempt" className="min-w-0 px-3 py-3 text-left font-normal">
                {String(attempt.attemptNumber)}
              </th>
              <td data-label="Attempted at" className="min-w-0 px-3 py-3">
                {formatObservedAt(attempt.attemptedAt)}
              </td>
              <td data-label="Outcome" className="min-w-0 px-3 py-3">
                {attempt.outcome}
              </td>
              <td data-label="Provider result" className="min-w-0 px-3 py-3 break-words">
                {attempt.providerResult}
              </td>
              <td data-label="Error kind" className="min-w-0 px-3 py-3">
                {attempt.errorKind ?? 'None'}
              </td>
              <td data-label="Retry after" className="min-w-0 px-3 py-3">
                {attempt.retryAfterSeconds == null ? 'None' : String(attempt.retryAfterSeconds) + ' s'}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

function RetryPanel({ delivery }: { delivery: DeliveryRow }) {
  const { generation, status } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const [requestId, setRequestId] = useState(() => newNotificationRequestId())
  const [pending, setPending] = useState(false)
  const [feedback, setFeedback] = useState<FormFeedback | null>(null)
  const [reconcileId, setReconcileId] = useState('')
  const retryable = deliveryIsRetryable(delivery.state)

  async function onRetry() {
    setPending(true)
    setFeedback(null)
    try {
      const result = await retryDeliveryEntry(delivery.deliveryId, requestId, csrfToken)
      setFeedback({
        tone: 'ok',
        message: result.deduplicated
          ? 'The Server already recorded this request id, so it returned the recorded result instead of queueing again.'
          : 'The Server accepted the retry command and queued the delivery. The state below updates when the worker runs; a provider failure is reported on the delivery, not here.',
      })
    } catch (error) {
      if (indeterminateOutcome(error)) {
        setReconcileId(requestId)
        setFeedback({ tone: 'error', message: INDETERMINATE_OUTCOME })
      } else if (
        error instanceof AdminApiError &&
        (error.code === 'notification_request_id_conflict' || error.code === 'delivery_already_queued')
      ) {
        setFeedback({
          tone: 'error',
          message: error.message + ' Start a new request id before retrying with different intent.',
        })
      } else {
        setFeedback({ tone: 'error', message: errorMessage(error, 'Unable to queue the retry') })
      }
    } finally {
      setPending(false)
    }
  }

  return (
    <CardX size="medium" className={CARD_SURFACE} title="Retry command">
      <div className="space-y-3">
        <p className="text-sm text-muted-foreground">
          A retry queues one new provider attempt for this delivery. The Server accepts a retry only from
          retry_scheduled, failed, or dead_letter; suppressed and cancelled deliveries stay terminal.
        </p>
        <DetailList>
          <DetailItem label="Delivery state">
            <StatusBadge status={delivery.state} tone={deliveryStateTone(delivery.state)} />
          </DetailItem>
          <DetailItem label="Request id">
            <span className="font-mono text-xs">{requestId}</span>
          </DetailItem>
        </DetailList>
        {!retryable && (
          <p role="status" className="text-sm">
            {delivery.state} is not a retryable state, so the Server refuses a retry command for this delivery.
          </p>
        )}
        <div className="flex flex-wrap gap-2">
          <Button
            size="sm"
            className="min-h-11"
            disabled={!retryable || pending || csrfToken.length === 0}
            onClick={() => void onRetry()}
          >
            {pending ? 'Queueing retry…' : 'Queue retry'}
          </Button>
          <Button
            variant="outline"
            size="sm"
            className="min-h-11"
            onClick={() => {
              setRequestId(newNotificationRequestId())
              setReconcileId('')
              setFeedback(null)
            }}
          >
            New request id
          </Button>
        </div>
        <FormFeedbackNote feedback={feedback} />
        {reconcileId.length > 0 && (
          <NotificationRequestPanel
            key={generation + ':' + reconcileId}
            initialRequestId={reconcileId}
            title="Reconcile the uncertain retry"
          />
        )}
      </div>
    </CardX>
  )
}

export default function AdminNotificationDeliveries() {
  const { generation } = useAuth()
  const [search, setSearch] = useSearchParams()
  const state = readState(search)
  const channel = readChannel(search)
  const limit = readLimit(search)
  const before = search.get('before') ?? ''
  const deliveries = useAdminDeliveries(generation, {
    state: state === 'all' ? undefined : state,
    channel: channel === 'all' ? undefined : channel,
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

  const nextBefore = deliveries.data?.nextBefore ?? null

  return (
    <div className="space-y-6">
      <div className="space-y-3">
        <h1 className="text-xl font-semibold tracking-tight">Notification Deliveries</h1>
        <p className="text-sm text-muted-foreground">
          The Server's per-channel delivery rows are the only record of what happened after a notification was
          accepted. Provider results and accumulated attempts are preserved; suppressed and cancelled rows are
          terminal.
        </p>
        <NotificationSectionNav />
      </div>

      <div className="flex flex-wrap items-end gap-2">
        <label className="text-xs font-medium tracking-wider text-muted-foreground" htmlFor="delivery-state-filter">
          State
          <Select
            id="delivery-state-filter"
            className="mt-1 min-h-11"
            value={state}
            onChange={(event) => setParam('state', event.target.value === 'all' ? null : event.target.value)}
          >
            <option value="all">all</option>
            {DELIVERY_STATES.map((value) => (
              <option key={value} value={value}>
                {value}
              </option>
            ))}
          </Select>
        </label>
        <label className="text-xs font-medium tracking-wider text-muted-foreground" htmlFor="delivery-channel-filter">
          Channel
          <Select
            id="delivery-channel-filter"
            className="mt-1 min-h-11"
            value={channel}
            onChange={(event) => setParam('channel', event.target.value === 'all' ? null : event.target.value)}
          >
            <option value="all">all</option>
            <option value="telegram">telegram</option>
          </Select>
        </label>
        <label className="text-xs font-medium tracking-wider text-muted-foreground" htmlFor="delivery-limit-filter">
          Page size
          <Select
            id="delivery-limit-filter"
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
          Older deliveries
        </Button>
      </div>

      {!deliveries.data && deliveries.isPending && (
        <p role="status" className="text-sm">
          <StatusBadge status="Starting" tone="neutral" /> Loading the notification deliveries…
        </p>
      )}
      {!deliveries.data && deliveries.isError && (
        <div role="alert" className="space-y-2 text-sm">
          <p>{errorMessage(deliveries.error, 'Unable to load the notification deliveries')}</p>
          <Button variant="link" size="sm" onClick={() => void deliveries.refetch()}>
            Try again
          </Button>
        </div>
      )}
      {deliveries.data && deliveries.isRefetchError && (
        <p role="alert" className="text-sm">
          Failed to refresh; showing the last successful notification deliveries.
        </p>
      )}
      {deliveries.data && deliveries.data.items.length === 0 && (
        <CardX size="medium" className={CARD_SURFACE}>
          <Empty description="No notification deliveries match the current filters." />
        </CardX>
      )}
      {deliveries.data && deliveries.data.items.length > 0 && (
        <CardX
          size="medium"
          className={CARD_SURFACE}
          contentClassName="p-0"
          segmented
          title={'Notification deliveries · ' + String(deliveries.data.items.length)}
        >
          <div className="overflow-x-auto">
            <table data-stack data-slot="notification-deliveries-table" className="w-full text-sm">
              <caption className="sr-only">Notification deliveries</caption>
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
                    Next attempt
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Last result
                  </th>
                </tr>
              </thead>
              <tbody>
                {deliveries.data.items.map((delivery) => (
                  <tr key={delivery.deliveryId} className="border-b border-border/60 align-top">
                    <th scope="row" data-label="Delivery" className="min-w-0 px-3 py-3 text-left font-normal">
                      <Link className="inline-flex min-h-11 items-center underline underline-offset-4" to={'/admin/notifications/deliveries/' + delivery.deliveryId}>
                        {shortId(delivery.deliveryId)}
                      </Link>
                      <span className="block text-xs text-muted-foreground">
                        {formatObservedAt(delivery.updatedAt)}
                      </span>
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
                    <td data-label="Next attempt" className="min-w-0 px-3 py-3">
                      {delivery.nextAttemptAt ? formatObservedAt(delivery.nextAttemptAt) : 'None'}
                    </td>
                    <td data-label="Last result" className="min-w-0 px-3 py-3 break-words">
                      {delivery.lastResult ?? 'None'}
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

export function AdminNotificationDeliveryDetailPage() {
  const { deliveryId = '' } = useParams()
  const { generation } = useAuth()
  const query = useAdminDeliveryDetail(generation, deliveryId)
  const notFound =
    query.error instanceof AdminApiError &&
    (query.error.code === 'notification_delivery_not_found' || query.error.code === 'not_found')

  if (notFound) {
    return (
      <div className="space-y-6">
        <h1 className="text-xl font-semibold tracking-tight">Notification Delivery not found</h1>
        <p className="text-sm text-muted-foreground">
          The Server has no notification delivery with this id. It may have been pruned, or the link is stale.
        </p>
        <Link
          className="inline-flex min-h-11 items-center text-sm font-medium underline-offset-4 hover:underline"
          to="/admin/notifications/deliveries">
          Back to notification deliveries
        </Link>
      </div>
    )
  }

  const detail = query.data

  return (
    <div className="space-y-6">
      <div className="space-y-3">
        <h1 className="text-xl font-semibold tracking-tight">
          Notification Delivery {shortId(deliveryId)}
        </h1>
        <Link
          className="inline-flex min-h-11 items-center text-sm font-medium underline-offset-4 hover:underline"
          to="/admin/notifications/deliveries">
          Back to notification deliveries
        </Link>
      </div>

      {!detail && query.isPending && (
        <p role="status" className="text-sm">
          <StatusBadge status="Starting" tone="neutral" /> Loading the notification delivery…
        </p>
      )}
      {!detail && query.isError && (
        <div role="alert" className="space-y-2 text-sm">
          <p>{errorMessage(query.error, 'Unable to load the notification delivery')}</p>
          <Button variant="link" size="sm" onClick={() => void query.refetch()}>
            Try again
          </Button>
        </div>
      )}
      {detail && (
        <>
          {query.isRefetchError && (
            <p role="alert" className="text-sm">
              Failed to refresh; showing the last successful notification delivery.
            </p>
          )}
          <CardX size="medium" className={CARD_SURFACE} title="Delivery">
            <div className="space-y-3">
              <DetailList>
                <DetailItem label="Delivery id">
                  <span className="font-mono text-xs">{detail.deliveryId}</span>
                </DetailItem>
                <DetailItem label="State">
                  <StatusBadge status={detail.state} tone={deliveryStateTone(detail.state)} />
                </DetailItem>
                <DetailItem label="Channel">{detail.channelKind}</DetailItem>
                <DetailItem label="Destination (masked)">
                  <span className="font-mono text-xs">{detail.destination}</span>
                </DetailItem>
                <DetailItem label="Event">
                  <Link className="inline-flex min-h-11 items-center underline underline-offset-4" to={'/admin/notifications/events/' + detail.eventId}>
                    {shortId(detail.eventId)}
                  </Link>
                </DetailItem>
                <DetailItem label="Event kind">{detail.event.eventKind}</DetailItem>
                <DetailItem label="Attempts">{String(detail.attemptCount)}</DetailItem>
                <DetailItem label="Created at">{formatObservedAt(detail.createdAt)}</DetailItem>
                <DetailItem label="Updated at">{formatObservedAt(detail.updatedAt)}</DetailItem>
                <DetailItem label="Last attempt at">
                  {detail.lastAttemptAt ? formatObservedAt(detail.lastAttemptAt) : 'None'}
                </DetailItem>
                <DetailItem label="Next attempt at">
                  {detail.nextAttemptAt ? formatObservedAt(detail.nextAttemptAt) : 'None'}
                </DetailItem>
                <DetailItem label="Last result">{detail.lastResult ?? 'None'}</DetailItem>
                <DetailItem label="Last error kind">{detail.lastErrorKind ?? 'None'}</DetailItem>
                <DetailItem label="Provider retry after">
                  {detail.retryAfterSeconds == null ? 'None' : String(detail.retryAfterSeconds) + ' s'}
                </DetailItem>
              </DetailList>
              <p className="text-xs text-muted-foreground">
                The destination is the Server's redaction of the configured chat id; the token and the full chat id
                are never returned to the browser.
              </p>
            </div>
          </CardX>

          <RetryPanel delivery={detail} />

          <CardX
            size="medium"
            className={CARD_SURFACE}
            contentClassName="p-0"
            segmented
            title={'Attempts · ' + String(detail.attempts.length)}
          >
            <AttemptsTable attempts={detail.attempts} />
          </CardX>
        </>
      )}
    </div>
  )
}
