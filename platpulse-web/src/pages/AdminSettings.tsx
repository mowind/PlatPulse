import { useEffect, useMemo, useState } from 'react'
import { Link } from 'react-router'
import {
  AdminApiError,
  triggerGeoRefreshEntry,
  updateAccessSettings,
  updateGeoProviderEntry,
  updateHistoryWindowEntry,
  useAdminAccess,
  useAdminGeo,
  useAdminHistoryWindow,
  useHistoryWindowImpact,
} from '../api/admin'
import type { GeoRefreshStatus, GeoStatusDiagnostic } from '../api/generated'
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
      <div className="settings-sections settings-surface">
        <HistoryWindowSettings generation={generation} csrfToken={csrfToken} />
        <SiteAccessSettings generation={generation} csrfToken={csrfToken} />
        <GeoProviderSettings generation={generation} csrfToken={csrfToken} />
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
    <article className="settings-block" aria-labelledby="history-window-heading">
      <div className="settings-block-heading">
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
            <div>
              <dt>Allowed range</dt>
              <dd>
                <span>{formatDayCount(currentWindow.minDays)}</span>–<span>{formatDayCount(currentWindow.maxDays)}</span>
              </dd>
            </div>
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
              className="settings-number-input"
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
    <article className="settings-block" aria-labelledby="site-access-heading">
      <div className="settings-block-heading">
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

/** The last-resort sentence for a provider the Server flagged as sending
 * Peer addresses but for which it returned no disclosure text. The Server
 * owns both the flag and the exact wording (including the fixed destination),
 * because it owns the destination the request is really sent to; this string
 * only exists so a flagged provider can never render with no consequence at
 * all. */
const GENERIC_EXTERNAL_GEO_NOTICE =
  'This provider sends each observed Peer public IP to a third-party service over HTTPS and keeps only the returned country code.'

/**
 * PAGE-ADMIN-SETTINGS Geo provider selection. Only providers this Server
 * actually implements are offered (issues #132, #134, and #135): Disabled,
 * Local MMDB, and the external IPinfo and GeoJS providers. The option list,
 * the privacy consequence, and the availability reason all come from the
 * Server, so a provider the Server cannot run is disabled with its own
 * reason and a provider it does not implement is never rendered. */
