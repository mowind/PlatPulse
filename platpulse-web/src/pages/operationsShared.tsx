import { operationIsActive, operationStatusLabel, operationTone } from '../api/admin'
import type { OperationIssue, OperationSummary } from '../api/generated'
import { StatusBadge } from '../components/StatusBadge'
import { ProgressThin, type ProgressStatus } from '../components/ui/progress-thin'
import { formatDuration } from '../formatDuration'
import { DetailItem, DetailList } from './notificationShared'

/**
 * Shared vocabulary for the Operations and Doctor Admin pages (issue #208).
 *
 * The Server owns the Operation ledger, the status/kind words, and the
 * progress percentage. These helpers only render what it sends, so every
 * surface tells the same story about queued versus terminal work.
 */

// The Admin detail primitives already live with the notification surface
// (issue #206). They are re-exported rather than copied so Operations pages
// read the same definitions instead of a parallel set.
export {
  CARD_SURFACE,
  DetailItem,
  DetailList,
  FormFeedbackNote,
  errorMessage,
  indeterminateOutcome,
  shortId,
  type FormFeedback,
} from './notificationShared'

/**
 * A transport failure while cancelling a task or queueing a Doctor run
 * means the Server may never have received the command. Cancellation and
 * Doctor runs carry no request id to reconcile, so the copy points at the
 * recorded ledger instead of promising a lookup that does not exist, and it
 * never re-sends the command on the reader's behalf.
 */
export const INDETERMINATE_OUTCOME =
  'The request may not have reached the Server, so the outcome is unknown. Nothing is re-sent automatically; the recorded state above is the only trustworthy answer.'

/**
 * The Server's own issues for one task, rendered verbatim: the code is the
 * machine-readable half, the message is the redacted one. Shared by the
 * Operation detail and the backup detail that tracks a verification task
 * (issues #208, #209).
 */
export function IssueList({ issues, label }: { issues: OperationIssue[]; label: string }) {
  if (issues.length === 0) {
    return <p className="text-sm text-muted-foreground">The Server recorded no {label}.</p>
  }
  return (
    <ul className="space-y-2 text-sm" data-slot={'operation-' + label.replace(/s$/, '') + '-list'}>
      {issues.map((issue, index) => (
        <li key={issue.code + String(index)} className="min-w-0 break-words">
          <span className="font-mono text-xs">{issue.code}</span>
          <span className="block text-muted-foreground">{issue.message}</span>
        </li>
      ))}
    </ul>
  )
}

/** Redacted Server payload, rendered in a locally scrollable region. */
export function ResultBlock({ result }: { result: unknown }) {
  return (
    <pre
      data-slot="operation-result"
      role="region"
      aria-label="Operation result payload"
      tabIndex={0}
      className="max-h-96 overflow-auto rounded-md border border-border/60 bg-muted/30 p-3 text-xs"
    >
      {JSON.stringify(result, null, 2)}
    </pre>
  )
}

/** The Server's fixed status vocabulary (webui.md §5.5). */
export const OPERATION_STATUSES = [
  'queued',
  'running',
  'succeeded',
  'succeeded_with_warnings',
  'failed',
  'cancelled',
] as const

/**
 * The Server's fixed kind vocabulary. `backup_create` stays listed so
 * historical rows remain filterable after ADR 0008 retired in-process
 * creation.
 */
export const OPERATION_KINDS = [
  'retention_run',
  'backup_create',
  'backup_verify',
  'doctor_run',
  'restore',
] as const

const PROGRESS_STATUS: Record<string, ProgressStatus> = {
  queued: 'info',
  running: 'info',
  succeeded: 'success',
  succeeded_with_warnings: 'warning',
  failed: 'error',
  cancelled: 'default',
}

/**
 * Progress applies only while a task is queued or running. A terminal
 * Operation no longer carries meaningful progress, so it reports
 * `null` and the page shows the recorded outcome instead: a finished task
 * with a stale 0% must never read as "Unknown", and a real 0% must never
 * read as "no progress recorded".
 */
export function operationProgressPercent(
  operation: { status?: string | null; progressPercent?: number | null } | null | undefined,
): number | null {
  if (!operationIsActive(operation)) return null
  return operation?.progressPercent ?? null
}

/** How long a terminal Operation ran. Unknown while it never finished. */
export function operationDuration(operation: OperationSummary): string {
  if (!operation.startedAt || !operation.finishedAt) return 'Unknown'
  const started = Date.parse(operation.startedAt)
  const finished = Date.parse(operation.finishedAt)
  if (!Number.isFinite(started) || !Number.isFinite(finished)) return 'Unknown'
  return formatDuration(finished - started)
}

/**
 * Age of a recorded timestamp. An absent or unparsable timestamp, and a
 * timestamp in the future, are named Unknown rather than rendered as a
 * small or negative age.
 */
export function recordedAge(
  timestamp: string | null | undefined,
  now: number = Date.now(),
): string {
  if (!timestamp) return 'Unknown'
  const parsed = Date.parse(timestamp)
  if (!Number.isFinite(parsed)) return 'Unknown'
  return formatDuration(now - parsed)
}

/**
 * A recorded cancel request is a request, not an outcome: the row keeps its
 * running status until the worker records the terminal one (issue #208).
 */
export function CancelRequestedNote({
  operation,
}: {
  operation: { cancelRequested?: boolean | null }
}) {
  if (!operation.cancelRequested) return null
  return (
    <span
      data-slot="operation-cancel-requested"
      className="block text-xs text-muted-foreground"
    >
      Cancel requested — the task stops at its next safe checkpoint, so the status below is still the recorded
      one until the outcome is written.
    </span>
  )
}

