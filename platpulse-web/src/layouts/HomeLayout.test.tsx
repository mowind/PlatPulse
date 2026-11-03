import { cleanup, render, screen, waitFor } from '@testing-library/react'
import { useLayoutEffect } from 'react'
import { createMemoryRouter, RouterProvider } from 'react-router'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { applySiteAccessSettings, resetPublicCache, usePublicNodeMetrics } from '../api/public'
import { client } from '../api/generated/client.gen'
import { resetRealtimeCursors } from '../api/transport'
import { ThemeProvider } from '../theme/ThemeProvider'
import HomeLayout, { useHomeRealtimeContext } from './HomeLayout'

vi.mock('../auth/AuthContext', () => ({
  useAuth: () => ({
    status: { state: 'authenticated', session: { role: 'viewer' } },
    outage: false,
    recheckSession: vi.fn(),
  }),
}))

const TEST_ORIGIN = 'http://platpulse.test'
const NODE_ID = 'node-1'

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json', 'X-PlatPulse-Public-Realtime-Cursor': '0' },
  })
}

/** Minimal EventSource stand-in: the test emits Invalidation Events by hand. */
class FakeEventSource {
  static latest: FakeEventSource | null = null
  private handlers: Record<string, Array<(event: { data?: string }) => void>> = {}
  onopen: (() => void) | null = null
  onerror: (() => void) | null = null
  constructor(public url: string) {
    FakeEventSource.latest = this
  }
  addEventListener(type: string, handler: (event: { data?: string }) => void) {
    ;(this.handlers[type] ??= []).push(handler)
  }
  removeEventListener(type: string, handler: (event: { data?: string }) => void) {
    this.handlers[type] = (this.handlers[type] ?? []).filter((candidate) => candidate !== handler)
  }
  close() {
    if (FakeEventSource.latest === this) FakeEventSource.latest = null
  }
  emit(type: string, data?: string) {
    for (const handler of this.handlers[type] ?? []) handler({ data })
  }
}

/**
 * A Node page whose Site Access answer lands while the shell is mounting:
 * after HomeLayout read the generation for its first render, before the
 * shell's own effects run (a child's layout effect runs in that gap).
 */
function NodeReader() {
  const { generation } = useHomeRealtimeContext()
  useLayoutEffect(() => {
    applySiteAccessSettings({ mode: 'private', authorizationGeneration: 3 })
  }, [])
  const metrics = usePublicNodeMetrics(NODE_ID, generation)
  return <p data-testid="reader">{'generation ' + generation + (metrics.data ? ' loaded' : '')}</p>
}

beforeEach(() => {
  client.setConfig({ baseUrl: TEST_ORIGIN })
  vi.stubGlobal('EventSource', FakeEventSource)
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  resetPublicCache(0)
  resetRealtimeCursors()
})

describe('HomeLayout Site Access generation', () => {
  it('keys Public reads by a generation answered between its first render and its subscription', async () => {
    const metricsGenerations: string[] = []
    vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL) => {
      const request = input instanceof Request ? input : new Request(String(input))
      const url = request.url.replace(TEST_ORIGIN, '').split('?')[0]
      if (url === '/api/public/v1/networks') return jsonResponse([])
      if (url === '/api/public/v1/nodes/' + NODE_ID + '/metrics') {
        metricsGenerations.push(request.headers.get('X-PlatPulse-Access-Generation') ?? '')
        return jsonResponse({ nodeId: NODE_ID, samples: [] })
      }
      return jsonResponse({ error: { code: 'not_found' } }, 404)
    }))
    const router = createMemoryRouter(
      [{ element: <HomeLayout />, children: [{ path: '/nodes/:nodeId', element: <NodeReader /> }] }],
      { initialEntries: ['/nodes/' + NODE_ID] },
    )
    render(<ThemeProvider><RouterProvider router={router} /></ThemeProvider>)

    // The shell follows the Server's answer instead of the fallback it rendered with.
    await waitFor(() => expect(screen.getByTestId('reader').textContent).toBe('generation 3 loaded'))
    await waitFor(() => expect(FakeEventSource.latest).not.toBeNull())
    const before = metricsGenerations.length

    // So an Invalidation Event of this Node, addressed to the Server's generation,
    // reaches the metric window the page reads.
    FakeEventSource.latest!.emit('invalidation', JSON.stringify({ resource: 'node', resourceId: NODE_ID, revision: 1, eventId: 1 }))
    await waitFor(() => expect(metricsGenerations.length).toBeGreaterThan(before))
    expect(metricsGenerations.at(-1)).toBe('3')
  })
})
