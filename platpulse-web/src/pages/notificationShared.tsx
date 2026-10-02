import { useState, type FormEvent, type ReactNode } from 'react'
import { Link, NavLink } from 'react-router'

import { AdminApiError, useAdminNotificationRequest } from '../api/admin'
import type { NotificationRequestResult } from '../api/generated'
import { useAuth } from '../auth/AuthContext'
import { StatusBadge, formatObservedAt } from '../components/StatusBadge'
import { Button } from '../components/ui/button'
import { CardX } from '../components/ui/card-x'
import { Input } from '../components/ui/input'
import { SURFACE_CARD_STATIC, SURFACE_TOOLBAR } from '../lib/surface'
import { cn } from '../lib/utils'

/**
 * Shared vocabulary for the notification Admin pages (issue #206). The Server
 * owns the request ledger, the Event/Delivery rows, and the provider result;
 * these helpers only render what it sends, so every page tells the same story
 * about request-level dedup versus external delivery guarantees.
 */

export const CARD_SURFACE = cn('rounded-md border-none', SURFACE_CARD_STATIC)

export function shortId(value: string): string {
  if (value.length <= 12) return value
  return value.slice(0, 8) + '…' + value.slice(-4)
}

export function errorMessage(error: unknown, fallback: string): string {
  return error instanceof Error && error.message.length > 0 ? error.message : fallback
}

/** A transport failure means the Server may never have seen the request. */
export function indeterminateOutcome(error: unknown): boolean {
  return error instanceof AdminApiError && error.code === 'network_unavailable'
}

export const INDETERMINATE_OUTCOME =
  'The request may not have reached the Server, so the outcome is unknown. The Server recorded the same request id for this command, so look it up below to reconcile the result; the browser never re-sends automatically.'

/**
 * Delivery states are Server-owned vocabulary (design §17.4). The word is
 * presented exactly as the Server sends it and only the tone is WebUI-owned.
 */
const DELIVERY_STATE_TONES: Record<string, 'ok' | 'error' | 'neutral'> = {
  succeeded: 'ok',
  failed: 'error',
  dead_letter: 'error',
}

export function deliveryStateTone(state: string): 'ok' | 'error' | 'neutral' {
  return DELIVERY_STATE_TONES[state] ?? 'neutral'
}

/** The Server accepts a retry only from these states (design §17.4). */
export function deliveryIsRetryable(state: string): boolean {
  return state === 'retry_scheduled' || state === 'failed' || state === 'dead_letter'
}

export type FormFeedback = { tone: 'ok' | 'error'; message: string }

export function FormFeedbackNote({ feedback }: { feedback: FormFeedback | null }) {
  if (!feedback) return null
  return (
    <p
      role={feedback.tone === 'error' ? 'alert' : 'status'}
      className={cn(
        'rounded-md border px-3 py-2 text-sm',
        feedback.tone === 'error'
          ? 'border-destructive/40 bg-destructive/5 text-destructive'
          : 'border-border/60 bg-muted/30',
      )}
    >
      {feedback.message}
    </p>
  )
}

export function DetailList({ children }: { children: ReactNode }) {
  return <dl className="grid grid-cols-1 gap-3 sm:grid-cols-2">{children}</dl>
}

export function DetailItem({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="min-w-0">
      <dt className="text-xs font-medium tracking-wider text-muted-foreground">{label}</dt>
      <dd className="mt-0.5 min-w-0 break-words text-sm">{children}</dd>
    </div>
  )
}

const SECTION_LINK =
  'inline-flex min-h-11 items-center rounded-md px-3 py-2 text-sm text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50'

/** In-page section switch for the notification surface (§15.9). */
export function NotificationSectionNav() {
  return (
    <nav
      aria-label="Notification sections"
      className={cn('flex flex-wrap items-center gap-1 rounded-md p-1', SURFACE_TOOLBAR)}
    >
      <NavLink
        to="/admin/notifications"
        end
        className={({ isActive }) => cn(SECTION_LINK, isActive && 'bg-accent font-semibold text-foreground')}
      >
        Overview
      </NavLink>
      <NavLink
        to="/admin/notifications/events"
        className={({ isActive }) => cn(SECTION_LINK, isActive && 'bg-accent font-semibold text-foreground')}
      >
        Events
      </NavLink>
      <NavLink
        to="/admin/notifications/deliveries"
        className={({ isActive }) => cn(SECTION_LINK, isActive && 'bg-accent font-semibold text-foreground')}
      >
        Deliveries
      </NavLink>
      <NavLink
        to="/admin/notifications/channels"
        className={({ isActive }) => cn(SECTION_LINK, isActive && 'bg-accent font-semibold text-foreground')}
      >
        Channels
      </NavLink>
    </nav>
  )
}

