/**
 * PAGE-ADMIN-METRIC-HISTORY (design §11.4, §11.5, §11.6; webui.md §8.2): the one
 * body that answers a stored series for whatever owner the caller names — a
 * Node's own Process series, or the Host series that every Node of one Agent
 * shares. The owner is the caller's business: this file charts the points, the
 * buckets, and the silences and never decides whose they are, so the same
 * evidence is never presented twice or attributed to the wrong owner.
 */
import { useMemo, useState, type ReactNode } from 'react'
import type { AdminMetricHistory } from '../api/admin'
import { StatusBadge, formatObservedAt } from '../components/StatusBadge'
import { Button } from '../components/ui/button'
import { CardX } from '../components/ui/card-x'
import { Input, Select } from '../components/ui/input'
import { SURFACE_CARD_STATIC } from '../lib/surface'
import { cn } from '../lib/utils'
import {
  METRIC_HISTORY_PRESETS,
  formatHistoryDuration,
  formatSampleDelay,
  hostMetricDefinition,
  hostMetricNeedsMount,
  metricAvailabilityNotice,
  metricBandPath,
  metricChartGeometry,
  metricGapKindLabel,
  metricGrainShortLabel,
  metricHistoryRange,
  metricLinePath,
  metricLineRuns,
  metricPointIsAggregate,
  metricPointLabel,
  metricPointProvesHole,
  metricSampleSummary,
  metricSegmentLabel,
  metricTierNotice,
  type MetricDefinition,
} from '../metricHistory'

const CARD_SURFACE = cn('rounded-md border-none', SURFACE_CARD_STATIC)

/** Emerald detail grid: label above its value, stacked on narrow viewports. */
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

/** The window an Operator is looking at, and the two ways it moves. */
export type MetricHistoryWindow = {
  hours: number
  range: { from: string; to: string }
  /** The cursor of the page being shown, null while the newest page is shown. */
  olderThan: string | null
  setOlderThan: (before: string | null) => void
  /** Answer another window; walking into a new range starts at its newest page. */
  selectRange: (nextHours: number) => void
  /** Re-answer the shown window as of now, starting again at its newest page. */
  reload: () => void
}

/**
 * The answered range is fixed per selection: the query key stays stable while
 * the page is open, and "Reload window" moves it explicitly rather than
 * re-asking on every render.
 */
export function useMetricHistoryWindow(initialHours = 24): MetricHistoryWindow {
  const [hours, setHours] = useState<number>(initialHours)
  const [range, setRange] = useState(() => metricHistoryRange(initialHours, new Date()))
  const [olderThan, setOlderThan] = useState<string | null>(null)
  return {
    hours,
    range,
    olderThan,
    setOlderThan,
    selectRange(nextHours: number) {
      setOlderThan(null)
      setHours(nextHours)
      setRange(metricHistoryRange(nextHours, new Date()))
    },
    reload() {
      setOlderThan(null)
      setRange(metricHistoryRange(hours, new Date()))
    },
  }
}

/** The series a caller has picked out of its family, and how to pick another. */
export type MetricHistorySelection = {
  metric: string
  definition: MetricDefinition
  onMetric: (metric: string) => void
}
/** The Host series an Operator picked, the path a storage series is named by,
 * and whether a series can be asked for at all yet. */
export type HostMetricSelection = MetricHistorySelection & {
  /** Whether this Host series is identified by a mount path. */
  needsMount: boolean
  /** The mount path as typed, while it still names no series. */
  mountPath: string
  onMountPath: (value: string) => void
  /** The dimension the query is asked with: the typed path, literally. */
  dimension: string
  /** False while a mount-identified series has no path yet. */
  asked: boolean
}

/** One answer of one series: what the Server said, or why there is none. */
export type MetricHistoryAnswer = {
  data: AdminMetricHistory | undefined
  isPending: boolean
  isError: boolean
  isRefetchError: boolean
  error: unknown
  refetch: () => void
}

