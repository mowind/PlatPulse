// Node metric history presentation (issues #213 and #214, design §11.4 and
// §11.6).
//
// The Server answers with the stored observations of the raw window, the
// aggregate buckets that answer the stretches beyond it, the silences between
// them, and the state of the series itself. The browser only shapes that
// answer: a 24 hour window at a 5 second cadence is a few tens of thousands of
// points, so points are folded into at most METRIC_HISTORY_MAX_COLUMNS columns,
// each carrying its own minimum/maximum so a spike is never averaged away. A
// bucket keeps the range and the count of the observations it summarizes and is
// never drawn as one stored sample; a raw sample is never approximated here.

/** Stored Node metric series (mirrors the Server's NODE_METRIC_SERIES). */
export type NodeMetricKey =
  | 'process_cpu_percent'
  | 'process_memory_percent'
  | 'data_directory_percent'
  | 'peer_inbound_count'
  | 'peer_outbound_count'

export type NodeMetricDefinition = {
  metric: NodeMetricKey
  label: string
  /** Unit line under the title. */
  unit: string
  /** Y-axis label for a value, used for the top, middle, and zero gridlines. */
  axisFormat: (value: number) => string
  /** One observation, as shown in the newest-samples table. */
  format: (value: number) => string
  /** A fixed axis maximum when the series has a natural ceiling. */
  fixedMax?: number
  description: string
}

const percent = (value: number) => value.toFixed(value >= 10 ? 0 : 1) + '%'
const count = (value: number) => Math.round(value).toString()

export const NODE_METRIC_SERIES: NodeMetricDefinition[] = [
  {
    metric: 'process_cpu_percent',
    label: 'Process CPU',
    unit: 'percent of one core, as observed by the Agent',
    axisFormat: percent,
    format: percent,
    description:
      'Process CPU has no ceiling: a multi-core deployment can exceed 100 percent of one core.',
  },
  {
    metric: 'process_memory_percent',
    label: 'Process memory',
    unit: 'percent of Host memory',
    axisFormat: percent,
    format: percent,
    fixedMax: 100,
    description: 'Process memory is a share of the Host total reported by the Agent.',
  },
  {
    metric: 'data_directory_percent',
    label: 'Data directory',
    unit: 'percent of the declared capacity',
    axisFormat: percent,
    format: percent,
    fixedMax: 100,
    description: 'Data directory usage is measured against the capacity the Agent declares.',
  },
  {
    metric: 'peer_inbound_count',
    label: 'Peer inbound',
    unit: 'inbound peer connections',
    axisFormat: count,
    format: (value) => Math.round(value).toString(),
    description: 'Inbound peer connections reported by this Node.',
  },
  {
    metric: 'peer_outbound_count',
    label: 'Peer outbound',
    unit: 'outbound peer connections',
    axisFormat: count,
    format: (value) => Math.round(value).toString(),
    description: 'Outbound peer connections reported by this Node.',
  },
]

export function nodeMetricDefinition(metric: string): NodeMetricDefinition {
  return NODE_METRIC_SERIES.find((item) => item.metric === metric) ?? NODE_METRIC_SERIES[0]
}

/** Range presets the Owner can pick. The raw window is 24 hours by default;
 * beyond it the 1 minute and 5 minute aggregate tiers answer, up to the 30 day
 * investigation horizon (issue #214, design §11.6), so the two longer lengths
 * are offered as well and the answer says which tier served which stretch. */
export const METRIC_HISTORY_PRESETS = [
  { label: '1 hour', hours: 1 },
  { label: '6 hours', hours: 6 },
  { label: '24 hours', hours: 24 },
  { label: '7 days', hours: 168 },
  { label: '30 days', hours: 720 },
] as const

/** Columns the plot draws for any window: enough resolution on a 1440px
 * desktop, few enough that a 5 second cadence stays readable. */
export const METRIC_HISTORY_MAX_COLUMNS = 120

/** The largest number of stored observations one answer carries, mirroring the
 * Server's MAX_SAMPLE_LIMIT
 * (crates/platpulse-server/src/metric_history.rs). The panel asks for as many
 * samples as a single answer can hold, so a series is narrowed by the Server's
 * own bound rather than by a request that asked for less than it could have.
 * At the fastest cadence an Agent may be configured with (1 second,
 * crates/platpulse-agent/src/config.rs), a full 24 hour window is 86400
 * observations and the answer is still truncated to its newest part; the panel
 * says so instead of drawing a partial window as if it were complete. */
export const METRIC_HISTORY_SAMPLE_LIMIT = 20000

