/**
 * Axis scales and label formats shared by the Node Detail six-chart deck.
 *
 * A ceiling is a display range, never a rescaling of the samples: the Server's
 * observations are drawn as they were reported, and the axis only decides how
 * much room they get. Kept out of the page so the pure scale rules can be
 * unit-tested without a browser.
 */
import { METRIC_CHART_BOTTOM_MARGIN, METRIC_CHART_BOTTOM_MARGIN_WITH_LEGEND, METRIC_CHART_TOP_MARGIN } from './nodeMetricChartOption'

/**
 * CPU and memory curves are read against the ceiling the window actually
 * reached; a fixed 0–100% axis flattens a low-utilisation Node onto the
 * baseline. The ladder keeps climbing past 100% because a Process CPU sample is
 * a process share and is not capped at one core: a 130% sample belongs on a
 * 150% axis, not truncated onto a 100% one.
 */
export function percentChartMax(peak: number): number {
  const max = Number.isFinite(peak) ? peak : 0
  for (const bucket of [5, 10, 25, 50, 100, 150, 200, 250, 400]) {
    if (max <= bucket) return bucket
  }
  return Math.ceil(max / 100) * 100
}

/**
 * One chart's value axis, resolved for the room it has: the ceiling, the tick
 * step, the axis title and the bare tick label. The title carries the unit, so a
 * tick is only a number — "Speed (MiB/s)" over "0 · 30 · 60", never a column of
 * "28.6 MiB/s" labels wider than the gutter Emerald gives the axis. One axis
 * never mixes units: the rule picks the unit once from the window's peak.
 *
 * The ceiling is always a whole number of steps, so the renderer never has to
 * add the ceiling as an extra off-rhythm label crowding the last tick.
 */
export type MetricAxis = {
  max: number
  interval: number
  name: string
  format: (value: number) => string
}

/**
 * Resolves a {@link MetricAxis} from the highest observation the window actually
 * plots and the most tick bands the plot height can space apart. Rules are built
 * once at module level, so their identity survives a render.
 */
export type MetricAxisRule = (peak: number, maxBands: number) => MetricAxis

/** Tick steps a reader counts by, per decade. 1.5, 6 and 8 keep the worst
 *  ceiling within about 31% of the target even with only five bands. */
const NICE_STEP_MULTIPLES = [1, 1.5, 2, 2.5, 3, 4, 5, 6, 8]

function niceSteps(minimum: number, integer: boolean): number[] {
  const exponent = Math.floor(Math.log10(minimum))
  const steps: number[] = []
  for (let power = exponent - 1; power <= exponent + 2; power += 1) {
    for (const multiple of NICE_STEP_MULTIPLES) {
      const step = Number((multiple * 10 ** power).toPrecision(12))
      if (step >= minimum * (1 - 1e-9) && (!integer || Number.isInteger(step))) steps.push(step)
    }
  }
  return steps
}

function clampBands(maxBands: number): number {
  return Number.isFinite(maxBands) ? Math.min(5, Math.max(1, Math.floor(maxBands))) : 5
}

/**
 * The lowest ceiling at or above `target` that a nice step divides into at most
 * `maxBands` bands. An integer scale only uses whole steps, so a two-connection
 * window can never print "0, 1, 1, 2".
 */
export function niceAxisScale(target: number, maxBands: number, integer = false): { max: number; interval: number } {
  const bands = clampBands(maxBands)
  const value = Number.isFinite(target) && target > 0 ? target : 1
  let best: { max: number; interval: number } | undefined
  for (const step of niceSteps(Math.max(value / bands, integer ? 1 : 0), integer)) {
    const count = Math.max(1, Math.ceil(value / step - 1e-9))
    if (count > bands) continue
    const max = Number((count * step).toPrecision(12))
    if (!best || max < best.max) best = { max, interval: step }
  }
  return best ?? { max: value, interval: value }
}

/** A tick label without a unit: the axis title states it. Float noise from the
 *  step arithmetic is dropped, never a real digit. */
function bareNumber(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return '0'
  return String(Number(value.toPrecision(6)))
}

/** Vertical room one value-axis label needs: an 11px label is 12px tall, and two
 *  more pixels keep neighbouring ticks apart. */
const MIN_TICK_SPACING = 14

/** How many tick bands a plot of this height can label without overlap; up to
 *  Emerald's five. Before the first measurement the desktop answer is used. */
export function metricChartMaxBands(height: number, showLegend: boolean): number {
  if (!(height > 0)) return 5
  const plot = height - METRIC_CHART_TOP_MARGIN - (showLegend ? METRIC_CHART_BOTTOM_MARGIN_WITH_LEGEND : METRIC_CHART_BOTTOM_MARGIN)
  return Math.max(1, Math.min(5, Math.floor(plot / MIN_TICK_SPACING)))
}

/**
 * CPU and memory: the percent ladder above, stepped evenly.
 *
 * The ladder rung is a target for the same nice-step search the other axes use,
 * never the ceiling itself: a ladder rung is not always divisible into a
 * countable number of bands (700% divides by no step a reader counts in), and an
 * axis whose ceiling cannot be stepped prints its floor and its ceiling and
 * nothing between them.
 */
export function percentAxis(name: string): MetricAxisRule {
  const format = bareNumber
  return (peak, maxBands) => ({
    ...niceAxisScale(percentChartMax(peak), maxBands),
    name: name + ' (%)',
    format,
  })
}

