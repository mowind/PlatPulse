/**
 * The WebUI's single on-demand ECharts entry point.
 *
 * ECharts is loaded with a dynamic import rather than at module scope: the
 * library is roughly 530 KiB minified and only Home's Peer Country View and the
 * Node Detail six-chart deck draw anything with it. Keeping it out of the entry
 * chunk means Login, Admin, Network and the rest of the shell never pay for it,
 * and every contributor registers the same set of charts and components once
 * instead of growing a second, subtly different `use([...])` list.
 *
 * Renderers are registered per drawing need: the map paints on canvas, the
 * six-chart deck paints SVG so the browser can render a 1.5px line crisp at any
 * device pixel ratio and so the curve's real geometry stays inspectable.
 */
export type EChartsCore = typeof import('echarts/core')

/**
 * One in-flight import at a time, and no poisoned memo: concurrent callers share
 * the same attempt, a successful one is remembered for the session, and a
 * rejected one is forgotten so the next mount retries instead of inheriting a
 * dead promise. The failure still reaches the caller — the deck paints an error
 * state rather than an empty plot.
 */
export function memoizeLoader<T>(load: () => Promise<T>): () => Promise<T> {
  let promise: Promise<T> | null = null
  return () => {
    promise ??= load().catch((error: unknown) => {
      promise = null
      throw error
    })
    return promise
  }
}

async function importECharts(): Promise<EChartsCore> {
  const [core, charts, components, renderers] = await Promise.all([
    import('echarts/core'),
    import('echarts/charts'),
    import('echarts/components'),
    import('echarts/renderers'),
  ])
  core.use([
    charts.LineChart,
    charts.BarChart,
    charts.MapChart,
    charts.ScatterChart,
    components.GridComponent,
    components.GeoComponent,
    components.TooltipComponent,
    components.AxisPointerComponent,
    components.LegendComponent,
    renderers.CanvasRenderer,
    renderers.SVGRenderer,
  ])
  return core
}

export const loadECharts = memoizeLoader(importECharts)
