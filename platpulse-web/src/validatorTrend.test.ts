/**
 * The daily-trend presentation contract (#219). These tests hold the surface to
 * the promises the Server makes: a configured local day with no snapshot is a
 * silence and never a zero, an unmeasured delay is Unknown and never 0, a
 * stored row that carries no metric is Unknown rather than a value, only the
 * configured calendar is read, coverage is disclosed instead of assumed, and
 * the counters stay cumulative.
 */
import { describe, expect, it } from 'vitest'
import type {
  AdminValidatorTrendPoint,
  AdminValidatorTrendResponse,
} from './api/generated'
import {
  VALIDATOR_TREND_PAGE_DAYS,
  VALIDATOR_TREND_PRESETS,
  VALIDATOR_TREND_SERIES,
  validatorTrendAssociationLabel,
  validatorTrendAssociationNotice,
  validatorTrendChartGeometry,
  validatorTrendCounterNotice,
  validatorTrendCoverage,
  validatorTrendDayOrdinal,
  validatorTrendDayWindow,
  validatorTrendDelay,
  validatorTrendForeignNotice,
  validatorTrendGapNotice,
  validatorTrendLinePath,
  validatorTrendMonthWindow,
  validatorTrendRange,
  validatorTrendSampleTimeLabel,
  validatorTrendStretchNotice,
  validatorTrendTimestamps,
  validatorTrendTruncationNotice,
  validatorTrendValue,
} from './validatorTrend'

function point(overrides: Partial<AdminValidatorTrendPoint> = {}): AdminValidatorTrendPoint {
  return {
    blockCount: 900,
    clockSuspect: false,
    dayEnd: '2026-02-01T15:00:00Z',
    dayStart: '2026-01-31T15:00:00Z',
    delaySeconds: 30,
    delegatorCount: 40,
    epoch: 5,
    localDate: '2026-02-01',
    monthKey: '2026-02',
    observationKey: 'observation-Asia/Tokyo-2026-02-01',
    providerTimestamp: '2026-01-31T15:00:00Z',
    rank: 12,
    receivedAt: '2026-01-31T15:00:30Z',
    rewardAmount: '25.000000',
    rewardRate: '0.05',
    sampleAt: '2026-01-31T15:00:00Z',
    sampleTime: 'provider',
    source: 'platsScan',
    stakeAmount: '1000.000000',
    ...overrides,
  }
}

function page(overrides: Partial<AdminValidatorTrendResponse> = {}): AdminValidatorTrendResponse {
  return {
    answeredFromLocalDate: '2026-02-01',
    answeredToLocalDate: '2026-02-03',
    associationHistoryPartial: false,
    associations: [],
    associationsTruncated: false,
    clamped: false,
    continuation: null,
    counterSemantics: 'cumulative',
    coverage: 'partial',
    deletedNodes: 0,
    expectedDays: 3,
    firstObservedLocalDate: '2026-02-01',
    foreignRows: 0,
    foreignTimezones: [],
    gaps: [],
    lastObservedLocalDate: '2026-02-03',
    missingDays: 1,
    months: [],
    networkKey: 'platon-mainnet',
    observedDays: 2,
    points: [],
    requestedDays: 3,
    requestedFrom: '2026-01-31T15:00:00Z',
    requestedFromLocalDate: '2026-02-01',
    requestedTo: '2026-02-03T15:00:00Z',
    requestedToLocalDate: '2026-02-03',
    timezone: 'Asia/Tokyo',
    truncated: false,
    validatorId: 'validator-1',
    ...overrides,
  }
}

describe('validatorTrendRange', () => {
  it('asks in UTC instants, with canonical seconds precision', () => {
    const range = validatorTrendRange(7, new Date('2026-02-08T12:00:00.500Z'))
    expect(range).toEqual({ from: '2026-02-01T12:00:00Z', to: '2026-02-08T12:00:00Z' })
  })

  it('keeps every preset inside the Server bound and asks for the extra day', () => {
    for (const preset of VALIDATOR_TREND_PRESETS) {
      expect(preset.limit).toBe(Math.min(preset.days + 1, VALIDATOR_TREND_PAGE_DAYS))
      expect(preset.limit).toBeLessThanOrEqual(preset.days + 1)
      expect(preset.limit).toBeLessThanOrEqual(366)
    }
    // A long window is answered as pages, so the older-days control is a real
    // Owner path instead of a control that can never be enabled.
    expect(
      VALIDATOR_TREND_PRESETS.some((preset) => preset.limit < preset.days + 1),
    ).toBe(true)
    expect(VALIDATOR_TREND_SERIES.map((item) => item.value)).toEqual([
      'rank',
      'stake',
      'delegators',
    ])
  })
})

