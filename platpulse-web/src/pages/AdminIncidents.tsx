import { useEffect, useMemo, useRef, useState, type FormEvent, type KeyboardEvent, type ReactNode } from 'react'
import { Link, useParams, useSearchParams } from 'react-router'
import { ChevronRight, ChevronUp } from 'lucide-react'
import {
  AdminApiError,
  acknowledgeIncidentEntry,
  useAdminIncidentDetail,
  useAdminIncidents,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { Button } from '../components/ui/button'
import { CardX } from '../components/ui/card-x'
import { Empty } from '../components/ui/empty'
import { Input, Select } from '../components/ui/input'
import { StatusBadge, formatObservedAt } from '../components/StatusBadge'
import { cn } from '../lib/utils'
import { SURFACE_CARD_STATIC, SURFACE_TOOLBAR } from '../lib/surface'
import { severityLabel, severityTone } from '../lib/severity'
import type {
  IncidentAcknowledgment,
  IncidentDetail,
  IncidentListItem,
} from '../api/generated'

/**
 * PAGE-ADMIN-INCIDENTS and PAGE-ADMIN-INCIDENT-DETAIL (parent #202, issue
 * #203; webui.md §15). Each row is one Incident occurrence — a subject
 * crossing a Rule's threshold — never a merged summary. An Incident owns its
 * state, severity, subject, evidence, and (once confirmed) one durable
 * Incident Acknowledgment recorded by the first Owner request. The
 * acknowledgment is a shared, non-retractable statement of who confirmed the
 * occurrence and when; it never marks the subject healthy, resolves the
 * Incident, or suppresses notifications, and it is separate from an Agent
 * Attention Acknowledgment. Acknowledging is reachable from the list and the
 * detail; recovered occurrences keep their acknowledgment, and a genuinely
 * recurring fault opens a new unacknowledged Incident.
 */

const CARD_SURFACE = cn('rounded-md border-none', SURFACE_CARD_STATIC)

// The bounded first page the list requests. Older occurrences are reachable by
// narrowing the filters, not by scrolling; there is no cursor yet (issue #203
// review C3).
const INCIDENT_PAGE_LIMIT = 200

function shortId(value: string): string {
  return value.length > 12 ? value.slice(0, 8) + '…' + value.slice(-4) : value
}

function capitalize(value: string): string {
  return value.length === 0 ? value : value.charAt(0).toUpperCase() + value.slice(1)
}

function stateLabel(state: string): string {
  return state === 'open' ? 'Open' : state === 'resolved' ? 'Resolved' : capitalize(state)
}

function subjectKindLabel(kind: string): string {
  return kind === 'agent' ? 'Agent' : kind === 'node' ? 'Node' : capitalize(kind)
}

/** An Incident is never reopened or manually recovered; the badge word carries
 * the meaning and the acknowledgment never changes it (webui.md §5.4). */
function stateTone(state: string): 'error' | 'ok' | 'neutral' {
  if (state === 'open') return 'error'
  if (state === 'resolved') return 'ok'
  return 'neutral'
}

function acknowledgmentText(acknowledgment: IncidentAcknowledgment): string {
  return 'Acknowledged by ' + acknowledgment.acknowledgedByUsername + ' at ' + formatObservedAt(acknowledgment.acknowledgedAt)
}

/** URL-state filters (design §10.1: back/forward preserves them). */
type IncidentFilterState = {
  state: string
  severity: string
  subjectKind: string
  subjectKey: string
  ruleKey: string
}

function readFilters(search: URLSearchParams): IncidentFilterState {
  return {
    state: search.get('state') ?? 'all',
    severity: search.get('severity') ?? 'all',
    subjectKind: search.get('subject') ?? 'all',
    // Exact subject key, set by the contextual Node/Agent shortcut (issue #202
    // Story 2). Empty means no subject narrowing.
    subjectKey: search.get('subject_key') ?? '',
    ruleKey: search.get('rule') ?? '',
  }
}

function DetailList({ children }: { children: ReactNode }) {
  return <dl className="grid grid-cols-1 gap-3 sm:grid-cols-2">{children}</dl>
}

function DetailItem({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="min-w-0">
      <dt className="text-xs font-medium tracking-wider text-muted-foreground">{label}</dt>
      <dd className="mt-0.5 min-w-0 break-words text-sm">{children}</dd>
    </div>
  )
}

/** Server-owned evidence (design §15.4): the JSON is presented unchanged; the
 * WebUI never rewrites an input detail, infers a cause, or fabricates a
 * recovery fact. */
function EvidenceBlock({ label, value }: { label: string; value: unknown }) {
  const text = useMemo(() => {
    try {
      return JSON.stringify(value ?? null, null, 2)
    } catch {
      return String(value)
    }
  }, [value])
  return (
    <div className="min-w-0 space-y-1">
      <h3 className="text-sm font-medium">{label}</h3>
      <pre
        data-slot="incident-evidence"
        tabIndex={0}
        className="max-h-72 min-w-0 overflow-auto rounded-md border border-border/60 bg-muted/30 p-3 text-xs whitespace-pre-wrap break-all"
      >
        {text}
      </pre>
    </div>
  )
}

type AcknowledgmentTarget = {
  incidentId: string
  acknowledgment?: IncidentAcknowledgment | null
  state: string
  subjectDeletedAt?: string | null
}

/**
 * The single Incident acknowledgment control, shared by the list and the
 * detail. A confirmation step precedes the request because the confirmation
 * is durable and non-retractable. A no-op repeat (another Owner confirmed
 * first) reports the authoritative identity instead of silently replacing
 * it, and refreshing is always safe.
 */
function IncidentAcknowledgmentPanel({
  incident,
  csrfToken,
  onAcknowledged,
  headingId,
  authoritativeAcknowledgment = null,
}: {
  incident: AcknowledgmentTarget
  csrfToken: string
  onAcknowledged: (incidentId: string, acknowledgment: IncidentAcknowledgment) => void
  headingId?: string
  /** The occurrence-scoped acknowledgment the parent already resolved (from a
   * write response, for example). It wins over the possibly stale read DTO so
   * the panel and the surrounding badge agree when the owner returns to an
   * occurrence whose refetch failed (issue #203 review B2). */
  authoritativeAcknowledgment?: IncidentAcknowledgment | null
}) {
  const [confirming, setConfirming] = useState(false)
  const [busy, setBusy] = useState(false)
  const [feedback, setFeedback] = useState<{ tone: 'ok' | 'conflict' | 'error'; message: string } | null>(null)
  // The write response carries the stored acknowledgment whether or not this
  // request recorded it. It is authoritative even when the follow-up read
  // fails, so the response — not the (possibly stale) props — drives what the
  // Owner sees after a confirmation attempt. It is tagged with the occurrence
  // it belongs to, so navigating to another Incident can never render one
  // occurrence's identity against a different one (issue #203 review R2).
  const [returned, setReturned] = useState<{ incidentId: string; acknowledgment: IncidentAcknowledgment } | null>(
    null,
  )
  // The occurrence this panel currently renders. A late response for a
  // previous occurrence must not touch the occurrence now on screen.
  const currentIncidentId = useRef(incident.incidentId)
  currentIncidentId.current = incident.incidentId
  const returnedAcknowledgment =
    returned && returned.incidentId === incident.incidentId ? returned.acknowledgment : null
  const acknowledgment =
    returnedAcknowledgment ?? authoritativeAcknowledgment ?? incident.acknowledgment ?? null
  const entryRef = useRef<HTMLButtonElement>(null)
  const confirmRef = useRef<HTMLButtonElement>(null)
  const resultRef = useRef<HTMLParagraphElement>(null)
  const headingRef = useRef<HTMLHeadingElement>(null)
  const sectionRef = useRef<HTMLElement>(null)
  const restoreFocus = useRef<'entry' | 'result' | null>(null)
  // Whether focus is currently inside this panel's action area. Used to detect
  // the external-acknowledgment case where the control the user was on is
  // removed by a refetch rather than by this panel's own state change.
  const focusInPanel = useRef(false)
  // Where the most recent focus went: true only while the last focused element
  // was inside this panel. Removing a focused control drops focus to BODY
  // without a new focusin, so this stays true exactly when the panel's own
  // control was removed, and is cleared as soon as anything outside the panel
  // takes focus (issue #203 reviews R3/N1).
  const lastFocusInPanel = useRef(false)
  const hadAction = useRef(!acknowledgment)

  // Focus management: entering the confirmation moves focus into it, cancelling
  // restores the entry control, and a settled result focuses the announced
  // outcome so a keyboard user is never dropped at the top of the page.
  useEffect(() => {
    const target = restoreFocus.current
    if (target === 'entry') {
      restoreFocus.current = null
      entryRef.current?.focus()
    } else if (target === 'result') {
      restoreFocus.current = null
      resultRef.current?.focus()
    } else if (confirming) {
      confirmRef.current?.focus()
    }
  }, [confirming, feedback])

  // Issue #203 review R3: an acknowledgment that arrives from a refetch can
  // remove the action area while focus is inside it, dropping a keyboard user
  // onto the document body. When the panel becomes answered and focus was in
  // its action area, move focus to the settled result (or the panel heading).
  // A background refresh that happens while focus is elsewhere is left alone.
  // Track the most recent focus owner so a default BODY focus is never mistaken
  // for the panel having held focus.
  useEffect(() => {
    const track = () => {
      lastFocusInPanel.current = sectionRef.current?.contains(document.activeElement) ?? false
    }
    document.addEventListener('focusin', track)
    return () => document.removeEventListener('focusin', track)
  }, [])

  useEffect(() => {
    const actionWasAlive = hadAction.current
    hadAction.current = !acknowledgment
    if (!acknowledgment || !actionWasAlive) return
    // The focused control was just removed, so the browser has already moved
    // focus to the body by the time this runs; that counts as focus having been
    // inside the panel. Focus somewhere else entirely is left untouched.
    const active = document.activeElement
    const focusWasInPanel =
      focusInPanel.current ||
      lastFocusInPanel.current ||
      (sectionRef.current !== null && active instanceof Node && sectionRef.current.contains(active))
    focusInPanel.current = false
    lastFocusInPanel.current = false
    if (!focusWasInPanel) return
    const target = resultRef.current ?? headingRef.current
    target?.focus()
  }, [acknowledgment])

  const submit = async () => {
    if (acknowledgment) return
    const targetId = incident.incidentId
    setBusy(true)
    setFeedback(null)
    try {
      const result = await acknowledgeIncidentEntry(targetId, csrfToken)
      // The Server returns the stored (first) acknowledgment in both cases, so
      // display that identity directly instead of waiting for a refetch. The
      // value is tagged with its occurrence and handed back even if the Owner
      // navigated away while the request was in flight (issue #203 review R2).
      setReturned({ incidentId: targetId, acknowledgment: result.acknowledgment })
      onAcknowledged(targetId, result.acknowledgment)
      if (currentIncidentId.current !== targetId) return
      restoreFocus.current = 'result'
      if (result.recorded) {
        setFeedback({ tone: 'ok', message: 'Acknowledgment recorded. This identity and time are authoritative.' })
      } else {
        setFeedback({
          tone: 'conflict',
          message:
            'This Incident already had an authoritative acknowledgment, so your request did not replace it. The stored identity and time are shown; a repeat is a no-op even for the same Owner.',
        })
      }
      setConfirming(false)
    } catch (error) {
      if (currentIncidentId.current !== targetId) return
      restoreFocus.current = 'result'
      setFeedback({
        tone: 'error',
        message: error instanceof Error ? error.message : 'Unable to acknowledge the Incident',
      })
    } finally {
      setBusy(false)
    }
  }

  return (
    <section
      ref={sectionRef}
      data-slot="incident-acknowledgment-panel"
      className="min-w-0 space-y-2"
      aria-labelledby={headingId}
      onFocus={() => {
        focusInPanel.current = true
        lastFocusInPanel.current = true
      }}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) {
          focusInPanel.current = false
        }
      }}
    >
      <div className="flex flex-wrap items-center gap-2">
        <h3 id={headingId} ref={headingRef} tabIndex={-1} className="text-sm font-medium">
          Incident acknowledgment
        </h3>
        {acknowledgment ? (
          <StatusBadge status="Acknowledged" tone="ok" />
        ) : (
          <StatusBadge status="Awaiting acknowledgment" tone="warning" />
        )}
      </div>
      {acknowledgment ? (
        <div data-slot="incident-acknowledgment" className="space-y-1">
          <p className="text-sm [overflow-wrap:anywhere]">
            {acknowledgmentText(acknowledgment)}
          </p>
          {acknowledgment.acknowledgedByUserId && (
            <p className="text-xs text-muted-foreground break-all">
              Owner account {acknowledgment.acknowledgedByUserId}
            </p>
          )}
          <p className="text-xs text-muted-foreground">
            The first successful confirmation is authoritative. Later requests — including a
            concurrent request from a second Owner or a repeat after a Server restart — cannot
            replace this identity or time, and a resolved occurrence keeps its acknowledgment.
          </p>
        </div>
      ) : (
        <div className="space-y-2">
          <p className="text-sm text-muted-foreground">
            No Owner has confirmed this Incident occurrence yet. Acknowledging records who
            confirmed it and when; it does not resolve the Incident, mark the subject healthy, or
            suppress notifications, and it is separate from an Agent Attention Acknowledgment.
          </p>
          {incident.state === 'resolved' && (
            <p className="text-xs text-muted-foreground">
              This occurrence is already resolved. Acknowledging only records that the Owner saw
              it.
            </p>
          )}
          {incident.subjectDeletedAt && (
            <p className="text-xs text-muted-foreground">
              The subject was permanently deleted. The Incident keeps its original facts and
              state; deletion is not a recovery.
            </p>
          )}
          {confirming ? (
            <div
              className="space-y-2 rounded-md border border-border/60 p-3"
              role="group"
              aria-label="Confirm Incident acknowledgment"
            >
              <p className="text-sm font-medium">Confirm acknowledgment of this Incident occurrence?</p>
              <p className="text-xs text-muted-foreground">
                The Server records your Owner identity and the current time as the durable, shared
                confirmation. This cannot be undone or replaced.
              </p>
              <div className="flex flex-wrap gap-2">
                <Button
                  ref={confirmRef}
                  size="sm"
                  className="min-h-11"
                  disabled={busy}
                  onClick={() => void submit()}
                >
                  {busy ? 'Acknowledging…' : 'Confirm acknowledgment'}
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  className="min-h-11"
                  disabled={busy}
                  onClick={() => {
                    restoreFocus.current = 'entry'
                    setConfirming(false)
                  }}
                >
                  Cancel
                </Button>
              </div>
            </div>
          ) : (
            <Button
              ref={entryRef}
              size="sm"
              className="min-h-11"
              disabled={busy || csrfToken.length === 0}
              aria-label={'Acknowledge Incident ' + incident.incidentId}
              onClick={() => setConfirming(true)}
            >
              Acknowledge this Incident
            </Button>
          )}
        </div>
      )}
      {feedback && (
        <p
          ref={resultRef}
          tabIndex={-1}
          role={feedback.tone === 'error' ? 'alert' : 'status'}
          className={cn(
            'rounded-md border p-3 text-sm',
            feedback.tone === 'ok' && 'border-border/60 bg-muted/30',
            feedback.tone === 'conflict' && 'border-amber-500/40 bg-amber-500/10',
            feedback.tone === 'error' && 'border-destructive/40 bg-destructive/5 text-destructive',
          )}
        >
          {feedback.message}
        </p>
      )}
    </section>
  )
}

