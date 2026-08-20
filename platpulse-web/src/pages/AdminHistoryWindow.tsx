import { useEffect, useMemo, useState } from 'react'
import { Link } from 'react-router'
import {
  AdminApiError,
  updateHistoryWindowEntry,
  useAdminHistoryWindow,
  useHistoryWindowImpact,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'

/** PAGE-ADMIN-HISTORY-WINDOW: one global, audited Block History bound. */
export default function AdminHistoryWindow() {
  const { generation, status } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const query = useAdminHistoryWindow(generation)
  const [days, setDays] = useState<number | null>(null)
  const [confirmation, setConfirmation] = useState('')
  const [saving, setSaving] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [fieldError, setFieldError] = useState<string | null>(null)

  useEffect(() => {
    if (query.data) setDays((current) => current ?? query.data.windowDays)
  }, [query.data])

  const window = query.data
  const boundsError = window ? validateBounds(window.minDays, window.maxDays, days) : null
  const confirmationTarget = useMemo(
    () => `history-window ${days ?? ''}`.trim(),
    [days],
  )
  const confirmationMatches = confirmation.trim() === confirmationTarget
  const impact = useHistoryWindowImpact(generation, days ?? 0, csrfToken)

  async function save(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (!window || days === null || boundsError || !confirmationMatches || saving) return
    setSaving(true)
    setNotice(null)
    setError(null)
    setFieldError(null)
    try {
      const response = await updateHistoryWindowEntry(days, csrfToken)
      setNotice(`History Window updated to ${response.window.windowDays} days (Audit #${response.auditEventId}).`)
      setConfirmation('')
    } catch (caught) {
      if (caught instanceof AdminApiError && caught.fields.includes('windowDays')) {
        setFieldError(caught.message)
      } else {
        setError(caught instanceof AdminApiError ? caught.message : 'Unable to update the History Window.')
      }
    } finally {
      setSaving(false)
    }
  }

  return (
    <section className="page">
      <p><Link to="/admin">← Admin overview</Link></p>
      <h1>History Window</h1>
      <p className="muted">
        This global bound applies immediately to retained Block Summaries. Shortening it
        asynchronously removes expired history; lengthening it cannot recover deleted or missed data.
      </p>
      {!window && query.isPending && <p role="status">Loading History Window…</p>}
      {!window && query.isError && (
        <p className="form-error" role="alert">Unable to load the History Window.</p>
      )}
      {window && (
        <form className="retention-form" onSubmit={save} noValidate>
          <dl className="detail-list">
            <div><dt>Current window</dt><dd>{window.windowDays} days</dd></div>
            <div><dt>Default</dt><dd>{window.defaultDays} days</dd></div>
            <div><dt>Safety bounds</dt><dd>{window.minDays}–{window.maxDays} days</dd></div>
            <div><dt>Last updated</dt><dd>{window.updatedAt}{window.updatedBy ? ` by ${window.updatedBy}` : ''}</dd></div>
          </dl>
          <div className="field">
            <label htmlFor="history-window-days">New window (days)</label>
            <input
              id="history-window-days"
              type="number"
              min={window.minDays}
              max={window.maxDays}
              step={1}
              value={days ?? ''}
              aria-invalid={Boolean(boundsError)}
              aria-describedby={boundsError ? 'history-window-days-error' : undefined}
              onChange={(event) => {
                setDays(event.target.value === '' ? null : Number(event.target.value))
                setConfirmation('')
                setFieldError(null)
              }}
            />
            {(boundsError || fieldError) && <p id="history-window-days-error" className="field-error" role="alert">{boundsError ?? fieldError}</p>}
          </div>
          {days !== null && !boundsError && (
            <div className="impact-preview" aria-live="polite">
              <h2>Consequences</h2>
              {impact.isFetching && <p className="muted">Estimating expired rows…</p>}
              {impact.isError && <p className="form-error" role="alert">Unable to preview the consequences; change the value to retry.</p>}
              {impact.data && <p>{impact.data.estimatedRows === null || impact.data.estimatedRows === undefined ? 'The number of rows affected is currently unknown.' : <>About <strong>{impact.data.estimatedRows} rows</strong> would be removed when the bound is shortened.</>} Protected history state is preserved.</p>}
            </div>
          )}
          <div className="field">
            <label htmlFor="history-window-confirmation">Type the change to confirm</label>
            <input
              id="history-window-confirmation"
              value={confirmation}
              autoComplete="off"
              aria-invalid={confirmation.length > 0 && !confirmationMatches}
              onChange={(event) => setConfirmation(event.target.value)}
            />
            <small className="muted">Type <code>{confirmationTarget}</code> to confirm.</small>
          </div>
          {notice && <p className="form-success" role="status">{notice}</p>}
          {error && <p className="form-error" role="alert">{error}</p>}
          <button className="primary-action" type="submit" disabled={Boolean(boundsError) || !confirmationMatches || saving || impact.isError}>
            {saving ? 'Saving…' : 'Save History Window'}
          </button>
        </form>
      )}
    </section>
  )
}

function validateBounds(min: number, max: number, days: number | null): string | null {
  if (days === null) return 'Enter a number of days.'
  if (!Number.isInteger(days) || days < min || days > max) return `Must be between ${min} and ${max} days.`
  return null
}