function RequestResult({ result }: { result: NotificationRequestResult }) {
  return (
    <div className="space-y-2" data-slot="notification-request-result">
      <DetailList>
        <DetailItem label="Request id">
          <span className="font-mono">{result.requestId}</span>
        </DetailItem>
        <DetailItem label="Command kind">{result.commandKind}</DetailItem>
        <DetailItem label="Server event">
          {result.eventId ? (
            <Link className="inline-flex min-h-11 items-center underline underline-offset-4" to={'/admin/notifications/events/' + result.eventId}>
              {shortId(result.eventId)}
            </Link>
          ) : (
            'None (delivery retry carries no new event)'
          )}
        </DetailItem>
        <DetailItem label="Delivery">
          <Link className="inline-flex min-h-11 items-center underline underline-offset-4" to={'/admin/notifications/deliveries/' + result.delivery.deliveryId}>
            {shortId(result.delivery.deliveryId)}
          </Link>
        </DetailItem>
        <DetailItem label="Delivery state">
          <StatusBadge status={result.delivery.state} tone={deliveryStateTone(result.delivery.state)} />
        </DetailItem>
        <DetailItem label="Recorded at">{formatObservedAt(result.createdAt)}</DetailItem>
        <DetailItem label="Lookup expires">{formatObservedAt(result.expiresAt)}</DetailItem>
        <DetailItem label="Audit entry">{String(result.auditEventId)}</DetailItem>
      </DetailList>
      <p className="text-xs text-muted-foreground">
        This is the Server command result, not a claim that Telegram accepted or delivered the message. The Delivery
        state above is the recorded provider outcome.
      </p>
    </div>
  )
}

/**
 * Looks up a recorded Server command by the request id an Owner supplied. Used
 * after a timeout or network failure: the result is reconciled from the Server
 * rather than by re-sending (issue #206 Story 32).
 */
export function NotificationRequestPanel({
  initialRequestId = '',
  title = 'Request result lookup',
}: {
  initialRequestId?: string
  title?: string
}) {
  const { generation } = useAuth()
  const [draft, setDraft] = useState(initialRequestId)
  const [lookupId, setLookupId] = useState(initialRequestId)
  const query = useAdminNotificationRequest(generation, lookupId)
  const unknownRequest =
    query.error instanceof AdminApiError &&
    (query.error.code === 'notification_request_not_found' || query.error.code === 'not_found')

  function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setLookupId(draft.trim())
  }

  return (
    <CardX size="medium" className={CARD_SURFACE} title={title}>
      <div className="space-y-3">
        <p className="text-sm text-muted-foreground">
          A request id identifies one Server command. Re-using the same id returns the recorded result instead of
          performing the command again, so enter the id from a test or retry response whenever the outcome is
          uncertain.
        </p>
        <form className="flex flex-wrap items-end gap-2" onSubmit={onSubmit}>
          <div className="min-w-0 flex-1">
            <label className="text-xs font-medium tracking-wider text-muted-foreground" htmlFor="notification-request-id">
              Request id
            </label>
            <Input
              id="notification-request-id"
              value={draft}
              autoComplete="off"
              spellCheck={false}
              onChange={(event) => setDraft(event.target.value)}
              className="mt-1"
            />
          </div>
          <Button type="submit" size="sm" className="min-h-11" disabled={draft.trim().length === 0}>
            Look up request
          </Button>
        </form>
        {lookupId.length === 0 && <p className="text-sm text-muted-foreground">No request id entered yet.</p>}
        {lookupId.length > 0 && !query.data && query.isPending && (
          <p role="status" className="text-sm">
            <StatusBadge status="Starting" tone="neutral" /> Looking up the request…
          </p>
        )}
        {lookupId.length > 0 && !query.data && query.isError && unknownRequest && (
          <p role="status" className="text-sm">
            The Server has no unexpired record for this request id. It may never have been accepted, or its retention
            window has elapsed; nothing was re-sent.
          </p>
        )}
        {lookupId.length > 0 && !query.data && query.isError && !unknownRequest && (
          <div role="alert" className="space-y-2 text-sm">
            <p>{errorMessage(query.error, 'Unable to look up the request')}</p>
            <Button variant="link" size="sm" onClick={() => void query.refetch()}>
              Try again
            </Button>
          </div>
        )}
        {query.data && <RequestResult result={query.data} />}
      </div>
    </CardX>
  )
}
