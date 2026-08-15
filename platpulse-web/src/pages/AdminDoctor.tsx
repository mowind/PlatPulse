import { useState } from 'react'
import { Link, useNavigate } from 'react-router'
import {
  AdminApiError,
  doctorCheckStatusLabel,
  operationKindLabel,
  operationStatusLabel,
  operationTone,
  runDoctorEntry,
  useAdminDoctor,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { StatusBadge, formatObservedAt } from '../components/StatusBadge'

/**
 * PAGE-ADMIN-DOCTOR (webui.md §4.5/§8.4): read-only diagnostic checks and
 * reports. Doctor never auto-fixes, deletes, migrates, or rotates secrets.
 * Checks distinguish Pass, Warning, Fail, NotConfigured, and Skipped; the
 * previous diagnostic result survives a failed run.
 */

function checkTone(status: string | undefined): 'ok' | 'warning' | 'error' | 'neutral' {
  switch (status) {
    case 'pass':
      return 'ok'
    case 'warning':
    case 'not_configured':
    case 'skipped':
      return 'warning'
    case 'fail':
      return 'error'
    default:
      return 'neutral'
  }
}

export default function AdminDoctor() {
  const { generation, status } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const query = useAdminDoctor(generation)
  const navigate = useNavigate()
  const [running, setRunning] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function onRun() {
    if (running) return
    setRunning(true)
    setError(null)
    try {
      const response = await runDoctorEntry(csrfToken)
      navigate(`/admin/operations/${response.operation.operation.operationId}`)
    } catch (caught) {
      setError(
        caught instanceof AdminApiError ? caught.message : 'Unable to start the Doctor run.',
      )
      setRunning(false)
    }
  }

  const lastRun = query.data?.lastRun
  const checks = query.data?.checks ?? []

  return (
    <section className="page">
      <h1>Doctor</h1>
      <p className="muted">
        Read-only diagnostics. Doctor never auto-fixes, deletes, migrates, or rotates
        secrets — every check reports a status and sanitized detail only.
      </p>
      <div className="page-actions">
        <button
          type="button"
          className="primary-action"
          onClick={onRun}
          disabled={running}
        >
          {running ? 'Queuing…' : 'Run Doctor'}
        </button>
      </div>
      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}
      {query.isError && (
        <p className="form-error" role="alert">
          Unable to load the Doctor report.{' '}
          {query.error instanceof Error ? query.error.message : ''}
        </p>
      )}
      {lastRun && (
        <p className="muted">
          Last report:{' '}
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
      {checks.length === 0 && !query.isError && (
        <p className="muted">No Doctor report yet — run Doctor to collect checks.</p>
      )}
      <div className="table-wrap">
        <table className="node-table">
          <caption className="visually-hidden">Doctor checks</caption>
          <thead>
            <tr>
              <th scope="col">Check</th>
              <th scope="col">Status</th>
              <th scope="col">Detail</th>
            </tr>
          </thead>
          <tbody>
            {checks.map((check) => (
              <tr key={check.checkId}>
                <td data-label="Check">
                  {check.label}
                  <small>{check.checkId}</small>
                </td>
                <td data-label="Status">
                  <StatusBadge
                    status={doctorCheckStatusLabel(check.status)}
                    tone={checkTone(check.status)}
                  />
                </td>
                <td data-label="Detail">
                  <small>{check.detail}</small>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  )
}