describe('validatorTrendCoverage', () => {
  it('reads each verdict the Server can send', () => {
    expect(validatorTrendCoverage(page({ coverage: 'complete' })).label).toBe('Complete')
    expect(validatorTrendCoverage(page({ coverage: 'complete' })).tone).toBe('ok')
    expect(validatorTrendCoverage(page({ coverage: 'partial' })).tone).toBe('warning')
    expect(validatorTrendCoverage(page({ coverage: 'unavailable' })).description).toContain(
      'not a zero',
    )
    expect(validatorTrendCoverage(page({ coverage: 'empty' })).label).toBe('Empty')
  })

  it('never reads a verdict it does not know, or no answer, as healthy', () => {
    expect(validatorTrendCoverage(undefined).label).toBe('Unknown')
    expect(validatorTrendCoverage(page({ coverage: 'something-new' })).label).toBe('Unknown')
    expect(validatorTrendCoverage(page({ coverage: 'something-new' })).tone).toBe('neutral')
  })

  it('says a silence only when the answer proves one, and an unanswered window otherwise', () => {
    const silence = validatorTrendCoverage(page({ coverage: 'partial', missingDays: 2 }))
    expect(silence.label).toBe('Partial')
    expect(silence.description).toContain('carry no stored snapshot')
    expect(silence.description).toContain('silences, never as zeros')

    // A full page that is only short of the requested window is partial for a
    // truncation, not because a day inside the answered stretch is missing.
    const unanswered = validatorTrendCoverage(
      page({
        coverage: 'partial',
        expectedDays: 32,
        missingDays: 0,
        observedDays: 31,
        truncated: true,
      }),
    )
    expect(unanswered.label).toBe('Partial')
    expect(unanswered.tone).toBe('warning')
    expect(unanswered.description).not.toContain('carry no stored snapshot')
    expect(unanswered.description).toContain('not the whole requested window')
    expect(unanswered.description).toContain('unanswered rather than silent')
  })
})

describe('validatorTrendValue', () => {
  it('reads a stored zero as a value and a missing metric as unknown', () => {
    expect(validatorTrendValue(point({ delegatorCount: 0 }), 'delegators')).toBe(0)
    expect(validatorTrendValue(point({ delegatorCount: null }), 'delegators')).toBeNull()
    expect(validatorTrendValue(point({ rank: null }), 'rank')).toBeNull()
    expect(validatorTrendValue(point({ stakeAmount: null }), 'stake')).toBeNull()
    expect(validatorTrendValue(point({ stakeAmount: 'not-a-number' }), 'stake')).toBeNull()
    expect(validatorTrendValue(point({ stakeAmount: '1234.500000' }), 'stake')).toBe(1234.5)
  })
})

describe('validatorTrendDayWindow', () => {
  it('states the real width of a configured local day', () => {
    const twentyFive = validatorTrendDayWindow(
      point({ dayStart: '2026-11-01T04:00:00Z', dayEnd: '2026-11-02T05:00:00Z' }),
    )
    expect(twentyFive.hours).toBe(25)
    const twentyThree = validatorTrendDayWindow(
      point({ dayStart: '2026-03-08T05:00:00Z', dayEnd: '2026-03-09T04:00:00Z' }),
    )
    expect(twentyThree.hours).toBe(23)
  })

  it('claims no width when the boundaries cannot be read', () => {
    const broken = validatorTrendDayWindow(point({ dayStart: '', dayEnd: '' }))
    expect(broken.hours).toBeNull()
  })
})

describe('validatorTrendGapNotice', () => {
  it('calls a single missing day a silence, not a zero', () => {
    const notice = validatorTrendGapNotice({
      days: 1,
      fromLocalDate: '2026-02-02',
      toLocalDate: '2026-02-02',
    })
    expect(notice).toContain('2026-02-02')
    expect(notice).toContain('silence, not a zero')
  })

  it('names the whole stretch when several days are missing', () => {
    const notice = validatorTrendGapNotice({
      days: 3,
      fromLocalDate: '2026-02-02',
      toLocalDate: '2026-02-04',
    })
    expect(notice).toContain('3 configured local days')
    expect(notice).toContain('2026-02-02')
    expect(notice).toContain('2026-02-04')
  })
})

