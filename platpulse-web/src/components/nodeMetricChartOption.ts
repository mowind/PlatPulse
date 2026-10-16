import type { EChartsOption } from 'echarts'
import type { MetricObservation } from '../metricCoverage'
import { integerChartTicks } from './metricChartAxis'

/**
 * The Node Detail six-chart option, ported from komari-theme-emerald's
 * `src/components/LoadChart.vue` onto PlatPulse's own data contract.
 *
 * Pure data, like `mapChartOption.ts`: the module owns literal colours keyed by
 * a `dark` boolean rather than reading CSS variables, because ECharts paints
 * outside the cascade and the light/dark answer has to be an input. No DOM, no
 * `window`, no timers — the option is unit-testable and server-renderable.
 *
 * Two deliberate departures from upstream, both recorded in the visual
 * migration register (docs/visual-migration/emerald/README.md):
 *   1. A missing observation is never drawn as zero. Emerald plots
 *      `r.ram ?? 0`, which turns a collection silence into a drop to the
 *      baseline; PlatPulse breaks the curve instead.
 *   2. Every chart is a real time axis with real timestamps, not a category axis
 *      over preformatted strings, so a tooltip can state the sample's exact
 *      instant and value — and so the axis can name its own window edges on the
 *      clock instead of waiting for a DOM caption underneath.
 */

/** Emerald's series palette (LoadChart.vue:47-59). CPU and memory are both the
 *  theme's coral, exactly as upstream paints its CPU and RAM curves; PlatPulse
 *  adopts coral as a data-series colour, so status red stays `--destructive` and
 *  an error is never mistaken for a curve. */
export type MetricSeriesKey =
  | 'cpu'
  | 'memory'
  | 'blockInterval'
  | 'transactions'
  | 'download'
  | 'upload'
  | 'inbound'
  | 'outbound'

export const METRIC_SERIES_COLORS: Record<MetricSeriesKey, string> = {
  cpu: '#FF6B6B',
  memory: '#FF6B6B',
  blockInterval: '#FFB347',
  transactions: '#A78BFA',
  download: '#60A5FA',
  upload: '#A78BFA',
  inbound: '#60A5FA',
  outbound: '#4ECDC4',
}

export type MetricChartTheme = {
  text: string
  textSecondary: string
  axisLine: string
  splitLine: string
  markerBorder: string
  tooltipBackground: string
  tooltipShadow: string
  crosshair: string
  crosshairLine: string
}

/** Emerald's chart theme (LoadChart.vue:62-71): light and dark differ only in
 *  the neutral ink, never in the series colours. */
export const METRIC_CHART_THEME: { light: MetricChartTheme; dark: MetricChartTheme } = {
  light: {
    text: 'rgba(0, 0, 0, 0.85)',
    textSecondary: 'rgba(0, 0, 0, 0.55)',
    axisLine: 'rgba(0, 0, 0, 0.1)',
    splitLine: 'rgba(0, 0, 0, 0.06)',
    markerBorder: '#ffffff',
    tooltipBackground: 'rgba(255, 255, 255, 0.8)',
    tooltipShadow: 'rgba(0, 0, 0, 0.06)',
    crosshair: 'rgba(0, 0, 0, 0.1)',
    crosshairLine: 'rgba(0, 0, 0, 0.1)',
  },
  dark: {
    text: 'rgba(255, 255, 255, 0.85)',
    textSecondary: 'rgba(255, 255, 255, 0.55)',
    axisLine: 'rgba(255, 255, 255, 0.1)',
    splitLine: 'rgba(255, 255, 255, 0.06)',
    markerBorder: '#0a0a0a',
    tooltipBackground: 'rgba(40, 40, 40, 0.95)',
    tooltipShadow: 'rgba(0, 0, 0, 0.4)',
    crosshair: 'rgba(255, 255, 255, 0.15)',
    crosshairLine: 'rgba(255, 255, 255, 0.15)',
  },
}

export type MetricChartSeriesInput = {
  /** Direction name, printed in the tooltip and in the DOM legend. */
  label: string
  color: string
  /** One array per uninterrupted stretch of the window (see coverageSegments). */
  segments: MetricObservation[][]
  /** Emerald's vertical transparent gradient, for the single-direction CPU and
   *  memory curves only. */
  area?: boolean
  /** The gradient's two stops. Emerald's own recipe (0.25 → 0.02) is the default
   *  and stays on the CPU curve; memory, which can hold one value across the whole
   *  window, is given a fainter pair so a steady reading is not painted as a slab. */
  areaOpacity?: readonly [number, number]
}

/** The composed option's series array. A chart mixes one entry per drawn run,
 *  so the list has to satisfy ECharts' whole series union rather than one chart
 *  type; naming the array type here keeps the option literal free of casts. */
type MetricChartSeries = Extract<NonNullable<EChartsOption['series']>, unknown[]>

