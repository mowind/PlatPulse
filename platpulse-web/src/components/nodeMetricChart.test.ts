// @vitest-environment node
import { describe, expect, it } from 'vitest'
import { init, use as registerEChartsModules } from 'echarts/core'
import { BarChart, LineChart } from 'echarts/charts'
import { GridComponent, TooltipComponent } from 'echarts/components'
import { SVGRenderer } from 'echarts/renderers'
import { METRIC_SERIES_COLORS, metricChartOption, type MetricChartOptionInput } from './nodeMetricChartOption'
import { formatPercentSample } from './metricChartAxis'

// Exercise ECharts' real layout, not a mocked option. The SVG SSR renderer runs
// the same axis and series geometry the browser draws, so this file is the
// machine proof that a collection silence is never drawn across, that a missing
// observation is never turned into a zero, that an isolated sample stays
// visible, and that a whole-number axis never repeats a rounded tick. The
// option's own contract is asserted in nodeMetricChartOption.test.ts.
registerEChartsModules([LineChart, BarChart, GridComponent, TooltipComponent, SVGRenderer])

const WIDTH = 280
const HEIGHT = 192
// Emerald's left margin is 56px across all cards.
const GUTTER = 56
const FROM = Date.parse('2026-01-01T00:00:00Z')
const WINDOW_SECONDS = 60
// Emerald's grid specifications:
// normal chart: { top: 30, right: 24, bottom: 32, left: 56 }
// with legend:  { top: 30, right: 24, bottom: 52, left: 56 }
const RIGHT_MARGIN = 24
const GRID_TOP = 30
const GRID_BOTTOM = 32
const INNER_WIDTH = WIDTH - GUTTER - RIGHT_MARGIN
const PLOT_HEIGHT = HEIGHT - GRID_TOP - GRID_BOTTOM
function at(seconds: number): number {
  return FROM + seconds * 1000
}
/** Where an observation at `seconds` must land, by arithmetic. */
function windowX(seconds: number): number {
  return GUTTER + (seconds / WINDOW_SECONDS) * INNER_WIDTH
}
/** Where a value must sit on a 0..max axis, by arithmetic. */
function plotY(value: number, max: number): number {
  return GRID_TOP + (1 - value / max) * PLOT_HEIGHT
}

type Sample = [number, number]
type Vertex = [number, number]

/** Named probes: the pixel each observation lands on, read from the live chart. */
function render(overrides: Partial<MetricChartOptionInput>, probes: Record<string, Sample> = {}) {
  const chart = init(null, undefined, { renderer: 'svg', ssr: true, width: WIDTH, height: HEIGHT })
  chart.setOption(metricChartOption({
    kind: 'line',
    from: FROM,
    to: at(60),
    max: 20,
    dark: false,
    axisFormat: (value) => Math.round(value) + '%',
    valueFormat: formatPercentSample,
    series: [{ label: 'Process CPU', color: METRIC_SERIES_COLORS.cpu, segments: [] }],
    ...overrides,
  }))
  const svg = chart.renderToSVGString()
  const pixels: Record<string, number[]> = {}
  for (const [name, sample] of Object.entries(probes)) {
    pixels[name] = chart.convertToPixel({ seriesIndex: 0 }, sample) as number[]
  }
  chart.dispose()
  return { svg, pixels }
}

/** Every M/L vertex of each path painted in one fill or stroke colour. */
function shapeVertices(svg: string, pattern: RegExp): Vertex[][] {
  return Array.from(svg.matchAll(pattern)).map((match) =>
    Array.from(match[1].matchAll(/([ML])(-?[\d.]+) (-?[\d.]+)/g)).map((vertex) => [Number(vertex[2]), Number(vertex[3])] as Vertex),
  )
}

