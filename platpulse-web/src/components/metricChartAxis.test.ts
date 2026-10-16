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
  formatRateAxisAt,
  formatRateSample,
  integerChartMax,
  integerChartTicks,
  niceChartMax,
  percentChartMax,
  rateChartMax,
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

// The byte-rate ceilings are their own ladder: the decimal 1/2/5 ladder put a
// 5 MiB/s window peak on a 9.5 MiB/s axis, which is a picture of an empty chart.
describe('rateChartMax', () => {
  it('puts the 5 MiB/s peak the deck actually saw on a 6 MiB/s axis', () => {
    expect(rateChartMax(points(5 * 1024 * 1024))).toBe(6 * 1024 * 1024)
    expect(rateChartMax(points(6 * 1024 * 1024))).toBe(7 * 1024 * 1024)
    expect(rateChartMax(points(7.7 * 1024 * 1024))).toBe(9 * 1024 * 1024)
    expect(rateChartMax(points(3 * 1024 * 1024))).toBe(3.5 * 1024 * 1024)
    expect(rateChartMax(points(1024 * 1024))).toBe(1.125 * 1024 * 1024)
    expect(rateChartMax(points(1024))).toBe(1152)
  })

  it('leaves a small headroom above the peak at every magnitude', () => {
    const peaks = [
      1024,
      12_288,
      1024 * 1024,
      1.5 * 1024 * 1024,
      1.6 * 1024 * 1024,
      3 * 1024 * 1024,
      5 * 1024 * 1024,
      6 * 1024 * 1024,
      9 * 1024 * 1024,
      12 * 1024 * 1024,
      12.7 * 1024 * 1024,
      1024 ** 3,
    ]
    for (const peak of peaks) {
      const max = rateChartMax(points(peak))
      expect(max, String(peak)).toBeGreaterThanOrEqual(peak)
      // The ceiling is the first rung that clears the peak by 5%, and the ladder's
      // widest rung gap is 1.2x, so the headroom cannot leave [5%, 26%].
      expect(max / peak, String(peak)).toBeGreaterThanOrEqual(1.05)
      expect(max / peak, String(peak)).toBeLessThanOrEqual(1.26)
    }
    // The 26% end of that band: a peak just above the 2.5 x 2^15 rung, whose next
    // rung (3 x 2^15) sits across the ladder's widest gap, so the ceiling must jump it.
    expect(rateChartMax(points(39_010))).toBe(49_152)
    expect(rateChartMax(points(40_960))).toBe(49_152)
  })

  it('holds that window across every magnitude, and keeps its ceiling above the peak', () => {
    // The ladder is 128 B x 2^n, so sweep each octave in even steps instead of
    // only the tidy peaks listed above: an off-ladder peak must still land in the
    // window, and a peak past the ladder's last rung must never outrun its ceiling.
    for (let index = 0; index <= 4000; index += 1) {
      const peak = 2 ** (40 * (index / 4000)) // 1 B/s .. 1 TiB/s
      const max = rateChartMax(points(peak))
      expect(max, String(peak)).toBeGreaterThanOrEqual(peak)
      if (peak >= 128) {
        // Below 128 B/s the axis simply sits on the ladder's own floor.
        expect(max / peak, String(peak)).toBeGreaterThanOrEqual(1.05)
        expect(max / peak, String(peak)).toBeLessThanOrEqual(1.26)
      }
    }
    for (const peak of [1024 ** 4, 10 ** 12, 10 ** 15]) {
      const max = rateChartMax(points(peak))
      expect(max, String(peak)).toBeGreaterThanOrEqual(peak)
      expect(max / peak, String(peak)).toBeLessThanOrEqual(1.26)
    }
  })

  it('reads the whole window and never returns a zero-height range', () => {
    expect(rateChartMax(points(3, 6 * 1024 * 1024, 12))).toBe(7 * 1024 * 1024)
    expect(rateChartMax([])).toBe(128)
    expect(rateChartMax(points(0))).toBe(128)
    expect(rateChartMax(points(Number.NaN))).toBe(128)
  })
})

describe('formatRateAxisAt', () => {
  it('states every tick in the unit the ceiling chose, never mixing KiB with MiB', () => {
    const max = 6 * 1024 * 1024
    expect(formatRateAxisAt(0, max)).toBe('0 MiB/s')
    expect(formatRateAxisAt(1024 * 1024, max)).toBe('1 MiB/s')
    expect(formatRateAxisAt(1.2 * 1024 * 1024, max)).toBe('1.2 MiB/s')
    expect(formatRateAxisAt(3 * 1024 * 1024, max)).toBe('3 MiB/s')
    expect(formatRateAxisAt(6 * 1024 * 1024, max)).toBe('6 MiB/s')
  })

  it('keeps the label inside the shared gutter at either end of the scale', () => {
    expect(formatRateAxisAt(512, 1024)).toBe('0.5 KiB/s')
    expect(formatRateAxisAt(262_144, 1.25 * 1024 * 1024)).toBe('0.25 MiB/s')
    expect(formatRateAxisAt(128 * 1024 * 1024, 200 * 1024 * 1024)).toBe('128 MiB/s')
    expect(formatRateAxisAt(0, 0)).toBe('0 B/s')
    expect(formatRateAxisAt(Number.NaN, 1024)).toBe('0 KiB/s')
  })
})

// A count axis whose ceiling does not divide by its split count is where
// "0, 1, 1, 2" appears: the step is stated outright instead of left to the
// renderer's rounding.
describe('integerChartTicks', () => {
  it('gives the count axis whole steps that cannot repeat a rounded label', () => {
    expect(integerChartTicks(2)).toEqual({ splitNumber: 2, interval: 1 })
    expect(integerChartTicks(4)).toEqual({ splitNumber: 4, interval: 1 })
    expect(integerChartTicks(5)).toEqual({ splitNumber: 5, interval: 1 })
    expect(integerChartTicks(10)).toEqual({ splitNumber: 5, interval: 2 })
    expect(integerChartTicks(50)).toEqual({ splitNumber: 5, interval: 10 })
  })

  it('keeps a whole step for a ceiling no split count divides', () => {
    expect(integerChartTicks(7)).toEqual({ splitNumber: 3, interval: 2 })
    expect(integerChartTicks(0)).toEqual({ splitNumber: 5, interval: 1 })
    expect(integerChartTicks(Number.NaN)).toEqual({ splitNumber: 5, interval: 1 })
  })
})