describe('validatorTrendStretchNotice', () => {
  it('stays silent while the answered stretch is the requested one', () => {
    expect(validatorTrendStretchNotice(page())).toBeNull()
  })

  it('says a paged answer did not answer the rest, and that the rest is not a silence', () => {
    const notice = validatorTrendStretchNotice(
      page({ answeredFromLocalDate: '2026-02-03', requestedFromLocalDate: '2026-02-01' }),
    )
    expect(notice).toContain('2026-02-03 to 2026-02-03')
    expect(notice).toContain('not silences in this answer')
  })

  it('says a narrowed request was narrowed, not silently honoured', () => {
    const notice = validatorTrendStretchNotice(
      page({
        answeredFromLocalDate: '2026-01-01',
        clamped: true,
        expectedDays: 730,
        requestedFromLocalDate: '2020-01-01',
      }),
    )
    expect(notice).toContain('bounded maximum window')
  })
})

describe('validatorTrendTruncationNotice', () => {
  it('stays silent unless the page really is truncated', () => {
    expect(validatorTrendTruncationNotice(page())).toBeNull()
  })

  it('names the cursor that continues strictly older without repeating a day', () => {
    const notice = validatorTrendTruncationNotice(
      page({ continuation: '2026-02-03', expectedDays: 30, truncated: true }),
    )
    expect(notice).toContain('2026-02-03')
    expect(notice).toContain('no day is answered twice')
  })
})

describe('validatorTrendForeignNotice', () => {
  it('stays silent when every stored day belongs to the configured calendar', () => {
    expect(validatorTrendForeignNotice(page())).toBeNull()
  })

  it('discloses foreign rows without merging them into the configured calendar', () => {
    const notice = validatorTrendForeignNotice(
      page({ foreignRows: 2, foreignTimezones: ['UTC', 'Asia/Tokyo'] }),
    )
    expect(notice).toContain('2 stored snapshot(s)')
    expect(notice).toContain('UTC, Asia/Tokyo')
    expect(notice).toContain('never merged into the configured calendar or quietly re-bucketed')
  })
})

describe('validatorTrendAssociationNotice', () => {
  it('says a purged Node reads as unavailable rather than never having existed', () => {
    const notice = validatorTrendAssociationNotice(
      page({ associationHistoryPartial: true, deletedNodes: 2 }),
    )
    expect(notice).toContain('2 Node(s)')
    expect(notice).toContain('unavailable here rather than never having existed')
    expect(notice).toContain('never re-attached to a surviving Node')
    expect(notice).toContain('retained')
  })

  it('discloses a truncated association list and stays silent otherwise', () => {
    expect(validatorTrendAssociationNotice(page())).toBeNull()
    const notice = validatorTrendAssociationNotice(
      page({ associationsTruncated: true }),
    )
    expect(notice).toContain('only the newest are shown')
  })
})

describe('validatorTrendCounterNotice', () => {
  it('keeps the counters cumulative and refuses to derive earnings', () => {
    const notice = validatorTrendCounterNotice(page())
    expect(notice).toContain('cumulative Provider counters')
    expect(notice).toContain('never computes period earnings, net profit, or a re-bucketed series')
    // Stake is an observed balance, not a Provider counter: the cumulative
    // reading is stated for the two counters it really describes.
    expect(notice).toContain('stake is the balance the Provider reported at that sample')
    expect(notice).not.toContain('Reward, block and stake')
  })

  it('claims no reading when the Server states no semantics', () => {
    expect(validatorTrendCounterNotice(page({ counterSemantics: '' }))).toContain(
      'no reading is claimed',
    )
    expect(validatorTrendCounterNotice(page({ counterSemantics: 'monthly' }))).toContain('monthly')
  })
})

describe('validatorTrendSampleTimeLabel and validatorTrendDelay', () => {
  it('names which timestamp chose the day', () => {
    expect(validatorTrendSampleTimeLabel('provider')).toBe('Provider timestamp')
    expect(validatorTrendSampleTimeLabel('receipt')).toBe('Server receipt (fallback)')
    expect(validatorTrendSampleTimeLabel(null)).toBe('Unknown')
  })

  it('states the sample, the Provider timestamp and the receipt as three named instants', () => {
    const stamps = validatorTrendTimestamps(point())
    expect(stamps.map((stamp) => stamp.label)).toEqual(['Sample', 'Provider', 'Receipt'])
    expect(stamps[0].value).toContain('2026-01-31 15:00:00 UTC')
    expect(stamps[1].value).toContain('2026-01-31 15:00:00 UTC')
    expect(stamps[2].value).toContain('2026-01-31 15:00:30 UTC')

    // A day chosen from the receipt says so, and a Provider timestamp the
    // observation never carried stays Unknown instead of borrowing it.
    const receiptOnly = validatorTrendTimestamps(
      point({ providerTimestamp: null, sampleTime: 'receipt' }),
    )
    expect(receiptOnly[1]).toEqual({ label: 'Provider', value: 'Unknown' })
    expect(receiptOnly[2].value).toContain('2026-01-31 15:00:30 UTC')
  })

  it('never reads an unmeasured delay as zero', () => {
    expect(validatorTrendDelay(null, false)).toBe('Unknown')
    expect(validatorTrendDelay(undefined, false)).toBe('Unknown')
    expect(validatorTrendDelay(0, false)).not.toBe('Unknown')
    expect(validatorTrendDelay(-30, true)).toContain('behind (Provider clock ahead)')
  })
})

