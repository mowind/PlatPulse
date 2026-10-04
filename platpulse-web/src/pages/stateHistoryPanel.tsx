/**
 * Recorded sync and consensus state history (issue #217, design section 11.7).
 *
 * The card renders evidence rather than a reconstructed series. Every row is a state the
 * Server actually recorded; every silence keeps its kind, its reason and the observations it
 * swallowed; a stretch nobody observed is never bridged with a constant state. The newest
 * state is aged against the end of the window instead of the silence after it being claimed,
 * a retained value shows the age it really has, and a flag the collection could not read is
 * unknown rather than false.
 */
import type { ReactNode } from 'react'
import type { AdminStateHistory } from '../api/admin'
import { StatusBadge, formatObservedAt } from '../components/StatusBadge'
import { Button } from '../components/ui/button'
import { CardX } from '../components/ui/card-x'
import { Select } from '../components/ui/input'
import { SURFACE_CARD_STATIC } from '../lib/surface'
import { cn } from '../lib/utils'
import { METRIC_HISTORY_PRESETS, formatHistoryDuration } from '../metricHistory'
import {
  STATE_HISTORY_COMPONENTS,
  stateHistoryComponentLabel,
  stateHistoryView,
} from '../stateHistory'
import type { MetricHistoryWindow } from './metricHistoryPanel'

const CARD_SURFACE = cn('rounded-md border-none', SURFACE_CARD_STATIC)

/** The shape the card reads off a query. A TanStack Query result satisfies it as it is. */
export type StateHistoryAnswer = {
  data: AdminStateHistory | undefined
  isPending: boolean
  isError: boolean
  isRefetchError: boolean
  error: unknown
  refetch: () => void
}

