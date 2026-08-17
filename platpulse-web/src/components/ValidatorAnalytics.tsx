import type {
  AdminValidatorAnalyticsResponse,
  AdminValidatorDailySnapshot,
  AdminValidatorMonthlyAggregate,
  PublicValidatorAnalyticsResponse,
  PublicValidatorDailySnapshot,
  PublicValidatorMonthlyAggregate,
} from '../api/generated'

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

/** Minimal, accessible inline bar chart. The table below always carries the
 * same data as a text alternative, satisfying the chart/table alternative
 * requirement without hover-only interaction. */
function RankChart({ days, id }: { days: DailySnapshot[]; id: string }) {
  const ranks = days
    .map((day) => ({ date: day.localDate, rank: parseRank(day) }))
    .filter((entry): entry is { date: string; rank: number } => entry.rank !== null)
    .slice(0, 31)
  if (ranks.length === 0) {
    return <p className="muted">No ranked daily samples yet.</p>
  }
  const maxRank = Math.max(1, ...ranks.map((entry) => entry.rank))
  const height = 64
  const width = 320
  const barWidth = Math.max(4, Math.floor(width / ranks.length) - 2)
  return (
    <svg
      className="validator-rank-chart"
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
      className={`validator-analytics${compact ? ' validator-analytics-compact' : ''}`}
      aria-label={`Validator analytics for ${analytics.validatorId}`}
    >
      <h3>Validator analytics</h3>
      <p>
        <span className={`status status-${analytics.state}`}>{stateLabel(analytics.state)}</span>
        {' · '}
        {analytics.freshness === 'unknown' ? 'Never observed' : analytics.freshness}
      </p>

      {daily.length === 0 && monthly.length === 0 && (
        <p className="muted">No Validator analytics yet.</p>
      )}

      {daily.length > 0 && (
        <>
          <RankChart days={daily} id={chartId} />
          <div className="table-wrap">
            <table className="node-table validator-daily-table">
              <caption className="sr-only">Daily Validator snapshots</caption>
              <thead>
                <tr>
                  <th scope="col">Local date</th>
                  <th scope="col">Rank</th>
                  <th scope="col">Stake</th>
                  <th scope="col">Reward</th>
                  <th scope="col">Reward rate</th>
                  <th scope="col">Blocks</th>
                  {daily.some(isAdminDaily) && <th scope="col">Received</th>}
                  {daily.some(isAdminDaily) && <th scope="col">Source</th>}
                </tr>
              </thead>
              <tbody>
                {daily.map((day) => (
                  <tr key={`${day.localDate}-${day.timezone}-${day.sampleAt}`}>
                    <td data-label="Local date">{day.localDate}</td>
                    <td data-label="Rank">{formatNumber(day.rank)}</td>
                    <td data-label="Stake">{formatText(day.stakeAmount)}</td>
                    <td data-label="Reward">{formatText(day.rewardAmount)}</td>
                    <td data-label="Reward rate">{formatText(day.rewardRate)}</td>
                    <td data-label="Blocks">{formatNumber(day.blockCount)}</td>
                    {isAdminDaily(day) && <td data-label="Received">{formatText(day.receivedAt)}</td>}
                    {isAdminDaily(day) && <td data-label="Source">{formatText(day.source)}</td>}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </>
      )}

      {monthly.length > 0 && (
        <div className="table-wrap">
          <table className="node-table validator-monthly-table">
            <caption className="sr-only">Monthly Validator aggregates</caption>
            <thead>
              <tr>
                <th scope="col">Month</th>
                <th scope="col">Samples</th>
                <th scope="col">Rank min/max/last</th>
                <th scope="col">Stake last</th>
                <th scope="col">Reward last</th>
                <th scope="col">Reward rate last</th>
                <th scope="col">Blocks last</th>
                {monthly.some(isAdminMonthly) && <th scope="col">Updated</th>}
              </tr>
            </thead>
            <tbody>
              {monthly.map((month) => (
                <tr key={`${month.monthKey}-${month.timezone}`}>
                  <td data-label="Month">{month.monthKey}</td>
                  <td data-label="Samples">{month.snapshotCount}</td>
                  <td data-label="Rank min/max/last">
                    {formatNumber(month.rankMin)} / {formatNumber(month.rankMax)} /{' '}
                    {formatNumber(month.rankLast)}
                  </td>
                  <td data-label="Stake last">{formatText(month.stakeLast)}</td>
                  <td data-label="Reward last">{formatText(month.rewardLast)}</td>
                  <td data-label="Reward rate last">{formatText(month.rewardRateLast)}</td>
                  <td data-label="Blocks last">{formatNumber(month.blockCountLast)}</td>
                  {isAdminMonthly(month) && <td data-label="Updated">{formatText(month.updatedAt)}</td>}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  )
}