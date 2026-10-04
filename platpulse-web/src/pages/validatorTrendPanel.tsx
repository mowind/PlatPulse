/**
 * PAGE-ADMIN-VALIDATOR-TREND (design §15.4; webui.md §15.21): the daily
 * snapshot trend of one chain identity over a bounded window of the Server's
 * *configured* calendar.
 *
 * The panel asks the Server for a window in UTC instants and shows the
 * configured local days it answered, so a day is never silently re-bucketed
 * into UTC: the real day boundaries (23, 24 or 25 hours), the configured month
 * boundaries, the sample time each day was chosen from, the measured delay, the
 * coverage verdict, and every silence are printed beside the series. A day the
 * answer proves holds no snapshot breaks the line and is listed as a silence,
 * and a metric a stored row never carried is Unknown rather than zero.
 *
 * Reward and block values are cumulative Provider counters as of each sample,
 * and stake is the balance the Provider reported then: the panel labels each as
 * what it is and never derives period earnings, net profit, or a re-bucketed
 * series from them.
 */
import { useState } from 'react'
import { Link } from 'react-router'
import { useAdminValidatorTrend } from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { DetailItem, DetailList } from '../components/DetailList'
import { StatusBadge, formatObservedAt } from '../components/StatusBadge'
import { Button } from '../components/ui/button'
import { CardX } from '../components/ui/card-x'
import { Select } from '../components/ui/input'
import { formatAmountExact } from '../lib/amount'
import { SURFACE_CARD_STATIC } from '../lib/surface'
import { cn } from '../lib/utils'
import {
  VALIDATOR_TREND_PRESETS,
  VALIDATOR_TREND_SERIES,
  validatorTrendAssociationLabel,
  validatorTrendAssociationNotice,
  validatorTrendChartGeometry,
  validatorTrendCounterNotice,
  validatorTrendCoverage,
  validatorTrendDayWindow,
  validatorTrendDelay,
  validatorTrendForeignNotice,
  validatorTrendGapNotice,
  validatorTrendLinePath,
  validatorTrendMonthWindow,
  validatorTrendRange,
  validatorTrendSampleTimeLabel,
  validatorTrendStretchNotice,
  validatorTrendTimestamps,
  validatorTrendTruncationNotice,
  type ValidatorTrendSeries,
} from '../validatorTrend'
import type {
  AdminValidatorTrendPoint,
  AdminValidatorTrendResponse,
} from '../api/generated'

const CARD_SURFACE = cn('rounded-md border-none', SURFACE_CARD_STATIC)

/** The window an Operator is looking at, and the ways it moves. The answered
 *  range is fixed per selection, so the query key stays stable while the page
 *  is open and only an explicit action re-asks the Server. */
export type ValidatorTrendWindow = {
  days: number
  limit: number
  range: { from: string; to: string }
  /** The cursor of the page being shown: null while the newest page is shown. */
  before: string | null
  setBefore: (before: string | null) => void
  selectRange: (days: number, limit: number) => void
  reload: () => void
}

export function useValidatorTrendWindow(initialDays = 30): ValidatorTrendWindow {
  const preset = VALIDATOR_TREND_PRESETS.find((item) => item.days === initialDays)
  const [days, setDays] = useState<number>(initialDays)
  const [limit, setLimit] = useState<number>(preset ? preset.limit : initialDays + 1)
  const [range, setRange] = useState(() => validatorTrendRange(initialDays, new Date()))
  const [before, setBefore] = useState<string | null>(null)
  return {
    days,
    limit,
    range,
    before,
    setBefore,
    selectRange(nextDays: number, nextLimit: number) {
      setBefore(null)
      setDays(nextDays)
      setLimit(nextLimit)
      setRange(validatorTrendRange(nextDays, new Date()))
    },
    reload() {
      setBefore(null)
      setRange(validatorTrendRange(days, new Date()))
    },
  }
}

/** The drawing of one series. A gap is a band, a missing value is a break, and
 *  the values are never filled in: the plot shows the evidence it has. */
