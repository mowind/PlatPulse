import { useState } from 'react'
import { Link, useNavigate } from 'react-router'
import {
  AdminApiError,
  operationKindLabel,
  operationStatusLabel,
  operationTone,
  runRetentionEntry,
  useAdminRetention,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { StatusBadge, formatObservedAt } from '../components/StatusBadge'

/**
 * PAGE-ADMIN-RETENTION (webui.md §4.5/§8.4): per-family policies with
 * fixed safety bounds and execution state. Retention is batched and can
 * never delete protected history state, coverage/gap/divergence state,
 * cumulative counters, or Audit constraints.
 */

function supportLabel(policy: {
  supported: boolean
  enabled: boolean
  retentionDays: number
}): string {
  if (!policy.supported) return 'Unsupported'
  if (!policy.enabled) return 'Disabled'
  return policy.retentionDays === 0 ? 'Keep forever' : `${policy.retentionDays} days`
}

export default function AdminRetentionList() {
  const { generation, status } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const query = useAdminRetention(generation)
  const navigate = useNavigate()
  const [running, setRunning] = useState(false)
  const [confirming, setConfirming] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  async function onRun(confirmed: boolean) {
    if (running) return
    if (!confirmed) {
      setConfirming(true)
      return
    }
    setConfirming(false)
    setRunning(true)
    setError(null)
    setNotice(null)
    try {
      const response = await runRetentionEntry(null, csrfToken)
      setNotice(
        `Retention run queued — track it in the ${
          response.operation.operation.status === 'queued'
            ? `Operations list`
            : operationKindLabel(response.operation.operation.kind)
        }.`,
      )
      navigate(`/admin/operations/${response.operation.operation.operationId}`)
    } catch (caught) {
      setError(
        caught instanceof AdminApiError
          ? caught.message
          : 'Unable to start the retention run.',
      )
    } finally {
      setRunning(false)
    }
  }

  const lastRun = query.data?.lastRun

  return (
    <section className="page">
      <h1>Retention</h1>
      <p className="muted">
        Each family is configurable within fixed safety bounds. Execution is batched and
        protected: historical high-water marks, coverage intervals, open or permanent gap
        records, cumulative counters, immutable Incident history, and Audit Events
        referenced by Operations are never deleted.
      </p>
      <div className="page-actions">
        {confirming ? (
          <>
            <span className="muted">Run retention for every enabled family now?</span>
            <button
              type="button"
              className="primary-action"
              onClick={() => onRun(true)}
              disabled={running}
            >
              {running ? 'Queuing…' : 'Yes, run now'}
            </button>
            <button type="button" onClick={() => setConfirming(false)} disabled={running}>
              Cancel
            </button>
          </>
        ) : (
          <button
            type="button"
            className="primary-action"
            onClick={() => onRun(false)}
            disabled={running}
          >
            {running ? 'Queuing…' : 'Run retention now'}
          </button>
        )}
      </div>
      {notice && (
        <p className="form-success" role="status">
          {notice}
        </p>
      )}
      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}
      {query.isError && (
        <p className="form-error" role="alert">
          Unable to load retention policies.{' '}
          {query.error instanceof Error ? query.error.message : ''}
        </p>
      )}
      {lastRun && (
        <p className="muted">
          Last run:{' '}
          <StatusBadge
            status={operationStatusLabel(lastRun.status)}
            tone={operationTone(lastRun.status)}
          />{' '}
          <Link to={`/admin/operations/${lastRun.operationId}`}>
            {operationKindLabel(lastRun.kind)}
          </Link>{' '}
          <small>{formatObservedAt(lastRun.createdAt)}</small>
        </p>
      )}
      <div className="table-wrap">
        <table className="node-table">
          <caption className="visually-hidden">Retention policy families</caption>
          <thead>
            <tr>
              <th scope="col">Family</th>
              <th scope="col">Retention</th>
              <th scope="col">Safety bounds</th>
              <th scope="col">State</th>
              <th scope="col">Updated</th>
            </tr>
          </thead>
          <tbody>
            {query.data?.policies.map((policy) => (
              <tr key={policy.family}>
                <td data-label="Family">
                  <Link to={`/admin/data/retention/edit?family=${policy.family}`}>
                    {policy.label}
                  </Link>
                  <small>{policy.family}</small>
                </td>
                <td data-label="Retention">{supportLabel(policy)}</td>
                <td data-label="Safety bounds">
                  <small>
                    {policy.maxDays === 0
                      ? policy.minDays === 0
                        ? 'Long-term only'
                        : `≥ ${policy.minDays} days`
                      : `${policy.minDays}–${policy.maxDays} days`}
                  </small>
                </td>
                <td data-label="State">
                  <StatusBadge
                    status={policy.supported ? (policy.enabled ? 'Current' : 'Disabled') : 'Unsupported'}
                    tone={policy.supported ? (policy.enabled ? 'ok' : 'neutral') : 'warning'}
                  />
                </td>
                <td data-label="Updated">
                  <small>
                    {formatObservedAt(policy.updatedAt)}
                    {policy.updatedBy ? ` by ${policy.updatedBy}` : ''}
                  </small>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {query.data && query.data.protectedState.length > 0 && (
        <>
          <h2>Protected state</h2>
          <p className="muted">Retention never deletes:</p>
          <ul className="issue-list">
            {query.data.protectedState.map((item) => (
              <li key={item}>{item}</li>
            ))}
          </ul>
        </>
      )}
    </section>
  )
}
