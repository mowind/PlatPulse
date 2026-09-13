/**
 * Node data percentage shared by the Home card and the Node Detail hero
 * (issue #140). An absent or non-positive capacity has no valid percentage.
 */
export function nodeDataProgress(size: number | null | undefined, capacity: number | null | undefined): number | null {
  if (size == null || capacity == null || capacity <= 0) return null
  return (size / capacity) * 100
}
