import { useEffect, useId, useRef, useState } from 'react'
import {
  AdminApiError,
  revokeOtherSessionsEntry,
  revokeSessionEntry,
  useAdminSessions,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { StatusBadge, formatObservedAt } from '../components/StatusBadge'
import { Button } from '../components/ui/button'
import { Empty } from '../components/ui/empty'
import { cn } from '../lib/utils'
import type { SessionItem } from '../api/generated'
import {
  ADMIN_ROW,
  ADMIN_SECONDARY,
  ADMIN_TD,
  ADMIN_TH,
  AdminPageHeader,
  AdminPanel,
} from '../components/AdminPage'

const SESSION_PRIVACY_NOTE =
  'Only coarse Session metadata is shown: creation, last activity, expiry, and a coarse client hint. Tokens, full User-Agents, and raw IPs are never stored or displayed.'

/** Time cells keep one line on wide desktops (xl); narrower tables and the
 * stacked phone cards wrap them instead of overflowing. */
const TIME_TD = cn(ADMIN_TD, 'min-w-0 tabular-nums xl:whitespace-nowrap')

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
  const [busy, setBusy] = useState(false)
  // A finished revoke removes or collapses the control that had focus; move
  // focus to the outcome message instead of dropping it on the page body.
  const feedbackRef = useRef<HTMLParagraphElement>(null)
  const focusFeedback = useRef(false)
  useEffect(() => {
    if (!focusFeedback.current || (!message && !error)) return
    focusFeedback.current = false
    feedbackRef.current?.focus()
  }, [message, error])

  async function revoke(session: SessionItem) {
    setMessage(null)
    setError(null)
    setBusy(true)
    focusFeedback.current = true
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

  async function revokeAll(): Promise<void> {
    setMessage(null)
    setError(null)
    setBusy(true)
    focusFeedback.current = true
    try {
      const result = await revokeOtherSessionsEntry(csrfToken)
      setMessage(
        `${result.revokedCount} other session${result.revokedCount === 1 ? '' : 's'} revoked. This session stays active.`,
      )
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : 'Unable to revoke the other sessions')
    } finally {
      setBusy(false)
    }
  }

  const sessions = query.data?.sessions ?? []
  const currentSession = sessions.find((session) => session.current)

  return (
    <section className="w-full space-y-4" data-slot="sessions-page">
      <AdminPageHeader
        title="Sessions"
        description="Signed-in human Sessions; revoke one, or every Session except your current one."
        info={{ label: 'About Session metadata', content: <p>{SESSION_PRIVACY_NOTE}</p> }}
      />
      {message && (
        <p
          ref={feedbackRef}
          role="status"
          tabIndex={-1}
          className="rounded-md outline-none border border-success/30 bg-success/10 px-4 py-3 text-sm text-success [overflow-wrap:anywhere]"
        >
          {message}
        </p>
      )}
      {error && (
        <p
          ref={feedbackRef}
          role="alert"
          tabIndex={-1}
          className="rounded-md outline-none border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm text-destructive [overflow-wrap:anywhere]"
        >
          {error}
        </p>
      )}
      <AdminPanel
        slot="sessions-panel"
        flush
        title="Active Sessions"
        count={
          query.data && (
            <span data-slot="sessions-count" className="text-xs text-muted-foreground tabular-nums">
              {sessions.length} Session{sessions.length === 1 ? '' : 's'}
            </span>
          )
        }
        footer={
          currentSession && sessions.length > 1 ? (
            <RevokeOthersAction busy={busy} onConfirm={revokeAll} />
          ) : undefined
        }
      >
        {!query.data && query.isPending && (
          <div role="status" className="flex items-center gap-2 px-4 py-6 text-sm text-muted-foreground">
            <StatusBadge status="Starting" tone="neutral" /> Loading Sessions…
          </div>
        )}
        {!query.data && query.isError && (
          <div role="alert" className="flex flex-wrap items-center gap-2 px-4 py-6 text-sm">
            <StatusBadge status="Error" tone="error" />
            <span className="min-w-0 text-destructive [overflow-wrap:anywhere]">
              {query.error instanceof Error ? query.error.message : 'Unable to load Sessions'}
            </span>
            <Button variant="ghost" size="sm" onClick={() => void query.refetch()}>
              Try again
            </Button>
          </div>
        )}
        {query.data && sessions.length === 0 && <Empty description="No active Sessions." />}
        {query.data && sessions.length > 0 && (
          <div data-slot="sessions-table" className="overflow-x-auto max-md:px-3 max-md:pt-3">
            <table data-stack className="w-full text-sm">
              <caption className="sr-only">Active human Sessions</caption>
              <thead>
                <tr className="border-b border-border/60">
                  <th scope="col" className={ADMIN_TH}>User</th>
                  <th scope="col" className={ADMIN_TH}>Client</th>
                  <th scope="col" className={ADMIN_TH}>Created</th>
                  <th scope="col" className={ADMIN_TH}>Last active</th>
                  <th scope="col" className={ADMIN_TH}>Expires</th>
                  <th scope="col" className={ADMIN_TH}>Action</th>
                </tr>
              </thead>
              <tbody>
                {sessions.map((session) => (
                  <SessionRow
                    key={session.sessionId}
                    session={session}
                    confirming={confirmingId === session.sessionId}
                    busy={busy}
                    onRequest={() => setConfirmingId(session.sessionId)}
                    onCancel={() => setConfirmingId(null)}
                    onConfirm={() => void revoke(session)}
                  />
                ))}
              </tbody>
            </table>
          </div>
        )}
      </AdminPanel>
    </section>
  )
}

