import { useRef, useState, type FormEvent } from 'react'
import { useSearchParams } from 'react-router'
import { AdminApiError, cancelSilenceEntry, createSilenceEntry, useAdminSilences } from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { Alert, AlertDescription, AlertTitle } from '../components/ui/alert'
import { Button } from '../components/ui/button'
import { CardX } from '../components/ui/card-x'
import { Empty } from '../components/ui/empty'
import { Input, Select } from '../components/ui/input'
import { StatusBadge, formatObservedAt } from '../components/StatusBadge'
import { cn } from '../lib/utils'
import { SURFACE_CARD_STATIC, SURFACE_TOOLBAR } from '../lib/surface'
import type { SilenceDto } from '../api/generated'

/**
 * PAGE-ADMIN-SILENCES (issue #205; webui.md §15.8). Owners list every Silence
 * with its Server-owned status (active, expired, or cancelled), the matcher
 * scope, the reason, the recorded window, and the authorship, then create a
 * time-bounded Silence or cancel an active one.
 *
 * A Silence suppresses delivery only: it never stops Alert evaluation and
 * never deletes an Incident. Ending or expiry never replays a suppressed
 * message, and a message already handed to the delivery channel is
 * non-retractable. The Server is authoritative; this surface never
 * optimistically marks a Silence created or cancelled, and a failure shows the
 * sanitized Server error. A transport failure is reported as an unknown outcome
 * with a reconciliation instruction, never as a failed mutation.
 */

const CARD_SURFACE = cn('rounded-md border-none', SURFACE_CARD_STATIC)

const MATCHER_KINDS = ['all', 'agent', 'node', 'network'] as const

const STATUS_FILTERS = ['all', 'active', 'expired', 'cancelled'] as const

function shortId(value: string): string {
  return value.length > 12 ? value.slice(0, 8) + '…' + value.slice(-4) : value
}

function capitalize(value: string): string {
  return value.length === 0 ? value : value.charAt(0).toUpperCase() + value.slice(1)
}

function errorMessage(error: unknown, fallback: string): string {
  return error instanceof Error && error.message.length > 0 ? error.message : fallback
}

/** A browser transport failure is not proof that the mutation did not commit. */
function indeterminateOutcome(error: unknown): boolean {
  return error instanceof AdminApiError && error.code === 'network_unavailable'
}

const INDETERMINATE_OUTCOME =
  'The request may not have reached the Server, so the outcome is unknown. The list was refreshed; verify it before retrying.'

/** A cancelled or never-cancelled Silence has no timestamp; show an explicit
 * placeholder rather than "Never observed", which is Agent-freshness wording. */
function formatOptional(timestamp: string | null | undefined): string {
  return timestamp ? formatObservedAt(timestamp) : '—'
}

function statusTone(status: string): 'ok' | 'neutral' {
  return status === 'active' ? 'ok' : 'neutral'
}

/** Convert a datetime-local control value to the RFC3339 instant the Server
 * requires; an unparseable value becomes empty so validation rejects it. */
function toRfc3339(value: string): string {
  const trimmed = value.trim()
  if (trimmed === '') return ''
  const date = new Date(trimmed)
  return Number.isNaN(date.getTime()) ? '' : date.toISOString()
}

type FormFeedback = { tone: 'ok' | 'error'; message: string }

function FormFeedbackNote({ feedback }: { feedback: FormFeedback | null }) {
  if (feedback === null) return null
  return (
    <p
      role={feedback.tone === 'error' ? 'alert' : 'status'}
      className={cn(
        'rounded-md border p-3 text-sm',
        feedback.tone === 'ok' && 'border-border/60 bg-muted/30',
        feedback.tone === 'error' && 'border-destructive/40 bg-destructive/5 text-destructive',
      )}
    >
      {feedback.message}
    </p>
  )
}

/** The status filter is Server-owned; anything else falls back to "all". */
function readStatus(search: URLSearchParams): string {
  const value = search.get('status') ?? 'all'
  return (STATUS_FILTERS as readonly string[]).includes(value) ? value : 'all'
}