export type MetricHistoryBodyProps = {
  /** The card heading, e.g. "Metric history" for a Node's Process series. */
  title: string
  /** What owns the series, named in the copy: "Node" or "Host". */
  subject: string
  /** What this card charts, in the caller's words. */
  intro: ReactNode
  /** Which surface renders this panel: a page that shows two of them names each one. */
  surface: string
  /** The series the picker offers, in the order the Operator reads them. */
  definitions: MetricDefinition[]
  /** The series being read, and how to answer another one of the family. */
  selection: MetricHistorySelection
  /** A control the caller owes the Operator beside the series picker, e.g. a mount path. */
  extraControl?: ReactNode
  /** Statements the caller owes the Operator under the controls. */
  note?: ReactNode
  /** The range being read and the three ways it moves: the window hook's own
   * return value satisfies this as it is, so a caller passes one grouped
   * contract rather than one prop per field. */
  view: MetricHistoryWindow
  /** The answer the caller holds: a query result satisfies this as it is, so a
   * caller passes the whole answer rather than one prop per field. */
  answer?: MetricHistoryAnswer
  /** True while the Operator has not named the series, so nothing has been
   * asked for: the body states that instead of inventing a loading answer. */
  unasked?: boolean
  /** What the body tells the Operator when no series has been asked for. */
  promptWhenUnasked?: ReactNode
}

/** The answer of a question nobody asked: no data, no load, no error. */
const NO_ANSWER: MetricHistoryAnswer = {
  data: undefined,
  isPending: false,
  isError: false,
  isRefetchError: false,
  error: null,
  refetch: () => {},
}

