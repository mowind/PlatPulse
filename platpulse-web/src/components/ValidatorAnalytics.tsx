import type {
  AdminValidatorAnalyticsResponse,
  AdminValidatorDailySnapshot,
  AdminValidatorMonthlyAggregate,
  PublicValidatorAnalyticsResponse,
  PublicValidatorDailySnapshot,
  PublicValidatorMonthlyAggregate,
} from '../api/generated'
import { StatusBadge } from './StatusBadge'
import { cn } from '../lib/utils'

type ValidatorAnalyticsResponse =
  | PublicValidatorAnalyticsResponse
  | AdminValidatorAnalyticsResponse

type DailySnapshot = PublicValidatorDailySnapshot | AdminValidatorDailySnapshot
type MonthlyAggregate = PublicValidatorMonthlyAggregate | AdminValidatorMonthlyAggregate

function stateLabel(state: string): string {
  switch (state) {
    case 'fresh': return 'Fresh'
    case 'stale': return 'Stale'
    case 'error': return 'Error'
    case 'unsupported': return 'Unsupported'
    case 'not_found': return 'Not found'
    case 'empty': return 'Empty'
    default: return 'Unknown'
  }
}

function stateTone(state: string): 'ok' | 'warning' | 'error' | 'neutral' {
  switch (state) {
    case 'fresh': return 'ok'
    case 'stale': return 'warning'
    case 'error': return 'error'
    case 'unsupported': return 'warning'
    default: return 'neutral'
  }
}

function isAdminDaily(day: DailySnapshot): day is AdminValidatorDailySnapshot {
  return 'receivedAt' in day
}

function isAdminMonthly(month: MonthlyAggregate): month is AdminValidatorMonthlyAggregate {
  return 'updatedAt' in month
}

function formatNumber(value: number | null | undefined): string {
  return value == null ? 'Unknown' : String(value)
}

function formatText(value: string | null | undefined): string {
  return value ?? 'Unknown'
}

function parseRank(day: DailySnapshot): number | null {
  return day.rank ?? null
}

const TH_CLASS = 'border-b px-2 py-2 text-left text-xs font-medium text-muted-foreground'
const TD_CLASS = 'border-b border-border/60 px-2 py-2 align-top'

/** Minimal, accessible inline bar chart. The table below always carries the
 * same data as a text alternative, satisfying the chart/table alternative
 * requirement without hover-only interaction. */
function RankChart({ days, id, compact }: { days: DailySnapshot[]; id: string; compact?: boolean }) {
  const ranks = days
    .map((day) => ({ date: day.localDate, rank: parseRank(day) }))
    .filter((entry): entry is { date: string; rank: number } => entry.rank !== null)
    .slice(0, 31)
  if (ranks.length === 0) {
    return <p className="m-0 text-sm text-muted-foreground">No ranked daily samples yet.</p>
  }
  const maxRank = Math.max(1, ...ranks.map((entry) => entry.rank))
  const height = 64
  const width = 320
  const barWidth = Math.max(4, Math.floor(width / ranks.length) - 2)
  return (
    <svg
      className={cn('block w-full text-foreground', compact && 'max-h-24')}
      viewBox={`0 0 ${width} ${height}`}
      role="img"
      aria-labelledby={id}
      preserveAspectRatio="none"
      style={{ width: '100%', height: 'auto', maxHeight: '8rem' }}
    >
      <title id={id}>Daily validator rank trend. Lower is better.</title>
      {ranks.map((entry, index) => {
        const barHeight = Math.max(4, Math.round((entry.rank / maxRank) * (height - 8)))
        const x = index * (barWidth + 2)
        const y = height - barHeight
        return (
          <rect
            key={entry.date}
            x={x}
            y={y}
            width={barWidth}
            height={barHeight}
            rx={1}
            fill="currentColor"
            opacity={0.75}
          >
            <title>{`${entry.date}: rank ${entry.rank}`}</title>
          </rect>
        )
      })}
    </svg>
  )
}

