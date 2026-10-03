import { verificationLabel, verificationTone } from '../api/admin'
import { formatObservedAt } from '../components/StatusBadge'

/**
 * Shared vocabulary for the Owner backup surface (webui.md §15.12, issue
 * #209). Backup artifacts are created offline by the `platpulse-server
 * backup` command (ADR 0008), so every page here is read-and-request: the
 * Server owns the recorded manifest metadata and the verification outcome,
 * and this module renders exactly what it sent.
 */

/** Placeholder for a value the Server did not record. */
export const NOT_RECORDED = 'Not recorded'

/** Size of the recorded artifact file, in binary units. */
export function formatArtifactBytes(bytes: number | null | undefined): string {
  if (bytes == null || !Number.isFinite(bytes) || bytes < 0) return 'Unknown'
  if (bytes < 1024) return String(bytes) + ' bytes'
  const units = ['KiB', 'MiB', 'GiB', 'TiB'] as const
  let value = bytes / 1024
  let unit = 0
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024
    unit += 1
  }
  return value.toFixed(1) + ' ' + (units[unit] ?? 'TiB')
}

/** Recorded data window of the snapshot; absent bounds stay Not recorded. */
export function formatDataRange(artifact: {
  dataRangeMin?: string | null
  dataRangeMax?: string | null
}): string {
  if (!artifact.dataRangeMin || !artifact.dataRangeMax) return NOT_RECORDED
  return formatObservedAt(artifact.dataRangeMin) + ' → ' + formatObservedAt(artifact.dataRangeMax)
}

/**
 * Plain-language reading of the recorded verification outcome. The Server's
 * own reason is always shown verbatim next to this; an unrecognized failure
 * shape degrades to `null` so the page shows the recorded text alone instead
 * of inventing a cause.
 */
export function readVerification(
  verification: string | null | undefined,
  verificationError?: string | null,
): { label: string; tone: 'ok' | 'warning' | 'error'; reading: string | null } {
  const label = verificationLabel(verification)
  const tone = verificationTone(verification)
  switch (verification) {
    case 'ok':
      return {
        label,
        tone,
        reading:
          'The Server re-read this file and every check it runs passed: the contents matched the recorded ' +
          'checksum, the snapshot passed a read-only integrity check, the schema version matched, and the scan ' +
          'found no unredacted sensitive text.',
      }
    case 'failed':
      return { label, tone, reading: readFailure(verificationError) }
    case 'pending':
      return {
        label,
        tone,
        reading:
          'No verification outcome is recorded for this artifact yet, so nothing here says the file is readable.',
      }
    default:
      return {
        label,
        tone,
        reading:
          'The Server recorded a verification state this page does not recognize, so only the recorded text below is authoritative.',
      }
  }
}

function readFailure(verificationError?: string | null): string | null {
  const recorded = (verificationError ?? '').toLowerCase()
  if (recorded.includes('checksum')) {
    return 'The file no longer matches the checksum recorded when the artifact was created, so its contents changed after creation.'
  }
  if (recorded.includes('cannot open')) {
    return 'The Server could not open the file, so none of its contents were checked.'
  }
  if (recorded.includes('integrity')) {
    return 'The file opened, but it is not an intact, readable SQLite snapshot.'
  }
  if (recorded.includes('schema')) {
    return 'The snapshot is readable, but its schema version does not match the version recorded for this artifact.'
  }
  if (recorded.includes('unsafe')) {
    return 'The Server refused to read the recorded path, so the file was not checked.'
  }
  return null
}

/**
 * What the tracked verification task's own recorded status means for the
 * artifact state above it. The Server owns both records, and they are two
 * different facts: the artifact keeps its last recorded outcome while a task
 * is in flight, cancelled, or failed before it wrote one. A task that ends
 * `failed` after a recorded checksum failure is not a task that wrote nothing,
 * so the copy also depends on whether the artifact record already points at
 * this task and on whether the task wrote an outcome payload of its own. A record that does not point at this
 * task says only that: the page never infers that a later link is coming, because a newer task may have
 * replaced the link before this one finished.
 */