export type StateHistoryBodyProps = {
  title: string
  subject: string
  surface: string
  intro: string
  component: string
  onComponent: (component: string) => void
  view: MetricHistoryWindow
  answer: StateHistoryAnswer
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

/** A count of recorded deliveries, or the honest absence of one. */
function counted(value: number, recorded: boolean): string {
  if (!recorded) return 'None recorded'
  return value === 1 ? '1 delivery' : value + ' deliveries'
}

export function StateHistoryBody({
  title,
  subject,
  surface,
  intro,
  component,
  onComponent,
  view: window,
  answer,
}: StateHistoryBodyProps) {
  const { data, isPending, isError, isRefetchError, error, refetch } = answer
  // Everything the card renders comes from the Server's own answer; there is no
  // derived series and no inferred state anywhere below.
  const view = data ? stateHistoryView(data) : null
  const state = !data
    ? isPending
      ? 'Loading'
      : 'No answer'
    : view && !view.observed
      ? 'Never observed'
      : 'Observed'
  const stateTone = !data ? 'neutral' : view && !view.observed ? 'neutral' : 'ok'
  const rows = view?.entries ?? []
  const gaps = view?.gaps ?? []

  return (
    <section className="contents" data-slot="state-history-panel" data-surface={surface}>
      <CardX
        size="medium"
        className={CARD_SURFACE}
        header={
          <>
            <h2 className="text-lg font-semibold">{title}</h2>
            <StatusBadge status={state} tone={stateTone} />
          </>
        }
      >
        <p className="text-sm text-muted-foreground">{intro}</p>
        <div className="mt-3 flex flex-wrap items-end gap-3">
          <label className="grid min-w-40 gap-1 text-xs font-medium text-muted-foreground">
            Component
            <Select
              className="min-h-11"
              value={component}
              onChange={(event) => onComponent(event.currentTarget.value)}
            >
              {STATE_HISTORY_COMPONENTS.map((item) => (
                <option key={item} value={item}>
                  {stateHistoryComponentLabel(item)}
                </option>
              ))}
            </Select>
          </label>
          <div className="flex flex-wrap gap-2" role="group" aria-label="History range">
            {METRIC_HISTORY_PRESETS.map((preset) => (
              <Button
                key={preset.label}
                variant={window.hours === preset.hours ? 'default' : 'outline'}
                aria-pressed={window.hours === preset.hours}
                className="min-h-11"
                onClick={() => window.selectRange(preset.hours)}
              >
                {preset.label}
              </Button>
            ))}
          </div>
          <Button variant="outline" className="min-h-11" onClick={window.reload}>
            Reload window
          </Button>
        </div>
        {!data && isPending && (
          <p
            className="mt-3 flex flex-wrap items-center gap-2 text-sm text-muted-foreground"
            role="status"
          >
            <StatusBadge status="Loading" tone="neutral" /> Loading the recorded states...
          </p>
        )}
        {!data && isError && (
          <div
            className="mt-3 flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
            role="alert"
          >
            <StatusBadge status="Error" tone="error" />{' '}
            <span className="min-w-0 break-words">
              {error instanceof Error ? error.message : 'Unable to load the recorded states'}
            </span>
            <Button variant="link" size="sm" onClick={() => void refetch()}>
              Try again
            </Button>
          </div>
        )}
        {data && isRefetchError && (
          <div
            className="mt-3 flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
            role="alert"
            data-slot="state-history-refresh-error"
          >
            <StatusBadge status="Error" tone="error" /> Failed to refresh; showing the last
            recorded states the Server answered with.
            <Button variant="link" size="sm" onClick={() => void refetch()}>
              Try again
            </Button>
          </div>
        )}
        {data && view && view.availabilityNotice && (
          <p
            className="mt-3 text-sm text-muted-foreground"
            role="status"
            data-slot="state-history-availability"
          >
            {view.availabilityNotice}
          </p>
        )}
        {data && view && (
          <>
            {view.unasked && (
              <p className="mt-3 text-sm text-muted-foreground" role="status" data-slot="state-history-unasked">
                {view.unasked}
              </p>
            )}
            <p className="mt-2 text-xs text-muted-foreground" data-slot="state-history-order">
              {view.order}
            </p>
            <div className="mt-3">
              <DetailList>
                <DetailItem label="Deliveries recorded">
                  {counted(view.entryCount, view.observed)}
                </DetailItem>
                <DetailItem label="Changes recorded">
                  {view.observed ? view.changeCount : 'None recorded'}
                </DetailItem>
                <DetailItem label="Anchors re-recorded">
                  {view.observed ? view.anchorCount : 'None recorded'}
                </DetailItem>
                <DetailItem label="First observed">{view.firstObservedAt ?? 'Never observed'}</DetailItem>
                <DetailItem label="Last observed">{view.lastObservedAt ?? 'Never observed'}</DetailItem>
                <DetailItem label="Anchor interval">
                  {view.anchorSeconds > 0
                    ? formatHistoryDuration(view.anchorSeconds) + ' at most'
                    : 'Not recorded'}
                </DetailItem>
                <DetailItem label="Reporting cadence">
                  {view.cadenceSeconds > 0 ? formatHistoryDuration(view.cadenceSeconds) : 'Unknown'}
                </DetailItem>
                <DetailItem label="Covered stretch">
                  {view.coverageSeconds > 0
                    ? formatHistoryDuration(view.coverageSeconds)
                    : 'No stretch proven'}
                </DetailItem>
                <DetailItem label="Retained window">{view.retentionDays + ' days'}</DetailItem>
                <DetailItem label="Released before">
                  {view.releasedBefore ?? 'Nothing released'}
                </DetailItem>
              </DetailList>
            </div>
            <p className="mt-3 text-sm text-muted-foreground" data-slot="state-history-anchor-rule">
              {view.anchor}
            </p>
            <p className="mt-2 text-sm text-muted-foreground" data-slot="state-history-cadence">
              {view.cadence}
            </p>
            <p className="mt-2 text-sm text-muted-foreground" data-slot="state-history-coverage">
              {view.coverage}
            </p>
            {view.newest && (
              <p className="mt-2 text-sm text-muted-foreground" role="status" data-slot="state-history-newest">
                {view.newest}
              </p>
            )}
            {view.latest && (
              <p className="mt-2 text-sm text-muted-foreground" data-slot="state-history-latest">
                {view.latest}
              </p>
            )}
            {(view.chart.markers.length > 0 || view.chart.gapBands.length > 0) && (
              <div className="mt-3 grid min-w-0 grid-cols-[3rem_minmax(0,1fr)] gap-x-2">
                <div
                  className="flex flex-col justify-between pr-1 text-right text-[11px] tabular-nums text-muted-foreground"
                  aria-hidden="true"
                >
                  <span>Change</span>
                  <span>Anchor</span>
                  <span>Base</span>
                </div>
                <svg
                  viewBox="0 0 600 150"
                  preserveAspectRatio="none"
                  role="img"
                  aria-label={
                    'Recorded ' + view.componentLabel + ' states of this ' +
                    subject.toLowerCase() + ': ' + rows.length + ' recorded states and ' +
                    gaps.length + ' silences across the window.'
                  }
                  className="col-start-2 h-40 w-full text-primary"
                  data-slot="state-history-chart"
                >
                  <title>Recorded {view.componentLabel} states</title>
                  <desc>
                    {view.order} {view.coverage}
                  </desc>
                  <g aria-hidden="true">
                    <line x1={0} x2={600} y1={8} y2={8} className="stroke-border [vector-effect:non-scaling-stroke]" />
                    <line x1={0} x2={600} y1={75} y2={75} className="stroke-border [vector-effect:non-scaling-stroke]" />
                    <line x1={0} x2={600} y1={142} y2={142} className="stroke-border [vector-effect:non-scaling-stroke]" />
                    {view.chart.provenBands.map((band) => (
                      <rect
                        key={'proven-' + band.x.toFixed(2)}
                        data-slot="state-history-proven"
                        className="fill-primary opacity-30"
                        x={band.x}
                        y={view.chart.provenY}
                        width={Math.max(1, band.width)}
                        height={view.chart.provenHeight}
                      />
                    ))}
                    {view.chart.gapBands.map((band) => (
                      <rect
                        key={'gap-' + band.x.toFixed(2)}
                        data-slot="state-history-gap-band"
                        className="fill-muted-foreground opacity-20"
                        x={band.x}
                        y={view.chart.gapY}
                        width={Math.max(1, band.width)}
                        height={view.chart.gapHeight}
                      />
                    ))}
                    {view.chart.markers.map((marker) => (
                      <rect
                        key={marker.observedAt + '-' + marker.entryKind}
                        data-slot={marker.anchor ? 'state-history-anchor' : 'state-history-entry'}
                        data-evidence={marker.evidence}
                        className={
                          marker.anchor
                            ? 'fill-muted-foreground'
                            : marker.tone === 'error'
                              ? 'fill-destructive'
                              : 'fill-primary'
                        }
                        x={Math.min(596, marker.x)}
                        y={view.chart.markerY}
                        width={4}
                        height={view.chart.markerHeight}
                      >
                        <title>{marker.title}</title>
                      </rect>
                    ))}
                  </g>
                </svg>
                <div
                  className="col-start-2 flex justify-between pt-1 text-[11px] tabular-nums text-muted-foreground"
                  aria-hidden="true"
                >
                  <span>{formatObservedAt(data.from)}</span>
                  <span>{formatObservedAt(data.to)}</span>
                </div>
              </div>
            )}
            {view.truncated && (
              <p className="mt-3 text-sm" role="status" data-slot="state-history-truncation">
                {view.truncation}
              </p>
            )}
            {view.truncation && data.continuation && (
              <Button
                variant="outline"
                className="mt-2 min-h-11"
                data-slot="state-history-older"
                onClick={() => window.setOlderThan(data.continuation ?? null)}
              >
                Load older states
              </Button>
            )}
            {window.olderThan !== null && (
              <p
                className="mt-3 flex flex-wrap items-center gap-2 text-sm text-muted-foreground"
                role="status"
                data-slot="state-history-page"
              >
                An older page of recorded states is shown.
                <Button
                  variant="outline"
                  size="sm"
                  className="min-h-11"
                  onClick={() => window.setOlderThan(null)}
                >
                  Return to the newest states
                </Button>
              </p>
            )}
            {view.pause && (
              <p className="mt-3 text-sm text-muted-foreground" role="status" data-slot="state-history-pause">
                {view.pause}
              </p>
            )}
            {gaps.length > 0 && (
              <div className="mt-4" data-slot="state-history-gaps">
                <h3 className="text-sm font-medium">Silences in this window</h3>
                <ul className="mt-1 space-y-1 text-sm text-muted-foreground">
                  {gaps.map((row) => (
                    <li
                      key={row.from + row.to}
                      data-slot="state-history-gap"
                      data-gap-kind={row.kind}
                    >
                      <span className="font-medium text-foreground">{row.kindLabel}</span>
                      {': '}
                      {formatObservedAt(row.from)} &rarr; {formatObservedAt(row.to)} (
                      {row.duration}) &middot; {row.reason}
                      {row.skippedCount != null
                        ? ' \u00b7 ' + row.skippedCount + ' observation(s) skipped behind it'
                        : ''}
                      <span className="block text-[11px] text-muted-foreground">{row.claim}</span>
                    </li>
                  ))}
                </ul>
              </div>
            )}
            {rows.length > 0 && (
              <div className="mt-4">
                <h3 className="text-sm font-medium">Recorded states in this window</h3>
                <div
                  className="mt-1 overflow-x-auto"
                  role="region"
                  aria-label={'Recorded ' + view.componentLabel + ' states'}
                  tabIndex={0}
                  data-slot="state-history-table-scroll"
                >
                  <table
                    data-stack
                    data-slot="state-history-samples"
                    className="w-full text-sm md:min-w-[64rem]"
                  >
                    <caption className="sr-only">
                      Recorded {view.componentLabel} states, with the evidence the Server kept for
                      each of them.
                    </caption>
                    <thead>
                      <tr className="text-xs font-medium tracking-wider text-muted-foreground">
                        <th scope="col" className="px-3 py-2 text-left">
                          State
                        </th>
                        <th scope="col" className="px-3 py-2 text-left">
                          Observed
                        </th>
                        <th scope="col" className="px-3 py-2 text-left">
                          Value age
                        </th>
                        <th scope="col" className="px-3 py-2 text-left">
                          Collection
                        </th>
                        <th scope="col" className="px-3 py-2 text-left">
                          Value source
                        </th>
                        <th scope="col" className="px-3 py-2 text-left">
                          Syncing
                        </th>
                        <th scope="col" className="px-3 py-2 text-left">
                          Failure
                        </th>
                        <th scope="col" className="px-3 py-2 text-left">
                          Delay
                        </th>
                        <th scope="col" className="px-3 py-2 text-left">
                          Clock
                        </th>
                      </tr>
                    </thead>
                    <tbody>
                      {rows.map((row) => (
                        <tr
                          key={row.observedAt + '-' + row.entryKind}
                          data-slot={row.anchor ? 'state-history-anchor' : 'state-history-entry'}
                          data-entry-kind={row.entryKind}
                          data-evidence={row.evidence}
                        >
                          <th scope="row" data-label="State" className="px-3 py-2 text-left align-top">
                            {row.entryKindLabel}
                          </th>
                          <td data-label="Observed" className="px-3 py-2 align-top tabular-nums">
                            {formatObservedAt(row.observedAt)}
                            <span className="block text-[11px] text-muted-foreground">
                              received {formatObservedAt(row.receivedAt)}
                            </span>
                          </td>
                          <td data-label="Value age" className="px-3 py-2 align-top">
                            {row.valueAge}
                          </td>
                          <td data-label="Collection" className="px-3 py-2 align-top">
                            {row.collection}
                          </td>
                          <td data-label="Value source" className="px-3 py-2 align-top">
                            {row.valueSourceLabel}
                            <span className="mt-1 block">
                              <StatusBadge status={row.evidenceLabel} tone={row.tone} />
                            </span>
                          </td>
                          <td data-label="Syncing" className="px-3 py-2 align-top">
                            {row.syncing}
                            <span className="block text-[11px] text-muted-foreground">
                              {row.syncingClaim}
                            </span>
                          </td>
                          <td data-label="Failure" className="px-3 py-2 align-top">
                            {row.errorCode ?? 'None recorded'}
                          </td>
                          <td data-label="Delay" className="px-3 py-2 align-top">
                            {row.delay}
                          </td>
                          <td data-label="Clock" className="px-3 py-2 align-top">
                            {row.clockSuspect ? 'Clock suspect' : 'Clock in step'}
                            {row.clockNote && (
                              <span className="block text-[11px] text-muted-foreground">
                                {row.clockNote}
                              </span>
                            )}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              </div>
            )}
          </>
        )}
      </CardX>
    </section>
  )
}
