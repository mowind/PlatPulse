import { QueryObserver } from '@tanstack/react-query'
import { afterEach, expect, it, vi } from 'vitest'
import { ADMIN_INVALIDATION_COALESCE_MS, adminQueryClient, handleAdminInvalidation, resetAdminCache } from './admin'

let generation = 1000

afterEach(() => {
  vi.useRealTimers()
  // A reset drops any queued invalidation so it never leaks into a later test.
  generation += 1
  resetAdminCache(generation)
})

it('reads again after a read that was already in flight when the event arrived', async () => {
  vi.useFakeTimers()
  const answers: Array<(value: string) => void> = []
  const queryFn = vi.fn(() => new Promise<string>((resolve) => answers.push(resolve)))
  const observer = new QueryObserver(adminQueryClient, { queryKey: ['admin', 'overview', generation], queryFn })
  const unsubscribe = observer.subscribe(() => {})
  expect(queryFn).toHaveBeenCalledTimes(1)

  // The change lands while the first read is still on the wire, so that read
  // may answer from before it: the flush cannot satisfy the event by joining it.
  handleAdminInvalidation(JSON.stringify({ eventId: 1, resource: 'operations' }), generation)
  await vi.advanceTimersByTimeAsync(ADMIN_INVALIDATION_COALESCE_MS)
  expect(queryFn).toHaveBeenCalledTimes(1)
  answers[0]?.('before the change')
  await vi.advanceTimersByTimeAsync(ADMIN_INVALIDATION_COALESCE_MS)

  expect(queryFn).toHaveBeenCalledTimes(2)
  answers[1]?.('after the change')
  await vi.advanceTimersByTimeAsync(ADMIN_INVALIDATION_COALESCE_MS)
  expect(observer.getCurrentResult().data).toBe('after the change')
  // Once a read starts after the event, the event is answered: no third read.
  expect(queryFn).toHaveBeenCalledTimes(2)
  unsubscribe()
})
