import { describe, expect, it } from 'vitest'
import {
  countAxis,
  durationAxis,
  formatCountSample,
  formatMetricRate,
  formatMillisecondsSample,
  formatPercentSample,
  formatRateSample,
  metricChartMaxBands,
  niceAxisScale,
  percentAxis,
  percentChartMax,
  rateAxis,
  type MetricAxis,
} from './metricChartAxis'

// The ceiling is a display range, never a rescaling of the samples: these rules
// decide how much room a curve gets, so they are the difference between a
// low-utilisation Node being readable and a 130% Process CPU sample being
// silently truncated onto a 100% axis.

const MiB = 1024 * 1024

/** Every label the renderer would print for this axis, bottom to top. */
function tickLabels(axis: MetricAxis): string[] {
  const labels: string[] = []
  for (let index = 0; index * axis.interval <= axis.max * (1 + 1e-9); index += 1) labels.push(axis.format(index * axis.interval))
  return labels
}

describe('percentChartMax', () => {
  it('keeps the ladder floor so a quiet Node is not flattened onto the baseline', () => {
    expect(percentChartMax(0)).toBe(5)
    expect(percentChartMax(1.4)).toBe(5)
    expect(percentChartMax(5)).toBe(5)
  })

  it('climbs to the bucket the window actually reached', () => {
    expect(percentChartMax(6)).toBe(10)
    expect(percentChartMax(12)).toBe(25)
    expect(percentChartMax(30)).toBe(50)
    expect(percentChartMax(80)).toBe(100)
  })

  it('keeps climbing past 100% because a Process CPU share is not one core', () => {
    expect(percentChartMax(130)).toBe(150)
    expect(percentChartMax(150)).toBe(150)
    expect(percentChartMax(160)).toBe(200)
    expect(percentChartMax(210)).toBe(250)
    expect(percentChartMax(380)).toBe(400)
    expect(percentChartMax(520)).toBe(600)
  })

  it('reads a peak the window cannot have as no load at all', () => {
    expect(percentChartMax(Number.NaN)).toBe(5)
    expect(percentChartMax(Number.POSITIVE_INFINITY)).toBe(5)
    expect(percentChartMax(-3)).toBe(5)
  })
})

describe('metricChartMaxBands', () => {
  it('labels the plot heights the deck pins, up to five bands like Emerald', () => {
    // 132px is the phone plot, 168px from md, 192px from lg (Emerald's h-48).
    expect(metricChartMaxBands(192, false)).toBe(5)
    expect(metricChartMaxBands(192, true)).toBe(5)
    expect(metricChartMaxBands(168, true)).toBe(5)
    expect(metricChartMaxBands(132, false)).toBe(5)
    // A phone-height plot that also gives 20px to a legend has room for three.
    expect(metricChartMaxBands(132, true)).toBe(3)
  })

  it('never drops below one band, however small the box a layout hands over', () => {
    expect(metricChartMaxBands(0, false)).toBe(5)
    expect(metricChartMaxBands(Number.NaN, false)).toBe(5)
    expect(metricChartMaxBands(20, true)).toBe(1)
  })
})

// The axis title carries the unit and the ticks are bare numbers: a tick column
// of "28.6 MiB/s" labels was wider than Emerald's 56px gutter and was clipped.
describe('rateAxis', () => {
  const speed = rateAxis('Speed')

  it('names the unit once in the title and steps in nice numbers of that unit', () => {
    const axis = speed(110 * MiB, 5)
    expect(axis.name).toBe('Speed (MiB/s)')
    expect(axis.max).toBe(120 * MiB)
    expect(axis.interval).toBe(30 * MiB)
    expect(tickLabels(axis)).toEqual(['0', '30', '60', '90', '120'])
  })

  it('picks the unit from the peak, so one axis never mixes KiB/s with MiB/s', () => {
    expect(speed(600 * 1024, 5).name).toBe('Speed (KiB/s)')
    expect(tickLabels(speed(600 * 1024, 5)).every((label) => /^\d+(\.\d+)?$/.test(label))).toBe(true)
    expect(speed(500, 5).name).toBe('Speed (B/s)')
    expect(speed(3 * 1024 * MiB, 5).name).toBe('Speed (GiB/s)')
  })

  it('clears the peak by at least 5% and wastes at most about a third of the box', () => {
    for (let peak = 1000; peak < 4 * 1024 * MiB; peak *= 1.37) {
      for (const bands of [3, 4, 5]) {
        const axis = speed(peak, bands)
        expect(axis.max, String(peak)).toBeGreaterThanOrEqual(peak * 1.05)
        const count = axis.max / axis.interval
        expect(Math.abs(count - Math.round(count)), 'ceiling is a whole number of steps').toBeLessThan(1e-9)
        expect(Math.round(count)).toBeLessThanOrEqual(bands)
        // The decimal ladder's widest rung gap is 1.5x, so the bound depends on
        // the band budget: measured worst cases are 31.2% at four and five bands
        // and 40.0% at three. A half-step rung (1.125) would tighten this, but
        // under a GiB/s unit it prints a 2.8125 tick, which is worse than the
        // headroom it saves.
        expect(axis.max / peak, JSON.stringify({ peak, bands })).toBeLessThan(bands >= 4 ? 1.32 : 1.41)
        const labels = tickLabels(axis)
        expect(new Set(labels).size, labels.join(',')).toBe(labels.length)
        expect(Math.max(...labels.map((label) => label.length)), labels.join(',')).toBeLessThanOrEqual(5)
      }
    }
  })

  it('still draws a box for an idle or unknown window', () => {
    expect(speed(0, 5).max).toBeGreaterThan(0)
    expect(speed(Number.NaN, 5).max).toBeGreaterThan(0)
  })
})

