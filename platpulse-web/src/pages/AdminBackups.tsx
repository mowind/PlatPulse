import { useEffect, useRef, useState } from 'react'
import { Link, useParams } from 'react-router'

import {
  AdminApiError,
  refetchAdminBackup,
  useAdminBackup,
  useAdminBackups,
  useAdminOperation,
  verifyBackupEntry,
} from '../api/admin'
import type { BackupArtifactSummary } from '../api/generated'
import { useAuth } from '../auth/AuthContext'
import { StatusBadge, formatObservedAt } from '../components/StatusBadge'
import { Button } from '../components/ui/button'
import { CardX } from '../components/ui/card-x'
import { Empty } from '../components/ui/empty'
import {
  BACKUP_SURFACE_SCOPE_NOTE,
  NOT_RECORDED,
  VERIFICATION_SCOPE_NOTE,
  formatArtifactBytes,
  formatDataRange,
  operationHref,
  readTaskLifecycle,
  readVerification,
} from './backupShared'
import {
  CARD_SURFACE,
  DetailItem,
  DetailList,
  FormFeedbackNote,
  INDETERMINATE_OUTCOME,
  IssueList,
  OperationProgress,
  OperationStatus,
  ResultBlock,
  errorMessage,
  indeterminateOutcome,
  recordedAge,
  shortId,
  type FormFeedback,
} from './operationsShared'

/**
 * PAGE-ADMIN-BACKUPS / PAGE-ADMIN-BACKUP-DETAIL (webui.md §15.12, issue #209).
 * The Server owns the artifact manifest and the verification outcome: these
 * pages read the recorded metadata, request a verification, and render the
 * outcome the worker writes. They never create, restore, or delete a backup,
 * they offer no scheduling, and a passing verification is never presented as
 * proof that a production restore will succeed.
 */

export default function AdminBackupsList() {
  const { generation } = useAuth()
  // The Admin SSE stream carries invalidation only, so the Server's own
  // "backups" event is what refreshes a verification outcome written while
  // this page is open.
  const backups = useAdminBackups(generation)
  const rows = backups.data ?? []

  return (
    <section className="w-full min-w-0 space-y-4">
      <div className="space-y-3">
        <h1 className="text-xl font-semibold tracking-tight">Backups</h1>
        <p className="text-sm text-muted-foreground">
          Every backup artifact the Server has recorded, with the manifest metadata and verification state it
          wrote. Backups are created offline; nothing on this page creates, restores, or deletes one.
        </p>
      </div>

      {!backups.data && backups.isPending && (
        <p role="status" className="flex flex-wrap items-center gap-2 text-sm">
          <StatusBadge status="Starting" tone="neutral" /> Loading recorded backup artifacts…
        </p>
      )}
      {!backups.data && backups.isError && (
        <div role="alert" className="space-y-2 text-sm">
          <p>{errorMessage(backups.error, 'Unable to load backup artifacts')}</p>
          <Button variant="link" size="sm" onClick={() => void backups.refetch()}>
            Try again
          </Button>
        </div>
      )}
      {backups.data && backups.isRefetchError && (
        <p role="alert" className="text-sm">
          Failed to refresh; showing the last successful backup artifact list.
        </p>
      )}
      {backups.data && rows.length === 0 && (
        <CardX size="medium" className={CARD_SURFACE}>
          <Empty description="No backup artifact is recorded yet. Artifacts appear here after an offline backup run on the Server host." />
        </CardX>
      )}
      {backups.data && rows.length > 0 && (
        <CardX
          size="medium"
          className={CARD_SURFACE}
          contentClassName="p-0"
          segmented
          title={'Recorded backup artifacts · ' + String(rows.length)}
        >
          <div className="overflow-x-auto">
            <table data-stack data-slot="backups-table" className="w-full text-sm">
              <caption className="sr-only">Recorded backup artifacts</caption>
              <thead>
                <tr className="border-b">
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Artifact
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Created
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Size
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Schema
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Data window
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Server
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Creating task
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Verification
                  </th>
                </tr>
              </thead>
              <tbody>
                {rows.map((artifact) => (
                  <ArtifactRow key={artifact.artifactId} artifact={artifact} />
                ))}
              </tbody>
            </table>
          </div>
        </CardX>
      )}
      {backups.data && rows.length > 0 && (
        <p className="text-xs text-muted-foreground">
          Every recorded artifact, newest first. Verification runs only when someone requests it from an artifact
          page: the Server never verifies on a schedule, and a recorded state is never overwritten by a request
          that has not produced an outcome yet.
        </p>
      )}
      <p className="text-xs text-muted-foreground">{BACKUP_SURFACE_SCOPE_NOTE}</p>
    </section>
  )
}