/** Status badge plus the cancel-requested note, used by every row and header. */
export function OperationStatus({
  operation,
}: {
  operation: { status?: string | null; cancelRequested?: boolean | null }
}) {
  return (
    <span className="min-w-0">
      <StatusBadge
        status={operationStatusLabel(operation.status)}
        tone={operationTone(operation.status)}
      />
      <CancelRequestedNote operation={operation} />
    </span>
  )
}

/** Percentage bar for queued and running work; terminal work is not tracked. */
export function OperationProgress({
  operation,
}: {
  operation: { status?: string | null; progressPercent?: number | null; progressLabel?: string | null }
}) {
  const percent = operationProgressPercent(operation)
  if (percent === null) {
    return <span className="text-xs text-muted-foreground">Not applicable</span>
  }
  const label = operation.progressLabel?.trim()
  return (
    <span className="block min-w-0 space-y-1">
      <ProgressThin
        percentage={percent}
        status={PROGRESS_STATUS[operation.status ?? ''] ?? 'default'}
        label={`Operation progress: ${percent}%`}
      />
      <span className="block text-xs text-muted-foreground">
        {label ? label + ' · ' : ''}
        {percent}%
      </span>
    </span>
  )
}

/**
 * The recorded outcome of a cancelled cleanup (issue #211, Story 40). The Server
 * writes this shape when a run stops: the phase it was stopped in, what it had
 * already released, and the planned work it did not attempt. Parsing is
 * deliberately defensive - the result column is Server-owned JSON - so an
 * unknown shape degrades to the raw payload below instead of inventing an
 * outcome this page cannot vouch for.
 */
export type CancellationPhase = 'queued' | 'running'

export type CancelledFamilyTotal = {
  family: string
  deletedRows: number
  estimatedRows: number
}

export type CancellationOutcome = {
  phase: CancellationPhase
  releasedRows: number
  remainingTargets: number
  families: CancelledFamilyTotal[]
  note: string
  previewId: string | null
}

function asRowCount(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

export function parseCancellationOutcome(result: unknown): CancellationOutcome | null {
  if (typeof result !== 'object' || result === null) return null
  const cancelled = (result as { cancelled?: unknown }).cancelled
  if (typeof cancelled !== 'object' || cancelled === null) return null
  const payload = cancelled as Record<string, unknown>
  const phase = payload.phase
  if (phase !== 'queued' && phase !== 'running') return null
  const releasedRows = asRowCount(payload.releasedRows)
  const remainingTargets = asRowCount(payload.remainingTargets)
  if (releasedRows === null || remainingTargets === null) return null
  const families: CancelledFamilyTotal[] = []
  if (Array.isArray(payload.families)) {
    for (const entry of payload.families) {
      if (typeof entry !== 'object' || entry === null) continue
      const family = (entry as { family?: unknown }).family
      if (typeof family !== 'string' || family.length === 0) continue
      const row = entry as Record<string, unknown>
      const deletedRows = asRowCount(row.deletedRows)
      const estimatedRows = asRowCount(row.estimatedRows)
      // A family the Server recorded no counts for is left out instead of being
      // shown as a fabricated "0 of an estimated 0" (webui.md §15.11: Server
      // owned facts are never invented); the raw result still renders below.
      if (deletedRows === null || estimatedRows === null) continue
      families.push({ family, deletedRows, estimatedRows })
    }
  }
  const previewId = (result as { previewId?: unknown }).previewId
  return {
    phase,
    releasedRows,
    remainingTargets,
    families,
    note: typeof payload.note === 'string' ? payload.note : '',
    previewId: typeof previewId === 'string' && previewId.length > 0 ? previewId : null,
  }
}

/**
 * What a stopped cleanup already did, in the Server's own words: released rows
 * stay released (a release is never rolled back), and the work that remained is
 * reported as stopped rather than as done.
 */
export function CancellationSummary({ result }: { result: unknown }) {
  const outcome = parseCancellationOutcome(result)
  if (!outcome) return null
  const phaseCopy =
    outcome.phase === 'queued'
      ? 'It was cancelled while it was still queued, so no batch ever ran and nothing was released.'
      : 'It stopped at a safe checkpoint between batches. The rows already released are work that happened; the rest was stopped, not attempted.'
  return (
    <div className="space-y-2 rounded-md border border-border/60 bg-muted/30 p-3" data-slot="operation-cancellation">
      <p className="text-sm font-medium" data-slot="operation-cancellation-phase">
        Cancelled {outcome.phase === 'queued' ? 'while queued' : 'while running'}
      </p>
      <p className="text-xs text-muted-foreground">{phaseCopy}</p>
      <DetailList>
        <DetailItem label="Rows already released">
          <span data-slot="operation-cancellation-released">{String(outcome.releasedRows)}</span>
        </DetailItem>
        <DetailItem label="Planned targets not completed">
          <span data-slot="operation-cancellation-remaining">{String(outcome.remainingTargets)}</span>
        </DetailItem>
        {outcome.previewId ? (
          <DetailItem label="Confirmed preview">
            <span className="font-mono text-xs break-all">{outcome.previewId}</span>
          </DetailItem>
        ) : null}
      </DetailList>
      {outcome.families.length > 0 && (
        <ul className="space-y-1 text-xs" data-slot="operation-cancellation-families">
          {outcome.families.map((family) => (
            <li key={family.family} className="min-w-0 break-words">
              <span className="font-mono">{family.family}</span>
              <span className="block text-muted-foreground">
                {String(family.deletedRows)} of an estimated {String(family.estimatedRows)} rows released
              </span>
            </li>
          ))}
        </ul>
      )}
      {outcome.note.length > 0 && (
        <p className="text-xs text-muted-foreground" data-slot="operation-cancellation-note">
          {outcome.note}
        </p>
      )}
    </div>
  )
}
