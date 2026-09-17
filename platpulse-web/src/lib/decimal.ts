/**
 * Shared nonnegative-decimal recognition for exact string formatting.
 *
 * Amounts (#155) and Validator rates (#156) both arrive as bounded decimal
 * strings. Neither may round-trip through a JavaScript number, so both use
 * this one predicate rather than duplicating the pattern.
 */

/** A nonnegative decimal string: `12`, `12.`, `.5`, or `12.5`. */
export const NONNEGATIVE_DECIMAL = /^(?:\d+(?:\.\d*)?|\.\d+)$/

/** Narrow an unknown value to a nonnegative decimal string. */
export function isNonnegativeDecimal(value: string | null | undefined): value is string {
  return typeof value === 'string' && NONNEGATIVE_DECIMAL.test(value)
}
