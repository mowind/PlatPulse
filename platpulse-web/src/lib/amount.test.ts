import { describe, expect, it } from 'vitest'
import { formatAmountCompact, formatAmountExact } from './amount'

/**
 * Issue #155: Provider monetary amounts arrive as bounded decimal strings
 * (PlatScan truncates LAT values to at most 12 decimal places). Formatting is
 * pure string arithmetic: a value is never parsed into binary floating point,
 * so large and fractional magnitudes keep their exact digits.
 */
describe('formatAmountExact', () => {
  it('preserves every source digit while grouping the integer part', () => {
    expect(formatAmountExact('1234567.890123456789')).toBe('1,234,567.890123456789')
    expect(formatAmountExact('1234.123456789012')).toBe('1,234.123456789012')
    expect(formatAmountExact('1000000000000')).toBe('1,000,000,000,000')
  })

  it('keeps a source-reported zero and small fractions distinct from unknown', () => {
    expect(formatAmountExact('0')).toBe('0')
    expect(formatAmountExact('0.000000000001')).toBe('0.000000000001')
    expect(formatAmountExact(null)).toBe('Unknown')
    expect(formatAmountExact(undefined)).toBe('Unknown')
    expect(formatAmountExact('')).toBe('Unknown')
  })

  it('rejects a malformed value instead of inventing a number', () => {
    expect(formatAmountExact('12abc')).toBe('Unknown')
    expect(formatAmountExact('-5')).toBe('Unknown')
    expect(formatAmountExact('1.2.3')).toBe('Unknown')
  })
})

describe('formatAmountCompact', () => {
  it('abbreviates large magnitudes without binary floating point', () => {
    expect(formatAmountCompact('1234567')).toBe('1.23M')
    expect(formatAmountCompact('1500000')).toBe('1.5M')
    expect(formatAmountCompact('1000')).toBe('1K')
    expect(formatAmountCompact('999999999999999')).toBe('999.99T')
  })

  it('keeps small values and nonzero fractions exact', () => {
    expect(formatAmountCompact('999')).toBe('999')
    expect(formatAmountCompact('0.5')).toBe('0.5')
    expect(formatAmountCompact('0.000000000001')).toBe('0.000000000001')
    expect(formatAmountCompact('1.000000000000')).toBe('1')
  })

  it('never presents a missing value as zero', () => {
    expect(formatAmountCompact(null)).toBe('Unknown')
    expect(formatAmountCompact('n/a')).toBe('Unknown')
  })
})
