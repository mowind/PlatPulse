import { operationIsActive, operationStatusLabel, operationTone } from '../api/admin'
import type { OperationSummary } from '../api/generated'
import { StatusBadge } from '../components/StatusBadge'
import { ProgressThin, type ProgressStatus } from '../components/ui/progress-thin'
import { formatDuration } from '../formatDuration'

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
