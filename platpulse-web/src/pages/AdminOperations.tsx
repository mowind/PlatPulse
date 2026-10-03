import { useEffect, useState } from 'react'
import { Link, useParams, useSearchParams } from 'react-router'

import {
  AdminApiError,
  cancelOperationEntry,
  operationKindLabel,
  useAdminOperation,
  useAdminOperations,
} from '../api/admin'
import type { OperationSummary } from '../api/generated'
import { useAuth } from '../auth/AuthContext'
import { StatusBadge, formatObservedAt } from '../components/StatusBadge'
import { Button } from '../components/ui/button'
import { CardX } from '../components/ui/card-x'
import { Empty } from '../components/ui/empty'
import { Select } from '../components/ui/input'
import {
  CARD_SURFACE,
  DetailItem,
  DetailList,
  FormFeedbackNote,
  INDETERMINATE_OUTCOME,
  IssueList,
  OPERATION_KINDS,
  OPERATION_STATUSES,
  CancellationSummary,
  OperationProgress,
  OperationStatus,
  ResultBlock,
  errorMessage,
  indeterminateOutcome,
  operationDuration,
  shortId,
  type FormFeedback,
} from './operationsShared'

/**
 * PAGE-ADMIN-OPERATIONS / PAGE-ADMIN-OPERATION-DETAIL (webui.md §15.11,
 * issue #208). The Server owns the Operation ledger: a command returns an
 * Operation reference immediately, the worker records the outcome later, and
 * this page renders exactly what the Server sent. A recorded cancel request
 * is never presented as a completed task, and there is no delete, repair, or
 * re-run control anywhere on either page.
 */

const PAGE_SIZES = [25, 50, 100, 200] as const

function readChoice(
  search: URLSearchParams,
  key: string,
  allowed: readonly string[],
): string {
  const value = search.get(key) ?? 'all'
  return allowed.includes(value) ? value : 'all'
}

function readLimit(search: URLSearchParams): number {
  const value = Number.parseInt(search.get('limit') ?? '', 10)
  return PAGE_SIZES.includes(value as (typeof PAGE_SIZES)[number]) ? value : 50
}

