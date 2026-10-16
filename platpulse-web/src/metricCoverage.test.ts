import { describe, expect, it } from 'vitest'
import type { PublicMetricPoint, PublicMetricSeriesCoverage } from './api/generated'
import { coverageNote, coverageNotes, coverageSegments, metricCoverage } from './metricCoverage'

const FROM = Date.parse('2026-01-01T00:00:00Z')
const TO = FROM + 60_000

const instant = (seconds: number): string => new Date(FROM + seconds * 1000).toISOString()

const point = (seconds: number, value: number): PublicMetricPoint => ({ sampledAt: instant(seconds), value })

function coverage(overrides: Partial<PublicMetricSeriesCoverage> = {}): PublicMetricSeriesCoverage {
  return {
    metric: 'processCpuPercent',
    observed: true,
    neverObservedProven: true,
    observationCount: 0,
    firstObservedAt: null,
    lastObservedAt: null,
    cadenceSeconds: 0,
    gapThresholdSeconds: 0,
    coveredSeconds: 0,
    unobservedTailSeconds: 0,
    gaps: [],
    ...overrides,
  }
}

describe('coverageSegments', () => {
  it('keeps only the window it was given and keeps the order it arrived in', () => {
    const segments = coverageSegments([point(-5, 5), point(10, 5), point(0, 0), point(70, 5)], FROM, TO, [])
    expect(segments).toEqual([[
      { sampledAt: FROM, value: 0 },
      { sampledAt: FROM + 10_000, value: 5 },
    ]])
  })

  it('keeps a measured zero as evidence', () => {
    const segments = coverageSegments([point(30, 0)], FROM, TO, [])
    expect(segments).toEqual([[{ sampledAt: FROM + 30_000, value: 0 }]])
  })

  it('breaks the line across a silence and resumes at the observation after it', () => {
    const segments = coverageSegments(
      [point(0, 10), point(10, 10), point(40, 10), point(50, 10)],
      FROM, TO,
      [{ from: instant(10), to: instant(40), seconds: 30, kind: 'collection_gap' }],
    )
    expect(segments.map((segment) => segment.map((item) => item.sampledAt - FROM)))
      .toEqual([[0, 10_000], [40_000, 50_000]])
  })

  it('never invents a sample for the silence it cut at', () => {
    // The gap is 30 seconds wide and the Server reported no observation inside
    // it: nothing is resampled, interpolated or filled with a zero to bridge it.
    const segments = coverageSegments(
      [point(0, 10), point(10, 10), point(40, 10), point(50, 10)],
      FROM, TO,
      [{ from: instant(10), to: instant(40), seconds: 30, kind: 'collection_gap' }],
    )
    expect(segments.flat()).toHaveLength(4)
  })

  it('keeps an isolated observation as its own run so it stays visible', () => {
    const segments = coverageSegments(
      [point(0, 10), point(10, 10), point(20, 10), point(55, 10)],
      FROM, TO,
      [{ from: instant(20), to: instant(55), seconds: 35, kind: 'collection_gap' }],
    )
    expect(segments.map((segment) => segment.length)).toEqual([3, 1])
    expect(segments[1]?.[0]).toEqual({ sampledAt: FROM + 55_000, value: 10 })
  })
})

describe('metricCoverage', () => {
  it('finds the answer for one metric and tolerates a response without coverage', () => {
    const series = [coverage(), coverage({ metric: 'processMemoryPercent' })]
    expect(metricCoverage({ series }, 'processMemoryPercent')).toEqual(series[1])
    expect(metricCoverage({ series }, 'peerInboundCount')).toBeUndefined()
    expect(metricCoverage({}, 'processCpuPercent')).toBeUndefined()
    expect(metricCoverage(undefined, 'processCpuPercent')).toBeUndefined()
  })
})

