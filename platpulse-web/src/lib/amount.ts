/**
 * Exact decimal formatting for Provider-sourced monetary amounts (#155).
 *
 * Validator amounts arrive as bounded decimal strings (the investigated
 * PlatScan serializer truncates LAT values downward to at most 12 decimal
 * places). They must never round-trip through a JavaScript number: binary
 * floating point corrupts large and fractional magnitudes. Every function here
 * is pure string arithmetic on the source digits, so the presentation adds or
 * removes nothing that the source did not provide.
 */

import { NONNEGATIVE_DECIMAL } from './decimal'

type DecimalParts = { integer: string; fractional: string }

function parseDecimalAmount(value: string | null | undefined): DecimalParts | null {
  if (typeof value !== 'string' || !NONNEGATIVE_DECIMAL.test(value)) return null
  const [rawInteger, rawFractional = ''] = value.split('.')
  const integer = (rawInteger === '' ? '0' : rawInteger).replace(/^0+(?=\d)/, '')
  return { integer, fractional: rawFractional }
}

/** Group the integer digits in threes: `1234567` -> `1,234,567`. */
function groupInteger(integer: string): string {
  return integer.replace(/\B(?=(\d{3})+(?!\d))/g, ',')
}

/** Render parsed digits with a grouped integer part and an optional
 *  trailing-zero trim (abbreviated reward cards only). */
function renderDecimal(parts: DecimalParts, trimFraction: boolean): string {
  const integer = groupInteger(parts.integer)
  const fractional = trimFraction ? parts.fractional.replace(/0+$/, '') : parts.fractional
  return fractional === '' ? integer : `${integer}.${fractional}`
}

/**
 * Full-precision presentation: the source's exact digits with a grouped integer
 * part. Used for cumulative block counts everywhere and for reward amounts in
 * detail views. Trailing fractional digits are preserved because the source
 * provided them.
 */
export function formatAmountExact(value: string | null | undefined): string {
  const parts = parseDecimalAmount(value)
  return parts ? renderDecimal(parts, false) : 'Unknown'
}

/** Add already-deduplicated Network subtotals without losing decimal digits.
 * Missing values contribute nothing; an entirely unknown selection stays null.
 * This is a numerical cross-Network overview, not currency conversion. */
export function sumKnownAmounts(values: ReadonlyArray<string | null | undefined>): string | null {
  const parts = values.map(parseDecimalAmount).filter((part): part is DecimalParts => part !== null)
  if (parts.length === 0) return null
  const scale = Math.max(...parts.map(part => part.fractional.length))
  const sum = parts.reduce((total, part) => total + BigInt(part.integer + part.fractional.padEnd(scale, '0')), 0n)
  const digits = sum.toString().padStart(scale + 1, '0')
  return scale === 0 ? digits : digits.slice(0, -scale) + '.' + digits.slice(-scale)
}

const COMPACT_UNITS: ReadonlyArray<{ minDigits: number; digitShift: number; suffix: string }> = [
  { minDigits: 13, digitShift: 12, suffix: 'T' },
  { minDigits: 10, digitShift: 9, suffix: 'B' },
  { minDigits: 7, digitShift: 6, suffix: 'M' },
  { minDigits: 4, digitShift: 3, suffix: 'K' },
]

/**
 * Abbreviated presentation kept for Home reward cards, where a tile is narrow
 * and the exact amount stays available in the accessible Breakdown. Values
 * below one thousand and any nonzero value below one stay readable without a
 * unit suffix; larger values scale by thousands using the source digit string,
 * truncating (never rounding up) to two fractional digits. A missing or
 * malformed value is Unknown — never a fabricated zero.
 */
export function formatAmountCompact(value: string | null | undefined): string {
  const parts = parseDecimalAmount(value)
  if (!parts) return 'Unknown'
  const { integer, fractional } = parts
  const unit = COMPACT_UNITS.find((candidate) => integer.length >= candidate.minDigits)
  if (!unit) return renderDecimal(parts, true)
  const digits = integer + fractional
  const integerDigits = digits.slice(0, integer.length - unit.digitShift)
  const fractionalDigits = digits.slice(integer.length - unit.digitShift, integer.length - unit.digitShift + 2).replace(/0+$/, '')
  const mantissa = fractionalDigits === '' ? integerDigits : `${integerDigits}.${fractionalDigits}`
  return `${mantissa}${unit.suffix}`
}

/** Reward tiles have bounded width. Reuse compact formatting, falling back to
 *  four significant source digits in scientific notation only when needed. Like
 *  compact K/M/B/T this truncates rather than rounding up; the Breakdown always
 *  exposes the exact digits. */
export function formatAmountOverview(value: string | null | undefined): string {
  const compact = formatAmountCompact(value)
  if (compact.length <= 10) return compact
  const parts = parseDecimalAmount(value)
  if (!parts) return 'Unknown'
  const significant = (parts.integer === '0' ? parts.fractional : parts.integer + parts.fractional).replace(/^0+/, '')
  if (!significant) return '0'
  const exponent = parts.integer === '0' ? significant.length - parts.fractional.length - 1 : parts.integer.length - 1
  const fraction = significant.slice(1, 4).replace(/0+$/, '')
  return significant[0] + (fraction ? '.' + fraction : '') + 'e' + (exponent >= 0 ? '+' : '') + exponent
}