/** One series of one owner, with its buckets, its silences, and its own state. */
export function MetricHistoryBody({
  title,
  subject,
  intro,
  surface,
  definitions,
  selection,
  extraControl,
  note,
  view,
  answer,
  unasked = false,
  promptWhenUnasked,
}: MetricHistoryBodyProps) {
  const { metric, definition, onMetric } = selection
  const { hours, olderThan } = view
  const { data, isPending, isError, isRefetchError, error, refetch } = unasked
    ? NO_ANSWER
    : (answer ?? NO_ANSWER)
  const fixedMax = definition.fixedMax
  const geometry = useMemo(
    () =>
      data
        ? metricChartGeometry(
            data.items,
            data.gaps,
            Date.parse(data.from),
            Date.parse(data.to),
            fixedMax,
          )
        : null,
    [data, fixedMax],
  )
  const notice = metricAvailabilityNotice(data?.availability)
  // Older fixtures predate the segments field: an answer without segments is
  // read as an answer whose single stretch is the raw window.
  const segments = data?.segments ?? []
  const tierNotice = metricTierNotice(segments)
  const series = data?.series
  const newest = data ? data.items.slice(-8).reverse() : []
  const state = unasked
    ? 'No series yet'
    : !data
      ? 'Loading'
      : series?.observed
        ? 'Observed'
        : 'Never observed'
  const stateTone = !data ? 'neutral' : series?.observed ? 'ok' : 'neutral'

  return (
    <section className="contents" data-slot="metric-history-panel" data-surface={surface}>
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
            Metric series
            <Select
              className="min-h-11"
              value={metric}
              onChange={(event) => {
                view.setOlderThan(null)
                onMetric(event.currentTarget.value)
              }}
            >
              {definitions.map((item) => (
                <option key={item.metric} value={item.metric}>
                  {item.label}
                </option>
              ))}
            </Select>
          </label>
          {extraControl}
          <div className="flex flex-wrap gap-2" role="group" aria-label="History range">
            {METRIC_HISTORY_PRESETS.map((preset) => (
              <Button
                key={preset.label}
                variant={hours === preset.hours ? 'default' : 'outline'}
                aria-pressed={hours === preset.hours}
                className="min-h-11"
                onClick={() => view.selectRange(preset.hours)}
              >
                {preset.label}
              </Button>
            ))}
          </div>
          <Button variant="outline" className="min-h-11" onClick={view.reload}>
            Reload window
          </Button>
        </div>
        {note}
        {unasked && promptWhenUnasked && (
          <p className="mt-2 text-xs text-muted-foreground" role="status">
            {promptWhenUnasked}
          </p>
        )}
        <p className="mt-2 text-xs text-muted-foreground">
          {definition.label}: {definition.unit}. {definition.description}
        </p>
        {!data && isPending && (
          <p
            className="mt-3 flex flex-wrap items-center gap-2 text-sm text-muted-foreground"
            role="status"
          >
            <StatusBadge status="Loading" tone="neutral" /> Loading the stored series…
          </p>
        )}
        {isError && (
          <div
            className="mt-3 flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
            role="alert"
          >
            <StatusBadge status="Error" tone="error" />{' '}
            <span className="min-w-0 break-words">
              {error instanceof Error ? error.message : 'Unable to load the metric history'}
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
          >
            <StatusBadge status="Error" tone="error" /> Failed to refresh; showing the last
            successful samples.
          </div>
        )}
        {data && notice && (
          <p className="mt-3 text-sm text-muted-foreground" data-slot="metric-history-availability">
            {notice}
          </p>
        )}
        {data && tierNotice && (
          <p className="mt-3 text-sm text-muted-foreground" data-slot="metric-history-tiers">
            {tierNotice}
          </p>
        )}
        {data && (
          <>
            <div className="mt-3">
              <DetailList>
                <DetailItem label="Ledger">
                  {series?.observed
                    ? series.observationCount + ' stored observation(s) since the first one'
                    : 'No observation was ever recorded for this series'}
                </DetailItem>
                <DetailItem label="First observed">
                  {formatObservedAt(series?.firstObservedAt)}
                </DetailItem>
                <DetailItem label="Last observed">
                  {formatObservedAt(series?.lastObservedAt)}
                </DetailItem>
                <DetailItem label="Last received">
                  {formatObservedAt(series?.lastReceivedAt)}
                </DetailItem>
                <DetailItem label="Coverage">
                  {formatHistoryDuration(series?.coverageSeconds ?? 0)} of{' '}
                  {formatHistoryDuration(series?.windowSeconds ?? 0)}
                  <span className="text-[11px] text-muted-foreground">
                    {' '}
                    · proven only between stored samples, never assumed across a silence or inside a
                    bucket that measured a hole of its own
                  </span>
                </DetailItem>
                <DetailItem label="Carried deliveries">
                  {series?.replayedCount ?? 0} replay(s), {series?.correctedCount ?? 0}{' '}
                  correction(s)
                  <span className="text-[11px] text-muted-foreground">
                    {' '}
                    · counted apart from observations
                  </span>
                </DetailItem>
                <DetailItem label="Points in this answer">
                  {series?.sampledCount ?? 0} · grain {metricGrainShortLabel(data.grain)}
                  {data.aggregateSupported ? '' : ' (raw only)'}
                </DetailItem>
                <DetailItem label="Newest delay">
                  {formatSampleDelay(series?.latestDelaySeconds)}
                  {series?.latestClockSuspect && (
                    <span className="text-destructive"> · clock suspect</span>
                  )}
                </DetailItem>
                <DetailItem label="Retained raw window">
                  {data.rawRetentionDays} {data.rawRetentionDays === 1 ? 'day' : 'days'} · requested
                  from {formatObservedAt(data.requestedFrom)}
                </DetailItem>
                <DetailItem label="Investigation horizon">
                  {data.historyHorizonDays} days · older stretches are answered by buckets, never by
                  a recreated sample
                </DetailItem>
              </DetailList>
            </div>
            {data.truncated && (
              <p className="mt-3 text-sm" role="status" data-slot="metric-history-truncated">
                This window holds more samples than one answer carries: the newest{' '}
                {data.items.length} points of this answer are drawn and the older ones are omitted,
                so the plot starts at the oldest point it actually has rather than inventing one at
                the window edge. The Server spends one budget on a whole answer and keeps the newest
                points of every tier, so the omitted stretch is a seam and never a hole in the
                middle. While the Server names a continuation coordinate, 'Load older points'
                follows it one page at a time rather than dropping back to the newest answer.
              </p>
            )}
            {/* The Server names the coordinate to continue from while the answer
              is truncated, and one cursor reaches every older page: the control
              stays as long as there is a coordinate to ask for, so an answer
              that is still truncated can be walked older again and again
              instead of the Owner being stopped on the second page. */}
            {data.continuation && (
              <Button
                variant="outline"
                className="mt-2 min-h-11"
                data-slot="metric-history-older"
                onClick={() => view.setOlderThan(data.continuation ?? null)}
              >
                Load older points
              </Button>
            )}
            {olderThan !== null && (
              <p
                className="mt-3 flex flex-wrap items-center gap-2 text-sm text-muted-foreground"
                role="status"
                data-slot="metric-history-page"
              >
                <span className="min-w-0 break-words">
                  An older page the Server continued into: this answer ends at{' '}
                  {formatObservedAt(data.to)}, the coordinate the next-newer answer stopped at, and
                  starts at {formatObservedAt(data.from)}. No point is skipped between two pages.
                </span>
                <Button
                  variant="outline"
                  size="sm"
                  className="min-h-11"
                  onClick={() => view.setOlderThan(null)}
                >
                  Return to the newest points
                </Button>
              </p>
            )}
            {series && !series.observed && (
              <p
                className="mt-3 text-sm text-muted-foreground"
                role="status"
                data-slot="metric-history-empty"
              >
                This {subject} never reported {definition.label}. Nothing is charted and nothing is
                shown as zero; the series appears with the first accepted observation.
              </p>
            )}
            {series?.observed && data.items.length === 0 && (
              <p
                className="mt-3 text-sm text-muted-foreground"
                role="status"
                data-slot="metric-history-empty"
              >
                The series is observed, but no stored sample falls inside this window, and no
                aggregate bucket answers for it either. The value is reported as unknown, never as
                zero.
              </p>
            )}
            {geometry && geometry.samples > 0 && (
              <div className="mt-3 grid min-w-0 grid-cols-[3rem_minmax(0,1fr)] gap-x-2">
                <div
                  className="flex flex-col justify-between pr-1 text-right text-[11px] tabular-nums text-muted-foreground"
                  aria-hidden="true"
                >
                  <span>{definition.axisFormat(geometry.max)}</span>
                  <span>{definition.axisFormat(geometry.max / 2)}</span>
                  <span>{definition.axisFormat(0)}</span>
                </div>
                <svg
                  viewBox="0 0 600 150"
                  preserveAspectRatio="none"
                  role="img"
                  aria-label={
                    definition.label +
                    ' history from ' +
                    formatObservedAt(data.from) +
                    ' to ' +
                    formatObservedAt(data.to) +
                    ' at ' +
                    metricGrainShortLabel(data.grain) +
                    ' grain'
                  }
                  className="col-start-2 h-40 w-full text-primary"
                  data-slot="metric-history-chart"
                >
                  <title>
                    {definition.label} history over {formatHistoryDuration(data.windowSeconds)},{' '}
                    {geometry.samples} point(s) standing for {geometry.weight} observation(s)
                  </title>
                  <desc>
                    {geometry.samples} point(s) standing for {geometry.weight} observation(s) folded
                    into {geometry.segments.length} drawn stretch(es); {geometry.aggregatedPoints}{' '}
                    of them are aggregate buckets, drawn as squares across the stretch each one
                    covers, and {geometry.holedPoints} of those proved a hole inside their own
                    stretch, so no line is drawn across them; {data.gaps.length} reported silence(s)
                    are left undrawn.
                  </desc>
                  <g aria-hidden="true">
                    <line
                      x1="0"
                      y1="8"
                      x2="600"
                      y2="8"
                      className="stroke-border [vector-effect:non-scaling-stroke]"
                    />
                    <line
                      x1="0"
                      y1="75"
                      x2="600"
                      y2="75"
                      className="stroke-border [vector-effect:non-scaling-stroke]"
                    />
                    <line
                      x1="0"
                      y1="142"
                      x2="600"
                      y2="142"
                      className="stroke-border [vector-effect:non-scaling-stroke]"
                    />
                    {geometry.gapBands.map((band) => (
                      <rect
                        key={band.x.toFixed(2)}
                        data-slot="metric-history-gap-band"
                        className="fill-muted-foreground opacity-20"
                        x={band.x}
                        y={8}
                        width={Math.max(1, band.width)}
                        height={134}
                      />
                    ))}
                  </g>
                  <g aria-hidden="true">
                    {geometry.segments.map((segment, index) => (
                      <g key={segment[0].x.toFixed(2) + '-' + index}>
                        {/* A stretch is only drawn as one line where its own
                          columns were continuous: a bucket that measured a hole
                          inside the stretch it stands for is not joined to
                          either neighbour, so the break the Server measured is
                          visible instead of being drawn over. */}
                        {metricLineRuns(segment).map((run, runIndex) =>
                          run.length > 1 ? (
                            <g key={'stretch-' + runIndex}>
                              <path className="fill-current opacity-15" d={metricBandPath(run)} />
                              <path
                                data-slot="metric-history-line"
                                className="fill-none stroke-current [vector-effect:non-scaling-stroke]"
                                strokeWidth={2}
                                strokeLinecap="round"
                                strokeLinejoin="round"
                                d={metricLinePath(run)}
                              />
                            </g>
                          ) : null,
                        )}
                        {segment.map((point) =>
                          point.aggregateCount > 0 ? (
                            /* A bucket is drawn as a square marker, not as a dot:
                             a dot claims one stored observation, a square says
                             the Server kept a counted stretch here. */
                            <rect
                              key={'bucket-' + point.x.toFixed(2)}
                              data-slot="metric-history-bucket"
                              data-hole={point.provenHole ? 'proved' : undefined}
                              className="fill-current"
                              x={point.x - 2.5}
                              y={point.y - 2.5}
                              width={5}
                              height={5}
                            />
                          ) : null,
                        )}
                        {segment.length === 1 && (
                          <>
                            {/* One column, but possibly several observations: a
                              vertical whisker spans the bucket's minimum and
                              maximum so an isolated spike is never hidden by
                              the single point drawn at its newest value. It
                              stays inside the column: no line is interpolated
                              across the time the column does not cover. */}
                            <line
                              data-slot="metric-history-whisker"
                              className="stroke-current [vector-effect:non-scaling-stroke]"
                              strokeWidth={2}
                              strokeLinecap="round"
                              x1={segment[0].x}
                              x2={segment[0].x}
                              y1={segment[0].top}
                              y2={segment[0].bottom}
                            />
                            <circle
                              data-slot="metric-history-point"
                              className="fill-current stroke-background [vector-effect:non-scaling-stroke]"
                              strokeWidth={1.5}
                              cx={segment[0].x}
                              cy={segment[0].y}
                              r={3.5}
                            />
                          </>
                        )}
                      </g>
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
                {geometry.aggregatedPoints > 0 && (
                  <p
                    className="col-start-2 pt-1 text-[11px] text-muted-foreground"
                    data-slot="metric-history-buckets"
                  >
                    {geometry.aggregatedPoints} of {geometry.samples} drawn points are aggregate
                    buckets: a circle is one stored observation, a square is a bucket, and the plot
                    stands for {geometry.weight} observation(s) in total, with every bucket drawn
                    across the stretch it kept.
                    {geometry.holedPoints > 0
                      ? ' ' +
                        geometry.holedPoints +
                        ' of them measured a hole inside their own stretch, so the square stands ' +
                        'alone there and no line is drawn across it: the stretch is not closed.'
                      : ''}
                  </p>
                )}
              </div>
            )}
            {segments.length > 0 && (
              <div className="mt-4" data-slot="metric-history-segments">
                <h3 className="text-sm font-medium">Tiers in this answer</h3>
                <ul className="mt-1 space-y-1 text-sm text-muted-foreground">
                  {segments.map((segment) => (
                    <li key={segment.grain + segment.from + segment.to}>
                      <span className="font-medium text-foreground">
                        {metricSegmentLabel(segment)}
                      </span>
                      {': '}
                      {formatObservedAt(segment.from)} → {formatObservedAt(segment.to)}
                      {segment.pointCount === 0 ? ' · consulted, holds nothing' : ''}
                    </li>
                  ))}
                </ul>
              </div>
            )}
            {data.gaps.length > 0 && (
              <div className="mt-4" data-slot="metric-history-gaps">
                <h3 className="text-sm font-medium">Silences in this window</h3>
                <ul className="mt-1 space-y-1 text-sm text-muted-foreground">
                  {data.gaps.map((gap) => (
                    <li key={gap.from + gap.to}>
                      <span className="font-medium text-foreground">
                        {metricGapKindLabel(gap.kind)}
                      </span>
                      {': '}
                      {formatObservedAt(gap.from)} → {formatObservedAt(gap.to)} (
                      {formatHistoryDuration(gap.seconds)}) · {gap.reason}
                      {gap.skippedCount != null
                        ? ' · ' + gap.skippedCount + ' observation(s) skipped'
                        : ''}
                    </li>
                  ))}
                </ul>
              </div>
            )}
            {newest.length > 0 && (
              <div className="mt-4">
                <h3 className="text-sm font-medium">Newest observations</h3>
                <div className="mt-1 overflow-x-auto">
                  <table className="w-full text-left text-sm" data-slot="metric-history-samples">
                    <thead>
                      <tr className="text-xs font-medium tracking-wider text-muted-foreground">
                        <th scope="col" className="py-1 pr-3">
                          Observed
                        </th>
                        <th scope="col" className="py-1 pr-3">
                          Received
                        </th>
                        <th scope="col" className="py-1 pr-3">
                          Grain
                        </th>
                        <th scope="col" className="py-1 pr-3">
                          Value
                        </th>
                        <th scope="col" className="py-1">
                          Delay
                        </th>
                      </tr>
                    </thead>
                    <tbody>
                      {newest.map((sample) => {
                        const summary = metricSampleSummary(sample, geometry?.cadenceSeconds ?? 0)
                        const aggregate = metricPointIsAggregate(sample)
                        // The same judgement the plot makes: a bucket whose own
                        // measured interval reaches the gap threshold did not
                        // cover its stretch continuously.
                        const provedHole = metricPointProvesHole(
                          sample,
                          geometry?.cadenceSeconds ?? 0,
                        )
                        return (
                          <tr
                            key={sample.observedAt + '-' + sample.value}
                            data-slot="metric-history-sample"
                          >
                            <td className="py-1 pr-3 tabular-nums">
                              {formatObservedAt(sample.observedAt)}
                              {aggregate && (
                                <span className="block text-[11px] text-muted-foreground">
                                  first kept{' '}
                                  {formatObservedAt(sample.firstObservedAt ?? sample.observedAt)} ·
                                  newest {formatObservedAt(sample.lastObservedAt)}
                                </span>
                              )}
                            </td>
                            <td className="py-1 pr-3 tabular-nums">
                              {formatObservedAt(sample.receivedAt)}
                            </td>
                            <td className="py-1 pr-3">
                              {metricPointLabel(sample)}
                              {summary && (
                                <span
                                  className="block text-[11px] text-muted-foreground"
                                  data-slot={provedHole ? 'metric-history-hole' : undefined}
                                >
                                  {summary}
                                </span>
                              )}
                            </td>
                            <td className="py-1 pr-3 tabular-nums font-medium">
                              {definition.format(sample.value)}
                              {aggregate && (
                                <span className="block text-[11px] font-normal text-muted-foreground">
                                  {definition.format(sample.minValue ?? sample.value)} to{' '}
                                  {definition.format(sample.maxValue ?? sample.value)}
                                </span>
                              )}
                            </td>
                            <td className="py-1 tabular-nums">
                              {formatSampleDelay(sample.delaySeconds)}
                              {sample.clockSuspect && (
                                <span className="block text-[11px] text-destructive">
                                  {sample.clockNote ??
                                    'The observation is stamped after the receipt.'}
                                </span>
                              )}
                            </td>
                          </tr>
                        )
                      })}
                    </tbody>
                  </table>
                </div>
                <p className="mt-1 text-xs text-muted-foreground">
                  The newest {newest.length} of {data.items.length} points in this answer. For a
                  bucket the value is its newest reading and the range is its minimum to its
                  maximum, so a spike the Server preserved is visible even where the bucket is drawn
                  as one marker. Observed, received, and the delay belong to the same point, so a
                  spooled or retried delivery stays visible per point.
                </p>
              </div>
            )}
          </>
        )}
      </CardX>
    </section>
  )
}

/** The one control a mount-identified series needs: the path the Agent
 * reported, typed by the Operator because no Admin DTO exposes a mount list. */
export function HostMountPathControl({
  value,
  onChange,
}: {
  value: string
  onChange: (value: string) => void
}) {
  return (
    <label className="grid min-w-40 gap-1 text-xs font-medium text-muted-foreground">
      Mount path
      <Input
        className="min-h-11"
        value={value}
        placeholder="/"
        onChange={(event) => onChange(event.currentTarget.value)}
      />
    </label>
  )
}

/** The Host series an Operator picks, and the mount path a storage series is
 * named by. One implementation for both pages that read Host evidence, so the
 * Node page and the Agent page cannot disagree about a dimension or about when
 * a question may be asked at all. */
export function useHostMetricSelection(view: MetricHistoryWindow): HostMetricSelection {
  const [metric, setMetric] = useState<string>('cpu_percent')
  const [mountPath, setMountPath] = useState('')
  const needsMount = hostMetricNeedsMount(metric)
  // A mount identity is literal, so the dimension is the path exactly as the
  // Operator typed it: the Agent reports the path it mounted, and a path that
  // differs only in surrounding whitespace is a different series. Trimming the
  // input would read another mount's evidence or claim this one was never seen.
  const dimension = needsMount ? mountPath : ''
  return {
    metric,
    definition: hostMetricDefinition(metric),
    needsMount,
    mountPath,
    dimension,
    asked: !needsMount || dimension.length > 0,
    // A new series is a new question, so it starts at the newest page.
    onMetric(next: string) {
      view.setOlderThan(null)
      setMetric(next)
    },
    onMountPath(next: string) {
      view.setOlderThan(null)
      setMountPath(next)
    },
  }
}
