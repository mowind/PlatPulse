import type { PublicMetricGap, PublicMetricPoint, PublicMetricSeriesCoverage } from './api/generated'
import { formatHistoryDuration } from './metricHistory'

/** One observed sample, in the Server's own units and the Server's own instant. */
export type MetricObservation = { sampledAt: number; value: number }

/**
 * Observation/coverage evidence the Server answered for one plotted direction.
 * Absent for responses that predate the contract, which is why every reader
 * treats "no coverage" as "nothing to say" rather than as "nothing observed".
 */
export function metricCoverage(
  history: { series?: PublicMetricSeriesCoverage[] } | undefined,
  metric: string,
): PublicMetricSeriesCoverage | undefined {
  return history?.series?.find((entry) => entry.metric === metric)
}

/**
 * The window's observations cut into runs at the silence the Server measured:
 * the run ends at the last observation before the gap and resumes at the first
 * one after it, so a chart built from these can never join two stretches nobody
 * observed into one curve. Observations outside [from, to] are dropped rather
 * than clamped onto the edges — the window is the evidence — and nothing is ever
 * synthesised, so a measured zero stays a measurement while a silence stays a
 * silence.
 *
 * This is the single seam every Node Detail chart reads: the real-valued runs
 * go straight onto a time axis, and the chart's accessible description is
 * written from the same coverage the runs were cut with.
 */
export function coverageSegments(
  points: PublicMetricPoint[],
  from: number,
  to: number,
  gaps: PublicMetricGap[] = [],
): MetricObservation[][] {
  const silence = gaps
    .map((gap) => ({ from: Date.parse(gap.from), to: Date.parse(gap.to) }))
    .filter((gap) => Number.isFinite(gap.from) && Number.isFinite(gap.to))
  const segments: MetricObservation[][] = []
  let previous: number | undefined
  const ordered = points
    .map((point) => ({ sampledAt: Date.parse(point.sampledAt), value: point.value }))
    .filter((point) => Number.isFinite(point.sampledAt) && Number.isFinite(point.value) && point.sampledAt >= from && point.sampledAt <= to)
    .sort((left, right) => left.sampledAt - right.sampledAt)
  for (const point of ordered) {
    // A run breaks only across the silence itself: the segment ending at the
    // last observation before a gap and the segment starting at the first one
    // after it both survive.
    const separated = previous !== undefined && silence.some((gap) => gap.from < point.sampledAt && gap.to > (previous as number))
    if (previous === undefined || separated) segments.push([])
    segments.at(-1)?.push(point)
    previous = point.sampledAt
  }
  return segments
}

/**
 * What the curve cannot show, in the evidence's own words. A series nobody
 * observed says so instead of reading as a flat line; a stale series names the
 * age of its last observation; a decided cadence that measured silence inside
 * the window counts it. An unknown cadence (threshold 0) never claims silence,
 * and neither does an absence the Server cannot prove: a series whose evidence
 * may have expired unrecorded is answered as unknown, in the evidence's own
 * words, rather than as a series nobody observed (issue #225, Story 49).
 */
export function coverageNote(coverage: PublicMetricSeriesCoverage | undefined): string | undefined {
  if (!coverage) return undefined
  if (!coverage.observed) {
    // Absence is only a proof that nobody ever reported the series where the
    // Server keeps a ledger for it: the two block series have none, so once
    // retention removes their rows the Server holds no evidence at all and this
    // says what is true instead of claiming the series was never observed.
    return coverage.neverObservedProven ? 'No samples reported yet' : 'No retained observations'
  }
  if (coverage.observationCount === 0) {
    return coverage.unobservedTailSeconds > 0
      ? 'Last observation ' + formatHistoryDuration(coverage.unobservedTailSeconds) + ' ago'
      : undefined
  }
  const parts: string[] = []
  const gaps = coverage.gaps.length
  if (gaps > 0) parts.push(gaps === 1 ? '1 gap in this window' : gaps + ' gaps in this window')
  if (coverage.gapThresholdSeconds > 0 && coverage.unobservedTailSeconds >= coverage.gapThresholdSeconds) {
    parts.push('No samples for the last ' + formatHistoryDuration(coverage.unobservedTailSeconds))
  }
  return parts.length > 0 ? parts.join(' · ') : undefined
}

/**
 * Card-level sentence for the chart's accessible description: one sentence only
 * when every direction has the same one to give, labelled directions otherwise,
 * and nothing at all when there is nothing to say. One silent direction beside a
 * healthy one must never read as the whole card going quiet, and neither must a
 * direction whose answer carries no coverage at all: both keep the label of the
 * direction that does report.
 */
export function coverageNotes(series: { label: string; coverage?: PublicMetricSeriesCoverage }[]): string | undefined {
  const noted = series
    .map((item) => ({ label: item.label, note: coverageNote(item.coverage) }))
    .filter((item): item is { label: string; note: string } => item.note !== undefined)
  if (noted.length === 0) return undefined
  const distinct = new Set(noted.map((item) => item.note))
  if (noted.length === series.length && distinct.size === 1) return noted[0]?.note
  return noted.map((item) => item.label + ': ' + item.note).join(' · ')
}
