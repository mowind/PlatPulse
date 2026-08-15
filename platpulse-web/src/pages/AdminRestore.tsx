import { useEffect, useMemo, useRef, useState } from 'react'
import { Link } from 'react-router'
import {
  AdminApiError,
  adminQueryClient,
  operationKindLabel,
  operationStatusLabel,
  operationTone,
  submitRestoreEntry,
  useAdminBackups,
  useAdminOperation,
  useAdminOperations,
  useRestoreValidation,
  verificationLabel,
  verificationTone,
} from '../api/admin'
import { ready } from '../api/generated/sdk.gen'
import { useAuth } from '../auth/AuthContext'
import { StatusBadge, formatObservedAt } from '../components/StatusBadge'

/**
 * PAGE-ADMIN-RESTORE (webui.md §4.5/§8.4, design §20.2): the highest-risk
 * Restore workflow. A dedicated route with explicit prerequisites, backup
 * identity selection, Server-computed checksum/integrity/schema
 * validation, and a typed confirmation (the backup file name). Restore
 * requires an exclusive stopped-Server condition: the running Server
 * validates and then refuses with the typed
 * `restore_requires_stopped_server` failure, preserving the current
 * database (`SCN-DATA-RESTORE-SERVER-RUNNING`). Restore never restores
 * secret files. After an offline apply (platpulse-server restore) and a
 * Server restart, this page presents the recorded Restore Operation once
 * authoritative health/readiness and the data refetch have settled.
 */