describe('percentAxis', () => {
  const cpu = percentAxis('CPU')

  it('keeps the percent ladder, past 100%, and moves % into the title', () => {
    const axis = cpu(131, 5)
    expect(axis).toMatchObject({ max: 150, interval: 30, name: 'CPU (%)' })
    expect(tickLabels(axis)).toEqual(['0', '30', '60', '90', '120', '150'])
    expect(tickLabels(cpu(2, 5))).toEqual(['0', '1', '2', '3', '4', '5'])
  })

  it('steps every ladder rung into bands a reader can count', () => {
    for (const peak of [1, 6, 12, 30, 80, 130, 160, 210, 380, 520, 1100]) {
      const axis = cpu(peak, 5)
      const labels = tickLabels(axis)
      const count = axis.max / axis.interval
      expect(Number.isInteger(count) && count <= 5, String(peak)).toBe(true)
      expect(labels.length, String(peak)).toBeGreaterThan(1)
      expect(new Set(labels).size, JSON.stringify(labels)).toBe(labels.length)
      expect(axis.max, String(peak)).toBeGreaterThanOrEqual(peak)
    }
  })

  it('moves the ceiling past a rung no nice step divides into five bands', () => {
    // A 620% peak lands on the 700% rung, and no step a reader counts in (250,
    // 300, 400 …) divides 700 into five bands. Printing only the floor and the
    // ceiling would satisfy "divides evenly" and still be an unreadable axis.
    expect(cpu(620, 5)).toMatchObject({ max: 750, interval: 150 })
    expect(tickLabels(cpu(620, 5))).toEqual(['0', '150', '300', '450', '600', '750'])
    expect(cpu(1100, 5)).toMatchObject({ max: 1200, interval: 300 })
    expect(tickLabels(cpu(1100, 5))).toEqual(['0', '300', '600', '900', '1200'])
  })
})

describe('countAxis', () => {
  const peers = countAxis('Connections')

  it('steps in whole numbers, so no count is printed twice', () => {
    expect(peers(2, 5)).toMatchObject({ max: 4, interval: 1, name: 'Connections' })
    expect(peers(220, 5)).toMatchObject({ max: 240, interval: 60 })
    expect(peers(220, 3)).toMatchObject({ max: 240, interval: 80 })
    for (let peak = 0; peak < 20_000; peak = peak * 1.3 + 1) {
      const axis = peers(peak, 3)
      expect(Number.isInteger(axis.interval) && axis.max >= peak).toBe(true)
      const labels = tickLabels(axis)
      expect(new Set(labels).size).toBe(labels.length)
    }
  })
})

// The axis used to be titled "ms" while its ticks read "0ms, 5s, 10s".
describe('durationAxis', () => {
  const interval = durationAxis('Duration')

  it('reads a short block interval in milliseconds, every tick in the same unit', () => {
    const axis = interval(900, 5)
    expect(axis.name).toBe('Duration (ms)')
    expect(tickLabels(axis)).toEqual(['0', '300', '600', '900'])
    expect(tickLabels(interval(1100, 5))).toEqual(['0', '300', '600', '900', '1200'])
  })

  it('reads a long block interval in seconds, every tick in the same unit', () => {
    const axis = interval(11_000, 5)
    expect(axis.name).toBe('Duration (s)')
    expect(axis.max).toBe(12_000)
    expect(tickLabels(axis)).toEqual(['0', '3', '6', '9', '12'])
  })

  it('keeps the ceiling above the real millisecond peak', () => {
    for (let peak = 1; peak < 600_000; peak *= 1.41) {
      const axis = interval(peak, 4)
      expect(axis.max).toBeGreaterThanOrEqual(peak)
      expect(Math.round(axis.max / axis.interval)).toBeLessThanOrEqual(4)
    }
  })
})

