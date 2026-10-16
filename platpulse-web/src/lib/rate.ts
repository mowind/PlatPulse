import { isNonnegativeDecimal } from './decimal'

/**
 * Percentage formatting for the two Validator production rates (#156).
 *
 * Both rates cross the Public API as percentage strings without the `%` sign
 * (for example `90.909091`, `0`, or `100`). Home cards default to two
 * decimals; Node detail shows exactly what the Server computed or PlatScan
 * reported. The rounding is pure string arithmetic on the source digits, so a
 * rate never round-trips through a binary float. A missing or malformed value
 * is Unknown, never a fabricated zero.
 */

/** Present the value with the source's own digits, without a trailing dot. */
function normalizedRate(value: string): string {
  const trimmed = value.endsWith('.') ? value.slice(0, -1) : value
  return trimmed.startsWith('.') ? `0${trimmed}` : trimmed
}

/** Round a percentage string half-up to two decimals and append `%`. */
export function formatRatePercent2(value: string | null | undefined): string {
  if (!isNonnegativeDecimal(value)) return 'Unknown'
  const [rawInteger, rawFractional = ''] = value.split('.')
  const integer = rawInteger === '' ? '0' : rawInteger
  const tenths = rawFractional[0] ?? '0'
  const hundredths = rawFractional[1] ?? '0'
  const thousandths = rawFractional[2] ?? '0'
  let kept = Number(`${tenths}${hundredths}`)
  let whole = BigInt(integer)
  if (Number(thousandths) >= 5) kept += 1
  if (kept >= 100) {
    kept -= 100
    whole += 1n
  }
  return `${whole}.${String(kept).padStart(2, '0')}%`
}

/**
 * Node detail's display form: at most two decimals, rounded half-up on the
 * source digits, without meaningless trailing zeros (`100.047221` → `100.05%`,
 * `75.50` → `75.5%`, `20` → `20%`). A value above 100 stays above 100. A
 * non-zero rate too small for two decimals reads `<0.01%`, so it can never be
 * mistaken for a real zero; only a source zero reads `0%`.
 */
export function formatRatePercentCompact(value: string | null | undefined): string {
  const rounded = formatRatePercent2(value)
  if (rounded === 'Unknown') return rounded
  const trimmed = rounded.slice(0, -1).replace(/0+$/, '').replace(/\.$/, '')
  if (trimmed === '0' && /[1-9]/.test(value ?? '')) return '<0.01%'
  return trimmed + '%'
}

/** The rate exactly as provided, with a single `%` and no added precision. */
export function formatRatePercentExact(value: string | null | undefined): string {
  if (!isNonnegativeDecimal(value)) return 'Unknown'
  return `${normalizedRate(value)}%`
}