const CHART_WIDTH = 600
const CHART_TOP = 8
const CHART_BOTTOM = 142
const CHART_SPAN = CHART_BOTTOM - CHART_TOP

/** Canonical second-precision RFC 3339, matching the Server's format. */
export function formatCanonicalInstant(date: Date): string {
  return date.toISOString().replace(/\.\d{3}Z$/, 'Z')
}

/** The requested range for a preset, computed once per selection so the query
 * key stays stable while the page is open. */
export function metricHistoryRange(hours: number, now: Date): { from: string; to: string } {
  const seconds = Math.max(1, Math.round(hours * 3600))
  return {
    from: formatCanonicalInstant(new Date(now.getTime() - seconds * 1000)),
    to: formatCanonicalInstant(now),
  }
}

/**
 * One point of an answer: a stored observation (issue #213) or an aggregate
 * bucket that summarizes observations the raw window no longer holds (issue
 * #214). The tier fields are optional so a point that carries only an instant
 * and a value is still read as the one stored observation it is.
 */
export type MetricHistorySample = {
  observedAt: string
  value: number
  /** The grain the Server served this point at: 'raw', '1m' or '5m'. */
  grain?: string
  /** 'raw' for a stored observation, 'aggregate' for a bucket. */
  source?: string
  /** The extremes of the observations behind this point. A raw sample is its
   * own extreme; a bucket keeps the spike the raw samples would have shown. */
  minValue?: number
  maxValue?: number
  /** Observations behind this point: 1 for a raw sample, the bucket's counted
   * observations for a bucket. */
  sampleCount?: number
  /** The newest observation this point holds. Absent means observedAt is it. */
  lastObservedAt?: string
  /** The oldest observation this point holds: the other end of the stretch the
   * point really testifies to, equal to observedAt for a stored observation. A
   * bucket's own observedAt is the aligned start of its window, which no
   * observation has to support, so a surface that says how long a point covers
   * measures from this instants instead of from the bucket start. */
  firstObservedAt?: string
  /** The widest interval the bucket measured between two consecutive
   * observations of its own: 0 for a stored observation and for a bucket that
   * counted one. Zero means the observations it counted were contiguous, never
   * that nobody measured them. */
  maxGapSeconds?: number
}

export type MetricColumn = {
  /** Where the column is drawn in the 0..600 viewBox: the centre of the
   * instants this column really holds, so it can never sit inside a silence
   * the Server reported between two of them. */
  x: number
  min: number
  max: number
  /** Newest value in the bucket, used for the connecting line. */
  last: number
  count: number
  /** Observations this column represents: the sum of the counts behind its
   * points, so a column holding aggregate buckets reports the evidence they
   * summarize rather than one observation per bucket. Equal to count while
   * every point is a stored observation. */
  weight: number
  /** Folded points the Server served from aggregate buckets, and folded points
   * that were stored observations. Both can be non-zero in one column: the
   * stretch where the raw window ends holds samples of one tier and buckets of
   * the next. */
  aggregateCount: number
  rawCount: number
  /** Oldest stored instant folded into the bucket, epoch milliseconds. */
  firstInstant: number
  /** Newest stored instant folded into the bucket, epoch milliseconds: the
   * connecting line ends here, and a silence the Server reported from this
   * observation is the one this column breaks on. A bucket ends at its newest
   * observation, which is inside the bucket. */
  lastInstant: number
  /** The widest interval measured between two consecutive observations of the
   * points folded here, 0 when none of them measured one. */
  maxGapSeconds: number
  /** Whether any point folded here proved a hole inside the stretch it stands
   * for: the column's own evidence was not continuous, so no line is drawn
   * across it. */
  provenHole: boolean
}

export type MetricColumnPoint = MetricColumn & {
  /** Column top (maximum) and bottom (minimum) in viewBox units. */
  top: number
  bottom: number
  /** Column centre height in viewBox units. */
  y: number
}

export type MetricGapBand = { x: number; width: number }

export type MetricChartGeometry = {
  segments: MetricColumnPoint[][]
  gapBands: MetricGapBand[]
  max: number
  /** Points folded into the drawn columns. */
  samples: number
  /** Observations those points represent, so a plot of aggregate buckets can
   * say how much evidence it stands for. Equal to samples while every point is
   * a stored observation. */
  weight: number
  /** Folded points the Server served from aggregate buckets. */
  aggregatedPoints: number
  /** Folded points that were stored observations. */
  rawPoints: number
  /** Folded columns whose own evidence proved a hole inside the stretch they
   * stand for: they are drawn as the markers they are, never joined by a line
   * to either neighbour. */
  holedPoints: number
  /** The cadence the answer itself showed, in seconds: 0 when it holds fewer
   * than two stored observations to measure one from, which is when each bucket
   * is judged by the grain it was folded at. */
  cadenceSeconds: number
}

