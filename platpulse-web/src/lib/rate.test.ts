import { describe, expect, it } from 'vitest'
import { formatRatePercent2, formatRatePercentExact } from './rate'

/**
 * Issue #156: both Validator production rates cross the Public API as
 * percentage strings without the `%` sign. Cards default to two decimals;
 * detail preserves what the Server computed or PlatScan reported. Formatting is
 * string arithmetic, so a rate never round-trips through a binary float.
 */
describe('formatRatePercent2', () => {
  it('rounds the source value half-up to two decimals', () => {
    expect(formatRatePercent2('90.909091')).toBe('90.91%')
    expect(formatRatePercent2('66.666667')).toBe('66.67%')
    expect(formatRatePercent2('12.5')).toBe('12.50%')
    expect(formatRatePercent2('100')).toBe('100.00%')
  })

  it('carries into the integer part instead of emitting 100 hundredths', () => {
    expect(formatRatePercent2('9.999')).toBe('10.00%')
    expect(formatRatePercent2('99.999')).toBe('100.00%')
  })

  it('keeps a source-reported zero distinct from unknown', () => {
    expect(formatRatePercent2('0')).toBe('0.00%')
    expect(formatRatePercent2(null)).toBe('Unknown')
    expect(formatRatePercent2(undefined)).toBe('Unknown')
    expect(formatRatePercent2('')).toBe('Unknown')
  })

  it('rejects a malformed value instead of inventing a number', () => {
    expect(formatRatePercent2('12abc')).toBe('Unknown')
    expect(formatRatePercent2('-5')).toBe('Unknown')
    expect(formatRatePercent2('1.2.3')).toBe('Unknown')
  })
})

describe('formatRatePercentExact', () => {
  it('preserves every source digit with a single percent sign', () => {
    expect(formatRatePercentExact('90.909091')).toBe('90.909091%')
    expect(formatRatePercentExact('75.5')).toBe('75.5%')
    expect(formatRatePercentExact('0')).toBe('0%')
  })

  it('normalizes a source trailing or leading dot instead of emitting a stray one', () => {
    expect(formatRatePercentExact('12.')).toBe('12%')
    expect(formatRatePercentExact('.5')).toBe('0.5%')
    expect(formatRatePercent2('12.')).toBe('12.00%')
  })

  it('never fabricates precision for a missing or malformed value', () => {
    expect(formatRatePercentExact(null)).toBe('Unknown')
    expect(formatRatePercentExact(undefined)).toBe('Unknown')
    expect(formatRatePercentExact('12%')).toBe('Unknown')
    expect(formatRatePercentExact('abc')).toBe('Unknown')
  })
})
