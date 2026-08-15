import { useState } from 'react'
import { Link } from 'react-router'
import {
  operationKindLabel,
  operationStatusLabel,
  operationTone,
  useAdminOperations,
  type OperationFilters,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { StatusBadge, formatObservedAt } from '../components/StatusBadge'

/**
 * PAGE-ADMIN-OPERATIONS (webui.md §4.4/§5.5): durable long-running
 * Operation history. REST is authoritative and survives navigation,
 * browser close, or SSE loss; SSE invalidations only accelerate refetch.
 */

export default function AdminOperationsList() {
  const { generation } = useAuth()
  const [statusFilter, setStatusFilter] = useState('')
  const [kindFilter, setKindFilter] = useState('')
  const filters: OperationFilters = {
    status: statusFilter || undefined,
    kind: kindFilter || undefined,
  }
  const query = useAdminOperations(generation, filters)

  return (
    <section className="page">
      <h1>Operations</h1>
      <p className="muted">
        Retention, backup, and Doctor work runs as durable Operations. Progress, warnings,
        errors, request IDs, and Audit links remain available here after navigation, a
        browser close, or realtime loss — the browser never cancels an Operation.
      </p>
      <div className="page-actions">
        <label className="field">
          <span>Status</span>
          <select
            aria-label="Filter by status"
            value={statusFilter}
            onChange={(event) => setStatusFilter(event.target.value)}
          >
            <option value="">All statuses</option>
            <option value="queued">Queued</option>
            <option value="running">Running</option>
            <option value="succeeded">Succeeded</option>
            <option value="succeeded_with_warnings">Succeeded with warnings</option>
            <option value="failed">Failed</option>
            <option value="cancelled">Cancelled</option>
          </select>
        </label>
        <label className="field">
          <span>Kind</span>
          <select
            aria-label="Filter by kind"
            value={kindFilter}
            onChange={(event) => setKindFilter(event.target.value)}
          >
            <option value="">All kinds</option>
            <option value="retention_run">Retention run</option>
            <option value="backup_create">Backup creation</option>
            <option value="backup_verify">Backup verification</option>
            <option value="doctor_run">Doctor</option>
          </select>
        </label>
      </div>
      {query.isError && (
        <p className="form-error" role="alert">
          Unable to load Operations. {query.error instanceof Error ? query.error.message : ''}
        </p>
      )}
      {query.isSuccess && query.data.length === 0 && (
        <p className="muted">No Operations match the current filters.</p>
      )}
      <div className="table-wrap">
        <table className="node-table">
          <caption className="visually-hidden">Operation history</caption>
          <thead>
            <tr>
              <th scope="col">Operation</th>
              <th scope="col">Status</th>
              <th scope="col">Progress</th>
              <th scope="col">Request ID</th>
              <th scope="col">Created</th>
            </tr>
          </thead>
          <tbody>
            {query.data?.map((operation) => (
              <tr key={operation.operationId}>
                <td data-label="Operation">
                  <Link to={`/admin/operations/${operation.operationId}`}>
                    {operationKindLabel(operation.kind)}
                  </Link>
                  <small>{operation.operationId.slice(0, 8)}</small>
                </td>
                <td data-label="Status">
                  <StatusBadge
                    status={operationStatusLabel(operation.status)}
                    tone={operationTone(operation.status)}
                  />
                </td>
                <td data-label="Progress">
                  <span aria-hidden="true">{operation.progressPercent}%</span>
                  <span className="visually-hidden">{operation.progressPercent} percent</span>
                  {operation.progressLabel && (
                    <small>{operation.progressLabel}</small>
                  )}
                </td>
                <td data-label="Request ID">
                  {operation.requestId ? (
                    <small>{operation.requestId}</small>
                  ) : (
                    <small>—</small>
                  )}
                </td>
                <td data-label="Created">
                  <small>{formatObservedAt(operation.createdAt)}</small>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  )
}
