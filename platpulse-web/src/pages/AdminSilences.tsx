import { useState, type FormEvent } from 'react'
import {
  AdminApiError,
  cancelSilenceEntry,
  createSilenceEntry,
  useAdminSilences,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { StatusBadge, formatObservedAt } from '../components/StatusBadge'
import type { SilenceDto } from '../api/generated'

/**
 * PAGE-ADMIN-SILENCES (design §4.4, webui.md §8.3): time-bounded delivery
 * Silence policies. A Silence suppresses matching notification delivery
 * only — it never stops Rule evaluation and never deletes or resolves
 * Incidents. Every mutation is confirmed, audited, and refetched; there is
 * no optimistic state.
 */

export function silenceStatusLabel(status: string | undefined): string {
  switch (status) {
    case 'active':
      return 'Active'
    case 'expired':
      return 'Expired'
    case 'cancelled':
      return 'Cancelled'
    default:
      return 'Unknown'
  }
}

function silenceTone(status: string | undefined): 'ok' | 'warning' | 'error' | 'neutral' {
  switch (status) {
    case 'active':
      return 'ok'
    case 'expired':
      return 'neutral'
    case 'cancelled':
      return 'warning'
    default:
      return 'neutral'
  }
}

export default function AdminSilencesList() {
  const { generation, status } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const [statusFilter, setStatusFilter] = useState('active')
  const query = useAdminSilences(generation, { status: statusFilter || undefined })
  const [creating, setCreating] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)

  return (
    <section className="page">
      <h1>Silences</h1>
      <p className="muted">
        A Silence suppresses matching notification delivery for its window. Evaluation and
        Incidents are never affected — an Open Incident stays open and resolves only on
        sustained fresh recovery.
      </p>
      <div className="page-actions">
        <button
          type="button"
          className="primary-action"
          onClick={() => setCreating((value) => !value)}
          aria-expanded={creating}
        >
          {creating ? 'Close form' : 'Create a Silence'}
        </button>
      </div>
      {notice && (
        <p className="form-success" role="status">
          {notice}
        </p>
      )}
      {creating && (
        <SilenceCreateForm
          csrfToken={csrfToken}
          onCreated={(reason) => {
            setNotice(`Silence created: ${reason}`)
            setCreating(false)
          }}
        />
      )}
      <div className="filter-bar" role="group" aria-label="Silence filters">
        <label htmlFor="silence-status-filter">Status</label>
        <select
          id="silence-status-filter"
          value={statusFilter}
          onChange={(event) => setStatusFilter(event.target.value)}
        >
          <option value="active">Active</option>
          <option value="expired">Expired</option>
          <option value="cancelled">Cancelled</option>
          <option value="">All</option>
        </select>
      </div>
      {!query.data && query.isPending && (
        <p className="panel-state" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading Silences…
        </p>
      )}
      {!query.data && query.isError && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Error" tone="error" />{' '}
          {query.error instanceof Error ? query.error.message : 'Unable to load Silences'}
          <button type="button" className="text-action" onClick={() => void query.refetch()}>
            Try again
          </button>
        </p>
      )}
      {query.data && query.data.length === 0 && (
        <p className="panel-state">
          <StatusBadge status="Empty" tone="ok" /> No {statusFilter || ''} Silences.
        </p>
      )}
      {query.data && query.data.length > 0 && (
        <div className="table-wrap">
          <table className="node-table">
            <caption className="sr-only">Silence policies and their windows</caption>
            <thead>
              <tr>
                <th scope="col">Matcher</th>
                <th scope="col">Reason</th>
                <th scope="col">Window</th>
                <th scope="col">Status</th>
                <th scope="col">Actions</th>
              </tr>
            </thead>
            <tbody>
              {query.data.map((silence) => (
                <SilenceRow
                  key={silence.silenceId}
                  silence={silence}
                  csrfToken={csrfToken}
                  onCancelled={() => {
                    setNotice(`Silence cancelled: ${silence.reason}`)
                    void query.refetch()
                  }}
                />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  )
}

function SilenceRow({
  silence,
  csrfToken,
  onCancelled,
}: {
  silence: SilenceDto
  csrfToken: string
  onCancelled: () => void
}) {
  const [confirming, setConfirming] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const onCancel = async () => {
    setError(null)
    try {
      await cancelSilenceEntry(silence.silenceId, csrfToken)
      onCancelled()
    } catch (cancelError) {
      setError(
        cancelError instanceof AdminApiError
          ? cancelError.message
          : 'Unable to cancel the Silence.',
      )
    }
  }

  const matcher = silence.matcherValue
    ? `${silence.matcherKind} · ${silence.matcherValue}`
    : 'all subjects'

  return (
    <tr>
      <td data-label="Matcher">{matcher}</td>
      <td data-label="Reason">{silence.reason}</td>
      <td data-label="Window">
        {formatObservedAt(silence.startsAt)} → {formatObservedAt(silence.endsAt)}
      </td>
      <td data-label="Status">
        <StatusBadge status={silenceStatusLabel(silence.status)} tone={silenceTone(silence.status)} />
      </td>
      <td data-label="Actions">
        {silence.status === 'active' && !confirming && (
          <button type="button" className="text-action" onClick={() => setConfirming(true)}>
            Cancel Silence
          </button>
        )}
        {silence.status === 'active' && confirming && (
          <span className="confirm-inline">
            <button type="button" className="text-action" onClick={() => void onCancel()}>
              Confirm cancellation
            </button>
            <button type="button" className="text-action" onClick={() => setConfirming(false)}>
              Keep
            </button>
          </span>
        )}
        {error && (
          <p className="form-error" role="alert">
            {error}
          </p>
        )}
      </td>
    </tr>
  )
}

function SilenceCreateForm({
  csrfToken,
  onCreated,
}: {
  csrfToken: string
  onCreated: (reason: string) => void
}) {
  const [matcherKind, setMatcherKind] = useState('all')
  const [matcherValue, setMatcherValue] = useState('')
  const [reason, setReason] = useState('')
  const [startsAt, setStartsAt] = useState('')
  const [endsAt, setEndsAt] = useState('')
  const [fieldErrors, setFieldErrors] = useState<Record<string, string>>({})
  const [saving, setSaving] = useState(false)

  const setField = (key: string, value: string, setter: (value: string) => void) => {
    setter(value)
    setFieldErrors((current) => {
      if (!(key in current)) return current
      const next = { ...current }
      delete next[key]
      return next
    })
  }

  const onSubmit = async (event: FormEvent) => {
    event.preventDefault()
    const errors: Record<string, string> = {}
    if (reason.trim().length === 0) {
      errors.reason = 'A reason is required.'
    }
    if (!startsAt || !endsAt) {
      errors.startsAt = 'Start and end times are required (RFC 3339 UTC, e.g. 2026-03-01T00:00:00Z).'
    }
    if (matcherKind !== 'all' && matcherValue.trim().length === 0) {
      errors.matcherValue = 'A matcher value is required for this scope.'
    }
    setFieldErrors(errors)
    if (Object.keys(errors).length > 0) return
    setSaving(true)
    try {
      await createSilenceEntry(
        {
          matcherKind,
          matcherValue: matcherKind === 'all' ? null : matcherValue.trim(),
          reason: reason.trim(),
          startsAt,
          endsAt,
        },
        csrfToken,
      )
      onCreated(reason.trim())
    } catch (createError) {
      setFieldErrors({
        _summary:
          createError instanceof AdminApiError
            ? createError.message
            : 'Unable to create the Silence.',
      })
      setSaving(false)
    }
  }

  const errorList = Object.entries(fieldErrors).filter(([key]) => key !== '_summary')

  return (
    <form className="page-form" onSubmit={onSubmit} noValidate>
      <h2>New Silence</h2>
      {(fieldErrors['_summary'] || errorList.length > 0) && (
        <ul className="form-error-list" role="alert" aria-label="Validation errors">
          {fieldErrors['_summary'] && <li>{fieldErrors['_summary']}</li>}
          {errorList.map(([key, message]) => (
            <li key={key}>{message}</li>
          ))}
        </ul>
      )}
      <div className="field">
        <label htmlFor="silence-matcher-kind">Applies to</label>
        <select
          id="silence-matcher-kind"
          value={matcherKind}
          onChange={(event) => setMatcherKind(event.target.value)}
        >
          <option value="all">All subjects</option>
          <option value="agent">One Agent (and its Nodes)</option>
          <option value="node">One Node</option>
          <option value="network">One Network</option>
        </select>
      </div>
      {matcherKind !== 'all' && (
        <div className="field">
          <label htmlFor="silence-matcher-value">Matcher value</label>
          <input
            id="silence-matcher-value"
            value={matcherValue}
            aria-invalid={fieldErrors.matcherValue ? true : undefined}
            aria-describedby={fieldErrors.matcherValue ? 'error-matcher-value' : undefined}
            onChange={(event) => setField('matcherValue', event.target.value, setMatcherValue)}
            placeholder={
              matcherKind === 'node'
                ? 'Node ID'
                : matcherKind === 'network'
                  ? 'Network key'
                  : 'Agent ID'
            }
          />
          {fieldErrors.matcherValue && (
            <p className="field-error" id="error-matcher-value" role="alert">
              {fieldErrors.matcherValue}
            </p>
          )}
        </div>
      )}
      <div className="field">
        <label htmlFor="silence-reason">Reason</label>
        <input
          id="silence-reason"
          value={reason}
          aria-invalid={fieldErrors.reason ? true : undefined}
          aria-describedby={fieldErrors.reason ? 'error-reason' : undefined}
          onChange={(event) => setField('reason', event.target.value, setReason)}
          placeholder="Why is delivery suppressed?"
        />
        {fieldErrors.reason && (
          <p className="field-error" id="error-reason" role="alert">
            {fieldErrors.reason}
          </p>
        )}
      </div>
      <div className="field">
        <label htmlFor="silence-starts-at">Starts at</label>
        <input
          id="silence-starts-at"
          value={startsAt}
          aria-invalid={fieldErrors.startsAt ? true : undefined}
          aria-describedby={fieldErrors.startsAt ? 'error-starts-at' : undefined}
          onChange={(event) => setField('startsAt', event.target.value, setStartsAt)}
          placeholder="2026-03-01T00:00:00Z"
        />
        {fieldErrors.startsAt && (
          <p className="field-error" id="error-starts-at" role="alert">
            {fieldErrors.startsAt}
          </p>
        )}
      </div>
      <div className="field">
        <label htmlFor="silence-ends-at">Ends at (required)</label>
        <input
          id="silence-ends-at"
          value={endsAt}
          aria-invalid={fieldErrors.endsAt ? true : undefined}
          aria-describedby={fieldErrors.endsAt ? 'error-ends-at' : undefined}
          onChange={(event) => setField('endsAt', event.target.value, setEndsAt)}
          placeholder="2026-03-01T02:00:00Z"
        />
        {fieldErrors.endsAt && (
          <p className="field-error" id="error-ends-at" role="alert">
            {fieldErrors.endsAt}
          </p>
        )}
      </div>
      <button type="submit" className="primary-action" disabled={saving}>
        {saving ? 'Creating…' : 'Create Silence'}
      </button>
    </form>
  )
}
