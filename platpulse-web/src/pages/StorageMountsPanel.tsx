import { useAuth } from '../auth/AuthContext'
import { useAdminAgentStorageMounts } from '../api/admin'
import { Button } from '../components/ui/button'
import { CardX } from '../components/ui/card-x'
import { Empty } from '../components/ui/empty'
import { StatusBadge } from '../components/StatusBadge'
import { cn } from '../lib/utils'
import { SURFACE_CARD_STATIC } from '../lib/surface'
import { storageMountsView, type StorageSeriesSummary } from '../storageMounts'

const CARD_SURFACE = cn('rounded-md border-none', SURFACE_CARD_STATIC)

const TH = 'px-3 py-2 text-left text-xs font-medium text-muted-foreground'
const TD = 'px-3 py-2 align-top'

/**
 * The mount paths this Agent reported, and what each path's storage series hold
 * (issue #216, design §11.6). A storage series is named by the mount path, so
 * without this list the Operator has to guess the exact string, and a path the
 * Agent stopped reporting becomes unreachable exactly when it matters. The list
 * comes from the stored evidence and never from a device identifier: a path is
 * compared literally, and two filesystems that swap places behind one path are
 * not claimed to be distinguishable.
 */
const STORAGE_MOUNTS_INTRO =
  'Every mount path this Agent reported, taken from the stored evidence rather than from a device identity. A path is the whole name of a storage series: a path that changed is a different series, and two spellings of one filesystem are two series.'

/** One side of one mount: how much evidence the series holds, and its newest reading. */
function SeriesCell({ series }: { series: StorageSeriesSummary }) {
  return (
    <div className="space-y-0.5">
      <div className="text-sm">{series.coverage}</div>
      <div className="break-all text-xs text-muted-foreground">{series.latest}</div>
      {series.evidence && <div className="text-xs text-muted-foreground">{series.evidence}</div>}
      {series.firstObservedAt && (
        <div className="text-[11px] text-muted-foreground">
          First observed {series.firstObservedAt}
          {series.lastObservedAt ? ' · newest instant ' + series.lastObservedAt : ''}
        </div>
      )}
    </div>
  )
}