export type MetricChartOptionInput = {
  kind: 'line' | 'bar'
  from: number
  to: number
  /** Display ceiling; the samples are drawn unscaled against it. */
  max: number
  series: MetricChartSeriesInput[]
  dark: boolean
  /** Whole-number axis: no tick may repeat a rounded label. */
  integer?: boolean
  axisFormat: (value: number) => string
  /** Exact value, as the tooltip states it. */
  valueFormat: (value: number) => string
  /** Bar width in pixels, measured from the container the chart was given. */
  barWidth?: number
}

/** The gutter every card shares, so all six plots start on the same column. It is
 *  wide enough for the longest label the deck prints: a byte rate stated in the
 *  unit its own ceiling chose, up to "1.25 MiB/s". */
export const METRIC_CHART_GUTTER = 60

/** The room the axis labels need on the right of the plot box, so the label
 *  centred on the window's last instant is painted whole. */
export const METRIC_CHART_RIGHT_MARGIN = 24

function withAlpha(color: string, alpha: number): string {
  const hex = color.replace('#', '')
  const full = hex.length === 3 ? hex.split('').map((part) => part + part).join('') : hex
  const red = Number.parseInt(full.slice(0, 2), 16)
  const green = Number.parseInt(full.slice(2, 4), 16)
  const blue = Number.parseInt(full.slice(4, 6), 16)
  return 'rgba(' + red + ', ' + green + ', ' + blue + ', ' + alpha + ')'
}

/** Emerald's area recipe, verbatim: 25% at the curve down to 2% at the axis. */
const EMERALD_AREA_STOPS = [0.25, 0.02] as const

function areaGradient(color: string, stops: readonly [number, number] = EMERALD_AREA_STOPS) {
  return {
    type: 'linear' as const,
    x: 0,
    y: 0,
    x2: 0,
    y2: 1,
    colorStops: [
      { offset: 0, color: withAlpha(color, stops[0]) },
      { offset: 1, color: withAlpha(color, stops[1]) },
    ],
  }
}

function clockTime(value: number): string {
  const date = new Date(value)
  const pad = (part: number) => String(part).padStart(2, '0')
  return pad(date.getHours()) + ':' + pad(date.getMinutes()) + ':' + pad(date.getSeconds())
}

function record(value: unknown): Record<string, unknown> | undefined {
  return typeof value === 'object' && value !== null ? (value as Record<string, unknown>) : undefined
}

function escapeText(value: string): string {
  return value.replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;')
}

/**
 * Emerald's hand-built tooltip body: the sample's absolute instant, then one row
 * per direction with the value as retained rather than as the page displays it —
 * byte rates keep their reported unit and percentages are not pre-rounded. The
 * row goes through `formatXSample`, whose three-decimal cap drops float noise
 * without becoming the rounded reading the card's own value prints; the instant
 * is the observation's own timestamp.
 */
function tooltipFormatter(input: MetricChartOptionInput) {
  const theme = input.dark ? METRIC_CHART_THEME.dark : METRIC_CHART_THEME.light
  return (raw: unknown): string => {
    const entries = (Array.isArray(raw) ? raw : [raw]).map(record).filter((entry) => entry !== undefined)
    const first = entries[0]
    const at = typeof first?.axisValue === 'number' ? first.axisValue : undefined
    const header = '<div style="font-weight:600;margin-bottom:6px;color:' + theme.textSecondary + '">' + (at === undefined ? '' : clockTime(at)) + '</div>'
    const rows = entries.map((entry) => {
      const seriesName = typeof entry.seriesName === 'string' ? entry.seriesName : ''
      const data = Array.isArray(entry.data) ? entry.data : []
      const value = typeof data[1] === 'number' ? data[1] : Number.NaN
      const color = typeof entry.color === 'string' ? entry.color : 'currentColor'
      if (!Number.isFinite(value)) return ''
      return '<div style="display:flex;align-items:center;gap:6px;line-height:20px">'
        + '<span style="width:8px;height:8px;border-radius:9999px;flex:none;background:' + color + '"></span>'
        + '<span style="flex:1 1 auto;white-space:nowrap">' + escapeText(seriesName) + '</span>'
        + '<span style="margin-left:16px;font-weight:600;font-variant-numeric:tabular-nums">' + escapeText(input.valueFormat(value)) + '</span>'
        + '</div>'
    })
    return header + rows.join('')
  }
}