/** PAGE-ADMIN-INCIDENTS: Owner-only Incident history with URL-state filters
 * and per-row evidence and acknowledgment. */
export default function AdminIncidentsList() {
  const { generation, status } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const [search, setSearch] = useSearchParams()
  const filters = readFilters(search)
  const [ruleKeyDraft, setRuleKeyDraft] = useState(filters.ruleKey)
  const [expanded, setExpanded] = useState<Set<string>>(new Set())
  // The acknowledgment returned by a write, keyed by Incident id. It overrides
  // the read result so a confirmed row reflects the authoritative identity even
  // if the follow-up refetch fails.
  const [ackOverrides, setAckOverrides] = useState<Record<string, IncidentAcknowledgment>>({})

  // Keep the text draft in step with the URL: browser back/forward changes the
  // rule filter without remounting the page, so a mount-only initializer would
  // leave a stale draft that overwrites the restored filter on the next Apply.
  useEffect(() => {
    setRuleKeyDraft(filters.ruleKey)
  }, [filters.ruleKey])

  const query = useAdminIncidents(generation, {
    state: filters.state === 'all' ? undefined : filters.state,
    severity: filters.severity === 'all' ? undefined : filters.severity,
    subjectKind: filters.subjectKind === 'all' ? undefined : filters.subjectKind,
    subjectKey: filters.subjectKey.trim() === '' ? undefined : filters.subjectKey.trim(),
    ruleKey: filters.ruleKey.trim() === '' ? undefined : filters.ruleKey.trim(),
    limit: INCIDENT_PAGE_LIMIT,
  })

  const incidents: IncidentListItem[] = query.data?.incidents ?? []
  const total = query.data?.total ?? 0

  const setFilter = (key: 'state' | 'severity' | 'subject', value: string) => {
    const next = new URLSearchParams(search)
    if (value === 'all') next.delete(key)
    else next.set(key, value)
    setSearch(next, { replace: false })
  }

  const applyRuleKey = (event: FormEvent) => {
    event.preventDefault()
    const next = new URLSearchParams(search)
    const value = ruleKeyDraft.trim()
    if (value === '') next.delete('rule')
    else next.set('rule', value)
    setSearch(next, { replace: false })
  }

  // The subject-key narrowing arrives from a Node/Agent detail shortcut; clear
  // it to return to the unfiltered history.
  const clearSubjectKey = () => {
    const next = new URLSearchParams(search)
    next.delete('subject_key')
    setSearch(next, { replace: false })
  }

  const toggle = (incidentId: string) => {
    setExpanded((current) => {
      const next = new Set(current)
      if (next.has(incidentId)) next.delete(incidentId)
      else next.add(incidentId)
      return next
    })
  }

  return (
    <section className="w-full min-w-0 space-y-4">
      <div className="space-y-1">
        <h1 className="text-lg font-semibold break-words">Incidents</h1>
        <p className="text-sm text-muted-foreground">
          Each row is one Incident occurrence. State, severity, subject, evidence, and the durable
          Owner acknowledgment are Server-owned; acknowledging never resolves an Incident or
          changes health, recovery, or notification policy.
        </p>
      </div>
      <form
        className={cn('flex flex-wrap items-end gap-3 rounded-md border-none p-3', SURFACE_TOOLBAR)}
        role="group"
        aria-label="Incident filters"
        onSubmit={applyRuleKey}
      >
        <label className="flex flex-col gap-1 text-xs font-medium tracking-wider text-muted-foreground">
          State
          <Select
            className="w-auto min-w-36"
            value={filters.state}
            onChange={(event) => setFilter('state', event.target.value)}
          >
            <option value="all">All</option>
            <option value="open">Open</option>
            <option value="resolved">Resolved</option>
          </Select>
        </label>
        <label className="flex flex-col gap-1 text-xs font-medium tracking-wider text-muted-foreground">
          Severity
          <Select
            className="w-auto min-w-36"
            value={filters.severity}
            onChange={(event) => setFilter('severity', event.target.value)}
          >
            <option value="all">All</option>
            <option value="critical">Critical</option>
            <option value="warning">Warning</option>
          </Select>
        </label>
        <label className="flex flex-col gap-1 text-xs font-medium tracking-wider text-muted-foreground">
          Subject
          <Select
            className="w-auto min-w-36"
            value={filters.subjectKind}
            onChange={(event) => setFilter('subject', event.target.value)}
          >
            <option value="all">All</option>
            <option value="agent">Agent</option>
            <option value="node">Node</option>
          </Select>
        </label>
        <label className="flex min-w-0 flex-col gap-1 text-xs font-medium tracking-wider text-muted-foreground">
          Rule key
          <Input
            className="w-full min-w-48"
            type="search"
            value={ruleKeyDraft}
            placeholder="e.g. node.rpc_unreachable"
            onChange={(event) => setRuleKeyDraft(event.target.value)}
          />
        </label>
        <Button type="submit" variant="outline" size="sm" className="min-h-11">
          Apply rule key
        </Button>
        {filters.subjectKey.trim() !== '' && (
          <span
            data-slot="incident-subject-filter"
            className="inline-flex min-w-0 max-w-full flex-wrap items-center gap-2 self-end rounded-md border border-border/60 px-3 py-2 text-xs text-muted-foreground"
          >
            <span className="min-w-0 [overflow-wrap:anywhere]">
              Subject key {filters.subjectKey}
            </span>
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="min-h-11"
              onClick={clearSubjectKey}
            >
              Clear subject
            </Button>
          </span>
        )}
      </form>
      {!query.data && query.isPending && (
        <p className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading the Incident history…
        </p>
      )}
      {!query.data && query.isError && (
        <div
          className="flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
          role="alert"
        >
          <StatusBadge status="Error" tone="error" />{' '}
          <span className="min-w-0 break-words">
            {query.error instanceof Error ? query.error.message : 'Unable to load Incidents'}
          </span>
          <Button variant="link" size="sm" onClick={() => void query.refetch()}>
            Try again
          </Button>
        </div>
      )}
      {query.data && query.isRefetchError && (
        <div
          className="flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
          role="alert"
        >
          <StatusBadge status="Error" tone="error" /> Failed to refresh; showing the last
          successful Incident values.
        </div>
      )}
      {query.data && incidents.length === 0 && (
        <CardX size="medium" className={CARD_SURFACE}>
          <Empty description="No Incidents match these filters." />
        </CardX>
      )}
      {query.data && incidents.length > 0 && (
        <CardX
          size="medium"
          className={CARD_SURFACE}
          contentClassName="p-0"
          segmented
          title={'Incident history · ' + incidents.length + ' of ' + total}
        >
          <div className="overflow-x-auto">
            <table data-stack data-slot="incident-table" className="w-full text-sm">
              <caption className="sr-only">
                Incident history: occurrence, subject, rule, severity, state, and acknowledgment
              </caption>
              <thead>
                <tr className="border-b">
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Incident
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Subject
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Rule
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Severity
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    State
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Sequence
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Acknowledgment
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    <span className="sr-only">Evidence</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {incidents.map((incident) => (
                  <IncidentListRow
                    key={incident.incidentId}
                    incident={
                      ackOverrides[incident.incidentId]
                        ? { ...incident, acknowledgment: ackOverrides[incident.incidentId] }
                        : incident
                    }
                    csrfToken={csrfToken}
                    expanded={expanded.has(incident.incidentId)}
                    onToggle={() => toggle(incident.incidentId)}
                    onAcknowledged={(confirmedIncidentId, acknowledgment) => {
                      setAckOverrides((current) => ({ ...current, [confirmedIncidentId]: acknowledgment }))
                    }}
                  />
                ))}
              </tbody>
            </table>
          </div>
          <p className="px-3 pb-3 text-xs text-muted-foreground">
            Showing up to the first {INCIDENT_PAGE_LIMIT} matching Incidents, newest first — a
            bounded first page, not the full history. Narrowing the filters finds older
            occurrences; a filter that still matches more than {INCIDENT_PAGE_LIMIT} Incidents
            cannot reach past them until cursor pagination exists.
          </p>
        </CardX>
      )}
    </section>
  )
}