/** Whether a point was served from an aggregate bucket rather than from a
 * stored observation (issue #214). A point that carries no tier information is
 * read as the raw sample it was before the tiers existed. */
export function metricPointIsAggregate(sample: { source?: string; grain?: string }): boolean {
  if (typeof sample.source === 'string' && sample.source.length > 0) {
    return sample.source === 'aggregate'
  }
  return typeof sample.grain === 'string' && sample.grain.length > 0 && sample.grain !== 'raw'
}

/** The observations one point stands for: the bucket's counted observations,
 * or the single observation a raw sample is. */
export function metricPointWeight(sample: { sampleCount?: number }): number {
  const count = sample.sampleCount
  return typeof count === 'number' && Number.isFinite(count) && count > 0 ? Math.round(count) : 1
}

/** Seconds one bucket of a grain covers, or 0 for a point that was not folded
 * into buckets at all. Mirrors the Server's own grain widths. */
export function metricGrainSeconds(grain?: string): number {
  if (grain === '1m') return 60
  if (grain === '5m') return 300
  return 0
}

/** The shortest silence that is ever a break, whatever the cadence is
 * (crates/platpulse-server/src/metric_history.rs, MIN_GAP_SECONDS). */
export const METRIC_HISTORY_MIN_GAP_SECONDS = 120

/** How many times the series' own cadence an interval has to span before it is
 * a break rather than the sampling rhythm (GAP_CADENCE_FACTOR). The Server is
 * never told an Agent's sampling interval, so both it and this plot measure the
 * cadence the answer actually showed. */
export const METRIC_HISTORY_GAP_CADENCE_FACTOR = 3

/** The slowest collection interval an Agent may be configured with, which caps
 * a measured cadence (MAX_OBSERVED_CADENCE_SECONDS): without the cap two
 * observations a day apart would make the whole day look like a believable
 * cadence and report a stretch nobody observed as covered. */
export const METRIC_HISTORY_MAX_CADENCE_SECONDS = 300

/** The interval at which a hole stops being the sampling rhythm and becomes a
 * break in the evidence: max(factor x cadence, the 2 minute floor), the same
 * judgement the Server's read path makes (gap_threshold_seconds).
 *
 * The cadence is truncated rather than rounded because the Server clamps an
 * integer number of seconds: a bucket of seven observations over 300 seconds is
 * judged at 42 seconds and a 126 second hole, and a plot that rounded 42.857 up
 * to 43 would call that bucket continuous while the Server reports it holed. */
export function metricGapThresholdSeconds(cadenceSeconds: number): number {
  const cadence = Number.isFinite(cadenceSeconds)
    ? Math.max(1, Math.min(METRIC_HISTORY_MAX_CADENCE_SECONDS, Math.floor(cadenceSeconds)))
    : 1
  return Math.max(METRIC_HISTORY_MIN_GAP_SECONDS, cadence * METRIC_HISTORY_GAP_CADENCE_FACTOR)
}

/** The cadence an answer showed: the fastest interval between two consecutive
 * stored observations it holds, mirroring the Server's observed_cadence_seconds.
 * 0 when it holds fewer than two observations, which is also the Server's answer
 * and the case where a bucket is judged by its own grain instead. */
export function metricMeasuredCadenceSeconds(samples: MetricHistorySample[]): number {
  let previous: number | undefined
  let fastest = 0
  for (const sample of samples) {
    // Buckets are judged by the observations they counted, not as observations
    // of their own: reading a bucket's aligned start as a sampling instant would
    // invent a cadence the series never had.
    if (metricPointIsAggregate(sample)) continue
    const instant = Date.parse(sample.observedAt)
    if (!Number.isFinite(instant)) continue
    if (previous !== undefined) {
      const delta = (instant - previous) / 1000
      if (delta > 0 && (fastest === 0 || delta < fastest)) fastest = delta
    }
    previous = instant
  }
  return fastest
}

/** The cadence one point is judged by, mirroring the Server's
 * judged_cadence_seconds: the cadence the answer measured when it measured one;
 * otherwise a bucket by the stretch its own observations average out to (its
 * grain over the observations it counted, truncated as the Server's integer
 * division truncates it), and a stored observation by a cadence the answer could
 * not measure. */
