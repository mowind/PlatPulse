import { useState } from 'react'
import {
  AdminApiError,
  revokeOtherSessionsEntry,
  revokeSessionEntry,
  useAdminSessions,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { StatusBadge, formatObservedAt } from '../components/StatusBadge'
import { Badge } from '../components/ui/badge'
import { Button } from '../components/ui/button'
import { CardX } from '../components/ui/card-x'
import { DataTooltip } from '../components/ui/data-tooltip'
import { Empty } from '../components/ui/empty'
import { cn } from '../lib/utils'
import { SURFACE_CARD } from '../lib/surface'
import type { SessionItem } from '../api/generated'

const TH =
  'px-3 py-2 text-left text-xs font-medium text-muted-foreground'
const TD = 'px-3 py-2 align-top'

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
    <section className="w-full" data-slot="sessions-page">
      <header className="flex flex-col gap-1">
        <h1 className="text-lg font-semibold">Sessions</h1>
        <p className="text-sm text-muted-foreground">
          Coarse Session metadata only: creation, last activity, expiry, and a
          coarse client hint. Tokens, full User-Agents, and raw IPs are never
          stored or displayed.
        </p>
      </header>
      {message && (
        <p
          role="status"
          className="mt-4 rounded-md border border-success/30 bg-success/10 px-4 py-3 text-sm text-success"
        >
          {message}
        </p>
      )}
      {error && (
        <p
          role="alert"
          className="mt-4 rounded-md border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm text-destructive"
        >
          {error}
        </p>
      )}
      <CardX
        size="medium"
        bordered={false}
        segmented
        data-slot="sessions-panel"
        className={cn('mt-4 rounded-md', SURFACE_CARD)}
        header={
          <div className="flex w-full items-center gap-2">
            <h2 className="min-w-0 flex-1 truncate text-sm font-medium">Active Sessions</h2>
            <DataTooltip
              placement="left"
              content="Only coarse Session metadata is shown: creation, last activity, expiry, and a client hint. Tokens, full User-Agents, and raw IPs are never stored or displayed."
            >
              <Button variant="ghost" size="icon-sm" aria-label="About Session metadata">
                ?
              </Button>
            </DataTooltip>
            {query.data && (
              <Badge variant="secondary" data-slot="sessions-count">
                {sessions.length}
              </Badge>
            )}
          </div>
        }
        footer={
          currentSession && sessions.length > 1 ? (
            <div className="flex flex-wrap items-center gap-3" data-slot="sessions-others-actions">
              {confirmingAll ? (
                <>
                  <span className="text-xs text-muted-foreground">
                    Revoke every other Session of your account? The current
                    Session stays active.
                  </span>
                  <Button
                    variant="destructive"
                    size="sm"
                    disabled={busy}
                    onClick={() => void revokeAll()}
                  >
                    Confirm revoke all others
                  </Button>
                  <Button variant="ghost" size="sm" onClick={() => setConfirmingAll(false)}>
                    Cancel
                  </Button>
                </>
              ) : (
                <Button variant="outline" size="sm" onClick={() => setConfirmingAll(true)}>
                  Revoke all other Sessions
                </Button>
              )}
            </div>
          ) : undefined
        }
      >
        {!query.data && query.isPending && (
          <div role="status" className="flex items-center gap-2 py-6 text-sm text-muted-foreground">
            <StatusBadge status="Starting" tone="neutral" /> Loading Sessions…
          </div>
        )}
        {!query.data && query.isError && (
          <div role="alert" className="flex flex-wrap items-center gap-2 py-6 text-sm">
            <StatusBadge status="Error" tone="error" />
            <span className="text-destructive">
              {query.error instanceof Error ? query.error.message : 'Unable to load Sessions'}
            </span>
            <Button variant="ghost" size="sm" onClick={() => void query.refetch()}>
              Try again
            </Button>
          </div>
        )}
        {query.data && sessions.length === 0 && <Empty description="No active Sessions." />}
        {query.data && sessions.length > 0 && (
          <div data-slot="sessions-table" className="overflow-x-auto">
            <table className="w-full min-w-[44rem] text-sm">
              <caption className="sr-only">Active human Sessions</caption>
              <thead>
                <tr className="border-b border-border">
                  <th scope="col" className={TH}>
                    User
                  </th>
                  <th scope="col" className={TH}>
                    Client
                  </th>
                  <th scope="col" className={TH}>
                    Created
                  </th>
                  <th scope="col" className={TH}>
                    Last active
                  </th>
                  <th scope="col" className={TH}>
                    Expires
                  </th>
                  <th scope="col" className={TH}>
                    Action
                  </th>
                </tr>
              </thead>
              <tbody>
                {sessions.map((session) => (
                  <tr key={session.sessionId} className="border-b border-border/60 align-top">
                    <th scope="row" className={cn(TD, 'text-left font-medium')}>
                      <span className="flex flex-wrap items-center gap-2">
                        <span className="min-w-0 break-words">{session.username}</span>
                        <span className="text-[11px] font-normal text-muted-foreground">
                          · {session.role}
                        </span>
                        {session.current && <StatusBadge status="Current" tone="ok" />}
                      </span>
                    </th>
                    <td className={TD}>{session.clientHint}</td>
                    <td className={cn(TD, 'whitespace-nowrap')}>
                      {formatObservedAt(session.createdAt)}
                    </td>
                    <td className={cn(TD, 'whitespace-nowrap')}>
                      {formatObservedAt(session.lastSeenAt)}
                    </td>
                    <td className={cn(TD, 'whitespace-nowrap')}>
                      {formatObservedAt(session.expiresAt)}
                    </td>
                    <td className={TD}>
                      {session.current ? (
                        <span className="text-[11px] text-muted-foreground">This session</span>
                      ) : confirmingId === session.sessionId ? (
                        <div className="flex min-w-[14rem] flex-col gap-2">
                          <span className="text-xs text-muted-foreground">
                            Revoke now? The user's streams close immediately.
                          </span>
                          <div className="flex flex-wrap gap-2">
                            <Button
                              variant="destructive"
                              size="sm"
                              disabled={busy}
                              onClick={() => void revoke(session)}
                            >
                              Confirm revoke
                            </Button>
                            <Button variant="ghost" size="sm" onClick={() => setConfirmingId(null)}>
                              Cancel
                            </Button>
                          </div>
                        </div>
                      ) : (
                        <Button
                          variant="outline"
                          size="sm"
                          onClick={() => setConfirmingId(session.sessionId)}
                        >
                          Revoke
                        </Button>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </CardX>
    </section>
  )
}