export function readTaskLifecycle({
  status,
  loaded,
  unreadable,
  linkedToArtifact,
  wroteOutcome,
  artifactOutcome,
}: {
  status: string | null | undefined
  loaded: boolean
  unreadable: boolean
  /** The artifact record this page shows already points at this task's id. */
  linkedToArtifact: boolean
  /** This task wrote a verification outcome payload of its own (linked or not). */
  wroteOutcome: boolean
  /** The outcome the artifact record above carries, as the Server recorded it. */
  artifactOutcome: string | null | undefined
}): string {
  if (unreadable) {
    return (
      'This page could not read the task’s own record, so the state above is still the one the Server last ' +
      'recorded. Open the task to read the Server’s answer.'
    )
  }
  if (!loaded) return 'Reading the task’s own state from the Server…'
  switch (status) {
    case 'queued':
    case 'running':
      return (
        'This task is queued or running. The state above is still the one the Server last recorded, and it ' +
        'changes only when a verification task writes its outcome.'
      )
    case 'succeeded':
      if (linkedToArtifact) {
        return 'This task finished and recorded the outcome shown above; the task page holds the Server’s full record.'
      }
      return (
        'This task finished, but the artifact record above does not point at it, so this task’s result is not ' +
        'attributed to the state shown there: that state is still the last outcome the Server recorded for this ' +
        'artifact.'
      )
    case 'succeeded_with_warnings':
      if (linkedToArtifact) {
        return 'This task finished with warnings, and the outcome shown above is the one it recorded.'
      }
      return (
        'This task finished with warnings, but the artifact record above does not point at it, so this task’s result ' +
        'is not attributed to the state shown there: that state is still the last outcome the Server recorded for ' +
        'this artifact.'
      )
    case 'cancelled':
      return (
        'The Server recorded this task as cancelled, so it wrote no outcome: the state above is still the last ' +
        'one the Server recorded for this artifact. Request verification again to start a new task.'
      )
    case 'failed':
      if (linkedToArtifact) {
        // The artifact above carries the outcome this task wrote; that is not the
        // same fact as the task ending in failure, because the Server can record
        // an outcome and still lose the task before it finalizes it (a restart
        // fails the running task while the outcome it wrote stands).
        if (artifactOutcome === 'failed') {
          return (
            'This task failed and the artifact record above holds the failure it recorded; the errors it wrote ' +
            'are listed below.'
          )
        }
        return (
          'This task ended as failed, and the artifact record above is the outcome this task wrote — read the two ' +
          'facts apart: the state above is what it recorded, the errors below are why the task itself ended as ' +
          'failed.'
        )
      }
      if (wroteOutcome) {
        return (
          'This task failed after writing a failure outcome of its own below, but the artifact record above does ' +
          'not point at it, so that failure is not attributed to the state shown there: that state is still the last ' +
          'outcome the Server recorded for this artifact.'
        )
      }
      return (
        'This task failed before it recorded an artifact outcome, so the state above is still the last one the ' +
        'Server recorded. The errors it wrote are listed below.'
      )
    default:
      return (
        'The Server recorded a task state this page does not recognize; the state above stands and the task page ' +
        'holds the authoritative record.'
      )
  }
}

/** A verification pass is a check, never a production-restore guarantee. */
export const VERIFICATION_SCOPE_NOTE =
  'A passing verification means the Server could re-read and check this file. It is not a restore rehearsal ' +
  'and it never proves that a production restore will succeed.'

/** The surface boundary every backup page states in its own words. */
export const BACKUP_SURFACE_SCOPE_NOTE =
  'Backups are created offline (ADR 0008) and restoring is a separate, deliberate operation. This surface only ' +
  'reads recorded artifacts and requests verification: it never creates, restores, or deletes a backup, and it ' +
  'offers no scheduling.'

/** The Operation ledger URL for a recorded task id. */
export function operationHref(operationId: string): string {
  return '/admin/operations/' + operationId
}