export function metricJudgedCadenceSeconds(
  sample: MetricHistorySample,
  measuredCadenceSeconds = 0,
): number {
  if (Number.isFinite(measuredCadenceSeconds) && measuredCadenceSeconds > 0) {
    return measuredCadenceSeconds
  }
  if (!metricPointIsAggregate(sample)) return 0
  const grain = metricGrainSeconds(sample.grain)
  if (grain <= 0) return 0
  // Integer seconds, as the Server's judged cadence is: 300 seconds over seven
  // observations is 42 seconds there, and a fractional 42.857 here would put the
  // two judgements 3 seconds apart at the threshold.
  return Math.max(1, Math.floor(grain / metricPointWeight(sample)))
}

/** Whether a point's own recorded evidence proves a hole inside the stretch it
 * stands for: the widest interval it measured between two consecutive
 * observations of its own reaches the gap threshold its judged cadence implies.
 * A point that measured none — a stored observation, or a bucket that counted a
 * single one — proves nothing either way, because 0 is the contiguity of what it
 * counted and never an unmeasured stretch. */
export function metricPointProvesHole(
  sample: MetricHistorySample,
  measuredCadenceSeconds = 0,
): boolean {
  const gap = sample.maxGapSeconds
  if (typeof gap !== 'number' || !Number.isFinite(gap) || gap <= 0) return false
  const threshold = metricGapThresholdSeconds(
    metricJudgedCadenceSeconds(sample, measuredCadenceSeconds),
  )
  return gap >= threshold
}

/** The newest instant a point really holds: a bucket ends at the newest
 * observation it counted, a raw sample is its own instant. */
function newestInstant(sample: MetricHistorySample): number {
  const instant = Date.parse(sample.observedAt)
  const newest = typeof sample.lastObservedAt === 'string' ? Date.parse(sample.lastObservedAt) : NaN
  return Number.isFinite(newest) && newest > instant ? newest : instant
}

/** The oldest instant a point really holds: the first observation behind a
 * bucket, or the single instant a stored observation is. A bucket's own
 * observedAt is deliberately not used as an end when the answer names the
 * observation: the aligned start of the window is a position no observation has
 * to support, so measuring from it would report a stretch that was never
 * observed as covered. An answer served before the field existed names only the
 * aligned instant, and there is nothing better to measure from. */
function firstInstant(sample: MetricHistorySample): number {
  const first = typeof sample.firstObservedAt === 'string' ? Date.parse(sample.firstObservedAt) : NaN
  if (Number.isFinite(first)) return first
  return Date.parse(sample.observedAt)
}

/** A readable axis maximum: 1/2/5 steps above the observed maximum. */
export function niceMetricMax(value: number): number {
  if (!Number.isFinite(value) || value <= 0) return 1
  const magnitude = Math.pow(10, Math.floor(Math.log10(value)))
  const scaled = value / magnitude
  const step = scaled <= 1 ? 1 : scaled <= 2 ? 2 : scaled <= 5 ? 5 : 10
  return step * magnitude
}

function chartX(instant: number, from: number, to: number): number {
  if (!Number.isFinite(instant) || to <= from) return 0
  const ratio = (instant - from) / (to - from)
  return Math.max(0, Math.min(CHART_WIDTH, ratio * CHART_WIDTH))
}

/**
 * Fold samples into at most `maxColumns` equal time buckets. Buckets are
 * time-based, not count-based, so a burst of samples cannot stretch a stretch
 * of the window and an empty stretch keeps its real width. Samples whose
 * timestamps or values are unusable are dropped rather than plotted as zero.
 *
 * Callers that know where the silence is (metricChartGeometry) split the
 * observations at every reported silence before folding them: a bucket is a
 * drawing unit, and one bucket can hold observations the Server says were
 * never continuous with each other.
 *
 * A bucket can also hold a hole the Server measured inside it (maxGapSeconds):
 * the column it is folded into is then marked as proved not continuous, so the
 * plot stops its line there instead of drawing one stretch across evidence that
 * was never continuous.
 *
 * A point that came from an aggregate bucket contributes what the Server kept
 * for it: its counted observations, its minimum and its maximum (issue #214,
 * design §11.6). Folding a long range into 120 columns therefore never averages
 * away the spike a bucket preserved, and never claims a bucket was one sample.
 */
