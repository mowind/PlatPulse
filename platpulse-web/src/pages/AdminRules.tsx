import { useEffect, useRef, useState, type FormEvent } from 'react'
import { Link, useParams } from 'react-router'
import { RefreshCw } from 'lucide-react'
import {
  AdminApiError,
  deleteRuleOverrideEntry,
  previewAlertRuleEntry,
  updateAlertRuleEntry,
  upsertRuleOverrideEntry,
  useAdminAlertRuleDetail,
  useAdminAlertRules,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { Alert, AlertDescription, AlertTitle } from '../components/ui/alert'
import { Button } from '../components/ui/button'
import { CardX } from '../components/ui/card-x'
import { Empty } from '../components/ui/empty'
import { Checkbox, Input, Select } from '../components/ui/input'
import { StatusBadge, formatObservedAt } from '../components/StatusBadge'
import { cn } from '../lib/utils'
import { SURFACE_CARD_STATIC, SURFACE_TOOLBAR } from '../lib/surface'
import { severityLabel, severityTone } from '../lib/severity'
import type {
  AlertRuleDetail,
  AlertRuleSummary,
  RuleCondition,
  RuleOverrideDto,
  RulePreviewSubject,
  RuleStateDto,
} from '../api/generated'

/**
 * PAGE-ADMIN-RULES and PAGE-ADMIN-RULE-DETAIL (parent #202, issue #204;
 * webui.md §15.7). Owners read the typed catalog, its effective inherited
 * configuration, and each Network/Node override's resolved fields; they save
 * typed edits and confirm previews before saving. Every save carries the
 * composed version the Owner read. A mismatch is rejected by the Server and
 * this surface refuses to overwrite: it asks the Owner to reload the current
 * configuration and review again. Preview is never a save and never promises
 * an Incident. Editing or disabling a Rule never rewrites an Incident's
 * opening rule version or evidence, and never clears an acknowledgment.
 */

const CARD_SURFACE = cn('rounded-md border-none', SURFACE_CARD_STATIC)

const SEVERITIES = ['info', 'warning', 'critical'] as const

/** The full inherited configuration the Server resolves for a subject: the
 * baseline, then a Network override, then a Node override, most specific
 * winning, with unset override fields inheriting from the layer below. */
const INHERITANCE_NOTE =
  'The baseline applies to every subject. For Node subjects, the Network override is applied first and the Node override second; unset override fields inherit the layer below, and the most specific value wins.'

/** A version conflict is the one Server error this surface must interpret as
 * "reload and review", not as a generic failure. */
function isVersionConflict(error: unknown): boolean {
  return error instanceof AdminApiError && error.code === 'alert_rule_version_conflict'
}

function errorMessage(error: unknown, fallback: string): string {
  return error instanceof Error && error.message.length > 0 ? error.message : fallback
}

type FormFeedback = { tone: 'ok' | 'error'; message: string }

/** One rendering of a save or preview result for every Rule form, so the forms
 * cannot drift on how a success, a rejection, or a conflict is announced
 * (issue #204 review, Standards #4). */
function FormFeedbackNote({ feedback }: { feedback: FormFeedback | null }) {
  if (feedback === null) return null
  return (
    <p
      role={feedback.tone === 'error' ? 'alert' : 'status'}
      className={cn(
        'rounded-md border p-3 text-sm',
        feedback.tone === 'ok' && 'border-border/60 bg-muted/30',
        feedback.tone === 'error' && 'border-destructive/40 bg-destructive/5 text-destructive',
      )}
    >
      {feedback.message}
    </p>
  )
}

function conditionSummary(condition: RuleCondition): string {
  const parts = ['for ' + condition.for_secs + 's', 'recovery ' + condition.recovery_for_secs + 's']
  if (condition.threshold !== undefined && condition.threshold !== null) {
    parts.push('threshold ' + condition.threshold)
  }
  return parts.join(' · ')
}

/** A missing override field means "inherit", shown explicitly so an Owner can
 * never mistake an unset field for a defaulted one. */
function InheritedOrValue({ value }: { value: string | null | undefined }) {
  if (value === null || value === undefined) {
    return <span className="text-muted-foreground italic">Inherited</span>
  }
  return <span>{value}</span>
}

interface RuleDraft {
  enabled: boolean
  severity: string
  forSecs: string
  recoveryForSecs: string
  threshold: string
  // The composed version the Owner read when the form was loaded. Saving
  // carries THIS version, not the live query's, so a background refetch from
  // another Owner's save cannot silently bless a stale draft.
  expectedVersion: number
}

function draftFromRule(rule: AlertRuleDetail): RuleDraft {
  return {
    enabled: rule.enabled,
    severity: rule.severity,
    forSecs: String(rule.condition.for_secs),
    recoveryForSecs: String(rule.condition.recovery_for_secs),
    threshold:
      rule.condition.threshold === undefined || rule.condition.threshold === null
        ? ''
        : String(rule.condition.threshold),
    expectedVersion: rule.version,
  }
}

function hasThresholdParam(rule: AlertRuleDetail): boolean {
  return rule.schema.some((parameter) => parameter.key === 'threshold')
}

/** One condition builder for both the baseline form and the override form, so
 * the two surfaces cannot drift on whether an absent threshold is omitted or
 * explicitly nulled: a Rule whose schema declares a threshold keeps the key. */
function conditionFromParts(
  rule: AlertRuleDetail,
  parts: { forSecs: string; recoveryForSecs: string; threshold: string },
): RuleCondition {
  const base: RuleCondition = {
    for_secs: Number(parts.forSecs),
    recovery_for_secs: Number(parts.recoveryForSecs),
  }
  if (!hasThresholdParam(rule)) return base
  if (parts.threshold.trim() === '') return { ...base, threshold: null }
  return { ...base, threshold: Number(parts.threshold) }
}

function conditionFromDraft(rule: AlertRuleDetail, draft: RuleDraft): RuleCondition {
  return conditionFromParts(rule, {
    forSecs: draft.forSecs,
    recoveryForSecs: draft.recoveryForSecs,
    threshold: draft.threshold,
  })
}

/** PAGE-ADMIN-RULES: the typed Rule catalog with each Rule's baseline version
 * and a link into its inherited configuration and overrides. */
export default function AdminRulesList() {
  const { generation } = useAuth()
  const query = useAdminAlertRules(generation)
  const rules: AlertRuleSummary[] = query.data ?? []

  return (
    <section className="w-full min-w-0 space-y-4">
      <div className="space-y-1">
        <h1 className="text-lg font-semibold break-words">Alert Rules</h1>
        <p className="text-sm text-muted-foreground">
          The typed Rule catalog. Open a Rule to read its effective inherited configuration and its
          Network/Node overrides, preview a change, or save a typed edit under version safety.
        </p>
      </div>

      {query.isPending && (
        <p className="flex items-center gap-2 text-sm text-muted-foreground" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading the Alert Rules…
        </p>
      )}

      {query.isError && !query.data && (
        <Alert variant="destructive">
          <AlertTitle>Unable to load the Alert Rules</AlertTitle>
          <AlertDescription className="flex flex-wrap items-center gap-3">
            <span>{errorMessage(query.error, 'The Admin API is unavailable.')}</span>
            <Button variant="outline" size="sm" onClick={() => void query.refetch()}>
              Try again
            </Button>
          </AlertDescription>
        </Alert>
      )}

      {query.data && rules.length === 0 && (
        <Empty description="No typed Alert Rules are defined in this Server build." />
      )}

      {rules.length > 0 && (
        <div className="overflow-x-auto rounded-md border border-border/60">
          <table data-stack className="w-full text-left text-sm" aria-label="Alert Rules">
            <thead className={cn('text-xs uppercase tracking-wide text-muted-foreground', SURFACE_TOOLBAR)}>
              <tr>
                <th scope="col" className="px-3 py-2 font-medium">Rule</th>
                <th scope="col" className="px-3 py-2 font-medium">Subject</th>
                <th scope="col" className="px-3 py-2 font-medium">State</th>
                <th scope="col" className="px-3 py-2 font-medium">Severity</th>
                <th scope="col" className="px-3 py-2 font-medium">Version</th>
                <th scope="col" className="px-3 py-2 font-medium">Open Incidents</th>
                <th scope="col" className="px-3 py-2 font-medium">Subjects firing / pending</th>
              </tr>
            </thead>
            <tbody>
              {rules.map((rule) => (
                <tr key={rule.ruleKey} className="border-t border-border/60">
                  <th scope="row" data-label="Rule" className="min-w-0 px-3 py-2 font-medium">
                    <Link
                      className="inline-flex min-h-11 items-center underline-offset-4 hover:underline"
                      to={'/admin/alerts/rules/' + encodeURIComponent(rule.ruleKey)}
                    >
                      {rule.ruleKey}
                    </Link>
                  </th>
                  <td data-label="Subject" className="min-w-0 px-3 py-2">{rule.subjectKind}</td>
                  <td data-label="State" className="min-w-0 px-3 py-2">
                    <StatusBadge
                      status={rule.enabled ? 'Enabled' : 'Disabled'}
                      tone={rule.enabled ? 'ok' : 'neutral'}
                    />
                  </td>
                  <td data-label="Severity" className="min-w-0 px-3 py-2">
                    <StatusBadge status={severityLabel(rule.severity)} tone={severityTone(rule.severity)} />
                  </td>
                  <td data-label="Version" className="min-w-0 px-3 py-2 tabular-nums">{rule.version}</td>
                  <td data-label="Open Incidents" className="min-w-0 px-3 py-2 tabular-nums">{rule.openIncidents}</td>
                  <td data-label="Subjects firing / pending" className="min-w-0 px-3 py-2 tabular-nums">
                    {rule.evaluation.evaluationUnavailable
                      ? 'Unavailable'
                      : rule.evaluation.firing + ' / ' + rule.evaluation.pending}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  )
}

function ConflictNotice({ onReload, busy }: { onReload: () => void; busy: boolean }) {
  return (
    <Alert variant="destructive" role="alert">
      <AlertTitle>This Rule changed since you read it</AlertTitle>
      <AlertDescription className="space-y-2">
        <p>
          The Server rejected the save because the composed version no longer matches. Reload the
          current configuration and review it before saving again.
        </p>
        <Button variant="outline" size="sm" onClick={onReload} disabled={busy}>
          <RefreshCw aria-hidden="true" size={16} /> Reload current configuration
        </Button>
      </AlertDescription>
    </Alert>
  )
}

function OverridesCard({
  rule,
  csrfToken,
  composedVersion,
  onConflict,
  onComposedVersion,
}: {
  rule: AlertRuleDetail
  csrfToken: string
  /** The composed revision this page reviewed. The override form sends THIS
   * version, never the live query's, so a background refetch from another
   * Owner's save cannot silently bless a stale override. */
  composedVersion: number
  onConflict: () => void
  /** The composed revision this write produced; the baseline form adopts it so
   * its next save does not carry a version the Server already superseded. */
  onComposedVersion: (version: number) => void
}) {
  const [scopeKind, setScopeKind] = useState('network')
  const [scopeValue, setScopeValue] = useState('')
  const [enabledChoice, setEnabledChoice] = useState('inherit')
  const [severityChoice, setSeverityChoice] = useState('inherit')
  const [conditionMode, setConditionMode] = useState('inherit')
  const [forSecs, setForSecs] = useState('')
  const [recoveryForSecs, setRecoveryForSecs] = useState('')
  const [threshold, setThreshold] = useState('')
  const [busy, setBusy] = useState(false)
  const [feedback, setFeedback] = useState<FormFeedback | null>(null)
  const [confirmingDelete, setConfirmingDelete] = useState<string | null>(null)

  const condition: RuleCondition | null =
    conditionMode === 'inherit'
      ? null
      : conditionFromParts(rule, { forSecs, recoveryForSecs, threshold })

  const everythingInherits =
    enabledChoice === 'inherit' && severityChoice === 'inherit' && conditionMode === 'inherit'

  const submit = async (event: FormEvent) => {
    event.preventDefault()
    setFeedback(null)
    if (scopeValue.trim() === '') {
      setFeedback({ tone: 'error', message: 'Choose the Network or Node the override applies to.' })
      return
    }
    if (everythingInherits) {
      setFeedback({
        tone: 'error',
        message: 'Override at least one field; an override with every field inherited is not allowed.',
      })
      return
    }
    setBusy(true)
    try {
      const response = await upsertRuleOverrideEntry(
        rule.ruleKey,
        {
          expectedVersion: composedVersion,
          scopeKind,
          scopeValue: scopeValue.trim(),
          enabled: enabledChoice === 'inherit' ? null : enabledChoice === 'enabled',
          severity: severityChoice === 'inherit' ? null : severityChoice,
          condition,
        },
        csrfToken,
      )
      onComposedVersion(response.version)
      setFeedback({
        tone: 'ok',
        message:
          'Override saved as composed version ' + response.version + '. A reader holding the previous version must reload before saving.',
      })
      setScopeValue('')
    } catch (error) {
      if (isVersionConflict(error)) {
        onConflict()
        return
      }
      setFeedback({
        tone: 'error',
        message: errorMessage(error, 'Unable to save the Rule override.'),
      })
    } finally {
      setBusy(false)
    }
  }

  const remove = async (override: RuleOverrideDto) => {
    setBusy(true)
    setFeedback(null)
    try {
      const response = await deleteRuleOverrideEntry(
        rule.ruleKey,
        override.scopeKind,
        override.scopeValue,
        csrfToken,
      )
      onComposedVersion(response.version)
      setFeedback({
        tone: 'ok',
        message:
          'Override removed as composed version ' + response.version + '; the affected subjects inherit the layer below.',
      })
    } catch (error) {
      if (isVersionConflict(error)) {
        onConflict()
        return
      }
      setFeedback({ tone: 'error', message: errorMessage(error, 'Unable to remove the override.') })
    } finally {
      setBusy(false)
      setConfirmingDelete(null)
    }
  }

  return (
    <CardX size="medium" className={CARD_SURFACE} title="Network and Node overrides">
      <div className="space-y-4">
        <p className="text-sm text-muted-foreground">{INHERITANCE_NOTE}</p>
        {rule.overrides.length === 0 ? (
          <Empty description="No override narrows this Rule; every subject uses the baseline." />
        ) : (
          <div className="overflow-x-auto rounded-md border border-border/60">
            <table data-stack className="w-full text-left text-sm" aria-label="Rule overrides">
              <thead className={cn('text-xs uppercase tracking-wide text-muted-foreground', SURFACE_TOOLBAR)}>
                <tr>
                  <th scope="col" className="px-3 py-2 font-medium">Scope</th>
                  <th scope="col" className="px-3 py-2 font-medium">Enabled</th>
                  <th scope="col" className="px-3 py-2 font-medium">Severity</th>
                  <th scope="col" className="px-3 py-2 font-medium">Condition</th>
                  <th scope="col" className="px-3 py-2 font-medium">Updated</th>
                  <th scope="col" className="px-3 py-2 font-medium">Action</th>
                </tr>
              </thead>
              <tbody>
                {rule.overrides.map((override) => {
                  const key = override.scopeKind + '/' + override.scopeValue
                  return (
                    <tr key={key} className="border-t border-border/60 align-top">
                      <th scope="row" data-label="Scope" className="min-w-0 px-3 py-2 font-medium">
                        {override.scopeKind}: <span className="break-all">{override.scopeValue}</span>
                      </th>
                      <td data-label="Enabled" className="min-w-0 px-3 py-2">
                        <InheritedOrValue
                          value={
                            override.enabled === null || override.enabled === undefined
                              ? null
                              : override.enabled
                                ? 'Enabled'
                                : 'Disabled'
                          }
                        />
                      </td>
                      <td data-label="Severity" className="min-w-0 px-3 py-2">
                        <InheritedOrValue
                          value={override.severity === null || override.severity === undefined ? null : severityLabel(override.severity)}
                        />
                      </td>
                      <td data-label="Condition" className="min-w-0 px-3 py-2">
                        <InheritedOrValue
                          value={
                            override.condition === null || override.condition === undefined
                              ? null
                              : conditionSummary(override.condition)
                          }
                        />
                      </td>
                      <td data-label="Updated" className="min-w-0 px-3 py-2">{formatObservedAt(override.updatedAt)}</td>
                      <td data-label="Action" className="min-w-0 px-3 py-2">
                        {confirmingDelete === key ? (
                          <span
                            className="flex flex-wrap items-center gap-2"
                            role="group"
                            aria-label={'Confirm removal of ' + key}
                          >
                            <Button variant="destructive" size="sm" disabled={busy} onClick={() => void remove(override)}>
                              Confirm removal
                            </Button>
                            <Button variant="ghost" size="sm" disabled={busy} onClick={() => setConfirmingDelete(null)}>
                              Cancel
                            </Button>
                          </span>
                        ) : (
                          <Button variant="outline" size="sm" onClick={() => setConfirmingDelete(key)}>
                            Remove
                          </Button>
                        )}
                      </td>
                    </tr>
                  )
                })}
              </tbody>
            </table>
          </div>
        )}

        <form className="space-y-3" onSubmit={(event) => void submit(event)} aria-label="Add or replace a Rule override">
          <div className="grid gap-3 sm:grid-cols-2">
            <label className="space-y-1 text-sm">
              <span className="font-medium">Scope kind</span>
              <Select
                value={scopeKind}
                onChange={(event) => {
                  setScopeKind(event.target.value)
                  setEnabledChoice(event.target.value === 'node' ? 'inherit' : enabledChoice)
                }}
                aria-label="Override scope kind"
              >
                <option value="network">Network</option>
                <option value="node">Node</option>
              </Select>
            </label>
            <label className="space-y-1 text-sm">
              <span className="font-medium">Scope value</span>
              <Input
                value={scopeValue}
                onChange={(event) => setScopeValue(event.target.value)}
                placeholder={scopeKind === 'network' ? 'network key' : 'node id'}
                aria-label="Override scope value"
              />
            </label>
            <label className="space-y-1 text-sm">
              <span className="font-medium">Enabled</span>
              <Select
                value={enabledChoice}
                onChange={(event) => setEnabledChoice(event.target.value)}
                aria-label="Override enabled"
              >
                <option value="inherit">Inherit</option>
                <option value="enabled">Enabled</option>
                <option value="disabled">Disabled</option>
              </Select>
            </label>
            <label className="space-y-1 text-sm">
              <span className="font-medium">Severity</span>
              <Select
                value={severityChoice}
                onChange={(event) => setSeverityChoice(event.target.value)}
                aria-label="Override severity"
              >
                <option value="inherit">Inherit</option>
                {SEVERITIES.map((severity) => (
                  <option key={severity} value={severity}>
                    {severityLabel(severity)}
                  </option>
                ))}
              </Select>
            </label>
          </div>
          <label className="flex items-center gap-2 text-sm">
            <Checkbox
              checked={conditionMode === 'override'}
              onChange={(event) => setConditionMode(event.target.checked ? 'override' : 'inherit')}
              aria-label="Override the condition"
            />
            <span className="font-medium">Override the condition</span>
          </label>
          {conditionMode === 'override' && (
            <div className="grid gap-3 sm:grid-cols-3">
              <label className="space-y-1 text-sm">
                <span>for seconds</span>
                <Input
                  type="number"
                  min={0}
                  value={forSecs}
                  onChange={(event) => setForSecs(event.target.value)}
                  aria-label="Override for seconds"
                />
              </label>
              <label className="space-y-1 text-sm">
                <span>recovery seconds</span>
                <Input
                  type="number"
                  min={0}
                  value={recoveryForSecs}
                  onChange={(event) => setRecoveryForSecs(event.target.value)}
                  aria-label="Override recovery seconds"
                />
              </label>
              <label className="space-y-1 text-sm">
                <span>threshold (optional)</span>
                <Input
                  type="number"
                  step="any"
                  value={threshold}
                  onChange={(event) => setThreshold(event.target.value)}
                  aria-label="Override threshold"
                />
              </label>
            </div>
          )}
          <FormFeedbackNote feedback={feedback} />
          <Button type="submit" disabled={busy}>
            Save override
          </Button>
        </form>
      </div>
    </CardX>
  )
}

function PreviewCard({
  rule,
  draft,
  csrfToken,
  disabled,
}: {
  rule: AlertRuleDetail
  draft: RuleDraft
  csrfToken: string
  disabled: boolean
}) {
  const [busy, setBusy] = useState(false)
  const [subjects, setSubjects] = useState<RulePreviewSubject[] | null>(null)
  const [error, setError] = useState<string | null>(null)

  const run = async () => {
    setBusy(true)
    setError(null)
    try {
      const result = await previewAlertRuleEntry(
        rule.ruleKey,
        {
          enabled: draft.enabled,
          severity: draft.severity,
          condition: conditionFromDraft(rule, draft),
        },
        csrfToken,
      )
      setSubjects(result.subjects)
    } catch (caught) {
      setError(errorMessage(caught, 'Unable to preview the Alert Rule.'))
    } finally {
      setBusy(false)
    }
  }

  return (
    <CardX size="medium" className={CARD_SURFACE} title="Preview this configuration">
      <div className="space-y-3">
        <p className="text-sm text-muted-foreground">
          Preview evaluates the unsaved form values against current facts. It is not a save: it
          creates, resolves, and acknowledges nothing, and a projected firing is not a promise that
          an Incident will open.
        </p>
        <Button variant="outline" onClick={() => void run()} disabled={busy || disabled}>
          Preview unsaved changes
        </Button>
        {error && (
          <p role="alert" className="rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm text-destructive">
            {error}
          </p>
        )}
        {subjects && subjects.length === 0 && (
          <p className="text-sm text-muted-foreground" role="status">
            No subject is currently eligible for this Rule.
          </p>
        )}
        {subjects && subjects.length > 0 && (
          <div className="overflow-x-auto rounded-md border border-border/60">
            <table data-stack className="w-full text-left text-sm" aria-label="Previewed subjects">
              <thead className={cn('text-xs uppercase tracking-wide text-muted-foreground', SURFACE_TOOLBAR)}>
                <tr>
                  <th scope="col" className="px-3 py-2 font-medium">Subject</th>
                  <th scope="col" className="px-3 py-2 font-medium">Current state</th>
                  <th scope="col" className="px-3 py-2 font-medium">Projected state</th>
                  <th scope="col" className="px-3 py-2 font-medium">Would fire</th>
                  <th scope="col" className="px-3 py-2 font-medium">Note</th>
                </tr>
              </thead>
              <tbody>
                {subjects.map((subject) => (
                  <tr key={subject.subjectKind + '/' + subject.subjectKey} className="border-t border-border/60">
                    <th scope="row" data-label="Subject" className="min-w-0 px-3 py-2 font-medium break-all">
                      {subject.subjectKind}: {subject.subjectKey}
                    </th>
                    <td data-label="Current state" className="min-w-0 px-3 py-2">{subject.currentState}</td>
                    <td data-label="Projected state" className="min-w-0 px-3 py-2">{subject.projectedState}</td>
                    <td data-label="Would fire" className="min-w-0 px-3 py-2">
                      <StatusBadge
                        status={subject.wouldFire ? 'Yes' : 'No'}
                        tone={subject.wouldFire ? 'warning' : 'neutral'}
                      />
                    </td>
                    <td data-label="Note" className="min-w-0 px-3 py-2">{subject.note}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>
    </CardX>
  )
}

function VersionsCard({ rule }: { rule: AlertRuleDetail }) {
  return (
    <CardX size="medium" className={CARD_SURFACE} title="Immutable version history">
      <div className="space-y-2">
        <p className="text-sm text-muted-foreground">
          Every configuration change, including an override change, appends an immutable composed
          revision. An Incident keeps the version it opened under.
        </p>
        <div className="overflow-x-auto rounded-md border border-border/60">
          <table data-stack className="w-full text-left text-sm" aria-label="Rule versions">
            <thead className={cn('text-xs uppercase tracking-wide text-muted-foreground', SURFACE_TOOLBAR)}>
              <tr>
                <th scope="col" className="px-3 py-2 font-medium">Version</th>
                <th scope="col" className="px-3 py-2 font-medium">Severity</th>
                <th scope="col" className="px-3 py-2 font-medium">Condition</th>
                <th scope="col" className="px-3 py-2 font-medium">Saved</th>
              </tr>
            </thead>
            <tbody>
              {rule.versions.map((version) => (
                <tr key={version.version} className="border-t border-border/60">
                  <th scope="row" data-label="Version" className="min-w-0 px-3 py-2 font-medium tabular-nums">
                    {version.version}
                  </th>
                  <td data-label="Severity" className="min-w-0 px-3 py-2">
                    <StatusBadge status={severityLabel(version.severity)} tone={severityTone(version.severity)} />
                  </td>
                  <td data-label="Condition" className="min-w-0 px-3 py-2">{conditionSummary(version.condition)}</td>
                  <td data-label="Saved" className="min-w-0 px-3 py-2">{formatObservedAt(version.createdAt)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
    </CardX>
  )
}

function StatesCard({ rule }: { rule: AlertRuleDetail }) {
  const states: RuleStateDto[] = rule.states
  return (
    <CardX size="medium" className={CARD_SURFACE} title="Evaluated subject state">
      {states.length === 0 ? (
        <Empty description="No subject has evaluated state for this Rule yet." />
      ) : (
        <div className="overflow-x-auto rounded-md border border-border/60">
          <table data-stack className="w-full text-left text-sm" aria-label="Rule subject states">
            <thead className={cn('text-xs uppercase tracking-wide text-muted-foreground', SURFACE_TOOLBAR)}>
              <tr>
                <th scope="col" className="px-3 py-2 font-medium">Subject</th>
                <th scope="col" className="px-3 py-2 font-medium">State</th>
                <th scope="col" className="px-3 py-2 font-medium">Input</th>
                <th scope="col" className="px-3 py-2 font-medium">Since</th>
                <th scope="col" className="px-3 py-2 font-medium">Last evaluated</th>
                <th scope="col" className="px-3 py-2 font-medium">Open Incidents</th>
              </tr>
            </thead>
            <tbody>
              {states.map((state) => (
                <tr key={state.subjectKind + '/' + state.subjectKey} className="border-t border-border/60">
                  <th scope="row" data-label="Subject" className="min-w-0 px-3 py-2 font-medium break-all">
                    {state.subjectKind}: {state.subjectKey}
                  </th>
                  <td data-label="State" className="min-w-0 px-3 py-2">
                    {state.evaluationUnavailable ? (
                      <StatusBadge status="Unavailable" tone="neutral" />
                    ) : (
                      state.state
                    )}
                  </td>
                  <td data-label="Input" className="min-w-0 px-3 py-2">
                    {state.evaluationUnavailable
                      ? 'Unknown (never treat unknown as 0)'
                      : state.inputValue === null || state.inputValue === undefined
                        ? state.inputKind
                        : state.inputKind + ' = ' + state.inputValue}
                  </td>
                  <td data-label="Since" className="min-w-0 px-3 py-2">{formatObservedAt(state.since)}</td>
                  <td data-label="Last evaluated" className="min-w-0 px-3 py-2">{formatObservedAt(state.lastEvaluatedAt)}</td>
                  <td data-label="Open Incidents" className="min-w-0 px-3 py-2 tabular-nums">{state.openIncidents}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </CardX>
  )
}

/** PAGE-ADMIN-RULE-DETAIL: one Rule's inherited configuration, typed edit,
 * preview, overrides, and immutable history. */
export function AdminRuleDetailPage() {
  const { ruleKey = '' } = useParams()
  const { generation, status } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const query = useAdminAlertRuleDetail(generation, ruleKey)
  const rule = query.data
  const [draft, setDraft] = useState<RuleDraft | null>(null)
  const [conflict, setConflict] = useState(false)
  const [reloading, setReloading] = useState(false)
  const [busy, setBusy] = useState(false)
  const [feedback, setFeedback] = useState<FormFeedback | null>(null)
  // Only a new Rule identity or an explicit reload re-syncs the form. A
  // background refetch (another Owner saved) must NOT silently adopt the new
  // version, or the stale-writer conflict could never be observed.
  const syncedRuleKey = useRef<string | null>(null)

  useEffect(() => {
    if (!rule) return
    if (syncedRuleKey.current === rule.ruleKey) return
    // Switching to a different Rule starts a clean surface; the post-save
    // resync (syncedRuleKey was cleared for the SAME key) must not erase the
    // "Saved as version N." feedback the mutation just set.
    const switchingRule = syncedRuleKey.current !== null
    syncedRuleKey.current = rule.ruleKey
    setDraft(draftFromRule(rule))
    setConflict(false)
    if (switchingRule) setFeedback(null)
  }, [rule])

  const notFound =
    query.isError &&
    query.error instanceof AdminApiError &&
    (query.error.code === 'alert_rule_not_found' || query.error.code === 'not_found')

  if (notFound) {
    return (
      <section className="w-full min-w-0 space-y-4">
        <div className="space-y-1">
          <h1 className="text-lg font-semibold break-words">Alert Rule not found</h1>
          <p className="text-sm text-muted-foreground">
            No typed Rule matches {ruleKey}. Rules are the fixed catalog the Server build ships;
            arbitrary DSL is never accepted.
          </p>
        </div>
        <Link
          className="inline-flex min-h-11 items-center text-sm font-medium underline-offset-4 hover:underline"
          to="/admin/alerts/rules"
        >
          Back to Alert Rules
        </Link>
      </section>
    )
  }

  if (!rule || !draft) {
    return (
      <section className="w-full min-w-0 space-y-4">
        <div className="space-y-1">
          <h1 className="text-lg font-semibold break-words">Alert Rule detail</h1>
          <p className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground" role="status">
            <StatusBadge status={query.isError ? 'Error' : 'Starting'} tone={query.isError ? 'error' : 'neutral'} />{' '}
            {query.isError
              ? errorMessage(query.error, 'Unable to load the Alert Rule')
              : 'Loading the Alert Rule…'}
          </p>
          {query.isError && (
            <Button variant="link" size="sm" onClick={() => void query.refetch()}>
              Try again
            </Button>
          )}
        </div>
        <Link
          className="inline-flex min-h-11 items-center text-sm font-medium underline-offset-4 hover:underline"
          to="/admin/alerts/rules"
        >
          Back to Alert Rules
        </Link>
      </section>
    )
  }

  const reloadConfiguration = async () => {
    setReloading(true)
    try {
      const refreshed = await query.refetch()
      if (refreshed.data) {
        syncedRuleKey.current = refreshed.data.ruleKey
        setDraft(draftFromRule(refreshed.data))
        setConflict(false)
        setFeedback({ tone: 'ok', message: 'Reloaded the current configuration. Review it before saving again.' })
      }
    } catch (error) {
      // Surface the failure through the refetch error state; keep the conflict
      // so the Owner cannot save a stale draft.
      setFeedback({ tone: 'error', message: errorMessage(error, 'Unable to reload the current configuration.') })
    } finally {
      setReloading(false)
    }
  }

  const save = async (event: FormEvent) => {
    event.preventDefault()
    setFeedback(null)
    setBusy(true)
    try {
      const response = await updateAlertRuleEntry(
        rule.ruleKey,
        {
          expectedVersion: draft.expectedVersion,
          enabled: draft.enabled,
          severity: draft.severity,
          condition: conditionFromDraft(rule, draft),
        },
        csrfToken,
      )
      setConflict(false)
      syncedRuleKey.current = null
      setFeedback({ tone: 'ok', message: 'Saved as version ' + response.rule.version + '.' })
    } catch (error) {
      if (isVersionConflict(error)) {
        setConflict(true)
        return
      }
      setFeedback({ tone: 'error', message: errorMessage(error, 'Unable to save the Alert Rule.') })
    } finally {
      setBusy(false)
    }
  }

  return (
    <section className="w-full min-w-0 space-y-4">
      <div className="space-y-1">
        <h1 className="text-lg font-semibold break-words">Alert Rule {rule.ruleKey}</h1>
        <p className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground">
          <StatusBadge status={rule.enabled ? 'Enabled' : 'Disabled'} tone={rule.enabled ? 'ok' : 'neutral'} />
          <StatusBadge status={severityLabel(rule.severity)} tone={severityTone(rule.severity)} />
          <span>Composed version {rule.version}</span>
          <span>·</span>
          <span>{rule.openIncidents} open Incident(s)</span>
        </p>
        <Link
          className="inline-flex min-h-11 items-center text-sm font-medium underline-offset-4 hover:underline"
          to="/admin/alerts/rules"
        >
          Back to Alert Rules
        </Link>
      </div>

      {query.isRefetchError && (
        <p role="alert" className="rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm text-destructive">
          Failed to refresh; showing the last successful Rule values.
        </p>
      )}

      {!draft.enabled && (
        <Alert>
          <AlertTitle>This Rule is disabled</AlertTitle>
          <AlertDescription>
            New Incidents will not open while it is disabled. Disabling does not resolve, retract, or
            acknowledge any existing Incident: each keeps its opening rule version and evidence, and
            its acknowledgment stays intact.
          </AlertDescription>
        </Alert>
      )}

      <CardX size="medium" className={CARD_SURFACE} title="Baseline configuration">
        <dl className="grid gap-3 sm:grid-cols-2">
          <div>
            <dt className="text-xs uppercase tracking-wide text-muted-foreground">Subject kind</dt>
            <dd className="text-sm">{rule.subjectKind}</dd>
          </div>
          <div>
            <dt className="text-xs uppercase tracking-wide text-muted-foreground">Enabled</dt>
            <dd className="text-sm">{rule.enabled ? 'Enabled' : 'Disabled'}</dd>
          </div>
          <div>
            <dt className="text-xs uppercase tracking-wide text-muted-foreground">Severity</dt>
            <dd className="text-sm">{severityLabel(rule.severity)}</dd>
          </div>
          <div>
            <dt className="text-xs uppercase tracking-wide text-muted-foreground">Condition</dt>
            <dd className="text-sm">{conditionSummary(rule.condition)}</dd>
          </div>
          <div>
            <dt className="text-xs uppercase tracking-wide text-muted-foreground">Created</dt>
            <dd className="text-sm">{formatObservedAt(rule.createdAt)}</dd>
          </div>
          <div>
            <dt className="text-xs uppercase tracking-wide text-muted-foreground">Updated</dt>
            <dd className="text-sm">{formatObservedAt(rule.updatedAt)}</dd>
          </div>
        </dl>
        <p className="mt-3 text-sm text-muted-foreground">{INHERITANCE_NOTE}</p>
      </CardX>

      {conflict && <ConflictNotice onReload={() => void reloadConfiguration()} busy={reloading} />}

      <CardX size="medium" className={CARD_SURFACE} title="Edit this Rule">
        <form className="space-y-3" onSubmit={(event) => void save(event)} aria-label="Edit the Alert Rule">
          <label className="flex items-center gap-2 text-sm">
            <Checkbox
              checked={draft.enabled}
              disabled={conflict}
              onChange={(event) => setDraft({ ...draft, enabled: event.target.checked })}
              aria-label="Rule enabled"
            />
            <span className="font-medium">Enabled</span>
          </label>
          <label className="block max-w-xs space-y-1 text-sm">
            <span className="font-medium">Severity</span>
            <Select
              value={draft.severity}
              disabled={conflict}
              onChange={(event) => setDraft({ ...draft, severity: event.target.value })}
              aria-label="Rule severity"
            >
              {SEVERITIES.map((severity) => (
                <option key={severity} value={severity}>
                  {severityLabel(severity)}
                </option>
              ))}
            </Select>
          </label>
          <div className="grid gap-3 sm:grid-cols-3">
            <label className="space-y-1 text-sm">
              <span className="font-medium">for seconds</span>
              <Input
                type="number"
                min={0}
                value={draft.forSecs}
                disabled={conflict}
                onChange={(event) => setDraft({ ...draft, forSecs: event.target.value })}
                aria-label="Condition for seconds"
              />
            </label>
            <label className="space-y-1 text-sm">
              <span className="font-medium">recovery seconds</span>
              <Input
                type="number"
                min={0}
                value={draft.recoveryForSecs}
                disabled={conflict}
                onChange={(event) => setDraft({ ...draft, recoveryForSecs: event.target.value })}
                aria-label="Condition recovery seconds"
              />
            </label>
            {hasThresholdParam(rule) && (
              <label className="space-y-1 text-sm">
                <span className="font-medium">threshold (optional)</span>
                <Input
                  type="number"
                  step="any"
                  value={draft.threshold}
                  disabled={conflict}
                  onChange={(event) => setDraft({ ...draft, threshold: event.target.value })}
                  aria-label="Condition threshold"
                />
              </label>
            )}
          </div>
          <p className="text-sm text-muted-foreground">
            This save carries composed version {draft.expectedVersion}. If another Owner saves first, the
            Server rejects this one and you must reload and review.
          </p>
          <FormFeedbackNote feedback={feedback} />
          <Button type="submit" disabled={busy || conflict || reloading}>
            Save Rule
          </Button>
        </form>
      </CardX>

      <PreviewCard rule={rule} draft={draft} csrfToken={csrfToken} disabled={conflict} />

      <OverridesCard
        rule={rule}
        csrfToken={csrfToken}
        composedVersion={draft.expectedVersion}
        onConflict={() => setConflict(true)}
        onComposedVersion={(version) =>
          setDraft((current) => (current === null ? current : { ...current, expectedVersion: version }))
        }
      />

      <VersionsCard rule={rule} />
      <StatesCard rule={rule} />
    </section>
  )
}