export function StorageMountsPanel({
  agentId,
  onReadSeries,
}: {
  agentId: string
  /** Read one path's storage series beside this list: the path is chosen here
   * rather than typed from memory. */
  onReadSeries?: (mountPath: string) => void
}) {
  const { generation } = useAuth()
  const query = useAdminAgentStorageMounts(generation, agentId)
  const view = query.data ? storageMountsView(query.data) : null
  // A failed refresh keeps the last good answer on the card, but it is no longer
  // a fresh answer: the card says so and offers the retry instead of showing the
  // stored answer as if nothing had gone wrong.
  const refreshFailed = query.isError && Boolean(query.data)
  const state = query.isPending && !query.data
    ? 'Starting'
    : query.isError && !query.data
      ? 'Error'
      : !view
        ? 'Unknown'
        : view.mounts.length === 0
          ? 'Empty'
          : 'Current'
  const tone =
    state === 'Error' ? 'error' : state === 'Current' && !refreshFailed ? 'ok' : 'neutral'

  return (
    <CardX
      size="medium"
      className={CARD_SURFACE}
      header={
        <>
          <h2 className="text-lg font-semibold">Storage by mount path</h2>
          <StatusBadge status={state} tone={tone} />
        </>
      }
      data-slot="storage-mounts-panel"
    >
      <p className="text-sm text-muted-foreground">{STORAGE_MOUNTS_INTRO}</p>
      {!query.data && query.isPending && (
        <div role="status" className="flex items-center gap-2 py-6 text-sm text-muted-foreground">
          <StatusBadge status="Starting" tone="neutral" /> Loading the mount paths…
        </div>
      )}
      {!query.data && query.isError && (
        <div role="alert" className="flex flex-wrap items-center gap-2 py-6 text-sm">
          <StatusBadge status="Error" tone="error" />
          <span className="text-destructive">
            {query.error instanceof Error
              ? query.error.message
              : 'Unable to load the storage mount paths'}
          </span>
          <Button variant="ghost" size="sm" onClick={() => void query.refetch()}>
            Try again
          </Button>
        </div>
      )}
      {view && refreshFailed && (
        <div
          data-slot="storage-mounts-refresh-error"
          role="alert"
          className="mt-3 flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
        >
          <StatusBadge status="Error" tone="error" />
          <span className="min-w-0 break-words">
            Failed to refresh the mount paths; showing the answer stored at {view.answeredAt}.
          </span>
          <Button variant="link" size="sm" onClick={() => void query.refetch()}>
            Try again
          </Button>
        </div>
      )}
      {view && (
        <>
          <ul className="mt-3 space-y-1 text-xs text-muted-foreground">
            <li data-slot="storage-mounts-cadence">{view.cadence}</li>
            <li data-slot="storage-mounts-silence">{view.silenceThreshold}</li>
            <li data-slot="storage-mounts-coverage">{view.order}</li>
            <li data-slot="storage-mounts-limit">{view.coverageLimit}</li>
            {view.truncation && (
              <li data-slot="storage-mounts-truncation" className="text-destructive" role="status">
                {view.truncation}
              </li>
            )}
            {view.pause && (
              <li data-slot="storage-mounts-pause" role="status">
                {view.pause}
              </li>
            )}
            <li data-slot="storage-mounts-answered">
              Assembled from the stored evidence at {view.answeredAt}. Used bytes are read from{' '}
              <span className="font-mono">{view.usedMetric}</span> and capacity from{' '}
              <span className="font-mono">{view.capacityMetric}</span>.
            </li>
          </ul>
          {view.mounts.length === 0 ? (
            <div className="mt-3">
              <Empty description="This Agent has reported no mount path yet, so no storage series exists for it." />
            </div>
          ) : (
            <div
              data-slot="storage-mounts-list"
              className="mt-3 overflow-x-auto"
              role="region"
              aria-label="Storage mount paths"
              tabIndex={0}
            >
              <table
                data-stack
                data-slot="storage-mounts-table"
                className="w-full text-sm md:min-w-[52rem]"
              >
                <caption className="sr-only">
                  Mount paths reported by this Agent, with the observation state of each path and
                  the coverage of its used-bytes and capacity series
                </caption>
                <thead>
                  <tr className="border-b border-border">
                    <th scope="col" className={TH}>
                      Mount path
                    </th>
                    <th scope="col" className={TH}>
                      Observation
                    </th>
                    <th scope="col" className={TH}>
                      Used
                    </th>
                    <th scope="col" className={TH}>
                      Capacity
                    </th>
                    {onReadSeries && (
                      <th scope="col" className={TH}>
                        Series
                      </th>
                    )}
                  </tr>
                </thead>
                <tbody>
                  {view.mounts.map((mount) => (
                    <tr
                      key={mount.mountPath}
                      data-slot="storage-mount-row"
                      data-mount-path={mount.mountPath}
                      data-mount-state={mount.state}
                      className="border-b border-border/60 last:border-b-0"
                    >
                      <th scope="row" data-label="Mount path" className={TD + ' text-left font-normal'}>
                        <span className="break-all font-mono text-sm">{mount.mountPath}</span>
                      </th>
                      <td data-label="Observation" className={TD}>
                        <div className="flex flex-wrap items-center gap-2">
                          <StatusBadge status={mount.stateLabel} tone={mount.tone} />
                          {mount.share && (
                            <span className="text-xs text-muted-foreground">{mount.share}</span>
                          )}
                        </div>
                        <p className="mt-1 text-xs text-muted-foreground">{mount.observation}</p>
                      </td>
                      <td data-label="Used" className={TD}>
                        <SeriesCell series={mount.used} />
                      </td>
                      <td data-label="Capacity" className={TD}>
                        <SeriesCell series={mount.capacity} />
                      </td>
                      {onReadSeries && (
                        <td data-label="Series" className={TD}>
                          <Button
                            variant="outline"
                            size="sm"
                            className="min-h-11"
                            aria-label={'Read the storage series of ' + mount.mountPath}
                            onClick={() => onReadSeries(mount.mountPath)}
                          >
                            Read series
                          </Button>
                        </td>
                      )}
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </>
      )}
    </CardX>
  )
}
