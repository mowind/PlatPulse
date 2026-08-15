import { useState, type FormEvent, type ReactNode } from 'react'
import { Link, useNavigate, useParams } from 'react-router'
import {
  AdminApiError,
  previewAlertRuleEntry,
  updateAlertRuleEntry,
  useAdminAlertRuleDetail,
  useAdminAlertRules,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { StatusBadge } from '../components/StatusBadge'
import type { AlertRuleDetail, AlertRuleSummary, RuleCondition, RulePreviewSubject } from '../api/generated'

/**
 * PAGE-ADMIN-ALERT-RULES / PAGE-ADMIN-ALERT-RULE / PAGE-ADMIN-ALERT-RULE-EDIT
 * (design §4.4, webui.md §8.3): the typed, Server-owned Rule catalog.
 * Rules have no scripts, SQL, or DSL input — the editor renders the Server
 * schema (duration + optional typed threshold). Edits create immutable
 * versions; the preview evaluates current facts without creating Incidents
 * or Notifications. Evaluation state (Normal/Pending/Firing/Recovering,
 * Evaluation unavailable, Disabled) is rendered independently from Incident
 * state (Open/Resolved).
 */

const STATE_TONE: Record<string, 'ok' | 'warning' | 'error' | 'neutral'> = {
  normal: 'ok',
  pending: 'warning',
  firing: 'error',
  recovering: 'warning',
}

export function evaluationStateLabel(state: string | null | undefined): string {
  switch (state) {
    case 'normal':
      return 'Normal'
    case 'pending':
      return 'Pending'
    case 'firing':
      return 'Firing'
    case 'recovering':
      return 'Recovering'
    default:
      return 'Unknown'
  }
}

export function ruleStateBadge(state: string | undefined): ReactNode {
  const label = evaluationStateLabel(state)
  return <StatusBadge status={label} tone={STATE_TONE[state ?? ''] ?? 'neutral'} />
}

function severityLabel(severity: string | undefined): string {
  switch (severity) {
    case 'info':
      return 'Info'
    case 'warning':
      return 'Warning'
    case 'critical':
      return 'Critical'
    default:
      return severity ?? 'Unknown'
  }
}

export default function AdminAlertRulesList() {
  const { generation } = useAuth()
  const query = useAdminAlertRules(generation)

  return (
    <section className="page">
      <h1>Alert Rules</h1>
      <p className="muted">
        Rules are typed and Server-owned: each one compares an accepted projection fact against
        a fixed threshold. Evaluation state and Incident state stay independent — Unknown or
        Stale input never resolves an Open Incident.
      </p>
      {!query.data && query.isPending && (
        <p className="panel-state" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading the Alert Rules…
        </p>
      )}
      {!query.data && query.isError && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Error" tone="error" />{' '}
          {query.error instanceof Error ? query.error.message : 'Unable to load Alert Rules'}
          <button type="button" className="text-action" onClick={() => void query.refetch()}>
            Try again
          </button>
        </p>
      )}
      {query.data && query.data.length === 0 && (
        <p className="panel-state">
          <StatusBadge status="Empty" tone="ok" /> No Rules registered.
        </p>
      )}
      {query.data && query.data.length > 0 && (
        <div className="table-wrap">
          <table className="node-table">
            <caption className="sr-only">
              Typed Alert Rules with severity, evaluation, and Open Incident counts
            </caption>
            <thead>
              <tr>
                <th scope="col">Rule</th>
                <th scope="col">Subject</th>
                <th scope="col">Severity</th>
                <th scope="col">Evaluation</th>
                <th scope="col">Open</th>
              </tr>
            </thead>
            <tbody>
              {query.data.map((rule) => (
                <RuleRow key={rule.ruleKey} rule={rule} />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  )
}

function RuleRow({ rule }: { rule: AlertRuleSummary }) {
  return (
    <tr>
      <td data-label="Rule">
        <Link to={`/admin/alerts/rules/${encodeURIComponent(rule.ruleKey)}`}>
          {rule.ruleKey}
        </Link>
        {!rule.enabled && (
          <p className="muted">
            <StatusBadge status="Disabled" tone="neutral" /> Evaluation stopped
          </p>
        )}
      </td>
      <td data-label="Subject">{rule.subjectKind}</td>
      <td data-label="Severity">{severityLabel(rule.severity)}</td>
      <td data-label="Evaluation">
        {rule.enabled ? (
          <>
            {ruleStateBadge(rule.evaluation.firing > 0 ? 'firing' : rule.evaluation.pending > 0 ? 'pending' : rule.evaluation.recovering > 0 ? 'recovering' : 'normal')}{' '}
            <small className="muted">
              {rule.evaluation.subjects} subject{rule.evaluation.subjects === 1 ? '' : 's'}
              {rule.evaluation.evaluationUnavailable > 0 &&
                ` · ${rule.evaluation.evaluationUnavailable} unavailable`}
            </small>
          </>
        ) : (
          <StatusBadge status="Disabled" tone="neutral" />
        )}
      </td>
      <td data-label="Open">
        <StatusBadge
          status={String(rule.openIncidents)}
          tone={rule.openIncidents > 0 ? 'error' : 'ok'}
        />
      </td>
    </tr>
  )
}

/** PAGE-ADMIN-ALERT-RULE: versions, overrides, per-subject evaluation, and
 * the typed edit entry. */
export function AdminAlertRuleDetail() {
  const { ruleKey = '' } = useParams()
  const { generation } = useAuth()
  const query = useAdminAlertRuleDetail(generation, ruleKey)

  return (
    <section className="page">
      <p>
        <Link to="/admin/alerts/rules" className="text-action">
          ← Alert Rules
        </Link>
      </p>
      <h1>{ruleKey}</h1>
      {!query.data && query.isPending && (
        <p className="panel-state" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading the Rule…
        </p>
      )}
      {!query.data && query.isError && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Error" tone="error" />{' '}
          {query.error instanceof Error ? query.error.message : 'Unable to load the Rule'}
        </p>
      )}
      {query.data && (
        <>
          <div className="page-actions">
            <Link
              to={`/admin/alerts/rules/${encodeURIComponent(ruleKey)}/edit`}
              className="primary-action"
            >
              Edit Rule
            </Link>
          </div>
          <dl className="detail-list">
            <dt>Subject</dt>
            <dd>{query.data.subjectKind}</dd>
            <dt>Severity</dt>
            <dd>{severityLabel(query.data.severity)}</dd>
            <dt>Status</dt>
            <dd>
              {query.data.enabled ? (
                ruleStateBadge(
                  query.data.states.some((s) => s.state === 'firing')
                    ? 'firing'
                    : query.data.states.some((s) => s.state === 'pending')
                      ? 'pending'
                      : query.data.states.some((s) => s.state === 'recovering')
                        ? 'recovering'
                        : 'normal',
                )
              ) : (
                <StatusBadge status="Disabled" tone="neutral" />
              )}
            </dd>
            <dt>Open Incidents</dt>
            <dd>{query.data.openIncidents}</dd>
            <dt>Version</dt>
            <dd>{query.data.version}</dd>
            <dt>Condition</dt>
            <dd>
              <ConditionSummary condition={query.data.condition} />
            </dd>
          </dl>

          <h2>Evaluation state</h2>
          {query.data.states.length === 0 && (
            <p className="panel-state">
              <StatusBadge status="Empty" tone="ok" /> No subjects evaluated yet.
            </p>
          )}
          {query.data.states.length > 0 && (
            <div className="table-wrap">
              <table className="node-table">
                <caption className="sr-only">Per-subject evaluation state</caption>
                <thead>
                  <tr>
                    <th scope="col">Subject</th>
                    <th scope="col">State</th>
                    <th scope="col">Input</th>
                    <th scope="col">Open</th>
                  </tr>
                </thead>
                <tbody>
                  {query.data.states.map((state) => (
                    <tr key={state.subjectKey}>
                      <td data-label="Subject">{state.subjectKey}</td>
                      <td data-label="State">
                        {ruleStateBadge(state.state)}
                        {state.evaluationUnavailable && (
                          <p className="muted">
                            <StatusBadge status="Evaluation unavailable" tone="warning" />
                          </p>
                        )}
                      </td>
                      <td data-label="Input">
                        <InputSummary state={state} />
                      </td>
                      <td data-label="Open">
                        <StatusBadge
                          status={String(state.openIncidents)}
                          tone={state.openIncidents > 0 ? 'error' : 'ok'}
                        />
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}

          <h2>Versions</h2>
          <div className="table-wrap">
            <table className="node-table">
              <caption className="sr-only">Immutable Rule versions</caption>
              <thead>
                <tr>
                  <th scope="col">Version</th>
                  <th scope="col">Severity</th>
                  <th scope="col">Condition</th>
                  <th scope="col">Created</th>
                </tr>
              </thead>
              <tbody>
                {query.data.versions.map((version) => (
                  <tr key={version.version}>
                    <td data-label="Version">{version.version}</td>
                    <td data-label="Severity">{severityLabel(version.severity)}</td>
                    <td data-label="Condition">
                      <ConditionSummary condition={version.condition} />
                    </td>
                    <td data-label="Created">{formatTs(version.createdAt)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          <h2>Network and Node overrides</h2>
          {query.data.overrides.length === 0 && (
            <p className="panel-state">
              <StatusBadge status="Empty" tone="ok" /> No overrides; every subject uses the
              global Rule parameters.
            </p>
          )}
          {query.data.overrides.length > 0 && (
            <div className="table-wrap">
              <table className="node-table">
                <caption className="sr-only">Rule overrides</caption>
                <thead>
                  <tr>
                    <th scope="col">Scope</th>
                    <th scope="col">Value</th>
                    <th scope="col">Enabled</th>
                    <th scope="col">Severity</th>
                    <th scope="col">Condition</th>
                  </tr>
                </thead>
                <tbody>
                  {query.data.overrides.map((override) => (
                    <tr key={`${override.scopeKind}:${override.scopeValue}`}>
                      <td data-label="Scope">{override.scopeKind}</td>
                      <td data-label="Value">{override.scopeValue}</td>
                      <td data-label="Enabled">
                        {override.enabled == null ? (
                          'inherit'
                        ) : override.enabled ? (
                          'enabled'
                        ) : (
                          <StatusBadge status="Disabled" tone="neutral" />
                        )}
                      </td>
                      <td data-label="Severity">
                        {override.severity == null ? 'inherit' : severityLabel(override.severity)}
                      </td>
                      <td data-label="Condition">
                        {override.condition == null ? (
                          'inherit'
                        ) : (
                          <ConditionSummary condition={override.condition} />
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </>
      )}
    </section>
  )
}

function ConditionSummary({ condition }: { condition: RuleCondition }) {
  const parts = [`for ${condition.for_secs}s`, `recover ${condition.recovery_for_secs}s`]
  if (condition.threshold != null) {
    parts.push(`threshold ${condition.threshold}`)
  }
  return <span>{parts.join(' · ')}</span>
}

function InputSummary({
  state,
}: {
  state: {
    inputKind: string
    inputValue?: number | null
    inputDetail?: string | null
  }
}) {
  switch (state.inputKind) {
    case 'known':
      return (
        <span>
          Known ({state.inputValue}) — {state.inputDetail ?? ''}
        </span>
      )
    case 'stale':
      return (
        <span>
          <StatusBadge status="Stale" tone="warning" /> {state.inputValue} —{' '}
          {state.inputDetail ?? ''}
        </span>
      )
    case 'unknown':
      return (
        <span>
          <StatusBadge status="Unknown" tone="neutral" /> {state.inputDetail ?? 'Unknown'}
        </span>
      )
    case 'unsupported':
      return (
        <span>
          <StatusBadge status="Unsupported" tone="warning" /> {state.inputDetail ?? ''}
        </span>
      )
    default:
      return (
        <span>
          <StatusBadge status="Unknown" tone="neutral" />{' '}
          {state.inputDetail ?? 'Never observed'}
        </span>
      )
  }
}

/** PAGE-ADMIN-ALERT-RULE-EDIT: the typed editor. The form renders the
 * Server schema, validates per field (webui.md §0.3: field-level messages
 * plus a page summary), and offers a read-only preview that never creates
 * Incidents or Notifications. */
export function AdminAlertRuleEdit() {
  const { ruleKey = '' } = useParams()
  const { generation } = useAuth()
  const query = useAdminAlertRuleDetail(generation, ruleKey)

  return (
    <section className="page">
      <p>
        <Link
          to={`/admin/alerts/rules/${encodeURIComponent(ruleKey)}`}
          className="text-action"
        >
          ← {ruleKey}
        </Link>
      </p>
      <h1>Edit {ruleKey}</h1>
      {query.isError && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Error" tone="error" />{' '}
          {query.error instanceof Error ? query.error.message : 'Unable to load the Rule'}
          <button type="button" className="text-action" onClick={() => void query.refetch()}>
            Try again
          </button>
        </p>
      )}
      {!query.data && query.isPending && (
        <p className="panel-state" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading the Rule…
        </p>
      )}
      {query.data && <RuleEditForm rule={query.data} ruleKey={ruleKey} />}
    </section>
  )
}

/** The form is a child of the loaded Rule: initial values are derived once
 * from props (never during a render of the parent), and the Server schema
 * drives the fields. */
function RuleEditForm({ rule, ruleKey }: { rule: AlertRuleDetail; ruleKey: string }) {
  const { status } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const navigate = useNavigate()
  const [enabled, setEnabled] = useState(rule.enabled)
  const [severity, setSeverity] = useState(rule.severity)
  const [values, setValues] = useState<Record<string, string>>(() => ({
    for_secs: String(rule.condition.for_secs),
    recovery_for_secs: String(rule.condition.recovery_for_secs),
    ...(rule.condition.threshold != null
      ? { threshold: String(rule.condition.threshold) }
      : {}),
  }))
  const [fieldErrors, setFieldErrors] = useState<Record<string, string>>({})
  const [notice, setNotice] = useState<string | null>(null)
  const [preview, setPreview] = useState<RulePreviewSubject[] | null>(null)
  const [previewNote, setPreviewNote] = useState<string | null>(null)
  const [previewing, setPreviewing] = useState(false)
  const [saving, setSaving] = useState(false)

  const buildCondition = (): RuleCondition | null => {
    const errors: Record<string, string> = {}
    const forSecs = Number(values['for_secs'])
    if (!Number.isFinite(forSecs) || forSecs < 1) {
      errors['for_secs'] = 'Sustained firing must be at least 1 second.'
    }
    const recoveryForSecs = Number(values['recovery_for_secs'])
    if (!Number.isFinite(recoveryForSecs) || recoveryForSecs < 1) {
      errors['recovery_for_secs'] = 'Sustained recovery must be at least 1 second.'
    }
    const condition: RuleCondition = {
      for_secs: forSecs,
      recovery_for_secs: recoveryForSecs,
    }
    const thresholdParam = rule.schema.find((param) => param.key === 'threshold')
    if (thresholdParam) {
      const threshold = Number(values['threshold'])
      if (
        !Number.isFinite(threshold) ||
        threshold < thresholdParam.min ||
        threshold > thresholdParam.max
      ) {
        errors['threshold'] = `Threshold must be between ${thresholdParam.min} and ${thresholdParam.max} ${thresholdParam.unit}.`
      } else {
        condition.threshold = threshold
      }
    }
    setFieldErrors(errors)
    return Object.keys(errors).length === 0 ? condition : null
  }

  const onPreview = async () => {
    const condition = buildCondition()
    if (!condition) return
    setPreviewing(true)
    setPreviewNote(null)
    try {
      const response = await previewAlertRuleEntry(
        ruleKey,
        { enabled, severity, condition },
        csrfToken,
      )
      setPreview(response.subjects)
      setPreviewNote(
        `${response.subjects.length} subject${
          response.subjects.length === 1 ? '' : 's'
        } evaluated — nothing was written.`,
      )
    } catch (error) {
      setPreviewNote(
        error instanceof AdminApiError ? error.message : 'Unable to preview the Rule.',
      )
    } finally {
      setPreviewing(false)
    }
  }

  const onSubmit = async (event: FormEvent) => {
    event.preventDefault()
    const condition = buildCondition()
    if (!condition) return
    setSaving(true)
    try {
      await updateAlertRuleEntry(ruleKey, { enabled, severity, condition }, csrfToken)
      setNotice(`Rule ${ruleKey} updated (version ${rule.version + 1}).`)
      navigate(`/admin/alerts/rules/${encodeURIComponent(ruleKey)}`)
    } catch (error) {
      setFieldErrors({
        _summary:
          error instanceof AdminApiError ? error.message : 'Unable to update the Alert Rule.',
      })
      setSaving(false)
    }
  }

  const setParam = (key: string, value: string) => {
    setValues((current) => ({ ...current, [key]: value }))
    setFieldErrors((current) => {
      if (!(key in current)) return current
      const next = { ...current }
      delete next[key]
      return next
    })
  }

  const summary = fieldErrors['_summary']

  return (
    <>
      <p className="muted">
        {rule.subjectKind} rule — the editor is generated from the Server schema; there is no
        script, SQL, or DSL input. Saving creates an immutable new version.
      </p>
      {notice && (
        <p className="form-success" role="status">
          {notice}
        </p>
      )}
      {(summary || Object.keys(fieldErrors).length > 0) && (
        <ul className="form-error-list" role="alert" aria-label="Validation errors">
          {summary && <li>{summary}</li>}
          {Object.entries(fieldErrors)
            .filter(([key]) => key !== '_summary')
            .map(([key, message]) => (
              <li key={key}>{message}</li>
            ))}
        </ul>
      )}
      <form className="page-form" onSubmit={onSubmit} noValidate>
        <div className="field">
          <label htmlFor="rule-enabled">Enabled</label>
          <select
            id="rule-enabled"
            value={enabled ? 'enabled' : 'disabled'}
            onChange={(event) => setEnabled(event.target.value === 'enabled')}
          >
            <option value="enabled">Enabled</option>
            <option value="disabled">Disabled</option>
          </select>
          <p className="muted">
            Disabling stops new evaluation without deleting Incident history; re-enabling
            evaluates current facts without fabricating historical Incidents.
          </p>
        </div>
        <div className="field">
          <label htmlFor="rule-severity">Severity</label>
          <select
            id="rule-severity"
            value={severity}
            onChange={(event) => setSeverity(event.target.value)}
          >
            <option value="info">Info</option>
            <option value="warning">Warning</option>
            <option value="critical">Critical</option>
          </select>
        </div>
        {rule.schema.map((param) => {
          const error = fieldErrors[param.key]
          const describedBy = error ? `error-${param.key}` : undefined
          return (
            <div className="field" key={param.key}>
              <label htmlFor={`param-${param.key}`}>
                {param.label} ({param.unit})
              </label>
              <input
                id={`param-${param.key}`}
                type="number"
                min={param.min}
                max={param.max}
                step="any"
                value={values[param.key] ?? ''}
                aria-invalid={error ? true : undefined}
                aria-describedby={describedBy}
                onChange={(event) => setParam(param.key, event.target.value)}
              />
              {error && (
                <p className="field-error" id={`error-${param.key}`} role="alert">
                  {error}
                </p>
              )}
              <p className="muted">{param.description}</p>
            </div>
          )
        })}
        <div className="page-actions">
          <button
            type="button"
            className="secondary-action"
            onClick={() => void onPreview()}
            disabled={previewing}
          >
            {previewing ? 'Previewing…' : 'Preview current facts'}
          </button>
          <button type="submit" className="primary-action" disabled={saving}>
            {saving ? 'Saving…' : 'Save version'}
          </button>
        </div>
      </form>
      {previewNote && (
        <p className="panel-state" role="status">
          {previewNote}
        </p>
      )}
      {preview && preview.length > 0 && (
        <div className="table-wrap">
          <table className="node-table">
            <caption className="sr-only">
              Preview: projected evaluation per subject without writing
            </caption>
            <thead>
              <tr>
                <th scope="col">Subject</th>
                <th scope="col">Current</th>
                <th scope="col">Input</th>
                <th scope="col">Would fire</th>
                <th scope="col">Projected</th>
              </tr>
            </thead>
            <tbody>
              {preview.map((subject) => (
                <tr key={subject.subjectKey}>
                  <td data-label="Subject">{subject.subjectKey}</td>
                  <td data-label="Current">{ruleStateBadge(subject.currentState)}</td>
                  <td data-label="Input">
                    {subject.input.kind} — {subject.input.detail}
                  </td>
                  <td data-label="Would fire">
                    {subject.wouldFire ? (
                      <StatusBadge status="Firing" tone="error" />
                    ) : (
                      <StatusBadge status="Normal" tone="ok" />
                    )}
                  </td>
                  <td data-label="Projected">{ruleStateBadge(subject.projectedState)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </>
  )
}

export function formatTs(timestamp: string | null | undefined): string {
  if (!timestamp) return 'Never'
  return `${timestamp.slice(0, 19).replace('T', ' ')} UTC`
}