export default function AdminRestore() {
  const { generation, status } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const backups = useAdminBackups(generation)
  const restoreHistory = useAdminOperations(generation, { kind: 'restore' })

  const [selectedId, setSelectedId] = useState('')
  const [validateRequested, setValidateRequested] = useState(false)
  const [confirmation, setConfirmation] = useState('')
  const [submitting, setSubmitting] = useState(false)
  const [submittedId, setSubmittedId] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  const selected = useMemo(
    () => backups.data?.find((artifact) => artifact.artifactId === selectedId),
    [backups.data, selectedId],
  )
  const validation = useRestoreValidation(
    generation,
    selectedId,
    csrfToken,
    validateRequested && selectedId.length > 0,
  )
  const submitted = useAdminOperation(generation, submittedId ?? '')

  const validationOk =
    validation.isSuccess &&
    validation.data.checksumOk === true &&
    validation.data.integrityOk === true &&
    validation.data.schemaCompatible === true
  const confirmed =
    selected !== undefined &&
    confirmation.trim().toLowerCase() === selected.filename.toLowerCase()

  // The newest Restore Operation is the presentation anchor after an
  // offline apply + Server restart (SSE reconnects and REST refetches).
  const latestRestore = restoreHistory.data?.[0]

  async function onValidate() {
    setValidateRequested(true)
    setError(null)
  }

  async function onSubmit(event: React.FormEvent) {
    event.preventDefault()
    if (!selected || !validationOk || !confirmed || submitting) return
    setSubmitting(true)
    setError(null)
    try {
      const response = await submitRestoreEntry(selected.artifactId, confirmation, csrfToken)
      setSubmittedId(response.operation.operation.operationId)
      setValidateRequested(false)
    } catch (caught) {
      setError(
        caught instanceof AdminApiError ? caught.message : 'Unable to start the Restore.',
      )
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <section className="page">
      <p>
        <Link to="/admin/data/backups">← Backups</Link>
      </p>
      <h1>Restore a backup</h1>
      <p className="muted">
        Restore replaces the Server database with one validated backup. It is the
        highest-risk operation: it requires an exclusive stopped Server, never restores
        secret files, and preserves the current database on any validation or execution
        failure.
      </p>

      <h2>Prerequisites</h2>
      <ul className="issue-list">
        <li>
          <strong>Exclusive stopped-Server condition</strong> — a running Server refuses
          to apply a Restore; the current database stays authoritative. Apply offline
          with <code>platpulse-server restore --artifact-id &lt;id&gt; --yes</code> after
          stopping the Server.
        </li>
        <li>
          <strong>Checksum, integrity, and schema validation</strong> — the Server
          re-verifies the artifact file before any confirmation; newer unsupported
          schemas are refused.
        </li>
        <li>
          <strong>Secrets are never restored</strong> — pepper, credentials, and provider
          tokens live outside the database and are never replaced.
        </li>
        <li>
          <strong>Failure preserves the current database</strong> — a failed validation
          or interrupted apply reports a recoverable typed Operation failure and never
          rolls back good state.
        </li>
      </ul>

      {latestRestore && (
        <>
          <h2>Latest Restore Operation</h2>
          <dl className="detail-list">
            <div>
              <dt>Outcome</dt>
              <dd>
                <StatusBadge
                  status={operationStatusLabel(latestRestore.status)}
                  tone={operationTone(latestRestore.status)}
                />{' '}
                <Link to={`/admin/operations/${latestRestore.operationId}`}>
                  {operationKindLabel(latestRestore.kind)}
                </Link>{' '}
                <small>{formatObservedAt(latestRestore.createdAt)}</small>
              </dd>
            </div>
            <div>
              <dt>Request ID</dt>
              <dd>{latestRestore.requestId ?? '—'}</dd>
            </div>
          </dl>
          {latestRestore.status === 'succeeded' ||
          latestRestore.status === 'succeeded_with_warnings' ? (
            <RestoreSucceededPanel operationId={latestRestore.operationId} />
          ) : null}
        </>
      )}

      <h2>Select the backup</h2>
      {backups.isError && (
        <p className="form-error" role="alert">
          Unable to load backup artifacts.{' '}
          {backups.error instanceof Error ? backups.error.message : ''}
        </p>
      )}
      {backups.isSuccess && backups.data.length === 0 && (
        <p className="muted">No backup artifacts yet — create one first.</p>
      )}
      {backups.isSuccess && backups.data.length > 0 && (
        <div className="table-wrap">
          <table className="node-table">
            <caption className="visually-hidden">Backups available for Restore</caption>
            <thead>
              <tr>
                <th scope="col">Select</th>
                <th scope="col">Artifact</th>
                <th scope="col">Schema</th>
                <th scope="col">Verification</th>
                <th scope="col">Created</th>
              </tr>
            </thead>
            <tbody>
              {backups.data.map((artifact) => (
                <tr key={artifact.artifactId}>
                  <td data-label="Select">
                    <input
                      type="radio"
                      name="restore-artifact"
                      id={`restore-${artifact.artifactId}`}
                      value={artifact.artifactId}
                      checked={selectedId === artifact.artifactId}
                      onChange={() => {
                        setSelectedId(artifact.artifactId)
                        setValidateRequested(false)
                        setConfirmation('')
                      }}
                    />
                  </td>
                  <td data-label="Artifact">
                    <label htmlFor={`restore-${artifact.artifactId}`}>
                      {artifact.filename}
                    </label>
                  </td>
                  <td data-label="Schema">{artifact.schemaVersion}</td>
                  <td data-label="Verification">
                    <StatusBadge
                      status={verificationLabel(artifact.verification)}
                      tone={verificationTone(artifact.verification)}
                    />
                  </td>
                  <td data-label="Created">
                    <small>{formatObservedAt(artifact.createdAt)}</small>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {selected && (
        <>
          <h2>Validate the backup</h2>
          <p className="muted">
            The Server re-computes the checksum and integrity of the artifact file and
            checks schema compatibility before any confirmation. Validation is read-only.
          </p>
          {!validateRequested ? (
            <button type="button" className="primary-action" onClick={onValidate}>
              Validate this backup
            </button>
          ) : (
            <dl className="detail-list" aria-live="polite">
              <div>
                <dt>Checksum</dt>
                <dd>
                  <ValidationCheck ok={validation.data?.checksumOk} loading={validation.isFetching} />
                </dd>
              </div>
              <div>
                <dt>Integrity</dt>
                <dd>
                  <ValidationCheck ok={validation.data?.integrityOk} loading={validation.isFetching} />
                </dd>
              </div>
              <div>
                <dt>Schema compatibility</dt>
                <dd>
                  <ValidationCheck ok={validation.data?.schemaCompatible} loading={validation.isFetching} />
                </dd>
              </div>
              <div>
                <dt>Schema versions</dt>
                <dd>
                  {validation.data
                    ? `backup ${validation.data.schemaVersion} / Server ${validation.data.currentSchemaVersion}`
                    : '—'}
                </dd>
              </div>
            </dl>
          )}
          {validation.isError && (
            <p className="form-error" role="alert">
              Unable to validate the backup.{' '}
              {validation.error instanceof Error ? validation.error.message : ''}
            </p>
          )}
          {validation.isSuccess && !validationOk && (
            <p className="form-error" role="alert">
              {validation.data.error ?? 'Restore validation failed'}:{' '}
              {validation.data.message ?? 'The backup is not restorable as-is.'}
            </p>
          )}

          <h2>Typed confirmation</h2>
          <form className="retention-form" onSubmit={onSubmit} noValidate>
            <div className="field">
              <label htmlFor="restore-confirmation">
                Type the backup file name to confirm
              </label>
              <input
                id="restore-confirmation"
                type="text"
                value={confirmation}
                autoComplete="off"
                aria-invalid={confirmation.length > 0 && !confirmed}
                aria-describedby="restore-confirmation-hint"
                onChange={(event) => setConfirmation(event.target.value)}
              />
              <small id="restore-confirmation-hint" className="muted">
                Type <code>{selected.filename}</code> to confirm the exact backup that
                would replace the database.
              </small>
            </div>
            {error && (
              <p className="form-error" role="alert">
                {error}
              </p>
            )}
            <div className="page-actions">
              <button
                type="submit"
                className="danger-action"
                disabled={!validationOk || !confirmed || submitting}
              >
                {submitting ? 'Queuing…' : 'Start Restore'}
              </button>
            </div>
            {!validationOk && (
              <p className="muted">
                The backup must pass checksum, integrity, and schema validation before
                Restore can be started.
              </p>
            )}
          </form>
        </>
      )}

      {submitted && submitted.data?.operation && (
        <>
          <h2>Restore Operation</h2>
          <RestoreOperationInline operationId={submitted.data.operation.operationId} />
        </>
      )}
    </section>
  )
}

function ValidationCheck({
  ok,
  loading,
}: {
  ok: boolean | null | undefined
  loading: boolean
}) {
  if (loading && ok === undefined) {
    return <StatusBadge status="Checking…" tone="warning" />
  }
  if (ok === null || ok === undefined) {
    // A short-circuited check was never reached; it is not a passing
    // result (webui.md §5: collection/value state must stay honest).
    return <StatusBadge status="Not checked" tone="neutral" />
  }
  return <StatusBadge status={ok ? 'Pass' : 'Fail'} tone={ok ? 'ok' : 'error'} />
}

/**
 * Presents a successful Restore only after authoritative health/readiness
 * and a data refetch: the page refetches the Admin cache once when the
 * succeeded Operation appears, polls /health/ready while it settles, and
 * shows the refetched backup summary with the new state.
 */
function RestoreSucceededPanel({ operationId }: { operationId: string }) {
  const { generation } = useAuth()
  const backups = useAdminBackups(generation)
  const invalidated = useRef<string | null>(null)
  const [readiness, setReadiness] = useState<'ready' | 'not_ready' | 'checking'>('checking')

  useEffect(() => {
    if (invalidated.current !== operationId) {
      invalidated.current = operationId
      void adminQueryClient.invalidateQueries()
    }
  }, [operationId])

  useEffect(() => {
    let cancelled = false
    let timer: ReturnType<typeof setTimeout> | undefined
    const check = async () => {
      const result = await ready({})
      if (cancelled) return
      const data = result.data
      setReadiness(data && data.status === 'ready' ? 'ready' : 'not_ready')
      if (data && data.status !== 'ready') {
        timer = setTimeout(check, 5000)
      }
    }
    void check()
    return () => {
      cancelled = true
      if (timer) clearTimeout(timer)
    }
  }, [])

  const dataSettled = backups.isSuccess && !backups.isFetching

  return (
    <div className="form-success" role="status" aria-live="polite">
      <p>
        <strong>Restore succeeded.</strong>{' '}
        {readiness === 'checking' && 'Checking Server readiness…'}
        {readiness === 'not_ready' && (
          <>Waiting for the Server to report ready — the restored database is still
            settling (forward migration or startup checks).</>
        )}
        {readiness === 'ready' && dataSettled && (
          <>
            The Server is ready and the data views were refetched from the restored
            database ({backups.data.length} backup artifacts listed).
          </>
        )}
      </p>
      {readiness === 'ready' && dataSettled && (
        <p>
          <Link to={`/admin/operations/${operationId}`}>View the Restore Operation</Link>
        </p>
      )}
    </div>
  )
}

/** Inline Operation state for the just-submitted Restore: progress,
 * warnings, errors, request ID, and the Audit link stay visible after
 * navigation or SSE loss (REST is authoritative). */
function RestoreOperationInline({ operationId }: { operationId: string }) {
  const { generation } = useAuth()
  const query = useAdminOperation(generation, operationId)
  const data = query.data
  const operation = data?.operation

  if (!data || !operation) {
    return query.isError ? (
      <p className="form-error" role="alert">
        Unable to load the Restore Operation.
      </p>
    ) : (
      <p className="muted">Loading the Restore Operation…</p>
    )
  }

  const refused = operation.status === 'failed' && data.errors.some(
    (issue) => issue.code === 'restore_requires_stopped_server',
  )

  return (
    <>
      <div className="page-actions">
        <StatusBadge
          status={operationStatusLabel(operation.status)}
          tone={operationTone(operation.status)}
        />
        <Link to={`/admin/operations/${operation.operationId}`}>Operation details</Link>
      </div>
      <dl className="detail-list">
        <div>
          <dt>Request ID</dt>
          <dd>{operation.requestId ?? '—'}</dd>
        </div>
        <div>
          <dt>Created</dt>
          <dd>{formatObservedAt(operation.createdAt)}</dd>
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
      {refused && (
        <p className="form-error" role="alert">
          Restore refused before any mutation: an exclusive stopped-Server condition is
          required. The current database remains authoritative. Stop the Server and run{' '}
          <code>platpulse-server restore --artifact-id &lt;id&gt; --yes</code> to apply
          this backup.
        </p>
      )}
      {data.errors.length > 0 && !refused && (
        <>
          <h3>Errors</h3>
          <ul className="issue-list issue-list-error">
            {data.errors.map((item, index) => (
              <li key={`${item.code}-${index}`}>
                <strong>{item.code}</strong>: {item.message}
              </li>
            ))}
          </ul>
        </>
      )}
      {data.warnings.length > 0 && (
        <>
          <h3>Warnings</h3>
          <ul className="issue-list">
            {data.warnings.map((warning, index) => (
              <li key={`${warning.code}-${index}`}>
                <strong>{warning.code}</strong>: {warning.message}
              </li>
            ))}
          </ul>
        </>
      )}
    </>
  )
}