function IncidentListRow({
  incident,
  csrfToken,
  expanded,
  onToggle,
  onAcknowledged,
}: {
  incident: IncidentListItem
  csrfToken: string
  expanded: boolean
  onToggle: () => void
  onAcknowledged: (incidentId: string, acknowledgment: IncidentAcknowledgment) => void
}) {
  const acknowledgment = incident.acknowledgment ?? null
  const detailId = 'incident-detail-' + incident.incidentId
  // The disclosure button survives every collapse, so restore focus to it when
  // the expanded region (or its Escape handler) removes the control the user
  // was on; otherwise focus falls back to the document body.
  const disclosureRef = useRef<HTMLButtonElement>(null)
  const collapseAndRestore = () => {
    onToggle()
    disclosureRef.current?.focus()
  }
  const collapseOnEscape = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key === 'Escape' && expanded) {
      event.preventDefault()
      onToggle()
      disclosureRef.current?.focus()
    }
  }
  return (
    <>
      <tr className="border-b border-border/60 align-top">
        <th scope="row" data-label="Incident" className="min-w-0 px-3 py-3 text-left">
          <Button
            ref={disclosureRef}
            variant="ghost"
            size="icon-xs"
            className="align-middle"
            aria-label={(expanded ? 'Collapse ' : 'Expand ') + 'Incident ' + incident.incidentId}
            aria-expanded={expanded}
            aria-controls={detailId}
            onClick={onToggle}
            onKeyDown={collapseOnEscape}
          >
            <ChevronRight size={16} aria-hidden="true" className={expanded ? 'rotate-90' : ''} />
          </Button>{' '}
          <Link
            className="inline-flex min-h-11 min-w-11 items-center break-all font-medium underline-offset-4 hover:underline"
            to={'/admin/alerts/incidents/' + incident.incidentId}
          >
            {shortId(incident.incidentId)}
          </Link>
          <small className="mt-0.5 block text-[11px] text-muted-foreground break-all" title={incident.incidentId}>
            Opened {formatObservedAt(incident.openedAt)}
          </small>
        </th>
        <td data-label="Subject" className="min-w-0 px-3 py-3">
          <span className="text-sm">
            {subjectKindLabel(incident.subjectKind)} · <span className="break-all">{incident.subjectKey}</span>
          </span>
          {incident.subjectDeletedAt && (
            <small className="mt-0.5 block">
              <StatusBadge status="Subject deleted" tone="neutral" />
            </small>
          )}
        </td>
        <td data-label="Rule" className="min-w-0 px-3 py-3">
          <span className="break-all text-sm">{incident.ruleKey}</span>
          <small className="mt-0.5 block text-[11px] text-muted-foreground">v{incident.ruleVersion}</small>
        </td>
        <td data-label="Severity" className="min-w-0 px-3 py-3">
          <StatusBadge status={severityLabel(incident.severity)} tone={severityTone(incident.severity)} />
        </td>
        <td data-label="State" className="min-w-0 px-3 py-3">
          <StatusBadge status={stateLabel(incident.state)} tone={stateTone(incident.state)} />
          {incident.resolvedAt && (
            <small className="mt-0.5 block text-[11px] text-muted-foreground">
              Resolved {formatObservedAt(incident.resolvedAt)}
            </small>
          )}
        </td>
        <td data-label="Sequence" className="min-w-0 px-3 py-3">
          <span className="font-bold leading-none tracking-tight">{incident.sequence}</span>
        </td>
        <td data-label="Acknowledgment" className="min-w-0 px-3 py-3">
          {acknowledgment ? (
            <>
              <StatusBadge status="Acknowledged" tone="ok" />
              <small className="mt-0.5 block text-[11px] text-muted-foreground [overflow-wrap:anywhere]">
                {acknowledgment.acknowledgedByUsername} · {formatObservedAt(acknowledgment.acknowledgedAt)}
              </small>
            </>
          ) : (
            <StatusBadge status="Awaiting acknowledgment" tone="warning" />
          )}
        </td>
        <td data-label="Evidence" className="min-w-0 px-3 py-3">
          <Button variant="link" size="sm" onClick={onToggle} aria-expanded={expanded} aria-controls={detailId}>
            {expanded ? 'Hide evidence' : 'Show evidence'}
          </Button>
        </td>
      </tr>
      {expanded && (
        <tr data-slot="detail-row" className="border-b border-border/60 bg-muted/30">
          <td colSpan={8} id={detailId} onKeyDown={collapseOnEscape} className="min-w-0 p-0">
            <div className="space-y-3 p-3">
              <Button variant="link" size="sm" onClick={collapseAndRestore}>
                Collapse evidence <ChevronUp size={16} aria-hidden="true" />
              </Button>
              <IncidentAcknowledgmentPanel
                incident={incident}
                csrfToken={csrfToken}
                onAcknowledged={onAcknowledged}
                authoritativeAcknowledgment={acknowledgment}
                headingId={'incident-list-ack-heading-' + incident.incidentId}
              />
              <p className="text-xs text-muted-foreground">
                Open the Incident for its full evidence, evaluation, and suppression context.
              </p>
              <Link
                className="inline-flex min-h-11 items-center text-sm font-medium underline-offset-4 hover:underline"
                to={'/admin/alerts/incidents/' + incident.incidentId}
              >
                Inspect Incident {shortId(incident.incidentId)}
              </Link>
            </div>
          </td>
        </tr>
      )}
    </>
  )
}

