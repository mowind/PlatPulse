import { useState } from 'react'
import { Link, useParams } from 'react-router'
import {
  AdminApiError,
  cancelOperationEntry,
  operationKindLabel,
  operationStatusLabel,
  operationTone,
  useAdminOperation,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { StatusBadge, formatObservedAt } from '../components/StatusBadge'

/**
 * PAGE-ADMIN-OPERATION (webui.md §4.4/§5.5): progress, warnings, errors,
 * result summary, request ID, and the Audit link of one Operation.
 * Cancellation is confirmed and audited; state always refetches from
 * authoritative REST (no optimistic update).
 */

export default function AdminOperationDetail() {
  const { operationId = '' } = useParams()
  const { generation, status } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const query = useAdminOperation(generation, operationId)
  const [cancelling, setCancelling] = useState(false)
  const [confirmingCancel, setConfirmingCancel] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  const operation = query.data?.operation

  async function onCancel() {
    if (!operation || cancelling) return
    setCancelling(true)
    setError(null)
    setNotice(null)
    try {
      const response = await cancelOperationEntry(operation.operationId, csrfToken)
      setNotice(
        `Cancellation requested — Operation is now ${operationStatusLabel(
          response.operation.operation.status,
        )}.`,
      )
    } catch (caught) {
      setError(
        caught instanceof AdminApiError
          ? caught.message
          : 'Unable to cancel the Operation. Reload the page and try again.',
      )
    } finally {
      setCancelling(false)
    }
  }

  return (
    <section className="page">
      <p>
        <Link to="/admin/operations">← Operations</Link>
      </p>
      <h1>{operation ? operationKindLabel(operation.kind) : 'Operation'}</h1>
      {query.isError && (
        <p className="form-error" role="alert">
          Unable to load the Operation.{' '}
          {query.error instanceof Error ? query.error.message : ''}
        </p>
      )}
      {query.isSuccess && operation && (
        <>
          <div className="page-actions">
            <StatusBadge
              status={operationStatusLabel(operation.status)}
              tone={operationTone(operation.status)}
            />
            {query.data.cancellable &&
              (confirmingCancel ? (
                <>
                  <span className="muted">Cancel this Operation? The worker stops at the
                    next bounded step.</span>
                  <button
                    type="button"
                    className="danger-action"
                    onClick={onCancel}
                    disabled={cancelling}
                    aria-busy={cancelling}
                  >
                    {cancelling ? 'Cancelling…' : 'Yes, cancel'}
                  </button>
                  <button
                    type="button"
                    onClick={() => setConfirmingCancel(false)}
                    disabled={cancelling}
                  >
                    Keep running
                  </button>
                </>
              ) : (
                <button
                  type="button"
                  className="danger-action"
                  onClick={() => setConfirmingCancel(true)}
                >
                  Cancel Operation
                </button>
              ))}
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
          <h2>Progress</h2>
          {/* webui.md §5.5: progress is shown only when the Server can
           * compute it reliably — retention reports a per-batch label;
           * one-step Operations only report a terminal 100%. */}
          {operation.progressLabel || operation.progressPercent >= 100 ? (
            <>
              <p>
                <span aria-hidden="true">{operation.progressPercent}%</span>
                <span className="visually-hidden">{operation.progressPercent} percent</span>
                {operation.progressLabel && (
                  <span className="muted"> — {operation.progressLabel}</span>
                )}
              </p>
              <div
                className="progress-track"
                role="progressbar"
                aria-valuenow={operation.progressPercent}
                aria-valuemin={0}
                aria-valuemax={100}
                aria-label={`${operationKindLabel(operation.kind)} progress`}
              >
                <div
                  className="progress-fill"
                  style={{ width: `${operation.progressPercent}%` }}
                />
              </div>
            </>
          ) : (
            <p className="muted">
              {operation.status === 'queued'
                ? 'Queued — waiting for the worker.'
                : 'Running — progress is not reported for this step.'}
            </p>
          )}

          <h2>Details</h2>
          <dl className="detail-list">
            <div>
              <dt>Operation ID</dt>
              <dd>{operation.operationId}</dd>
            </div>
            <div>
              <dt>Request ID</dt>
              <dd>{operation.requestId ?? '—'}</dd>
            </div>
            <div>
              <dt>Created</dt>
              <dd>{formatObservedAt(operation.createdAt)}</dd>
            </div>
            <div>
              <dt>Started</dt>
              <dd>{operation.startedAt ? formatObservedAt(operation.startedAt) : '—'}</dd>
            </div>
            <div>
              <dt>Finished</dt>
              <dd>{operation.finishedAt ? formatObservedAt(operation.finishedAt) : '—'}</dd>
            </div>
            <div>
              <dt>Audit event</dt>
              <dd>
                {operation.auditEventId ? (
                  <Link to="/admin/access/audit">
                    #{operation.auditEventId} — Audit history
                  </Link>
                ) : (
                  '—'
                )}
              </dd>
            </div>
          </dl>

          {query.data.warnings.length > 0 && (
            <>
              <h2>Warnings</h2>
              <ul className="issue-list">
                {query.data.warnings.map((warning, index) => (
                  <li key={`${warning.code}-${index}`}>
                    <strong>{warning.code}</strong>: {warning.message}
                  </li>
                ))}
              </ul>
            </>
          )}
          {query.data.errors.length > 0 && (
            <>
              <h2>Errors</h2>
              <ul className="issue-list issue-list-error">
                {query.data.errors.map((item, index) => (
                  <li key={`${item.code}-${index}`}>
                    <strong>{item.code}</strong>: {item.message}
                  </li>
                ))}
              </ul>
            </>
          )}
          {query.data.result !== undefined && query.data.result !== null && (
            <>
              <h2>Result</h2>
              <pre className="result-summary">
                {JSON.stringify(query.data.result, null, 2)}
              </pre>
            </>
          )}
        </>
      )}
    </section>
  )
}
