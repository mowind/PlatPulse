import { useEffect, useMemo, useState } from 'react'
import { Link, useNavigate, useSearchParams } from 'react-router'
import {
  AdminApiError,
  updateRetentionPolicyEntry,
  useAdminRetention,
  useRetentionImpact,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'

/**
 * PAGE-ADMIN-RETENTION-EDIT (webui.md §4.5/§8.4): safety-bounded retention
 * mutation. The form shows the family's fixed bounds and a read-only
 * impact preview computed by the Server, then requires typed confirmation
 * before the audited update. No optimistic state.
 */

export default function AdminRetentionEdit() {
  const [searchParams] = useSearchParams()
  const family = searchParams.get('family') ?? ''
  const navigate = useNavigate()
  const { generation, status } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const overview = useAdminRetention(generation)
  const policy = overview.data?.policies.find((entry) => entry.family === family)

  const [days, setDays] = useState<number | null>(null)
  useEffect(() => {
    if (policy) setDays((current) => current ?? policy.retentionDays)
  }, [policy])

  const [confirmation, setConfirmation] = useState('')
  const [saving, setSaving] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  const impact = useRetentionImpact(generation, family, days ?? 0, csrfToken)
  const confirmationTarget = useMemo(
    () => (policy ? `${policy.family} ${days ?? ''}`.trim() : ''),
    [policy, days],
  )
  const confirmationMatches = confirmation.trim() === confirmationTarget

  const boundsError = policy ? validateAgainstBounds(policy, days) : null

  async function onSave(event: React.FormEvent) {
    event.preventDefault()
    if (!policy || boundsError || !confirmationMatches || saving) return
    setSaving(true)
    setError(null)
    setNotice(null)
    try {
      const response = await updateRetentionPolicyEntry(policy.family, days ?? 0, csrfToken)
      setNotice(
        `${response.policy.label} now retains ${response.policy.retentionDays} days (Audit #${response.auditEventId}).`,
      )
    } catch (caught) {
      setError(
        caught instanceof AdminApiError
          ? caught.message
          : 'Unable to update the retention policy.',
      )
    } finally {
      setSaving(false)
    }
  }

  if (!policy) {
    return (
      <section className="page">
        <p>
          <Link to="/admin/data/retention">← Retention</Link>
        </p>
        <h1>Edit retention policy</h1>
        {overview.isError ? (
          <p className="form-error" role="alert">
            Unable to load retention policies.
          </p>
        ) : (
          <p className="muted">Unknown retention family.</p>
        )}
      </section>
    )
  }

  return (
    <section className="page">
      <p>
        <Link to="/admin/data/retention">← Retention</Link>
      </p>
      <h1>Edit {policy.label}</h1>
      <p className="muted">
        Safety bounds:{' '}
        {policy.maxDays === 0
          ? policy.minDays === 0
            ? 'this long-term family can only be kept forever (0 days)'
            : `at least ${policy.minDays} days (0 keeps forever)`
          : `${policy.minDays}–${policy.maxDays} days`}
        . Aggregate families are not produced in this phase and cannot be changed.
      </p>

      <form className="retention-form" onSubmit={onSave} noValidate>
        <div className="field">
          <label htmlFor="retention-days">Retention (days)</label>
          <input
            id="retention-days"
            type="number"
            min={policy.minDays}
            max={policy.maxDays || undefined}
            step={1}
            value={days ?? ''}
            disabled={!policy.supported}
            aria-invalid={Boolean(boundsError)}
            aria-describedby={boundsError ? 'retention-days-error' : undefined}
            onChange={(event) => {
              setDays(event.target.value === '' ? null : Number(event.target.value))
              setConfirmation('')
            }}
          />
          {boundsError && (
            <p id="retention-days-error" className="field-error" role="alert">
              {boundsError}
            </p>
          )}
        </div>

        {policy.supported && days !== null && (
          <div className="impact-preview" aria-live="polite">
            <h2>Impact preview</h2>
            {impact.isFetching && <p className="muted">Estimating…</p>}
            {impact.isError && (
              <p className="form-error" role="alert">
                Impact preview unavailable — the policy cannot be saved until the preview
                succeeds. Change the value to retry.
              </p>
            )}
            {impact.isSuccess && impact.data && (
              <p>
                {impact.data.unsupported ? (
                  <span>This family is unsupported; nothing can be removed.</span>
                ) : (
                  <span>
                    About{' '}
                    <strong>
                      {impact.data.estimatedRows ?? 0} row
                      {impact.data.estimatedRows === 1 ? '' : 's'}
                    </strong>{' '}
                    would be removed at {impact.data.retentionDays} days — no Incidents,
                    coverage, gap, counter, or Audit-link state is touched.
                  </span>
                )}
              </p>
            )}
          </div>
        )}

        <div className="field">
          <label htmlFor="confirmation">Type the family and value to confirm</label>
          <input
            id="confirmation"
            type="text"
            value={confirmation}
            autoComplete="off"
            aria-invalid={confirmation.length > 0 && !confirmationMatches}
            onChange={(event) => setConfirmation(event.target.value)}
          />
          <small className="muted">
            Type <code>{confirmationTarget}</code> to confirm this audited change.
          </small>
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
        <div className="page-actions">
          <button
            type="submit"
            className="primary-action"
            disabled={
              Boolean(boundsError) ||
              !confirmationMatches ||
              saving ||
              !policy.supported ||
              // Typed confirmation happens only after a valid impact
              // preview (webui.md §8.4); a failed preview blocks the save.
              impact.isError
            }
          >
            {saving ? 'Saving…' : 'Save policy'}
          </button>
          <button type="button" onClick={() => navigate('/admin/data/retention')}>
            Cancel
          </button>
        </div>
      </form>
    </section>
  )
}

function validateAgainstBounds(
  policy: { minDays: number; maxDays: number },
  days: number | null,
): string | null {
  if (days === null) return 'Enter a number of days.'
  if (days < 0) return 'Retention days must be zero or positive.'
  if (policy.maxDays === 0) {
    if (policy.minDays === 0) {
      return days === 0 ? null : 'This long-term family can only be kept forever (0 days).'
    }
    if (days !== 0 && days < policy.minDays) {
      return `Cannot be lowered below ${policy.minDays} days (safety floor).`
    }
    return null
  }
  if (days < policy.minDays || days > policy.maxDays) {
    return `Must be between ${policy.minDays} and ${policy.maxDays} days.`
  }
  return null
}
