import { useEffect, useId, useMemo, useRef, useState, type ReactNode } from 'react'
import type { ECharts } from 'echarts/core'
import { cn } from '../lib/utils'
import { useIsDark } from '../lib/useIsDark'
import { loadECharts } from './echartsCore'
import { METRIC_CHART_GUTTER, metricChartOption, type MetricChartSeriesInput } from './nodeMetricChartOption'

/**
 * One Node Detail monitoring chart: a 60-second window, the Server's real
 * observations, and nothing else.
 *
 * The data semantics are the page's, not ECharts': the caller hands over runs
 * already cut at the Server-judged silences (see coverageSegments), and this
 * component never joins them, never fills a hole with zero and never draws an
 * observation that falls outside [from, to]. What it adds is the picture: a
 * crosshair tooltip stating each sample's own instant and the value as the API
 * retained it rather than as the page displays it (`formatXSample` caps the
 * decimals at three without converting the unit or pre-rounding to one decimal), a
 * responsive size, both themes, and an accessible name and description that say
 * out loud what the curve cannot show.
 *
 * The plot is one of the pinned heights the accepted visual contract fixes: 132px
 * on phones, 168px from md, 192px from lg — the last matching Emerald's own h-48
 * plot box. The ECharts grid then spends 12px above and 26px below, so the painted
 * band is 154px on desktop and the axis itself can print its clock labels.
 */
export type NodeMetricChartProps = {
  /** Card title, used to name the chart for assistive technology. */
  label: string
  kind: 'line' | 'bar'
  from: number
  to: number
  /** Display ceiling; samples are drawn unscaled against it. */
  max: number
  series: MetricChartSeriesInput[]
  /** Whole-number metric: the axis asks for whole ticks. */
  integer?: boolean
  axisFormat: (value: number) => string
  /** Exact value as the tooltip states it. Module-level function identity keeps
   *  the option stable across renders. */
  valueFormat: (value: number) => string
  seconds: number
  /** Replaces the empty-state sentence while history is still loading. */
  message?: string
  /** What the curve cannot show, read out by assistive technology. */
  description: string
  legend?: ReactNode
  className?: string
}

const CHART_HEIGHT = 'h-[132px] md:h-[168px] lg:h-[192px]'

export default function NodeMetricChart({
  label,
  kind,
  from,
  to,
  max,
  series,
  integer = false,
  axisFormat,
  valueFormat,
  seconds,
  message,
  description,
  legend,
  className,
}: NodeMetricChartProps) {
  const container = useRef<HTMLDivElement>(null)
  const chart = useRef<ECharts | null>(null)
  const optionRef = useRef<ReturnType<typeof metricChartOption> | null>(null)
  const [width, setWidth] = useState(0)
  // A chart that could not load says so. Leaving the plot silently blank would
  // let the card look intact while the picture is missing.
  const [failed, setFailed] = useState(false)
  const dark = useIsDark()
  const descriptionId = 'node-metric-chart-desc-' + useId().replaceAll(':', '')

  const points = series.reduce((total, entry) => total + entry.segments.reduce((count, segment) => count + segment.length, 0), 0)
  const runs = series.reduce((total, entry) => total + entry.segments.filter((segment) => segment.length > 0).length, 0)
  const hasSamples = points > 0
  const emptyMessage = message ?? 'No samples in the last minute'

  // Bar width follows the samples the window actually has, so sixty one-second
  // observations cannot overlap and four observations do not read as hairlines.
  const barWidth = kind === 'bar' && width > 0
    ? Math.max(2, Math.min(12, ((width - METRIC_CHART_GUTTER - 10) / Math.max(1, points)) * 0.55))
    : undefined

  const option = useMemo(
    () => metricChartOption({ kind, from, to, max, series, dark, integer, axisFormat, valueFormat, barWidth }),
    [kind, from, to, max, series, dark, integer, axisFormat, valueFormat, barWidth],
  )
  optionRef.current = option

  // The instance is created once per mounted plot and only updated afterwards,
  // so a refetch or a theme switch never rebuilds the chart element.
  useEffect(() => {
    const element = container.current
    if (!hasSamples || !element) return undefined
    let disposed = false
    let instance: ECharts | null = null
    let observer: ResizeObserver | null = null
    loadECharts()
      .then((core) => {
        if (disposed || !container.current) return
        instance = core.init(container.current, undefined, { renderer: 'svg' })
        chart.current = instance
        if (optionRef.current) instance.setOption(optionRef.current)
        observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver((entries) => {
          const box = entries[0]?.contentRect
          if (box) setWidth(Math.round(box.width))
          instance?.resize()
        })
        observer?.observe(container.current)
        setWidth(Math.round(container.current.clientWidth))
      })
      .catch(() => {
        // A chart that cannot load leaves the card's value, coverage sentence
        // and legend in place — the evidence is never replaced by a blank box —
        // and marks the plot so the gap is visible instead of silent.
        if (!disposed) setFailed(true)
      })
    return () => {
      disposed = true
      observer?.disconnect()
      instance?.dispose()
      chart.current = null
    }
  }, [hasSamples])

  useEffect(() => {
    chart.current?.setOption(option, { notMerge: true })
  }, [option])

  return (
    <div className={cn('mt-auto grid min-w-0 grid-cols-1', className)}>
      <div
        data-slot="node-metric-chart"
        data-kind={kind}
        data-runs={runs}
        data-points={points}
        data-chart-error={failed ? 'load' : undefined}
        role="img"
        aria-label={label + ' ' + kind + ' chart over the last ' + seconds + ' seconds'}
        aria-describedby={descriptionId}
        className={cn('relative min-w-0 w-full', CHART_HEIGHT)}
      >
        {hasSamples
          ? <div ref={container} className="h-full w-full" aria-hidden="true" />
          : <p data-slot="node-metric-chart-empty" className="m-0 flex h-full items-center justify-center text-[13px] text-muted-foreground">{emptyMessage}</p>}
        {failed && <p data-slot="node-metric-chart-error" className="absolute inset-0 m-0 flex items-center justify-center text-[13px] text-muted-foreground">{label + ' chart unavailable'}</p>}
      </div>
      {/* The window edges moved into the axis, where they are real instants on the
          Server's window instead of a relative caption that has to be kept in step
          with it. The legend keeps its slot on every card so the six plots end on
          the same baseline, whether or not the card has directions to name. */}
      <div data-slot="node-metric-legend" className="min-h-6 pt-1">{legend}</div>
      <p id={descriptionId} className="sr-only">{description}</p>
    </div>
  )
}
