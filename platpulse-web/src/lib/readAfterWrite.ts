import type { QueryClient, QueryFunctionContext, QueryKey } from '@tanstack/react-query'
import type { DetailReadResult } from './readBack'

/** Reads an entity's authoritative detail after a write has been answered.
 *
 *  A read that began before the write cannot stand for its read-back: it may
 *  carry the value from before the write while finishing later. Cancel every
 *  in-flight read of this exact query first, then fetch it again and report
 *  that read's own state. The caller never sends the write again. */
export async function readAfterWrite<T>(
  client: QueryClient,
  queryKey: QueryKey,
  queryFn: (context: QueryFunctionContext) => Promise<T>,
): Promise<DetailReadResult> {
  await client.cancelQueries({ queryKey, exact: true })
  try {
    await client.fetchQuery({ queryKey, queryFn, staleTime: 0 })
    const state = client.getQueryState(queryKey)
    return {
      isError: false,
      errorUpdateCount: state?.errorUpdateCount ?? 0,
      dataUpdatedAt: state?.dataUpdatedAt ?? 0,
      errorUpdatedAt: state?.errorUpdatedAt ?? 0,
    }
  } catch {
    const state = client.getQueryState(queryKey)
    return {
      isError: true,
      errorUpdateCount: state?.errorUpdateCount ?? 0,
      dataUpdatedAt: state?.dataUpdatedAt ?? 0,
      errorUpdatedAt: state?.errorUpdatedAt ?? Date.now(),
    }
  }
}