function TrendChart({
  page,
  series,
}: {
  page: AdminValidatorTrendResponse
  series: ValidatorTrendSeries
}) {
  const chart = validatorTrendChartGeometry(page, series)
  const definition = VALIDATOR_TREND_SERIES.find((item) => item.value === series)
  const label =
    definition?.label +
    ' of this Validator over ' +
    page.answeredFromLocalDate +
    ' to ' +
    page.answeredToLocalDate +
    ' in ' +
    page.timezone
  return (
    <figure className="mt-3" data-slot="validator-trend-chart">
      <svg
        className="h-40 w-full text-foreground"
        viewBox={'0 0 ' + chart.width + ' ' + (chart.bottom + 8)}
        role="img"
        aria-label={
          label +
          ': ' +
          chart.columns.length +
          ' stored day(s) drawn, ' +
          chart.unknownDays +
          ' configured local day(s) without a snapshot and ' +
          chart.unknownValues +
          ' stored day(s) whose row carries no value for this series'
        }
        preserveAspectRatio="none"
      >
        {chart.gapBands.map((band) => (
          <rect
            key={band.key}
            x={band.x}
            y={chart.top}
            width={band.width}
            height={chart.bottom - chart.top}
            className="fill-muted-foreground/20"
          />
        ))}
        {chart.segments.map((segment) =>
          segment.length > 1 ? (
            <path
              key={'line-' + segment[0].localDate}
              d={validatorTrendLinePath(segment)}
              className="stroke-current"
              fill="none"
              strokeWidth={1.5}
            />
          ) : null,
        )}
        {chart.columns.map((column) => (
          <circle
            key={column.localDate}
            cx={column.x}
            cy={column.y}
            r={2}
            className="fill-current"
          />
        ))}
      </svg>
      <figcaption className="mt-1 text-xs text-muted-foreground">
        {label}. {chart.columns.length} stored day(s) are drawn
        {chart.unknownDays > 0
          ? '; ' +
            chart.unknownDays +
            ' configured local day(s) of this stretch hold no snapshot and are drawn as silence'
          : ''}
        {chart.unknownValues > 0
          ? '; ' +
            chart.unknownValues +
            ' stored day(s) whose row carries no ' +
            (definition?.label.toLowerCase() ?? 'value') +
            ' are left blank'
          : ''}
        . Shaded bands are silences, and the line breaks across them rather than drawing through.
      </figcaption>
    </figure>
  )
}