export function ValidatorAnalytics({
  analytics,
  compact = false,
}: {
  analytics: ValidatorAnalyticsResponse
  compact?: boolean
}) {
  const daily = analytics.daily
  const monthly = analytics.monthly
  const chartId = `validator-rank-chart-${analytics.validatorId}`

  return (
    <section
      className={cn('min-w-0', compact && 'mt-3 text-sm')}
      aria-label={`Validator analytics for ${analytics.validatorId}`}
    >
      <h3 className="m-0 text-sm font-medium">Validator analytics</h3>
      <p className="m-0 mt-1 flex flex-wrap items-center gap-2">
        <StatusBadge status={stateLabel(analytics.state)} tone={stateTone(analytics.state)} />
        <span className="text-[11px] text-muted-foreground">
          {analytics.freshness === 'unknown' ? 'Never observed' : analytics.freshness}
        </span>
      </p>

      {daily.length === 0 && monthly.length === 0 && (
        <p className="m-0 mt-2 text-sm text-muted-foreground">No Validator analytics yet.</p>
      )}

      {daily.length > 0 && (
        <>
          <RankChart days={daily} id={chartId} compact={compact} />
          <div className="mt-3 w-full overflow-x-auto">
            <table className="w-full border-collapse text-sm">
              <caption className="sr-only">Daily Validator snapshots</caption>
              <thead>
                <tr>
                  <th scope="col" className={TH_CLASS}>Local date</th>
                  <th scope="col" className={TH_CLASS}>Rank</th>
                  <th scope="col" className={TH_CLASS}>Stake</th>
                  <th scope="col" className={TH_CLASS}>Reward</th>
                  <th scope="col" className={TH_CLASS}>Reward rate</th>
                  <th scope="col" className={TH_CLASS}>Blocks</th>
                  {daily.some(isAdminDaily) && <th scope="col" className={TH_CLASS}>Received</th>}
                  {daily.some(isAdminDaily) && <th scope="col" className={TH_CLASS}>Source</th>}
                </tr>
              </thead>
              <tbody>
                {daily.map((day) => (
                  <tr key={`${day.localDate}-${day.timezone}-${day.sampleAt}`} className="border-b border-border/60">
                    <td data-label="Local date" className={TD_CLASS}>{day.localDate}</td>
                    <td data-label="Rank" className={TD_CLASS}>{formatNumber(day.rank)}</td>
                    <td data-label="Stake" className={TD_CLASS}>{formatText(day.stakeAmount)}</td>
                    <td data-label="Reward" className={TD_CLASS}>{formatText(day.rewardAmount)}</td>
                    <td data-label="Reward rate" className={TD_CLASS}>{formatText(day.rewardRate)}</td>
                    <td data-label="Blocks" className={TD_CLASS}>{formatNumber(day.blockCount)}</td>
                    {isAdminDaily(day) && <td data-label="Received" className={TD_CLASS}>{formatText(day.receivedAt)}</td>}
                    {isAdminDaily(day) && <td data-label="Source" className={TD_CLASS}>{formatText(day.source)}</td>}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </>
      )}

      {monthly.length > 0 && (
        <div className="mt-3 w-full overflow-x-auto">
          <table className="w-full border-collapse text-sm">
            <caption className="sr-only">Monthly Validator aggregates</caption>
            <thead>
              <tr>
                <th scope="col" className={TH_CLASS}>Month</th>
                <th scope="col" className={TH_CLASS}>Samples</th>
                <th scope="col" className={TH_CLASS}>Rank min/max/last</th>
                <th scope="col" className={TH_CLASS}>Stake last</th>
                <th scope="col" className={TH_CLASS}>Reward last</th>
                <th scope="col" className={TH_CLASS}>Reward rate last</th>
                <th scope="col" className={TH_CLASS}>Blocks last</th>
                {monthly.some(isAdminMonthly) && <th scope="col" className={TH_CLASS}>Updated</th>}
              </tr>
            </thead>
            <tbody>
              {monthly.map((month) => (
                <tr key={`${month.monthKey}-${month.timezone}`} className="border-b border-border/60">
                  <td data-label="Month" className={TD_CLASS}>{month.monthKey}</td>
                  <td data-label="Samples" className={TD_CLASS}>{month.snapshotCount}</td>
                  <td data-label="Rank min/max/last" className={TD_CLASS}>
                    {formatNumber(month.rankMin)} / {formatNumber(month.rankMax)} /{' '}
                    {formatNumber(month.rankLast)}
                  </td>
                  <td data-label="Stake last" className={TD_CLASS}>{formatText(month.stakeLast)}</td>
                  <td data-label="Reward last" className={TD_CLASS}>{formatText(month.rewardLast)}</td>
                  <td data-label="Reward rate last" className={TD_CLASS}>{formatText(month.rewardRateLast)}</td>
                  <td data-label="Blocks last" className={TD_CLASS}>{formatNumber(month.blockCountLast)}</td>
                  {isAdminMonthly(month) && <td data-label="Updated" className={TD_CLASS}>{formatText(month.updatedAt)}</td>}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  )
}