export default function AdminOperations() {
  const { generation } = useAuth()
  const [search, setSearch] = useSearchParams()
  const status = readChoice(search, 'status', OPERATION_STATUSES)
  const kind = readChoice(search, 'kind', OPERATION_KINDS)
  const limit = readLimit(search)
  // The Admin SSE stream carries invalidation only, so the poll is what makes
  // a queued or running task observable without a manual reload.
  const operations = useAdminOperations(
    generation,
    {
      status: status === 'all' ? undefined : status,
      kind: kind === 'all' ? undefined : kind,
      limit,
    },
    true,
  )

  function setParam(key: string, value: string | null) {
    const next = new URLSearchParams(search)
    if (value === null) next.delete(key)
    else next.set(key, value)
    setSearch(next, { replace: false })
  }

  const rows = operations.data ?? []

  return (
    <section className="w-full min-w-0 space-y-4">
      <div className="space-y-3">
        <h1 className="text-xl font-semibold tracking-tight">Operations</h1>
        <p className="text-sm text-muted-foreground">
          Every server-side task is recorded here: retention runs, backup verification, restores, and Doctor
          runs. The Server owns the ledger, so a task stays Queued or Running until its worker records the
          outcome, and a cancel request stays a request until then. Reloading this page, or returning to it
          later, shows the same recorded state.
        </p>
      </div>

      <form
        className={'flex flex-wrap items-end gap-3 rounded-md border-none p-3 bg-background/60'}
        role="group"
        aria-label="Operation filters"
      >
        <label
          className="text-xs font-medium tracking-wider text-muted-foreground"
          htmlFor="operation-status-filter"
        >
          Status
          <Select
            id="operation-status-filter"
            className="mt-1 min-h-11"
            value={status}
            onChange={(event) => setParam('status', event.target.value === 'all' ? null : event.target.value)}
          >
            <option value="all">all</option>
            {OPERATION_STATUSES.map((value) => (
              <option key={value} value={value}>
                {value}
              </option>
            ))}
          </Select>
        </label>
        <label
          className="text-xs font-medium tracking-wider text-muted-foreground"
          htmlFor="operation-kind-filter"
        >
          Kind
          <Select
            id="operation-kind-filter"
            className="mt-1 min-h-11"
            value={kind}
            onChange={(event) => setParam('kind', event.target.value === 'all' ? null : event.target.value)}
          >
            <option value="all">all</option>
            {OPERATION_KINDS.map((value) => (
              <option key={value} value={value}>
                {value}
              </option>
            ))}
          </Select>
        </label>
        <label
          className="text-xs font-medium tracking-wider text-muted-foreground"
          htmlFor="operation-limit-filter"
        >
          Page size
          <Select
            id="operation-limit-filter"
            className="mt-1 min-h-11"
            value={String(limit)}
            onChange={(event) => setParam('limit', event.target.value)}
          >
            {PAGE_SIZES.map((value) => (
              <option key={value} value={value}>
                {value}
              </option>
            ))}
          </Select>
        </label>
      </form>

      {!operations.data && operations.isPending && (
        <p role="status" className="text-sm">
          <StatusBadge status="Starting" tone="neutral" /> Loading the Operations ledger…
        </p>
      )}
      {!operations.data && operations.isError && (
        <div role="alert" className="space-y-2 text-sm">
          <p>{errorMessage(operations.error, 'Unable to load Operations')}</p>
          <Button variant="link" size="sm" onClick={() => void operations.refetch()}>
            Try again
          </Button>
        </div>
      )}
      {operations.data && operations.isRefetchError && (
        <p role="alert" className="text-sm">
          Failed to refresh; showing the last successful Operation list.
        </p>
      )}
      {operations.data && rows.length === 0 && (
        <CardX size="medium" className={CARD_SURFACE}>
          <Empty description="No Operations match the current filters." />
        </CardX>
      )}
      {operations.data && rows.length > 0 && (
        <CardX
          size="medium"
          className={CARD_SURFACE}
          contentClassName="p-0"
          segmented
          title={'Operation ledger · ' + String(rows.length)}
        >
          <div className="overflow-x-auto">
            <table data-stack data-slot="operations-table" className="w-full text-sm">
              <caption className="sr-only">Recorded Operations</caption>
              <thead>
                <tr className="border-b">
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Task
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Kind
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Status
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Progress
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Started
                  </th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                    Finished
                  </th>
                </tr>
              </thead>
              <tbody>
                {rows.map((operation) => (
                  <tr key={operation.operationId} className="border-b border-border/60 align-top">
                    <th scope="row" data-label="Task" className="min-w-0 px-3 py-3 text-left font-normal">
                      <Link
                        className="inline-flex min-h-11 items-center underline underline-offset-4"
                        to={'/admin/operations/' + operation.operationId}
                      >
                        {shortId(operation.operationId)}
                      </Link>
                      <span className="block text-xs text-muted-foreground">
                        {formatObservedAt(operation.createdAt)}
                      </span>
                    </th>
                    <td data-label="Kind" className="min-w-0 px-3 py-3">
                      {operationKindLabel(operation.kind)}
                    </td>
                    <td data-label="Status" className="min-w-0 px-3 py-3">
                      <OperationStatus operation={operation} />
                    </td>
                    <td data-label="Progress" className="min-w-0 px-3 py-3">
                      <OperationProgress operation={operation} />
                    </td>
                    <td data-label="Started" className="min-w-0 px-3 py-3">
                      {operation.startedAt ? formatObservedAt(operation.startedAt) : 'Not started yet'}
                    </td>
                    <td data-label="Finished" className="min-w-0 px-3 py-3">
                      {operation.finishedAt ? formatObservedAt(operation.finishedAt) : 'Not finished yet'}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </CardX>
      )}
      {operations.data && rows.length > 0 && (
        <p className="text-xs text-muted-foreground">
          The Server returns the newest {String(limit)} matching Operations, so this page is a bounded window
          and not a complete history. Progress applies while a task is queued or running; once the outcome is
          recorded the Server stops tracking a percentage and the detail page shows the outcome and duration
          instead, so a finished task never renders a stale 0% as if it were Unknown.
        </p>
      )}
    </section>
  )
}

function CancelPanel({ detail }: { detail: { operation: OperationSummary; cancellable: boolean } }) {
  const { status } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const [confirming, setConfirming] = useState(false)
  const [pending, setPending] = useState(false)
  const [feedback, setFeedback] = useState<FormFeedback | null>(null)
  const operation = detail.operation

  // The Server owns cancellability. A poll that records a cancel request or a
  // terminal status disarms the armed confirmation instead of leaving a
  // command the Server has already said it will refuse.
  useEffect(() => {
    if (!detail.cancellable) setConfirming(false)
  }, [detail.cancellable])

  async function onCancel() {
    if (!detail.cancellable) {
      setConfirming(false)
      return
    }
    setPending(true)
    setFeedback(null)
    try {
      const result = await cancelOperationEntry(operation.operationId, csrfToken)
      setConfirming(false)
      setFeedback({
        tone: 'ok',
        message:
          'The Server recorded the cancel request. A queued task becomes Cancelled immediately; a running task ' +
          'keeps its recorded status until the worker stops it and writes the outcome.',
      })
      return result
    } catch (error) {
      if (indeterminateOutcome(error)) {
        setFeedback({ tone: 'error', message: INDETERMINATE_OUTCOME })
      } else if (
        error instanceof AdminApiError &&
        error.code === 'operation_not_cancellable'
      ) {
        setConfirming(false)
        setFeedback({
          tone: 'error',
          message:
            error.message +
            ' The recorded status is authoritative; read it above instead of assuming the request took effect.',
        })
      } else {
        setFeedback({ tone: 'error', message: errorMessage(error, 'Unable to request cancellation') })
      }
      return null
    } finally {
      setPending(false)
    }
  }

  return (
    <CardX size="medium" className={CARD_SURFACE} title="Cancel this task">
      <div className="space-y-3">
        <p className="text-sm text-muted-foreground">
          Cancelling asks the Server to stop this task. It never deletes recorded history, and it is not a
          completion: the row keeps the status the Server recorded until the worker stops the work and writes
          the terminal outcome.
        </p>
        {!confirming ? (
          <Button
            size="sm"
            className="min-h-11"
            disabled={!detail.cancellable || pending || csrfToken.length === 0}
            onClick={() => setConfirming(true)}
          >
            Cancel task
          </Button>
        ) : (
          <div className="space-y-2">
            <p role="status" className="text-sm">
              Request cancellation of {shortId(operation.operationId)}?
            </p>
            <div className="flex flex-wrap gap-2">
              <Button
                size="sm"
                className="min-h-11"
                disabled={pending || csrfToken.length === 0 || !detail.cancellable}
                onClick={() => void onCancel()}
              >
                {pending ? 'Requesting cancel…' : 'Confirm cancel request'}
              </Button>
              <Button
                variant="outline"
                size="sm"
                className="min-h-11"
                disabled={pending}
                onClick={() => setConfirming(false)}
              >
                Keep running
              </Button>
            </div>
          </div>
        )}
        {!detail.cancellable && !confirming && (
          <p role="status" className="text-sm text-muted-foreground">
            The Server will not accept a cancel request for this task in its recorded state, so no control is
            offered here.
          </p>
        )}
        <FormFeedbackNote feedback={feedback} />
      </div>
    </CardX>
  )
}

export function AdminOperationDetailPage() {
  const { generation } = useAuth()
  const { operationId = '' } = useParams()
  const operation = useAdminOperation(generation, operationId, true)
  const notFound =
    operation.isError &&
    operation.error instanceof AdminApiError &&
    (operation.error.code === 'operation_not_found' || operation.error.code === 'not_found')

  if (notFound) {
    return (
      <section className="w-full min-w-0 space-y-3">
        <h1 className="text-xl font-semibold tracking-tight">Operation not found</h1>
        <p className="text-sm text-muted-foreground">
          The Server has no Operation with this id. It may never have existed, or the ledger may have been
          trimmed by retention.
        </p>
        <Link className="inline-flex min-h-11 items-center underline underline-offset-4" to="/admin/operations">
          Back to Operations
        </Link>
      </section>
    )
  }

  if (!operation.data) {
    return (
      <section className="w-full min-w-0 space-y-3">
        <h1 className="text-xl font-semibold tracking-tight">Operation detail</h1>
        <p role={operation.isError ? 'alert' : 'status'} className="flex flex-wrap items-center gap-2 text-sm">
          <StatusBadge status={operation.isError ? 'Error' : 'Starting'} tone={operation.isError ? 'error' : 'neutral'} />
          {/* A pending read has no error to report, so it never borrows the
              failure copy. */}
          {operation.isError
            ? errorMessage(operation.error, 'Unable to load the Operation')
            : 'Loading the recorded Operation...'}
        </p>
        {operation.isError && (
          <Button variant="link" size="sm" onClick={() => void operation.refetch()}>
            Try again
          </Button>
        )}
      </section>
    )
  }

  const detail = operation.data
  const summary = detail.operation

  return (
    <section className="w-full min-w-0 space-y-4">
      <div className="space-y-3">
        <h1 className="text-xl font-semibold tracking-tight">Operation detail</h1>
        <p className="text-sm text-muted-foreground">
          The recorded task, its progress, and the outcome the Server wrote. This page is read-and-command
          only: it never deletes the task, its history, or anything the task touched.
        </p>
        <Link className="inline-flex min-h-11 items-center underline underline-offset-4" to="/admin/operations">
          Back to Operations
        </Link>
      </div>

      {operation.isRefetchError && (
        <p role="alert" className="text-sm">
          Failed to refresh; showing the last successful Operation state.
        </p>
      )}

      <CardX size="medium" className={CARD_SURFACE} title="Recorded task">
        <div className="space-y-3">
          <OperationProgress operation={summary} />
          <DetailList>
            <DetailItem label="Task id">
              <span className="font-mono text-xs break-all">{summary.operationId}</span>
            </DetailItem>
            <DetailItem label="Kind">{operationKindLabel(summary.kind)}</DetailItem>
            <DetailItem label="Status">
              <OperationStatus operation={summary} />
            </DetailItem>
            <DetailItem label="Duration">{operationDuration(summary)}</DetailItem>
            <DetailItem label="Created">{formatObservedAt(summary.createdAt)}</DetailItem>
            <DetailItem label="Started">
              {summary.startedAt ? formatObservedAt(summary.startedAt) : 'Not started yet'}
            </DetailItem>
            <DetailItem label="Finished">
              {summary.finishedAt ? formatObservedAt(summary.finishedAt) : 'Not finished yet'}
            </DetailItem>
            <DetailItem label="Request id">
              {summary.requestId ? (
                <span className="font-mono text-xs break-all">{summary.requestId}</span>
              ) : (
                'Not recorded'
              )}
            </DetailItem>
            <DetailItem label="Audit entry">
              {summary.auditEventId ? (
                <Link
                  className="inline-flex min-h-11 items-center underline underline-offset-4"
                  to={'/admin/access/audit#event-' + String(summary.auditEventId)}
                >
                  Event {String(summary.auditEventId)}
                </Link>
              ) : (
                'Not recorded'
              )}
            </DetailItem>
          </DetailList>
          <p className="text-xs text-muted-foreground">
            Progress is only meaningful while the task is queued or running. After the Server records the
            outcome it stops tracking a percentage, so the outcome and the duration above are authoritative.
          </p>
        </div>
      </CardX>

      <CardX size="medium" className={CARD_SURFACE} title="Recorded outcome">
        <div className="space-y-3">
          <div className="space-y-2">
            <h2 className="text-sm font-medium">Warnings</h2>
            <IssueList issues={detail.warnings} label="warnings" />
          </div>
          <div className="space-y-2">
            <h2 className="text-sm font-medium">Errors</h2>
            <IssueList issues={detail.errors} label="errors" />
          </div>
          <div className="space-y-2">
            <h2 className="text-sm font-medium">Result</h2>
            {/* A cancelled cleanup carries its own recorded shape: work already
                released, work stopped at the checkpoint (issue #211). */}
            <CancellationSummary result={detail.result} />
            {detail.result == null ? (
              <p className="text-sm text-muted-foreground">
                The Server recorded no result payload for this task yet. A task that is still queued or running
                produces its result when the worker finishes.
              </p>
            ) : (
              <ResultBlock result={detail.result} />
            )}
          </div>
          <p className="text-xs text-muted-foreground">
            Warnings, errors, and the result payload are redacted by the Server before they are stored, so
            secrets never reach this page.
          </p>
        </div>
      </CardX>

      <CancelPanel detail={detail} />
    </section>
  )
}
