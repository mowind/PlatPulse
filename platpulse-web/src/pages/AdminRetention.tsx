import { useEffect, useState } from 'react'
import { Link } from 'react-router'

import {
  AdminApiError,
  createRetentionPreviewEntry,
  runRetentionEntry,
  updateRetentionPolicyEntry,
  useAdminRetention,
  useRetentionImpact,
} from '../api/admin'
import type { OperationSummary, RetentionPolicyDto, RetentionPreviewDto } from '../api/generated'
import { useAuth } from '../auth/AuthContext'
import { StatusBadge, formatObservedAt } from '../components/StatusBadge'
import { Alert, AlertDescription } from '../components/ui/alert'
import { Button } from '../components/ui/button'
import { CardX } from '../components/ui/card-x'
import { Empty } from '../components/ui/empty'
import { Input } from '../components/ui/input'
import {
  CARD_SURFACE,
  DetailItem,
  DetailList,
  OperationProgress,
  OperationStatus,
  errorMessage,
  indeterminateOutcome,
  recordedAge,
  shortId,
} from './operationsShared'

/**
 * PAGE-ADMIN-RETENTION (issue #210). The Server owns retention: the catalogue,
 * the safety floors, the cutoffs, the expiring preview, and the queued run.
 * This page only reads the policy table, changes one bounded value, composes
 * the preview a run is bound to, and queues that run.
 *
 * Three Server rules shape the copy:
 *  1. A preview is the authority for a run. Its counts are estimates (upper
 *     bounds) computed at composition time, never a frozen set of rows; it
 *     binds the policy versions, the scope, and a fixed per-family cutoff.
 *  2. A run is queued by preview id and never re-estimates. An expired or
 *     no-longer-matching preview is refused by the Server, and the Operator
 *     composes a fresh preview instead of retrying.
 *  3. A policy save submits the version the Operator read, so a concurrent
 *     edit is refused instead of silently overwritten.
 */

const LABEL = 'text-xs font-medium tracking-wider text-muted-foreground'
const LINK = 'inline-flex min-h-11 items-center underline underline-offset-4'
const FIELD = 'flex flex-col gap-1'
const CONSEQUENCE = 'min-w-0 rounded-md border bg-muted/40 p-3 text-xs break-words text-muted-foreground'
const NOTICE = 'rounded-md border border-success/30 bg-success/10 p-3 text-sm text-success'
const ERROR = 'rounded-md border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive'
const INLINE_CODE = 'rounded-sm border bg-muted px-1 py-0.5 text-[11px]'

/**
 * A transport failure means the Server may never have seen the request, so the
 * page never guesses an outcome and never re-sends by itself.
 */
const UNKNOWN_POLICY_OUTCOME =
  'The request may not have reached the Server, so whether this policy changed is unknown. The table above is the authority: it has been reloaded, and the recorded value must be read before saving again. This page never re-sends the write by itself.'
const UNKNOWN_PREVIEW_OUTCOME =
  'The request may not have reached the Server, so whether a preview was composed is unknown. Nothing is bound here until one is composed successfully; composing again is safe and deletes nothing.'
const UNKNOWN_RUN_OUTCOME =
  'The request may not have reached the Server, so whether a run was queued is unknown. The recorded retention run below is the authority; this page never re-sends the run by itself.'

/** The Server's own meaning of a retention value: 0 keeps history forever. */
function formatRetention(days: number): string {
  if (days === 0) return 'Keep forever'
  if (days === 1) return '1 day'
  return String(days) + ' days'
}

function formatBounds(policy: RetentionPolicyDto): string {
  if (policy.maxDays === 0) {
    return policy.minDays === 0
      ? 'Keep forever'
      : 'Keep forever, or at least ' + formatRetention(policy.minDays)
  }
  return formatRetention(policy.minDays) + ' – ' + formatRetention(policy.maxDays)
}

/**
 * The page mirrors the Server's own contract (retention::validate_policy_days)
 * from the bounds the Server sends, so an obvious mistake is refused before a
 * round trip and the floor is visible as a number. It never widens what the
 * Server accepts: the Server still refuses any value it does not implement.
 */
