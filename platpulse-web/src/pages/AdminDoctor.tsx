import { useState } from 'react'
import { Link } from 'react-router'

import {
  doctorCheckStatusLabel,
  doctorCheckTone,
  operationKindLabel,
  runDoctorEntry,
  useAdminDoctor,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { StatusBadge, formatObservedAt } from '../components/StatusBadge'
import { Button } from '../components/ui/button'
import { CardX } from '../components/ui/card-x'
import { Empty } from '../components/ui/empty'
import {
  CARD_SURFACE,
  DetailItem,
  DetailList,
  FormFeedbackNote,
  INDETERMINATE_OUTCOME,
  OperationProgress,
  OperationStatus,
  errorMessage,
  indeterminateOutcome,
  recordedAge,
  shortId,
  type FormFeedback,
} from './operationsShared'

/**
 * PAGE-ADMIN-DOCTOR (webui.md §15.11, issue #208). Doctor diagnoses and
 * never repairs: a run reads the Server state, records checks, and stops. It
 * deletes nothing, migrates nothing, and rotates no secret, so starting a run
 * is a read-only command and deliberately carries no confirmation dialog.
 *
 * The page shows the in-flight run beside the last recorded report, so a
 * refresh mid-run still shows Queued or Running instead of pretending the
 * previous report is current.
 */

function RunPanel({
  onQueued,
  feedback,
}: {
  onQueued: (operationId: string, error?: unknown) => void
  feedback: FormFeedback | null
}) {
  const { status } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const [pending, setPending] = useState(false)

  async function onRun() {
    setPending(true)
    // One exit path: the Server either queued a run and names its id, or the
    // request failed and the page must say so without guessing.
    try {
      const result = await runDoctorEntry(csrfToken)
      onQueued(result.operation.operation.operationId)
    } catch (error) {
      onQueued('', error)
    } finally {
      setPending(false)
    }
  }

  return (
    <CardX size="medium" className={CARD_SURFACE} title="Run Doctor">
      <div className="space-y-3">
        <p className="text-sm text-muted-foreground">
          A run is queued on the Server and executed by its worker. It only reads: it never repairs, deletes,
          migrates, or rotates anything, so it needs no confirmation. The outcome is recorded as a task you can
          open at any time.
        </p>
        <Button
          size="sm"
          className="min-h-11"
          disabled={pending || csrfToken.length === 0}
          onClick={() => void onRun()}
        >
          {pending ? 'Queueing Doctor run…' : 'Run Doctor'}
        </Button>
        <FormFeedbackNote feedback={feedback} />
      </div>
    </CardX>
  )
}

export default function AdminDoctor() {
  const { generation } = useAuth()
  const doctor = useAdminDoctor(generation)
  const [feedback, setFeedback] = useState<FormFeedback | null>(null)

  function onQueued(operationId: string, error?: unknown) {
    if (error !== undefined) {
      setFeedback({
        tone: 'error',
        message: indeterminateOutcome(error)
          ? INDETERMINATE_OUTCOME
          : errorMessage(error, 'Unable to queue a Doctor run'),
      })
      return
    }
    setFeedback({
      tone: 'ok',
      message:
        'The Server queued a Doctor run as ' +
        operationId +
        '. It starts as Queued and becomes Running when a worker picks it up; the report below updates when the ' +
        'run records its outcome.',
    })
  }

  const overview = doctor.data
  const currentRun = overview?.currentRun ?? null
  const lastRun = overview?.lastRun ?? null
  const checks = overview?.checks ?? []

  return (
    <section className="w-full min-w-0 space-y-4">
      <div className="space-y-3">
        <h1 className="text-xl font-semibold tracking-tight">Doctor</h1>
        <p className="text-sm text-muted-foreground">
          Doctor diagnoses the Server and reports what it finds. It never repairs anything, so a failing check
          is information to act on from the page that owns the setting — not a fix this page performs.
        </p>
      </div>

      <RunPanel onQueued={onQueued} feedback={feedback} />

      {!overview && doctor.isPending && (
        <p role="status" className="text-sm">
          <StatusBadge status="Starting" tone="neutral" /> Loading the Doctor report…
        </p>
      )}
      {!overview && doctor.isError && (
        <div role="alert" className="space-y-2 text-sm">
          <p>{errorMessage(doctor.error, 'Unable to load the Doctor report')}</p>
          <Button variant="link" size="sm" onClick={() => void doctor.refetch()}>
            Try again
          </Button>
        </div>
      )}
      {overview && doctor.isRefetchError && (
        <p role="alert" className="text-sm">
          Failed to refresh; showing the last successful Doctor report.
        </p>
      )}

      {overview && currentRun && (
        <CardX
          size="medium"
          className={CARD_SURFACE}
          title="Run in flight"
          data-slot="doctor-current-run"
        >
          <div className="space-y-3">
            <DetailList>
              <DetailItem label="Task">
                <Link
                  className="inline-flex min-h-11 items-center underline underline-offset-4"
                  to={'/admin/operations/' + currentRun.operationId}
                >
                  {shortId(currentRun.operationId)}
                </Link>
              </DetailItem>
              <DetailItem label="Kind">{operationKindLabel(currentRun.kind)}</DetailItem>
              <DetailItem label="Status">
                <OperationStatus operation={currentRun} />
              </DetailItem>
              <DetailItem label="Queued at">{formatObservedAt(currentRun.createdAt)}</DetailItem>
            </DetailList>
            <OperationProgress operation={currentRun} />
            <p className="text-sm text-muted-foreground">
              {currentRun.status === 'queued'
                ? 'This run is Queued: the Server accepted it and a worker has not started it yet. The report below is still the previous one.'
                : 'This run is Running. The report below is still the previous one until this run records its outcome.'}
            </p>
          </div>
        </CardX>
      )}

      {overview && (
        <CardX size="medium" className={CARD_SURFACE} title="Last recorded report">
          <div className="space-y-3">
            {lastRun ? (
              <>
                <DetailList>
                  <DetailItem label="Task">
                    <Link
                      className="inline-flex min-h-11 items-center underline underline-offset-4"
                      to={'/admin/operations/' + lastRun.operationId}
                    >
                      {shortId(lastRun.operationId)}
                    </Link>
                  </DetailItem>
                  <DetailItem label="Status">
                    <OperationStatus operation={lastRun} />
                  </DetailItem>
                  <DetailItem label="Report age">
                    {recordedAge(lastRun.finishedAt ?? lastRun.createdAt)}
                  </DetailItem>
                  <DetailItem label="Recorded at">
                    {formatObservedAt(lastRun.finishedAt ?? lastRun.createdAt)}
                  </DetailItem>
                </DetailList>
                {currentRun && (
                  <p className="text-sm text-muted-foreground">
                    This report is the last one the Server recorded. A newer run is in flight and will replace it
                    when it finishes.
                  </p>
                )}
              </>
            ) : (
              <p className="text-sm text-muted-foreground">
                Doctor has not produced a report on this Server yet, so there are no checks to show. Running
                Doctor records the first one.
              </p>
            )}
          </div>
        </CardX>
      )}

      {overview && (
        <CardX
          size="medium"
          className={CARD_SURFACE}
          contentClassName="p-0"
          segmented
          title={'Checks · ' + String(checks.length)}
        >
          {checks.length === 0 ? (
            <Empty description="No checks are recorded in the last report. Run Doctor to produce one." />
          ) : (
            <div className="overflow-x-auto">
              <table data-stack data-slot="doctor-checks-table" className="w-full text-sm">
                <caption className="sr-only">Checks from the last recorded Doctor report</caption>
                <thead>
                  <tr className="border-b">
                    <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                      Check
                    </th>
                    <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                      Status
                    </th>
                    <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                      Detail
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {checks.map((check) => (
                    <tr key={check.checkId} className="border-b border-border/60 align-top">
                      <th scope="row" data-label="Check" className="min-w-0 px-3 py-3 text-left font-normal">
                        {check.label}
                        <span className="block font-mono text-xs text-muted-foreground">{check.checkId}</span>
                      </th>
                      <td data-label="Status" className="min-w-0 px-3 py-3">
                        <StatusBadge
                          status={doctorCheckStatusLabel(check.status)}
                          tone={doctorCheckTone(check.status)}
                        />
                      </td>
                      <td data-label="Detail" className="min-w-0 px-3 py-3 break-words">
                        {check.detail}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </CardX>
      )}

      {overview && checks.length > 0 && (
        <p className="text-xs text-muted-foreground">
          Every status word above is the Server&apos;s own. An unrecognised status is shown as Unknown rather
          than as a pass, and a Not configured check means the feature is off — it is not a failure.
        </p>
      )}
    </section>
  )
}
