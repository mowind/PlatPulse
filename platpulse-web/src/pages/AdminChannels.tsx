import { useState } from 'react'
import { Link, useParams } from 'react-router'
import {
  AdminApiError,
  testNotificationChannelEntry,
  useAdminChannelDetail,
  useAdminChannels,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { StatusBadge } from '../components/StatusBadge'
import type { ChannelDto } from '../api/generated'

/**
 * PAGE-ADMIN-CHANNELS / PAGE-ADMIN-CHANNEL (design §4.4): supported
 * notification channels and their Server-owned policy. Channel
 * configuration is deployment config: destinations and provider
 * references are redacted by the Server (destination shows the last four
 * characters only; the provider reference is the secret file base name).
 * The test action sends a `test` Notification Event that is clearly
 * separate from business Incidents, is audited, and reports the resulting
 * Delivery state — provider tokens never enter the request, response, or
 * Audit.
 */

export default function AdminChannelsList() {
  const { generation } = useAuth()
  const query = useAdminChannels(generation)

  return (
    <section className="page">
      <h1>Channels</h1>
      <p className="muted">
        Notification channels are configured on the Server (design §17.4): provider tokens live
        in dedicated secret files and never reach this page; destinations are shown redacted.
        Delivery policy — automatic attempt bound and retry backoff — is Server config, not
        browser state.
      </p>
      {!query.data && query.isPending && (
        <p className="panel-state" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading Channels…
        </p>
      )}
      {!query.data && query.isError && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Error" tone="error" />{' '}
          {query.error instanceof Error ? query.error.message : 'Unable to load Channels'}
          <button type="button" className="text-action" onClick={() => void query.refetch()}>
            Try again
          </button>
        </p>
      )}
      {query.data && query.data.length === 0 && (
        <p className="panel-state">
          <StatusBadge status="Empty" tone="ok" /> No notification channel is configured.
          Deliveries are only created for configured channels.
        </p>
      )}
      {query.data && query.data.length > 0 && (
        <div className="table-wrap">
          <table className="node-table">
            <caption className="sr-only">Configured notification channels</caption>
            <thead>
              <tr>
                <th scope="col">Channel</th>
                <th scope="col">Kind</th>
                <th scope="col">Destination</th>
                <th scope="col">Provider reference</th>
                <th scope="col">Policy</th>
              </tr>
            </thead>
            <tbody>
              {query.data.map((channel) => (
                <ChannelRow key={channel.channelId} channel={channel} />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  )
}

function ChannelRow({ channel }: { channel: ChannelDto }) {
  return (
    <tr>
      <td data-label="Channel">
        <Link to={`/admin/alerts/channels/${encodeURIComponent(channel.channelId)}`}>
          {channel.channelId}
        </Link>
      </td>
      <td data-label="Kind">{channel.channelKind}</td>
      <td data-label="Destination">
        <span title="Redacted destination summary">{channel.destination}</span>{' '}
        <small className="muted">(redacted)</small>
      </td>
      <td data-label="Provider reference">
        <span title="Redacted provider reference">{channel.providerRef}</span>
      </td>
      <td data-label="Policy">
        {channel.enabled ? (
          <StatusBadge status="Enabled" tone="ok" />
        ) : (
          <StatusBadge status="Disabled" tone="neutral" />
        )}{' '}
        {channel.maxAttempts} attempts · {channel.retryBaseSeconds}s backoff
      </td>
    </tr>
  )
}

/** PAGE-ADMIN-CHANNEL: policy and test action. */
export function AdminChannelDetail() {
  const { channelId = '' } = useParams()
  const { generation } = useAuth()
  const status = useAuth().status
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const query = useAdminChannelDetail(generation, channelId)
  const [testing, setTesting] = useState(false)
  const [testError, setTestError] = useState<string | null>(null)
  const [testResult, setTestResult] = useState<string | null>(null)

  const onTest = async () => {
    if (
      !window.confirm(
        'Send a test notification through this channel? The test is audited and separate from business Incidents.',
      )
    ) {
      return
    }
    setTesting(true)
    setTestError(null)
    setTestResult(null)
    try {
      const response = await testNotificationChannelEntry(channelId, csrfToken)
      setTestResult(
        `Test Event ${response.eventId.slice(0, 8)} sent — Delivery ${response.deliveryId.slice(0, 8)}: ${deliveryStateLabel(response.state)}.`,
      )
    } catch (testFailure) {
      if (testFailure instanceof AdminApiError && testFailure.code === 'channel_disabled') {
        setTestError('This channel is disabled; enable it before sending a test.')
      } else {
        setTestError(
          testFailure instanceof AdminApiError
            ? testFailure.message
            : 'Unable to send the test notification.',
        )
      }
    } finally {
      setTesting(false)
    }
  }

  return (
    <section className="page">
      <p>
        <Link to="/admin/alerts/channels" className="text-action">
          ← Channels
        </Link>
      </p>
      <h1>Channel {channelId}</h1>
      {!query.data && query.isPending && (
        <p className="panel-state" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading the Channel…
        </p>
      )}
      {!query.data && query.isError && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Error" tone="error" />{' '}
          {query.error instanceof Error ? query.error.message : 'Unable to load the Channel'}
        </p>
      )}
      {query.data && (
        <>
          <dl className="detail-list">
            <dt>Channel</dt>
            <dd>{query.data.channelId}</dd>
            <dt>Kind</dt>
            <dd>{query.data.channelKind}</dd>
            <dt>Enabled</dt>
            <dd>
              {query.data.enabled ? (
                <StatusBadge status="Enabled" tone="ok" />
              ) : (
                <StatusBadge status="Disabled" tone="neutral" />
              )}
            </dd>
            <dt>Destination</dt>
            <dd>
              <span title="Redacted destination summary">{query.data.destination}</span>{' '}
              <small className="muted">(redacted)</small>
            </dd>
            <dt>Provider reference</dt>
            <dd>
              <span title="Redacted provider reference">{query.data.providerRef}</span>{' '}
              <small className="muted">(secret file; token never shown)</small>
            </dd>
            <dt>Maximum automatic attempts</dt>
            <dd>{query.data.maxAttempts}</dd>
            <dt>Retry backoff base</dt>
            <dd>{query.data.retryBaseSeconds} s (exponential, capped at 1 h)</dd>
          </dl>

          <div className="field">
            <button
              type="button"
              className="button-primary"
              onClick={() => void onTest()}
              disabled={testing || !query.data.enabled}
            >
              {testing ? 'Sending…' : 'Send test notification'}
            </button>
            {!query.data.enabled && (
              <p className="muted">This channel is disabled; tests are refused by the Server.</p>
            )}
            {testError && (
              <p className="field-error" role="alert">
                {testError}
              </p>
            )}
            {testResult && (
              <p className="form-ok" role="status">
                {testResult}
              </p>
            )}
          </div>
          <p className="muted">
            Test notifications create a Notification Event of kind Test — never an Incident or a
            business transition — and every test is recorded in the Audit log with the Delivery
            outcome.
          </p>
        </>
      )}
    </section>
  )
}

function deliveryStateLabel(state: string): string {
  switch (state) {
    case 'succeeded':
      return 'delivered'
    case 'failed':
      return 'failed'
    case 'retry_scheduled':
      return 'retry scheduled'
    case 'dead_letter':
      return 'dead letter'
    case 'suppressed':
      return 'suppressed'
    default:
      return state
  }
}
