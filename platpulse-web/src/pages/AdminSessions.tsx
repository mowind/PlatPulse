import { useState } from 'react'
import {
  AdminApiError,
  revokeOtherSessionsEntry,
  revokeSessionEntry,
  useAdminSessions,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { StatusBadge, formatObservedAt } from '../components/StatusBadge'
import type { SessionItem } from '../api/generated'

/**
 * PAGE-ACCESS-SESSIONS (design §12.3, issue #47): coarse, non-sensitive
 * Session review and revoke. Only creation, last activity, expiry, and a
 * coarse client hint are shown — never tokens, full User-Agents, or raw
 * IPs. Revoking a Session closes its bound Admin/Public streams and sends
 * the access-generation signal; "keep current" and "revoke all others"
 * remain distinct operations.
 */
export default function AdminSessions() {
  const { status, generation } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const query = useAdminSessions(generation)
  const [message, setMessage] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [confirmingId, setConfirmingId] = useState<string | null>(null)
  const [confirmingAll, setConfirmingAll] = useState(false)
  const [busy, setBusy] = useState(false)

  async function revoke(session: SessionItem) {
    setMessage(null)
    setError(null)
    setBusy(true)
    try {
      const result = await revokeSessionEntry(session.sessionId, csrfToken)
      setMessage(
        `Session for ${session.username} revoked at ${formatObservedAt(result.revokedAt)}. The user's open tabs must sign in again.`,
      )
      setConfirmingId(null)
    } catch (caught) {
      // Revoke race: a concurrent operator already revoked it. Reload the
      // authoritative listing instead of showing an optimistic row.
      if (caught instanceof AdminApiError && caught.code === 'session_already_revoked') {
        void query.refetch()
      }
      setError(caught instanceof Error ? caught.message : 'Unable to revoke the session')
      setConfirmingId(null)
    } finally {
      setBusy(false)
    }
  }

  async function revokeAll() {
    setMessage(null)
    setError(null)
    setBusy(true)
    try {
      const result = await revokeOtherSessionsEntry(csrfToken)
      setMessage(
        `${result.revokedCount} other session${result.revokedCount === 1 ? '' : 's'} revoked. This session stays active.`,
      )
      setConfirmingAll(false)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : 'Unable to revoke the other sessions')
      setConfirmingAll(false)
    } finally {
      setBusy(false)
    }
  }

  const sessions = query.data?.sessions ?? []
  const currentSession = sessions.find((session) => session.current)

  return (
    <section className="page">
      <h1>Sessions</h1>
      <p className="muted">
        Coarse Session metadata only: creation, last activity, expiry, and a
        coarse client hint. Tokens, full User-Agents, and raw IPs are never
        stored or displayed.
      </p>
      {message && (
        <p className="form-success" role="status">
          {message}
        </p>
      )}
      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}
      <article className="panel">
        <div className="panel-heading">
          <h2>Active Sessions</h2>
          {query.data && <span className="panel-count">{sessions.length}</span>}
        </div>
        {!query.data && query.isPending && (
          <p className="panel-state" role="status">
            <StatusBadge status="Starting" tone="neutral" /> Loading Sessions…
          </p>
        )}
        {!query.data && query.isError && (
          <p className="panel-state" role="alert">
            <StatusBadge status="Error" tone="error" />{' '}
            {query.error instanceof Error ? query.error.message : 'Unable to load Sessions'}
            <button type="button" className="text-action" onClick={() => void query.refetch()}>
              Try again
            </button>
          </p>
        )}
        {query.data && sessions.length === 0 && (
          <p className="panel-state">
            <StatusBadge status="Empty" tone="ok" /> No active Sessions.
          </p>
        )}
        {query.data && sessions.length > 0 && (
          <div className="table-wrap">
            <table className="sessions-table">
              <caption className="sr-only">Active human Sessions</caption>
              <thead>
                <tr>
                  <th scope="col">User</th>
                  <th scope="col">Client</th>
                  <th scope="col">Created</th>
                  <th scope="col">Last active</th>
                  <th scope="col">Expires</th>
                  <th scope="col">Action</th>
                </tr>
              </thead>
              <tbody>
                {sessions.map((session) => (
                  <tr key={session.sessionId}>
                    <th scope="row" data-label="User">
                      {session.username}
                      <small className="muted"> · {session.role}</small>
                      {session.current && <StatusBadge status="Current" tone="ok" />}
                    </th>
                    <td data-label="Client">{session.clientHint}</td>
                    <td data-label="Created">{formatObservedAt(session.createdAt)}</td>
                    <td data-label="Last active">{formatObservedAt(session.lastSeenAt)}</td>
                    <td data-label="Expires">{formatObservedAt(session.expiresAt)}</td>
                    <td data-label="Action">
                      {session.current ? (
                        <small className="muted">This session</small>
                      ) : confirmingId === session.sessionId ? (
                        <>
                          <span className="confirm-copy">
                            Revoke now? The user's streams close immediately.
                          </span>
                          <button
                            type="button"
                            className="danger-action"
                            disabled={busy}
                            onClick={() => void revoke(session)}
                          >
                            Confirm revoke
                          </button>
                          <button
                            type="button"
                            className="text-action"
                            onClick={() => setConfirmingId(null)}
                          >
                            Cancel
                          </button>
                        </>
                      ) : (
                        <button
                          type="button"
                          className="text-action"
                          onClick={() => setConfirmingId(session.sessionId)}
                        >
                          Revoke
                        </button>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
        {currentSession && sessions.length > 1 && (
          <div className="session-others-actions">
            {confirmingAll ? (
              <>
                <span className="confirm-copy">
                  Revoke every other Session of your account? The current
                  Session stays active.
                </span>
                <button
                  type="button"
                  className="danger-action"
                  disabled={busy}
                  onClick={() => void revokeAll()}
                >
                  Confirm revoke all others
                </button>
                <button
                  type="button"
                  className="text-action"
                  onClick={() => setConfirmingAll(false)}
                >
                  Cancel
                </button>
              </>
            ) : (
              <button
                type="button"
                className="text-action"
                onClick={() => setConfirmingAll(true)}
              >
                Revoke all other Sessions
              </button>
            )}
          </div>
        )}
      </article>
    </section>
  )
}
