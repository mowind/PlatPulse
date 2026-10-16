import { act, cleanup, render } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { EChartsOption } from 'echarts'
import NodeMetricChart from './NodeMetricChart'
import { countAxis, formatPercentSample } from './metricChartAxis'
import { METRIC_SERIES_COLORS, type MetricChartSeriesInput } from './nodeMetricChartOption'

// The component's side of the legend contract, with the engine replaced by a
// recorder: it subscribes once, restates the remembered choice on every full
// replacement and unsubscribes on unmount.
type Handler = (event: unknown) => void
const instances: Array<{
  setOption: ReturnType<typeof vi.fn>
  on: ReturnType<typeof vi.fn>
  off: ReturnType<typeof vi.fn>
  dispose: ReturnType<typeof vi.fn>
  resize: ReturnType<typeof vi.fn>
  handlers: Map<string, Handler>
}> = []

vi.mock('./echartsCore', () => ({
  loadECharts: () => Promise.resolve({
    init: () => {
      const handlers = new Map<string, Handler>()
      const instance = {
        handlers,
        setOption: vi.fn(),
        on: vi.fn((name: string, handler: Handler) => { handlers.set(name, handler) }),
        off: vi.fn((name: string) => { handlers.delete(name) }),
        dispose: vi.fn(),
        resize: vi.fn(),
      }
      instances.push(instance)
      return instance
    },
  }),
}))

const AXIS = countAxis('Connections')
const FROM = Date.parse('2026-01-01T00:00:00Z')
const series = (offset: number): MetricChartSeriesInput[] => [
  { label: 'Inbound', color: METRIC_SERIES_COLORS.inbound, segments: [[{ sampledAt: FROM + offset, value: 3 }, { sampledAt: FROM + offset + 1000, value: 4 }]] },
  { label: 'Outbound', color: METRIC_SERIES_COLORS.outbound, segments: [[{ sampledAt: FROM + offset, value: 2 }]] },
]

function chart(offset: number) {
  return (
    <NodeMetricChart
      label="Peer connections"
      kind="line"
      from={FROM + offset}
      to={FROM + offset + 60_000}
      series={series(offset)}
      axis={AXIS}
      valueFormat={formatPercentSample}
      seconds={60}
      description="Peer connections"
      showLegend
    />
  )
}

const lastLegend = (index = 0) => {
  const calls = instances[index].setOption.mock.calls
  const [option, options] = calls[calls.length - 1] as [EChartsOption, { notMerge?: boolean }]
  expect(options).toEqual({ notMerge: true })
  return option.legend as { selected?: Record<string, boolean> }
}

beforeEach(() => { instances.length = 0 })
afterEach(cleanup)

describe('NodeMetricChart legend selection', () => {
  it('restates a hidden direction on every refresh and theme switch, and cleans up once', async () => {
    const view = render(chart(0))
    await act(async () => {})
    expect(instances).toHaveLength(1)
    expect(instances[0].on).toHaveBeenCalledTimes(1)
    expect(lastLegend().selected).toBeUndefined()

    act(() => instances[0].handlers.get('legendselectchanged')?.({ name: 'Outbound', selected: { Inbound: true, Outbound: false } }))
    for (const offset of [1000, 2000, 3000]) {
      view.rerender(chart(offset))
      expect(lastLegend().selected).toEqual({ Inbound: true, Outbound: false })
    }
    await act(async () => { document.documentElement.classList.add('dark') })
    expect(lastLegend().selected).toEqual({ Inbound: true, Outbound: false })
    document.documentElement.classList.remove('dark')
    await act(async () => {})

    expect(instances).toHaveLength(1)
    expect(instances[0].on).toHaveBeenCalledTimes(1)
    view.unmount()
    expect(instances[0].off).toHaveBeenCalledWith('legendselectchanged')
    expect(instances[0].dispose).toHaveBeenCalledTimes(1)
  })

  it('starts a newly mounted chart with every direction shown', async () => {
    const first = render(chart(0))
    await act(async () => {})
    act(() => instances[0].handlers.get('legendselectchanged')?.({ selected: { Inbound: false, Outbound: true } }))
    first.unmount()
    render(chart(0))
    await act(async () => {})
    expect(instances).toHaveLength(2)
    expect(lastLegend(1).selected).toBeUndefined()
  })
})
