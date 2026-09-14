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
import { Alert, AlertDescription } from '../components/ui/alert'
import { Button } from '../components/ui/button'
import { CardX } from '../components/ui/card-x'
import { DataTooltip } from '../components/ui/data-tooltip'
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '../components/ui/dialog'
import { Input } from '../components/ui/input'
import { SURFACE_CARD } from '../lib/surface'
import { cn } from '../lib/utils'

/** Emerald's card shell for the ordered Settings cards. */
const CARD = cn(
  'rounded-md border-none transition-all',
  SURFACE_CARD,
  'hover:shadow-[0_0_20px,0_0_0_1px] hover:shadow-emerald-600/10',
)
const CARD_HEADER = 'flex min-w-0 flex-1 flex-wrap items-start justify-between gap-2'
const LABEL = 'text-xs font-medium tracking-wider text-muted-foreground'
const CONSEQUENCE = 'rounded-md border bg-muted/40 p-3 text-xs text-muted-foreground'
const TEXT_LINK = 'inline-flex min-h-11 min-w-11 items-center font-medium text-primary hover:underline'
const FIELD = 'flex flex-col gap-1'
const NOTICE = 'rounded-md border border-success/30 bg-success/10 p-3 text-sm text-success'
const ERROR = 'rounded-md border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive'
const DETAIL_LIST = 'grid grid-cols-1 gap-2 sm:grid-cols-2'
const DETAIL_ROW = 'min-w-0'
const DETAIL_DD = 'm-0 text-sm break-words'
const INLINE_CODE = 'rounded-sm border bg-muted px-1 py-0.5 text-[11px]'

/** PAGE-ADMIN-SETTINGS: concise Owner-only server-wide configuration. */
export default function AdminSettings() {
  const { generation, status } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''

  return (
    <section className="mx-auto flex w-full min-w-0 max-w-[1280px] flex-col gap-4 pb-12">
      <p className="m-0">
        <Link className={TEXT_LINK} to="/admin">← Admin overview</Link>
      </p>
      <h1 className="text-lg font-semibold break-words">Settings</h1>
      <div className="grid max-w-[58rem] gap-3">
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
    <CardX
      role="article"
      aria-labelledby="history-window-heading"
      bordered={false}
      className={CARD}
      header={
        <div className={CARD_HEADER}>
          <div className="min-w-0">
            <h2 id="history-window-heading" className="text-sm font-medium">History Window</h2>
            <p className="mt-1 text-xs text-muted-foreground">Bounded retention for Block Summaries.</p>
          </div>
        </div>
      }
    >
      {!currentWindow && query.isPending && <p role="status" className="text-sm text-muted-foreground">Loading History Window…</p>}
      {!currentWindow && query.isError && (
        <Alert variant="destructive">
          <AlertDescription>Unable to load the History Window.</AlertDescription>
        </Alert>
      )}

      {currentWindow && (
        <form className="grid gap-4" onSubmit={save} noValidate>
          {query.isError && (
            <Alert variant="destructive">
              <AlertDescription>
                Unable to refresh the History Window. Showing the last successful value.
              </AlertDescription>
            </Alert>
          )}
          <dl className={DETAIL_LIST}>
            <div className={DETAIL_ROW}><dt className={LABEL}>Current</dt><dd className={DETAIL_DD}>{formatDayCount(currentWindow.windowDays)}</dd></div>
            <div className={DETAIL_ROW}><dt className={LABEL}>Default</dt><dd className={DETAIL_DD}>{formatDayCount(currentWindow.defaultDays)}</dd></div>
            <div className={DETAIL_ROW}>
              <dt className={LABEL}>Allowed range</dt>
              <dd className={DETAIL_DD}>
                <span>{formatDayCount(currentWindow.minDays)}</span>–<span>{formatDayCount(currentWindow.maxDays)}</span>
              </dd>
            </div>
            <div className={DETAIL_ROW}>
              <dt className={LABEL}>Last updated</dt>
              <dd className={DETAIL_DD}>
                {currentWindow.updatedAt}
                {currentWindow.updatedBy ? ' by ' + currentWindow.updatedBy : ''}
              </dd>
            </div>
          </dl>

          <p className={CONSEQUENCE}>
            Shortening removes expired history asynchronously. Lengthening cannot recover deleted or missed history.
          </p>

          <div className={FIELD}>
            <label htmlFor="history-window-days" className={LABEL}>New window (days)</label>
            <Input
              id="history-window-days"
              className="max-w-[10rem]"
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
              <p id="history-window-days-error" className="text-xs text-destructive" role="alert">
                {boundsError ?? fieldError}
              </p>
            )}
          </div>

          {days !== null && !boundsError && (
            <div className={CONSEQUENCE} aria-live="polite">
              <h3 className="text-sm font-medium">Impact preview</h3>
              {impact.isFetching && <p className="text-xs text-muted-foreground">Estimating affected rows…</p>}
              {impact.isError && (
                <p className="text-xs text-destructive" role="alert">
                  Unable to preview the impact. Change the value to retry.
                </p>
              )}
              {impact.data && impact.data.windowDays === days && (
                <p className="mt-1">
                  {impact.data.estimatedRows === null || impact.data.estimatedRows === undefined
                    ? 'The number of affected rows is currently unknown.'
                    : <>About <strong>{impact.data.estimatedRows} rows</strong> would be removed when shortening.</>}
                  {' '}Protected history state is preserved.
                </p>
              )}
            </div>
          )}

          <div className={FIELD}>
            <label htmlFor="history-window-confirmation" className={LABEL}>Type the change to confirm</label>
            <Input
              id="history-window-confirmation"
              className="max-w-[16rem]"
              value={confirmation}
              autoComplete="off"
              aria-invalid={confirmation.length > 0 && !confirmationMatches}
              onChange={(event) => setConfirmation(event.target.value)}
            />
            <small className="text-[11px] text-muted-foreground">
              Type <code className={INLINE_CODE}>{confirmationTarget}</code> to confirm.
            </small>
          </div>

          {notice && <p className={NOTICE} role="status">{notice}</p>}
          {error && <p className={ERROR} role="alert">{error}</p>}
          <Button type="submit" className="justify-self-start" disabled={!canSave}>
            {saving ? 'Saving…' : 'Save History Window'}
          </Button>
        </form>
      )}
    </CardX>
  )
}