function ArtifactRow({ artifact }: { artifact: BackupArtifactSummary }) {
  const reading = readVerification(artifact.verification)
  return (
    <tr className="border-b border-border/60 align-top">
      <th scope="row" data-label="Artifact" className="min-w-0 px-3 py-3 text-left font-normal">
        <Link
          className="inline-flex min-h-11 items-center break-all font-mono text-xs underline underline-offset-4"
          to={'/admin/backups/' + artifact.artifactId}
        >
          {artifact.filename}
        </Link>
        <span className="block font-mono text-xs text-muted-foreground">{shortId(artifact.artifactId)}</span>
      </th>
      <td data-label="Created" className="min-w-0 px-3 py-3">
        {formatObservedAt(artifact.createdAt)}
        <span className="block text-xs text-muted-foreground">{recordedAge(artifact.createdAt) + ' ago'}</span>
      </td>
      <td data-label="Size" className="min-w-0 px-3 py-3">
        {formatArtifactBytes(artifact.bytes)}
      </td>
      <td data-label="Schema" className="min-w-0 px-3 py-3">
        {String(artifact.schemaVersion)}
      </td>
      <td data-label="Data window" className="min-w-0 px-3 py-3 text-xs">
        {formatDataRange(artifact)}
      </td>
      <td data-label="Server version" className="min-w-0 px-3 py-3 font-mono text-xs">
        {artifact.serverVersion}
      </td>
      <td data-label="Creating task" className="min-w-0 px-3 py-3 text-xs">
        {artifact.createOperationId ? (
          <Link
            className="inline-flex min-h-11 items-center font-mono text-xs underline underline-offset-4"
            to={operationHref(artifact.createOperationId)}
          >
            {shortId(artifact.createOperationId)}
          </Link>
        ) : (
          NOT_RECORDED
        )}
      </td>
      <td data-label="Verification" className="min-w-0 px-3 py-3">
        <span data-slot="backup-verification-state" className="block min-w-0">
          <StatusBadge status={reading.label} tone={reading.tone} />
          <span className="mt-1 block text-xs text-muted-foreground">
            {artifact.verifiedAt
              ? 'Last checked ' + recordedAge(artifact.verifiedAt) + ' ago'
              : 'No verification outcome recorded'}
          </span>
        </span>
      </td>
    </tr>
  )
}

export function AdminBackupDetailPage() {
  const { generation } = useAuth()
  const { artifactId = '' } = useParams()
  const backup = useAdminBackup(generation, artifactId)
  const notFound =
    backup.isError &&
    backup.error instanceof AdminApiError &&
    (backup.error.code === 'backup_artifact_not_found' || backup.error.code === 'not_found')

  if (notFound) {
    return (
      <section className="w-full min-w-0 space-y-3">
        <h1 className="text-xl font-semibold tracking-tight">Backup artifact not found</h1>
        <p className="text-sm text-muted-foreground">
          The Server has no recorded artifact with this id. Artifacts are recorded when an offline backup run
          writes one into the configured backup directory, and this page never creates one.
        </p>
        <Link className="inline-flex min-h-11 items-center underline underline-offset-4" to="/admin/backups">
          Back to Backups
        </Link>
      </section>
    )
  }

  if (!backup.data) {
    return (
      <section className="w-full min-w-0 space-y-3">
        <h1 className="text-xl font-semibold tracking-tight">Backup artifact</h1>
        <p
          role={backup.isError ? 'alert' : 'status'}
          className="flex flex-wrap items-center gap-2 text-sm"
        >
          <StatusBadge
            status={backup.isError ? 'Error' : 'Starting'}
            tone={backup.isError ? 'error' : 'neutral'}
          />
          {backup.isError
            ? errorMessage(backup.error, 'Unable to load the backup artifact')
            : 'Loading the recorded artifact…'}
        </p>
        {backup.isError && (
          <Button variant="link" size="sm" onClick={() => void backup.refetch()}>
            Try again
          </Button>
        )}
      </section>
    )
  }

  const artifact = backup.data.artifact

  return (
    <section className="w-full min-w-0 space-y-4">
      <div className="space-y-3">
        <h1 className="text-xl font-semibold tracking-tight break-all font-mono">{artifact.filename}</h1>
        <p className="text-sm text-muted-foreground">
          The manifest metadata the Server recorded for this artifact, and the outcome of the last verification
          that ran against it. Restoring is a separate, deliberate operation that this page does not perform.
        </p>
        <Link className="inline-flex min-h-11 items-center underline underline-offset-4" to="/admin/backups">
          Back to Backups
        </Link>
      </div>

      {backup.isRefetchError && (
        <p role="alert" className="text-sm">
          Failed to refresh; showing the last successful recorded artifact.
        </p>
      )}

      <CardX size="medium" className={CARD_SURFACE} title="Recorded artifact">
        <DetailList>
          <DetailItem label="Artifact id">
            <span className="font-mono text-xs break-all">{artifact.artifactId}</span>
          </DetailItem>
          <DetailItem label="File name">
            <span className="font-mono text-xs break-all">{artifact.filename}</span>
          </DetailItem>
          <DetailItem label="Size">{formatArtifactBytes(artifact.bytes)}</DetailItem>
          <DetailItem label="SHA-256">
            <span className="font-mono text-xs break-all">{artifact.sha256}</span>
          </DetailItem>
          <DetailItem label="Schema version">{String(artifact.schemaVersion)}</DetailItem>
          <DetailItem label="Server version">
            <span className="font-mono text-xs break-all">{artifact.serverVersion}</span>
          </DetailItem>
          <DetailItem label="Created">{formatObservedAt(artifact.createdAt)}</DetailItem>
          <DetailItem label="Data window">{formatDataRange(artifact)}</DetailItem>
          <DetailItem label="Created by task">
            {artifact.createOperationId ? (
              <Link
                className="inline-flex min-h-11 items-center underline underline-offset-4"
                to={operationHref(artifact.createOperationId)}
              >
                {shortId(artifact.createOperationId)}
              </Link>
            ) : (
              NOT_RECORDED
            )}
          </DetailItem>
        </DetailList>
      </CardX>

      {/* Keyed by identity: another artifact or another session must never
          inherit the accepted task and feedback of the one before it. */}
      <VerificationCard
        key={generation + ':' + artifact.artifactId}
        artifact={artifact}
        verificationError={backup.data.verificationError}
      />

      <p className="text-xs text-muted-foreground">{BACKUP_SURFACE_SCOPE_NOTE}</p>
    </section>
  )
}

