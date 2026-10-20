import { useEffect, useId, useMemo, useRef, useState } from 'react'
import type { ECharts } from 'echarts/core'
import { cn } from '../lib/utils'
import { useIsDark } from '../lib/useIsDark'
import type { MetricWindowText } from '../metricWindow'
import { loadECharts } from './echartsCore'
import { metricChartMaxBands, type MetricAxisRule } from './metricChartAxis'
import {
  METRIC_CHART_GUTTER,
  metricChartOption,
  withLegendSelection,
  type MetricChartSeriesInput,
  type MetricCountNoun,
  type MetricLegendSelection,
} from './nodeMetricChartOption'

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
 * plot box. The ECharts grid keeps Emerald's margins inside it, so the painted
 * band shrinks on a phone; the value axis is told how many tick bands that band
 * can space apart, so its labels never sit on top of each other.
 *
 * Directions the reader hides through the legend stay hidden for as long as this
 * chart is mounted: across refetches, resizes, theme switches and a window in
 * which the direction has no samples. A different Node mounts a different chart
 * (the page keys the deck by Node), so choices never travel between Nodes.
 */
export type NodeMetricChartProps = {
  /** Card title, used to name the chart for assistive technology. */
  label: string
  kind: 'line' | 'bar'
  from: number
  to: number
  series: MetricChartSeriesInput[]
  /** Ceiling, tick step, unit-bearing title and bare tick label, resolved from
   *  the highest plotted observation and the room the plot has. Samples are
   *  drawn unscaled against the ceiling. Module-level identity keeps the option
   *  stable across renders. */
  axis: MetricAxisRule
  /** Exact value as the tooltip states it. Module-level function identity keeps
   *  the option stable across renders. */
  valueFormat: (value: number) => string
  seconds: number
  /** The Metric Window's own words (ADR 0012). Absent, the chart speaks of the
   *  latest `seconds` exactly as the Live deck always has. */
  windowText?: MetricWindowText
  /** Time-axis label and tooltip instant of the window. */
  timeFormat?: (value: number) => string
  /** What a bucket's count counts in the tooltip; `samples` when absent. A
   *  module-level constant keeps the option stable across renders. */
  countNoun?: MetricCountNoun
  /** Replaces the empty-state sentence while history is still loading. */
  message?: string
  /** What the curve cannot show, read out by assistive technology. */
  description: string
  /** ECharts native legend displayed at bottom: 4, matching Emerald. */
  showLegend?: boolean
  /** The window is the last successful one, held after a failed refresh; the
   *  accessible name then stops calling it "the last N seconds". */
  retained?: boolean
  className?: string
}

/** The pinned plot heights; a card that mounts no chart holds the same box. */
export const NODE_METRIC_CHART_HEIGHT = 'h-[132px] md:h-[168px] lg:h-[192px]'
const CHART_HEIGHT = NODE_METRIC_CHART_HEIGHT

export default function NodeMetricChart({
  label,
  kind,
  from,
  to,
  series,
  axis,
  valueFormat,
  seconds,
  windowText,
  timeFormat,
  countNoun,
  message,
  description,
  showLegend = false,
  retained = false,
  className,
}: NodeMetricChartProps) {
  const container = useRef<HTMLDivElement>(null)
  const chart = useRef<ECharts | null>(null)
  const optionRef = useRef<ReturnType<typeof metricChartOption> | null>(null)
  // Kept outside React state: a legend click is already painted by ECharts, so it
  // only needs remembering for the next full option replacement.
  const legendSelection = useRef<MetricLegendSelection>({})
  const [width, setWidth] = useState(0)
  const [height, setHeight] = useState(0)
  // A chart that could not load says so. Leaving the plot silently blank would
  // let the card look intact while the picture is missing.
  const [failed, setFailed] = useState(false)
  const dark = useIsDark()
  const descriptionId = 'node-metric-chart-desc-' + useId().replaceAll(':', '')

  const points = series.reduce((total, entry) => total + entry.segments.reduce((count, segment) => count + segment.length, 0), 0)
  const runs = series.reduce((total, entry) => total + entry.segments.filter((segment) => segment.length > 0).length, 0)
  const hasSamples = points > 0
  const emptyMessage = message ?? windowText?.empty ?? 'No samples in the last minute'
  const span = windowText
    ? (retained ? 'a retained ' + windowText.adjective + ' window' : windowText.span)
    : (retained ? 'a retained ' + seconds + '-second window' : 'the last ' + seconds + ' seconds')

  // Bar width follows the samples the window actually has, so sixty one-second
  // observations cannot overlap and four observations do not read as hairlines.
  const barWidth = kind === 'bar' && width > 0
    ? Math.max(2, Math.min(12, ((width - METRIC_CHART_GUTTER - 10) / Math.max(1, points)) * 0.55))
    : undefined

  // The axis is scaled to what is drawn: only observations inside the window
  // reach the plot, so only they decide the ceiling.
  const peak = series.reduce((highest, entry) => entry.segments.reduce((top, segment) => segment.reduce((value, item) => (Number.isFinite(item.value) && item.value > value ? item.value : value), top), highest), 0)
  const maxBands = metricChartMaxBands(height, showLegend)
  const scale = useMemo(() => axis(peak, maxBands), [axis, peak, maxBands])

  const option = useMemo(
    () => metricChartOption({ kind, from, to, max: scale.max, interval: scale.interval, series, dark, axisFormat: scale.format, valueFormat, barWidth, showLegend, yAxisName: scale.name, ...(timeFormat ? { timeFormat } : {}), ...(countNoun ? { countNoun } : {}) }),
    [kind, from, to, scale, series, dark, valueFormat, barWidth, showLegend, timeFormat, countNoun],
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
        if (optionRef.current) instance.setOption(withLegendSelection(optionRef.current, legendSelection.current), { notMerge: true })
        instance.on('legendselectchanged', (event) => {
          const selected = (event as { selected?: Record<string, boolean> }).selected
          if (selected) legendSelection.current = { ...selected }
        })
        observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver((entries) => {
          const box = entries[0]?.contentRect
          if (box) {
            setWidth(Math.round(box.width))
            setHeight(Math.round(box.height))
          }
          instance?.resize()
        })
        observer?.observe(container.current)
        setWidth(Math.round(container.current.clientWidth))
        setHeight(Math.round(container.current.clientHeight))
      })
      .catch(() => {
        // A chart that cannot load leaves the card's value, its accessible
        // description and the legend in place — the evidence is never replaced
        // by a blank box — and marks the plot so the gap is visible instead of
        // silent.
        if (!disposed) setFailed(true)
      })
    return () => {
      disposed = true
      observer?.disconnect()
      instance?.off('legendselectchanged')
      instance?.dispose()
      chart.current = null
    }
  }, [hasSamples])

  useEffect(() => {
    chart.current?.setOption(withLegendSelection(option, legendSelection.current), { notMerge: true })
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
        aria-label={label + ' ' + kind + ' chart over ' + span}
        aria-describedby={descriptionId}
        className={cn('relative min-w-0 w-full', CHART_HEIGHT)}
      >
        {hasSamples
          ? <div ref={container} className="h-full w-full" aria-hidden="true" />
          : <p data-slot="node-metric-chart-empty" className="m-0 flex h-full items-center justify-center text-[13px] text-muted-foreground">{emptyMessage}</p>}
        {failed && <p data-slot="node-metric-chart-error" className="absolute inset-0 m-0 flex items-center justify-center text-[13px] text-muted-foreground">{label + ' chart unavailable'}</p>}
      </div>
      <p id={descriptionId} className="sr-only">{description}</p>
    </div>
  )
}