export function bucketMetricSamples(
  samples: MetricHistorySample[],
  from: number,
  to: number,
  maxColumns: number = METRIC_HISTORY_MAX_COLUMNS,
  measuredCadenceSeconds = 0,
): MetricColumn[] {
  if (!Number.isFinite(from) || !Number.isFinite(to) || to <= from) return []
  const columns = Math.max(1, Math.floor(maxColumns))
  const width = to - from
  const buckets = new Map<
    number,
    {
      index: number
      min: number
      max: number
      last: number
      firstInstant: number
      lastInstant: number
      count: number
      weight: number
      aggregateCount: number
      rawCount: number
      maxGapSeconds: number
      provenHole: boolean
    }
  >()
  for (const sample of samples) {
    const instant = Date.parse(sample.observedAt)
    if (!Number.isFinite(instant) || !Number.isFinite(sample.value)) continue
    // A bucket answers for a stretch, not for one instant: the extremes it kept
    // are the extremes of the observations it counted, and it ends at its
    // newest observation. A raw sample is its own single instant and value.
    const min =
      typeof sample.minValue === 'number' && Number.isFinite(sample.minValue)
        ? Math.min(sample.minValue, sample.value)
        : sample.value
    const max =
      typeof sample.maxValue === 'number' && Number.isFinite(sample.maxValue)
        ? Math.max(sample.maxValue, sample.value)
        : sample.value
    const weight = metricPointWeight(sample)
    const aggregate = metricPointIsAggregate(sample)
    const newest = newestInstant(sample)
    // The widest interval this point measured between two of its own
    // consecutive observations, and whether that interval is wide enough to be
    // a hole rather than the cadence the answer itself showed.
    const gap =
      typeof sample.maxGapSeconds === 'number' &&
      Number.isFinite(sample.maxGapSeconds) &&
      sample.maxGapSeconds > 0
        ? sample.maxGapSeconds
        : 0
    const provesHole = metricPointProvesHole(sample, measuredCadenceSeconds)
    const clamped = Math.max(from, Math.min(to, instant))
    const index = Math.min(columns - 1, Math.floor(((clamped - from) / width) * columns))
    const bucket = buckets.get(index)
    if (!bucket) {
      buckets.set(index, {
        index,
        min,
        max,
        last: sample.value,
        firstInstant: instant,
        lastInstant: newest,
        count: 1,
        weight,
        aggregateCount: aggregate ? 1 : 0,
        rawCount: aggregate ? 0 : 1,
        maxGapSeconds: gap,
        provenHole: provesHole,
      })
      continue
    }
    bucket.min = Math.min(bucket.min, min)
    bucket.max = Math.max(bucket.max, max)
    bucket.firstInstant = Math.min(bucket.firstInstant, instant)
    // A column holds what its points hold: if any of them measured a hole, the
    // whole column was not continuous, and the plot must not hide that because
    // another point folded into the same column happened to be continuous.
    bucket.maxGapSeconds = Math.max(bucket.maxGapSeconds, gap)
    bucket.provenHole = bucket.provenHole || provesHole
    bucket.count += 1
    bucket.weight += weight
    if (aggregate) bucket.aggregateCount += 1
    else bucket.rawCount += 1
    // The column's newest reading is the one whose own observation is newest,
    // so a bucket held back by spooling cannot pass its older reading off as
    // the newest thing this column holds.
    if (newest > bucket.lastInstant || (newest === bucket.lastInstant && instant >= bucket.lastInstant)) {
      bucket.lastInstant = newest
      bucket.last = sample.value
    }
  }
  return [...buckets.values()]
    .sort((left, right) => left.index - right.index)
    .map((bucket) => ({
      // Drawn where this column's own observations are, not at the centre of
      // the bucket it fell in: the bucket centre is a position no observation
      // in this column has to support, and a reported silence between two of
      // them can cover it. A column holding an aggregate bucket therefore sits
      // at the middle of the stretch that bucket really covers.
      x: chartX((bucket.firstInstant + bucket.lastInstant) / 2, from, to),
      min: bucket.min,
      max: bucket.max,
      last: bucket.last,
      count: bucket.count,
      weight: bucket.weight,
      aggregateCount: bucket.aggregateCount,
      rawCount: bucket.rawCount,
      firstInstant: bucket.firstInstant,
      lastInstant: bucket.lastInstant,
      maxGapSeconds: bucket.maxGapSeconds,
      provenHole: bucket.provenHole,
    }))
}

/** A reported silence, in epoch milliseconds. */
type SilenceInterval = { from: number; to: number }

function silenceIntervals(gaps: { from: string; to: string }[]): SilenceInterval[] {
  const silences: SilenceInterval[] = []
  for (const gap of gaps) {
    const from = Date.parse(gap.from)
    const to = Date.parse(gap.to)
    if (!Number.isFinite(from) || !Number.isFinite(to) || to <= from) continue
    silences.push({ from, to })
  }
  return silences
}

