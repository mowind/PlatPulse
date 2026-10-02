import { Link } from 'react-router'

import { useAdminChannels } from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { Alert, AlertDescription, AlertTitle } from '../components/ui/alert'
import { CardX } from '../components/ui/card-x'
import { Empty } from '../components/ui/empty'
import { StatusBadge } from '../components/StatusBadge'
import { Button } from '../components/ui/button'
import {
  CARD_SURFACE,
  DetailItem,
  DetailList,
  NotificationSectionNav,
  errorMessage,
} from './notificationShared'

const SECTIONS = [
  {
    to: '/admin/notifications/events',
    title: 'Notification Events',
    description: 'Every Server notification event, its suppression decision, and the Delivery rows it produced.',
  },
  {
    to: '/admin/notifications/deliveries',
    title: 'Notification Deliveries',
    description: 'Per-channel attempts, provider results, and the retry command for eligible deliveries.',
  },
  {
    to: '/admin/notifications/channels',
    title: 'Notification Channels',
    description: 'Masked channel status and the controlled test command with request-id reconciliation.',
  },
]

export default function AdminNotifications() {
  const { generation } = useAuth()
  const channels = useAdminChannels(generation)
  const telegram = channels.data?.find((channel) => channel.channelId === 'telegram') ?? null

  return (
    <div className="space-y-6">
      <div className="space-y-3">
        <h1 className="text-xl font-semibold tracking-tight">Notifications</h1>
        <p className="text-sm text-muted-foreground">
          Inspect notification events, deliveries, and channel status, and run a controlled Telegram test. Channel
          credentials and destinations stay operator configuration; this surface never edits them.
        </p>
        <NotificationSectionNav />
      </div>

      <Alert>
        <AlertTitle>Request dedup is not an end-to-end delivery guarantee</AlertTitle>
        <AlertDescription className="space-y-1">
          <p>
            A request id identifies one Server command. Re-using the same id returns the recorded result instead of
            running the command again, and the Server enforces a configurable test cooldown plus a bounded ledger
            retention window. That protects the Server from duplicate external actions.
          </p>
          <p>
            It does not make Telegram delivery exactly-once: the recorded Delivery and Attempt rows are the only
            evidence of what the provider did, and a suppressed or cancelled delivery stays terminal and never becomes
            retryable. After a timeout or network failure, query the request result instead of re-sending.
          </p>
        </AlertDescription>
      </Alert>

      <CardX size="medium" className={CARD_SURFACE} title="Channel status">
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
        {channels.data && !telegram && (
          <Empty description="Telegram is not configured on this Server. An operator must configure the channel and its token file; the WebUI never edits channel credentials." />
        )}
        {channels.data && telegram && (
          <div className="space-y-3">
            <DetailList>
              <DetailItem label="Channel">
                <StatusBadge status={telegram.enabled ? 'Enabled' : 'Disabled'} tone={telegram.enabled ? 'ok' : 'neutral'} />
              </DetailItem>
              <DetailItem label="Destination (masked)">{telegram.destination}</DetailItem>
              <DetailItem label="Provider reference">{telegram.providerRef}</DetailItem>
              <DetailItem label="Max attempts">{String(telegram.maxAttempts)}</DetailItem>
              <DetailItem label="Retry base seconds">{String(telegram.retryBaseSeconds)}</DetailItem>
            </DetailList>
            <p className="text-xs text-muted-foreground">
              The destination and provider reference are redacted by the Server. The full chat id and the token file
              contents are never sent to the browser, stored in the query cache, or written to the Audit log.
            </p>
          </div>
        )}
      </CardX>

      <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
        {SECTIONS.map((section) => (
          <CardX
            key={section.to}
            size="medium"
            className={CARD_SURFACE}
            header={
              <Link
                className="flex min-h-11 min-w-0 flex-1 items-center truncate text-sm font-medium underline"
                to={section.to}
              >
                {section.title}
              </Link>
            }
          >
            <p className="text-sm text-muted-foreground">{section.description}</p>
          </CardX>
        ))}
      </div>
    </div>
  )
}
