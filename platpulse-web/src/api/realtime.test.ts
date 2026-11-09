import { QueryClient, QueryObserver } from '@tanstack/react-query'
import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  createInvalidationChannel,
  INVALIDATION_COALESCE_MS,
  REALTIME_EXEMPT_META,
  type InvalidationAdapter,
} from './realtime'

type Resource = 'item' | 'other'

const queryClient = new QueryClient({ defaultOptions: { queries: { retry: 0 } } })

function channelWith(inStreamReset: InvalidationAdapter<Resource>['inStreamReset'] = 'refetch') {
  return createInvalidationChannel<Resource>({
    surface: 'public',
    queryClient,
    root: ['t'],
    inStreamReset,
    targets: {
      item: (id) => id ? [{ prefix: ['t', 'item', id], exact: true }] : [{ prefix: ['t', 'item'] }],
      other: () => [{ prefix: ['t', 'other'] }],
    },
  })
}

function built(queryKey: readonly unknown[], meta?: Record<string, unknown>) {
  return queryClient.getQueryCache().build(queryClient, { queryKey, meta })
}

const event = (body: unknown) => JSON.stringify(body)

afterEach(() => {
  vi.useRealTimers()
  vi.restoreAllMocks()
  queryClient.clear()
})

describe('invalidation channel', () => {
  it('drops an event no newer than the last one seen for the same resource', () => {
    vi.useFakeTimers()
    const channel = channelWith()
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries')
    built(['t', 'item', 'a', 1])

    channel.receive(event({ resource: 'item', resourceId: 'a', eventId: 9 }), 1)
    vi.advanceTimersByTime(INVALIDATION_COALESCE_MS)
    channel.receive(event({ resource: 'item', resourceId: 'a', eventId: 8 }), 1)
    // An event without an id falls back to its revision.
    channel.receive(event({ resource: 'item', resourceId: 'a', revision: 9 }), 1)
    vi.advanceTimersByTime(INVALIDATION_COALESCE_MS)
    expect(invalidate).toHaveBeenCalledTimes(1)

    channel.receive(event({ resource: 'item', resourceId: 'a', revision: 10 }), 1)
    vi.advanceTimersByTime(INVALIDATION_COALESCE_MS)
    expect(invalidate).toHaveBeenCalledTimes(2)
  })

  it('reaches only the queries of the given generation, and an exact target only its own key', () => {
    vi.useFakeTimers()
    const channel = channelWith()
    const current = built(['t', 'item', 'a', 4])
    const deeper = built(['t', 'item', 'a', 'detail', 4])
    const older = built(['t', 'item', 'a', 3])
    const sibling = built(['t', 'item', 'b', 4])

    channel.receive(event({ resource: 'item', resourceId: 'a', eventId: 1 }), 4)
    vi.advanceTimersByTime(INVALIDATION_COALESCE_MS)

    expect(current.state.isInvalidated).toBe(true)
    expect(deeper.state.isInvalidated).toBe(false)
    expect(older.state.isInvalidated).toBe(false)
    expect(sibling.state.isInvalidated).toBe(false)
  })

  it('never wakes a query whose meta exempts it from realtime invalidation', () => {
    vi.useFakeTimers()
    const channel = channelWith()
    const live = built(['t', 'item', 'a', 0])
    const clocked = built(['t', 'item', 'a', '7d', 0], REALTIME_EXEMPT_META)

    channel.receive(event({ resource: 'item', eventId: 1 }), 0)
    channel.receive('not json', 0)
    vi.advanceTimersByTime(INVALIDATION_COALESCE_MS)

    expect(live.state.isInvalidated).toBe(true)
    expect(clocked.state.isInvalidated).toBe(false)
  })

  it('coalesces a burst of events into one refetch that never cancels a read', () => {
    vi.useFakeTimers()
    const channel = channelWith()
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries')
    built(['t', 'item', 'a', 0])

    for (let eventId = 1; eventId <= 25; eventId += 1) {
      channel.receive(event({ resource: 'item', resourceId: 'a', eventId }), 0)
    }
    expect(invalidate).not.toHaveBeenCalled()
    vi.advanceTimersByTime(INVALIDATION_COALESCE_MS)

    expect(invalidate).toHaveBeenCalledTimes(1)
    expect(invalidate.mock.calls[0]).toEqual([
      { queryKey: ['t', 'item', 'a', 0], exact: true, refetchType: 'active' },
      { cancelRefetch: false },
    ])
  })

  it('reads again after a read that was already in flight when the event arrived', async () => {
    vi.useFakeTimers()
    const channel = channelWith()
    const answers: Array<(value: string) => void> = []
    const queryFn = vi.fn(() => new Promise<string>((resolve) => answers.push(resolve)))
    const observer = new QueryObserver(queryClient, { queryKey: ['t', 'other', 0], queryFn })
    const unsubscribe = observer.subscribe(() => {})
    expect(queryFn).toHaveBeenCalledTimes(1)

    // The change lands while the first read is still on the wire, so that read
    // may answer from before it: the flush cannot satisfy the event by joining it.
    channel.receive(event({ resource: 'other', eventId: 1 }), 0)
    await vi.advanceTimersByTimeAsync(INVALIDATION_COALESCE_MS)
    expect(queryFn).toHaveBeenCalledTimes(1)
    answers[0]?.('before the change')
    await vi.advanceTimersByTimeAsync(INVALIDATION_COALESCE_MS)

    expect(queryFn).toHaveBeenCalledTimes(2)
    answers[1]?.('after the change')
    await vi.advanceTimersByTimeAsync(INVALIDATION_COALESCE_MS)
    expect(observer.getCurrentResult().data).toBe('after the change')
    // Once a read starts after the event, the event is answered: no third read.
    expect(queryFn).toHaveBeenCalledTimes(2)
    unsubscribe()
  })

  it.each([
    ['not JSON', 'not json'],
    ['not an object', event([1, 2])],
    ['an unknown resource', event({ resource: 'history-window', eventId: 1 })],
    ['a resource that is not a string', event({ resource: 7, eventId: 1 })],
    ['a resource id that is not a string', event({ resource: 'item', resourceId: 7, eventId: 1 })],
  ])('refetches the whole namespace for an event that is %s', (_, data) => {
    vi.useFakeTimers()
    const channel = channelWith('reset')
    const item = built(['t', 'item', 'a', 0])
    const other = built(['t', 'other', 0])

    expect(channel.receive(data, 0)).toBe('handled')
    vi.advanceTimersByTime(INVALIDATION_COALESCE_MS)

    expect(item.state.isInvalidated).toBe(true)
    expect(other.state.isInvalidated).toBe(true)
  })

  it('hands an in-stream reset to the stream when the adapter retires its cache', () => {
    vi.useFakeTimers()
    const channel = channelWith('reset')
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries')
    built(['t', 'other', 0])

    expect(channel.receive(event({ resource: 'collection', reset: true, eventId: 3 }), 0)).toBe('reset')
    vi.advanceTimersByTime(INVALIDATION_COALESCE_MS)

    expect(invalidate).not.toHaveBeenCalled()
  })

  it('refetches the namespace for an in-stream reset and forgets the event ids it saw', () => {
    vi.useFakeTimers()
    const channel = channelWith('refetch')
    built(['t', 'item', 'a', 0])
    channel.receive(event({ resource: 'item', resourceId: 'a', eventId: 9 }), 0)
    vi.advanceTimersByTime(INVALIDATION_COALESCE_MS)
    queryClient.clear()
    const other = built(['t', 'other', 0])

    expect(channel.receive(event({ resource: 'server_shutdown', reset: true, eventId: 10 }), 0)).toBe('handled')
    vi.advanceTimersByTime(INVALIDATION_COALESCE_MS)
    expect(other.state.isInvalidated).toBe(true)

    // A restarted Server counts from 1 again.
    queryClient.clear()
    const item = built(['t', 'item', 'a', 0])
    channel.receive(event({ resource: 'item', resourceId: 'a', eventId: 1 }), 0)
    vi.advanceTimersByTime(INVALIDATION_COALESCE_MS)
    expect(item.state.isInvalidated).toBe(true)
  })

  it('drops queued refetches when it is cleared', () => {
    vi.useFakeTimers()
    const channel = channelWith()
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries')
    built(['t', 'other', 0])

    channel.receive(event({ resource: 'other', eventId: 7 }), 0)
    channel.clear()
    vi.advanceTimersByTime(INVALIDATION_COALESCE_MS)

    expect(invalidate).not.toHaveBeenCalled()
  })

  it('refetches every query of the namespace except an exempt one after a reconnect', async () => {
    const channel = channelWith()
    const item = built(['t', 'item', 'a', 0])
    const clocked = built(['t', 'item', 'a', '7d', 0], REALTIME_EXEMPT_META)
    const foreign = built(['u', 0])

    await channel.refetchAll()

    expect(item.state.isInvalidated).toBe(true)
    expect(clocked.state.isInvalidated).toBe(false)
    expect(foreign.state.isInvalidated).toBe(false)
  })

  it('closes the streams registered with it', () => {
    const channel = channelWith()
    const open = vi.fn()
    const retired = vi.fn()
    channel.registerStream(open)
    const unregister = channel.registerStream(retired)
    unregister()

    channel.closeStreams()

    expect(open).toHaveBeenCalledTimes(1)
    expect(retired).not.toHaveBeenCalled()
  })
})
