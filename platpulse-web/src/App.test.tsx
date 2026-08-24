import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { onlineManager } from '@tanstack/react-query'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import App from './App'
import { adminQueryClient, resetAdminCache } from './api/admin'
import { resetPublicCache } from './api/public'
import { resetRealtimeCursors } from './api/transport'
import { client } from './api/generated/client.gen'

const OWNER_SESSION = {
  session: {
    userId: 'u1',
    username: 'admin',
    role: 'owner',
    createdAt: '2026-08-12T00:00:00Z',
    lastSeenAt: '2026-08-12T00:00:00Z',
    expiresAt: '2026-08-19T00:00:00Z',
  },
  csrfToken: 'csrf-token',
}

const VIEWER_SESSION = {
  ...OWNER_SESSION,
  session: { ...OWNER_SESSION.session, userId: 'u2', username: 'viewer1', role: 'viewer' },
}

function jsonResponse(body: unknown, status: number): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      'Content-Type': 'application/json',
      'X-PlatPulse-Public-Realtime-Cursor': '0',
      'X-PlatPulse-Admin-Realtime-Cursor': '0',
    },
  })
}

function errorBody(code: string): Response {
  return jsonResponse({ error: { code, message: code, requestId: 'r1', fields: [] } }, 401)
}

type RouteHandler = (init?: RequestInit) => Response | Promise<Response>

/** Stub global fetch with per-URL handlers; `*`-suffixed keys match prefixes. */
const TEST_ORIGIN = 'http://platpulse.test'

function mockFetch(routes: Record<string, RouteHandler>) {
  const fetchMock = vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
    // The generated client calls fetch with a `Request` object.
    const url = (input instanceof Request ? input.url : String(input)).replace(TEST_ORIGIN, '')
    for (const [pattern, handler] of Object.entries(routes)) {
      if (pattern.endsWith('*')) {
        if (url.startsWith(pattern.slice(0, -1))) return Promise.resolve(handler(init))
      } else if (url === pattern) {
        return Promise.resolve(handler(init))
      }
    }
    return Promise.resolve(jsonResponse({ error: { code: 'not_found' } }, 404))
  })
  vi.stubGlobal('fetch', fetchMock)
  return fetchMock
}

async function signIn(username = 'admin', password = 'correct horse battery') {
  fireEvent.change(screen.getByLabelText('Username'), { target: { value: username } })
  fireEvent.change(screen.getByLabelText('Password'), { target: { value: password } })
  fireEvent.click(screen.getByRole('button', { name: 'Sign in' }))
  await waitFor(() =>
    expect(screen.getByRole('heading', { level: 1, name: 'Home' })).toBeTruthy(),
  )
}

/** Navigate the in-memory router to /admin the way a browser back/forward
 * event would, so tests exercise the same route transition as the shell.
 * Wrapped in `act` so React flushes the navigation before assertions run. */
async function goToAdmin() {
  await act(async () => {
    window.history.pushState({}, '', '/admin')
    window.dispatchEvent(new PopStateEvent('popstate'))
    await Promise.resolve()
  })
}

/** Minimal EventSource stand-in so Admin SSE behavior is testable in jsdom. */
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

beforeEach(() => {
  window.history.replaceState({}, '', '/')
  Object.defineProperty(window.navigator, 'onLine', { configurable: true, value: true })
  onlineManager.setOnline(true)
  // The generated fetch client builds `new Request(url)`; Node's undici
  // rejects relative URLs, so tests run against an absolute test origin.
  client.setConfig({ baseUrl: TEST_ORIGIN })
})

afterEach(() => {
  cleanup()
  Object.defineProperty(window.navigator, 'onLine', { configurable: true, value: true })
  onlineManager.setOnline(true)
  window.dispatchEvent(new Event('online'))
  vi.unstubAllGlobals()
  // The Admin QueryClient lives at module scope (like the router); drop its
  // values between tests so no cached REST data crosses test boundaries.
  adminQueryClient.clear()
  resetAdminCache(0)
  resetPublicCache(0)
  resetRealtimeCursors()
})