export function metricChartOption(input: MetricChartOptionInput): EChartsOption {
  const theme = input.dark ? METRIC_CHART_THEME.dark : METRIC_CHART_THEME.light
  const series: MetricChartSeries = input.series.flatMap((entry, direction): MetricChartSeries => {
    if (input.kind === 'bar') {
      // Bars are discrete observations, not a curve: each in-window sample is
      // one bar, and there is no stretch between them to bridge.
      return [{
        type: 'bar' as const,
        name: entry.label,
        data: entry.segments.flat().map((point) => [point.sampledAt, point.value]),
        barMaxWidth: 12,
        ...(input.barWidth !== undefined ? { barWidth: input.barWidth } : {}),
        // A bar is one observation and its height is the only thing worth
        // reading: at full saturation a row of bars reads as a filled area rather
        // than as discrete samples. Emerald has no bar chart to copy, so the
        // softening is ours — the palette colour stays literal, only its weight
        // drops, and the rounded cap keeps a tall bar from ending in a hard edge.
        itemStyle: { color: entry.color, borderRadius: [3, 3, 0, 0], opacity: 0.75 },
        emphasis: { focus: 'none' as const },
        animation: false,
      }]
    }
    return entry.segments
      .filter((segment) => segment.length > 0)
      .map((segment) => ({
        type: 'line' as const,
        name: entry.label,
        data: segment.map((point) => [point.sampledAt, point.value]),
        // An isolated observation is still evidence: it is drawn as its own
        // marker instead of disappearing as a zero-length polyline.
        showSymbol: segment.length === 1,
        symbol: 'circle',
        // r4 for the card's first direction, r3.5 for the others, so two
        // directions whose only observation is the same instant stay readable
        // instead of one marker covering the other.
        symbolSize: direction === 0 ? 8 : 7,
        itemStyle: { color: entry.color, borderColor: theme.markerBorder, borderWidth: segment.length === 1 ? 1.5 : 0 },
        lineStyle: { width: 1.5, color: entry.color, cap: 'round' as const },
        // A break is a break: the segment before a silence and the segment after
        // it are separate series, and nothing is interpolated between them.
        connectNulls: false,
        smooth: false,
        z: 2,
        emphasis: { focus: 'none' as const },
        ...(entry.area === true && segment.length > 1
          ? { areaStyle: { color: areaGradient(entry.color, entry.areaOpacity) } }
          : {}),
        animation: false,
      }))
  })

  // A whole-number axis is given its own step: without one the renderer divides
  // whatever ceiling it was handed into five and rounds each label, so a
  // four-connection window can print "0, 1, 1, 2" — two labels for one count.
  const ticks = input.integer === true ? integerChartTicks(input.max) : undefined

  return {
    animation: false,
    // Emerald's own margin recipe, kept for its reason: the axis labels hang
    // outside the plot box, so the 24px on the right reserve room for the label
    // centred on the window's last instant instead of letting ECharts shrink the
    // grid by whatever that label happens to measure.
    grid: { top: 12, right: METRIC_CHART_RIGHT_MARGIN, bottom: 26, left: METRIC_CHART_GUTTER, containLabel: false },
    // The window's own clock labels the axis, the way Emerald prints its sampled
    // times, and the axis states the Server's window edges because min/max are
    // that window: two readable instants plus whatever fits between them.
    xAxis: {
      type: 'time',
      min: input.from,
      max: input.to,
      // ECharts 6 registers a bar-series 'containShape' handler that expands the
      // scale by half a bar width (chart/helper/axisSnippets.js, "Simply expand
      // Scale extent by half bandWidth"), which would pad the two bar charts and
      // let them disagree with the four line charts over the same 60 seconds.
      // The window is the Server's, so the expansion is switched off; a bar
      // centred exactly on an edge is then clipped to the plot box, which is the
      // honest reading of an observation that sits on the window's boundary.
      ...(input.kind === 'bar' ? { containShape: false } : {}),
      axisLabel: {
        show: true,
        fontSize: 11,
        color: theme.textSecondary,
        margin: 8,
        hideOverlap: true,
        formatter: (value: number) => clockTime(value),
      },
      axisTick: { show: false },
      axisLine: { show: true, lineStyle: { color: theme.axisLine, width: 1 } },
      axisPointer: { label: { show: false } },
    },
    yAxis: {
      type: 'value',
      min: 0,
      max: input.max,
      splitNumber: ticks ? ticks.splitNumber : 5,
      ...(ticks ? { interval: ticks.interval } : {}),
      axisLabel: { fontSize: 11, color: theme.textSecondary, margin: 6, formatter: input.axisFormat },
      axisLine: { show: false },
      axisTick: { show: false },
      splitLine: { lineStyle: { color: theme.splitLine, type: 'dashed' as const } },
      // The crosshair's second label is noise: Emerald suppresses it the same way.
      axisPointer: { label: { show: false }, lineStyle: { opacity: 0 } },
    },
    tooltip: {
      trigger: 'axis',
      // Confined, unlike Emerald's `confine: false`: an unconfined panel is 162px
      // wide, so on a 360px phone it lands at a negative x and scrolls the whole
      // page sideways (the suite's no-horizontal-overflow acceptance catches it).
      // Confined, the same panel stays inside the plot box it describes.
      confine: true,
      className: 'node-metric-chart-tooltip',
      backgroundColor: theme.tooltipBackground,
      borderColor: 'transparent',
      borderWidth: 0,
      borderRadius: 6,
      padding: [6, 10, 6, 10],
      textStyle: { color: theme.text, fontSize: 12, lineHeight: 20 },
      extraCssText: 'z-index: 9; border-radius: 6px; box-shadow: 0 0 0 1px ' + theme.tooltipShadow + ', 0 0 16px ' + theme.tooltipShadow + ';',
      axisPointer: {
        type: 'cross',
        crossStyle: { color: theme.crosshairLine },
        lineStyle: { color: theme.crosshairLine, width: 1, type: 'dashed' as const },
        shadowStyle: { color: theme.crosshair },
      },
      formatter: tooltipFormatter(input),
    },
    series,
  }
}
