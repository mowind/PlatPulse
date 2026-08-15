import { useState, type FormEvent } from 'react'
import {
  AdminApiError,
  cancelMaintenanceEntry,
  createMaintenanceEntry,
  useAdminMaintenance,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { StatusBadge, formatObservedAt } from '../components/StatusBadge'
import type { MaintenanceDto } from '../api/generated'

/**
 * PAGE-ADMIN-MAINTENANCE (design §4.4, webui.md §8.3): time-bounded
 * Maintenance Windows for an Agent, Node, or Network scope. Maintenance
 * suppresses expected delivery and marks expected Incidents suppressed
 * without changing facts, evaluation, or Node Health. Expected conditions
 * are a typed allowlist of Rule keys (empty = any Rule). Every mutation is
 * confirmed, audited, and refetched.
 */

const ALERT_RULE_KEYS = [
  'agent.offline',
  'node.rpc_unreachable',
  'node.head_subscription_disconnected',
  'node.observation_stale',
  'node.process_not_running',
  'node.block_stalled',
  'node.sync_lag',
  'node.network_identity_mismatch',
  'node.consensus_stalled',
  'host.disk_pressure',
  'host.memory_pressure',
]

export function maintenanceStatusLabel(status: string | undefined): string {
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

function maintenanceTone(status: string | undefined): 'ok' | 'warning' | 'error' | 'neutral' {
  switch (status) {
    case 'active':
      return 'warning'
    case 'expired':
      return 'neutral'
    case 'cancelled':
      return 'neutral'
    default:
      return 'neutral'
  }
}

export default function AdminMaintenanceList() {
  const { generation, status } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const [statusFilter, setStatusFilter] = useState('active')
  const query = useAdminMaintenance(generation, { status: statusFilter || undefined })
  const [creating, setCreating] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)

  return (
    <section className="page">
      <h1>Maintenance Windows</h1>
      <p className="muted">
        A Maintenance Window marks expected Incidents suppressed and suppresses expected
        delivery for an Agent, Node, or Network scope. Facts, evaluation, and Node Health are
        never changed; when the window ends, current facts are re-evaluated normally.
      </p>
      <div className="page-actions">
        <button
          type="button"
          className="primary-action"
          onClick={() => setCreating((value) => !value)}
          aria-expanded={creating}
        >
          {creating ? 'Close form' : 'Schedule Maintenance'}
        </button>
      </div>
      {notice && (
        <p className="form-success" role="status">
          {notice}
        </p>
      )}
      {creating && (
        <MaintenanceCreateForm
          csrfToken={csrfToken}
          onCreated={(reason) => {
            setNotice(`Maintenance Window created: ${reason}`)
            setCreating(false)
          }}
        />
      )}
      <div className="filter-bar" role="group" aria-label="Maintenance filters">
        <label htmlFor="maintenance-status-filter">Status</label>
        <select
          id="maintenance-status-filter"
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
          <StatusBadge status="Starting" tone="neutral" /> Loading Maintenance Windows…
        </p>
      )}
      {!query.data && query.isError && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Error" tone="error" />{' '}
          {query.error instanceof Error ? query.error.message : 'Unable to load Windows'}
          <button type="button" className="text-action" onClick={() => void query.refetch()}>
            Try again
          </button>
        </p>
      )}
      {query.data && query.data.length === 0 && (
        <p className="panel-state">
          <StatusBadge status="Empty" tone="ok" /> No {statusFilter || ''} Maintenance Windows.
        </p>
      )}
      {query.data && query.data.length > 0 && (
        <div className="table-wrap">
          <table className="node-table">
            <caption className="sr-only">Maintenance Windows and their scopes</caption>
            <thead>
              <tr>
                <th scope="col">Scope</th>
                <th scope="col">Expected conditions</th>
                <th scope="col">Reason</th>
                <th scope="col">Window</th>
                <th scope="col">Status</th>
                <th scope="col">Actions</th>
              </tr>
            </thead>
            <tbody>
              {query.data.map((window) => (
                <MaintenanceRow
                  key={window.windowId}
                  window={window}
                  csrfToken={csrfToken}
                  onCancelled={() => {
                    setNotice(`Maintenance Window cancelled: ${window.reason}`)
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

function MaintenanceRow({
  window,
  csrfToken,
  onCancelled,
}: {
  window: MaintenanceDto
  csrfToken: string
  onCancelled: () => void
}) {
  const [confirming, setConfirming] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const onCancel = async () => {
    setError(null)
    try {
      await cancelMaintenanceEntry(window.windowId, csrfToken)
      onCancelled()
    } catch (cancelError) {
      setError(
        cancelError instanceof AdminApiError
          ? cancelError.message
          : 'Unable to cancel the Maintenance Window.',
      )
    }
  }

  const expected = window.expectedRuleKeys.length
    ? window.expectedRuleKeys.join(', ')
    : 'any Rule'

  return (
    <tr>
      <td data-label="Scope">
        {window.scopeKind} · {window.scopeValue}
      </td>
      <td data-label="Expected conditions">{expected}</td>
      <td data-label="Reason">{window.reason}</td>
      <td data-label="Window">
        {formatObservedAt(window.startsAt)} → {formatObservedAt(window.endsAt)}
      </td>
      <td data-label="Status">
        <StatusBadge
          status={maintenanceStatusLabel(window.status)}
          tone={maintenanceTone(window.status)}
        />
      </td>
      <td data-label="Actions">
        {window.status === 'active' && !confirming && (
          <button type="button" className="text-action" onClick={() => setConfirming(true)}>
            Cancel Window
          </button>
        )}
        {window.status === 'active' && confirming && (
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

function MaintenanceCreateForm({
  csrfToken,
  onCreated,
}: {
  csrfToken: string
  onCreated: (reason: string) => void
}) {
  const [scopeKind, setScopeKind] = useState('node')
  const [scopeValue, setScopeValue] = useState('')
  const [expectedRuleKeys, setExpectedRuleKeys] = useState<string[]>([])
  const [reason, setReason] = useState('')
  const [startsAt, setStartsAt] = useState('')
  const [endsAt, setEndsAt] = useState('')
  const [fieldErrors, setFieldErrors] = useState<Record<string, string>>({})
  const [saving, setSaving] = useState(false)

  const toggleRule = (ruleKey: string, checked: boolean) => {
    setExpectedRuleKeys((current) =>
      checked ? [...current, ruleKey] : current.filter((key) => key !== ruleKey),
    )
  }

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
    if (scopeValue.trim().length === 0) {
      errors.scopeValue = 'A scope value is required.'
    }
    if (reason.trim().length === 0) {
      errors.reason = 'A reason is required.'
    }
    if (!startsAt || !endsAt) {
      errors.startsAt = 'Start and end times are required (RFC 3339 UTC, e.g. 2026-03-01T00:00:00Z).'
    }
    setFieldErrors(errors)
    if (Object.keys(errors).length > 0) return
    setSaving(true)
    try {
      await createMaintenanceEntry(
        {
          scopeKind,
          scopeValue: scopeValue.trim(),
          expectedRuleKeys,
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
            : 'Unable to create the Maintenance Window.',
      })
      setSaving(false)
    }
  }

  const errorList = Object.entries(fieldErrors).filter(([key]) => key !== '_summary')

  return (
    <form className="page-form" onSubmit={onSubmit} noValidate>
      <h2>New Maintenance Window</h2>
      {(fieldErrors['_summary'] || errorList.length > 0) && (
        <ul className="form-error-list" role="alert" aria-label="Validation errors">
          {fieldErrors['_summary'] && <li>{fieldErrors['_summary']}</li>}
          {errorList.map(([key, message]) => (
            <li key={key}>{message}</li>
          ))}
        </ul>
      )}
      <div className="field">
        <label htmlFor="maintenance-scope-kind">Scope</label>
        <select
          id="maintenance-scope-kind"
          value={scopeKind}
          onChange={(event) => setScopeKind(event.target.value)}
        >
          <option value="agent">Agent</option>
          <option value="node">Node</option>
          <option value="network">Network</option>
        </select>
      </div>
      <div className="field">
        <label htmlFor="maintenance-scope-value">Scope value</label>
        <input
          id="maintenance-scope-value"
          value={scopeValue}
          aria-invalid={fieldErrors.scopeValue ? true : undefined}
          aria-describedby={fieldErrors.scopeValue ? 'error-scope-value' : undefined}
          onChange={(event) => setField('scopeValue', event.target.value, setScopeValue)}
          placeholder={
            scopeKind === 'node' ? 'Node ID' : scopeKind === 'network' ? 'Network key' : 'Agent ID'
          }
        />
        {fieldErrors.scopeValue && (
          <p className="field-error" id="error-scope-value" role="alert">
            {fieldErrors.scopeValue}
          </p>
        )}
      </div>
      <fieldset className="field">
        <legend>Expected conditions</legend>
        <p className="muted">Leave empty to match any Rule.</p>
        <div className="check-list">
          {ALERT_RULE_KEYS.map((ruleKey) => (
            <label key={ruleKey}>
              <input
                type="checkbox"
                checked={expectedRuleKeys.includes(ruleKey)}
                onChange={(event) => toggleRule(ruleKey, event.target.checked)}
              />
              {ruleKey}
            </label>
          ))}
        </div>
      </fieldset>
      <div className="field">
        <label htmlFor="maintenance-reason">Reason</label>
        <input
          id="maintenance-reason"
          value={reason}
          aria-invalid={fieldErrors.reason ? true : undefined}
          aria-describedby={fieldErrors.reason ? 'error-reason' : undefined}
          onChange={(event) => setField('reason', event.target.value, setReason)}
          placeholder="Why is this maintenance expected?"
        />
        {fieldErrors.reason && (
          <p className="field-error" id="error-reason" role="alert">
            {fieldErrors.reason}
          </p>
        )}
      </div>
      <div className="field">
        <label htmlFor="maintenance-starts-at">Starts at</label>
        <input
          id="maintenance-starts-at"
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
        <label htmlFor="maintenance-ends-at">Ends at (required)</label>
        <input
          id="maintenance-ends-at"
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
        {saving ? 'Creating…' : 'Schedule Window'}
      </button>
    </form>
  )
}