/**
 * Split the observations wherever a reported silence separates two of them.
 *
 * A silence is only trusted to cut a stretch it really leaves silent: the
 * Server bounds every one of them with the stored observations on either side,
 * so the cut is judged between the evidence the two neighbouring points really
 * hold — the silence must start no later than the newest instant the previous
 * point testifies to and end no earlier than the oldest instant the next one
 * does. Comparing the single instants the points are drawn at would miss a
 * silence whose bracketing observations share a bucket: a bucket is drawn at
 * its oldest observation while the Server bounds the silence from its newest,
 * so the silence would look as if it started inside the previous bucket's span
 * and the line would be drawn straight through it. A bucket is a drawing unit,
 * not a promise that what it holds was continuous; a raw sample is its own
 * single instant, so both ends of it are the same. Usable observations are kept
 * in order; an unusable one is left out here and dropped again by the folding
 * step.
 */
function splitAtSilences(
  samples: MetricHistorySample[],
  silences: SilenceInterval[],
): MetricHistorySample[][] {
  const runs: MetricHistorySample[][] = []
  let current: MetricHistorySample[] = []
  let previous: MetricHistorySample | undefined
  for (const sample of samples) {
    // The oldest instant this point testifies to: the first observation behind
    // it, which is the start of the stretch a bucket covers and the single
    // instant a raw sample is.
    const firstInstant = Date.parse(sample.observedAt)
    if (!Number.isFinite(firstInstant)) continue
    // The newest instant the previous point testifies to is where a silence the
    // Server reported from that point starts, so the two ends compared here are
    // the ends of the real evidence and never of the drawn marker alone.
    const previousSample = previous
    if (
      previousSample !== undefined &&
      silences.some(
        (silence) => silence.from <= newestInstant(previousSample) && silence.to >= firstInstant,
      )
    ) {
      runs.push(current)
      current = []
    }
    current.push(sample)
    previous = sample
  }
  if (current.length > 0) runs.push(current)
  return runs
}

/**
 * Place the columns on the plot. The line breaks where the data itself breaks:
 * a gap that the Server reported, or a stretch the window holds no observation
 * for, never gets a segment drawn across it.
 *
 * The Server bounds every silence it reports with the two stored observations
 * that bracket it, and those two can fall inside one bucket. The observations
 * are therefore split at the reported silences before they are folded, judged
 * between the first and last instant each point really holds rather than
 * between the instants the two points are drawn at: a bucket is a drawing unit,
 * not a claim that what it holds was continuous, and cutting first is what
 * keeps a silence whose bracketing observations share a bucket from being
 * drawn through.
 *
 * A stretch the Server reported no silence for can still hold a hole: an
 * aggregate bucket measures the widest interval between two consecutive
 * observations of its own, and once that interval reaches the gap threshold the
 * answer's cadence implies, the bucket proved that the stretch it stands for was
 * not continuous. Such a column is left out of the line it would otherwise be
 * joined into (holedPoints, metricLineRuns): the plot breaks where the evidence
 * broke, even between two points nobody reported a silence for.
 */
export function metricChartGeometry(
  samples: MetricHistorySample[],
  gaps: { from: string; to: string }[],
  from: number,
  to: number,
  fixedMax?: number,
  maxColumns: number = METRIC_HISTORY_MAX_COLUMNS,
): MetricChartGeometry {
  const silences = silenceIntervals(gaps)
  // The cadence the answer itself showed is what decides how wide an interval
  // has to be before it is a hole: neither the Server nor this plot is told an
  // Agent's sampling interval, so both measure the one the answer holds.
  const cadenceSeconds = metricMeasuredCadenceSeconds(samples)
  const runs = splitAtSilences(samples, silences)
  const bucketed = runs.map((run) =>
    bucketMetricSamples(run, from, to, maxColumns, cadenceSeconds),
  )
  const columns = bucketed.flat()
  const observed = columns.reduce((highest, column) => Math.max(highest, column.max), 0)
  const max = fixedMax && fixedMax > 0 ? fixedMax : niceMetricMax(observed)
  const gapBands = silences
    .map((silence) => {
      const left = chartX(silence.from, from, to)
      const right = chartX(silence.to, from, to)
      return { x: Math.min(left, right), width: Math.abs(right - left) }
    })
    .filter((band) => band.width > 0)
  const place = (column: MetricColumn): MetricColumnPoint => ({
    ...column,
    top: CHART_BOTTOM - (Math.max(0, Math.min(max, column.max)) / max) * CHART_SPAN,
    bottom: CHART_BOTTOM - (Math.max(0, Math.min(max, column.min)) / max) * CHART_SPAN,
    y: CHART_BOTTOM - (Math.max(0, Math.min(max, column.last)) / max) * CHART_SPAN,
  })
  // One run of observations is one drawn stretch, and nothing is ever drawn
  // between two runs: every silence the Server reported already cut the run in
  // two, and each side is drawn where its own observations really are.
  const segments = bucketed.map((run) => run.map(place)).filter((segment) => segment.length > 0)
  return {
    segments,
    gapBands,
    max,
    samples: columns.reduce((total, column) => total + column.count, 0),
    // How much evidence the plot stands for, and how much of it the Server
    // served from buckets: a stretch older than the raw window is drawn from
    // aggregates, and the panel says so instead of calling them samples.
    weight: columns.reduce((total, column) => total + column.weight, 0),
    aggregatedPoints: columns.reduce((total, column) => total + column.aggregateCount, 0),
    rawPoints: columns.reduce((total, column) => total + column.rawCount, 0),
    // Columns holding evidence that was not continuous: each is drawn as its own
    // marker and never joined by a line, so the plot shows the hole the Server
    // measured instead of drawing across it.
    holedPoints: columns.reduce((total, column) => total + (column.provenHole ? 1 : 0), 0),
    cadenceSeconds,
  }
}