function GeoProviderSettings({ generation, csrfToken }: SettingsSectionProps) {
  const query = useAdminGeo(generation)
  const [selection, setSelection] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (query.data) setSelection((current) => current ?? query.data.provider)
  }, [query.data])

  const current = query.data
  const option = current?.providers.find((candidate) => candidate.provider === selection)
  const canSave = Boolean(
    current &&
    selection &&
    selection !== current.provider &&
    option?.available &&
    !saving,
  )
  const externalProviders = current?.providers.filter((candidate) => candidate.sends_peer_addresses) ?? []
  const currentSendsPeerAddresses = Boolean(
    current?.providers.find((candidate) => candidate.provider === current.provider)
      ?.sends_peer_addresses,
  )

  async function save(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (!current || !selection || !canSave) return
    setSaving(true)
    setNotice(null)
    setError(null)
    try {
      const result = await updateGeoProviderEntry(selection, csrfToken)
      setNotice(
        'Geo provider is now ' + result.geo.provider_label +
          ' (Audit #' + result.audit_event_id + ').',
      )
    } catch (caught) {
      setError(
        caught instanceof AdminApiError
          ? caught.message
          : 'Unable to change the Geo provider.',
      )
    } finally {
      setSaving(false)
    }
  }

  return (
    <article className="settings-block" aria-labelledby="geo-provider-heading">
      <div className="settings-block-heading">
        <div>
          <h2 id="geo-provider-heading">Geo provider</h2>
          <p className="muted">
            Country resolution for the Peer addresses the Server already observes.
          </p>
        </div>
        {current && (
          <StatusBadge
            status={geoStateLabel(current.state)}
            tone={current.state === 'error' ? 'error' : current.state === 'disabled' ? 'neutral' : 'ok'}
          />
        )}
      </div>

      {!current && query.isPending && <p role="status">Loading Geo provider…</p>}
      {!current && query.isError && (
        <p className="form-error" role="alert">Unable to load Geo provider status.</p>
      )}

      {current && (
        <form className="settings-form" onSubmit={save} noValidate>
          {query.isError && (
            <p className="form-error" role="alert">
              Unable to refresh the Geo provider. Showing the last successful value.
            </p>
          )}

          <p className="settings-consequence">
            Country resolution runs in the Server's background path and never blocks report
            ingestion. Local MMDB resolves countries from the operator-provided GeoLite2 Country
            database on this Server. Disabled schedules no lookups at all. Peer addresses never
            leave the Server unless an external provider is selected, and the Server never falls
            back to another provider on its own.
          </p>

          {externalProviders.map((candidate) => (
            <p className="settings-consequence" key={candidate.provider} role="note">
              <strong>{candidate.label}.</strong>{' '}
              {candidate.disclosure ?? GENERIC_EXTERNAL_GEO_NOTICE}{' '}
              Only the Owner can select it, and it is never enabled by an upgrade.
            </p>
          ))}

          <fieldset className="field">
            <legend>Provider</legend>
            {current.providers.map((candidate) => (
              <div key={candidate.provider}>
                <label htmlFor={'geo-provider-' + candidate.provider}>
                  <input
                    id={'geo-provider-' + candidate.provider}
                    type="radio"
                    name="geo-provider"
                    value={candidate.provider}
                    checked={selection === candidate.provider}
                    disabled={!candidate.available}
                    onChange={() => {
                      setSelection(candidate.provider)
                      setNotice(null)
                      setError(null)
                    }}
                  />
                  {' '}{candidate.label}
                </label>
                {candidate.sends_peer_addresses && (
                  <small className="muted"> Sends observed Peer public IPs to a third party.</small>
                )}
                {candidate.unavailable_reason && (
                  <small className="muted"> {candidate.unavailable_reason}</small>
                )}
              </div>
            ))}
          </fieldset>

          <dl className="detail-list settings-detail-list">
            <div><dt>Current</dt><dd>{current.provider_label}</dd></div>
            <div><dt>Status</dt><dd>{geoStateLabel(current.state)}</dd></div>
            <div>
              <dt>Peer addresses</dt>
              <dd>
                {currentSendsPeerAddresses
                  ? 'Sent to ' + current.provider_label
                  : 'Stay on this Server'}
              </dd>
            </div>
            <div>
              <dt>Local database</dt>
              <dd>{current.configured ? 'Configured' : 'Not configured'}</dd>
            </div>
            <div>
              <dt>Cached countries</dt>
              <dd>{current.cache_country_count}</dd>
            </div>
            <div>
              <dt>Lookups pending</dt>
              <dd>
                {current.pending_lookup_count === null || current.pending_lookup_count === undefined
                  ? 'Not scheduled'
                  : current.pending_lookup_count}
              </dd>
            </div>
            <div>
              <dt>Last success</dt>
              <dd>{current.last_success_at ?? 'None yet'}</dd>
            </div>
            {current.rate_limited_until && (
              <div>
                <dt>Rate limited until</dt>
                <dd>{current.rate_limited_until}</dd>
              </div>
            )}
          </dl>

          {current.last_error && (
            <p className="form-error" role="alert">Provider error: {current.last_error}</p>
          )}

          {notice && <p className="form-success" role="status">{notice}</p>}
          {error && <p className="form-error" role="alert">{error}</p>}
          <button className="primary-action" type="submit" disabled={!canSave}>
            {saving ? 'Saving…' : 'Save Geo provider'}
          </button>
        </form>
      )}

      {current && <GeoRefreshControl geo={current} csrfToken={csrfToken} />}
    </article>
  )
}

/**
 * PAGE-ADMIN-SETTINGS Geo refresh (issue #136). The Owner-only action really
 * re-resolves every public Peer address the current Networks reference
 * through the selected provider, deliberately bypassing a retained country
 * that is still valid, and the Server reports real per-address progress. The
 * counts are IP lookups; the Peer records those addresses serve are stated
 * beside them, because the Public country map counts Peer records per Node.
 * Geo that cannot resolve anything is refused by the Server with a stable
 * reason, so a refresh is never rendered as a success that did not happen.
 */
