import { useRef, useState, type FormEvent } from 'react'
import { useSearchParams } from 'react-router'
import {
  AdminApiError,
  cancelMaintenanceEntry,
  createMaintenanceEntry,
  useAdminMaintenance,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { Alert, AlertDescription, AlertTitle } from '../components/ui/alert'
import { Button } from '../components/ui/button'
import { CardX } from '../components/ui/card-x'
import { Empty } from '../components/ui/empty'
import { Input, Select, Textarea } from '../components/ui/input'
import { StatusBadge, formatObservedAt } from '../components/StatusBadge'
import { cn } from '../lib/utils'
import { SURFACE_CARD_STATIC, SURFACE_TOOLBAR } from '../lib/surface'
import type { MaintenanceDto } from '../api/generated'

/**
 * PAGE-ADMIN-MAINTENANCE (issue #205; webui.md §15.8). Owners list every
 * Maintenance Window with its Server-owned status (active, expired, or
 * cancelled), the Agent/Node/Network scope, the typed expected-rule allowlist,
 * the reason, the recorded window, and the authorship, then create a
 * time-bounded Window or cancel an active one.
 *
 * A Maintenance Window preserves Alert facts and auditability: it never hides
 * an Alert or an Incident from the record. Ending or expiry never replays a
 * suppressed message and never invents a reminder, and a message already handed
 * to the delivery channel is non-retractable. The Server is authoritative; this
 * surface never optimistically marks a Window created or cancelled, and a
 * failure shows the sanitized Server error.
 */

const CARD_SURFACE = cn('rounded-md border-none', SURFACE_CARD_STATIC)

const SCOPE_KINDS = ['agent', 'node', 'network'] as const

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

/** A cancelled or never-cancelled Window has no timestamp; show an explicit
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

/** The multi-entry allowlist accepts one key per line or comma-separated keys;
 * an empty allowlist means every Rule is expected, never "no Rule". */
function parseRuleKeys(value: string): string[] {
  const keys = value
    .split(/[\n,]+/)
    .map((key) => key.trim())
    .filter((key) => key !== '')
  return Array.from(new Set(keys))
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

export default function AdminMaintenanceList() {
  const { generation, status } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const [search, setSearch] = useSearchParams()
  const statusFilter = readStatus(search)
  const query = useAdminMaintenance(generation, statusFilter === 'all' ? {} : { status: statusFilter })
  const windows: MaintenanceDto[] = query.data ?? []

  const [scopeKind, setScopeKind] = useState('agent')
  const [scopeValue, setScopeValue] = useState('')
  const [expectedRuleKeys, setExpectedRuleKeys] = useState('')
  const [reason, setReason] = useState('')
  const [startsAt, setStartsAt] = useState('')
  const [endsAt, setEndsAt] = useState('')
  const [confirming, setConfirming] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [feedback, setFeedback] = useState<FormFeedback | null>(null)
  const headingRef = useRef<HTMLHeadingElement>(null)
  const restoreFocusTo = useRef<string | null>(null)
  const [fieldErrors, setFieldErrors] = useState<{
    scopeValue?: string
    reason?: string
    startsAt?: string
    endsAt?: string
  }>({})
  const clearFieldError = (key: 'scopeValue' | 'reason' | 'startsAt' | 'endsAt') => {
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
    const trimmedScope = scopeValue.trim()
    const trimmedReason = reason.trim()
    const start = toRfc3339(startsAt)
    const end = toRfc3339(endsAt)
    const nextErrors: {
      scopeValue?: string
      reason?: string
      startsAt?: string
      endsAt?: string
    } = {}
    if (trimmedScope === '') {
      nextErrors.scopeValue = 'Enter a scope value for this Maintenance Window.'
    }
    if (trimmedReason === '') {
      nextErrors.reason = 'Enter a reason for this Maintenance Window.'
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
    if (nextErrors.scopeValue || nextErrors.reason || nextErrors.startsAt || nextErrors.endsAt) {
      // §10.3 (webui.md): the page summary and the field-level messages are
      // both required; the summary keeps the stable wording the Server errors use.
      const message = nextErrors.scopeValue
        ? 'A scope value is required for every Maintenance Window.'
        : nextErrors.reason
          ? 'A reason is required for every Maintenance Window.'
          : start === '' || end === ''
            ? 'A valid start and end time are required.'
            : 'The end time must be after the start time.'
      setFeedback({ tone: 'error', message })
      return
    }
    setBusy(true)
    try {
      await createMaintenanceEntry(
        {
          scopeKind,
          scopeValue: trimmedScope,
          expectedRuleKeys: parseRuleKeys(expectedRuleKeys),
          reason: trimmedReason,
          startsAt: start,
          endsAt: end,
        },
        csrfToken,
      )
      setFeedback({
        tone: 'ok',
        message: 'Maintenance Window created. The Server is authoritative for it.',
      })
      setScopeKind('agent')
      setScopeValue('')
      setExpectedRuleKeys('')
      setReason('')
      setStartsAt('')
      setEndsAt('')
    } catch (error) {
      const message = errorMessage(error, 'Unable to create the Maintenance Window.')
      // #202: a transport error does not prove the mutation did not commit.
      setFeedback({
        tone: 'error',
        message: indeterminateOutcome(error) ? message + ' ' + INDETERMINATE_OUTCOME : message,
      })
    } finally {
      setBusy(false)
    }
  }

  const confirmCancel = async (windowId: string) => {
    setBusy(true)
    setFeedback(null)
    try {
      await cancelMaintenanceEntry(windowId, csrfToken)
      setConfirming(null)
      // §15.5 (webui.md): an active-only list drops the confirmed row, so move
      // focus to the panel heading instead of losing it with the unmounted control.
      headingRef.current?.focus()
      setFeedback({
        tone: 'ok',
        message: 'Maintenance Window cancelled. Suppressed messages are not replayed.',
      })
    } catch (error) {
      const message = errorMessage(error, 'Unable to cancel the Maintenance Window.')
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
        <h1 ref={headingRef} tabIndex={-1} className="text-lg font-semibold break-words">Maintenance</h1>
        <p className="text-sm text-muted-foreground">
          Each row is one time-bounded Maintenance Window with the Server-owned status and the
          recorded authorship. Creating or cancelling one is an audited Owner action.
        </p>
      </div>

      <Alert>
        <AlertTitle>What a Maintenance Window does and does not do</AlertTitle>
        <AlertDescription>
          A Maintenance Window preserves Alert facts and auditability. Ending or expiry never
          replays a suppressed message and never invents a reminder, and a message already handed
          to the delivery channel is non-retractable. An empty expected-rule list means every Rule.
          Failures show the sanitized Server error; a transport failure is reported as an
          unknown outcome with a reconciliation instruction, never as a failed mutation. This
          page never optimistically mutates Server truth.
        </AlertDescription>
      </Alert>

      <form
        className={cn('flex flex-wrap items-end gap-3 rounded-md border-none p-3', SURFACE_TOOLBAR)}
        role="group"
        aria-label="Maintenance filters"
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

      <CardX size="medium" className={CARD_SURFACE} title="Create Maintenance Window">
        <form className="space-y-3" onSubmit={submit} aria-label="Create Maintenance Window">
          <div className="grid gap-3 sm:grid-cols-2">
            <label className="space-y-1 text-sm">
              <span className="font-medium">Scope kind</span>
              <Select
                aria-label="Scope kind"
                value={scopeKind}
                onChange={(event) => {
                  setScopeKind(event.target.value)
                  clearFieldError('scopeValue')
                }}
              >
                {SCOPE_KINDS.map((kind) => (
                  <option key={kind} value={kind}>
                    {capitalize(kind)}
                  </option>
                ))}
              </Select>
            </label>
            <label className="space-y-1 text-sm">
              <span className="font-medium">Scope value</span>
              <Input
                id="maintenance-scope-value"
                aria-label="Scope value"
                aria-invalid={fieldErrors.scopeValue ? true : undefined}
                aria-describedby={
                  fieldErrors.scopeValue ? 'maintenance-scope-value-error' : undefined
                }
                value={scopeValue}
                placeholder="agent-1, node-a, or mainnet"
                onChange={(event) => {
                  setScopeValue(event.target.value)
                  clearFieldError('scopeValue')
                }}
              />
              {fieldErrors.scopeValue && (
                <span
                  id="maintenance-scope-value-error"
                  role="alert"
                  className="block text-xs text-destructive"
                >
                  {fieldErrors.scopeValue}
                </span>
              )}
            </label>
            <label className="space-y-1 text-sm">
              <span className="font-medium">Starts at</span>
              <Input
                id="maintenance-starts-at"
                aria-label="Starts at"
                aria-invalid={fieldErrors.startsAt ? true : undefined}
                aria-describedby={fieldErrors.startsAt ? 'maintenance-starts-at-error' : undefined}
                type="datetime-local"
                value={startsAt}
                onChange={(event) => {
                  setStartsAt(event.target.value)
                  clearFieldError('startsAt')
                }}
              />
              {fieldErrors.startsAt && (
                <span
                  id="maintenance-starts-at-error"
                  role="alert"
                  className="block text-xs text-destructive"
                >
                  {fieldErrors.startsAt}
                </span>
              )}
            </label>
            <label className="space-y-1 text-sm">
              <span className="font-medium">Ends at</span>
              <Input
                id="maintenance-ends-at"
                aria-label="Ends at"
                aria-invalid={fieldErrors.endsAt ? true : undefined}
                aria-describedby={fieldErrors.endsAt ? 'maintenance-ends-at-error' : undefined}
                type="datetime-local"
                value={endsAt}
                onChange={(event) => {
                  setEndsAt(event.target.value)
                  clearFieldError('endsAt')
                }}
              />
              {fieldErrors.endsAt && (
                <span
                  id="maintenance-ends-at-error"
                  role="alert"
                  className="block text-xs text-destructive"
                >
                  {fieldErrors.endsAt}
                </span>
              )}
            </label>
          </div>
          <label className="block space-y-1 text-sm">
            <span className="font-medium">Reason</span>
            <Input
              id="maintenance-reason"
              aria-label="Reason"
              aria-invalid={fieldErrors.reason ? true : undefined}
              aria-describedby={fieldErrors.reason ? 'maintenance-reason-error' : undefined}
              maxLength={500}
              value={reason}
              placeholder="Why the Window is open"
              onChange={(event) => {
                setReason(event.target.value)
                clearFieldError('reason')
              }}
            />
            {fieldErrors.reason && (
              <span
                id="maintenance-reason-error"
                role="alert"
                className="block text-xs text-destructive"
              >
                {fieldErrors.reason}
              </span>
            )}
          </label>
          <label className="block space-y-1 text-sm">
            <span className="font-medium">Expected rule keys</span>
            <Textarea
              aria-label="Expected rule keys"
              value={expectedRuleKeys}
              rows={3}
              placeholder={'node.rpc_unreachable\nnode.peer_count_low'}
              onChange={(event) => setExpectedRuleKeys(event.target.value)}
            />
            <span className="block text-xs text-muted-foreground">
              One known Rule key per line or comma-separated. Leave empty to expect every Rule.
            </span>
          </label>
          <Button type="submit" size="sm" className="min-h-11" disabled={busy || csrfToken.length === 0}>
            Create Maintenance Window
          </Button>
          <FormFeedbackNote feedback={feedback} />
        </form>
      </CardX>

      {!query.data && query.isPending && (
        <p className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading the Maintenance Windows…
        </p>
      )}

      {!query.data && query.isError && (
        <div
          className="flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
          role="alert"
        >
          <StatusBadge status="Error" tone="error" />
          <span className="min-w-0 break-words">
            {errorMessage(query.error, 'Unable to load the Maintenance Windows')}
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
          Maintenance Window values.
        </div>
      )}

      {query.data && windows.length === 0 && (
        <CardX size="medium" className={CARD_SURFACE}>
          <Empty description="No Maintenance Windows match these filters." />
        </CardX>
      )}

      {query.data && windows.length > 0 && (
        <CardX
          size="medium"
          className={CARD_SURFACE}
          contentClassName="p-0"
          segmented
          title={'Maintenance Windows · ' + windows.length}
        >
          <div className="overflow-x-auto">
            <table data-stack data-slot="maintenance-table" className="w-full text-sm">
              <caption className="sr-only">Maintenance Windows</caption>
              <thead>
                <tr className="border-b">
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Window
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Scope
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Expected rules
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
                {windows.map((window) => (
                  <tr key={window.windowId} className="border-b border-border/60 align-top">
                    <th scope="row" data-label="Window" className="min-w-0 px-3 py-3 text-left">
                      <span className="break-all font-medium">{shortId(window.windowId)}</span>
                      <span className="mt-1 block">
                        <StatusBadge status={capitalize(window.status)} tone={statusTone(window.status)} />
                      </span>
                    </th>
                    <td data-label="Scope" className="min-w-0 px-3 py-3">
                      <span className="font-medium">{capitalize(window.scopeKind)}</span>
                      <span className="mt-1 block break-words text-muted-foreground">
                        {window.scopeValue}
                      </span>
                    </td>
                    <td data-label="Expected rules" className="min-w-0 px-3 py-3 break-words">
                      {window.expectedRuleKeys.length === 0
                        ? 'All Rules'
                        : window.expectedRuleKeys.join(', ')}
                    </td>
                    <td data-label="Reason" className="min-w-0 px-3 py-3 break-words">
                      {window.reason}
                    </td>
                    <td data-label="Window" className="min-w-0 px-3 py-3">
                      <span className="block">{formatObservedAt(window.startsAt)}</span>
                      <span className="block text-muted-foreground">
                        to {formatObservedAt(window.endsAt)}
                      </span>
                    </td>
                    <td data-label="Created" className="min-w-0 px-3 py-3">
                      <span className="block">{formatObservedAt(window.createdAt)}</span>
                      <span className="mt-1 block break-all text-muted-foreground">
                        by {window.createdBy}
                      </span>
                    </td>
                    <td data-label="Cancelled" className="min-w-0 px-3 py-3">
                      <span className="block">{formatOptional(window.cancelledAt)}</span>
                      <span className="mt-1 block break-all text-muted-foreground">
                        {window.cancelledBy ? 'by ' + window.cancelledBy : ''}
                      </span>
                    </td>
                    <td data-label="Action" className="min-w-0 px-3 py-3">
                      {window.status === 'active' ? (
                        confirming === window.windowId ? (
                          <span
                            className="flex flex-wrap items-center gap-2"
                            role="group"
                            aria-label={
                              'Confirm cancellation of Maintenance Window ' + shortId(window.windowId)
                            }
                          >
                            <Button
                              variant="destructive"
                              size="sm"
                              className="min-h-11"
                              disabled={busy}
                              onClick={() => void confirmCancel(window.windowId)}
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
                                restoreFocusTo.current = window.windowId
                                setConfirming(null)
                              }}
                            >
                              Keep Window
                            </Button>
                          </span>
                        ) : (
                          <Button
                            variant="outline"
                            size="sm"
                            className="min-h-11"
                            disabled={busy}
                            ref={(element) => {
                              if (restoreFocusTo.current === window.windowId) {
                                restoreFocusTo.current = null
                                element?.focus()
                              }
                            }}
                            aria-label={'Cancel Maintenance Window ' + shortId(window.windowId)}
                            onClick={() => setConfirming(window.windowId)}
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