export function metricLinePath(points: MetricColumnPoint[]): string {
  return points
    .map((point, index) => (index === 0 ? 'M' : 'L') + ' ' + point.x.toFixed(2) + ' ' + point.y.toFixed(2))
    .join(' ')
}

export function metricBandPath(points: MetricColumnPoint[]): string {
  if (points.length < 2) return ''
  const upper = points.map((point) => 'L ' + point.x.toFixed(2) + ' ' + point.top.toFixed(2)).join(' ')
  const lower = [...points]
    .reverse()
    .map((point) => 'L ' + point.x.toFixed(2) + ' ' + point.bottom.toFixed(2))
    .join(' ')
  return 'M ' + points[0].x.toFixed(2) + ' ' + points[0].top.toFixed(2) + ' ' + upper + ' ' + lower + ' Z'
}

/** The stretches one run of columns can honestly be drawn as: a column whose own
 * evidence proved a hole inside the stretch it stands for is left out of the
 * line entirely, because joining it to either neighbour would draw a stretch
 * nobody measured. The column still shows as its own marker, and a run of fewer
 * than two columns draws no line at all. */
export function metricLineRuns(points: MetricColumnPoint[]): MetricColumnPoint[][] {
  const runs: MetricColumnPoint[][] = []
  let current: MetricColumnPoint[] = []
  for (const point of points) {
    if (point.provenHole) {
      if (current.length > 0) runs.push(current)
      current = []
      continue
    }
    current.push(point)
  }
  if (current.length > 0) runs.push(current)
  return runs
}

/** Human window/duration, e.g. 86400 -> "24 hours", 1260 -> "21 minutes". */
export function formatHistoryDuration(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds <= 0) return '0 seconds'
  const total = Math.round(seconds)
  const days = Math.floor(total / 86400)
  // One day is named as its 24 hours: that is the raw window the Operator
  // compares coverage against, not a calendar boundary.
  if (days >= 2 && total % 86400 === 0) return days + ' days'
  const hours = Math.floor(total / 3600)
  const minutes = Math.floor((total % 3600) / 60)
  if (hours > 0) {
    const rest = minutes > 0 ? ' ' + minutes + (minutes === 1 ? ' minute' : ' minutes') : ''
    return hours + (hours === 1 ? ' hour' : ' hours') + rest
  }
  if (minutes > 0) return minutes + (minutes === 1 ? ' minute' : ' minutes')
  const remainder = total % 60
  return remainder + (remainder === 1 ? ' second' : ' seconds')
}

/**
 * The delay between a stored observation and its receipt, as the panel shows
 * it. A negative delay is not a fast delivery: it means the observation is
 * stamped after the receipt that carried it, so the magnitude is reported with
 * the direction that made it suspicious and a float that rounds below a second
 * is never printed as "0 seconds".
 */
export function formatSampleDelay(delaySeconds: number | null | undefined): string {
  if (delaySeconds == null || !Number.isFinite(delaySeconds)) return 'Unknown'
  if (delaySeconds < 0) {
    return formatHistoryDuration(Math.max(1, Math.round(-delaySeconds))) + ' ahead of receipt'
  }
  return formatHistoryDuration(delaySeconds)
}

/**
 * How the answer relates to the investigation horizon (issues #213 and #214,
 * design §11.4 and §11.6): the Server reports what it could not answer instead
 * of faking points for it. A range older than the raw window but inside the
 * horizon is still answered, by the aggregate tiers.
 */