describe('niceAxisScale', () => {
  it('chooses the lowest whole-step ceiling within the band budget', () => {
    expect(niceAxisScale(115.5, 5)).toEqual({ max: 120, interval: 30 })
    expect(niceAxisScale(115.5, 3)).toEqual({ max: 120, interval: 40 })
    expect(niceAxisScale(4, 5, true)).toEqual({ max: 4, interval: 1 })
    expect(niceAxisScale(0.3, 5, true)).toEqual({ max: 1, interval: 1 })
  })
})

describe('sample and current-value formats', () => {
  it('states a retained byte rate at its own precision, and refuses to invent one', () => {
    expect(formatMetricRate(null)).toBe('Unknown')
    expect(formatMetricRate(undefined)).toBe('Unknown')
    expect(formatMetricRate(-1)).toBe('Unknown')
    expect(formatMetricRate(Number.NaN)).toBe('Unknown')
    expect(formatMetricRate(5)).toBe('5.00 B/s')
    expect(formatMetricRate(12.5)).toBe('12.5 B/s')
    expect(formatMetricRate(2048)).toBe('2.00 KiB/s')
    expect(formatMetricRate(1024 * 1024)).toBe('1.00 MiB/s')
    // Below 1024 of its unit a rate stays in that unit, four digits at most…
    expect(formatMetricRate(1023)).toBe('1023 B/s')
    expect(formatMetricRate(1000)).toBe('1000 B/s')
    // …and a value that only rounds up to 1024 moves to the next unit rather
    // than printing "1024 B/s" or "1024 KiB/s".
    expect(formatMetricRate(1023.7)).toBe('1.00 KiB/s')
    expect(formatMetricRate(1024 * 1024 - 1)).toBe('1.00 MiB/s')
    expect(formatMetricRate(1024 ** 4)).toBe('1.00 TiB/s')
  })

  // A percentage, count or duration tooltip states the sample the Server stored:
  // no display rounding, so two different samples never print the same reading.
  it('states a tooltip sample as it was retained, never as a display reading', () => {
    expect(formatPercentSample(12.345)).toBe('12.345%')
    expect(formatPercentSample(12.1)).toBe('12.1%')
    // Three decimals is the display cap, and it is the cap, not the value, that
    // stops there: no displayed percentage or unit conversion happens first.
    expect(formatPercentSample(12.3456)).toBe('12.346%')
    expect(formatPercentSample(0)).toBe('0%')
    expect(formatPercentSample(Number.NaN)).toBe('Unknown%')
    expect(formatCountSample(7)).toBe('7')
    expect(formatCountSample(7.4)).toBe('7')
    expect(formatCountSample(Number.NaN)).toBe('Unknown')
    expect(formatMillisecondsSample(12480)).toBe('12480 ms')
    expect(formatMillisecondsSample(12480.5)).toBe('12480.5 ms')
    expect(formatMillisecondsSample(Number.NaN)).toBe('Unknown ms')
  })

  // Issue #249: a byte-rate tooltip is read against the card's own header and
  // its `Speed (MiB/s)` axis title, so it walks the same binary ladder with the
  // same decimals instead of printing a bare `151618 B/s`.
  it('states a byte-rate tooltip sample in the binary unit the card header reads it in', () => {
    expect(formatRateSample(82366)).toBe('80.4 KiB/s')
    expect(formatRateSample(151618)).toBe('148 KiB/s')
    expect(formatRateSample(1.52 * 1024 * 1024)).toBe('1.52 MiB/s')
    expect(formatRateSample(3 * 1024 ** 3)).toBe('3.00 GiB/s')
    // Below one KiB the sample stays in bytes rather than becoming 0.78 KiB/s.
    expect(formatRateSample(800)).toBe('800 B/s')
    expect(formatRateSample(12.5)).toBe('12.5 B/s')
    expect(formatRateSample(0)).toBe('0.00 B/s')
    expect(formatRateSample(1024)).toBe('1.00 KiB/s')
    expect(formatRateSample(Number.NaN)).toBe('Unknown')
    // The header and the tooltip are one reading of the same sample.
    for (const sample of [0, 800, 1023, 1024, 82366, 151618, 5 * 1024 * 1024]) {
      expect(formatRateSample(sample), String(sample)).toBe(formatMetricRate(sample))
    }
  })
})