describe('App shell with private Home', () => {
  it('hands each first SSE stream the cursor captured by its REST surface', async () => {
    vi.stubGlobal('EventSource', FakeEventSource)
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/public/v1/access': () => jsonResponse({ mode: 'private', authorizationGeneration: 0 }, 200),
      '/api/public/v1/networks': () => jsonResponse([], 200),
    })

    render(<App />)
    await screen.findByRole('heading', { level: 1, name: 'Home' })
    await waitFor(() => expect(FakeEventSource.latest?.url).toBe('/api/public/v1/events?after=0'))

    await goToAdmin()
    await screen.findByRole('heading', { level: 1, name: 'Overview' })
    await waitFor(() => expect(FakeEventSource.latest?.url).toBe('/api/admin/v1/events?after=0'))

    // Leave the module-level browser router at Home for the following shell
    // tests; unmounting RouterProvider alone does not reset its location.
    await act(async () => {
      window.history.pushState({}, '', '/')
      window.dispatchEvent(new PopStateEvent('popstate'))
      await Promise.resolve()
    })
    await screen.findByRole('heading', { level: 1, name: 'Home' })
  })

  it('renders the production public Node Detail contract and switches tabs by keyboard', async () => {
    window.history.replaceState({}, '', '/')
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/public/v1/networks': () => jsonResponse([], 200),
      '/api/public/v1/nodes/node-1': () => jsonResponse({
        nodeId: 'node-1',
        displayName: 'Validator A',
        networkKey: 'mainnet',
        health: 'unhealthy',
        healthReason: 'RPC observation failed',
        freshness: 'stale',
        rpcState: 'error',
        syncState: 'unknown',
        consensusState: 'unsupported',
        processState: 'disabled',
        resyncState: 'normal',
        networkReferenceConfidence: 'unknown',
        currentHead: 123,
        historicalHighWatermark: 128,
        networkReferenceHead: null,
        hostCpuPercent: 42.5,
        peers: {
          state: 'error',
          freshness: 'stale',
          peerCount: 12,
          inboundCount: 8,
          outboundCount: 4,
          receivedAt: '2026-08-20T00:00:00Z',
          staleSince: '2026-08-20T00:05:00Z',
        },
      }, 200),
      '/api/public/v1/nodes/node-1/history': () => jsonResponse([{
        nodeId: 'node-1',
        height: 123,
        blockTimeMs: 1_755_638_400_000,
        transactionCount: 4,
        observedAt: '2026-08-20T00:00:00Z',
      }], 200),
      '/api/public/v1/nodes/node-1/history/export': () => jsonResponse([{
        nodeId: 'node-1',
        height: 123,
        observedAt: '2026-08-20T00:00:00Z',
      }], 200),
      '/api/public/v1/nodes/node-1/peer-history': () => jsonResponse({
        state: 'ok',
        freshness: 'current',
        fiveMinute: [],
        hourly: [],
      }, 200),
    })

    render(<App />)
    await screen.findByRole('heading', { level: 1, name: 'Home' })
    expect(screen.queryByRole('navigation', { name: 'Prototype variants' })).toBeNull()

    for (const variant of ['signal-stack', 'mission-control', 'evidence-ledger']) {
      await act(async () => {
        window.history.pushState({}, '', `/?variant=${variant}`)
        window.dispatchEvent(new PopStateEvent('popstate'))
        await Promise.resolve()
      })
      expect(screen.getByRole('heading', { level: 1, name: 'Home' })).toBeTruthy()
      expect(screen.queryByRole('navigation', { name: 'Prototype variants' })).toBeNull()
    }

    await act(async () => {
      window.history.pushState({}, '', '/nodes/node-1?variant=signal-stack')
      window.dispatchEvent(new PopStateEvent('popstate'))
      await Promise.resolve()
    })

    expect(await screen.findByRole('heading', { level: 1, name: 'Validator A' })).toBeTruthy()
    expect(screen.getByText('Node Health Summary')).toBeTruthy()
    expect(screen.getAllByText('RPC').length).toBeGreaterThan(0)
    expect(screen.getAllByText('Sync').length).toBeGreaterThan(0)
    expect(screen.getAllByText('Consensus').length).toBeGreaterThan(0)
    expect(screen.getAllByText('Process').length).toBeGreaterThan(0)
    expect(screen.getAllByText('Resync').length).toBeGreaterThan(0)
    expect(screen.getAllByText('Current Head').length).toBeGreaterThan(0)
    expect(screen.getAllByText('History Boundary').length).toBeGreaterThan(0)
    expect(screen.getByText('Bounded Block History')).toBeTruthy()
    expect(screen.getByText(/Server-configured history window; absent blocks are not zero/)).toBeTruthy()
    expect(screen.getByText('Last-good peers')).toBeTruthy()
    expect(screen.getAllByText('12').length).toBeGreaterThan(0)
    expect(screen.getAllByText('Error').length).toBeGreaterThan(0)
    expect(screen.getAllByText('Unknown').length).toBeGreaterThan(0)
    expect(screen.getAllByText('Unsupported').length).toBeGreaterThan(0)
    expect(screen.getAllByText('Disabled').length).toBeGreaterThan(0)
    expect(screen.getAllByText('Stale').length).toBeGreaterThan(0)
    expect(screen.queryByRole('navigation', { name: 'Prototype variants' })).toBeNull()

    for (const variant of ['mission-control', 'evidence-ledger']) {
      await act(async () => {
        window.history.pushState({}, '', `/nodes/node-1?variant=${variant}`)
        window.dispatchEvent(new PopStateEvent('popstate'))
        await Promise.resolve()
      })
      expect(screen.getByRole('heading', { level: 1, name: 'Validator A' })).toBeTruthy()
      expect(screen.queryByRole('navigation', { name: 'Prototype variants' })).toBeNull()
    }

    const createObjectUrl = vi.fn(() => 'blob:public-history')
    const revokeObjectUrl = vi.fn()
    Object.defineProperty(URL, 'createObjectURL', { configurable: true, value: createObjectUrl })
    Object.defineProperty(URL, 'revokeObjectURL', { configurable: true, value: revokeObjectUrl })
    fireEvent.click(screen.getByRole('button', { name: 'Export public history' }))
    await waitFor(() => expect(createObjectUrl).toHaveBeenCalled())

    const detailsTab = screen.getByRole('tab', { name: 'Details' })
    const networkTab = screen.getByRole('tab', { name: 'Network' })
    expect(detailsTab.getAttribute('aria-selected')).toBe('true')
    expect(screen.getByRole('tabpanel', { name: 'Details' })).toBeTruthy()

    networkTab.focus()
    fireEvent.keyDown(networkTab, { key: 'Enter' })
    expect(networkTab.getAttribute('aria-selected')).toBe('true')
    expect(screen.getByRole('tabpanel', { name: 'Network' })).toBeTruthy()
    expect(screen.getByRole('heading', { name: 'Peer history' })).toBeTruthy()
  })

  it('guides an unauthenticated visitor to the login page', async () => {
    mockFetch({ '/api/public/v1/session': () => errorBody('auth_required') })

    render(<App />)
    expect(
      await screen.findByRole('heading', { level: 1, name: 'Sign in to PlatPulse' }),
    ).toBeTruthy()
  })

  it('renders the Home shell for an authenticated Owner', async () => {
    mockFetch({ '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200) })

    render(<App />)
    expect(await screen.findByRole('heading', { level: 1, name: 'Home' })).toBeTruthy()
    expect(screen.getByRole('link', { name: 'Admin' })).toBeTruthy()
  })

  it('signs in and returns to Home', async () => {
    mockFetch({
      '/api/public/v1/session': () => errorBody('auth_required'),
      '/api/public/v1/login': () => jsonResponse(OWNER_SESSION, 200),
    })

    render(<App />)
    await screen.findByRole('heading', { level: 1, name: 'Sign in to PlatPulse' })
    await signIn()
    expect(screen.getByRole('link', { name: 'Admin' })).toBeTruthy()
  })

  it('shows the invalid-credentials error without navigating', async () => {
    mockFetch({
      '/api/public/v1/session': () => errorBody('auth_required'),
      '/api/public/v1/login': () => errorBody('invalid_credentials'),
    })

    render(<App />)
    await screen.findByRole('heading', { level: 1, name: 'Sign in to PlatPulse' })
    fireEvent.change(screen.getByLabelText('Username'), { target: { value: 'admin' } })
    fireEvent.change(screen.getByLabelText('Password'), { target: { value: 'wrong' } })
    fireEvent.click(screen.getByRole('button', { name: 'Sign in' }))

    await screen.findByRole('alert')
    expect(screen.getByRole('alert').textContent).toContain('Invalid username or password')
    expect(screen.getByRole('heading', { level: 1, name: 'Sign in to PlatPulse' })).toBeTruthy()
  })

  it('signs out back to the login page', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/public/v1/logout': () => new Response(null, { status: 204 }),
    })

    render(<App />)
    await screen.findByRole('heading', { level: 1, name: 'Home' })
    await goToAdmin()
    await screen.findByRole('heading', { level: 1, name: 'Overview' })
    fireEvent.click(screen.getByRole('button', { name: 'Sign out' }))
    expect(
      await screen.findByRole('heading', { level: 1, name: 'Sign in to PlatPulse' }),
    ).toBeTruthy()
  })

  it('stays signed in when logout revocation fails', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/public/v1/logout': () =>
        jsonResponse(
          {
            error: {
              code: 'session_revocation_failed',
              message: 'could not revoke the session; try again',
              requestId: 'r2',
              fields: [],
            },
          },
          500,
        ),
    })

    render(<App />)
    await screen.findByRole('heading', { level: 1, name: 'Home' })
    await goToAdmin()
    await screen.findByRole('heading', { level: 1, name: 'Overview' })
    fireEvent.click(screen.getByRole('button', { name: 'Sign out' }))
    expect(await screen.findByText('Could not sign out. Try again.')).toBeTruthy()
    expect(screen.getByRole('heading', { level: 1, name: 'Overview' })).toBeTruthy()
  })

  it('allows a Viewer into Home, hides Admin, and refuses the Admin shell', async () => {
    mockFetch({ '/api/public/v1/session': () => jsonResponse(VIEWER_SESSION, 200) })

    render(<App />)
    await act(async () => {
      window.history.pushState({}, '', '/')
      window.dispatchEvent(new PopStateEvent('popstate'))
      await Promise.resolve()
    })
    await screen.findByRole('heading', { level: 1, name: 'Home' })
    // Viewers are not offered an Admin entry point; the Server remains the
    // enforcement boundary for anyone who navigates there anyway.
    expect(screen.queryByRole('link', { name: 'Admin' })).toBeNull()
    window.history.pushState({}, '', '/admin')
    window.dispatchEvent(new PopStateEvent('popstate'))
    expect(
      await screen.findByRole('heading', { level: 1, name: 'Owner access required' }),
    ).toBeTruthy()
  })

  it('renders published Network and Node data on Home', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/public/v1/networks': () => jsonResponse([{
        networkKey: 'mainnet',
        displayName: 'Mainnet',
        nodes: [{
          nodeId: 'node-1',
          displayName: 'Validator A',
          networkKey: 'mainnet',
          health: 'healthy',
          healthReason: 'rpc reachable',
          freshness: '2026-08-12T00:00:00Z',
          rpcState: 'ok',
          hostCpuPercent: 42.5,
        }],
      }], 200),
      '/api/public/v1/networks/mainnet': () => jsonResponse({
        networkKey: 'mainnet',
        displayName: 'Mainnet',
        nodes: [{
          nodeId: 'node-1',
          displayName: 'Validator A',
          networkKey: 'mainnet',
          health: 'healthy',
          healthReason: 'rpc reachable',
          freshness: 'current',
          rpcState: 'ok',
          syncState: 'synced',
          consensusState: 'current',
          processState: 'running',
          resyncState: 'idle',
          currentHead: 123,
          historicalHighWatermark: 120,
          peers: { state: 'current', freshness: 'current', peerCount: 3 },
        }],
        peers: { state: 'current', freshness: 'current', peerCount: 3 },
        geo: { state: 'disabled' },
        validators: [],
      }, 200),
    })

    render(<App />)
    const homeLink = await screen.findByRole('link', { name: 'Home' })
    fireEvent.click(homeLink)
    expect(await screen.findByRole('heading', { level: 1, name: 'Home' })).toBeTruthy()
    expect(await screen.findByRole('link', { name: 'Mainnet' })).toBeTruthy()
    expect(screen.getByRole('link', { name: 'Validator A' })).toBeTruthy()
    expect(screen.getByText('Healthy')).toBeTruthy()
    fireEvent.click(screen.getByRole('link', { name: 'Mainnet' }))
    expect(await screen.findByRole('heading', { level: 1, name: 'Mainnet' })).toBeTruthy()
    expect(screen.getByText(/NETWORK OVERVIEW/)).toBeTruthy()
    expect(screen.getByText('PlatON Nodes')).toBeTruthy()
    expect(screen.getByText('Network key')).toBeTruthy()
  })

  it('keeps the Public Node route and last-good detail during a failed live refresh', async () => {
    let nodeCalls = 0
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/public/v1/networks': () => jsonResponse([], 200),
      '/api/public/v1/nodes/node-1': () => {
        nodeCalls += 1
        return nodeCalls === 1
          ? jsonResponse({
              nodeId: 'node-1', displayName: 'Validator A', networkKey: 'mainnet',
              health: 'healthy', healthReason: 'rpc reachable', freshness: 'current',
              rpcState: 'ok', syncState: 'synced', consensusState: 'current', processState: 'running',
              currentHead: 123, historicalHighWatermark: 120, networkReferenceHead: 123,
              networkReferenceConfidence: 'high', resyncState: 'idle', resyncProgress: null,
              hostCpuPercent: 42.5, peers: { peerCount: 5, freshness: 'fresh', state: 'fresh' },
            }, 200)
          : jsonResponse({ error: { code: 'unavailable', message: 'refresh failed' } }, 503)
      },
      '/api/public/v1/nodes/node-1/history': () => jsonResponse([], 200),
      '/api/public/v1/nodes/node-1/peer-history': () => jsonResponse({ state: 'ok', freshness: 'current', fiveMinute: [], hourly: [] }, 200),
    })
    vi.stubGlobal('EventSource', FakeEventSource)
    window.history.replaceState({}, '', '/nodes/node-1')

    render(<App />)
    await act(async () => {
      window.dispatchEvent(new PopStateEvent('popstate'))
      await Promise.resolve()
    })
    expect(await screen.findByRole('heading', { level: 1, name: 'Validator A' })).toBeTruthy()
    fireEvent.click(screen.getByRole('tab', { name: 'Network' }))
    expect(screen.getByRole('tab', { name: 'Network' }).getAttribute('aria-selected')).toBe('true')

    await act(async () => {
      expect(FakeEventSource.latest).toBeTruthy()
      FakeEventSource.latest?.emit(
        'invalidation',
        JSON.stringify({ version: 1, eventId: 3, resource: 'collection', revision: 3 }),
      )
      await Promise.resolve()
    })

    await waitFor(() => expect(nodeCalls).toBe(2))
    window.dispatchEvent(new Event('offline'))
    expect(await screen.findByText('You are offline')).toBeTruthy()
    expect(screen.getByRole('heading', { level: 1, name: 'Validator A' })).toBeTruthy()
    expect(screen.getByRole('tab', { name: 'Network' }).getAttribute('aria-selected')).toBe('true')
    expect(screen.getByText(/last successful Node data/i)).toBeTruthy()
  })

  it('submits the Owner visibility mutation from Admin', async () => {
    const fetchMock = mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/nodes/node-1/visibility': () => jsonResponse({ nodeId: 'node-1', visibility: 'public' }, 200),
    })
    window.history.replaceState({}, '', '/admin')

    render(<App />)
    const adminLink = await screen.findByRole('link', { name: 'Admin' })
    fireEvent.click(adminLink)
    await screen.findByRole('heading', { level: 1, name: 'Overview' })
    fireEvent.change(screen.getByLabelText('Node ID'), { target: { value: 'node-1' } })
    fireEvent.click(screen.getByRole('button', { name: 'Update visibility' }))

    expect(await screen.findByText('node-1 is now public.')).toBeTruthy()
    const reportRequest = fetchMock.mock.calls
      .map(([input]) => input)
      .find((input): input is Request => input instanceof Request && input.url.includes('/api/admin/v1/nodes/node-1/visibility'))
    expect(reportRequest).toBeTruthy()
    expect(reportRequest?.method).toBe('PUT')
    expect(reportRequest?.headers.get('X-CSRF-Token')).toBe('csrf-token')
  })

  it('shows Checking access… before authorization resolves and never renders a session flash', async () => {
    let resolveSession: ((value: Response) => void) | null = null
    const sessionGate = new Promise<Response>((resolve) => {
      resolveSession = resolve
    })
    mockFetch({ '/api/public/v1/session': () => sessionGate })

    render(<App />)
    await goToAdmin()
    expect(await screen.findByText(/Checking access/)).toBeTruthy()
    expect(screen.queryByRole('heading', { level: 1, name: 'Overview' })).toBeNull()

    resolveSession!(jsonResponse(OWNER_SESSION, 200))
    expect(await screen.findByRole('heading', { level: 1, name: 'Overview' })).toBeTruthy()
  })

  it('refetches Admin REST after an SSE invalidation without remounting the page', async () => {
    let overviewCalls = 0
    const fetchMock = mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/overview': () => {
        overviewCalls += 1
        return jsonResponse(
          {
            generatedAt: '2026-08-12T00:00:00Z',
            summary: {
              agents: { total: 1, online: 1, offline: 0, unknown: 0 },
              nodes: { total: 1, healthy: 1, unhealthy: 0, unknown: 0, retired: 0, published: 1 },
            },
            attention: [],
          },
          200,
        )
      },
      '/api/admin/v1/agents': () => jsonResponse([], 200),
    })
    vi.stubGlobal('EventSource', FakeEventSource)

    render(<App />)
    await goToAdmin()
    await screen.findByRole('heading', { level: 1, name: 'Overview' })
    await waitFor(() => expect(overviewCalls).toBe(1))
    const overviewRequest = fetchMock.mock.calls
      .map(([input]) => input)
      .find((input): input is Request => input instanceof Request && input.url.includes('/api/admin/v1/overview'))
    expect(overviewRequest?.headers.get('X-PlatPulse-Access-Generation')).toBe('1')

    await act(async () => {
      FakeEventSource.latest?.emit(
        'invalidation',
        JSON.stringify({ version: 1, eventId: 2, resource: 'node', revision: 2 }),
      )
      await Promise.resolve()
    })
    await waitFor(() => expect(overviewCalls).toBe(2))
    // The heading and page survive the refetch.
    expect(screen.getByRole('heading', { level: 1, name: 'Overview' })).toBeTruthy()
  })

  it('treats an SSE access reset as a session loss without leaking Admin data', async () => {
    let sessionActive = true
    const fetchMock = vi.fn((input: RequestInfo | URL) => {
      const url = String(input instanceof Request ? input.url : input).replace(TEST_ORIGIN, '')
      if (url === '/api/public/v1/session') {
        return Promise.resolve(
          sessionActive ? jsonResponse(OWNER_SESSION, 200) : errorBody('auth_required'),
        )
      }
      if (url === '/api/admin/v1/overview') {
        return Promise.resolve(
          jsonResponse(
            {
              generatedAt: '2026-08-12T00:00:00Z',
              summary: {
                agents: { total: 1, online: 1, offline: 0, unknown: 0 },
                nodes: { total: 1, healthy: 1, unhealthy: 0, unknown: 0, retired: 0, published: 1 },
              },
              attention: [],
            },
            200,
          ),
        )
      }
      if (url === '/api/admin/v1/agents') return Promise.resolve(jsonResponse([], 200))
      return Promise.resolve(jsonResponse({ error: { code: 'not_found' } }, 404))
    })
    vi.stubGlobal('fetch', fetchMock)
    vi.stubGlobal('EventSource', FakeEventSource)

    render(<App />)
    await goToAdmin()
    await screen.findByRole('heading', { level: 1, name: 'Overview' })

    sessionActive = false
    await act(async () => {
      FakeEventSource.latest?.emit(
        'invalidation',
        JSON.stringify({ version: 1, eventId: 0, resource: 'collection', reset: true }),
      )
      await Promise.resolve()
    })

    await screen.findByRole('heading', { level: 1, name: 'Sign in to PlatPulse' }, { timeout: 3000 })
    expect(screen.getByText(/expired or was revoked/)).toBeTruthy()
    expect(screen.queryByRole('heading', { level: 1, name: 'Overview' })).toBeNull()
  })

  it('never flashes a previous session Admin cache after re-login', async () => {
    let session: unknown = OWNER_SESSION
    const pendingOverview: Array<{ resolve: (value: Response) => void; slot: 1 | 2 }> = []
    let overviewCalls = 0
    const attention = (label: string, message: string) => [
      {
        id: `node_unhealthy:node:${label}`,
        kind: 'node_unhealthy',
        severity: 'critical',
        subject_kind: 'node',
        subject_id: label,
        subject_label: label,
        message,
        observed_at: '2026-08-12T00:00:00Z',
      },
    ]
    const firstOverview = {
      generated_at: '2026-08-12T00:00:00Z',
      summary: {
        agents: { total: 1, online: 1, offline: 0, unknown: 0 },
        nodes: { total: 1, healthy: 1, unhealthy: 0, unknown: 0, retired: 0, published: 1 },
      },
      attention: attention('Node X', 'RPC collection failed'),
    }
    const secondOverview = {
      ...firstOverview,
      attention: attention('Node Y', 'sync collection failed'),
    }
    const fetchMock = vi.fn((input: RequestInfo | URL) => {
      const url = String(input instanceof Request ? input.url : input).replace(TEST_ORIGIN, '')
      if (url === '/api/public/v1/session') {
        return Promise.resolve(session ? jsonResponse(session, 200) : errorBody('auth_required'))
      }
      if (url === '/api/public/v1/login') {
        session = {
          ...OWNER_SESSION,
          session: { ...OWNER_SESSION.session, userId: 'u9', username: 'admin-b' },
        }
        return Promise.resolve(jsonResponse(session, 200))
      }
      if (url === '/api/admin/v1/overview') {
        overviewCalls += 1
        // Hold the fetch so the test can prove the panel starts from a clean
        // slate while the REST refetch is still in flight.
        return new Promise((resolve) => {
          pendingOverview.push({ resolve, slot: overviewCalls === 1 ? 1 : 2 })
        })
      }
      if (url === '/api/admin/v1/agents') return Promise.resolve(jsonResponse([], 200))
      return Promise.resolve(jsonResponse({ error: { code: 'not_found' } }, 404))
    })
    vi.stubGlobal('fetch', fetchMock)
    vi.stubGlobal('EventSource', FakeEventSource)

    render(<App />)
    await goToAdmin()
    await waitFor(() => expect(overviewCalls).toBe(1))
    // While the first fetch is held, the panel must show Starting, not a
    // previous session's data.
    expect(screen.getByText('Checking the Server for attention…')).toBeTruthy()
    pendingOverview.find((entry) => entry.slot === 1)?.resolve(jsonResponse(firstOverview, 200))
    await screen.findByText('Node X')

    // Session A is revoked while the Admin surface is open.
    session = null
    await act(async () => {
      FakeEventSource.latest?.emit(
        'reset',
        JSON.stringify({ version: 1, eventId: 0, resource: 'collection', reset: true }),
      )
      await Promise.resolve()
    })
    await screen.findByRole('heading', { level: 1, name: 'Sign in to PlatPulse' })
    expect(screen.queryByText('Node X')).toBeNull()

    // A different Owner signs in and re-enters Admin while the new REST
    // refetch is deliberately slow: neither the old session's data nor the
    // new payload may appear before the refetch completes.
    fireEvent.change(screen.getByLabelText('Username'), { target: { value: 'admin-b' } })
    fireEvent.change(screen.getByLabelText('Password'), { target: { value: 'pw' } })
    fireEvent.click(screen.getByRole('button', { name: 'Sign in' }))
    await waitFor(() =>
      expect(screen.getByRole('heading', { level: 1, name: 'Home' })).toBeTruthy(),
    )
    await goToAdmin()
    await waitFor(() => expect(overviewCalls).toBe(2))
    expect(screen.queryByText('Node X')).toBeNull()
    expect(screen.queryByText('Node Y')).toBeNull()

    pendingOverview.find((entry) => entry.slot === 2)?.resolve(jsonResponse(secondOverview, 200))
    expect(await screen.findByText('Node Y')).toBeTruthy()
    expect(screen.queryByText('Node X')).toBeNull()
  })

  it('refuses the Admin shell for a Viewer without rendering Admin data', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(VIEWER_SESSION, 200),
      '/api/admin/v1/overview': () =>
        jsonResponse({ error: { code: 'owner_required' } }, 403),
    })

    render(<App />)
    // Settle the shared router at Home first (the Viewer session renders the
    // Home shell from any starting route), then navigate to the Admin route.
    await act(async () => {
      window.history.pushState({}, '', '/')
      window.dispatchEvent(new PopStateEvent('popstate'))
      await Promise.resolve()
    })
    await screen.findByRole('heading', { level: 1, name: 'Home' })
    await goToAdmin()
    expect(
      await screen.findByRole('heading', { level: 1, name: 'Owner access required' }),
    ).toBeTruthy()
    expect(screen.queryByRole('heading', { level: 1, name: 'Overview' })).toBeNull()
    expect(screen.queryByText('Attention queue')).toBeNull()
  })

  it('keeps keyboard submission working on the login form', async () => {
    mockFetch({
      '/api/public/v1/session': () => errorBody('auth_required'),
      '/api/public/v1/login': () => jsonResponse(OWNER_SESSION, 200),
    })

    render(<App />)
    // Settle the shared router at Home/root first: earlier tests left the
    // router on /admin, and an anonymous Guest there now gets the stable
    // Owner-required panel instead of a redirect (design §12.1).
    await act(async () => {
      window.history.pushState({}, '', '/')
      window.dispatchEvent(new PopStateEvent('popstate'))
      await Promise.resolve()
    })
    await screen.findByRole('heading', { level: 1, name: 'Sign in to PlatPulse' })
    fireEvent.change(screen.getByLabelText('Username'), { target: { value: 'admin' } })
    fireEvent.change(screen.getByLabelText('Password'), { target: { value: 'correct horse battery' } })
    const form = screen.getByLabelText('Password').closest('form')
    expect(form).toBeTruthy()
    fireEvent.submit(form!)
    await waitFor(() =>
      expect(screen.getByRole('heading', { level: 1, name: 'Home' })).toBeTruthy(),
    )
  })
})