function localBoundsError(policy: RetentionPolicyDto, days: number): string | null {
  if (days < 0) return 'Retention must be zero or more days.'
  if (policy.maxDays === 0) {
    if (policy.minDays === 0) {
      return days === 0
        ? null
        : 'This family can only be kept forever (0 days); the Server implements no bounded cleanup for it.'
    }
    if (days !== 0 && days < policy.minDays) {
      return 'This family cannot be lowered below ' + formatRetention(policy.minDays) + '.'
    }
    return null
  }
  if (days < policy.minDays) {
    return 'The safety floor for this family is ' + formatRetention(policy.minDays) + '.'
  }
  if (days > policy.maxDays) {
    return 'The safety ceiling for this family is ' + formatRetention(policy.maxDays) + '.'
  }
  return null
}

function isActionable(policy: RetentionPolicyDto): boolean {
  return policy.supported && policy.enabled
}

/** A preview the page knows the Server will refuse a run for. */
type Rejection = { previewId: string; message: string }

function isExpired(preview: RetentionPreviewDto): boolean {
  const expires = Date.parse(preview.expiresAt)
  return Number.isFinite(expires) && expires <= Date.now()
}

/** PAGE-ADMIN-RETENTION: the retention surface for the Owner. */
export default function AdminRetention() {
  const { generation, status } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const overview = useAdminRetention(generation)
  const data = overview.data

  const [selectedFamily, setSelectedFamily] = useState<string | null>(null)
  // The preview this page is bound to. A Server refetch never rebinds the page
  // to a different preview: only composing one does, so the Operator always
  // runs the estimates they reviewed.
  const [composed, setComposed] = useState<RetentionPreviewDto | null>(null)
  const [rejection, setRejection] = useState<Rejection | null>(null)

  const policies = data?.policies ?? []
  const selected = policies.find((policy) => policy.family === selectedFamily) ?? null
  const boundPreview = composed ?? data?.preview ?? null
  const boundRejection =
    rejection && boundPreview && rejection.previewId === boundPreview.previewId ? rejection : null

  /**
   * A save changes a policy version, so a preview composed against the old
   * value no longer matches what the run would execute. The page says so
   * instead of offering a run the Server is guaranteed to refuse.
   */
  function onPolicySaved(saved: RetentionPolicyDto) {
    const preview = boundPreview
    if (!preview) return
    const inScope = preview.scope ? preview.scope.includes(saved.family) : true
    if (!inScope) return
    setRejection({
      previewId: preview.previewId,
      message:
        'The ' +
        saved.label +
        ' policy changed after this preview was composed, so the plan it froze no longer matches the ' +
        'current policies. The Server refuses a run for it; compose a new preview to run the new value.',
    })
  }

  return (
    <section data-slot="retention-page" className="w-full min-w-0 space-y-4 pb-12">
      <div className="space-y-3">
        <h1 className="text-xl font-semibold tracking-tight">Retention</h1>
        <p className="text-sm text-muted-foreground">
          The Server owns retention. This page reads the bound each family is kept under, changes one value at a
          time, composes the impact preview a run is bound to, and queues that run. No count here is a promise
          about rows: a preview&apos;s numbers are Server estimates (upper bounds) computed when the preview was
          composed, and a run executes the plan that preview froze.
        </p>
      </div>

      {!data && overview.isPending && (
        <p role="status" className="text-sm">
          <StatusBadge status="Starting" tone="neutral" /> Loading retention policies…
        </p>
      )}
      {!data && overview.isError && (
        <div role="alert" className="space-y-2 text-sm">
          <p>{errorMessage(overview.error, 'Unable to load retention policies')}</p>
          <Button variant="link" size="sm" onClick={() => void overview.refetch()}>
            Try again
          </Button>
        </div>
      )}
      {data && overview.isError && (
        <Alert variant="destructive">
          <AlertDescription>
            Unable to refresh retention policies. Showing the last successful values; nothing below is zeroed
            or marked Healthy because this refresh failed.
          </AlertDescription>
        </Alert>
      )}

      {data && (
        <>
          <PoliciesCard
            policies={policies}
            selectedFamily={selected?.family ?? null}
            onSelect={(family) => {
              setSelectedFamily(family)
              setRejection(null)
            }}
            onDeselect={() => setSelectedFamily(null)}
          />

          {selected && (
            <PolicyEditor
              key={selected.family}
              policy={selected}
              generation={generation}
              csrfToken={csrfToken}
              onSaved={onPolicySaved}
              onClose={() => setSelectedFamily(null)}
            />
          )}

          <PreviewPanel
            preview={boundPreview}
            rejection={boundRejection}
            csrfToken={csrfToken}
            onComposed={(preview) => {
              setComposed(preview)
              setRejection(null)
            }}
          />

          <RunPanel
            preview={boundPreview}
            rejection={boundRejection}
            csrfToken={csrfToken}
            onRejected={(message) => {
              if (boundPreview) setRejection({ previewId: boundPreview.previewId, message })
            }}
          />

          <ProtectedStateCard states={data.protectedState} />

          <LastRunCard lastRun={data.lastRun ?? null} />
        </>
      )}
    </section>
  )
}

