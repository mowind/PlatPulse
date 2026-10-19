/** The latest read of an entity's authoritative detail, as its page's query
 *  holds it. */
export type DetailRead = {
  isError: boolean
  errorUpdateCount: number
  dataUpdatedAt: number
}

/** What one read of the detail returns (a TanStack Query refetch result). */
export type DetailReadResult = DetailRead & { errorUpdatedAt: number }

/** A read-back that failed after a write had already succeeded. */
export type ReadBackFailure = {
  errorUpdateCount: number
  errorUpdatedAt: number
}

/** Whether a failed read-back has since been followed by a successful read of
 *  the same detail query. The detail the page held before the write, or a
 *  render whose props have not caught up with the failure yet, is never taken
 *  for that read: a later read must have succeeded after the failure itself
 *  (the query no longer in error, and its error counter or data time past the
 *  failure). */
export function readBackRecovered(failure: ReadBackFailure, detail: DetailRead): boolean {
  if (detail.isError) return false
  return (
    detail.errorUpdateCount >= failure.errorUpdateCount ||
    detail.dataUpdatedAt > failure.errorUpdatedAt
  )
}
