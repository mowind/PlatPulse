import { describe, expect, it } from 'vitest'
import {
  METRIC_WINDOW_REFRESH_MS,
  METRIC_WINDOWS,
  metricAxisTimeFormat,
  metricWindowSearch,
  metricWindowText,
  readMetricWindow,
} from './metricWindow'

describe('Metric Window (ADR 0012, §15.38)', () => {
  it('reads the URL parameter and falls back to Live for anything outside the enum', () => {
    expect(readMetricWindow(null)).toEqual({ window: 'live', canonical: true })
    for (const window of ['1h', '6h', '24h', '7d'] as const) {
      expect(readMetricWindow(window)).toEqual({ window, canonical: true })
    }
    // Live writes no parameter, so an explicit "live" is rewritten too.
    for (const raw of ['live', '', '30d', '1H', '720h', 'bogus']) {
      expect(readMetricWindow(raw), raw).toEqual({ window: 'live', canonical: false })
    }
  })

  it('writes the selection and keeps every other parameter', () => {
    const search = new URLSearchParams('a=1&window=6h')
    expect(metricWindowSearch(search, '7d').toString()).toBe('a=1&window=7d')
    expect(metricWindowSearch(search, 'live').toString()).toBe('a=1')
    expect(search.toString()).toBe('a=1&window=6h')
  })

  it('refreshes Live only by invalidation and longer windows at most once per grain', () => {
    expect(METRIC_WINDOWS).toEqual(['live', '1h', '6h', '24h', '7d'])
    expect(METRIC_WINDOW_REFRESH_MS).toEqual({ live: false, '1h': 60_000, '6h': 60_000, '24h': 300_000, '7d': 300_000 })
  })

  it('names each window, and keeps Live in the words it always used', () => {
    expect(metricWindowText('live')).toEqual({
      title: 'Latest 60 seconds',
      adjective: '60-second',
      span: 'the last 60 seconds',
      empty: 'No samples in the last minute',
      loading: 'Loading metric history…',
    })
    expect(metricWindowText('6h').loading).toBe('Loading 6-hour window…')
    expect(metricWindowText('7d').loading).toBe('Loading 7-day window…')
    expect(metricWindowText('1h').title).toBe('Latest hour')
    expect(metricWindowText('24h').adjective).toBe('24-hour')
  })

  it('labels the time axis at each window\'s own grain and dates a label across midnight', () => {
    const noon = new Date(2026, 9, 20, 12, 34, 56).getTime()
    const earlier = new Date(2026, 9, 20, 6, 0, 0).getTime()
    const yesterday = new Date(2026, 9, 19, 23, 0, 0).getTime()
    expect(metricAxisTimeFormat('live', noon)(noon)).toBe('12:34:56')
    expect(metricAxisTimeFormat('1h', noon)(noon)).toBe('12:34:56')
    expect(metricAxisTimeFormat('6h', noon)(noon)).toBe('12:34')
    expect(metricAxisTimeFormat('24h', noon)(noon)).toBe('12:34')
    expect(metricAxisTimeFormat('7d', noon)(noon)).toBe('10-20 12:34')
    // Where a window crosses midnight, the labels before it carry their date
    // and the window's own last day stays bare.
    expect(metricAxisTimeFormat('24h', noon)(yesterday)).toBe('10-19 23:00')
    expect(metricAxisTimeFormat('24h', noon)(noon)).toBe('12:34')
    expect(metricAxisTimeFormat('1h', noon)(yesterday)).toBe('10-19 23:00:00')
    expect(metricAxisTimeFormat('6h', noon)(earlier)).toBe('06:00')
    // Live keeps its clock labels exactly as before, midnight or not.
    expect(metricAxisTimeFormat('live', noon)(noon)).toBe('12:34:56')
  })
})