function escaped(color: string): string {
  return color.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

function strokedPaths(color: string): RegExp {
  return new RegExp('<path d="([^"]*)" fill="none" pointer-events="visible" stroke="' + escaped(color) + '"', 'g')
}

/** The dashed split lines: the same stroke as the axis, at 6% opacity. */
function splitLines(color: string): RegExp {
  return new RegExp('<path d="([^"]*)" fill="none" pointer-events="visible" stroke="' + escaped(color) + '" stroke-opacity="0.06"', 'g')
}

function filledPaths(color: string): RegExp {
  return new RegExp('<path d="([^"]+)"[^>]*fill="' + escaped(color) + '"', 'g')
}

/** Every y-axis tick label the renderer painted. The clock labels share the same
 *  canvas now, so a label counts only when the axis placed it inside the gutter:
 *  the y axis is the only text that hangs to the left of the plot box. */
function axisLabels(svg: string): string[] {
  return Array.from(svg.matchAll(/<text([^>]*)>([^<>]+)<\/text>/g))
    .filter((match) => Number(/translate\((-?[\d.]+)/.exec(match[1])?.[1] ?? Number.NaN) < GUTTER - 1)
    .map((match) => match[2])
}

/** The symbol paths: one per drawn marker, tagged with their data index. */
function markers(svg: string): string[] {
  return Array.from(svg.matchAll(/<path [^>]*ecmeta_ssr_type="chart"[^>]*>/g)).map((match) => match[0])
}

function expectNear(actual: number, expected: number, tolerance = 0.15): void {
  expect(Math.abs(actual - expected)).toBeLessThanOrEqual(tolerance)
}

const gap = [
  [{ sampledAt: at(0), value: 10 }, { sampledAt: at(10), value: 10 }, { sampledAt: at(20), value: 10 }],
  [{ sampledAt: at(40), value: 10 }, { sampledAt: at(50), value: 10 }],
]

describe('Node Detail metric chart geometry', () => {
  it('breaks the curve at the Server silence and draws neither side across it', () => {
    const { svg, pixels } = render({ series: [{ label: 'Process CPU', color: METRIC_SERIES_COLORS.cpu, segments: gap }] }, {
      first: [at(0), 10],
      second: [at(10), 10],
      beforeGap: [at(20), 10],
      afterGap: [at(40), 10],
      last: [at(50), 10],
      // Where a fabricated zero for the silence would have been drawn.
      gapMiddleZero: [at(30), 0],
    })
    const runs = shapeVertices(svg, strokedPaths(METRIC_SERIES_COLORS.cpu))
    expect(runs).toHaveLength(2)
    expect(runs[0]).toHaveLength(3)
    expect(runs[1]).toHaveLength(2)
    // Every drawn vertex sits exactly where the Server's own instant puts it:
    // the expectation is arithmetic on the window, never a value ECharts hands
    // back, so the engine cannot agree with itself into a pass.
    const expected: Vertex[][] = [
      [[windowX(0), plotY(10, 20)], [windowX(10), plotY(10, 20)], [windowX(20), plotY(10, 20)]],
      [[windowX(40), plotY(10, 20)], [windowX(50), plotY(10, 20)]],
    ]
    runs.forEach((run, runIndex) => {
      run.forEach((vertex, index) => {
        expectNear(vertex[0], expected[runIndex][index][0])
        expectNear(vertex[1], expected[runIndex][index][1])
      })
    })
    const gapStart = windowX(20)
    const gapEnd = windowX(40)
    expect(gapStart).toBeLessThan(gapEnd)
    for (const vertex of [...runs[0], ...runs[1]]) {
      expect(vertex[0] > gapStart + 0.2 && vertex[0] < gapEnd - 0.2, 'nothing is drawn inside the silence').toBe(false)
      expect(Math.abs(vertex[1] - plotY(0, 20)) > 1, 'a silence is not a drop to zero').toBe(true)
    }
    // The probe ECharts reports agrees with that arithmetic (cross-check only).
    expectNear(pixels.beforeGap[0], windowX(20))
    expectNear(pixels.afterGap[0], windowX(40))
  })

  it('keeps an isolated observation visible as its own marker', () => {
    const { svg } = render(
      { series: [{ label: 'Process CPU', color: METRIC_SERIES_COLORS.cpu, segments: [[{ sampledAt: at(5), value: 10 }]] }] },
      { lone: [at(5), 10] },
    )
    const [run] = shapeVertices(svg, strokedPaths(METRIC_SERIES_COLORS.cpu))
    expect(run, 'a single sample has nothing to connect to').toHaveLength(1)
    const [marker] = markers(svg)
    expect(marker, 'the lone observation is still drawn').toBeDefined()
    expect(marker).toContain('ecmeta_series_index="0"')
    expect(marker).toContain('ecmeta_data_index="0"')
    expect(marker).toContain('fill="' + METRIC_SERIES_COLORS.cpu + '"')
    // symbolSize 8 => zrender scales the unit circle by 4, centred on the sample.
    const transform = /matrix\(([\d.]+),0,0,([\d.]+),([\d.]+),([\d.]+)\)/.exec(marker)
    expect(transform, 'the marker is an 8px circle').not.toBeNull()
    expectNear(Number(transform?.[1]), 4, 0.001)
    expectNear(Number(transform?.[2]), 4, 0.001)
    expectNear(Number(transform?.[3]), windowX(5))
    expectNear(Number(transform?.[4]), plotY(10, 20))
    // A run with two points is a line: it is not decorated with fake markers.
    const curve = render({ series: [{ label: 'Process CPU', color: METRIC_SERIES_COLORS.cpu, segments: [[{ sampledAt: at(0), value: 10 }, { sampledAt: at(30), value: 10 }]] }] })
    expect(markers(curve.svg)).toHaveLength(0)
  })

  it('gives a whole-number metric consecutive whole ticks, with no label printed twice', () => {
    const { svg } = render({
      kind: 'bar',
      max: 2,
      integer: true,
      axisFormat: (value) => String(Math.round(value)),
      series: [{ label: 'Transactions / block', color: METRIC_SERIES_COLORS.transactions, segments: [[{ sampledAt: at(10), value: 1 }, { sampledAt: at(20), value: 2 }]] }],
    })
    const labels = axisLabels(svg)
    expect(labels).toEqual(['0', '1', '2'])
    expect(new Set(labels).size).toBe(labels.length)
  })

  it('never truncates a Process CPU sample above 100%', () => {
    const { svg, pixels } = render({
      max: 150,
      axisFormat: (value) => value + '%',
      series: [{ label: 'Process CPU', color: METRIC_SERIES_COLORS.cpu, segments: [[{ sampledAt: at(0), value: 130 }, { sampledAt: at(30), value: 5 }]] }],
    }, { sampled: [at(0), 130], hundred: [at(0), 100] })
    expect(axisLabels(svg)).toEqual(['0%', '30%', '60%', '90%', '120%', '150%'])
    const [run] = shapeVertices(svg, strokedPaths(METRIC_SERIES_COLORS.cpu))
    expect(run).toHaveLength(2)
    expectNear(run[0][0], pixels.sampled[0])
    expectNear(run[0][1], pixels.sampled[1])
    expect(run[0][1], 'a 130% sample is drawn above the 100% line, not clamped onto it').toBeLessThan(pixels.hundred[1])
  })

  it('draws one bar per observation, centred on its own sample', () => {
    const { svg } = render({
      kind: 'bar',
      max: 4,
      integer: true,
      barWidth: 10,
      axisFormat: (value) => String(Math.round(value)),
      series: [{ label: 'Peer connections', color: METRIC_SERIES_COLORS.inbound, segments: [[{ sampledAt: at(10), value: 1 }, { sampledAt: at(40), value: 3 }]] }],
    })
    const bars = shapeVertices(svg, filledPaths(METRIC_SERIES_COLORS.inbound))
    expect(bars).toHaveLength(2)
    const samples = [[at(10), 1], [at(40), 3]] as const
    bars.forEach((bar, index) => {
      const seconds = (samples[index][0] - FROM) / 1000
      const xs = bar.map((vertex) => vertex[0])
      const top = Math.min(...bar.map((vertex) => vertex[1]))
      // Both expectations are arithmetic: the bar stands on its own instant and
      // its height is the sample's share of the axis.
      expectNear((Math.min(...xs) + Math.max(...xs)) / 2, windowX(seconds), 0.1)
      expectNear(top, plotY(samples[index][1], 4), 0.1)
    })
  })

  it('keeps the Server window on the plot box for lines and for bars alike', () => {
    // A line joining the window's two edges spans the whole plot box.
    const edges = render({ series: [{ label: 'Process CPU', color: METRIC_SERIES_COLORS.cpu, segments: [[{ sampledAt: at(0), value: 10 }, { sampledAt: at(60), value: 10 }]] }] })
    const [span] = shapeVertices(edges.svg, strokedPaths(METRIC_SERIES_COLORS.cpu))
    expect(span).toHaveLength(2)
    expectNear(span[0][0], GUTTER, 0.05)
    expectNear(span[1][0], WIDTH - RIGHT_MARGIN, 0.05)

    // Bars land on their own instant too. ECharts' bar layout would otherwise
    // expand the axis by half a bar width ('containShape'), which moves every
    // bar and fakes blank time at the window's edges.
    for (const samples of [[30], [15, 30, 45], [5, 12, 19, 26, 33, 40, 47]]) {
      const { svg } = render({
        kind: 'bar',
        max: 4,
        integer: true,
        barWidth: 10,
        axisFormat: (value) => String(Math.round(value)),
        series: [{ label: 'Transactions / block', color: METRIC_SERIES_COLORS.transactions, segments: [samples.map((seconds) => ({ sampledAt: at(seconds), value: 2 }))] }],
      })
      const bars = shapeVertices(svg, filledPaths(METRIC_SERIES_COLORS.transactions))
      expect(bars, samples.length + ' observations draw ' + samples.length + ' bars').toHaveLength(samples.length)
      bars.forEach((bar, index) => {
        const xs = bar.map((vertex) => vertex[0])
        expectNear((Math.min(...xs) + Math.max(...xs)) / 2, windowX(samples[index]), 0.1)
      })
    }

    // An observation on the boundary is drawn inside the plot box, half a bar
    // wide: nothing is ever painted outside the window.
    const boundary = render({
      kind: 'bar',
      max: 4,
      integer: true,
      barWidth: 10,
      axisFormat: (value) => String(Math.round(value)),
      series: [{ label: 'Transactions / block', color: METRIC_SERIES_COLORS.transactions, segments: [[{ sampledAt: at(0), value: 2 }, { sampledAt: at(60), value: 2 }]] }],
    })
    const boundaryBars = shapeVertices(boundary.svg, filledPaths(METRIC_SERIES_COLORS.transactions))
    expect(boundaryBars).toHaveLength(2)
    for (const bar of boundaryBars) {
      const xs = bar.map((vertex) => vertex[0])
      expect(Math.min(...xs)).toBeGreaterThanOrEqual(GUTTER - 0.1)
      expect(Math.max(...xs)).toBeLessThanOrEqual(WIDTH - RIGHT_MARGIN + 0.1)
    }
  })

  it('keeps two directions visible when their only observation is the same instant', () => {
    const { svg } = render({
      series: [
        { label: 'Inbound', color: METRIC_SERIES_COLORS.inbound, segments: [[{ sampledAt: at(5), value: 3 }]] },
        { label: 'Outbound', color: METRIC_SERIES_COLORS.outbound, segments: [[{ sampledAt: at(5), value: 1 }]] },
      ],
    })
    const drawn = markers(svg)
    expect(drawn).toHaveLength(2)
    // r4 for the first direction, r3.5 for the second: the smaller marker stays
    // readable on top of the larger one.
    const scales = drawn.map((path) => Number(/matrix\(([\d.]+),0,0,/.exec(path)?.[1]))
    expect(scales).toEqual([4, 3.5])
  })

  it('reserves the gutter every card shares and paints the dark grid in white', () => {
    const light = render({ series: [{ label: 'Process CPU', color: METRIC_SERIES_COLORS.cpu, segments: gap }] })
    const gridlines = shapeVertices(light.svg, splitLines('rgb(0,0,0)'))
    expect(gridlines).toHaveLength(5)
    for (const line of gridlines) {
      // Dashed, low-contrast, and aligned to the gutter every card shares.
      expectNear(Math.min(...line.map((vertex) => vertex[0])), GUTTER)
      expectNear(Math.max(...line.map((vertex) => vertex[0])), WIDTH - RIGHT_MARGIN)
    }
    expect(light.svg).toContain('stroke-dasharray="4,2"')
    expect(light.svg).toContain('fill-opacity="0.55"')

    const dark = render({ dark: true, series: [{ label: 'Process CPU', color: METRIC_SERIES_COLORS.cpu, segments: gap }] })
    expect(dark.svg).toContain('stroke="rgb(255,255,255)" stroke-opacity="0.06" stroke-dasharray="4,2"')
    // The series colours are the brand constants in both modes; only ink moves.
    expect(shapeVertices(dark.svg, strokedPaths(METRIC_SERIES_COLORS.cpu))).toHaveLength(2)
  })
})