describe('validatorTrendChartGeometry', () => {
  const fourDays = page({
    answeredFromLocalDate: '2026-02-01',
    answeredToLocalDate: '2026-02-04',
    expectedDays: 4,
    missingDays: 0,
    observedDays: 4,
    points: [
      point({ localDate: '2026-02-01', rank: 4 }),
      point({ localDate: '2026-02-02', rank: 6 }),
      point({ localDate: '2026-02-03', rank: null }),
      point({ localDate: '2026-02-04', rank: 8 }),
    ],
    requestedToLocalDate: '2026-02-04',
  })

  it('breaks the line where a metric is missing instead of drawing through it', () => {
    const chart = validatorTrendChartGeometry(fourDays, 'rank')
    expect(chart.columns.map((column) => column.localDate)).toEqual([
      '2026-02-01',
      '2026-02-02',
      '2026-02-04',
    ])
    expect(chart.segments.map((segment) => segment.map((column) => column.localDate))).toEqual([
      ['2026-02-01', '2026-02-02'],
      ['2026-02-04'],
    ])
    expect(chart.unknownValues).toBe(1)
    expect(chart.unknownDays).toBe(0)
  })

  it('places each column where its configured local day falls, in order', () => {
    const chart = validatorTrendChartGeometry(fourDays, 'rank')
    const xs = chart.columns.map((column) => column.x)
    expect(xs[0]).toBeLessThan(xs[1])
    expect(xs[1]).toBeLessThan(xs[2])
    expect(chart.columns[0].y).toBeGreaterThan(chart.columns[1].y)
    expect(validatorTrendLinePath(chart.segments[0])).toMatch(/^M [\d.]+ [\d.]+ L [\d.]+ [\d.]+$/)
  })

  it('carries the days this answer proved silent into the drawing', () => {
    const chart = validatorTrendChartGeometry(
      page({
        gaps: [{ days: 1, fromLocalDate: '2026-02-02', toLocalDate: '2026-02-02' }],
        missingDays: 1,
        points: [point({ localDate: '2026-02-01' })],
      }),
      'rank',
    )
    expect(chart.gapBands.length).toBe(1)
    expect(chart.gapBands[0].width).toBeGreaterThan(0)
    expect(chart.unknownDays).toBe(1)
  })

  it('draws nothing it was not given', () => {
    const chart = validatorTrendChartGeometry(page({ points: [] }), 'stake')
    expect(chart.columns).toEqual([])
    expect(chart.segments).toEqual([])
  })
})

describe('configured calendar labels', () => {
  it('states the month boundary the Server mapped into UTC', () => {
    expect(
      validatorTrendMonthWindow({
        firstLocalDate: '2026-02-01',
        lastLocalDate: '2026-02-28',
        monthEnd: '2026-02-28T15:00:00Z',
        monthKey: '2026-02',
        monthStart: '2026-01-31T15:00:00Z',
        observedDays: 28,
      }),
    ).toBe('2026-01-31T15:00:00Z → 2026-02-28T15:00:00Z')
  })

  it('names an association by its resolved display name, otherwise its identifier', () => {
    expect(validatorTrendAssociationLabel({ nodeDisplayName: ' Node A ', nodeId: 'node-1' })).toBe(
      'Node A',
    )
    expect(validatorTrendAssociationLabel({ nodeDisplayName: null, nodeId: 'node-1' })).toBe('node-1')
  })

  it('gives no ordinal to something that is not a real local date', () => {
    expect(validatorTrendDayOrdinal('2026-02-01')).toBe(
      Math.round(Date.UTC(2026, 1, 1) / 86400_000),
    )
    expect(validatorTrendDayOrdinal('2026-02-30')).toBeNull()
    expect(validatorTrendDayOrdinal('')).toBeNull()
  })
})
