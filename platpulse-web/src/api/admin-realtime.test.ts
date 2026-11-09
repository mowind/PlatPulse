import { afterEach, describe, expect, it, vi } from 'vitest'
import { adminQueryClient, adminRealtime, resetAdminCache } from './admin'
import { INVALIDATION_COALESCE_MS } from './realtime'

let generation = 1000

const event = (body: unknown) => JSON.stringify(body)
const built = (queryKey: readonly unknown[]) =>
  adminQueryClient.getQueryCache().build(adminQueryClient, { queryKey: [...queryKey, generation] })

afterEach(() => {
  vi.useRealTimers()
  // A reset clears the realtime channel, so a queued refetch never leaks into
  // a later test.
  generation += 1
  resetAdminCache(generation)
})

describe('Admin realtime adapter', () => {
  it('refetches an addressed Node with its own evidence, and nothing it does not reach', () => {
    vi.useFakeTimers()
    const reached = [
      built(['admin', 'overview']),
      built(['admin', 'nodes', 'node-1']),
      built(['admin', 'nodes', 'node-1', 'state-history', 'sync', 'a', 'b']),
      built(['admin', 'nodes', 'node-1', 'host-metric-history', 'cpu', 'all', 'a', 'b']),
    ]
    const untouched = [built(['admin', 'agents', 'agent-1']), built(['admin', 'people'])]

    adminRealtime.receive(event({ resource: 'node', resourceId: 'node-1', eventId: 1 }), generation)
    vi.advanceTimersByTime(INVALIDATION_COALESCE_MS)

    expect(reached.map((query) => query.state.isInvalidated)).toEqual([true, true, true, true])
    expect(untouched.map((query) => query.state.isInvalidated)).toEqual([false, false])
  })

  it('refetches the whole namespace for a reset on the invalidation stream, without an access reset', () => {
    vi.useFakeTimers()
    const people = built(['admin', 'people'])
    const overview = built(['admin', 'overview'])

    expect(adminRealtime.receive(event({ resource: 'collection', reset: true, eventId: 2 }), generation)).toBe('handled')
    vi.advanceTimersByTime(INVALIDATION_COALESCE_MS)

    expect(people.state.isInvalidated).toBe(true)
    expect(overview.state.isInvalidated).toBe(true)
  })

  it('drops queued refetches when the namespace is reset', () => {
    vi.useFakeTimers()
    const invalidate = vi.spyOn(adminQueryClient, 'invalidateQueries')
    built(['admin', 'operations'])

    adminRealtime.receive(event({ resource: 'operations', eventId: 3 }), generation)
    resetAdminCache(generation + 1)
    vi.advanceTimersByTime(INVALIDATION_COALESCE_MS)

    expect(invalidate).not.toHaveBeenCalled()
    invalidate.mockRestore()
  })
})
