import { describe, expect, it } from 'vitest'
import {
  METRIC_CHART_GUTTER,
  METRIC_CHART_THEME,
  METRIC_SERIES_COLORS,
  metricChartOption,
  type MetricChartOptionInput,
} from './nodeMetricChartOption'
import { formatPercentSample, formatRateSample } from './metricChartAxis'

// The option is pure data, so the whole chart contract can be read without a
// canvas: which run became which series, what the axis promises, and the exact
// text a pointer would show. The rendered geometry is asserted separately in
// nodeMetricChart.test.ts, against the real ECharts layout engine.

function build(overrides: Partial<MetricChartOptionInput> = {}) {
  return metricChartOption({
    kind: 'line',
    from: FROM,
    to: at(60),
    max: 20,
    dark: false,
    axisFormat: (value) => value + '%',
    valueFormat: formatPercentSample,
    series: [{ label: 'Process CPU', color: METRIC_SERIES_COLORS.cpu, segments: [[{ sampledAt: at(0), value: 10 }]] }],
    ...overrides,
  }) as unknown as {
    animation: boolean
    grid: Record<string, unknown>
    xAxis: Record<string, unknown>
    yAxis: Record<string, unknown>
    legend?: unknown
    tooltip: { formatter: (raw: unknown) => string; trigger: string; confine: boolean; axisPointer: Record<string, unknown> }
    series: Array<Record<string, unknown>>
  }
}

const FROM = Date.parse('2026-01-01T00:00:00Z')
function at(seconds: number): number {
  return FROM + seconds * 1000
}

/** The same clock the tooltip prints, derived independently of the module. */
function clockOf(value: number): string {
  const date = new Date(value)
  const pad = (part: number) => String(part).padStart(2, '0')
  return pad(date.getHours()) + ':' + pad(date.getMinutes()) + ':' + pad(date.getSeconds())
}

const gap = [
  [{ sampledAt: at(0), value: 10 }, { sampledAt: at(10), value: 10 }, { sampledAt: at(20), value: 10 }],
  [{ sampledAt: at(40), value: 10 }, { sampledAt: at(50), value: 10 }],
]