function PointsTable({
  points,
  timezone,
  series,
}: {
  points: AdminValidatorTrendPoint[]
  timezone: string
  series: ValidatorTrendSeries
}) {
  return (
    <div className="mt-3 max-h-96 overflow-auto">
      <table data-stack data-slot="validator-trend-points" className="w-full text-sm">
        <caption className="sr-only">
          Stored configured local days of this trend, their real UTC stretch, and the values the
          Provider counters carried
        </caption>
        <thead>
          <tr className="border-b">
            <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
              Local day ({timezone})
            </th>
            <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
              Rank
            </th>
            <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
              Stake
            </th>
            <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
              Delegators
            </th>
            <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
              Reward (cumulative)
            </th>
            <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
              Blocks (cumulative)
            </th>
            <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
              Sample time
            </th>
            <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
              Delay
            </th>
            <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
              Source
            </th>
          </tr>
        </thead>
        <tbody>
          {points.map((point) => {
            const window = validatorTrendDayWindow(point)
            return (
              <tr key={point.localDate + point.observationKey} className="border-b border-border/60 align-top">
                <th scope="row" data-label="Local day" className="min-w-0 px-3 py-3 text-left">
                  <span className="block font-medium">{point.localDate}</span>
                  <span className="mt-0.5 block text-[11px] text-muted-foreground break-all">
                    {window.from} → {window.to}
                    {window.hours != null ? ' · ' + window.hours + ' hours' : ''}
                  </span>
                  <span className="mt-0.5 block text-[11px] text-muted-foreground">
                    Month {point.monthKey}
                  </span>
                </th>
                <td data-label="Rank" className="min-w-0 px-3 py-3">
                  {point.rank ?? 'Unknown'}
                </td>
                <td data-label="Stake" className="min-w-0 px-3 py-3">
                  {formatAmountExact(point.stakeAmount)}
                </td>
                <td data-label="Delegators" className="min-w-0 px-3 py-3">
                  {point.delegatorCount ?? 'Unknown'}
                </td>
                <td data-label="Reward (cumulative)" className="min-w-0 px-3 py-3">
                  {formatAmountExact(point.rewardAmount)}
                </td>
                <td data-label="Blocks (cumulative)" className="min-w-0 px-3 py-3">
                  {point.blockCount ?? 'Unknown'}
                </td>
                <td data-label="Sample time" className="min-w-0 px-3 py-3">
                  <span className="block">{validatorTrendSampleTimeLabel(point.sampleTime)}</span>
                  {validatorTrendTimestamps(point).map((stamp) => (
                    <span
                      key={stamp.label}
                      className="mt-0.5 block text-[11px] text-muted-foreground"
                    >
                      {stamp.label} {stamp.value}
                    </span>
                  ))}
                </td>
                <td data-label="Delay" className="min-w-0 px-3 py-3">
                  {validatorTrendDelay(point.delaySeconds, point.clockSuspect)}
                </td>
                <td data-label="Source" className="min-w-0 px-3 py-3">
                  <span className="block break-all">{point.source}</span>
                  <span className="mt-0.5 block text-[11px] text-muted-foreground break-all">
                    Epoch {point.epoch ?? 'Unknown'}
                  </span>
                </td>
              </tr>
            )
          })}
        </tbody>
      </table>
      {series === 'stake' ? (
        <p className="mt-2 text-xs text-muted-foreground">
          The Stake column carries the exact decimal the Provider reported; the plot places it on
          the value axis as a number.
        </p>
      ) : null}
    </div>
  )
}

