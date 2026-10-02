import { useState } from 'react'
import { Link } from 'react-router'

import {
  AdminApiError,
  newNotificationRequestId,
  testNotificationChannelEntry,
  useAdminChannels,
} from '../api/admin'
import type { ChannelTestResponse } from '../api/generated'
import { useAuth } from '../auth/AuthContext'
import { StatusBadge } from '../components/StatusBadge'
import { Button } from '../components/ui/button'
import { CardX } from '../components/ui/card-x'
import { Empty } from '../components/ui/empty'
import {
  CARD_SURFACE,
  DetailItem,
  DetailList,
  FormFeedbackNote,
  INDETERMINATE_OUTCOME,
  NotificationRequestPanel,
  NotificationSectionNav,
  deliveryStateTone,
  errorMessage,
  indeterminateOutcome,
  shortId,
  type FormFeedback,
} from './notificationShared'

export default function AdminNotificationChannels() {
  const { generation, status } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const channels = useAdminChannels(generation)
  const telegram = channels.data?.find((channel) => channel.channelId === 'telegram') ?? null
  const [requestId, setRequestId] = useState(() => newNotificationRequestId())
  const [pending, setPending] = useState(false)
  const [feedback, setFeedback] = useState<FormFeedback | null>(null)
  const [outcome, setOutcome] = useState<ChannelTestResponse | null>(null)
  const [reconcileId, setReconcileId] = useState('')

  const testable = telegram !== null && telegram.enabled

  async function onTest() {
    setPending(true)
    setFeedback(null)
    setOutcome(null)
    try {
      const result = await testNotificationChannelEntry('telegram', requestId, csrfToken)
      setOutcome(result)
      setFeedback({
        tone: 'ok',
        message: result.deduplicated
          ? 'The Server already recorded this request id, so it returned the recorded command instead of sending another test.'
          : 'The Server accepted the test command and recorded its Audit entry. The Delivery state below is the provider outcome; a provider failure is not an HTTP failure.',
      })
    } catch (error) {
      if (indeterminateOutcome(error)) {
        setReconcileId(requestId)
        setFeedback({ tone: 'error', message: INDETERMINATE_OUTCOME })
      } else if (error instanceof AdminApiError && error.code === 'test_cooldown_active') {
        setFeedback({
          tone: 'error',
          message:
            error.message +
            ' The cooldown bounds how often the channel can be exercised; wait for it to elapse, or look up the request id to confirm the earlier result.',
        })
      } else if (error instanceof AdminApiError && error.code === 'notification_request_id_conflict') {
        setFeedback({
          tone: 'error',
          message: error.message + ' Start a new request id before sending a different test.',
        })
      } else {
        setFeedback({ tone: 'error', message: errorMessage(error, 'Unable to send the test notification') })
      }
    } finally {
      setPending(false)
    }
  }

  return (
    <div className="space-y-6">
      <div className="space-y-3">
        <h1 className="text-xl font-semibold tracking-tight">Notification Channels</h1>
        <p className="text-sm text-muted-foreground">
          Masked channel status and the controlled test command. The Server redacts the destination and the provider
          reference; channel enablement, the chat id, and the token file remain operator configuration.
        </p>
        <NotificationSectionNav />
      </div>

      <CardX size="medium" className={CARD_SURFACE} title="Configured channels">
        {!channels.data && channels.isPending && (
          <p role="status" className="text-sm">
            <StatusBadge status="Starting" tone="neutral" /> Loading the configured channels…
          </p>
        )}
        {!channels.data && channels.isError && (
          <div role="alert" className="space-y-2 text-sm">
            <p>{errorMessage(channels.error, 'Unable to load the configured channels')}</p>
            <Button variant="link" size="sm" onClick={() => void channels.refetch()}>
              Try again
            </Button>
          </div>
        )}
        {channels.data && channels.isRefetchError && (
          <p role="alert" className="text-sm">
            Failed to refresh; showing the last successful channel status.
          </p>
        )}
        {channels.data && channels.data.length === 0 && (
          <Empty description="No notification channel is configured on this Server. An operator must configure Telegram before the WebUI can run a controlled test." />
        )}
        {channels.data && channels.data.length > 0 && (
          <div className="overflow-x-auto">
            <table data-stack data-slot="notification-channels-table" className="w-full text-sm">
              <caption className="sr-only">Notification channels</caption>
              <thead>
                <tr className="border-b">
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Channel
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Enabled
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Destination
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Provider reference
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Max attempts
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Retry base
                  </th>
                </tr>
              </thead>
              <tbody>
                {channels.data.map((channel) => (
                  <tr key={channel.channelId} className="border-b border-border/60 align-top">
                    <th scope="row" data-label="Channel" className="min-w-0 px-3 py-3 text-left font-normal">
                      {channel.channelId}
                    </th>
                    <td data-label="Enabled" className="min-w-0 px-3 py-3">
                      <StatusBadge
                        status={channel.enabled ? 'Enabled' : 'Disabled'}
                        tone={channel.enabled ? 'ok' : 'neutral'}
                      />
                    </td>
                    <td data-label="Destination" className="min-w-0 px-3 py-3 font-mono text-xs">
                      {channel.destination}
                    </td>
                    <td data-label="Provider reference" className="min-w-0 px-3 py-3">
                      {channel.providerRef}
                    </td>
                    <td data-label="Max attempts" className="min-w-0 px-3 py-3">
                      {String(channel.maxAttempts)}
                    </td>
                    <td data-label="Retry base" className="min-w-0 px-3 py-3">
                      {String(channel.retryBaseSeconds)} s
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </CardX>

      <CardX size="medium" className={CARD_SURFACE} title="Controlled test">
        <div className="space-y-3">
          <p className="text-sm text-muted-foreground">
            Sends one test notification through the configured Telegram channel. The Server deduplicates by request id
            and enforces a configurable cooldown, but it cannot promise exactly-once delivery to Telegram: the recorded
            Delivery and its attempts are the provider outcome.
          </p>
          <DetailList>
            <DetailItem label="Channel">
              {telegram ? (
                <>
                  telegram ·{' '}
                  <StatusBadge
                    status={telegram.enabled ? 'Enabled' : 'Disabled'}
                    tone={telegram.enabled ? 'ok' : 'neutral'}
                  />
                </>
              ) : (
                'Not configured'
              )}
            </DetailItem>
            <DetailItem label="Request id">
              <span className="font-mono text-xs">{requestId}</span>
            </DetailItem>
          </DetailList>
          {!telegram && (
            <p role="status" className="text-sm">
              Telegram is not configured on this Server, so the test command is unavailable.
            </p>
          )}
          {telegram && !telegram.enabled && (
            <p role="status" className="text-sm">
              The Telegram channel is disabled by operator configuration, so the Server refuses a test command.
            </p>
          )}
          <div className="flex flex-wrap gap-2">
            <Button
              size="sm"
              className="min-h-11"
              disabled={!testable || pending || csrfToken.length === 0}
              onClick={() => void onTest()}
            >
              {pending ? 'Sending test…' : 'Send test notification'}
            </Button>
            <Button
              variant="outline"
              size="sm"
              className="min-h-11"
              onClick={() => {
                setRequestId(newNotificationRequestId())
                setReconcileId('')
                setFeedback(null)
                setOutcome(null)
              }}
            >
              New request id
            </Button>
          </div>
          <FormFeedbackNote feedback={feedback} />
          {outcome && (
            <div className="space-y-2" data-slot="notification-test-outcome">
              <DetailList>
                <DetailItem label="Deduplicated">{outcome.deduplicated ? 'Yes' : 'No'}</DetailItem>
                <DetailItem label="Delivery state">
                  <StatusBadge status={outcome.state} tone={deliveryStateTone(outcome.state)} />
                </DetailItem>
                <DetailItem label="Event">
                  <Link className="inline-flex min-h-11 items-center underline underline-offset-4" to={'/admin/notifications/events/' + outcome.eventId}>
                    {shortId(outcome.eventId)}
                  </Link>
                </DetailItem>
                <DetailItem label="Delivery">
                  <Link className="inline-flex min-h-11 items-center underline underline-offset-4" to={'/admin/notifications/deliveries/' + outcome.deliveryId}>
                    {shortId(outcome.deliveryId)}
                  </Link>
                </DetailItem>
                <DetailItem label="Audit entry">{String(outcome.auditEventId)}</DetailItem>
              </DetailList>
            </div>
          )}
          {reconcileId.length > 0 && (
            <NotificationRequestPanel
              key={generation + ':' + reconcileId}
              initialRequestId={reconcileId}
              title="Reconcile the uncertain test"
            />
          )}
        </div>
      </CardX>

      <NotificationRequestPanel />
    </div>
  )
}
