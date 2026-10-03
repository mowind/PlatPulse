import { describe, expect, it } from 'vitest'
import {
  METRIC_HISTORY_MAX_COLUMNS,
  METRIC_HISTORY_SAMPLE_LIMIT,
  bucketMetricSamples,
  formatCanonicalInstant,
  formatHistoryDuration,
  formatSampleDelay,
  metricAvailabilityNotice,
  metricBandPath,
  metricChartGeometry,
  metricGapKindLabel,
  metricHistoryRange,
  metricLinePath,
  niceMetricMax,
  nodeMetricDefinition,
} from './metricHistory'

const HOUR = 3600 * 1000
const BASE = Date.parse('2026-08-12T00:00:00Z')

function instant(offsetMs: number): string {
  return new Date(BASE + offsetMs).toISOString().replace(/\.\d{3}Z$/, 'Z')
}

describe('metric history presentation', () => {
  it('folds a 24 hour window into bounded columns that keep every extreme', () => {
    // A 5 second cadence over 24 hours is 17,280 samples: one per bucket at
    // the column budget, and a single spike must still be visible.
    const samples = Array.from({ length: 17_280 }, (_, index) => ({
      observedAt: instant(index * 5000),
      value: index === 9000 ? 88.5 : 2.5,
    }))
    const columns = bucketMetricSamples(samples, BASE, BASE + 24 * HOUR)

    expect(columns.length).toBeLessThanOrEqual(METRIC_HISTORY_MAX_COLUMNS)
    expect(columns.reduce((total, column) => total + column.count, 0)).toBe(17_280)
    expect(columns.some((column) => column.max === 88.5)).toBe(true)
    // The spike is a maximum, never the whole bucket's value.
    const spike = columns.find((column) => column.max === 88.5)
    expect(spike?.min).toBe(2.5)
  })

  it('keeps an unobserved stretch empty instead of plotting it as zero', () => {
    const columns = bucketMetricSamples(
      [
        { observedAt: instant(0), value: 10 },
        { observedAt: instant(24 * HOUR - 1000), value: 20 },
      ],
      BASE,
      BASE + 24 * HOUR,
    )
    expect(columns.length).toBe(2)
    expect(columns.every((column) => column.min > 0)).toBe(true)
    expect(columns[1].x - columns[0].x).toBeGreaterThan(METRIC_HISTORY_MAX_COLUMNS / 2)
  })

  it('drops unusable samples rather than drawing them', () => {
    const columns = bucketMetricSamples(
      [
        { observedAt: 'not-a-time', value: 5 },
        { observedAt: instant(60_000), value: Number.NaN },
        { observedAt: instant(120_000), value: 4 },
      ],
      BASE,
      BASE + HOUR,
    )
    expect(columns.reduce((total, column) => total + column.count, 0)).toBe(1)
    expect(columns[0].min).toBe(4)
  })

  it('breaks the line on the silence the Server reported, not on column distance', () => {
    const samples = [
      { observedAt: instant(0), value: 3 },
      { observedAt: instant(5 * 60_000), value: 3 },
      { observedAt: instant(10 * 60_000), value: 3 },
      // A protection pause: nothing is stored between minute 10 and minute 40.
      { observedAt: instant(40 * 60_000), value: 3 },
      { observedAt: instant(45 * 60_000), value: 3 },
    ]
    // Bounds exactly as the Server reports them: the two stored observations
    // that bracket the pause, never a fabricated edge inside it.
    const gap = { from: instant(10 * 60_000), to: instant(40 * 60_000) }
    const geometry = metricChartGeometry(samples, [gap], BASE, BASE + HOUR)

    expect(geometry.segments.length).toBe(2)
    expect(geometry.samples).toBe(5)
    expect(geometry.gapBands.length).toBe(1)
    expect(geometry.gapBands[0].width).toBeGreaterThan(0)
    // Every drawn stretch stays on one side of the reported silence: the line
    // is cut where the Server says the silence starts and ends.
    expect(
      geometry.segments[0].every((point) => point.lastInstant <= Date.parse(gap.from)),
    ).toBe(true)
    expect(
      geometry.segments[1].every((point) => point.firstInstant >= Date.parse(gap.to)),
    ).toBe(true)
    const firstEnd = geometry.segments[0].at(-1)
    const secondStart = geometry.segments[1][0]
    // A line joining these two would have crossed the silence.
    expect(secondStart.firstInstant).toBeGreaterThan(firstEnd?.lastInstant ?? 0)
  })

  it('splits a column a reported silence cuts in two, and never draws across it', () => {
    // Two columns for the hour, so minute 10 and minute 20 fold into the same
    // bucket — the case a bucket centre cannot answer. The Server says the
    // stretch between them was silent, so they are drawn apart, each where its
    // own observations are, and the silence keeps its band.
    const samples = [
      { observedAt: instant(0), value: 3 },
      { observedAt: instant(10 * 60_000), value: 3 },
      { observedAt: instant(20 * 60_000), value: 3 },
      { observedAt: instant(25 * 60_000), value: 3 },
      { observedAt: instant(45 * 60_000), value: 3 },
    ]
    const geometry = metricChartGeometry(
      samples,
      [{ from: instant(10 * 60_000), to: instant(20 * 60_000) }],
      BASE,
      BASE + HOUR,
      undefined,
      2,
    )

    expect(geometry.samples).toBe(5)
    expect(geometry.gapBands.length).toBe(1)
    const band = geometry.gapBands[0]
    // The observation before the silence sits alone on its side of it, at the
    // instant it really has: minute 5, not the centre of the half hour.
    expect(geometry.segments.length).toBe(2)
    expect(geometry.segments[0]).toHaveLength(1)
    expect(geometry.segments[0][0].x).toBeCloseTo((5 / 60) * 600, 6)
    expect(geometry.segments[0][0].x).toBeLessThan(band.x)
    expect(geometry.segments[1]).toHaveLength(2)
    for (const segment of geometry.segments) {
      const xs = segment.map((point) => point.x)
      expect(Math.max(...xs) > band.x && Math.min(...xs) < band.x + band.width).toBe(false)
    }
  })

  it('never draws through a silence that shares a bucket with its observations', () => {
    // The reproduction from the review: six observations five minutes apart in
    // pairs, a 24 hour window folded into 120 buckets, and the silence between
    // the first pair and the second. Every observation shares its bucket with
    // the silence, so a bucket centre was plotted inside the silence and the
    // line was drawn straight through it.
    const samples = [300, 330, 540, 570, 750, 780].map((seconds) => ({
      observedAt: instant(seconds * 1000),
      value: 3,
    }))
    const silence = { from: instant(330 * 1000), to: instant(540 * 1000) }
    const geometry = metricChartGeometry(samples, [silence], BASE, BASE + 24 * HOUR)

    expect(geometry.samples).toBe(6)
    expect(geometry.segments.length).toBe(2)
    expect(geometry.gapBands).toHaveLength(1)
    const band = geometry.gapBands[0]
    for (const segment of geometry.segments) {
      const xs = segment.map((point) => point.x)
      // No tolerance is granted here: the drawn stretch itself has to stay
      // outside the silence the Server reported.
      expect(Math.max(...xs) > band.x && Math.min(...xs) < band.x + band.width).toBe(false)
    }
    // The line is cut at the observations that bracket the silence.
    expect(geometry.segments[0].at(-1)?.lastInstant).toBe(Date.parse(silence.from))
    expect(geometry.segments[1][0].firstInstant).toBe(Date.parse(silence.to))
  })

  it('honours a fixed axis maximum and otherwise rounds up to a readable one', () => {
    const samples = [
      { observedAt: instant(0), value: 12 },
      { observedAt: instant(60_000), value: 14 },
    ]
    expect(metricChartGeometry(samples, [], BASE, BASE + HOUR, 100).max).toBe(100)
    expect(metricChartGeometry(samples, [], BASE, BASE + HOUR).max).toBe(20)
    expect(niceMetricMax(0)).toBe(1)
    expect(metricChartGeometry(samples, [], BASE, BASE + HOUR).max).toBe(20)
  })

  it('renders no path without points and no band without two columns', () => {
    expect(metricLinePath([])).toBe('')
    const single = metricChartGeometry(
      [{ observedAt: instant(0), value: 5 }],
      [],
      BASE,
      BASE + HOUR,
    )
    expect(metricBandPath(single.segments[0])).toBe('')
    expect(metricLinePath(single.segments[0])).toContain('M ')
  })

  it('reports a delay that runs the wrong way as a magnitude ahead of receipt', () => {
    // A receipt that precedes its own observation is a clock disagreement: it
    // is reported in the direction that made it suspicious and never as a
    // negative delay, a negative duration, or "0 seconds".
    expect(formatSampleDelay(120)).toBe('2 minutes')
    expect(formatSampleDelay(0)).toBe('0 seconds')
    expect(formatSampleDelay(-300)).toBe('5 minutes ahead of receipt')
    expect(formatSampleDelay(-0.4)).toBe('1 second ahead of receipt')
    expect(formatSampleDelay(null)).toBe('Unknown')
    expect(formatSampleDelay(Number.NaN)).toBe('Unknown')
  })

  it('names windows, durations, and availability honestly', () => {
    expect(formatHistoryDuration(86400)).toBe('24 hours')
    expect(formatHistoryDuration(3600)).toBe('1 hour')
    expect(formatHistoryDuration(1260)).toBe('21 minutes')
    expect(formatHistoryDuration(45)).toBe('45 seconds')
    expect(formatHistoryDuration(0)).toBe('0 seconds')
    expect(metricAvailabilityNotice(null)).toBeNull()
    expect(metricAvailabilityNotice('partial')).toContain('starts before the retained raw window')
    expect(metricAvailabilityNotice('unavailable')).toContain('entirely older than the retained raw window')
    expect(metricGapKindLabel('protection_pause')).toBe('Protection pause')
    expect(metricGapKindLabel('collection_gap')).toBe('Collection gap')
    expect(formatCanonicalInstant(new Date('2026-08-12T00:00:00.750Z'))).toBe('2026-08-12T00:00:00Z')
    // The panel asks for the Server's own maximum, so a dense window is bounded
    // by Server policy rather than by a smaller request.
    expect(METRIC_HISTORY_SAMPLE_LIMIT).toBe(20000)
  })

  it('builds a preset range with stable canonical bounds', () => {
    const range = metricHistoryRange(24, new Date('2026-08-12T12:00:00.400Z'))
    expect(range.to).toBe('2026-08-12T12:00:00Z')
    expect(range.from).toBe('2026-08-11T12:00:00Z')
    expect(metricHistoryRange(1, new Date('2026-08-12T12:00:00Z')).from).toBe('2026-08-12T11:00:00Z')
  })

  it('describes every stored Node series with its own unit', () => {
    expect(nodeMetricDefinition('peer_inbound_count').label).toBe('Peer inbound')
    expect(nodeMetricDefinition('data_directory_percent').fixedMax).toBe(100)
    // An unknown metric falls back to the first series instead of throwing.
    expect(nodeMetricDefinition('carrier_pigeons').metric).toBe('process_cpu_percent')
  })
})
