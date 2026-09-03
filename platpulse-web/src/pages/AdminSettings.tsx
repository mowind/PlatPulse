import { useEffect, useMemo, useState } from 'react'
import { Link } from 'react-router'
import {
  AdminApiError,
  updateAccessSettings,
  updateHistoryWindowEntry,
  useAdminAccess,
  useAdminHistoryWindow,
  useHistoryWindowImpact,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { StatusBadge } from '../components/StatusBadge'

/** PAGE-ADMIN-SETTINGS: concise Owner-only server-wide configuration. */
export default function AdminSettings() {
  const { generation, status } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''

  return (
    <section className="page settings-page">
      <p><Link to="/admin">← Admin overview</Link></p>
      <h1>Settings</h1>
      <div className="settings-sections">
        <HistoryWindowSettings generation={generation} csrfToken={csrfToken} />
        <SiteAccessSettings generation={generation} csrfToken={csrfToken} />
      </div>
    </section>
  )
}

function HistoryWindowSettings({ generation, csrfToken }: SettingsSectionProps) {
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

  const currentWindow = query.data
  const boundsError = currentWindow
    ? validateBounds(currentWindow.minDays, currentWindow.maxDays, days)
    : null
  const confirmationTarget = useMemo(
    () => ('history-window ' + (days ?? '')).trim(),
    [days],
  )
  const confirmationMatches = confirmation.trim() === confirmationTarget
  const previewDays = days !== null && !boundsError ? days : -1
  const impact = useHistoryWindowImpact(generation, previewDays, csrfToken)
  const previewReady = impact.data?.windowDays === days && !impact.isFetching && !impact.isError
  const canSave = Boolean(
    currentWindow &&
    days !== null &&
    !boundsError &&
    confirmationMatches &&
    previewReady &&
    !saving,
  )

  async function save(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (!currentWindow || days === null || !canSave) return
    setSaving(true)
    setNotice(null)
    setError(null)
    setFieldError(null)
    try {
      const response = await updateHistoryWindowEntry(days, csrfToken)
      setNotice(
        'History Window updated to ' + response.window.windowDays +
          ' days (Audit #' + response.auditEventId + ').',
      )
      setConfirmation('')
    } catch (caught) {
      if (caught instanceof AdminApiError && caught.fields.includes('windowDays')) {
        setFieldError(caught.message)
      } else {
        setError(
          caught instanceof AdminApiError
            ? caught.message
            : 'Unable to update the History Window.',
        )
      }
    } finally {
      setSaving(false)
    }
  }

  return (
    <article className="panel settings-card" aria-labelledby="history-window-heading">
      <div className="settings-card-heading">
        <div>
          <h2 id="history-window-heading">History Window</h2>
          <p className="muted">Bounded retention for Block Summaries.</p>
        </div>
      </div>

      {!currentWindow && query.isPending && <p role="status">Loading History Window…</p>}
      {!currentWindow && query.isError && (
        <p className="form-error" role="alert">Unable to load the History Window.</p>
      )}

      {currentWindow && (
        <form className="retention-form settings-form" onSubmit={save} noValidate>
          {query.isError && (
            <p className="form-error" role="alert">
              Unable to refresh the History Window. Showing the last successful value.
            </p>
          )}
          <dl className="detail-list settings-detail-list">
            <div><dt>Current</dt><dd>{formatDayCount(currentWindow.windowDays)}</dd></div>
            <div><dt>Default</dt><dd>{formatDayCount(currentWindow.defaultDays)}</dd></div>
            <div><dt>Minimum</dt><dd>{formatDayCount(currentWindow.minDays)}</dd></div>
            <div><dt>Maximum</dt><dd>{formatDayCount(currentWindow.maxDays)}</dd></div>
            <div>
              <dt>Last updated</dt>
              <dd>
                {currentWindow.updatedAt}
                {currentWindow.updatedBy ? ' by ' + currentWindow.updatedBy : ''}
              </dd>
            </div>
          </dl>

          <p className="settings-consequence">
            Shortening removes expired history asynchronously. Lengthening cannot recover deleted or missed history.
          </p>

          <div className="field">
            <label htmlFor="history-window-days">New window (days)</label>
            <input
              id="history-window-days"
              type="number"
              min={currentWindow.minDays}
              max={currentWindow.maxDays}
              step={1}
              value={days ?? ''}
              aria-invalid={Boolean(boundsError || fieldError)}
              aria-describedby={boundsError || fieldError ? 'history-window-days-error' : undefined}
              onChange={(event) => {
                setDays(event.target.value === '' ? null : Number(event.target.value))
                setConfirmation('')
                setFieldError(null)
                setError(null)
                setNotice(null)
              }}
            />
            {(boundsError || fieldError) && (
              <p id="history-window-days-error" className="field-error" role="alert">
                {boundsError ?? fieldError}
              </p>
            )}
          </div>

          {days !== null && !boundsError && (
            <div className="impact-preview" aria-live="polite">
              <h3>Impact preview</h3>
              {impact.isFetching && <p className="muted">Estimating affected rows…</p>}
              {impact.isError && (
                <p className="form-error" role="alert">
                  Unable to preview the impact. Change the value to retry.
                </p>
              )}
              {impact.data && impact.data.windowDays === days && (
                <p>
                  {impact.data.estimatedRows === null || impact.data.estimatedRows === undefined
                    ? 'The number of affected rows is currently unknown.'
                    : <>About <strong>{impact.data.estimatedRows} rows</strong> would be removed when shortening.</>}
                  {' '}Protected history state is preserved.
                </p>
              )}
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
          <button className="primary-action" type="submit" disabled={!canSave}>
            {saving ? 'Saving…' : 'Save History Window'}
          </button>
        </form>
      )}
    </article>
  )
}

function SiteAccessSettings({ generation, csrfToken }: SettingsSectionProps) {
  const query = useAdminAccess(generation)
  const [busy, setBusy] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  async function toggle() {
    if (!query.data || busy) return
    const next = query.data.mode === 'public' ? 'private' : 'public'
    const confirmed = window.confirm(
      next === 'public'
        ? 'Make Home Public? Anonymous visitors will be able to read Home.'
        : 'Make Home Private? Home will require Owner login.',
    )
    if (!confirmed) return

    setBusy(true)
    setNotice(null)
    setError(null)
    try {
      const result = await updateAccessSettings(next, csrfToken)
      setNotice(
        'Site Access Mode is now ' +
          (result.mode === 'public' ? 'Public' : 'Private') +
          '. Audit was recorded.',
      )
    } catch (caught) {
      setError(
        caught instanceof Error ? caught.message : 'Unable to update Site Access Mode.',
      )
    } finally {
      setBusy(false)
    }
  }

  return (
    <article className="panel settings-card" aria-labelledby="site-access-heading">
      <div className="settings-card-heading">
        <div>
          <h2 id="site-access-heading">Site Access Mode</h2>
          <p className="muted">Public permits anonymous Home reads. Private requires Owner login.</p>
        </div>
        {query.data && (
          <StatusBadge
            status={query.data.mode === 'public' ? 'Public' : 'Private'}
            tone={query.data.mode === 'public' ? 'ok' : 'neutral'}
          />
        )}
      </div>

      {!query.data && query.isPending && <p role="status">Loading Site Access Mode…</p>}
      {!query.data && query.isError && (
        <p className="form-error" role="alert">Unable to load Site Access Mode.</p>
      )}

      {query.data && (
        <div className="settings-form">
          {query.isError && (
            <p className="form-error" role="alert">
              Unable to refresh Site Access Mode. Showing the last successful value.
            </p>
          )}
          <p>
            {query.data.mode === 'public'
              ? 'Anonymous visitors can read the Home Public Projection.'
              : 'Home is private and requires Owner login.'}
          </p>
          <p className="muted">
            A change closes affected streams, clears sensitive caches, discards older responses, and reloads authoritative state.
          </p>
          <button
            className="primary-action"
            type="button"
            disabled={busy}
            onClick={() => void toggle()}
          >
            {busy
              ? 'Updating…'
              : query.data.mode === 'public'
                ? 'Make Home Private'
                : 'Make Home Public'}
          </button>
          {notice && <p className="form-success" role="status">{notice}</p>}
          {error && <p className="form-error" role="alert">{error}</p>}
        </div>
      )}
    </article>
  )
}

type SettingsSectionProps = {
  generation: number
  csrfToken: string
}

function formatDayCount(days: number): string {
  return days + (days === 1 ? ' day' : ' days')
}

function validateBounds(min: number, max: number, days: number | null): string | null {
  if (days === null) return 'Enter a number of days.'
  if (!Number.isInteger(days)) return 'Enter a whole number of days.'
  if (days < min || days > max) return 'Must be between ' + min + ' and ' + max + ' days.'
  return null
}
