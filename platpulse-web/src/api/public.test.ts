import { afterEach, describe, expect, it, vi } from 'vitest'
import { adminQueryClient } from './admin'
import { client } from './generated/client.gen'
import {
  fetchNetworks,
  fetchNodeMetrics,
  fetchRewardWallet,
  publicKeys,
  publicQueryClient,
  publicRealtime,
  resetPublicCache,
} from './public'
import { INVALIDATION_COALESCE_MS } from './realtime'

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
  // A reset clears the realtime channel, so a queued refetch never leaks into
  // the next test.
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

  it('asks for a longer Metric Window by its preset and for Live with no parameter', async () => {
    client.setConfig({ baseUrl: 'http://platpulse.test' })
    const fetchMock = vi.fn((input: RequestInfo | URL) => {
      void input
      return Promise.resolve(response({}))
    })
    vi.stubGlobal('fetch', fetchMock)
    await fetchNodeMetrics('node-1', undefined, 0)
    await fetchNodeMetrics('node-1', undefined, 0, '24h')
    const urls = fetchMock.mock.calls.map(([request]) => (request as unknown as Request).url)
    expect(urls).toEqual([
      'http://platpulse.test/api/public/v1/nodes/node-1/metrics',
      'http://platpulse.test/api/public/v1/nodes/node-1/metrics?window=24h',
    ])
  })

  it('reads the Reward Wallet Balance from its own Public route', async () => {
    client.setConfig({ baseUrl: 'http://platpulse.test' })
    const fetchMock = vi.fn((input: RequestInfo | URL) => {
      void input
      return Promise.resolve(response({ address: 'lat1', balance: '1', unit: 'LAT', state: 'current' }))
    })
    vi.stubGlobal('fetch', fetchMock)

    const wallet = await fetchRewardWallet(undefined, 5)

    const request = fetchMock.mock.calls[0]?.[0] as unknown as Request
    expect(request.url).toContain('/api/public/v1/reward-wallet')
    expect(request.headers.get('X-PlatPulse-Access-Generation')).toBe('5')
    expect(wallet.balance).toBe('1')
  })

})

describe('Public realtime adapter', () => {
  const event = (body: unknown) => JSON.stringify(body)
  const built = (queryKey: readonly unknown[]) =>
    publicQueryClient.getQueryCache().build(publicQueryClient, { queryKey })

  it('refetches the detail, history and Live metrics of an addressed Node', () => {
    vi.useFakeTimers()
    const reached = [
      built([...publicKeys.node('node-1'), 0]),
      built([...publicKeys.history('node-1'), 0]),
      built([...publicKeys.metrics('node-1'), 0]),
    ]
    const untouched = [
      built([...publicKeys.networks, 0]),
      built([...publicKeys.node('node-2'), 0]),
      built([...publicKeys.rewardWallet, 0]),
    ]

    publicRealtime.receive(event({ resource: 'node', resourceId: 'node-1', eventId: 1 }), 0)
    vi.advanceTimersByTime(INVALIDATION_COALESCE_MS)

    expect(reached.map((query) => query.state.isInvalidated)).toEqual([true, true, true])
    expect(untouched.map((query) => query.state.isInvalidated)).toEqual([false, false, false])
  })

  it.each([
    ['network', publicKeys.networks],
    ['reward_wallet', publicKeys.rewardWallet],
  ] as const)('refetches only its own list for a %s event', (resource, key) => {
    vi.useFakeTimers()
    const own = built([...key, 0])
    const node = built([...publicKeys.node('node-1'), 0])

    publicRealtime.receive(event({ resource, eventId: 1 }), 0)
    vi.advanceTimersByTime(INVALIDATION_COALESCE_MS)

    expect(own.state.isInvalidated).toBe(true)
    expect(node.state.isInvalidated).toBe(false)
  })

  it('retires the Public cache for a reset on the invalidation stream', () => {
    expect(publicRealtime.receive(event({ resource: 'site_access_mode', reset: true, eventId: 4 }), 0)).toBe('reset')
  })

  it('drops queued refetches when the namespace is reset', () => {
    vi.useFakeTimers()
    const invalidate = vi.spyOn(publicQueryClient, 'invalidateQueries')
    built([...publicKeys.networks, 0])

    publicRealtime.receive(event({ resource: 'network', eventId: 7 }), 0)
    resetPublicCache(8)
    vi.advanceTimersByTime(INVALIDATION_COALESCE_MS)

    expect(invalidate).not.toHaveBeenCalled()
  })
})