export default function AdminSilencesList() {
  const { generation, status } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const [search, setSearch] = useSearchParams()
  const statusFilter = readStatus(search)
  const query = useAdminSilences(generation, statusFilter === 'all' ? {} : { status: statusFilter })
  const silences: SilenceDto[] = query.data ?? []

  const [matcherKind, setMatcherKind] = useState('all')
  const [matcherValue, setMatcherValue] = useState('')
  const [reason, setReason] = useState('')
  const [startsAt, setStartsAt] = useState('')
  const [endsAt, setEndsAt] = useState('')
  const [confirming, setConfirming] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [feedback, setFeedback] = useState<FormFeedback | null>(null)
  const headingRef = useRef<HTMLHeadingElement>(null)
  const restoreFocusTo = useRef<string | null>(null)
  const [fieldErrors, setFieldErrors] = useState<{
    matcherValue?: string
    reason?: string
    startsAt?: string
    endsAt?: string
  }>({})
  const clearFieldError = (key: 'matcherValue' | 'reason' | 'startsAt' | 'endsAt') => {
    setFieldErrors((current) => {
      if (!(key in current)) return current
      const next = { ...current }
      delete next[key]
      return next
    })
  }

  const setStatus = (value: string) => {
    const next = new URLSearchParams(search)
    if (value === 'all') next.delete('status')
    else next.set('status', value)
    setSearch(next, { replace: false })
  }

  const submit = async (event: FormEvent) => {
    event.preventDefault()
    setFeedback(null)
    const trimmedMatcher = matcherValue.trim()
    const trimmedReason = reason.trim()
    const start = toRfc3339(startsAt)
    const end = toRfc3339(endsAt)
    const nextErrors: {
      matcherValue?: string
      reason?: string
      startsAt?: string
      endsAt?: string
    } = {}
    if (matcherKind !== 'all' && trimmedMatcher === '') {
      nextErrors.matcherValue = 'Enter a matcher value for the selected matcher kind.'
    }
    if (trimmedReason === '') {
      nextErrors.reason = 'Enter a reason for this Silence.'
    }
    if (start === '') {
      nextErrors.startsAt = 'Enter a valid start time.'
    }
    if (end === '') {
      nextErrors.endsAt = 'Enter a valid end time.'
    } else if (start !== '' && start >= end) {
      nextErrors.endsAt = 'Move the end time later than the start time.'
    }
    setFieldErrors(nextErrors)
    if (nextErrors.matcherValue || nextErrors.reason || nextErrors.startsAt || nextErrors.endsAt) {
      // §10.3 (webui.md): the page summary and the field-level messages are
      // both required; the summary keeps the stable wording the Server errors use.
      const message = nextErrors.matcherValue
        ? 'A matcher value is required unless the matcher kind is all.'
        : nextErrors.reason
          ? 'A reason is required for every Silence.'
          : start === '' || end === ''
            ? 'A valid start and end time are required.'
            : 'The end time must be after the start time.'
      setFeedback({ tone: 'error', message })
      return
    }
    setBusy(true)
    try {
      await createSilenceEntry(
        {
          matcherKind,
          matcherValue: matcherKind === 'all' ? null : trimmedMatcher,
          reason: trimmedReason,
          startsAt: start,
          endsAt: end,
        },
        csrfToken,
      )
      setFeedback({ tone: 'ok', message: 'Silence created. The Server is authoritative for it.' })
      setMatcherKind('all')
      setMatcherValue('')
      setReason('')
      setStartsAt('')
      setEndsAt('')
    } catch (error) {
      const message = errorMessage(error, 'Unable to create the Silence.')
      // #202: a transport error does not prove the mutation did not commit.
      setFeedback({
        tone: 'error',
        message: indeterminateOutcome(error) ? message + ' ' + INDETERMINATE_OUTCOME : message,
      })
    } finally {
      setBusy(false)
    }
  }

  const confirmCancel = async (silenceId: string) => {
    setBusy(true)
    setFeedback(null)
    try {
      await cancelSilenceEntry(silenceId, csrfToken)
      setConfirming(null)
      // §15.5 (webui.md): an active-only list drops the confirmed row, so move
      // focus to the panel heading instead of losing it with the unmounted control.
      headingRef.current?.focus()
      setFeedback({
        tone: 'ok',
        message: 'Silence cancelled. Suppressed messages are not replayed.',
      })
    } catch (error) {
      const message = errorMessage(error, 'Unable to cancel the Silence.')
      // The cancellation may have committed even though the response was lost.
      setFeedback({
        tone: 'error',
        message: indeterminateOutcome(error) ? message + ' ' + INDETERMINATE_OUTCOME : message,
      })
    } finally {
      setBusy(false)
    }
  }

  return (
    <section className="w-full min-w-0 space-y-4">
      <div className="space-y-1">
        <h1 ref={headingRef} tabIndex={-1} className="text-lg font-semibold break-words">Silences</h1>
        <p className="text-sm text-muted-foreground">
          Each row is one time-bounded Silence with the Server-owned status and the recorded
          authorship. Creating or cancelling one is an audited Owner action.
        </p>
      </div>

      <Alert>
        <AlertTitle>What a Silence does and does not do</AlertTitle>
        <AlertDescription>
          A Silence suppresses delivery only. It never stops Alert evaluation and never deletes an
          Incident. Ending or expiry never replays a suppressed message, and a message already
          handed to the delivery channel is non-retractable. Failures show the sanitized Server
          error, and this page never optimistically mutates Server truth.
        </AlertDescription>
      </Alert>

      <form
        className={cn('flex flex-wrap items-end gap-3 rounded-md border-none p-3', SURFACE_TOOLBAR)}
        role="group"
        aria-label="Silence filters"
        onSubmit={(event) => event.preventDefault()}
      >
        <label className="flex flex-col gap-1 text-xs font-medium tracking-wider text-muted-foreground">
          Status
          <Select
            className="w-auto min-w-36"
            value={statusFilter}
            onChange={(event) => setStatus(event.target.value)}
          >
            <option value="all">All</option>
            <option value="active">Active</option>
            <option value="expired">Expired</option>
            <option value="cancelled">Cancelled</option>
          </Select>
        </label>
      </form>

      <CardX size="medium" className={CARD_SURFACE} title="Create Silence">
        <form className="space-y-3" onSubmit={submit} aria-label="Create Silence">
          <div className="grid gap-3 sm:grid-cols-2">
            <label className="space-y-1 text-sm">
              <span className="font-medium">Matcher kind</span>
              <Select
                aria-label="Matcher kind"
                value={matcherKind}
                onChange={(event) => {
                  setMatcherKind(event.target.value)
                  clearFieldError('matcherValue')
                }}
              >
                {MATCHER_KINDS.map((kind) => (
                  <option key={kind} value={kind}>
                    {kind === 'all' ? 'All alerts' : capitalize(kind)}
                  </option>
                ))}
              </Select>
            </label>
            <label className="space-y-1 text-sm">
              <span className="font-medium">Matcher value</span>
              <Input
                id="silence-matcher-value"
                aria-label="Matcher value"
                aria-invalid={fieldErrors.matcherValue ? true : undefined}
                aria-describedby={
                  fieldErrors.matcherValue ? 'silence-matcher-value-error' : undefined
                }
                value={matcherValue}
                disabled={matcherKind === 'all'}
                placeholder={matcherKind === 'all' ? 'All alerts' : 'agent-1, node-a, or mainnet'}
                onChange={(event) => {
                  setMatcherValue(event.target.value)
                  clearFieldError('matcherValue')
                }}
              />
              {fieldErrors.matcherValue && (
                <span
                  id="silence-matcher-value-error"
                  role="alert"
                  className="block text-xs text-destructive"
                >
                  {fieldErrors.matcherValue}
                </span>
              )}
            </label>
            <label className="space-y-1 text-sm">
              <span className="font-medium">Starts at</span>
              <Input
                id="silence-starts-at"
                aria-label="Starts at"
                aria-invalid={fieldErrors.startsAt ? true : undefined}
                aria-describedby={fieldErrors.startsAt ? 'silence-starts-at-error' : undefined}
                type="datetime-local"
                value={startsAt}
                onChange={(event) => {
                  setStartsAt(event.target.value)
                  clearFieldError('startsAt')
                }}
              />
              {fieldErrors.startsAt && (
                <span id="silence-starts-at-error" role="alert" className="block text-xs text-destructive">
                  {fieldErrors.startsAt}
                </span>
              )}
            </label>
            <label className="space-y-1 text-sm">
              <span className="font-medium">Ends at</span>
              <Input
                id="silence-ends-at"
                aria-label="Ends at"
                aria-invalid={fieldErrors.endsAt ? true : undefined}
                aria-describedby={fieldErrors.endsAt ? 'silence-ends-at-error' : undefined}
                type="datetime-local"
                value={endsAt}
                onChange={(event) => {
                  setEndsAt(event.target.value)
                  clearFieldError('endsAt')
                }}
              />
              {fieldErrors.endsAt && (
                <span id="silence-ends-at-error" role="alert" className="block text-xs text-destructive">
                  {fieldErrors.endsAt}
                </span>
              )}
            </label>
          </div>
          <label className="block space-y-1 text-sm">
            <span className="font-medium">Reason</span>
            <Input
              id="silence-reason"
              aria-label="Reason"
              aria-invalid={fieldErrors.reason ? true : undefined}
              aria-describedby={fieldErrors.reason ? 'silence-reason-error' : undefined}
              maxLength={500}
              value={reason}
              placeholder="Why delivery is suppressed"
              onChange={(event) => {
                setReason(event.target.value)
                clearFieldError('reason')
              }}
            />
            {fieldErrors.reason && (
              <span id="silence-reason-error" role="alert" className="block text-xs text-destructive">
                {fieldErrors.reason}
              </span>
            )}
          </label>
          <Button type="submit" size="sm" className="min-h-11" disabled={busy || csrfToken.length === 0}>
            Create Silence
          </Button>
          <FormFeedbackNote feedback={feedback} />
        </form>
      </CardX>

      {!query.data && query.isPending && (
        <p className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading the Silences…
        </p>
      )}

      {!query.data && query.isError && (
        <div
          className="flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
          role="alert"
        >
          <StatusBadge status="Error" tone="error" />
          <span className="min-w-0 break-words">
            {errorMessage(query.error, 'Unable to load the Silences')}
          </span>
          <Button variant="link" size="sm" onClick={() => void query.refetch()}>
            Try again
          </Button>
        </div>
      )}

      {query.data && query.isRefetchError && (
        <div
          className="flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
          role="alert"
        >
          <StatusBadge status="Error" tone="error" /> Failed to refresh; showing the last successful
          Silence values.
        </div>
      )}

      {query.data && silences.length === 0 && (
        <CardX size="medium" className={CARD_SURFACE}>
          <Empty description="No Silences match these filters." />
        </CardX>
      )}

      {query.data && silences.length > 0 && (
        <CardX
          size="medium"
          className={CARD_SURFACE}
          contentClassName="p-0"
          segmented
          title={'Silences · ' + silences.length}
        >
          <div className="overflow-x-auto">
            <table data-stack data-slot="silence-table" className="w-full text-sm">
              <caption className="sr-only">Silences</caption>
              <thead>
                <tr className="border-b">
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Silence
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Matcher
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Reason
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Window
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Created
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Cancelled
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Action
                  </th>
                </tr>
              </thead>
              <tbody>
                {silences.map((silence) => (
                  <tr key={silence.silenceId} className="border-b border-border/60 align-top">
                    <th scope="row" data-label="Silence" className="min-w-0 px-3 py-3 text-left">
                      <span className="break-all font-medium">{shortId(silence.silenceId)}</span>
                      <span className="mt-1 block">
                        <StatusBadge status={capitalize(silence.status)} tone={statusTone(silence.status)} />
                      </span>
                    </th>
                    <td data-label="Matcher" className="min-w-0 px-3 py-3">
                      <span className="font-medium">{capitalize(silence.matcherKind)}</span>
                      <span className="mt-1 block break-words text-muted-foreground">
                        {silence.matcherValue ?? 'All alerts'}
                      </span>
                    </td>
                    <td data-label="Reason" className="min-w-0 px-3 py-3 break-words">
                      {silence.reason}
                    </td>
                    <td data-label="Window" className="min-w-0 px-3 py-3">
                      <span className="block">{formatObservedAt(silence.startsAt)}</span>
                      <span className="block text-muted-foreground">
                        to {formatObservedAt(silence.endsAt)}
                      </span>
                    </td>
                    <td data-label="Created" className="min-w-0 px-3 py-3">
                      <span className="block">{formatObservedAt(silence.createdAt)}</span>
                      <span className="mt-1 block break-all text-muted-foreground">
                        by {silence.createdBy}
                      </span>
                    </td>
                    <td data-label="Cancelled" className="min-w-0 px-3 py-3">
                      <span className="block">{formatOptional(silence.cancelledAt)}</span>
                      <span className="mt-1 block break-all text-muted-foreground">
                        {silence.cancelledBy ? 'by ' + silence.cancelledBy : ''}
                      </span>
                    </td>
                    <td data-label="Action" className="min-w-0 px-3 py-3">
                      {silence.status === 'active' ? (
                        confirming === silence.silenceId ? (
                          <span
                            className="flex flex-wrap items-center gap-2"
                            role="group"
                            aria-label={'Confirm cancellation of Silence ' + shortId(silence.silenceId)}
                          >
                            <Button
                              variant="destructive"
                              size="sm"
                              className="min-h-11"
                              disabled={busy}
                              onClick={() => void confirmCancel(silence.silenceId)}
                            >
                              Confirm cancellation
                            </Button>
                            <Button
                              variant="outline"
                              size="sm"
                              className="min-h-11"
                              disabled={busy}
                              onClick={() => {
                                // Restore focus to the row control that opened the
                                // confirmation once the group unmounts.
                                restoreFocusTo.current = silence.silenceId
                                setConfirming(null)
                              }}
                            >
                              Keep Silence
                            </Button>
                          </span>
                        ) : (
                          <Button
                            variant="outline"
                            size="sm"
                            className="min-h-11"
                            disabled={busy}
                            ref={(element) => {
                              if (restoreFocusTo.current === silence.silenceId) {
                                restoreFocusTo.current = null
                                element?.focus()
                              }
                            }}
                            aria-label={'Cancel Silence ' + shortId(silence.silenceId)}
                            onClick={() => setConfirming(silence.silenceId)}
                          >
                            Cancel
                          </Button>
                        )
                      ) : (
                        <span className="text-muted-foreground">—</span>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </CardX>
      )}
    </section>
  )
}