export function metricAvailabilityNotice(availability: string | null | undefined): string | null {
  if (availability === 'partial') {
    return 'The requested range starts before the investigation horizon. Only the part inside the horizon is answered; the older stretch keeps no evidence at any grain.'
  }
  if (availability === 'unavailable') {
    return 'The requested range is entirely older than the investigation horizon. No tier keeps evidence for it; the series state below is what outlives the observations.'
  }
  return null
}

export function metricGapKindLabel(kind: string): string {
  return kind === 'protection_pause' ? 'Protection pause' : 'Collection gap'
}

/** One tier's stretch of an answer, as the Server reports it (issue #214). */
export type MetricHistorySegment = {
  from: string
  to: string
  grain: string
  source: string
  pointCount: number
  truncated: boolean
}

/** The grain a stretch or a point was served at, named for an Operator. */
export function metricGrainLabel(grain: string): string {
  if (grain === 'raw') return 'raw samples'
  if (grain === '1m') return '1 minute buckets'
  if (grain === '5m') return '5 minute buckets'
  if (grain === 'none') return 'no evidence'
  return grain
}

/** The grain in a tight table cell, where the full noun does not fit. */
export function metricGrainShortLabel(grain: string): string {
  if (grain === 'raw') return 'raw'
  if (grain === '1m') return '1 minute'
  if (grain === '5m') return '5 minutes'
  if (grain === 'none') return 'none'
  return grain
}

/** Where one point came from, as the samples table names it: a bucket is never
 * called a sample. */
export function metricPointLabel(sample: { source?: string; grain?: string }): string {
  if (!metricPointIsAggregate(sample)) return 'Stored sample'
  if (sample.grain === '1m') return '1 minute bucket'
  if (sample.grain === '5m') return '5 minute bucket'
  return 'Aggregate bucket'
}

/** The evidence a bucket stands for, or null for a stored sample that stands
 * only for itself: the table says how many observations a bucket counted and how
 * long a stretch they cover — measured between the oldest and the newest
 * observation the bucket really holds, never from the aligned start of its
 * window — so a thin bucket is visible as thin (a bucket holding one observation
 * covers no stretch at all) instead of looking like a closed stretch. A bucket
 * whose own evidence proves a hole inside that stretch says so as well: it is the
 * one thing the plot cannot draw a line through, and the panel must not call it
 * closed. */
export function metricSampleSummary(
  sample: MetricHistorySample,
  measuredCadenceSeconds = 0,
): string | null {
  if (!metricPointIsAggregate(sample)) return null
  const count = metricPointWeight(sample)
  const observations = count + (count === 1 ? ' observation' : ' observations')
  const span = Math.round((newestInstant(sample) - firstInstant(sample)) / 1000)
  if (!Number.isFinite(span) || span <= 0) return observations
  const stretch = observations + ' over ' + formatHistoryDuration(span)
  const gap = sample.maxGapSeconds
  if (
    typeof gap !== 'number' ||
    !Number.isFinite(gap) ||
    !metricPointProvesHole(sample, measuredCadenceSeconds)
  ) {
    return stretch
  }
  return (
    stretch +
    ' · the widest measured interval inside it is ' +
    formatHistoryDuration(gap) +
    ', so no line is drawn across it'
  )
}

/** Which tier answered which stretch, as the panel lists it. */
export function metricSegmentLabel(segment: MetricHistorySegment): string {
  const points = segment.pointCount + (segment.pointCount === 1 ? ' point' : ' points')
  const source =
    segment.source === 'aggregate' && segment.grain !== 'raw'
      ? metricGrainLabel(segment.grain)
      : 'stored samples'
  return source + ' · ' + points + (segment.truncated ? ' · truncated' : '')
}

/** What the tiers mean for the answer, or null while every point is a stored
 * observation (issue #214, design §11.6). */
export function metricTierNotice(segments: MetricHistorySegment[]): string | null {
  const aggregate = segments.filter((segment) => segment.source === 'aggregate')
  if (aggregate.length === 0) return null
  const grains: string[] = []
  for (const segment of aggregate) {
    const label = metricGrainLabel(segment.grain)
    if (!grains.includes(label)) grains.push(label)
  }
  return (
    'Stretches older than the retained raw window are answered by ' +
    grains.join(' and ') +
    ': each bucket keeps the count, the minimum and the maximum of the observations it summarizes, so a spike inside a bucket is still drawn, and no bucket is presented as a single stored sample.'
  )
}
