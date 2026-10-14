/**
 * Node data percentage shared by the Home card, the Node Detail hero and the
 * Home Disk sort (issues #140 and #242). Only two measured byte counts have a
 * valid percentage: an absent, negative, non-finite, or non-positive capacity
 * yields none, so every reader publishes Unknown instead of a number it cannot
 * stand behind, and the card's column and the order that ranks it can never
 * disagree about which Nodes have a ratio at all.
 */
export function nodeDataProgress(size: number | null | undefined, capacity: number | null | undefined): number | null {
  if (!isMeasuredBytes(size) || !isMeasuredBytes(capacity) || capacity <= 0) return null
  return (size / capacity) * 100
}

/** A byte count the Server can stand behind: a finite, non-negative number. */
function isMeasuredBytes(value: number | null | undefined): value is number {
  return value != null && Number.isFinite(value) && value >= 0
}
