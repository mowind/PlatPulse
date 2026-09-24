/**
 * Full-precision amount rendering.
 *
 * Used where a value must be shown complete — cumulative block counts, exact
 * amounts in a Breakdown, full-precision detail. A long exact decimal must
 * still fit, so this inserts a soft break opportunity after every thousands
 * separator and after the decimal point: the digits are never clipped or split
 * mid-digit, and the value wraps only at those boundaries. `<wbr>` adds no
 * text, so assistive technology reads the exact number.
 */
export function ExactAmount({ value, className, split = false, muteFraction = false }: {
  value: string
  className?: string
  /** Split even a short value. Overview tiles use this so a medium number
   *  wraps at its group separators instead of mid-digit; a compact metric cell
   *  keeps a short value as one text node. */
  split?: boolean
  /** De-emphasise the fractional digits so the magnitude reads first. The
   *  digits stay in the DOM in full and in order, so the accessible name and
   *  the exact string are unchanged; only the colour differs. */
  muteFraction?: boolean
}) {
  const chunks = (part: string) => (part.match(/[^,.]+[.,]?/g) ?? [part]).map((chunk, index) => (
    <span key={index}>
      {chunk}
      <wbr />
    </span>
  ))
  if (!split && !muteFraction && value.length <= 15) return <span className={className}>{value}</span>
  const separator = muteFraction ? value.lastIndexOf('.') : -1
  // Only a real fractional part is muted: an integer amount renders whole.
  if (separator > 0 && separator < value.length - 1) {
    return (
      <span className={className}>
        {chunks(value.slice(0, separator))}
        {/* The separator is emitted verbatim: the chunker matches runs of
            non-separator characters, so feeding it a leading separator would
            silently drop it. */}
        <span className="text-muted-foreground">{value[separator]}{chunks(value.slice(separator + 1))}</span>
      </span>
    )
  }
  return <span className={className}>{chunks(value)}</span>
}