/**
 * Counts are whole numbers on whole steps. A window that never reached four is
 * still drawn against four, so a steady two-connection line is not pinned to the
 * top of its box.
 */
export function countAxis(name: string): MetricAxisRule {
  const format = (value: number) => Math.round(value).toLocaleString()
  return (peak, maxBands) => ({ ...niceAxisScale(Math.max(peak, 4), maxBands, true), name, format })
}

/**
 * Block intervals are measured in milliseconds; an axis whose ceiling reaches
 * five seconds is read in seconds instead. The whole axis then uses that one
 * unit — never "0ms" under "5s" — and the tooltip keeps the exact millisecond
 * sample.
 */
export function durationAxis(name: string): MetricAxisRule {
  const milliseconds = (value: number) => bareNumber(value)
  const seconds = (value: number) => bareNumber(value / 1000)
  return (peak, maxBands) => {
    const value = Number.isFinite(peak) && peak > 0 ? peak : 1
    if (value < 5000) return { ...niceAxisScale(value, maxBands, true), name: name + ' (ms)', format: milliseconds }
    const scale = niceAxisScale(value / 1000, maxBands)
    return { max: scale.max * 1000, interval: scale.interval * 1000, name: name + ' (s)', format: seconds }
  }
}

const RATE_UNITS = ['B/s', 'KiB/s', 'MiB/s', 'GiB/s', 'TiB/s']

/** The ceiling clears the window's peak by at least this much, so the measured
 *  curve never runs into the top of its box. */
const RATE_CHART_MIN_HEADROOM = 1.05

/**
 * Byte rates are stepped in the binary unit they are read in: the unit is picked
 * from the window's peak, and the steps are nice numbers *of that unit* (0, 30,
 * 60, 90, 120 MiB/s) rather than decimal-byte steps printed as 28.6 MiB/s. The
 * ceiling clears the peak by at least 5%.
 */
export function rateAxis(name: string): MetricAxisRule {
  const formats = RATE_UNITS.map((_, unit) => (value: number) => bareNumber(value / 1024 ** unit))
  return (peak, maxBands) => {
    const target = (Number.isFinite(peak) && peak > 0 ? peak : 1) * RATE_CHART_MIN_HEADROOM
    let unit = 0
    while (target >= 1024 ** (unit + 1) && unit < RATE_UNITS.length - 1) unit += 1
    const scale = 1024 ** unit
    const { max, interval } = niceAxisScale(target / scale, maxBands)
    return { max: max * scale, interval: interval * scale, name: name + ' (' + RATE_UNITS[unit] + ')', format: formats[unit] as (value: number) => string }
  }
}


/**
 * A percentage, count or duration tooltip states the sample itself, not a
 * reading of it: percentages are not pre-rounded (12.345% must not print as
 * "12.3%"). Trimming to three decimals is a display cap that drops float noise,
 * not a claim of unbounded precision; the card's own current value keeps the
 * page's display formatting. Byte rates are the exception (issue #249): see
 * `formatRateSample`.
 */
function trimSample(value: number, digits: number): string {
  if (!Number.isFinite(value)) return 'Unknown'
  return String(Number(value.toFixed(digits)))
}

export function formatPercentSample(value: number): string {
  return trimSample(value, 3) + '%'
}

/**
 * A byte-rate tooltip sample is read exactly as the card header reads the
 * current rate — the same binary ladder and decimals as `formatMetricRate`
 * (82366 B/s → "80.4 KiB/s", 800 B/s stays "800 B/s") — so the tooltip, the
 * header and the `Speed (MiB/s)` axis title never state one card in two units.
 * Two samples inside one rounding step (1024 and 1025 B/s) therefore share a
 * reading, which the header has always accepted (issue #249, option A).
 */
export function formatRateSample(value: number): string {
  return formatMetricRate(value)
}

export function formatCountSample(value: number): string {
  return Number.isFinite(value) ? Math.round(value).toLocaleString() : 'Unknown'
}

export function formatMillisecondsSample(value: number): string {
  return trimSample(value, 3) + ' ms'
}

/**
 * A byte rate read in the binary unit that keeps its printed number below 1024,
 * on the same `RATE_UNITS` ladder the rate axis titles use: two decimals under
 * 10, one under 100, none above (`5.00 B/s`, `80.4 KiB/s`, `148 KiB/s`,
 * `1023 B/s`). A value that only rounds up to 1024 moves to the next unit
 * (1023.7 B/s → `1.00 KiB/s`). The card header's current value and the hover
 * tooltip both print it.
 */
export function formatMetricRate(value: number | null | undefined): string {
  if (value == null || !Number.isFinite(value) || value < 0) return 'Unknown'
  const fixed = (scaled: number) => scaled.toFixed(scaled >= 100 ? 0 : scaled >= 10 ? 1 : 2)
  let scaled = value
  let unit = 0
  while (scaled >= 1024 && unit < RATE_UNITS.length - 1) {
    scaled /= 1024
    unit += 1
  }
  if (Number(fixed(scaled)) >= 1024 && unit < RATE_UNITS.length - 1) {
    scaled /= 1024
    unit += 1
  }
  return fixed(scaled) + ' ' + RATE_UNITS[unit]
}
