// Node raw metric history presentation (issue #213, design §11.4).
//
// The Server answers with the stored observations, the silences between them,
// and the state of the series itself. The browser only shapes that answer: a
// 24 hour window at a 5 second cadence is a few tens of thousands of samples,
// so samples are folded into at most METRIC_HISTORY_MAX_COLUMNS columns, each
// carrying its own minimum/maximum so a spike is never averaged away. Raw
// samples stay the payload; the Server never approximates them here.

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

/** Range presets the Owner can pick. The raw window is 24 hours by default,
 * so nothing longer is offered until the aggregate tiers exist (issue #214). */
export const METRIC_HISTORY_PRESETS = [
  { label: '1 hour', hours: 1 },
  { label: '6 hours', hours: 6 },
  { label: '24 hours', hours: 24 },
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

export type MetricHistorySample = { observedAt: string; value: number }

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
  /** Oldest stored instant folded into the bucket, epoch milliseconds. */
  firstInstant: number
  /** Newest stored instant folded into the bucket, epoch milliseconds: the
   * connecting line ends here, and a silence the Server reported from this
   * observation is the one this column breaks on. */
  lastInstant: number
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
  samples: number
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
 */
export function bucketMetricSamples(
  samples: MetricHistorySample[],
  from: number,
  to: number,
  maxColumns: number = METRIC_HISTORY_MAX_COLUMNS,
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
    }
  >()
  for (const sample of samples) {
    const instant = Date.parse(sample.observedAt)
    if (!Number.isFinite(instant) || !Number.isFinite(sample.value)) continue
    const clamped = Math.max(from, Math.min(to, instant))
    const index = Math.min(columns - 1, Math.floor(((clamped - from) / width) * columns))
    const bucket = buckets.get(index)
    if (!bucket) {
      buckets.set(index, {
        index,
        min: sample.value,
        max: sample.value,
        last: sample.value,
        firstInstant: instant,
        lastInstant: instant,
        count: 1,
      })
      continue
    }
    bucket.min = Math.min(bucket.min, sample.value)
    bucket.max = Math.max(bucket.max, sample.value)
    bucket.firstInstant = Math.min(bucket.firstInstant, instant)
    bucket.count += 1
    if (instant >= bucket.lastInstant) {
      bucket.lastInstant = instant
      bucket.last = sample.value
    }
  }
  return [...buckets.values()]
    .sort((left, right) => left.index - right.index)
    .map((bucket) => ({
      // Drawn where this column's own observations are, not at the centre of
      // the bucket it fell in: the bucket centre is a position no observation
      // in this column has to support, and a reported silence between two of
      // them can cover it.
      x: chartX((bucket.firstInstant + bucket.lastInstant) / 2, from, to),
      min: bucket.min,
      max: bucket.max,
      last: bucket.last,
      count: bucket.count,
      firstInstant: bucket.firstInstant,
      lastInstant: bucket.lastInstant,
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
 * A silence is only trusted to cut a stretch it really covers: the Server
 * bounds every one of them with the stored observations on either side, and a
 * silence that starts at one observation and ends at the next is exactly the
 * stretch that must not be joined. Usable observations are kept in order; an
 * unusable one is left out here and dropped again by the folding step.
 */
function splitAtSilences(
  samples: MetricHistorySample[],
  silences: SilenceInterval[],
): MetricHistorySample[][] {
  const runs: MetricHistorySample[][] = []
  let current: MetricHistorySample[] = []
  let previous: number | undefined
  for (const sample of samples) {
    const instant = Date.parse(sample.observedAt)
    if (!Number.isFinite(instant)) continue
    const previousInstant = previous
    if (
      previousInstant !== undefined &&
      silences.some((silence) => silence.from <= previousInstant && silence.to >= instant)
    ) {
      runs.push(current)
      current = []
    }
    current.push(sample)
    previous = instant
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
 * are therefore split at the reported silences before they are folded: a
 * bucket is a drawing unit, not a claim that what it holds was continuous, and
 * cutting first is what keeps a same-bucket silence from being drawn through.
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
  const runs = splitAtSilences(samples, silences)
  const bucketed = runs.map((run) => bucketMetricSamples(run, from, to, maxColumns))
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
  return { segments, gapBands, max, samples: columns.reduce((total, column) => total + column.count, 0) }
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
 * How the answer relates to the retained raw window (design §11.4): the
 * Server reports what it could not answer instead of faking samples for it.
 */
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

export function metricAvailabilityNotice(availability: string | null | undefined): string | null {
  if (availability === 'partial') {
    return 'The requested range starts before the retained raw window. Only the retained part is answered, and the older stretch is not drawn.'
  }
  if (availability === 'unavailable') {
    return 'The requested range is entirely older than the retained raw window. No raw sample survives for it; the series state below is what outlives the samples.'
  }
  return null
}

export function metricGapKindLabel(kind: string): string {
  return kind === 'protection_pause' ? 'Protection pause' : 'Collection gap'
}
