import { useState } from 'react'
import { Link, useNavigate } from 'react-router'
import { AdminApiError, createBackupEntry } from '../api/admin'
import { useAuth } from '../auth/AuthContext'

/**
 * PAGE-ADMIN-BACKUP-CREATE (webui.md §4.5/§8.4): Backup Operation
 * submission with typed confirmation. The mutation returns an Operation
 * reference immediately; progress is tracked on the Operation detail.
 * Confirmed, audited, refetched — never optimistic.
 */

const CONFIRMATION_PHRASE = 'create backup'

export default function AdminBackupCreate() {
  const { status } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const navigate = useNavigate()
  const [confirmation, setConfirmation] = useState('')
  const [creating, setCreating] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const confirmed = confirmation.trim().toLowerCase() === CONFIRMATION_PHRASE

  async function onCreate(event: React.FormEvent) {
    event.preventDefault()
    if (!confirmed || creating) return
    setCreating(true)
    setError(null)
    try {
      const response = await createBackupEntry(csrfToken)
      navigate(`/admin/operations/${response.operation.operation.operationId}`)
    } catch (caught) {
      setError(
        caught instanceof AdminApiError ? caught.message : 'Unable to start the backup.',
      )
      setCreating(false)
    }
  }

  return (
    <section className="page">
      <p>
        <Link to="/admin/data/backups">← Backups</Link>
      </p>
      <h1>Create a backup</h1>
      <p className="muted">
        The Server writes a consistent snapshot into the configured backup directory (never
        the state directory), fsyncs it, and records sanitized metadata: checksum, schema
        version, Server version, and data range. Database contents never leave the Server.
      </p>
      <form className="retention-form" onSubmit={onCreate} noValidate>
        <div className="field">
          <label htmlFor="backup-confirmation">Type the confirmation phrase</label>
          <input
            id="backup-confirmation"
            type="text"
            value={confirmation}
            autoComplete="off"
            aria-invalid={confirmation.length > 0 && !confirmed}
            aria-describedby="backup-confirmation-hint"
            onChange={(event) => setConfirmation(event.target.value)}
          />
          <small id="backup-confirmation-hint" className="muted">
            Type <code>{CONFIRMATION_PHRASE}</code> to queue the audited backup Operation.
          </small>
        </div>
        {error && (
          <p className="form-error" role="alert">
            {error}
          </p>
        )}
        <div className="page-actions">
          <button type="submit" className="primary-action" disabled={!confirmed || creating}>
            {creating ? 'Queuing…' : 'Queue backup'}
          </button>
          <button type="button" onClick={() => navigate('/admin/data/backups')}>
            Cancel
          </button>
        </div>
      </form>
    </section>
  )
}
