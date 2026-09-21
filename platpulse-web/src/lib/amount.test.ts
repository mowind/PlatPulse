import { describe, expect, it } from 'vitest'
import { formatAmountCompact, formatAmountExact, formatAmountOverview, sumKnownAmounts } from './amount'

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

/**
 * Home prints cumulative block counts complete and keeps the abbreviated reward
 * form on the tile. The exact formatter must therefore stay lossless for block
 * counts, while the compact form is explicitly a display-only abbreviation of
 * the same source digits.
 */
describe('formatAmountExact on overview-scale values', () => {
  it('renders a complete grouped block count instead of an abbreviation', () => {
    expect(formatAmountExact('1730000')).toBe('1,730,000')
    expect(formatAmountExact('201186')).toBe('201,186')
    expect(formatAmountExact('999999999999999')).toBe('999,999,999,999,999')
  })

  it('keeps a long source fraction in full', () => {
    expect(formatAmountExact('1730000.123456789012')).toBe('1,730,000.123456789012')
  })

  it('carries a huge value without a JavaScript number conversion', () => {
    expect(formatAmountExact('18446744073709551614')).toBe('18,446,744,073,709,551,614')
    expect(formatAmountExact('123456789012345678901234567890')).toBe('123,456,789,012,345,678,901,234,567,890')
  })
})

describe('formatAmountCompact (reward tiles)', () => {
  it('abbreviates large reward magnitudes without binary floating point', () => {
    expect(formatAmountCompact('1234567')).toBe('1.23M')
    expect(formatAmountCompact('1730000.5')).toBe('1.73M')
    expect(formatAmountCompact('1500000')).toBe('1.5M')
    expect(formatAmountCompact('296110')).toBe('296.11K')
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

describe('formatAmountOverview (bounded reward tile)', () => {
  it('reuses the compact form while it fits the tile', () => {
    expect(formatAmountOverview('1234567')).toBe('1.23M')
    expect(formatAmountOverview('201186')).toBe('201.18K')
    expect(formatAmountOverview('999')).toBe('999')
  })

  it('bounds an extreme reward value without inventing precision', () => {
    expect(formatAmountOverview('18446744073709551614')).toBe('1.844e+19')
    expect(formatAmountOverview('123456789012345678901234567890')).toBe('1.234e+29')
    expect(formatAmountOverview('0.123456789012')).toBe('1.234e-1')
  })

  it('never turns a nonzero or missing value into zero or Unknown', () => {
    expect(formatAmountOverview('0.000000000001')).toBe('1e-12')
    expect(formatAmountOverview(null)).toBe('Unknown')
  })
})

/**
 * The explicitly approved Cross-Network Validator Overview adds the Server's
 * already-deduplicated Network subtotals. It is pure string arithmetic: no
 * Node projections, no floating point, and no fabricated zero for a Network
 * whose value is unknown.
 */
describe('sumKnownAmounts', () => {
  it('adds known values exactly and ignores unknown contributions', () => {
    expect(sumKnownAmounts(['1108', '7'])).toBe('1115')
    expect(sumKnownAmounts(['11.750000000001', '2'])).toBe('13.750000000001')
    expect(sumKnownAmounts(['9007199254740993', '1'])).toBe('9007199254740994')
    expect(sumKnownAmounts(['18446744073709551614', '18446744073709551614'])).toBe('36893488147419103228')
  })

  it('aligns differing fractional scales instead of truncating digits', () => {
    expect(sumKnownAmounts(['999.999', '0.001'])).toBe('1000.000')
    expect(sumKnownAmounts(['0.1', '0.02', '0.003'])).toBe('0.123')
  })

  it('keeps a real zero and reports all-unknown as unknown, never zero', () => {
    expect(sumKnownAmounts(['0.000', '0'])).toBe('0.000')
    expect(sumKnownAmounts([])).toBeNull()
    expect(sumKnownAmounts([null, undefined])).toBeNull()
    expect(sumKnownAmounts(['0', null])).toBe('0')
  })
})