describe('Admin MVP route inventory (issue #92)', () => {
  /** Navigate the shared in-memory router the way browser history does. */
  async function renderAt(path: string) {
    await act(async () => {
      window.history.pushState({}, '', path)
      window.dispatchEvent(new PopStateEvent('popstate'))
      await Promise.resolve()
    })
  }

  // Complete MVP Admin inventory (issue #92): Overview, Agents, Agent
  // Detail, Nodes, Node Detail, Networks, Network Detail, History Window,
  // Site Access, Sessions and Audit. Each route renders its own page shell
  // under the Owner gate; the Server REST mock answers 404s so the pages'
  // headings are asserted without seeding page data.
  const MVP_ROUTES: Array<[path: string, heading: RegExp]> = [
    ['/admin', /Overview/],
    ['/admin/agents', /Agents/],
    ['/admin/agents/agent-1', /Agent agent-1/],
    ['/admin/nodes', /Nodes/],
    ['/admin/nodes/node-1', /node-1/],
    ['/admin/networks', /Networks/],
    ['/admin/networks/mainnet', /mainnet/],
    ['/admin/history-window', /History Window/],
    ['/admin/site-access', /Site Access/],
    ['/admin/access/sessions', /Sessions/],
    ['/admin/access/audit', /Audit log/],
  ]

  it('reaches every MVP Admin page through the production router', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/access-mode': () =>
        jsonResponse({ mode: 'private', authorizationGeneration: 0 }, 200),
    })
    render(<App />)
    await renderAt('/')
    await screen.findByRole('heading', { level: 1, name: 'Home' })
    await renderAt('/admin')
    await screen.findByRole('heading', { level: 1, name: 'Overview' })

    for (const [path, heading] of MVP_ROUTES) {
      await renderAt(path)
      await screen.findByRole('heading', { level: 1, name: heading })
    }
  })

  // Removed legacy/deferred routes (issue #92): Validator administration,
  // People, Alerts/Incidents/Silences/Maintenance, Delivery/Channel,
  // Operations, Data/Retention, Backup/Restore, Doctor, Node Transfer,
  // Node Visibility and Agent Enrollment/Recovery/Rotation. Direct
  // navigation must land on the safe Admin fallback — never on a legacy
  // page. `/admin/agents/enroll` is covered separately: it matches the
  // generic `agents/:agentId` detail route and resolves to the normal
  // unknown-Agent outcome instead of the removed enrollment page.
  const REMOVED_ROUTES: Array<[path: string, legacyHeading: RegExp]> = [
    ['/admin/validators', /Validators/],
    ['/admin/validators/v-1', /Validators/],
    ['/admin/access/people', /People/],
    ['/admin/alerts', /Alerts/],
    ['/admin/alerts/rules', /Alert Rules/],
    ['/admin/alerts/rules/r-1', /Alert Rules/],
    ['/admin/alerts/rules/r-1/edit', /Alert Rules/],
    ['/admin/alerts/incidents', /Incidents/],
    ['/admin/alerts/incidents/i-1', /Incidents/],
    ['/admin/alerts/silences', /Silences/],
    ['/admin/alerts/maintenance', /Maintenance/],
    ['/admin/alerts/deliveries', /Deliveries/],
    ['/admin/alerts/deliveries/d-1', /Deliveries/],
    ['/admin/alerts/channels', /Channels/],
    ['/admin/alerts/channels/c-1', /Channels/],
    ['/admin/operations', /Operations/],
    ['/admin/operations/o-1', /Operations/],
    ['/admin/data', /Data/],
    ['/admin/data/retention', /Retention/],
    ['/admin/data/retention/edit', /Retention/],
    ['/admin/data/backups', /Backups/],
    ['/admin/data/backups/create', /Backups/],
    ['/admin/data/backups/b-1', /Backups/],
    ['/admin/data/restore', /Restore/],
    ['/admin/data/doctor', /Doctor/],
    ['/admin/nodes/node-1/visibility', /Node visibility/],
    ['/admin/nodes/node-1/transfer', /Transfer Node ownership/],
    ['/admin/agents/agent-1/recover', /Recover Agent/],
    ['/admin/agents/agent-1/rotate', /Rotate credential/],
  ]

  it('resolves every removed legacy or deferred Admin route to the safe fallback', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
    })
    render(<App />)
    await renderAt('/')
    await screen.findByRole('heading', { level: 1, name: 'Home' })
    await renderAt('/admin')
    await screen.findByRole('heading', { level: 1, name: 'Overview' })

    for (const [path, legacyHeading] of REMOVED_ROUTES) {
      await renderAt(path)
      await screen.findByRole('heading', { level: 1, name: 'Section not found' })
      expect(screen.queryByRole('heading', { level: 1, name: legacyHeading })).toBeNull()
    }

    // `/admin/agents/enroll` is not registered: it falls through to the
    // generic Agent Detail route for an unknown id, never the enrollment
    // workflow.
    await renderAt('/admin/agents/enroll')
    await screen.findByRole('heading', { level: 1, name: /Agent enrol/ })
    expect(screen.queryByRole('heading', { level: 1, name: /Enroll a new Agent/ })).toBeNull()
  })

  it('exposes only the MVP page groups through Admin navigation', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
    })
    render(<App />)
    await renderAt('/')
    await screen.findByRole('heading', { level: 1, name: 'Home' })
    await renderAt('/admin')
    await screen.findByRole('heading', { level: 1, name: 'Overview' })

    const adminNav = screen.getByRole('navigation', { name: 'Admin' })
    const menu = adminNav.querySelectorAll('a')
    const links = Array.from(menu).map((element) => ({
      name: element.textContent?.trim() ?? '',
      href: element.getAttribute('href'),
    }))
    expect(links).toEqual([
      { name: 'Overview', href: '/admin' },
      { name: 'Agents', href: '/admin/agents' },
      { name: 'Nodes', href: '/admin/nodes' },
      { name: 'Networks', href: '/admin/networks' },
      { name: 'History Window', href: '/admin/history-window' },
      { name: 'Site Access', href: '/admin/site-access' },
      { name: 'Sessions', href: '/admin/access/sessions' },
      { name: 'Audit', href: '/admin/access/audit' },
    ])
    for (const removed of ['Validators', 'People', 'Alert Rules', 'Incidents', 'Silences', 'Maintenance', 'Deliveries', 'Channels', 'Operations', 'Data', 'Retention', 'Backups', 'Restore', 'Doctor', 'Enroll', 'Recover', 'Rotate']) {
      expect(
        Array.from(adminNav.querySelectorAll('a')).some((element) =>
          element.textContent?.includes(removed),
        ),
        'removed page ' + removed + ' must not be linked from Admin navigation',
      ).toBe(false)
    }
  })
})