/** PAGE-ADMIN-VALIDATOR-TREND body: the trend of one Validator identity. */
export function ValidatorTrendPanel({ validatorId }: { validatorId: string }) {
  const { generation } = useAuth()
  const view = useValidatorTrendWindow()
  const [series, setSeries] = useState<ValidatorTrendSeries>('rank')
  const query = useAdminValidatorTrend(generation, validatorId, {
    from: view.range.from,
    to: view.range.to,
    before: view.before,
    limit: view.limit,
  })
  const data = query.data
  const coverage = validatorTrendCoverage(data)
  const stretch = validatorTrendStretchNotice(data)
  const truncated = validatorTrendTruncationNotice(data)
  const foreign = validatorTrendForeignNotice(data)
  const associations = validatorTrendAssociationNotice(data)
  const counters = validatorTrendCounterNotice(data)
  const gaps = data?.gaps ?? []

  return (
    <section className="contents" data-slot="validator-trend-panel">
      <CardX
        size="medium"
        className={CARD_SURFACE}
        header={
          <>
            <h2 className="text-lg font-semibold">Daily trend</h2>
            <StatusBadge status={coverage.label} tone={coverage.tone} />
          </>
        }
      >
        <p className="text-sm text-muted-foreground">
          One stored snapshot per configured local day, over a bounded window. The days and their
          month boundaries are the Server's configured calendar mapped into UTC, so a
          daylight-saving day is 23 or 25 hours wide here instead of a pretended 24, and no stored
          bucket is silently re-bucketed into UTC. A day with no snapshot is a silence, and a metric
          the row never carried is Unknown — never a zero.
        </p>
        <div className="mt-3 flex flex-wrap items-end gap-3">
          <label className="grid min-w-40 gap-1 text-xs font-medium text-muted-foreground">
            Series
            <Select
              className="min-h-11"
              value={series}
              onChange={(event) => setSeries(event.currentTarget.value as ValidatorTrendSeries)}
            >
              {VALIDATOR_TREND_SERIES.map((item) => (
                <option key={item.value} value={item.value}>
                  {item.label}
                </option>
              ))}
            </Select>
          </label>
          <div className="flex flex-wrap gap-2" role="group" aria-label="Trend range">
            {VALIDATOR_TREND_PRESETS.map((preset) => (
              <Button
                key={preset.label}
                variant={view.days === preset.days ? 'default' : 'outline'}
                aria-pressed={view.days === preset.days}
                className="min-h-11"
                onClick={() => view.selectRange(preset.days, preset.limit)}
              >
                {preset.label}
              </Button>
            ))}
          </div>
          <Button variant="outline" className="min-h-11" onClick={view.reload}>
            Reload window
          </Button>
        </div>
        {!data && query.isPending && (
          <p
            className="mt-3 flex flex-wrap items-center gap-2 text-sm text-muted-foreground"
            role="status"
          >
            <StatusBadge status="Loading" tone="neutral" /> Loading the daily trend…
          </p>
        )}
        {query.isError && (
          <div
            className="mt-3 flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
            role="alert"
          >
            <StatusBadge status="Error" tone="error" />{' '}
            <span className="min-w-0 break-words">
              {query.error instanceof Error ? query.error.message : 'Unable to load the Validator trend'}
            </span>
            <Button variant="link" size="sm" onClick={() => void query.refetch()}>
              Try again
            </Button>
          </div>
        )}
        {data && query.isRefetchError && (
          <div
            className="mt-3 flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
            role="alert"
          >
            <StatusBadge status="Error" tone="error" /> Failed to refresh; showing the last
            successful trend answer.
          </div>
        )}
        {data && (
          <>
            <p className="mt-3 text-sm text-muted-foreground" data-slot="validator-trend-coverage">
              {coverage.description}
            </p>
            {stretch && (
              <p className="mt-2 text-sm text-muted-foreground" data-slot="validator-trend-stretch">
                {stretch}
              </p>
            )}
            {truncated && (
              <p className="mt-2 text-sm text-muted-foreground" data-slot="validator-trend-truncated">
                {truncated}
              </p>
            )}
            {foreign && (
              <p className="mt-2 text-sm text-muted-foreground" data-slot="validator-trend-foreign">
                {foreign}
              </p>
            )}
            {associations && (
              <p
                className="mt-2 text-sm text-muted-foreground"
                data-slot="validator-trend-associations-notice"
              >
                {associations}
              </p>
            )}
            <p className="mt-2 text-sm text-muted-foreground" data-slot="validator-trend-counters">
              {counters}
            </p>
            <div className="mt-3">
              <DetailList>
                <DetailItem label="Configured timezone">{data.timezone}</DetailItem>
                <DetailItem label="Coverage">{coverage.label}</DetailItem>
                <DetailItem label="Requested">
                  {data.requestedFromLocalDate} → {data.requestedToLocalDate} ({data.requestedDays}{' '}
                  configured local day(s))
                </DetailItem>
                <DetailItem label="Answered">
                  {data.answeredFromLocalDate} → {data.answeredToLocalDate} ({data.expectedDays} day(s))
                </DetailItem>
                <DetailItem label="Observed / missing">
                  {data.observedDays} observed · {data.missingDays} missing
                </DetailItem>
                <DetailItem label="First / last observed">
                  {(data.firstObservedLocalDate ?? 'Unknown') + ' / ' + (data.lastObservedLocalDate ?? 'Unknown')}
                </DetailItem>
                <DetailItem label="Association history">
                  {data.associationHistoryPartial
                    ? 'Partial · ' + data.deletedNodes + ' deleted Node(s)'
                    : 'Resolvable intervals complete'}
                </DetailItem>
              </DetailList>
            </div>
            <TrendChart page={data} series={series} />
            <div className="mt-3 flex flex-wrap gap-2" role="group" aria-label="Trend paging">
              <Button
                variant="outline"
                className="min-h-11"
                disabled={!data.continuation}
                onClick={() => view.setBefore(data.continuation ?? null)}
              >
                Load older days
              </Button>
              <Button
                variant="outline"
                className="min-h-11"
                disabled={view.before == null}
                onClick={() => view.setBefore(null)}
              >
                Return to newest
              </Button>
            </div>
            <h3 className="mt-4 text-sm font-semibold">Silences in this stretch</h3>
            {gaps.length === 0 ? (
              <p className="mt-1 text-sm text-muted-foreground" data-slot="validator-trend-no-gaps">
                No configured local day of the answered stretch is missing a snapshot.
              </p>
            ) : (
              <ul className="mt-1 list-disc space-y-1 pl-5 text-sm" data-slot="validator-trend-gaps">
                {gaps.map((gap) => (
                  <li key={gap.fromLocalDate + '..' + gap.toLocalDate}>
                    {validatorTrendGapNotice(gap)}
                  </li>
                ))}
              </ul>
            )}
            <h3 className="mt-4 text-sm font-semibold">Stored days</h3>
            {data.points.length === 0 ? (
              <p className="mt-1 text-sm text-muted-foreground" data-slot="validator-trend-empty">
                No snapshot was stored for any configured local day of this answer, so there is no
                day to draw and no value to read.
              </p>
            ) : (
              <PointsTable points={data.points} timezone={data.timezone} series={series} />
            )}
            <h3 className="mt-4 text-sm font-semibold">Configured months</h3>
            {data.months.length === 0 ? (
              <p className="mt-1 text-sm text-muted-foreground">No configured month is covered.</p>
            ) : (
              <div className="mt-1 overflow-x-auto">
                <table data-stack data-slot="validator-trend-months" className="w-full text-sm">
                  <caption className="sr-only">
                    Configured calendar months this answer touches, with the UTC instants their
                    boundaries really fall on
                  </caption>
                  <thead>
                    <tr className="border-b">
                      <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                        Month
                      </th>
                      <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                        Configured boundary in UTC
                      </th>
                      <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                        Observed days
                      </th>
                      <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">
                        First / last observed
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {data.months.map((month) => (
                      <tr key={month.monthKey} className="border-b border-border/60 align-top">
                        <th scope="row" data-label="Month" className="min-w-0 px-3 py-3 text-left font-medium">
                          {month.monthKey}
                        </th>
                        <td data-label="Configured boundary in UTC" className="min-w-0 px-3 py-3 break-all">
                          {validatorTrendMonthWindow(month)}
                        </td>
                        <td data-label="Observed days" className="min-w-0 px-3 py-3">
                          {month.observedDays}
                        </td>
                        <td data-label="First / last observed" className="min-w-0 px-3 py-3">
                          {(month.firstLocalDate ?? 'Unknown') + ' / ' + (month.lastLocalDate ?? 'Unknown')}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
            <h3 className="mt-4 text-sm font-semibold">Node associations</h3>
            {data.associations.length === 0 ? (
              <p className="mt-1 text-sm text-muted-foreground" data-slot="validator-trend-no-associations">
                No association interval of this Validator still resolves to a Node
                {data.associationHistoryPartial
                  ? '; the deleted Node intervals above are unavailable rather than never having existed.'
                  : '.'}
              </p>
            ) : (
              <ul className="mt-1 space-y-1 text-sm" data-slot="validator-trend-association-list">
                {data.associations.map((association) => (
                  <li key={association.linkId} className="flex flex-wrap items-center gap-2">
                    <Link
                      className="inline-flex min-h-11 min-w-11 items-center font-medium underline-offset-4 hover:underline"
                      to={'/admin/nodes/' + association.nodeId}
                    >
                      {validatorTrendAssociationLabel(association)}
                    </Link>
                    <span className="text-muted-foreground">
                      {association.origin} · {formatObservedAt(association.validFrom)} →{' '}
                      {association.validUntil
                        ? formatObservedAt(association.validUntil)
                        : 'open interval'}
                    </span>
                    <StatusBadge
                      status={association.current ? 'Open' : 'Ended'}
                      tone={association.current ? 'ok' : 'neutral'}
                    />
                    <span className="text-[11px] text-muted-foreground">
                      Node {association.nodeLifecycle}
                    </span>
                  </li>
                ))}
              </ul>
            )}
          </>
        )}
      </CardX>
    </section>
  )
}
