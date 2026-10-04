import { describe, expect, it } from 'vitest'
import {
  HOST_METRIC_SERIES,
  METRIC_HISTORY_MAX_COLUMNS,
  METRIC_HISTORY_PRESETS,
  METRIC_HISTORY_SAMPLE_LIMIT,
  NODE_METRIC_SERIES,
  bucketMetricSamples,
  formatCanonicalInstant,
  formatHistoryDuration,
  formatSampleDelay,
  hostMetricDefinition,
  hostMetricNeedsMount,
  metricAvailabilityNotice,
  metricBandPath,
  metricChartGeometry,
  metricGapKindLabel,
  metricGapThresholdSeconds,
  metricGrainLabel,
  metricGrainShortLabel,
  metricGrainSeconds,
  metricHistoryRange,
  metricJudgedCadenceSeconds,
  metricLinePath,
  metricLineRuns,
  metricMeasuredCadenceSeconds,
  metricPointIsAggregate,
  metricPointLabel,
  metricPointProvesHole,
  metricPointWeight,
  metricSampleSummary,
  metricSegmentLabel,
  metricTierNotice,
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

  it('cuts between two buckets when the silence starts inside the previous bucket', () => {
    // The review's reproduction on the aggregate tiers: the Server bounds the
    // silence with the stored observations that bracket it, and those two fall
    // inside the buckets themselves — the first bucket firstInstant 12:00:00 /
    // lastInstant 12:04:30, the second bucket firstInstant 12:10:05, and the
    // silence from 12:04:30 to 12:10:05. The first bucket is drawn at its first
    // observation, so judging the cut at the drawn instants puts the silence
    // after the previous point and the line is drawn straight through it.
    const first = {
      observedAt: instant(12 * HOUR),
      lastObservedAt: instant(12 * HOUR + 270_000),
      value: 3,
      grain: '5m',
      source: 'aggregate',
      minValue: 2,
      maxValue: 4,
      sampleCount: 6,
    }
    const second = {
      observedAt: instant(12 * HOUR + 605_000),
      lastObservedAt: instant(12 * HOUR + 875_000),
      value: 5,
      grain: '5m',
      source: 'aggregate',
      minValue: 5,
      maxValue: 6,
      sampleCount: 6,
    }
    const silence = {
      from: instant(12 * HOUR + 270_000),
      to: instant(12 * HOUR + 605_000),
    }
    const from = BASE + 12 * HOUR
    const geometry = metricChartGeometry([first, second], [silence], from, from + HOUR)

    // Two runs, so two drawn stretches: the cut is judged between the last
    // instant the first bucket testifies to and the first instant the second
    // one does, and neither stretch is drawn across the silence.
    expect(geometry.segments.length).toBe(2)
    expect(geometry.samples).toBe(2)
    expect(geometry.aggregatedPoints).toBe(2)
    expect(geometry.gapBands).toHaveLength(1)
    expect(geometry.segments[0]).toHaveLength(1)
    expect(geometry.segments[0][0].lastInstant).toBe(Date.parse(silence.from))
    expect(geometry.segments[1]).toHaveLength(1)
    expect(geometry.segments[1][0].firstInstant).toBe(Date.parse(silence.to))
  })

  it('leaves two points joined when the reported silence does not separate their evidence', () => {
    const from = BASE + 12 * HOUR
    const bucket = (offsetMs: number) => ({
      observedAt: instant(12 * HOUR + offsetMs),
      lastObservedAt: instant(12 * HOUR + offsetMs + 60_000),
      value: 3,
      grain: '5m',
      source: 'aggregate',
      sampleCount: 2,
    })
    // The silence is inside the first bucket's own span: its evidence runs past
    // the silence and the next bucket's evidence starts long after it, so the
    // two points are still the two ends of one stretch and the line is not cut.
    const inside = metricChartGeometry(
      [bucket(0), bucket(605_000)],
      [{ from: instant(12 * HOUR), to: instant(12 * HOUR + 30_000) }],
      from,
      from + HOUR,
    )
    expect(inside.segments.length).toBe(1)
    expect(inside.segments[0]).toHaveLength(2)

    // A silence that touches neither point cuts nothing either: the two stored
    // observations before it are one stretch, and the silence is left undrawn.
    const elsewhere = metricChartGeometry(
      [
        { observedAt: instant(0), value: 3 },
        { observedAt: instant(5 * 60_000), value: 3 },
      ],
      [{ from: instant(20 * 60_000), to: instant(40 * 60_000) }],
      BASE,
      BASE + HOUR,
    )
    expect(elsewhere.segments.length).toBe(1)
    expect(elsewhere.segments[0]).toHaveLength(2)
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
    expect(metricAvailabilityNotice('partial')).toContain('starts before the investigation horizon')
    expect(metricAvailabilityNotice('unavailable')).toContain(
      'entirely older than the investigation horizon',
    )
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
    // The preset list now reaches past the raw window into the aggregate
    // tiers: 7 days is the 1 minute maximum age, 30 days the horizon.
    expect(METRIC_HISTORY_PRESETS.map((preset) => preset.hours)).toEqual([1, 6, 24, 168, 720])
    expect(METRIC_HISTORY_PRESETS[3].label).toBe('7 days')
    expect(METRIC_HISTORY_PRESETS[4].label).toBe('30 days')
  })

  it('draws a bucket as the counted stretch it kept, never as one sample', () => {
    const columns = bucketMetricSamples(
      [
        { observedAt: instant(0), value: 2.5, grain: 'raw', source: 'raw', sampleCount: 1 },
        {
          // Inside the same 30 second column: one hour over the column budget.
          observedAt: instant(20_000),
          value: 4,
          grain: '1m',
          source: 'aggregate',
          minValue: 1,
          maxValue: 9,
          sampleCount: 6,
          lastObservedAt: instant(25_000),
        },
      ],
      BASE,
      BASE + HOUR,
    )
    expect(columns.length).toBe(1)
    const [column] = columns
    // Two folded points, seven observations: the weight is what the column
    // stands for, and both tiers are visible in the same column.
    expect(column.count).toBe(2)
    expect(column.weight).toBe(7)
    expect(column.aggregateCount).toBe(1)
    expect(column.rawCount).toBe(1)
    // The newest reading is the bucket's, and the counted extremes still shape
    // the band instead of the spike being averaged into the newest value.
    expect(column.last).toBe(4)
    expect(column.min).toBe(1)
    expect(column.max).toBe(9)
    expect(column.lastInstant).toBe(BASE + 25_000)
  })

  it('names the grain a folded point came from', () => {
    const raw = { observedAt: instant(0), value: 2.5, grain: 'raw', source: 'raw', sampleCount: 1 }
    const bucket = { observedAt: instant(0), value: 4, grain: '1m', source: 'aggregate', sampleCount: 6 }
    expect(metricPointIsAggregate(raw)).toBe(false)
    expect(metricPointIsAggregate(bucket)).toBe(true)
    expect(metricPointIsAggregate({ grain: '5m' })).toBe(true)
    // A point carrying no tier information is the stored sample it was before
    // the aggregate tiers existed.
    expect(metricPointIsAggregate({})).toBe(false)
    expect(metricPointWeight(raw)).toBe(1)
    expect(metricPointWeight(bucket)).toBe(6)
    expect(metricPointWeight({})).toBe(1)
    expect(metricPointLabel(raw)).toBe('Stored sample')
    expect(metricPointLabel(bucket)).toBe('1 minute bucket')
    expect(metricPointLabel({ grain: '5m', source: 'aggregate' })).toBe('5 minute bucket')
    expect(metricPointLabel({ source: 'aggregate' })).toBe('Aggregate bucket')
    expect(metricGrainLabel('raw')).toBe('raw samples')
    expect(metricGrainLabel('1m')).toBe('1 minute buckets')
    expect(metricGrainLabel('5m')).toBe('5 minute buckets')
    expect(metricGrainLabel('none')).toBe('no evidence')
    expect(metricGrainShortLabel('5m')).toBe('5 minutes')
    expect(metricGrainShortLabel('none')).toBe('none')
  })

  it('says how many observations a bucket counted and how long they span', () => {
    expect(metricSampleSummary({ observedAt: instant(0), value: 2.5, source: 'raw' })).toBeNull()
    expect(
      metricSampleSummary({
        observedAt: instant(0),
        value: 4,
        grain: '5m',
        source: 'aggregate',
        sampleCount: 1,
      }),
    ).toBe('1 observation')
    expect(
      metricSampleSummary({
        observedAt: instant(0),
        value: 4,
        grain: '5m',
        source: 'aggregate',
        sampleCount: 6,
        lastObservedAt: instant(240_000),
      }),
    ).toBe('6 observations over 4 minutes')
  })

  it('measures a bucket between the observations it holds, not from its window start', () => {
    // The bucket is aligned to 12:00:00 but its oldest observation is at
    // 12:00:05: measuring from the aligned start would report a stretch no
    // observation supports.
    expect(
      metricSampleSummary({
        observedAt: instant(0),
        value: 4,
        grain: '1m',
        source: 'aggregate',
        sampleCount: 6,
        firstObservedAt: instant(5_000),
        lastObservedAt: instant(50_000),
        maxGapSeconds: 0,
      }),
    ).toBe('6 observations over 45 seconds')
    // A bucket that counted a single observation covers no stretch at all,
    // whatever its own window is wide.
    expect(
      metricSampleSummary({
        observedAt: instant(0),
        value: 12,
        grain: '5m',
        source: 'aggregate',
        sampleCount: 1,
        firstObservedAt: instant(50_000),
        lastObservedAt: instant(50_000),
        maxGapSeconds: 0,
      }),
    ).toBe('1 observation')
    // An answer served before the field existed names only the aligned instant,
    // and there is nothing better to measure from.
    expect(
      metricSampleSummary({
        observedAt: instant(0),
        value: 4,
        grain: '5m',
        source: 'aggregate',
        sampleCount: 6,
        lastObservedAt: instant(240_000),
      }),
    ).toBe('6 observations over 4 minutes')
  })

  it('judges a hole against the cadence the answer showed, never against a zero', () => {
    // max(3 x cadence, the 2 minute floor), capped at the slowest collection
    // interval an Agent may be configured with.
    expect(metricGapThresholdSeconds(0)).toBe(120)
    expect(metricGapThresholdSeconds(5)).toBe(120)
    expect(metricGapThresholdSeconds(60)).toBe(180)
    expect(metricGapThresholdSeconds(300)).toBe(900)
    expect(metricGapThresholdSeconds(3600)).toBe(900)
    // The grain a bucket is judged by when the answer measured no cadence.
    expect(metricGrainSeconds('1m')).toBe(60)
    expect(metricGrainSeconds('5m')).toBe(300)
    expect(metricGrainSeconds('raw')).toBe(0)

    // Only stored observations measure a cadence: a bucket's aligned start is
    // not a sampling instant.
    expect(
      metricMeasuredCadenceSeconds([
        { observedAt: instant(0), value: 1 },
        { observedAt: instant(10_000), value: 1 },
        { observedAt: instant(30_000), value: 1 },
      ]),
    ).toBe(10)
    expect(metricMeasuredCadenceSeconds([{ observedAt: instant(0), value: 1 }])).toBe(0)
    expect(
      metricMeasuredCadenceSeconds([
        { observedAt: instant(0), value: 1, grain: '1m', source: 'aggregate', sampleCount: 60 },
        {
          observedAt: instant(60_000),
          value: 1,
          grain: '1m',
          source: 'aggregate',
          sampleCount: 60,
        },
      ]),
    ).toBe(0)

    // With no cadence in the answer, a bucket is judged by the stretch its own
    // observations average out to: a minute over thirty observations is 2
    // seconds, so the floor decides a five minute interval is a hole.
    const holed = {
      observedAt: instant(0),
      value: 4,
      grain: '1m',
      source: 'aggregate',
      sampleCount: 30,
      maxGapSeconds: 300,
    }
    expect(metricJudgedCadenceSeconds(holed)).toBe(2)
    expect(metricPointProvesHole(holed)).toBe(true)
    // A cadence measured at one observation a minute judges the same bucket
    // instead, and 2.5 minutes inside it is then only the sampling rhythm.
    const thin = { ...holed, maxGapSeconds: 150 }
    expect(metricPointProvesHole(thin)).toBe(true)
    expect(metricPointProvesHole(thin, 60)).toBe(false)
    // Zero is the contiguity of what a point counted, never a stretch nobody
    // measured.
    expect(
      metricPointProvesHole({ observedAt: instant(0), value: 4, source: 'raw', maxGapSeconds: 0 }),
    ).toBe(false)
    expect(
      metricPointProvesHole({
        observedAt: instant(0),
        value: 4,
        grain: '5m',
        source: 'aggregate',
        sampleCount: 1,
      }),
    ).toBe(false)
  })

  it('judges a bucket by the integer cadence the Server judges it by', () => {
    // The Server truncates: seven observations over five minutes are judged at
    // 42 seconds there (metric_history.rs judged_cadence_seconds), so the hole
    // threshold is 126 seconds. Rounding 42.857 up here would put the plot 3
    // seconds past the Server's own verdict and draw a line across a hole the
    // Server reports as a break.
    const bucket = {
      observedAt: instant(0),
      value: 4,
      grain: '5m',
      source: 'aggregate',
      sampleCount: 7,
      maxGapSeconds: 126,
    }
    expect(metricJudgedCadenceSeconds(bucket)).toBe(42)
    expect(metricGapThresholdSeconds(metricJudgedCadenceSeconds(bucket))).toBe(126)
    expect(metricPointProvesHole(bucket)).toBe(true)
    expect(metricPointProvesHole({ ...bucket, maxGapSeconds: 125 })).toBe(false)
    // A fractional cadence never rounds up past an integer one either.
    expect(metricGapThresholdSeconds(42.857)).toBe(126)
  })

  it('says a bucket whose own evidence proved a hole was not a closed stretch', () => {
    expect(
      metricSampleSummary({
        observedAt: instant(0),
        value: 4,
        grain: '1m',
        source: 'aggregate',
        sampleCount: 30,
        firstObservedAt: instant(0),
        lastObservedAt: instant(60_000),
        maxGapSeconds: 300,
      }),
    ).toBe(
      '30 observations over 1 minute · the widest measured interval inside it is 5 minutes, so no line is drawn across it',
    )
  })

  it('leaves a column that proved a hole out of the drawn line', () => {
    // Four minute buckets in one run, the third of which measured a five minute
    // interval between two of its own observations: joining it to either
    // neighbour would draw a stretch nobody observed, so it stays a marker of
    // its own between two separate stretches.
    const samples = [0, 60_000, 120_000, 180_000].map((offset, index) => ({
      observedAt: instant(offset),
      value: index + 1,
      grain: '1m',
      source: 'aggregate',
      sampleCount: 60,
      firstObservedAt: instant(offset),
      lastObservedAt: instant(offset + 59_000),
      maxGapSeconds: index === 2 ? 300 : 1,
    }))
    const geometry = metricChartGeometry(samples, [], BASE, BASE + 4 * 60_000)

    expect(geometry.cadenceSeconds).toBe(0)
    expect(geometry.holedPoints).toBe(1)
    expect(geometry.segments.length).toBe(1)
    expect(geometry.segments[0].length).toBe(4)
    expect(geometry.segments[0][1].provenHole).toBe(false)
    expect(geometry.segments[0][2].provenHole).toBe(true)

    const runs = metricLineRuns(geometry.segments[0])
    expect(runs.map((run) => run.length)).toEqual([2, 1])
    // The stretch before the hole is drawn, and the column after it has no
    // neighbour left to be joined to.
    expect(metricLinePath(runs[0]).split('L').length).toBe(2)
  })

  it('lists the tiers an answer was served from and what they mean', () => {
    const rawSegment = {
      from: instant(0),
      to: instant(HOUR),
      grain: 'raw',
      source: 'raw',
      pointCount: 12,
      truncated: false,
    }
    const minuteSegment = {
      from: instant(-24 * HOUR),
      to: instant(0),
      grain: '1m',
      source: 'aggregate',
      pointCount: 1,
      truncated: true,
    }
    expect(metricSegmentLabel(rawSegment)).toBe('stored samples · 12 points')
    expect(metricSegmentLabel(minuteSegment)).toBe('1 minute buckets · 1 point · truncated')
    expect(metricSegmentLabel({ ...minuteSegment, pointCount: 0 })).toBe(
      '1 minute buckets · 0 points · truncated',
    )
    // A single raw stretch explains nothing about tiers to the Operator.
    expect(metricTierNotice([rawSegment])).toBeNull()
    const notice = metricTierNotice([rawSegment, minuteSegment])
    expect(notice).toContain('answered by 1 minute buckets')
    expect(notice).toContain('minimum and the maximum')
    expect(notice).toContain('no bucket is presented as a single stored sample')
    expect(metricTierNotice([rawSegment, minuteSegment, { ...minuteSegment, grain: '5m' }])).toContain(
      '1 minute buckets and 5 minute buckets',
    )
  })

  it('describes every stored Node series with its own unit', () => {
    expect(nodeMetricDefinition('peer_inbound_count').label).toBe('Peer inbound')
    expect(nodeMetricDefinition('data_directory_percent').fixedMax).toBe(100)
    // An unknown metric falls back to the first series instead of throwing.
    expect(nodeMetricDefinition('carrier_pigeons').metric).toBe(
      'process_cpu_percent',
    )
  })

  it('describes every stored Host series and names the ones a mount path identifies', () => {
    expect(HOST_METRIC_SERIES.map((series) => series.metric)).toEqual([
      'cpu_percent',
      'memory_used_bytes',
      'memory_total_bytes',
      'load1',
      'load5',
      'load15',
      'network_rx_bytes_per_sec',
      'network_tx_bytes_per_sec',
      'disk_used_bytes',
      'disk_total_bytes',
    ])
    expect(
      HOST_METRIC_SERIES.every(
        (series) =>
          series.label.length > 0 &&
          series.unit.length > 0 &&
          series.description.length > 0,
      ),
    ).toBe(true)
    // Host CPU is a share of the machine, so it has no ceiling to clamp to the
    // way a Node's own process percentage has.
    expect(
      HOST_METRIC_SERIES.every((series) => series.fixedMax === undefined),
    ).toBe(true)
    expect(nodeMetricDefinition('data_directory_percent').fixedMax).toBe(100)
    // Each quantity is shown in its own unit, never as a bare number.
    expect(hostMetricDefinition('cpu_percent').format(11)).toBe('11%')
    expect(
      hostMetricDefinition('memory_total_bytes').format(17_179_869_184),
    ).toBe('16.0 GiB')
    expect(hostMetricDefinition('load1').format(0.4)).toBe('0.40')
    expect(
      hostMetricDefinition('network_rx_bytes_per_sec').format(1_048_576),
    ).toBe('1.00 MiB/s')
    expect(hostMetricDefinition('disk_used_bytes').format(100)).toBe('100 B')
    // Only the storage series is identified by a mount path, so only those ask
    // for one before reading.
    expect(
      HOST_METRIC_SERIES.filter((series) =>
        hostMetricNeedsMount(series.metric),
      ).map((series) => series.metric),
    ).toEqual(['disk_used_bytes', 'disk_total_bytes'])
    expect(hostMetricNeedsMount('carrier_pigeons')).toBe(false)
    // An unknown metric falls back to the first Host series instead of throwing.
    expect(hostMetricDefinition('carrier_pigeons').metric).toBe('cpu_percent')
    expect(hostMetricDefinition('disk_total_bytes').label).toBe(
      'Host storage capacity',
    )
  })

  it('presents the Host series as the machine the Agent runs on, not one Node', () => {
    expect(hostMetricDefinition('cpu_percent').description).toContain(
      'not one Node process',
    )
    expect(hostMetricDefinition('disk_used_bytes').description).toContain(
      'mount path',
    )
    expect(hostMetricDefinition('disk_used_bytes').description).toContain(
      'a different series',
    )
    expect(hostMetricDefinition('disk_total_bytes').description).toContain(
      'never shown without the capacity',
    )
    expect(
      hostMetricDefinition('network_tx_bytes_per_sec').description,
    ).toContain('shared by every Node')
    // The Node series are a different vocabulary and stay untouched by this one:
    // no name is shared, and Node-only names never appear among the Host ones.
    const hostNames = HOST_METRIC_SERIES.map((series) => String(series.metric))
    const nodeNames = NODE_METRIC_SERIES.map((series) => String(series.metric))
    expect(hostNames.filter((name) => nodeNames.includes(name))).toEqual([])
    expect(hostNames).not.toContain('process_cpu_percent')
  })
})
