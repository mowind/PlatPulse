import type { PublicMetricPoint } from '../api/generated'

/**
 * Axis scales and label formats shared by the Node Detail six-chart deck.
 *
 * A ceiling is a display range, never a rescaling of the samples: the Server's
 * observations are drawn as they were reported, and the axis only decides how
 * much room they get. Kept out of the page so the pure scale rules can be
 * unit-tested without a browser.
 */

/**
 * CPU and memory curves are read against the ceiling the window actually
 * reached; a fixed 0–100% axis flattens a low-utilisation Node onto the
 * baseline. The ladder keeps climbing past 100% because a Process CPU sample is
 * a process share and is not capped at one core: a 130% sample belongs on a
 * 150% axis, not truncated onto a 100% one.
 */
export function percentChartMax(points: PublicMetricPoint[]): number {
  const values = points.map((point) => point.value).filter(Number.isFinite)
  const max = values.length > 0 ? Math.max(...values) : 0
  for (const bucket of [5, 10, 25, 50, 100, 150, 200, 250, 400]) {
    if (max <= bucket) return bucket
  }
  return Math.ceil(max / 100) * 100
}

export function niceChartMax(value: number): number {
  if (!Number.isFinite(value) || value <= 0) return 1
  const magnitude = 10 ** Math.floor(Math.log10(value))
  const normalized = value / magnitude
  const step = normalized <= 1 ? 1 : normalized <= 2 ? 2 : normalized <= 5 ? 5 : 10
  return step * magnitude
}

/**
 * Counts are integers, so the axis is asked for whole ticks and given a
 * five-step ceiling: at most five whole numbers fit under it, which is what
 * stops "0, 1, 1, 2, 2" from appearing under a two-connection window without
 * inflating the reported values.
 */
export function integerChartMax(value: number): number {
  if (!Number.isFinite(value) || value <= 0) return 4
  return value <= 4 ? 4 : niceChartMax(value)
}

/** Steps a count axis is willing to print, in the order a reader would choose
 *  them: whole numbers first, then the steps one counts by. */
const INTEGER_TICK_INTERVALS = [1, 2, 5, 10, 20, 25, 50, 100, 200, 250, 500, 1000]

/**
 * The tick step a whole-number ceiling has to be given explicitly. Without one
 * the renderer divides whatever ceiling it was handed into five and rounds each
 * label, so a four-connection window can print "0, 1, 1, 2" — two labels for one
 * observation count. The first readable step that divides the ceiling into two to
 * five bands wins (20 counts are read in fives, not in fours), and a ceiling no
 * step divides falls back to the smallest whole step that still fits five bands.
 */
export function integerChartTicks(max: number): { splitNumber: number; interval: number } {
  if (!Number.isFinite(max) || max <= 0) return { splitNumber: 5, interval: 1 }
  for (const interval of INTEGER_TICK_INTERVALS) {
    if (interval > max) break
    const splits = max / interval
    if (Number.isInteger(splits) && splits >= 2 && splits <= 5) return { splitNumber: splits, interval }
  }
  const interval = Math.max(1, Math.ceil(max / 5))
  return { splitNumber: Math.max(1, Math.floor(max / interval)), interval }
}

/**
 * Byte rates get their own ladder. `niceChartMax` normalises in decimal steps —
 * a 5 MiB/s window peak (5 242 880 B/s) sits between 5 and 10 × 10^6 — so the
 * network axis climbed to 9.5 MiB/s and the curve lived in the lower half of an
 * empty box. This ladder steps in the units a rate is read in (1, 1.125, 1.25, 1.5,
 * 1.75, 2, 2.25, 2.5, 3, 3.5, 4, 4.5, 5, 6, 7, 8, 9, 10, 12, 14, 16, 18 × 2^n ×
 * 128 B), and the ceiling is the first rung clearing the peak by at least 5%: the
 * plot keeps a sliver of headroom and never spends a quarter of its height on
 * empty axis. The half-step rungs (1.125, 2.25, 4.5, 9, 18) cap the ladder's
 * widest gap at 1.2x, which is what holds that promise: without them the gap
 * between rungs is 1.25x, so a peak landing just above a rung — a real 7.7 MiB/s
 * window, between the 8 and 10 MiB/s rungs — was handed a 30% ceiling either way.
 */
const RATE_CHART_MULTIPLES = [
  1, 1.125, 1.25, 1.5, 1.75, 2, 2.25, 2.5, 3, 3.5, 4, 4.5, 5, 6, 7, 8, 9, 10, 12, 14, 16, 18,
]

