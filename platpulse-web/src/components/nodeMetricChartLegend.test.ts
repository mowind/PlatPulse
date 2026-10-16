// @vitest-environment node
import { describe, expect, it } from 'vitest'
import { init, use as registerEChartsModules, type ECharts } from 'echarts/core'
import { LineChart } from 'echarts/charts'
import { GridComponent, LegendComponent, TooltipComponent } from 'echarts/components'
import { SVGRenderer } from 'echarts/renderers'
import {
  METRIC_SERIES_COLORS,
  metricChartOption,
  withLegendSelection,
  type MetricChartSeriesInput,
  type MetricLegendSelection,
} from './nodeMetricChartOption'
import { formatPercentSample } from './metricChartAxis'

// The legend contract against ECharts' real engine: every update replaces the
// whole option so expired runs disappear, and the reader's legend choices are
// restated on each replacement so a hidden direction stays hidden.
registerEChartsModules([LineChart, GridComponent, LegendComponent, TooltipComponent, SVGRenderer])

const FROM = Date.parse('2026-01-01T00:00:00Z')
const at = (seconds: number, base = FROM) => base + seconds * 1000

function escaped(color: string): string {
  return color.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}
function strokedRuns(svg: string, color: string): number[][] {
  const pattern = new RegExp('<path d="([^"]*)" fill="none" pointer-events="visible" stroke="' + escaped(color) + '"', 'g')
  return Array.from(svg.matchAll(pattern)).map((match) => Array.from(match[1].matchAll(/[ML](-?[\d.]+) /g)).map((vertex) => Number(vertex[1])))
}

function network(base: number, download: MetricChartSeriesInput['segments'], upload: MetricChartSeriesInput['segments'], dark = false) {
  return metricChartOption({
    kind: 'line',
    from: base,
    to: at(60, base),
    max: 20,
    dark,
    axisFormat: (value) => String(value),
    valueFormat: formatPercentSample,
    showLegend: true,
    series: [
      { label: 'Download', color: METRIC_SERIES_COLORS.download, segments: download },
      { label: 'Upload', color: METRIC_SERIES_COLORS.upload, segments: upload },
    ],
  })
}

/** Mirrors NodeMetricChart: remember the legend event, restate it on every replacement. */
function mount(width = 280) {
  const chart: ECharts = init(null, undefined, { renderer: 'svg', ssr: true, width, height: 192 })
  let selection: MetricLegendSelection = {}
  chart.on('legendselectchanged', (event) => {
    const selected = (event as { selected?: Record<string, boolean> }).selected
    if (selected) selection = { ...selected }
  })
  return {
    chart,
    update: (option: ReturnType<typeof network>) => chart.setOption(withLegendSelection(option, selection), { notMerge: true }),
    selection: () => selection,
  }
}

const run = (base: number, ...seconds: number[]) => seconds.map((second) => ({ sampledAt: at(second, base), value: 10 }))

describe('Node Detail metric chart legend selection', () => {
  it('keeps a hidden direction hidden across refreshes, resizes and theme switches', () => {
    const { chart, update, selection } = mount()
    update(network(FROM, [run(FROM, 0, 10, 20)], [run(FROM, 0, 10, 20)]))
    expect(strokedRuns(chart.renderToSVGString(), METRIC_SERIES_COLORS.upload)).toHaveLength(1)

    chart.dispatchAction({ type: 'legendToggleSelect', name: 'Upload' })
    expect(selection()).toEqual({ Download: true, Upload: false })
    expect(strokedRuns(chart.renderToSVGString(), METRIC_SERIES_COLORS.upload)).toHaveLength(0)

    for (let refresh = 1; refresh <= 4; refresh += 1) {
      const base = at(refresh * 5)
      update(network(base, [run(base, 0, 10, 20), run(base, 40, 50)], [run(base, 0, 10), run(base, 30, 40)], refresh % 2 === 0))
      if (refresh === 2) chart.resize({ width: 360, height: 192 })
      const svg = chart.renderToSVGString()
      expect(strokedRuns(svg, METRIC_SERIES_COLORS.upload), 'refresh ' + refresh + ' keeps Upload hidden').toHaveLength(0)
      expect(strokedRuns(svg, METRIC_SERIES_COLORS.download), 'refresh ' + refresh + ' keeps Download visible').toHaveLength(2)
    }
    chart.dispose()
  })

  it('without restating the choice a full replacement would show the direction again', () => {
    // Guards the reason withLegendSelection exists: notMerge forgets legend state.
    const chart = init(null, undefined, { renderer: 'svg', ssr: true, width: 280, height: 192 })
    chart.setOption(network(FROM, [run(FROM, 0, 10)], [run(FROM, 0, 10)]), { notMerge: true })
    chart.dispatchAction({ type: 'legendToggleSelect', name: 'Upload' })
    chart.setOption(network(FROM, [run(FROM, 0, 10)], [run(FROM, 0, 10)]), { notMerge: true })
    expect(strokedRuns(chart.renderToSVGString(), METRIC_SERIES_COLORS.upload)).toHaveLength(1)
    chart.dispose()
  })

  it('removes expired runs and never bridges a silence while one entry hides every run of a direction', () => {
    const { chart, update } = mount()
    update(network(FROM, [run(FROM, 0, 10), run(FROM, 20, 30), run(FROM, 40, 50)], [run(FROM, 0, 10)]))
    expect(strokedRuns(chart.renderToSVGString(), METRIC_SERIES_COLORS.download)).toHaveLength(3)

    // The window moves on: two runs are left, and the expired one is gone.
    const base = at(25)
    update(network(base, [run(base, 0, 5), run(base, 20, 25)], [run(base, 0, 10)]))
    const runs = strokedRuns(chart.renderToSVGString(), METRIC_SERIES_COLORS.download)
    expect(runs).toHaveLength(2)
    expect((chart.getOption().series as unknown[]).length, 'no expired series is left behind').toBe(3)
    // The silence between 5s and 20s stays empty: no run spans it.
    const [first, second] = runs
    expect(Math.max(...first)).toBeLessThan(Math.min(...second))

    chart.dispatchAction({ type: 'legendToggleSelect', name: 'Download' })
    expect(strokedRuns(chart.renderToSVGString(), METRIC_SERIES_COLORS.download), 'one entry hides every run').toHaveLength(0)
    chart.dispose()
  })

  it('keeps the choice while a direction has no samples and applies it when samples return', () => {
    const { chart, update, selection } = mount()
    update(network(FROM, [run(FROM, 0, 10)], [run(FROM, 0, 10)]))
    chart.dispatchAction({ type: 'legendToggleSelect', name: 'Download' })
    update(network(FROM, [], [run(FROM, 0, 10)]))
    expect(selection()).toEqual({ Download: false, Upload: true })
    const base = at(10)
    update(network(base, [run(base, 0, 10, 20)], [run(base, 0, 10)]))
    const svg = chart.renderToSVGString()
    expect(strokedRuns(svg, METRIC_SERIES_COLORS.download)).toHaveLength(0)
    expect(strokedRuns(svg, METRIC_SERIES_COLORS.upload)).toHaveLength(1)
    chart.dispose()
  })

  it('leaves an option without a legend, or without choices, untouched', () => {
    const option = network(FROM, [], [])
    expect(withLegendSelection(option, {})).toBe(option)
    const plain = { ...option, legend: undefined }
    expect(withLegendSelection(plain, { Upload: false })).toBe(plain)
  })
})