function GeoRefreshControl({
  geo,
  csrfToken,
}: {
  geo: GeoStatusDiagnostic
  csrfToken: string
}) {
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const refresh = geo.refresh
  const running = refresh?.state === 'running'
  const unavailable = geo.refresh_unavailable_reason ?? null
  const disabled = submitting || running || unavailable !== null

  async function start() {
    if (disabled) return
    setSubmitting(true)
    setError(null)
    try {
      await triggerGeoRefreshEntry(csrfToken)
    } catch (caught) {
      setError(
        caught instanceof AdminApiError
          ? caught.message
          : 'Unable to refresh Peer geolocation.',
      )
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <section className="geo-refresh" aria-labelledby="geo-refresh-heading">
      <h3 id="geo-refresh-heading">Refresh Peer geolocation</h3>
      <p className="muted">
        Re-resolves every public Peer IP any Network currently references, even when a cached
        country result is still valid. One lookup per distinct IP address; the Peer country map
        counts Peer records per Node.
      </p>
      <button
        className="primary-action"
        type="button"
        onClick={() => void start()}
        disabled={disabled}
        aria-busy={running || submitting}
      >
        {running ? 'Refreshing…' : 'Refresh Peer geolocation'}
      </button>
      {unavailable && <p className="muted" role="note">{unavailable}</p>}
      {error && <p className="form-error" role="alert">{error}</p>}
      {refresh && <GeoRefreshRunStatus refresh={refresh} currentProviderLabel={geo.provider_label} />}
    </section>
  )
}

function GeoRefreshRunStatus({
  refresh,
  currentProviderLabel,
}: {
  refresh: GeoRefreshStatus
  currentProviderLabel: string
}) {
  return (
    <div className="geo-refresh-status" role="status">
      <p>
        {refresh.state === 'running' && (
          <>
            Refreshing: {refresh.completed_lookups} of {refresh.total_lookups} IP lookups done
            ({refresh.resolved_lookups} resolved, {refresh.no_country_lookups} without a country,{' '}
            {refresh.failed_lookups} failed).
          </>
        )}
        {refresh.state === 'completed' && (
          <>
            Refresh complete: {refresh.resolved_lookups} resolved, {refresh.no_country_lookups}{' '}
            without a country, {refresh.failed_lookups} failed.
          </>
        )}
        {refresh.state === 'aborted' && (
          <>
            {refresh.abort_reason ?? 'The refresh stopped early.'} It completed{' '}
            {refresh.completed_lookups} of {refresh.total_lookups} IP lookups
            ({refresh.resolved_lookups} resolved, {refresh.no_country_lookups} without a country,{' '}
            {refresh.failed_lookups} failed).
          </>
        )}
      </p>
      {refresh.total_lookups === 0 && (
        <p className="muted">
          No eligible public Peer address is currently referenced by any Network, so there was
          nothing to re-resolve.
        </p>
      )}
      <dl className="detail-list settings-detail-list">
        <div><dt>Status</dt><dd>{refreshStateLabel(refresh.state)}</dd></div>
        <div><dt>Provider for this run</dt><dd>{refresh.provider_label}</dd></div>
        <div>
          <dt>IP lookups</dt>
          <dd>{refresh.completed_lookups} of {refresh.total_lookups} complete</dd>
        </div>
        <div><dt>Resolved</dt><dd>{refresh.resolved_lookups}</dd></div>
        <div><dt>Without a country</dt><dd>{refresh.no_country_lookups}</dd></div>
        <div><dt>Failed</dt><dd>{refresh.failed_lookups}</dd></div>
        {refresh.rate_limited_lookups > 0 && (
          <div><dt>Rate limited</dt><dd>{refresh.rate_limited_lookups}</dd></div>
        )}
        <div>
          <dt>Peer records referencing them</dt>
          <dd>{refresh.peer_records_in_scope}</dd>
        </div>
        <div><dt>Started</dt><dd>{refresh.started_at}</dd></div>
        {refresh.finished_at && <div><dt>Finished</dt><dd>{refresh.finished_at}</dd></div>}
      </dl>
      {refresh.provider_label !== currentProviderLabel && (
        <p className="muted" role="note">
          This run used {refresh.provider_label}; the current provider is {currentProviderLabel}.
        </p>
      )}
    </div>
  )
}

function refreshStateLabel(state: string): string {
  switch (state) {
    case 'running': return 'Running'
    case 'completed': return 'Completed'
    default: return 'Stopped early'
  }
}

function geoStateLabel(state: string): string {
  switch (state) {
    case 'current': return 'Current'
    case 'stale': return 'Stale'
    case 'error': return 'Error'
    default: return 'Disabled'
  }
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