describe('metricChartOption', () => {
  it('draws one series per uninterrupted run and never bridges a silence', () => {
    const option = build({ series: [{ label: 'Process CPU', color: METRIC_SERIES_COLORS.cpu, segments: gap }] })
    expect(option.series).toHaveLength(2)
    expect(option.series[0].data).toEqual([[at(0), 10], [at(10), 10], [at(20), 10]])
    expect(option.series[1].data).toEqual([[at(40), 10], [at(50), 10]])
    for (const series of option.series) {
      expect(series.type).toBe('line')
      expect(series.connectNulls, 'a hole is never interpolated across').toBe(false)
      expect(series.smooth).toBe(false)
      expect(series.name).toBe('Process CPU')
      expect(series.lineStyle).toEqual({ width: 1.5, color: METRIC_SERIES_COLORS.cpu, cap: 'round' })
    }
  })

  it('carries the observations it was given and nothing else', () => {
    const option = build({ series: [{ label: 'Process CPU', color: METRIC_SERIES_COLORS.cpu, segments: gap }] })
    const drawn = option.series.flatMap((series) => (series.data as number[][]).map((point) => point[0]))
    expect(drawn).toEqual([at(0), at(10), at(20), at(40), at(50)])
    // A silence is not a zero: nothing is invented for the missing stretch, so
    // no point exists between the run before the gap and the run after it.
    for (const point of drawn) {
      expect(point <= at(20) || point >= at(40), 'no sample is placed inside the silence').toBe(true)
    }
  })

  it('shows a marker for an isolated observation so a single sample stays visible', () => {
    const isolated = build({ series: [{ label: 'Process CPU', color: METRIC_SERIES_COLORS.cpu, segments: [[{ sampledAt: at(5), value: 10 }]] }] })
    expect(isolated.series[0].showSymbol).toBe(true)
    expect(isolated.series[0].symbolSize, 'r4 for the card\'s first direction').toBe(8)
    expect(isolated.series[0].itemStyle).toMatchObject({ borderWidth: 1.5, borderColor: METRIC_CHART_THEME.light.markerBorder })

    const curve = build({ series: [{ label: 'Process CPU', color: METRIC_SERIES_COLORS.cpu, segments: [gap[1]] }] })
    expect(curve.series[0].showSymbol).toBe(false)
    expect(curve.series[0].itemStyle).toMatchObject({ borderWidth: 0 })
  })

  it('sizes the second direction\'s lone marker smaller so two coincident samples stay readable', () => {
    const option = build({
      series: [
        { label: 'Inbound', color: METRIC_SERIES_COLORS.inbound, segments: [[{ sampledAt: at(5), value: 3 }]] },
        { label: 'Outbound', color: METRIC_SERIES_COLORS.outbound, segments: [[{ sampledAt: at(5), value: 1 }]] },
      ],
    })
    expect(option.series.map((series) => series.symbolSize)).toEqual([8, 7])
  })

  it('fills only the single-direction curves, with Emerald\'s 25%-to-2% vertical gradient', () => {
    const filled = build({ series: [{ label: 'Process CPU', color: METRIC_SERIES_COLORS.cpu, area: true, segments: [gap[1]] }] })
    expect(filled.series[0].areaStyle).toEqual({
      color: {
        type: 'linear',
        x: 0,
        y: 0,
        x2: 0,
        y2: 1,
        colorStops: [
          { offset: 0, color: 'rgba(255, 107, 107, 0.25)' },
          { offset: 1, color: 'rgba(255, 107, 107, 0.02)' },
        ],
      },
    })
    // A lone observation has no stretch to fill, and a two-direction chart
    // (network, peer connections) asks for no fill at all.
    const lone = build({ series: [{ label: 'Process CPU', color: METRIC_SERIES_COLORS.cpu, area: true, segments: [[{ sampledAt: at(5), value: 10 }]] }] })
    expect(lone.series[0].areaStyle).toBeUndefined()
    const plain = build({ series: [{ label: 'Download', color: METRIC_SERIES_COLORS.download, segments: [gap[1]] }] })
    expect(plain.series[0].areaStyle).toBeUndefined()
  })

  it('leaves the legend off the canvas because the card owns it', () => {
    expect(build().legend).toBeUndefined()
  })

  it('uses a real time axis bounded by the Server window', () => {
    const option = build()
    expect(option.xAxis).toMatchObject({ type: 'time', min: FROM, max: at(60) })
    // The relative "60s … 0s" labels are DOM text under the plot; a time axis
    // aligned to the clock cannot promise the window's own edges.
    expect(option.xAxis.axisLabel).toEqual({ show: false })
    // A bar chart must not let ECharts pad the Server's window: the half-band
    // 'containShape' expansion belongs to the bar layout, not to this axis.
    expect(option.xAxis.containShape).toBeUndefined()
    expect(build({ kind: 'bar' }).xAxis.containShape).toBe(false)
    expect(option.yAxis.axisPointer).toEqual({ label: { show: false }, lineStyle: { opacity: 0 } })
    expect(option.grid.left).toBe(METRIC_CHART_GUTTER)
    expect(option.tooltip.trigger).toBe('axis')
    expect(option.tooltip.confine).toBe(true)
    expect(option.tooltip.axisPointer).toMatchObject({ type: 'cross' })
    expect(option.animation).toBe(false)
  })

  it('states the sample\'s own instant and its exact value', () => {
    const option = build()
    const body = option.tooltip.formatter({ axisValue: at(20), seriesName: 'Process CPU', data: [at(20), 12.5], color: METRIC_SERIES_COLORS.cpu })
    expect(body).toContain(clockOf(at(20)))
    expect(body).toContain('Process CPU')
    expect(body).toContain('12.5%')
    expect(body).toContain('background:' + METRIC_SERIES_COLORS.cpu)
  })

  it('prints the retained sample itself, not a display reading of it', () => {
    // 12.345% must not be pre-rounded onto the card's own one-decimal reading,
    // and a byte rate must keep the unit the Server reported: 1025 B/s and
    // 1024 B/s are different samples, so neither may print as "1.00 KiB/s".
    const percent = build()
    const rate = build({
      valueFormat: formatRateSample,
      series: [{ label: 'Download', color: METRIC_SERIES_COLORS.download, segments: [[{ sampledAt: at(20), value: 1025 }]] }],
    })
    expect(percent.tooltip.formatter({ axisValue: at(20), seriesName: 'Process CPU', data: [at(20), 12.345], color: METRIC_SERIES_COLORS.cpu })).toContain('12.345%')
    expect(percent.tooltip.formatter({ axisValue: at(20), seriesName: 'Process CPU', data: [at(20), 12.1], color: METRIC_SERIES_COLORS.cpu })).toContain('12.1%')
    const body = rate.tooltip.formatter({ axisValue: at(20), seriesName: 'Download', data: [at(20), 1025], color: METRIC_SERIES_COLORS.download })
    expect(body).toContain('1025 B/s')
    expect(body, 'a rate is never rescaled away from its sample').not.toContain('KiB')
  })

  it('drops a direction with no finite value and escapes what it prints', () => {
    const option = build()
    const body = option.tooltip.formatter([
      { axisValue: at(20), seriesName: 'Download', data: [at(20), 1024], color: METRIC_SERIES_COLORS.download },
      { axisValue: at(20), seriesName: 'Upload', data: [at(20), null], color: METRIC_SERIES_COLORS.upload },
    ])
    expect(body).toContain('Download')
    expect(body, 'an absent value is not printed as a number').not.toContain('Upload')
    const escaped = option.tooltip.formatter([{ axisValue: at(20), seriesName: '<Upload & Co>', data: [at(20), 1], color: '#000000' }])
    expect(escaped).toContain('&lt;Upload &amp; Co&gt;')
    expect(escaped).not.toContain('<Upload')
  })

  it('keeps a whole-number axis whole so no tick repeats a label', () => {
    expect(build({ integer: true }).yAxis.minInterval).toBe(1)
    expect(build().yAxis.minInterval).toBeUndefined()
    expect(build({ max: 4, integer: true }).yAxis.max).toBe(4)
  })

  it('draws an integer metric as bars that stay inside their own sample', () => {
    const option = build({
      kind: 'bar',
      max: 4,
      integer: true,
      series: [{ label: 'Transactions / block', color: METRIC_SERIES_COLORS.transactions, segments: gap }],
    })
    expect(option.series).toHaveLength(1)
    expect(option.series[0].type).toBe('bar')
    expect(option.series[0].data).toEqual([[at(0), 10], [at(10), 10], [at(20), 10], [at(40), 10], [at(50), 10]])
    expect(option.series[0].barMaxWidth).toBe(14)
    expect(option.series[0].itemStyle).toEqual({ color: METRIC_SERIES_COLORS.transactions, borderRadius: 2 })
  })

  it('reads its ink from the theme the page is in', () => {
    const light = build({ dark: false })
    const dark = build({ dark: true })
    expect(light.yAxis.axisLabel).toMatchObject({ color: METRIC_CHART_THEME.light.textSecondary, fontSize: 11 })
    expect(dark.yAxis.axisLabel).toMatchObject({ color: METRIC_CHART_THEME.dark.textSecondary })
    expect(dark.tooltip).toBeDefined()
    expect((dark.yAxis.splitLine as { lineStyle: { color: string } }).lineStyle.color).toBe(METRIC_CHART_THEME.dark.splitLine)
    // Series colours are the brand constants in both modes; only the neutrals move.
    for (const key of ['cpu', 'memory', 'download', 'upload', 'inbound', 'outbound', 'blockInterval', 'transactions'] as const) {
      const series = build({ dark: true, series: [{ label: key, color: METRIC_SERIES_COLORS[key], segments: [gap[1]] }] })
      expect((series.series[0].lineStyle as { color: string }).color).toBe(METRIC_SERIES_COLORS[key])
    }
  })
})
