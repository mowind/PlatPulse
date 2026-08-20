import { afterEach, describe, expect, it, vi } from 'vitest'
import { adminQueryClient } from './admin'
import { client } from './generated/client.gen'
import {
  fetchNetworks,
  invalidatePublicResource,
  publicKeys,
  publicQueryClient,
} from './public'

function response(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { 'Content-Type': 'application/json' },
  })
}

afterEach(() => {
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
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
    const invalidate = vi.spyOn(publicQueryClient, 'invalidateQueries')

    invalidatePublicResource('node', 'node-1', 9)
    invalidatePublicResource('node', 'node-1', 8)

    expect(invalidate).toHaveBeenCalledTimes(3)
    expect(invalidate.mock.calls[0]?.[0]).toEqual({
      queryKey: [...publicKeys.node('node-1'), 0],
      exact: true,
      refetchType: 'active',
    })
  })

  it('invalidates only the addressed Network query', () => {
    const invalidate = vi.spyOn(publicQueryClient, 'invalidateQueries')

    invalidatePublicResource('network', 'network-a', 11)

    expect(invalidate).toHaveBeenCalledTimes(2)
    expect(invalidate.mock.calls.map(([options]) => options)).toEqual([
      { queryKey: [...publicKeys.networks, 0], exact: true, refetchType: 'active' },
      { queryKey: [...publicKeys.network('network-a'), 0], exact: true, refetchType: 'active' },
    ])
  })
})