/** PAGE-ADMIN-INCIDENT-DETAIL (webui.md §15): one occurrence's Server-owned
 * facts — state, subject, evidence, evaluation, suppressions, and the durable
 * acknowledgment. There is deliberately no manual resolve/reopen/delete
 * control (design #202: acknowledgment never changes health or recovery). */
export function AdminIncidentDetailPage() {
  const { incidentId = '' } = useParams()
  const { generation, status } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const query = useAdminIncidentDetail(generation, incidentId)
  // The authoritative acknowledgments returned by writes, keyed by occurrence.
  // They override the read result so the header badge and card stay correct even
  // when a refetch fails, and a late response for an occurrence the Owner already
  // left cannot displace the one currently on screen (issue #203 reviews R2/B2).
  const [ackOverrides, setAckOverrides] = useState<Record<string, IncidentAcknowledgment>>({})
  const notFound =
    query.isError &&
    query.error instanceof AdminApiError &&
    (query.error.code === 'incident_not_found' || query.error.code === 'not_found')

  if (notFound) {
    return (
      <section className="w-full min-w-0 space-y-4">
        <div className="space-y-1">
          <h1 className="text-lg font-semibold break-words">Incident not found</h1>
          <p className="text-sm text-muted-foreground">
            No Incident matches {incidentId}. Incidents are never hand-created, and a recovered
            occurrence keeps its own record.
          </p>
        </div>
        <Link
          className="inline-flex min-h-11 items-center text-sm font-medium underline-offset-4 hover:underline"
          to="/admin/alerts/incidents"
        >
          Back to Incidents
        </Link>
      </section>
    )
  }

  if (!query.data) {
    return (
      <section className="w-full min-w-0 space-y-4">
        <div className="space-y-1">
          <h1 className="text-lg font-semibold break-words">Incident detail</h1>
          <p className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground" role="status">
            <StatusBadge status={query.isError ? 'Error' : 'Starting'} tone={query.isError ? 'error' : 'neutral'} />{' '}
            {query.isError
              ? query.error instanceof Error
                ? query.error.message
                : 'Unable to load the Incident'
              : 'Loading the Incident…'}
          </p>
          {query.isError && (
            <Button variant="link" size="sm" onClick={() => void query.refetch()}>
              Try again
            </Button>
          )}
        </div>
        <Link
          className="inline-flex min-h-11 items-center text-sm font-medium underline-offset-4 hover:underline"
          to="/admin/alerts/incidents"
        >
          Back to Incidents
        </Link>
      </section>
    )
  }

  const incident: IncidentDetail = query.data
  const evaluation = incident.evaluation ?? null
  // The current effective Rule configuration, resolved through the baseline and
  // any Network/Node override. It is deliberately separate from the opening
  // rule version and evidence: null means "unknown", never "disabled", and a
  // later Rule edit or disable never rewrites the opening facts.
  const currentRule = incident.currentRule ?? null
  const ruleEnabled: boolean | null = currentRule === null ? null : currentRule.enabled
  const acknowledgment = ackOverrides[incident.incidentId] ?? incident.acknowledgment ?? null

  return (
    <section className="w-full min-w-0 space-y-4">
      <div className="space-y-1">
        <h1 className="text-lg font-semibold break-words">Incident {shortId(incident.incidentId)}</h1>
        <p className="text-xs text-muted-foreground break-all">{incident.incidentId}</p>
        <p className="text-sm text-muted-foreground">
          One Incident occurrence for {subjectKindLabel(incident.subjectKind)} {incident.subjectKey} on
          Rule {incident.ruleKey} v{incident.ruleVersion}.
        </p>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <StatusBadge status={stateLabel(incident.state)} tone={stateTone(incident.state)} />
        <StatusBadge status={severityLabel(incident.severity)} tone={severityTone(incident.severity)} />
        <StatusBadge
          status={acknowledgment ? 'Acknowledged' : 'Awaiting acknowledgment'}
          tone={acknowledgment ? 'ok' : 'warning'}
        />
        {incident.subjectDeletedAt && <StatusBadge status="Subject deleted" tone="neutral" />}
      </div>
      {query.isRefetchError && (
        <div
          className="flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
          role="alert"
        >
          <StatusBadge status="Error" tone="error" /> Failed to refresh; showing the last successful
          Incident values.
        </div>
      )}
      <CardX size="medium" className={CARD_SURFACE} title="Occurrence">
        <DetailList>
          <DetailItem label="State">{stateLabel(incident.state)}</DetailItem>
          <DetailItem label="Severity">{severityLabel(incident.severity)}</DetailItem>
          <DetailItem label="Subject">
            {subjectKindLabel(incident.subjectKind)} · <span className="break-all">{incident.subjectKey}</span>
          </DetailItem>
          <DetailItem label="Rule">
            <span className="break-all">{incident.ruleKey}</span> · v{incident.ruleVersion}
          </DetailItem>
          <DetailItem label="Occurrence sequence">{incident.sequence}</DetailItem>
          <DetailItem label="Opened at">{formatObservedAt(incident.openedAt)}</DetailItem>
          <DetailItem label="Resolved at">{formatObservedAt(incident.resolvedAt)}</DetailItem>
          <DetailItem label="Subject deleted at">{formatObservedAt(incident.subjectDeletedAt)}</DetailItem>
        </DetailList>
        <p className="mt-3 text-xs text-muted-foreground">
          Subject deletion is annotated separately from recovery: a deleted subject is not a
          recovered Incident, and a genuinely recovered subject that faults again opens a new
          occurrence with its own sequence and its own unacknowledged record.
        </p>
      </CardX>
      <CardX size="medium" className={CARD_SURFACE} title="Current Rule configuration">
        {currentRule ? (
          <>
            <DetailList>
              <DetailItem label="Rule">
                <Link
                  className="underline-offset-4 hover:underline"
                  to={'/admin/alerts/rules/' + encodeURIComponent(currentRule.ruleKey)}
                >
                  <span className="break-all">{currentRule.ruleKey}</span>
                </Link>
              </DetailItem>
              <DetailItem label="Current effective version">v{currentRule.version}</DetailItem>
              <DetailItem label="Current enabled">{currentRule.enabled ? 'Enabled' : 'Disabled'}</DetailItem>
              <DetailItem label="Current effective severity">{severityLabel(currentRule.severity)}</DetailItem>
              <DetailItem label="Current effective condition">
                for {currentRule.condition.for_secs}s · recovery{' '}
                {currentRule.condition.recovery_for_secs}s
                {currentRule.condition.threshold == null
                  ? ''
                  : ' · threshold ' + currentRule.condition.threshold}
              </DetailItem>
            </DetailList>
            <p className="mt-3 text-xs text-muted-foreground">
              This is the configuration the Server resolves for this subject now, applying the
              baseline and then any Network/Node override, most specific winning. It is separate from
              the opening rule version (v{incident.ruleVersion}) and the opened evidence: editing or
              disabling the Rule never rewrites the opening facts and never clears the
              acknowledgment.
            </p>
          </>
        ) : (
          <p className="text-sm text-muted-foreground">
            The current effective Rule configuration could not be resolved for this subject. This is
            reported as unknown — never as a disabled Rule, a recovered subject, or an escalation.
            The opening rule version (v{incident.ruleVersion}) and its evidence are unaffected.
          </p>
        )}
      </CardX>
      <CardX size="medium" className={CARD_SURFACE} title="Acknowledgment">
        <IncidentAcknowledgmentPanel
          key={incident.incidentId}
          incident={incident}
          csrfToken={csrfToken}
          authoritativeAcknowledgment={acknowledgment}
          onAcknowledged={(confirmedIncidentId, ack) => {
            setAckOverrides((current) => ({ ...current, [confirmedIncidentId]: ack }))
          }}
          headingId="incident-detail-ack-heading"
        />
      </CardX>
      <CardX size="medium" className={CARD_SURFACE} title="Evaluation">
        {evaluation ? (
          <>
            {ruleEnabled === false && (
              <p className="mb-3 flex flex-wrap items-center gap-2 text-sm text-muted-foreground">
                <StatusBadge status="Rule disabled" tone="neutral" />
                This Rule is disabled, so the values below are its last recorded assessment
                ({formatObservedAt(evaluation.lastEvaluatedAt)}), not the current Rule state.
              </p>
            )}
            {ruleEnabled == null && (
              <p className="mb-3 flex flex-wrap items-center gap-2 text-sm text-muted-foreground">
                <StatusBadge status="Current Rule state unknown" tone="neutral" />
                The current Rule state could not be resolved for this subject, so the values below
                are the last recorded assessment ({formatObservedAt(evaluation.lastEvaluatedAt)}),
                not a statement about the present.
              </p>
            )}
            <DetailList>
              <DetailItem label="Rule state">{evaluation.state}</DetailItem>
              <DetailItem label="Evaluation available">
                {ruleEnabled === false
                  ? 'Not currently — the Rule is disabled'
                  : ruleEnabled == null
                    ? 'Unknown — the current Rule state could not be resolved'
                    : evaluation.evaluationUnavailable
                      ? 'No — evaluation is unavailable'
                      : 'Yes'}
              </DetailItem>
              <DetailItem label="Input kind">{evaluation.inputKind}</DetailItem>
              <DetailItem label="Input value">
                {evaluation.inputValue == null ? 'Unknown' : String(evaluation.inputValue)}
              </DetailItem>
              <DetailItem label="Input detail">{evaluation.inputDetail ?? 'None reported'}</DetailItem>
              <DetailItem label="Last evaluated at">{formatObservedAt(evaluation.lastEvaluatedAt)}</DetailItem>
              <DetailItem label="Since">{formatObservedAt(evaluation.since)}</DetailItem>
              <DetailItem label="Firing since">{formatObservedAt(evaluation.firingSince)}</DetailItem>
              <DetailItem label="Pending since">{formatObservedAt(evaluation.pendingSince)}</DetailItem>
              <DetailItem label="Recovering since">{formatObservedAt(evaluation.recoveringSince)}</DetailItem>
              <DetailItem label="Open Incidents for this subject">{evaluation.openIncidents}</DetailItem>
            </DetailList>
            <p className="mt-3 text-xs text-muted-foreground">
              {ruleEnabled === false
                ? 'A disabled Rule receives no fresh evidence, so these values are the last recorded assessment — never a claim that the subject recovered.'
                : ruleEnabled == null
                  ? 'The current Rule state could not be resolved, so these values are the last recorded assessment and may not describe the present.'
                  : 'The evaluation is the current Rule state, not the historical trigger for this occurrence. An unavailable evaluation is reported as unavailable and never as a recovered subject.'}
            </p>
          </>
        ) : (
          <p className="text-sm text-muted-foreground">
            No current evaluation row covers this subject. The Incident keeps its original opened
            evidence; a missing evaluation is never treated as recovery.
          </p>
        )}
      </CardX>
      <CardX size="medium" className={CARD_SURFACE} title="Suppressions">
        {incident.suppressions.length === 0 ? (
          <p className="text-sm text-muted-foreground">
            No Silence or Maintenance Window currently covers this subject. Suppressions are
            evaluated against the present, so an elapsed window is not listed.
          </p>
        ) : (
          <ul className="space-y-3">
            {incident.suppressions.map((suppression) => (
              <li
                key={suppression.id}
                className="min-w-0 space-y-1 border-b border-border/60 pb-3 last:border-b-0 last:pb-0"
              >
                <div className="flex flex-wrap items-center gap-2">
                  <StatusBadge status={capitalize(suppression.kind)} tone="neutral" />
                  <span className="text-sm">
                    {suppression.marksIncident
                      ? 'Marks the Incident suppressed'
                      : 'Does not mark the Incident suppressed'}
                  </span>
                </div>
                <p className="text-sm break-words">{suppression.reason}</p>
                <p className="text-xs text-muted-foreground">
                  {formatObservedAt(suppression.startsAt)} → {formatObservedAt(suppression.endsAt)}
                </p>
              </li>
            ))}
          </ul>
        )}
      </CardX>
      <CardX size="medium" className={CARD_SURFACE} title="Evidence">
        <div className="space-y-4">
          <EvidenceBlock label="Opened evidence" value={incident.openedEvidence} />
          {incident.state === 'resolved' && (
            <EvidenceBlock label="Resolved evidence" value={incident.resolvedEvidence} />
          )}
        </div>
      </CardX>
      <Link
        className="inline-flex min-h-11 items-center text-sm font-medium underline-offset-4 hover:underline"
        to="/admin/alerts/incidents"
      >
        Back to Incidents
      </Link>
    </section>
  )
}
