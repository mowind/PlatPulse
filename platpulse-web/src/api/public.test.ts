import { afterEach, describe, expect, it, vi } from 'vitest'
import { adminQueryClient } from './admin'
import { client } from './generated/client.gen'
import {
  applySiteAccessSettings,
  fetchNetworks,
  invalidatePublicResource,
  PUBLIC_INVALIDATION_COALESCE_MS,
  publicKeys,
  publicQueryClient,
  resetPublicCache,
} from './public'

function response(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { 'Content-Type': 'application/json' },
  })
}

afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
  // Clear module-level coalescing state so a pending timer never leaks into
  // the next test (resetPublicCache also cancels it).
  resetPublicCache(0)
  publicQueryClient.clear()
  adminQueryClient.clear()
})

describe('Public adapter and query namespace', () => {
  it('uses a cache that cannot satisfy an Admin query', () => {
    expect(publicQueryClient).not.toBe(adminQueryClient)
    expect(publicKeys.networks[0]).toBe('public')
    expect(publicKeys.node('node-1')[0]).toBe('public')
  })

  it('uses the generated client and carries the access generation header', async () => {
    client.setConfig({ baseUrl: 'http://platpulse.test' })
    const fetchMock = vi.fn((input: RequestInfo | URL) => {
      void input
      return Promise.resolve(response([]))
    })
    vi.stubGlobal('fetch', fetchMock)

    await fetchNetworks(undefined, 17)

    const request = fetchMock.mock.calls[0]?.[0] as unknown as Request
    expect(request).toBeInstanceOf(Request)
    expect((request as Request).url).toContain('/api/public/v1/networks')
    expect((request as Request).headers.get('X-PlatPulse-Access-Generation')).toBe('17')
  })

  it('ignores an older SSE revision for the same resource', async () => {
    vi.useFakeTimers()
    const invalidate = vi.spyOn(publicQueryClient, 'invalidateQueries')

    invalidatePublicResource('node', 'node-1', 9)
    invalidatePublicResource('node', 'node-1', 8)
    vi.advanceTimersByTime(PUBLIC_INVALIDATION_COALESCE_MS)

    expect(invalidate).toHaveBeenCalledTimes(4)
    expect(invalidate.mock.calls.map(([options]) => options)).toEqual([
      { queryKey: [...publicKeys.node('node-1'), 0], exact: true, refetchType: 'active' },
      { queryKey: [...publicKeys.history('node-1'), 0], exact: true, refetchType: 'active' },
      { queryKey: [...publicKeys.metrics('node-1'), 0], exact: true, refetchType: 'active' },
      { queryKey: [...publicKeys.peerHistory('node-1'), 0], exact: true, refetchType: 'active' },
    ])
  })

  it('invalidates the Network list for an addressed Network resource', () => {
    vi.useFakeTimers()
    const invalidate = vi.spyOn(publicQueryClient, 'invalidateQueries')

    invalidatePublicResource('network', 'network-a', 11)
    vi.advanceTimersByTime(PUBLIC_INVALIDATION_COALESCE_MS)

    expect(invalidate).toHaveBeenCalledTimes(1)
    expect(invalidate.mock.calls.map(([options]) => options)).toEqual([
      { queryKey: [...publicKeys.networks, 0], exact: true, refetchType: 'active' },
    ])
  })

  it('coalesces a burst of invalidations into one non-cancelling refetch', () => {
    vi.useFakeTimers()
    const invalidate = vi.spyOn(publicQueryClient, 'invalidateQueries')

    for (let eventId = 1; eventId <= 25; eventId += 1) {
      invalidatePublicResource('network', 'network-a', eventId)
    }

    // Nothing is fetched while the burst is still arriving: the in-flight
    // REST read from the previous flush is never cancelled by the next event.
    expect(invalidate).not.toHaveBeenCalled()
    vi.advanceTimersByTime(PUBLIC_INVALIDATION_COALESCE_MS)
    expect(invalidate).toHaveBeenCalledTimes(1)
    expect(invalidate.mock.calls[0]?.[0]).toEqual({
      queryKey: [...publicKeys.networks, 0],
      exact: true,
      refetchType: 'active',
    })
    expect(invalidate.mock.calls[0]?.[1]).toEqual({ cancelRefetch: false })
  })

  it('drops queued invalidations when the namespace is reset', () => {
    vi.useFakeTimers()
    const invalidate = vi.spyOn(publicQueryClient, 'invalidateQueries')

    invalidatePublicResource('network', 'network-a', 7)
    resetPublicCache(8)
    vi.advanceTimersByTime(PUBLIC_INVALIDATION_COALESCE_MS)

    expect(invalidate).not.toHaveBeenCalled()
  })

  it('targets the authoritative Site Access generation after a reset', () => {
    vi.useFakeTimers()
    applySiteAccessSettings({ mode: 'public', authorizationGeneration: 42 })
    resetPublicCache(99)
    applySiteAccessSettings({ mode: 'public', authorizationGeneration: 42 })
    const invalidate = vi.spyOn(publicQueryClient, 'invalidateQueries')

    invalidatePublicResource('network', 'network-a', 12)
    vi.advanceTimersByTime(PUBLIC_INVALIDATION_COALESCE_MS)

    expect(invalidate.mock.calls[0]?.[0]).toMatchObject({
      queryKey: [...publicKeys.networks, 42],
      exact: true,
    })
  })
})
