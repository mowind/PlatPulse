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

/** Byte rates as an axis label: three significant-ish digits so the label fits
 *  the fixed 56px gutter every card shares. The current value and the tooltip
 *  keep the full precision of `formatMetricRate`. */
export function formatRateAxis(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return '0 B/s'
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB']
  let scaled = value
  let unit = 0
  while (scaled >= 1024 && unit < units.length - 1) {
    scaled /= 1024
    unit += 1
  }
  const digits = scaled >= 10 || unit === 0 ? 0 : 1
  return scaled.toFixed(digits) + ' ' + units[unit] + '/s'
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