function VerificationCard({
  artifact,
  verificationError,
}: {
  artifact: BackupArtifactSummary
  verificationError?: string | null
}) {
  const { generation, status } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const [pending, setPending] = useState(false)
  const [feedback, setFeedback] = useState<FormFeedback | null>(null)
  const [acceptedOperationId, setAcceptedOperationId] = useState<string | null>(null)
  const reading = readVerification(artifact.verification, verificationError)
  // The card is keyed by artifact and session, so a late response can only
  // ever land on the card that sent it; this ref keeps it from writing into a
  // card React has already detached.
  const mounted = useRef(true)
  useEffect(() => {
    // Setup restores the flag: an effect that only cleared it on cleanup would
    // stay false after React re-runs it (StrictMode remounts effects in
    // development), and every later response would be dropped.
    mounted.current = true
    return () => {
      mounted.current = false
    }
  }, [])

  // The task this page reports on: the one it just accepted, or the last one
  // the Server linked to this artifact. Reading the task's own recorded state
  // is what keeps the card truthful after acceptance — a cancelled or failed
  // task writes no artifact outcome, so the artifact keeps its last recorded
  // one and only this task's own status can say what happened to the request.
  const taskOperationId = acceptedOperationId ?? artifact.verifyOperationId ?? ''
  const task = useAdminOperation(generation, taskOperationId, true)
  const taskDetail = task.data ?? null
  const taskStatus = taskDetail?.operation.status ?? null
  // Whether the artifact record on screen is the one this task wrote: a task
  // that ran and wrote an outcome is not the same fact as an artifact record
  // that already points at it.
  const linkedToArtifact = taskOperationId !== '' && artifact.verifyOperationId === taskOperationId
  const taskNote = readTaskLifecycle({
    status: taskStatus,
    loaded: taskDetail !== null,
    unreadable: task.isError,
    linkedToArtifact,
    wroteOutcome: taskDetail?.result != null,
    artifactOutcome: artifact.verification,
  })

  // Once the accepted task is terminal it has written whatever it will write,
  // so the artifact record is re-read rather than assumed to have caught up.
  const taskIsTerminal =
    taskStatus === 'succeeded' ||
    taskStatus === 'succeeded_with_warnings' ||
    taskStatus === 'failed' ||
    taskStatus === 'cancelled'
  useEffect(() => {
    if (acceptedOperationId === null || !taskIsTerminal) return
    refetchAdminBackup(artifact.artifactId)
  }, [acceptedOperationId, taskIsTerminal, artifact.artifactId])

  async function onVerify() {
    if (csrfToken.length === 0) return
    setPending(true)
    setFeedback(null)
    try {
      const result = await verifyBackupEntry(artifact.artifactId, csrfToken)
      if (!mounted.current) return
      const operationId = result.operation.operation.operationId
      setAcceptedOperationId(operationId)
      setFeedback({
        tone: 'ok',
        message:
          'The Server accepted the request and queued a verification task. Acceptance is not a result: the ' +
          'recorded state above stays as the Server last wrote it until a verification task records an outcome ' +
          'for this artifact.',
      })
    } catch (error) {
      if (!mounted.current) return
      if (indeterminateOutcome(error)) {
        setFeedback({ tone: 'error', message: INDETERMINATE_OUTCOME })
      } else if (error instanceof AdminApiError && error.code === 'backup_artifact_not_found') {
        setFeedback({
          tone: 'error',
          message:
            error.message +
            ' The Server has no recorded artifact with this id, so no verification could be queued.',
        })
      } else {
        setFeedback({ tone: 'error', message: errorMessage(error, 'Unable to request verification') })
      }
    } finally {
      if (mounted.current) setPending(false)
    }
  }

  return (
    <CardX size="medium" className={CARD_SURFACE} title="Verification">
      <div className="space-y-3">
        <div data-slot="backup-verification" className="space-y-2">
          <p className="flex flex-wrap items-center gap-2 text-sm">
            <StatusBadge status={reading.label} tone={reading.tone} />
            <span className="text-muted-foreground">
              {artifact.verifiedAt
                ? 'Last checked ' + formatObservedAt(artifact.verifiedAt) + ' (' + recordedAge(artifact.verifiedAt) + ' ago)'
                : 'No verification has recorded an outcome for this artifact yet.'}
            </span>
          </p>
          {reading.reading && <p className="text-sm">{reading.reading}</p>}
          {(artifact.verification === 'failed' || Boolean(verificationError)) && (
            <p className="text-sm">
              <span className="block text-xs font-medium tracking-wider text-muted-foreground">
                Reason the Server recorded
              </span>
              <span data-slot="backup-verification-reason" className="block break-words font-mono text-xs">
                {verificationError ? verificationError : NOT_RECORDED}
              </span>
            </p>
          )}
          {artifact.verification === 'failed' && !reading.reading && (
            <p className="text-sm text-muted-foreground">
              {verificationError
                ? "The recorded reason above is the Server's own text; this page has no plainer reading for this shape."
                : 'The Server recorded the failure without a reason text, so the reason stays Not recorded rather than being invented here.'}
            </p>
          )}
        </div>

        <p className="text-sm text-muted-foreground">{VERIFICATION_SCOPE_NOTE}</p>

        <div className="space-y-2">
          <Button size="sm" className="min-h-11" disabled={pending || csrfToken.length === 0} onClick={() => void onVerify()}>
            {pending ? 'Requesting verification…' : 'Request verification'}
          </Button>
          <p className="text-xs text-muted-foreground">
            The Server re-reads the file and checks its checksum, read-only integrity, schema version, and privacy
            redaction. This reads the artifact; it never writes to it and never touches the running Server.
          </p>
        </div>

        {taskOperationId !== '' && (
          <div
            data-slot="backup-verify-task"
            className="space-y-2 rounded-md border border-border/60 bg-muted/20 p-3"
          >
            <p className="flex flex-wrap items-center gap-2 text-sm">
              <span className="text-xs font-medium tracking-wider text-muted-foreground">Verification task</span>
              <Link
                className="inline-flex min-h-11 items-center font-mono text-xs underline underline-offset-4"
                to={operationHref(taskOperationId)}
              >
                {shortId(taskOperationId)}
              </Link>
              {taskDetail ? (
                <OperationStatus operation={taskDetail.operation} />
              ) : (
                <StatusBadge
                  status={task.isError ? 'Unknown' : 'Loading'}
                  tone={task.isError ? 'warning' : 'neutral'}
                />
              )}
            </p>
            <p role="status" data-slot="backup-verify-task-state" className="text-sm text-muted-foreground">
              {taskNote}
            </p>
            {taskDetail && <OperationProgress operation={taskDetail.operation} />}
            {taskDetail && taskDetail.errors.length > 0 && (
              <div className="space-y-1">
                <span className="block text-xs font-medium tracking-wider text-muted-foreground">
                  Errors the Server recorded for this task
                </span>
                <IssueList issues={taskDetail.errors} label="errors" />
              </div>
            )}
            {taskDetail && taskDetail.warnings.length > 0 && (
              <IssueList issues={taskDetail.warnings} label="warnings" />
            )}
            {taskDetail && taskDetail.result != null && (
              <div className="space-y-1">
                <span className="block text-xs font-medium tracking-wider text-muted-foreground">
                  Result the Server recorded for this task
                </span>
                <ResultBlock result={taskDetail.result} />
              </div>
            )}
          </div>
        )}

        <FormFeedbackNote feedback={feedback} />
      </div>
    </CardX>
  )
}