function SiteAccessSettings({ generation, csrfToken }: SettingsSectionProps) {
  const query = useAdminAccess(generation)
  const [busy, setBusy] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [pendingMode, setPendingMode] = useState<'public' | 'private' | null>(null)

  async function apply(next: 'public' | 'private') {
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

  function requestChange() {
    if (!query.data || busy) return
    setPendingMode(query.data.mode === 'public' ? 'private' : 'public')
  }

  function confirmChange() {
    const next = pendingMode
    setPendingMode(null)
    if (next) void apply(next)
  }

  return (
    <CardX
      role="article"
      aria-labelledby="site-access-heading"
      bordered={false}
      className={CARD}
      header={
        <div className={CARD_HEADER}>
          <div className="min-w-0">
            <h2 id="site-access-heading" className="text-sm font-medium">Site Access Mode</h2>
            <p className="mt-1 text-xs text-muted-foreground">Public permits anonymous Home reads. Private requires Owner login.</p>
          </div>
          {query.data && (
            <StatusBadge
              status={query.data.mode === 'public' ? 'Public' : 'Private'}
              tone={query.data.mode === 'public' ? 'ok' : 'neutral'}
            />
          )}
        </div>
      }
    >
      {!query.data && query.isPending && <p role="status" className="text-sm text-muted-foreground">Loading Site Access Mode…</p>}
      {!query.data && query.isError && (
        <Alert variant="destructive">
          <AlertDescription>Unable to load Site Access Mode.</AlertDescription>
        </Alert>
      )}

      {query.data && (
        <div className="grid gap-4">
          {query.isError && (
            <Alert variant="destructive">
              <AlertDescription>
                Unable to refresh Site Access Mode. Showing the last successful value.
              </AlertDescription>
            </Alert>
          )}
          <p className="text-sm">
            {query.data.mode === 'public'
              ? 'Anonymous visitors can read the Home Public Projection.'
              : 'Home is private and requires Owner login.'}
          </p>
          <p className="text-xs text-muted-foreground">
            A change closes affected streams, clears sensitive caches, discards older responses, and reloads authoritative state.
          </p>
          <Button type="button" className="justify-self-start" disabled={busy} onClick={requestChange}>
            {busy
              ? 'Updating…'
              : query.data.mode === 'public'
                ? 'Make Home Private'
                : 'Make Home Public'}
          </Button>
          {notice && <p className={NOTICE} role="status">{notice}</p>}
          {error && <p className={ERROR} role="alert">{error}</p>}
        </div>
      )}

      <Dialog open={pendingMode !== null} onOpenChange={(open) => { if (!open) setPendingMode(null) }}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              {pendingMode === 'public' ? 'Make Home Public?' : 'Make Home Private?'}
            </DialogTitle>
            <DialogDescription>
              {pendingMode === 'public'
                ? 'Anonymous visitors will be able to read Home.'
                : 'Home will require Owner login.'}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <DialogClose asChild>
              <Button variant="outline">Cancel</Button>
            </DialogClose>
            <Button onClick={confirmChange} disabled={busy}>
              {pendingMode === 'public' ? 'Make Home Public' : 'Make Home Private'}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </CardX>
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
    <CardX
      role="article"
      aria-labelledby="geo-provider-heading"
      bordered={false}
      className={CARD}
      header={
        <div className={CARD_HEADER}>
          <div className="min-w-0">
            <h2 id="geo-provider-heading" className="text-sm font-medium">Geo provider</h2>
            <p className="mt-1 text-xs text-muted-foreground">
              Country resolution for the Peer addresses the Server already observes.
            </p>
          </div>
          {current && (
            <DataTooltip as="span" content="The Server owns this state; the Owner cannot override it directly.">
              <StatusBadge
                status={geoStateLabel(current.state)}
                tone={current.state === 'error' ? 'error' : current.state === 'disabled' ? 'neutral' : 'ok'}
              />
            </DataTooltip>
          )}
        </div>
      }
    >
      {!current && query.isPending && <p role="status" className="text-sm text-muted-foreground">Loading Geo provider…</p>}
      {!current && query.isError && (
        <Alert variant="destructive">
          <AlertDescription>Unable to load Geo provider status.</AlertDescription>
        </Alert>
      )}

      {current && (
        <form className="grid gap-4" onSubmit={save} noValidate>
          {query.isError && (
            <Alert variant="destructive">
              <AlertDescription>
                Unable to refresh the Geo provider. Showing the last successful value.
              </AlertDescription>
            </Alert>
          )}

          <p className={CONSEQUENCE}>
            Country resolution runs in the Server's background path and never blocks report
            ingestion. Local MMDB resolves countries from the operator-provided GeoLite2 Country
            database on this Server. Disabled schedules no lookups at all. Peer addresses never
            leave the Server unless an external provider is selected, and the Server never falls
            back to another provider on its own.
          </p>

          {externalProviders.map((candidate) => (
            <p className={CONSEQUENCE} key={candidate.provider} role="note">
              <strong>{candidate.label}.</strong>{' '}
              {candidate.disclosure ?? GENERIC_EXTERNAL_GEO_NOTICE}{' '}
              Only the Owner can select it, and it is never enabled by an upgrade.
            </p>
          ))}

          <fieldset className="grid gap-2">
            <legend className={LABEL}>Provider</legend>
            {current.providers.map((candidate) => (
              <div key={candidate.provider} className="flex flex-col gap-1">
                <label htmlFor={'geo-provider-' + candidate.provider} className="flex min-h-11 items-center gap-2 text-sm">
                  <input
                    id={'geo-provider-' + candidate.provider}
                    className="size-4 shrink-0 accent-primary"
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
                  <small className="text-[11px] text-muted-foreground"> Sends observed Peer public IPs to a third party.</small>
                )}
                {candidate.unavailable_reason && (
                  <small className="text-[11px] text-muted-foreground"> {candidate.unavailable_reason}</small>
                )}
              </div>
            ))}
          </fieldset>

          <dl className={DETAIL_LIST}>
            <div className={DETAIL_ROW}><dt className={LABEL}>Current</dt><dd className={DETAIL_DD}>{current.provider_label}</dd></div>
            <div className={DETAIL_ROW}><dt className={LABEL}>Status</dt><dd className={DETAIL_DD}>{geoStateLabel(current.state)}</dd></div>
            <div className={DETAIL_ROW}>
              <dt className={LABEL}>Peer addresses</dt>
              <dd className={DETAIL_DD}>
                {currentSendsPeerAddresses
                  ? 'Sent to ' + current.provider_label
                  : 'Stay on this Server'}
              </dd>
            </div>
            <div className={DETAIL_ROW}>
              <dt className={LABEL}>Local database</dt>
              <dd className={DETAIL_DD}>{current.configured ? 'Configured' : 'Not configured'}</dd>
            </div>
            <div className={DETAIL_ROW}>
              <dt className={LABEL}>Cached countries</dt>
              <dd className={DETAIL_DD}>{current.cache_country_count}</dd>
            </div>
            <div className={DETAIL_ROW}>
              <dt className={LABEL}>Lookups pending</dt>
              <dd className={DETAIL_DD}>
                {current.pending_lookup_count === null || current.pending_lookup_count === undefined
                  ? 'Not scheduled'
                  : current.pending_lookup_count}
              </dd>
            </div>
            <div className={DETAIL_ROW}>
              <dt className={LABEL}>Last success</dt>
              <dd className={DETAIL_DD}>{current.last_success_at ?? 'None yet'}</dd>
            </div>
            {current.rate_limited_until && (
              <div className={DETAIL_ROW}>
                <dt className={LABEL}>Rate limited until</dt>
                <dd className={DETAIL_DD}>{current.rate_limited_until}</dd>
              </div>
            )}
          </dl>

          {current.last_error && (
            <p className={ERROR} role="alert">Provider error: {current.last_error}</p>
          )}

          {notice && <p className={NOTICE} role="status">{notice}</p>}
          {error && <p className={ERROR} role="alert">{error}</p>}
          <Button type="submit" className="justify-self-start" disabled={!canSave}>
            {saving ? 'Saving…' : 'Save Geo provider'}
          </Button>
        </form>
      )}

      {current && <GeoRefreshControl geo={current} csrfToken={csrfToken} />}
    </CardX>
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
    <section className="mt-4 grid gap-3 border-t pt-4" aria-labelledby="geo-refresh-heading">
      <h3 id="geo-refresh-heading" className="text-sm font-medium">Refresh Peer geolocation</h3>
      <p className="text-xs text-muted-foreground">
        Re-resolves every public Peer IP any Network currently references, even when a cached
        country result is still valid. One lookup per distinct IP address; the Peer country map
        counts Peer records per Node.
      </p>
      <Button
        type="button"
        variant="outline"
        className="justify-self-start"
        onClick={() => void start()}
        disabled={disabled}
        aria-busy={running || submitting}
      >
        {running ? 'Refreshing…' : 'Refresh Peer geolocation'}
      </Button>
      {unavailable && <p className="text-xs text-muted-foreground" role="note">{unavailable}</p>}
      {error && <p className={ERROR} role="alert">{error}</p>}
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
    <div className="grid gap-3" role="status">
      <p className="text-sm">
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
        <p className="text-xs text-muted-foreground">
          No eligible public Peer address is currently referenced by any Network, so there was
          nothing to re-resolve.
        </p>
      )}
      <dl className={DETAIL_LIST}>
        <div className={DETAIL_ROW}><dt className={LABEL}>Status</dt><dd className={DETAIL_DD}>{refreshStateLabel(refresh.state)}</dd></div>
        <div className={DETAIL_ROW}><dt className={LABEL}>Provider for this run</dt><dd className={DETAIL_DD}>{refresh.provider_label}</dd></div>
        <div className={DETAIL_ROW}>
          <dt className={LABEL}>IP lookups</dt>
          <dd className={DETAIL_DD}>{refresh.completed_lookups} of {refresh.total_lookups} complete</dd>
        </div>
        <div className={DETAIL_ROW}><dt className={LABEL}>Resolved</dt><dd className={DETAIL_DD}>{refresh.resolved_lookups}</dd></div>
        <div className={DETAIL_ROW}><dt className={LABEL}>Without a country</dt><dd className={DETAIL_DD}>{refresh.no_country_lookups}</dd></div>
        <div className={DETAIL_ROW}><dt className={LABEL}>Failed</dt><dd className={DETAIL_DD}>{refresh.failed_lookups}</dd></div>
        {refresh.rate_limited_lookups > 0 && (
          <div className={DETAIL_ROW}><dt className={LABEL}>Rate limited</dt><dd className={DETAIL_DD}>{refresh.rate_limited_lookups}</dd></div>
        )}
        <div className={DETAIL_ROW}>
          <dt className={LABEL}>Peer records referencing them</dt>
          <dd className={DETAIL_DD}>{refresh.peer_records_in_scope}</dd>
        </div>
        <div className={DETAIL_ROW}><dt className={LABEL}>Started</dt><dd className={DETAIL_DD}>{refresh.started_at}</dd></div>
        {refresh.finished_at && <div className={DETAIL_ROW}><dt className={LABEL}>Finished</dt><dd className={DETAIL_DD}>{refresh.finished_at}</dd></div>}
      </dl>
      {refresh.provider_label !== currentProviderLabel && (
        <p className="text-xs text-muted-foreground" role="note">
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
