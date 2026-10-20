import type { PublicMetricWindow } from './api/generated'

/**
 * The Node Detail Metric Window (ADR 0012, docs/design/webui.md §15.38): the
 * fixed presets the Server answers, and everything the page says about each of
 * them. The Server owns the window and its grain; this module only names them.
 */
export type MetricWindow = PublicMetricWindow

/** Selector order: Live first, then the longer presets from short to long. */
export const METRIC_WINDOWS: readonly MetricWindow[] = ['live', '1h', '6h', '24h', '7d']

/** The URL search parameter that holds the selection. Live writes none. */
export const METRIC_WINDOW_PARAM = 'window'

export function isMetricWindow(value: unknown): value is MetricWindow {
  return typeof value === 'string' && (METRIC_WINDOWS as readonly string[]).includes(value)
}

/**
 * The window a URL names, and whether the URL already says it the canonical
 * way. Live is canonical only without the parameter; anything outside the enum
 * falls back to Live, and either case is rewritten (with `replace`) by the page.
 */
export function readMetricWindow(raw: string | null): { window: MetricWindow; canonical: boolean } {
  if (raw === null) return { window: 'live', canonical: true }
  if (isMetricWindow(raw) && raw !== 'live') return { window: raw, canonical: true }
  return { window: 'live', canonical: false }
}

/**
 * The search string for a selection, with every other parameter kept. Live
 * removes the parameter rather than writing `window=live`.
 */
export function metricWindowSearch(search: URLSearchParams, window: MetricWindow): URLSearchParams {
  const next = new URLSearchParams(search)
  if (window === 'live') next.delete(METRIC_WINDOW_PARAM)
  else next.set(METRIC_WINDOW_PARAM, window)
  return next
}

/** The selector's label for each preset. */
export const METRIC_WINDOW_LABELS: Record<MetricWindow, string> = {
  live: 'Live',
  '1h': '1h',
  '6h': '6h',
  '24h': '24h',
  '7d': '7d',
}

/**
 * How often a longer window is fetched again: at most once per grain, with the
 * window's end following the clock. Live is refetched by Invalidation Events
 * only, so it has no interval.
 */
export const METRIC_WINDOW_REFRESH_MS: Record<MetricWindow, number | false> = {
  live: false,
  '1h': 60_000,
  '6h': 60_000,
  '24h': 300_000,
  '7d': 300_000,
}

/** Every sentence the page states about one window. */
export type MetricWindowText = {
  /** Section title while the window is current: "Latest 6 hours". */
  title: string
  /** Adjective form: "6-hour", as in "Retained 6-hour window". */
  adjective: string
  /** "the last 6 hours", for chart names and descriptions. */
  span: string
  /** The empty-state sentence of a chart with nothing observed. */
  empty: string
  /** Every card's message while the window's first answer is loading. */
  loading: string
}

const LONG_WINDOW_TEXT: Record<Exclude<MetricWindow, 'live'>, MetricWindowText> = {
  '1h': { title: 'Latest hour', adjective: '1-hour', span: 'the last hour', empty: 'No samples in the last hour', loading: 'Loading 1-hour window…' },
  '6h': { title: 'Latest 6 hours', adjective: '6-hour', span: 'the last 6 hours', empty: 'No samples in the last 6 hours', loading: 'Loading 6-hour window…' },
  '24h': { title: 'Latest 24 hours', adjective: '24-hour', span: 'the last 24 hours', empty: 'No samples in the last 24 hours', loading: 'Loading 24-hour window…' },
  '7d': { title: 'Latest 7 days', adjective: '7-day', span: 'the last 7 days', empty: 'No samples in the last 7 days', loading: 'Loading 7-day window…' },
}

/**
 * The page's words for a window. Live keeps the sentences it had before the
 * Metric Window, measured from the answer's own `windowSeconds`.
 */
export function metricWindowText(window: MetricWindow, liveSeconds = 60): MetricWindowText {
  if (window !== 'live') return LONG_WINDOW_TEXT[window]
  return {
    title: 'Latest ' + liveSeconds + ' seconds',
    adjective: liveSeconds + '-second',
    span: 'the last ' + liveSeconds + ' seconds',
    empty: 'No samples in the last minute',
    loading: 'Loading metric history…',
  }
}

const pad = (part: number) => String(part).padStart(2, '0')

function clock(date: Date, seconds: boolean): string {
  return pad(date.getHours()) + ':' + pad(date.getMinutes()) + (seconds ? ':' + pad(date.getSeconds()) : '')
}

function day(date: Date): string {
  return pad(date.getMonth() + 1) + '-' + pad(date.getDate())
}

function sameLocalDay(left: number, right: number): boolean {
  const a = new Date(left)
  const b = new Date(right)
  return a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate()
}

/**
 * The time-axis label of a window: `HH:mm:ss` for Live and 1h, `HH:mm` for
 * 6h and 24h, `MM-dd HH:mm` for 7d. Where a longer window crosses midnight,
 * every label on a day other than the window's own last day carries its date,
 * so no tick before midnight can be read as today's; the labels of the last day
 * stay bare. Live keeps its clock labels exactly as before. The tooltip states
 * each point's or bucket's own instant in the same format.
 */
export function metricAxisTimeFormat(window: MetricWindow, to: number): (value: number) => string {
  const seconds = window === 'live' || window === '1h'
  return (value: number) => {
    const date = new Date(value)
    const dated = window === '7d' || (window !== 'live' && Number.isFinite(to) && !sameLocalDay(value, to))
    return (dated ? day(date) + ' ' : '') + clock(date, seconds)
  }
}