/** The policy table: what each family is kept under, and which are actionable. */
function PoliciesCard({
  policies,
  selectedFamily,
  onSelect,
  onDeselect,
}: {
  policies: RetentionPolicyDto[]
  selectedFamily: string | null
  onSelect: (family: string) => void
  onDeselect: () => void
}) {
  return (
    <CardX
      size="medium"
      className={CARD_SURFACE}
      contentClassName="p-0"
      segmented
      title={'Retention policies · ' + String(policies.length)}
      data-slot="retention-policies"
    >
      {policies.length === 0 ? (
        <Empty description="The Server reports no retention families. Nothing can be edited until it does." />
      ) : (
        <div className="overflow-x-auto">
          <table data-stack data-slot="retention-policies-table" className="w-full text-sm">
            <caption className="sr-only">
              Retention policies: the bound each family is kept under, its allowed range, and whether the Server
              can act on it
            </caption>
            <thead>
              <tr className="border-b">
                <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                  Family
                </th>
                <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                  Retained
                </th>
                <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                  Allowed range
                </th>
                <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                  Default
                </th>
                <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                  State
                </th>
                <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                  Updated
                </th>
                <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                  Edit
                </th>
              </tr>
            </thead>
            <tbody>
              {policies.map((policy) => (
                <tr key={policy.family} className="border-b border-border/60 align-top">
                  <th scope="row" data-label="Family" className="min-w-0 px-3 py-3 text-left font-normal">
                    {policy.label}
                    <span className="block font-mono text-xs text-muted-foreground">{policy.family}</span>
                  </th>
                  <td data-label="Retained" className="min-w-0 px-3 py-3">
                    {policy.supported ? (
                      formatRetention(policy.retentionDays)
                    ) : (
                      <span className="text-muted-foreground">
                        The Server implements no cleanup for this family, so no retention is claimed for it.
                      </span>
                    )}
                  </td>
                  <td data-label="Allowed range" className="min-w-0 px-3 py-3">
                    {formatBounds(policy)}
                  </td>
                  <td data-label="Default" className="min-w-0 px-3 py-3">
                    {formatRetention(policy.defaultDays)}
                  </td>
                  <td data-label="State" className="min-w-0 px-3 py-3">
                    {isActionable(policy) ? (
                      <StatusBadge status="Current" tone="ok" />
                    ) : policy.supported ? (
                      <StatusBadge status="Disabled" tone="neutral" />
                    ) : (
                      <StatusBadge status="Unsupported" tone="warning" />
                    )}
                  </td>
                  <td data-label="Updated" className="min-w-0 px-3 py-3">
                    {formatObservedAt(policy.updatedAt)}
                    {policy.updatedBy ? <span className="block text-xs text-muted-foreground">by {policy.updatedBy}</span> : null}
                  </td>
                  <td data-label="Edit" className="min-w-0 px-3 py-3">
                    {isActionable(policy) ? (
                      <Button
                        variant="outline"
                        size="sm"
                        className="min-h-11"
                        data-slot="retention-policy-edit"
                        aria-label={'Edit ' + policy.label + ' retention'}
                        aria-expanded={selectedFamily === policy.family}
                        aria-controls="retention-edit-form"
                        onClick={() => (selectedFamily === policy.family ? onDeselect() : onSelect(policy.family))}
                      >
                        {selectedFamily === policy.family ? 'Close' : 'Edit'}
                      </Button>
                    ) : (
                      <span className="text-xs text-muted-foreground">Not editable</span>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      <p className="px-3 py-3 text-xs text-muted-foreground">
        A family the Server does not implement, or one it has switched off, is not actionable: the page never
        claims history is retained that the Server does not keep. Every minimum already includes the safety floor
        the Server enforces, and a longer existing retention is never shortened by anything except an explicit
        edit like this one.
      </p>
    </CardX>
  )
}

/** Bounded, audited edit of one policy value, with an impact estimate. */
function PolicyEditor({
  policy,
  generation,
  csrfToken,
  onSaved,
  onClose,
}: {
  policy: RetentionPolicyDto
  generation: number
  csrfToken: string
  onSaved: (policy: RetentionPolicyDto) => void
  onClose: () => void
}) {
  const [days, setDays] = useState<number | null>(policy.retentionDays)
  const [confirmation, setConfirmation] = useState('')
  const [saving, setSaving] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [fieldError, setFieldError] = useState<string | null>(null)

  const boundsError = days === null ? null : localBoundsError(policy, days)
  // The estimate is asked of the Server for every non-negative draft, including
  // one the page already knows is out of bounds: the Server owns the contract,
  // so its refusal is what the Operator reads.
  const impact = useRetentionImpact(generation, policy.family, days !== null && days >= 0 ? days : -1, csrfToken)
  const impactReady = impact.data?.retentionDays === days && !impact.isFetching && !impact.isError
  const confirmationTarget = 'retention ' + policy.family + ' ' + String(days ?? '')
  const confirmationMatches = confirmation.trim() === confirmationTarget
  const canSave = Boolean(
    days !== null && !boundsError && confirmationMatches && impactReady && !saving && csrfToken.length > 0,
  )

  async function save(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (days === null || !canSave) return
    setSaving(true)
    setNotice(null)
    setError(null)
    setFieldError(null)
    try {
      const response = await updateRetentionPolicyEntry(
        policy.family,
        days,
        policy.policyVersion,
        csrfToken,
      )
      setNotice(
        policy.label +
          ' is now retained for ' +
          formatRetention(response.policy.retentionDays) +
          ' (Audit #' +
          String(response.auditEventId) +
          '). Releasing history older than that bound is a separate, previewed run.',
      )
      setDays(response.policy.retentionDays)
      setConfirmation('')
      onSaved(response.policy)
    } catch (caught) {
      if (caught instanceof AdminApiError && caught.code === 'retention_policy_version_conflict') {
        // The value the Operator confirmed is no longer the value on the Server,
        // so the confirmation is discarded and must be re-entered.
        setConfirmation('')
        setError(
          'This policy changed since this page read it (fingerprint ' +
            shortId(policy.policyVersion) +
            '). The Server refused the write, so nothing changed: the table above reloaded with the current ' +
            'value, and the edit must be entered and confirmed again against what is now shown.',
        )
      } else if (caught instanceof AdminApiError && caught.fields.includes('retentionDays')) {
        setFieldError(caught.message)
      } else if (indeterminateOutcome(caught)) {
        setError(UNKNOWN_POLICY_OUTCOME)
      } else {
        setError(errorMessage(caught, 'Unable to update the retention policy.'))
      }
    } finally {
      setSaving(false)
    }
  }

  return (
    <CardX
      role="article"
      data-slot="retention-edit"
      aria-labelledby="retention-edit-heading"
      bordered={false}
      className={CARD_SURFACE}
      header={
        <div className="flex min-w-0 flex-1 flex-wrap items-start justify-between gap-2">
          <div className="min-w-0">
            <h2 id="retention-edit-heading" className="text-sm font-medium">
              Edit {policy.label} retention
            </h2>
            <p className="mt-1 text-xs text-muted-foreground">
              Bounded by the Server&apos;s floor and ceiling for this family: {formatBounds(policy)}.
            </p>
          </div>
          <Button variant="outline" size="sm" className="min-h-11" onClick={onClose}>
            Close
          </Button>
        </div>
      }
    >
      <form id="retention-edit-form" className="grid gap-4" onSubmit={save} noValidate>
        <dl className="grid grid-cols-1 gap-2 sm:grid-cols-2">
          <DetailItem label="Retained now">{formatRetention(policy.retentionDays)}</DetailItem>
          <DetailItem label="Default">{formatRetention(policy.defaultDays)}</DetailItem>
          <DetailItem label="Allowed range">{formatBounds(policy)}</DetailItem>
          <DetailItem label="Policy version">
            <span className="font-mono text-xs">{shortId(policy.policyVersion)}</span>
          </DetailItem>
        </dl>

        <p className={CONSEQUENCE}>
          Shortening this bound does not delete anything by itself: it changes what a later retention run may
          release, and a run only executes the plan of the preview you compose and confirm. Lengthening cannot
          recover history that a previous run already released.
        </p>

        <div className={FIELD}>
          <label htmlFor="retention-days" className={LABEL}>
            New retention (days)
          </label>
          <Input
            id="retention-days"
            className="max-w-[10rem]"
            type="number"
            min={policy.maxDays === 0 ? undefined : policy.minDays}
            max={policy.maxDays === 0 ? undefined : policy.maxDays}
            step={1}
            value={days ?? ''}
            aria-invalid={Boolean(boundsError || fieldError)}
            aria-describedby={boundsError || fieldError ? 'retention-days-error' : undefined}
            onChange={(event) => {
              setDays(event.target.value === '' ? null : Number(event.target.value))
              setConfirmation('')
              setFieldError(null)
              setError(null)
              setNotice(null)
            }}
          />
          <small className="text-[11px] text-muted-foreground">
            0 means keep forever and is accepted only for a family the Server keeps forever.
          </small>
          {(boundsError || fieldError) && (
            <p id="retention-days-error" className="text-xs text-destructive" role="alert">
              {boundsError ?? fieldError}
            </p>
          )}
        </div>

        {days !== null && days >= 0 && (
          <div className={CONSEQUENCE} data-slot="retention-impact" aria-live="polite">
            <h3 className="text-sm font-medium">Impact estimate</h3>
            {impact.isFetching && <p className="text-xs text-muted-foreground">Estimating affected rows…</p>}
            {impact.isError && (
              <p className="text-xs text-destructive" role="alert">
                {impact.error instanceof AdminApiError && impact.error.code === 'retention_out_of_bounds'
                  ? 'The Server refuses this value: ' + impact.error.message
                  : 'Unable to preview the impact. Change the value to retry.'}
              </p>
            )}
            {impactReady && impact.data && (
              <p className="mt-1">
                {impact.data.estimatedRows === null || impact.data.estimatedRows === undefined
                  ? 'The Server reports no row estimate for this family.'
                  : 'About ' + String(impact.data.estimatedRows) + ' rows are older than this bound now.'}
                {impact.data.unsupported
                  ? ' The Server does not implement cleanup for this family, so a run releases nothing.'
                  : ' This is an estimate of what is currently older than the bound, not what a run would delete: the run executes its preview.'}
              </p>
            )}
          </div>
        )}

        <div className={FIELD}>
          <label htmlFor="retention-confirmation" className={LABEL}>
            Type the change to confirm
          </label>
          <Input
            id="retention-confirmation"
            className="max-w-[20rem]"
            value={confirmation}
            autoComplete="off"
            aria-invalid={confirmation.length > 0 && !confirmationMatches}
            onChange={(event) => setConfirmation(event.target.value)}
          />
          <small className="text-[11px] text-muted-foreground">
            Type <code className={INLINE_CODE}>{confirmationTarget}</code> to confirm.
          </small>
        </div>

        {notice && (
          <p className={NOTICE} role="status">
            {notice}
          </p>
        )}
        {error && (
          <p className={ERROR} role="alert">
            {error}
          </p>
        )}
        <Button type="submit" className="justify-self-start" disabled={!canSave} data-slot="retention-save">
          {saving ? 'Saving…' : 'Save retention'}
        </Button>
      </form>
    </CardX>
  )
}

/** The bound preview: what a run would execute, and when it stops binding. */
function PreviewPanel({
  preview,
  rejection,
  csrfToken,
  onComposed,
}: {
  preview: RetentionPreviewDto | null
  rejection: Rejection | null
  csrfToken: string
  onComposed: (preview: RetentionPreviewDto) => void
}) {
  const [composing, setComposing] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function compose() {
    setComposing(true)
    setError(null)
    try {
      // No scope arguments: the Server composes the preview over every enabled,
      // supported family and reports the scope it bound.
      const created = await createRetentionPreviewEntry(null, csrfToken)
      onComposed(created)
    } catch (caught) {
      setError(
        indeterminateOutcome(caught)
          ? UNKNOWN_PREVIEW_OUTCOME
          : errorMessage(caught, 'Unable to compose a retention preview.'),
      )
    } finally {
      setComposing(false)
    }
  }

  const expired = preview ? isExpired(preview) : false

  return (
    <CardX size="medium" className={CARD_SURFACE} title="Impact preview" data-slot="retention-preview">
      <div className="space-y-3">
        <p className="text-sm text-muted-foreground">
          A run is queued by preview id. The Server freezes the policy versions, the scope, and a per-family
          cutoff when the preview is composed, and re-checks them when the run is queued — so the plan the run
          executes is the one you reviewed here, and a preview that expired or no longer matches is refused
          instead of silently re-estimated.
        </p>

        {!preview && (
          <p className="text-sm">
            No preview is bound on this Server. Composing one never deletes anything and is not audited.
          </p>
        )}

        {preview && (
          <>
            <DetailList>
              <DetailItem label="Preview">
                <span className="font-mono text-xs">{shortId(preview.previewId)}</span>
              </DetailItem>
              <DetailItem label="Composed by">{preview.createdBy}</DetailItem>
              <DetailItem label="Composed at">{formatObservedAt(preview.createdAt)}</DetailItem>
              <DetailItem label="Expires at">
                {formatObservedAt(preview.expiresAt)}
                {expired && (
                  <span className="block text-xs text-destructive">
                    Expired: a run for this preview is refused.
                  </span>
                )}
              </DetailItem>
              <DetailItem label="Scope">
                {preview.scope
                  ? preview.scope.length === 0
                    ? 'No family'
                    : preview.scope.join(', ')
                  : 'Every enabled, supported family'}
              </DetailItem>
              <DetailItem label="Policy version">
                <span className="font-mono text-xs">{shortId(preview.policyVersion)}</span>
              </DetailItem>
              <DetailItem label="Estimated rows the plan may release">
                {String(preview.estimatedRows)} (Server estimate, an upper bound)
              </DetailItem>
            </DetailList>

            {expired && (
              <p className={ERROR} role="alert">
                This preview expired at {formatObservedAt(preview.expiresAt)}. The Server refuses a run for an
                expired preview; compose a new preview to run retention now.
              </p>
            )}

            {rejection && (
              <p className={ERROR} role="alert">
                {rejection.message}
              </p>
            )}

            {preview.families.length === 0 ? (
              <Empty description="The Server bound no family in this preview, so a run would release nothing." />
            ) : (
              <div className="overflow-x-auto">
                <table data-stack data-slot="retention-preview-families" className="w-full text-sm">
                  <caption className="sr-only">
                    Families bound by this preview, with the cutoff older rows must pass and the Server&apos;s row
                    estimate
                  </caption>
                  <thead>
                    <tr className="border-b">
                      <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                        Family
                      </th>
                      <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                        Retention
                      </th>
                      <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                        Cutoff
                      </th>
                      <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                        Estimated rows
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {preview.families.map((family) => (
                      <tr
                        key={family.family}
                        data-slot="retention-preview-family"
                        data-family={family.family}
                        className="border-b border-border/60 align-top"
                      >
                        <th scope="row" data-label="Family" className="min-w-0 px-3 py-3 text-left font-normal">
                          {family.family}
                        </th>
                        <td data-label="Retention" className="min-w-0 px-3 py-3">
                          {formatRetention(family.retentionDays)}
                        </td>
                        <td data-label="Cutoff" className="min-w-0 px-3 py-3">
                          {formatObservedAt(family.cutoff)}
                        </td>
                        <td data-label="Estimated rows" className="min-w-0 px-3 py-3">
                          {String(family.estimatedRows)}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}

            {preview.skipped.length > 0 && (
              <div className="space-y-2" data-slot="retention-preview-skipped">
                <h3 className="text-sm font-medium">Not acted on</h3>
                <ul className="space-y-2 text-sm">
                  {preview.skipped.map((skip) => (
                    <li key={skip.family} className="min-w-0 break-words">
                      <span className="font-mono text-xs">{skip.family}</span>
                      <span className="block text-muted-foreground">
                        {skip.code}: {skip.message}
                      </span>
                    </li>
                  ))}
                </ul>
              </div>
            )}

            {preview.notes.length > 0 && (
              <ul className="space-y-1 text-xs text-muted-foreground" data-slot="retention-preview-notes">
                {preview.notes.map((note, index) => (
                  <li key={note + String(index)} className="break-words">
                    {note}
                  </li>
                ))}
              </ul>
            )}
          </>
        )}

        {error && (
          <p className={ERROR} role="alert">
            {error}
          </p>
        )}
        <Button
          className="justify-self-start"
          disabled={composing || csrfToken.length === 0}
          onClick={() => void compose()}
          data-slot="retention-preview-compose"
        >
          {composing ? 'Composing…' : preview ? 'Compose a new preview' : 'Compose preview'}
        </Button>
      </div>
    </CardX>
  )
}

/** The run: queued for the bound preview, never re-estimated here. */
function RunPanel({
  preview,
  rejection,
  csrfToken,
  onRejected,
}: {
  preview: RetentionPreviewDto | null
  rejection: Rejection | null
  csrfToken: string
  onRejected: (message: string) => void
}) {
  const [running, setRunning] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  const boundPreviewId = preview ? preview.previewId : null
  // A refusal belongs to the preview it was refused for: binding a different
  // preview clears it, while a queued-run notice stays true whatever else
  // refetches.
  useEffect(() => {
    setError(null)
  }, [boundPreviewId])

  const expired = preview ? isExpired(preview) : false
  const canRun = Boolean(preview && !expired && !rejection && !running && csrfToken.length > 0)

  async function run() {
    if (!preview) return
    setRunning(true)
    setNotice(null)
    setError(null)
    try {
      const response = await runRetentionEntry(preview.previewId, csrfToken)
      const operationId = response.operation.operation.operationId
      setNotice(
        'The Server queued the retention run as ' +
          operationId +
          '. It starts Queued and releases only what the preview bound; the recorded task below shows its ' +
          'outcome.',
      )
    } catch (caught) {
      if (
        caught instanceof AdminApiError &&
        (caught.code === 'retention_preview_stale' || caught.code === 'retention_preview_not_found')
      ) {
        // The Server refused this preview; the page must not retry it. The
        // Operator composes a fresh preview and reviews the new estimates.
        setError(caught.message)
        onRejected(
          'The Server refused a run for this preview: ' +
            caught.message +
            ' Nothing was queued, and this page will not retry it — compose a new preview above and review ' +
            'the new estimates before running.',
        )
      } else {
        setError(
          indeterminateOutcome(caught) ? UNKNOWN_RUN_OUTCOME : errorMessage(caught, 'Unable to start the retention run.'),
        )
      }
    } finally {
      setRunning(false)
    }
  }

  return (
    <CardX size="medium" className={CARD_SURFACE} title="Run retention" data-slot="retention-run">
      <div className="space-y-3">
        {preview ? (
          <p className="text-sm">
            This would queue a run for preview{' '}
            <code className={INLINE_CODE}>{shortId(preview.previewId)}</code>: the Server executes the plan it
            froze, releases only rows older than each bound cutoff, never touches protected state, and never
            re-estimates. The run is recorded as a task you can follow.
          </p>
        ) : (
          <p className="text-sm">
            Compose a preview before running retention. With no preview bound there is nothing to run: the Server
            queues runs by preview id, never against a bare policy.
          </p>
        )}

        {notice && (
          <p className={NOTICE} role="status">
            {notice}
          </p>
        )}
        {error && (
          <p className={ERROR} role="alert">
            {error}
          </p>
        )}
        {rejection && !error && (
          <p className={ERROR} role="alert">
            {rejection.message}
          </p>
        )}

        <Button
          className="justify-self-start"
          disabled={!canRun}
          onClick={() => void run()}
          data-slot="retention-run-submit"
        >
          {running ? 'Queueing…' : 'Run retention for this preview'}
        </Button>
        <p className="text-xs text-muted-foreground">
          Nothing is queued while this button is unavailable: an expired preview, a preview this page already
          knows changed, and a missing preview are all reasons the Server would refuse the run.
        </p>
      </div>
    </CardX>
  )
}

/** State the Server never releases, whatever the policies say. */
function ProtectedStateCard({ states }: { states: string[] }) {
  return (
    <CardX size="medium" className={CARD_SURFACE} title="Protected state" data-slot="retention-protected-state">
      <div className="space-y-3">
        {states.length === 0 ? (
          <p className="text-sm text-muted-foreground">
            The Server reports no protected state for retention. That is a statement about this Server, not a
            promise that nothing is protected.
          </p>
        ) : (
          <>
            <ul className="space-y-1 text-sm">
              {states.map((state, index) => (
                <li key={state + String(index)} className="min-w-0 break-words">
                  {state}
                </li>
              ))}
            </ul>
            <p className="text-xs text-muted-foreground">
              A retention run never releases these records, whatever bound you set and whatever cutoff a preview
              binds. They are released only by the explicit operation that owns them.
            </p>
          </>
        )}
      </div>
    </CardX>
  )
}

/** The last retention run the Server recorded, with its Operation link. */
function LastRunCard({ lastRun }: { lastRun: OperationSummary | null }) {
  return (
    <CardX size="medium" className={CARD_SURFACE} title="Last recorded retention run" data-slot="retention-last-run">
      <div className="space-y-3">
        {lastRun ? (
          <>
            <DetailList>
              <DetailItem label="Task">
                <Link className={LINK} to={'/admin/operations/' + lastRun.operationId}>
                  {shortId(lastRun.operationId)}
                </Link>
              </DetailItem>
              <DetailItem label="Status">
                <OperationStatus operation={lastRun} />
              </DetailItem>
              <DetailItem label="Run age">{recordedAge(lastRun.finishedAt ?? lastRun.createdAt)}</DetailItem>
              <DetailItem label="Recorded at">
                {formatObservedAt(lastRun.finishedAt ?? lastRun.createdAt)}
              </DetailItem>
            </DetailList>
            <OperationProgress operation={lastRun} />
            <p className="text-sm text-muted-foreground">
              This is the last retention run the Server recorded. A run releases only the rows its preview bound;
              the list below names the state it never releases.
            </p>
          </>
        ) : (
          <p className="text-sm text-muted-foreground">
            This Server has recorded no retention run yet. Running retention records the first one, and nothing is
            released before a preview is composed and confirmed.
          </p>
        )}
      </div>
    </CardX>
  )
}
