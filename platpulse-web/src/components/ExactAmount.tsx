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
export function ExactAmount({ value, className, split = false }: {
  value: string
  className?: string
  /** Split even a short value. Overview tiles use this so a medium number
   *  wraps at its group separators instead of mid-digit; a compact metric cell
   *  keeps a short value as one text node. */
  split?: boolean
}) {
  if (!split && value.length <= 15) return <span className={className}>{value}</span>
  const chunks = value.match(/[^,.]+[.,]?/g) ?? [value]
  return (
    <span className={className}>
      {chunks.map((chunk, index) => (
        <span key={index}>
          {chunk}
          <wbr />
        </span>
      ))}
    </span>
  )
}