/** Moves focus to the confirmation's Cancel on open and back to the opening
 * action on cancel, so the toggle never drops focus onto the page body. */
function useConfirmFocus(confirming: boolean) {
  const opener = useRef<HTMLButtonElement>(null)
  const cancel = useRef<HTMLButtonElement>(null)
  const pending = useRef<'opener' | 'cancel' | null>(null)
  useEffect(() => {
    const target = pending.current === 'opener' ? opener.current : pending.current === 'cancel' ? cancel.current : null
    pending.current = null
    target?.focus()
  }, [confirming])
  return {
    opener,
    cancel,
    focusCancelNext: () => {
      pending.current = 'cancel'
    },
    focusOpenerNext: () => {
      pending.current = 'opener'
    },
  }
}

function SessionRow({
  session,
  confirming,
  busy,
  onRequest,
  onCancel,
  onConfirm,
}: {
  session: SessionItem
  confirming: boolean
  busy: boolean
  onRequest: () => void
  onCancel: () => void
  onConfirm: () => void
}) {
  const focus = useConfirmFocus(confirming)
  const promptId = useId()
  return (
    <tr className={ADMIN_ROW}>
      <th scope="row" data-label="User" className={cn(ADMIN_TD, 'min-w-0 text-left font-normal')}>
        <span className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
          <span className="min-w-0 font-medium [overflow-wrap:anywhere]">{session.username}</span>
          <span className={ADMIN_SECONDARY}>{session.role}</span>
          {session.current && <StatusBadge status="Current" tone="ok" />}
        </span>
      </th>
      <td data-label="Client" className={cn(ADMIN_TD, 'min-w-0 [overflow-wrap:anywhere]')}>
        {session.clientHint}
      </td>
      <td data-label="Created" className={TIME_TD}>{formatObservedAt(session.createdAt)}</td>
      <td data-label="Last active" className={TIME_TD}>{formatObservedAt(session.lastSeenAt)}</td>
      <td data-label="Expires" className={TIME_TD}>{formatObservedAt(session.expiresAt)}</td>
      <td data-label="Action" className={cn(ADMIN_TD, 'min-w-0 py-1 max-md:w-full')}>
        {session.current ? (
          <span className={cn(ADMIN_SECONDARY, 'inline-flex items-center md:min-h-11')}>This session</span>
        ) : confirming ? (
          <div
            role="group"
            aria-labelledby={promptId}
            data-slot="session-revoke-confirm"
            className="flex w-full min-w-0 flex-col gap-1 py-1.5 md:w-56"
          >
            <span id={promptId} className={ADMIN_SECONDARY}>
              Revoke now? The user's streams close immediately.
            </span>
            <div className="flex flex-wrap gap-2">
              <Button variant="destructive" size="sm" disabled={busy} onClick={onConfirm}>
                Confirm revoke
              </Button>
              <Button
                ref={focus.cancel}
                variant="ghost"
                size="sm"
                onClick={() => {
                  focus.focusOpenerNext()
                  onCancel()
                }}
              >
                Cancel
              </Button>
            </div>
          </div>
        ) : (
          <Button
            ref={focus.opener}
            variant="outline"
            size="sm"
            onClick={() => {
              focus.focusCancelNext()
              onRequest()
            }}
          >
            Revoke
          </Button>
        )}
      </td>
    </tr>
  )
}

/** "Revoke all other Sessions": the current Session always stays active —
 * distinct from revoking any single row. */
function RevokeOthersAction({ busy, onConfirm }: { busy: boolean; onConfirm: () => Promise<void> }) {
  const [confirming, setConfirming] = useState(false)
  const focus = useConfirmFocus(confirming)
  const promptId = useId()
  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-2" data-slot="sessions-others-actions">
      {confirming ? (
        <div role="group" aria-labelledby={promptId} className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-2">
          <span id={promptId} className={cn(ADMIN_SECONDARY, 'min-w-0 basis-full sm:basis-auto')}>
            Revoke every other Session of your account? The current Session stays active.
          </span>
          <Button
            variant="destructive"
            size="sm"
            disabled={busy}
            onClick={() => {
              // The page moves focus to the outcome message once it settles.
              void onConfirm().finally(() => setConfirming(false))
            }}
          >
            Confirm revoke all others
          </Button>
          <Button
            ref={focus.cancel}
            variant="ghost"
            size="sm"
            onClick={() => {
              focus.focusOpenerNext()
              setConfirming(false)
            }}
          >
            Cancel
          </Button>
        </div>
      ) : (
        <Button
          ref={focus.opener}
          variant="outline"
          size="sm"
          onClick={() => {
            focus.focusCancelNext()
            setConfirming(true)
          }}
        >
          Revoke all other Sessions
        </Button>
      )}
    </div>
  )
}
