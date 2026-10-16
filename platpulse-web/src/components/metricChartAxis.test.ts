import { describe, expect, it } from 'vitest'
import type { PublicMetricPoint } from '../api/generated'
import {
  formatCountAxis,
  formatCountSample,
  formatMetricRate,
  formatMillisecondsAxis,
  formatMillisecondsSample,
  formatPercentAxis,
  formatPercentSample,
  formatRateAxis,
  formatRateSample,
  integerChartMax,
  niceChartMax,
  percentChartMax,
} from './metricChartAxis'

// The ceiling is a display range, never a rescaling of the samples: these rules
// decide how much room a curve gets, so they are the difference between a
// low-utilisation Node being readable and a 130% Process CPU sample being
// silently truncated onto a 100% axis.
const points = (...values: number[]): PublicMetricPoint[] =>
  values.map((value, index) => ({ sampledAt: new Date(1_800_000_000_000 + index * 1000).toISOString(), value }))

describe('percentChartMax', () => {
  it('keeps the ladder floor so a quiet Node is not flattened onto the baseline', () => {
    expect(percentChartMax([])).toBe(5)
    expect(percentChartMax(points(0))).toBe(5)
    expect(percentChartMax(points(1.4))).toBe(5)
    expect(percentChartMax(points(5))).toBe(5)
  })

  it('climbs to the bucket the window actually reached', () => {
    expect(percentChartMax(points(6))).toBe(10)
    expect(percentChartMax(points(12))).toBe(25)
    expect(percentChartMax(points(30))).toBe(50)
    expect(percentChartMax(points(80))).toBe(100)
  })

  it('keeps climbing past 100% because a Process CPU share is not one core', () => {
    expect(percentChartMax(points(130))).toBe(150)
    expect(percentChartMax(points(150))).toBe(150)
    expect(percentChartMax(points(160))).toBe(200)
    expect(percentChartMax(points(210))).toBe(250)
    expect(percentChartMax(points(380))).toBe(400)
    expect(percentChartMax(points(520))).toBe(600)
  })

  it('reads the whole window, not its first sample', () => {
    expect(percentChartMax(points(3, 47, 12))).toBe(50)
    // A non-finite observation is not a ceiling.
    expect(percentChartMax(points(Number.NaN, 7))).toBe(10)
    expect(percentChartMax(points(Number.POSITIVE_INFINITY, 7))).toBe(10)
  })
})

describe('niceChartMax', () => {
  it('rounds a positive maximum up to a 1/2/5 x 10^n step', () => {
    expect(niceChartMax(1)).toBe(1)
    expect(niceChartMax(7)).toBe(10)
    expect(niceChartMax(40)).toBe(50)
    expect(niceChartMax(120)).toBe(200)
    expect(niceChartMax(900)).toBe(1000)
    expect(niceChartMax(4500)).toBe(5000)
  })

  it('never returns a zero-height range', () => {
    expect(niceChartMax(0)).toBe(1)
    expect(niceChartMax(-4)).toBe(1)
    expect(niceChartMax(Number.NaN)).toBe(1)
  })
})

describe('integerChartMax', () => {
  it('gives a small count five whole steps, so no rounded tick repeats', () => {
    expect(integerChartMax(0)).toBe(4)
    expect(integerChartMax(1)).toBe(4)
    expect(integerChartMax(4)).toBe(4)
    expect(integerChartMax(Number.NaN)).toBe(4)
  })

  it('follows the nice ladder once five steps are not enough', () => {
    expect(integerChartMax(5)).toBe(5)
    expect(integerChartMax(7)).toBe(10)
    expect(integerChartMax(48)).toBe(50)
  })
})

describe('axis label formats', () => {
  it('keeps whole percents whole and one decimal under ten', () => {
    expect(formatPercentAxis(0)).toBe('0%')
    expect(formatPercentAxis(2.5)).toBe('2.5%')
    expect(formatPercentAxis(9.94)).toBe('9.9%')
    expect(formatPercentAxis(10)).toBe('10%')
    expect(formatPercentAxis(150)).toBe('150%')
  })

  it('prints counts as whole numbers', () => {
    expect(formatCountAxis(0)).toBe('0')
    expect(formatCountAxis(7)).toBe('7')
    expect(formatCountAxis(1234.2).replace(/\D/g, '')).toBe('1234')
  })

  it('switches block intervals to seconds only once the number needs it', () => {
    expect(formatMillisecondsAxis(0)).toBe('0ms')
    expect(formatMillisecondsAxis(500.4)).toBe('500ms')
    expect(formatMillisecondsAxis(999)).toBe('999ms')
    expect(formatMillisecondsAxis(1000)).toBe('1s')
    expect(formatMillisecondsAxis(1500)).toBe('1.5s')
    expect(formatMillisecondsAxis(3000)).toBe('3s')
    expect(formatMillisecondsAxis(15000)).toBe('15s')
  })

  it('keeps a byte rate inside the shared 56px gutter', () => {
    expect(formatRateAxis(0)).toBe('0 B/s')
    expect(formatRateAxis(-5)).toBe('0 B/s')
    expect(formatRateAxis(512)).toBe('512 B/s')
    expect(formatRateAxis(1536)).toBe('1.5 KiB/s')
    expect(formatRateAxis(1024 * 1024 * 100)).toBe('100 MiB/s')
  })

  it('states a retained byte rate at its own precision, and refuses to invent one', () => {
    expect(formatMetricRate(null)).toBe('Unknown')
    expect(formatMetricRate(undefined)).toBe('Unknown')
    expect(formatMetricRate(-1)).toBe('Unknown')
    expect(formatMetricRate(Number.NaN)).toBe('Unknown')
    expect(formatMetricRate(5)).toBe('5.00 B/s')
    expect(formatMetricRate(12.5)).toBe('12.5 B/s')
    expect(formatMetricRate(2048)).toBe('2.00 KiB/s')
    expect(formatMetricRate(1024 * 1024)).toBe('1.00 MiB/s')
  })

  // The tooltip states the sample the Server stored: no display rounding, no
  // rescaled unit, so two different samples can never print the same reading.
  it('states a tooltip sample as it was retained, never as a display reading', () => {
    expect(formatPercentSample(12.345)).toBe('12.345%')
    expect(formatPercentSample(12.1)).toBe('12.1%')
    // Three decimals is the display cap, and it is the cap, not the value, that
    // stops there: no displayed percentage or unit conversion happens first.
    expect(formatPercentSample(12.3456)).toBe('12.346%')
    expect(formatRateSample(1025.4)).toBe('1025.4 B/s')
    expect(formatPercentSample(0)).toBe('0%')
    expect(formatPercentSample(Number.NaN)).toBe('Unknown%')
    expect(formatRateSample(1025)).toBe('1025 B/s')
    expect(formatRateSample(1024)).toBe('1024 B/s')
    expect(formatRateSample(0)).toBe('0 B/s')
    expect(formatCountSample(7)).toBe('7')
    expect(formatCountSample(7.4)).toBe('7')
    expect(formatCountSample(Number.NaN)).toBe('Unknown')
    expect(formatMillisecondsSample(12480)).toBe('12480 ms')
    expect(formatMillisecondsSample(12480.5)).toBe('12480.5 ms')
    expect(formatMillisecondsSample(Number.NaN)).toBe('Unknown ms')
  })
})