/** The ceiling clears the window's peak by at least this much, so the measured
 *  curve never runs into the top of its box. */
const RATE_CHART_MIN_HEADROOM = 1.05

/** The smallest step on the rate ladder at or above the given value. The scale
 *  keeps doubling instead of stopping at a fixed bound: a peak beyond the
 *  ladder's usual range (a nonsense rate, but a real number) still gets a
 *  ceiling above it rather than a ceiling it has already passed. */
function rateChartStep(value: number): number {
  if (!(value > 0)) return 128
  for (let scale = 128; ; scale *= 2) {
    for (const multiple of RATE_CHART_MULTIPLES) {
      const step = multiple * scale
      if (step >= value) return step
    }
  }
}

export function rateChartMax(points: ReadonlyArray<{ value: number }>): number {
  const values = points.map((point) => point.value).filter(Number.isFinite)
  const peak = values.length > 0 ? Math.max(...values) : 0
  if (!(peak > 0)) return 128
  // The first rung that clears the peak by the minimum headroom. The widest rung
  // gap is 1.2x, so the ceiling lands between 5% and 26% above the real peak.
  return rateChartStep(peak * RATE_CHART_MIN_HEADROOM)
}

/** Emerald's axis label size and rhythm: whole percents stay whole, and a
 *  narrow range keeps one decimal so 2.5% is not rounded onto 2%. */
export function formatPercentAxis(value: number): string {
  if (value <= 0) return '0%'
  return value.toFixed(value < 10 ? 1 : 0) + '%'
}

export function formatCountAxis(value: number): string {
  return Math.round(value).toLocaleString()
}

export function formatMillisecondsAxis(value: number): string {
  if (value >= 1000) return Number((value / 1000).toFixed(value >= 10_000 ? 0 : 1)) + 's'
  return Math.round(value) + 'ms'
}

/**
 * A byte-rate tick, stated in the unit the ceiling chose: one network axis never
 * mixes "1.2 MiB/s" with "600 KiB/s", because a mixed axis cannot be compared
 * down a column. The decimals are capped where they would push the label out of
 * the shared gutter the six cards all reserve — whole numbers stay whole, and
 * only a value below ten keeps the two decimals it needs to stay distinct. The
 * card's own current value and the tooltip keep the full precision of
 * `formatMetricRate`.
 */
export function formatRateAxisAt(value: number, axisMax: number): string {
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB']
  let unit = 0
  let scale = 1
  const ceiling = Number.isFinite(axisMax) && axisMax > 0 ? axisMax : 0
  while (ceiling >= scale * 1024 && unit < units.length - 1) {
    scale *= 1024
    unit += 1
  }
  if (!Number.isFinite(value) || value <= 0) return '0 ' + units[unit] + '/s'
  const scaled = value / scale
  const digits = scaled >= 100 ? 0 : scaled >= 10 ? (Number.isInteger(scaled) ? 0 : 1) : 2
  return Number(scaled.toFixed(digits)) + ' ' + units[unit] + '/s'
}

/**
 * The tooltip states the sample itself, not a reading of it: byte rates keep
 * their reported unit (1025 B/s must not become an indistinguishable
 * "1.00 KiB/s") and percentages are not pre-rounded (12.345% must not print as
 * "12.3%"). Trimming to three decimals is a display cap that drops float noise,
 * not a claim of unbounded precision; the card's own current value keeps the
 * page's display formatting.
 */
function trimSample(value: number, digits: number): string {
  if (!Number.isFinite(value)) return 'Unknown'
  return String(Number(value.toFixed(digits)))
}

export function formatPercentSample(value: number): string {
  return trimSample(value, 3) + '%'
}

export function formatRateSample(value: number): string {
  return trimSample(value, 3) + ' B/s'
}

export function formatCountSample(value: number): string {
  return Number.isFinite(value) ? Math.round(value).toLocaleString() : 'Unknown'
}

export function formatMillisecondsSample(value: number): string {
  return trimSample(value, 3) + ' ms'
}

/** The exact retained byte rate, as a current value and in the tooltip. */
export function formatMetricRate(value: number | null | undefined): string {
  if (value == null || !Number.isFinite(value) || value < 0) return 'Unknown'
  const units = ['B/s', 'KiB/s', 'MiB/s', 'GiB/s']
  let scaled = value
  let unit = 0
  while (scaled >= 1024 && unit < units.length - 1) {
    scaled /= 1024
    unit += 1
  }
  return scaled.toFixed(scaled >= 100 ? 0 : scaled >= 10 ? 1 : 2) + ' ' + units[unit]
}