describe('coverageNote', () => {
  it('says a series nobody observed was never observed', () => {
    expect(coverageNote(coverage({ observed: false }))).toBe('No samples reported yet')
  })

  it('calls an absence the Server cannot prove an unknown instead of a never-observed series', () => {
    // The two block series have no ledger, so once retention removes their rows
    // the Server cannot tell a series that went quiet long ago from one nobody
    // ever reported: it must not claim the second (issue #225, Story 49).
    expect(coverageNote(coverage({ observed: false, neverObservedProven: false })))
      .toBe('No retained observations')
    expect(coverageNote(coverage({ observed: false, observationCount: 0, neverObservedProven: false })))
      .toBe('No retained observations')
  })

  it('leaves a series that is observed with nothing silent to report', () => {
    // The provenance flag qualifies an absence; it never turns an observed
    // series into an unnoticed one.
    expect(coverageNote(coverage({ neverObservedProven: false }))).toBeUndefined()
  })

  it('names the age of the last observation when the window holds none', () => {
    // The shared duration formatter names the unit it reached, so 90 seconds reads as a minute.
    expect(coverageNote(coverage({ observationCount: 0, unobservedTailSeconds: 90 })))
      .toBe('Last observation 1 minute ago')
  })

  it('says nothing for a window that holds observations and no decided silence', () => {
    expect(coverageNote(coverage({ observationCount: 4, cadenceSeconds: 35, gapThresholdSeconds: 105, coveredSeconds: 45, unobservedTailSeconds: 20 })))
      .toBeUndefined()
  })

  it('counts the gaps it measured and names a tail at least one threshold long', () => {
    const gaps = [{ from: instant(10), to: instant(40), seconds: 30, kind: 'collection_gap' }]
    expect(coverageNote(coverage({ observationCount: 4, cadenceSeconds: 6, gapThresholdSeconds: 18, coveredSeconds: 24, unobservedTailSeconds: 30, gaps })))
      .toBe('1 gap in this window · No samples for the last 30 seconds')
    expect(coverageNote(coverage({ observationCount: 2, cadenceSeconds: 6, gapThresholdSeconds: 18, coveredSeconds: 10, unobservedTailSeconds: 20 })))
      .toBe('No samples for the last 20 seconds')
  })

  it('never claims silence from an unknown cadence', () => {
    expect(coverageNote(coverage({ observationCount: 3, cadenceSeconds: 0, gapThresholdSeconds: 0, coveredSeconds: 20, unobservedTailSeconds: 40 })))
      .toBeUndefined()
  })
})

describe('coverageNotes', () => {
  it('says one sentence when every direction agrees and names them when they differ', () => {
    const same = [
      coverage({ metric: 'networkTxBytesPerSec', observed: false }),
      coverage({ metric: 'networkRxBytesPerSec', observed: false }),
    ]
    expect(coverageNotes([{ label: 'Upload', coverage: same[0] }, { label: 'Download', coverage: same[1] }]))
      .toBe('No samples reported yet')
    expect(coverageNotes([
      { label: 'Upload', coverage: coverage({ observed: false }) },
      { label: 'Download', coverage: coverage({ observationCount: 0, unobservedTailSeconds: 30 }) },
    ])).toBe('Upload: No samples reported yet · Download: Last observation 30 seconds ago')
    expect(coverageNotes([{ label: 'Upload' }, { label: 'Download' }])).toBeUndefined()

    const unknown = [
      coverage({ metric: 'networkTxBytesPerSec', observed: false, neverObservedProven: false }),
      coverage({ metric: 'networkRxBytesPerSec', observed: false, neverObservedProven: false }),
    ]
    expect(coverageNotes([{ label: 'Upload', coverage: unknown[0] }, { label: 'Download', coverage: unknown[1] }]))
      .toBe('No retained observations')
  })

  it('keeps the label of the one direction that has something to report', () => {
    // Upload is silent and Download is healthy: an unlabelled sentence would read
    // as the whole card having gone quiet.
    expect(coverageNotes([
      { label: 'Upload', coverage: coverage({ observationCount: 4, cadenceSeconds: 5, gapThresholdSeconds: 15, coveredSeconds: 20, unobservedTailSeconds: 40 }) },
      { label: 'Download', coverage: coverage({ observationCount: 3, cadenceSeconds: 5, gapThresholdSeconds: 15, coveredSeconds: 20, unobservedTailSeconds: 5 }) },
    ])).toBe('Upload: No samples for the last 40 seconds')
    // A direction whose answer carries no coverage is not vouched for either, so
    // the direction that does report keeps its label.
    expect(coverageNotes([
      { label: 'Upload', coverage: coverage({ observed: false }) },
      { label: 'Download' },
    ])).toBe('Upload: No samples reported yet')
  })
})
