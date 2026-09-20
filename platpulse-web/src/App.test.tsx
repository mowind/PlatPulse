import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { onlineManager } from '@tanstack/react-query'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import App from './App'
import { adminQueryClient, resetAdminCache } from './api/admin'
import { applySiteAccessSettings, resetPublicCache } from './api/public'
import { resetRealtimeCursors } from './api/transport'
import { client } from './api/generated/client.gen'
import type { PublicNode } from './api/generated/types.gen'

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
    expect(screen.getByRole('region', { name: 'Home' })).toBeTruthy(),
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

/** Open the Node Detail Peer diagnostics disclosure and return it. */
function openPeerDisclosure(): HTMLDetailsElement {
  const disclosure = screen.getByText('Peer diagnostics').closest('details')
  if (!disclosure) throw new Error('Peer diagnostics disclosure is missing')
  const summary = disclosure.querySelector('summary')
  if (!summary) throw new Error('Peer diagnostics disclosure has no summary to activate')
  fireEvent.click(summary)
  return disclosure as HTMLDetailsElement
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
  vi.useRealTimers()
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
    await screen.findByRole('region', { name: 'Home' })
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
    await screen.findByRole('region', { name: 'Home' })
  })

  it.each([
    { name: 'omitted', size: undefined, capacity: undefined, freshness: 'unknown', usage: 'Unknown', bytes: 'Unknown' },
    { name: 'null', size: null, capacity: null, freshness: 'unknown', usage: 'Unknown', bytes: 'Unknown' },
    { name: 'non-zero', size: 2048, capacity: 8192, freshness: 'current', usage: '25.0%', bytes: '2.00 KiB / 8.00 KiB' },
    { name: 'zero size', size: 0, capacity: 8192, freshness: 'current', usage: '0.0%', bytes: '0 B / 8.00 KiB' },
    { name: 'zero capacity', size: 0, capacity: 0, freshness: 'current', usage: 'Unknown', bytes: '0 B / 0 B' },
    { name: 'retained last-good', size: 2048, capacity: 8192, freshness: 'stale', usage: '25.0%', bytes: '2.00 KiB / 8.00 KiB' },
    { name: 'size only', size: 2048, capacity: null, freshness: 'current', usage: 'Unknown', bytes: '2.00 KiB / —' },
    { name: 'capacity only', size: null, capacity: 8192, freshness: 'current', usage: 'Unknown', bytes: '— / 8.00 KiB' },
  ])('renders Node directory usage for $name observations without literal undefined', async ({ size, capacity, freshness, usage, bytes }) => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/public/v1/networks': () => jsonResponse([], 200),
      '/api/public/v1/nodes/node-1': () => jsonResponse({
        nodeId: 'node-1',
        displayName: 'Validator A',
        networkKey: 'mainnet',
        health: 'unknown',
        healthReason: 'Observation unavailable',
        freshness,
        peers: { state: 'starting', freshness: 'unknown' },
        nodeDataDirectorySizeBytes: size,
        nodeDataDirectoryCapacityBytes: capacity,
      }, 200),
    })

    render(<App />)
    await screen.findByRole('region', { name: 'Home' })
    try {
      await act(async () => {
        window.history.pushState({}, '', '/nodes/node-1')
        window.dispatchEvent(new PopStateEvent('popstate'))
      })
      const directory = await screen.findByRole('group', { name: 'Node data directory' })
      expect(within(directory).getByText(usage, { exact: true })).toBeTruthy()
      expect(directory.textContent).not.toContain('undefined')
      expect(within(directory).getByText(bytes + ' · directory size against the hosting filesystem capacity, not whole-Host disk usage', { exact: true })).toBeTruthy()
      if (usage === 'Unknown') {
        expect(within(directory).queryByRole('progressbar')).toBeNull()
        expect(directory.textContent).not.toContain('0%')
      } else {
        expect(within(directory).getByRole('progressbar').getAttribute('aria-valuenow')).toBe(String(parseFloat(usage)))
      }
    } finally {
      await act(async () => {
        window.history.pushState({}, '', '/')
        window.dispatchEvent(new PopStateEvent('popstate'))
      })
      await screen.findByRole('region', { name: 'Home' })
    }
  })

  it('renders the production public Node Detail continuous reading contract', async () => {
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
        consensusState: 'ok',
        processState: 'ok',
        resyncState: 'normal',
        networkReferenceConfidence: 'unknown',
        currentHead: 123,
        latestBlockTransactionCount: 4,
        historicalHighWatermark: 128,
        networkReferenceHead: null,
        processCpuPercent: 12.5,
        processMemoryPercent: 6.25,
        processStartedAt: '2026-08-19T22:58:00Z',
        processUptimeMs: 3_720_000,
        lastReportAt: '2026-08-20T00:00:05Z',
        nodeDataDirectorySizeBytes: 2_147_483_648,
        nodeDataDirectoryCapacityBytes: 8_589_934_592,
        hostNetworkRxBytesPerSec: 4096,
        hostNetworkTxBytesPerSec: 2048,
        consensus: {
          state: 'ok',
          freshness: 'current',
          highestQcBlock: 122,
          highestLockBlock: 121,
          highestCommitBlock: 120,
          validator: true,
        },
        validator: {
          activity: 'producing',
          activityState: 'current',
        },
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
      '/api/public/v1/nodes/node-1/history?limit=2': () => jsonResponse([{
        nodeId: 'node-1',
        height: 123,
        blockTimeMs: 1_755_638_400_000,
        transactionCount: 4,
        observedAt: '2026-08-20T00:00:00Z',
      }, {
        nodeId: 'node-1',
        height: 122,
        blockTimeMs: 1_755_638_398_000,
        transactionCount: 3,
        observedAt: '2026-08-19T23:59:58Z',
      }, {
        nodeId: 'node-1',
        height: null,
        gapFromHeight: 120,
        gapToHeight: 121,
        gapKind: 'unrecoverable_backfill',
        gapReason: 'history interval unavailable',
        observedAt: '2026-08-20T00:01:00Z',
      }], 200),
      '/api/public/v1/nodes/node-1/metrics': () => jsonResponse({
        from: '2026-08-19T23:59:00Z',
        to: '2026-08-20T00:00:00Z',
        windowSeconds: 60,
        processCpuPercent: [{ sampledAt: '2026-08-19T23:59:00Z', value: 10 }, { sampledAt: '2026-08-20T00:00:00Z', value: 12.5 }],
        processMemoryPercent: [{ sampledAt: '2026-08-19T23:59:00Z', value: 20 }, { sampledAt: '2026-08-20T00:00:00Z', value: 25 }],
        dataDirectoryPercent: [{ sampledAt: '2026-08-19T23:59:00Z', value: 45 }, { sampledAt: '2026-08-20T00:00:00Z', value: 50 }],
        networkRxBytesPerSec: [{ sampledAt: '2026-08-19T23:59:00Z', value: 2048 }, { sampledAt: '2026-08-20T00:00:00Z', value: 4096 }],
        networkTxBytesPerSec: [{ sampledAt: '2026-08-19T23:59:00Z', value: 1024 }, { sampledAt: '2026-08-20T00:00:00Z', value: 2048 }],
        peerInboundCount: [{ sampledAt: '2026-08-19T23:59:00Z', value: 7 }, { sampledAt: '2026-08-20T00:00:00Z', value: 8 }],
        peerOutboundCount: [{ sampledAt: '2026-08-19T23:59:00Z', value: 3 }, { sampledAt: '2026-08-20T00:00:00Z', value: 4 }],
        blockIntervalMs: [{ sampledAt: '2026-08-19T23:59:00Z', value: 1800 }, { sampledAt: '2026-08-20T00:00:00Z', value: 2000 }],
        transactionCount: [{ sampledAt: '2026-08-19T23:59:00Z', value: 3 }, { sampledAt: '2026-08-20T00:00:00Z', value: 4 }],
      }, 200),
      '/api/public/v1/nodes/node-1/peer-history': () => jsonResponse({
        state: 'ok',
        freshness: 'current',
        fiveMinute: [],
        hourly: [],
      }, 200),
    })

    render(<App />)
    await screen.findByRole('region', { name: 'Home' })
    expect(screen.queryByRole('navigation', { name: 'Prototype variants' })).toBeNull()

    for (const variant of ['signal-stack', 'mission-control', 'evidence-ledger']) {
      await act(async () => {
        window.history.pushState({}, '', `/?variant=${variant}`)
        window.dispatchEvent(new PopStateEvent('popstate'))
        await Promise.resolve()
      })
      expect(screen.getByRole('region', { name: 'Home' })).toBeTruthy()
      expect(screen.queryByRole('navigation', { name: 'Prototype variants' })).toBeNull()
    }

    await act(async () => {
      window.history.pushState({}, '', '/nodes/node-1?variant=signal-stack')
      window.dispatchEvent(new PopStateEvent('popstate'))
      await Promise.resolve()
    })

    expect(await screen.findByRole('heading', { level: 1, name: 'Validator A' })).toBeTruthy()
    expect(screen.getByRole('img', { name: 'Unhealthy' })).toBeTruthy()
    // Node Validator Activity is not rendered by the current SPA, so the
    // identity block carries no Node-status label and no activity wording.
    expect(screen.queryByText('Node status')).toBeNull()
    expect(screen.queryByText('Producing')).toBeNull()
    expect(screen.getByText('Process uptime')).toBeTruthy()
    expect(screen.getByText('1h 2m')).toBeTruthy()
    expect(screen.getByText('Head')).toBeTruthy()
    expect(screen.getByText('QC')).toBeTruthy()
    expect(screen.getByText('Locked')).toBeTruthy()
    expect(screen.getByText('Committed')).toBeTruthy()
    expect(screen.getByText('Validator')).toBeTruthy()
    expect(screen.getByText('True')).toBeTruthy()
    expect(screen.queryByText('False')).toBeNull()
    expect(screen.getByText('RPC observation failed')).toBeTruthy()
    expect(screen.getByText('Started')).toBeTruthy()
    expect(screen.getByText('Last report')).toBeTruthy()
    const summary = screen.getByLabelText('Node key summary')
    expect(summary.textContent).toContain('Head')
    expect(summary.textContent).toContain('Sync')
    expect(summary.textContent).toContain('Peers')
    expect(summary.textContent).toContain('Process uptime')
    const processGroup = screen.getByLabelText('PlatON process resources')
    expect(processGroup.textContent).toContain('CPU')
    expect(processGroup.textContent).toContain('12.5%')
    expect(processGroup.textContent).toContain('Memory')
    expect(processGroup.textContent).toContain('6.3%')
    // The accepted A container merges PlatON process resources and the Node
    // Data directory into one panel, so the panel carries all three tracks
    // while the Node Data directory keeps its own labelled region.
    expect(processGroup.querySelectorAll('[data-slot="progress-thin"]')).toHaveLength(3)
    const nodeDataGroup = screen.getByLabelText('Node data directory')
    expect(processGroup.contains(nodeDataGroup)).toBe(true)
    expect(nodeDataGroup.textContent).toContain('Directory usage')
    expect(nodeDataGroup.textContent).toContain('25.0%')
    expect(nodeDataGroup.textContent).toContain('2.00 GiB / 8.00 GiB')
    expect(nodeDataGroup.querySelectorAll('[data-slot="progress-thin"]')).toHaveLength(1)
    const hostGroup = screen.getByLabelText('Shared Host resources')
    expect(hostGroup.textContent).toContain('Host CPU')
    expect(hostGroup.textContent).toContain('Host upload')
    expect(hostGroup.textContent).toContain('shared by every Node')

    // The container contract is the accepted A calibration rather than the
    // earlier single-hero-card composition: an uncarded identity block, four
    // summary tiles, and three parallel observation panels.
    expect(document.querySelector('[data-slot="node-hero-card"]')).toBeNull()
    expect(summary.querySelectorAll('[data-slot="node-summary-tile"]')).toHaveLength(4)
    expect(document.querySelectorAll('[data-slot="node-info-group"]')).toHaveLength(3)
    for (const title of ['Chain & consensus', 'PlatON process & Node Data', /^Host resources/]) {
      expect(screen.getByRole('heading', { level: 2, name: title })).toBeTruthy()
    }
    // The final six-chart order is process CPU %, process memory %, shared
    // Host upload/download, Peer inbound/outbound, block interval, then
    // transactions per block; the first four are lines and the last two bars.
    for (const heading of ['Process CPU', 'Process memory', 'Host network', 'Peer connections', 'Block interval', 'Transactions per block']) {
      expect(screen.getByRole('heading', { level: 3, name: heading })).toBeTruthy()
    }
    expect(screen.getAllByText('2.00 KiB/s').length).toBeGreaterThan(0)
    expect(screen.getAllByText('4.00 KiB/s').length).toBeGreaterThan(0)
    const networkLegend = screen.getByLabelText('Host network chart legend')
    expect(networkLegend.textContent).toContain('Upload')
    expect(networkLegend.textContent).toContain('Download')
    const connectionsLegend = screen.getByLabelText('Peer connections chart legend')
    expect(connectionsLegend.textContent).toContain('Inbound')
    expect(connectionsLegend.textContent).toContain('Outbound')
    expect(screen.getAllByText('12.5%').length).toBeGreaterThan(0)
    expect(screen.getByText('2.00 s')).toBeTruthy()
    expect(screen.getByRole('heading', { level: 2, name: 'Latest 60 seconds' })).toBeTruthy()
    expect(screen.getAllByRole('img', { name: /line chart over the last 60 seconds/ })).toHaveLength(4)
    expect(screen.getAllByRole('img', { name: /bar chart over the last 60 seconds/ })).toHaveLength(2)
    expect(screen.getAllByText('60s')).toHaveLength(6)
    // The accepted resource panels are now the only progress bars on the page,
    // and each one names the value it tracks. The removed bounded-history
    // widget's bar must stay gone.
    const tracks = screen.getAllByRole('progressbar')
    expect(tracks.length).toBeGreaterThan(0)
    for (const track of tracks) expect(track.getAttribute('aria-label')).toBeTruthy()
    expect(screen.queryByText('Bounded Block History')).toBeNull()
    expect(screen.queryByRole('button', { name: 'Export public history' })).toBeNull()
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

    // Continuous reading replaces the Details/Network tabs. Peer diagnostics
    // and low-frequency technical details are keyboard-operable disclosures.
    expect(screen.queryByRole('tab')).toBeNull()
    const peerDisclosure = openPeerDisclosure()
    expect(screen.getByRole('heading', { name: 'Peer history' })).toBeTruthy()
    expect(peerDisclosure.hasAttribute('open')).toBe(true)
    const technicalDisclosure = screen.getByText('Identifiers and technical details').closest('details')
    if (!technicalDisclosure) throw new Error('Technical details disclosure is missing')
    expect(within(technicalDisclosure).getByText('Reference confidence')).toBeTruthy()
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
    expect(await screen.findByRole('region', { name: 'Home' })).toBeTruthy()
    const brand = screen.getByRole('link', { name: 'PlatPulse' })
    expect(brand.querySelector('img')?.getAttribute('src')).toContain('platpulse-mark')
    expect(brand.textContent).toBe('PlatPulse')
    const adminLink = screen.getByRole('link', { name: 'Admin' })
    // Icon-only like the Emerald reference, but still named for assistive
    // technology and still a full-size target.
    expect(adminLink.textContent).toBe('')
    expect(adminLink.getAttribute('aria-label')).toBe('Admin')
    expect(adminLink.querySelector('svg')).toBeTruthy()
  })

  it('renders the unified Admin brand link and retained header actions', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/public/v1/networks': () => jsonResponse([], 200),
    })

    render(<App />)
    await screen.findByRole('region', { name: 'Home' })
    await goToAdmin()
    await screen.findByRole('heading', { level: 1, name: 'Overview' })

    try {
      const brand = screen.getByRole('link', { name: 'PlatPulse' })
      expect(brand.getAttribute('href')).toBe('/')
      expect(brand.querySelector('img')?.getAttribute('src')).toContain('platpulse-mark')
      expect(brand.querySelector('img')?.getAttribute('alt')).toBe('')
      expect(screen.queryByRole('link', { name: 'Home' })).toBeNull()
      expect(screen.getByRole('button', { name: 'Sign out' })).toBeTruthy()
      expect(screen.getByRole('link', { name: 'Overview' }).getAttribute('aria-current')).toBe('page')
    } finally {
      await act(async () => {
        window.history.pushState({}, '', '/')
        window.dispatchEvent(new PopStateEvent('popstate'))
        await Promise.resolve()
      })
      await screen.findByRole('region', { name: 'Home' })
    }
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
    await screen.findByRole('region', { name: 'Home' })
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
    await screen.findByRole('region', { name: 'Home' })
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
    await screen.findByRole('region', { name: 'Home' })
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
    const nodePayload: PublicNode = {
      nodeId: 'node-1',
      displayName: 'Validator A',
      networkKey: 'mainnet',
      health: 'healthy',
      healthReason: 'rpc reachable',
      freshness: 'current',
      rpcState: 'ok',
      syncState: 'synced',
      consensusState: 'current',
      consensus: { state: 'ok', freshness: 'current' },
      processState: 'running',
      resyncState: 'idle',
      currentHead: 123,
      historicalHighWatermark: 120,
      networkReferenceHead: 123,
      networkReferenceConfidence: 'high',
      hostCpuPercent: 42.5,
      resyncProgress: null,
      peers: { state: 'ok', freshness: 'current', peerCount: 3 },
      validator: null,
    }
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/public/v1/networks': () => jsonResponse([{
        networkKey: 'mainnet',
        displayName: 'Mainnet',
        nodes: [nodePayload],
      }], 200),
      '/api/public/v1/nodes/node-1': () => jsonResponse(nodePayload, 200),
    })

    render(<App />)
    const homeLink = await screen.findByRole('link', { name: 'PlatPulse' })
    fireEvent.click(homeLink)
    expect(await screen.findByRole('region', { name: 'Home' })).toBeTruthy()
    // The whole-card Node link names the Node; the Network stays plain text.
    const nodeCard = await screen.findByRole('link', { name: /Validator A/ })
    expect(nodeCard.getAttribute('href')).toBe('/nodes/node-1')
    expect(nodeCard.textContent).toContain('Mainnet')
    expect(screen.queryByRole('link', { name: 'Mainnet' })).toBeNull()
    expect(screen.getByRole('img', { name: 'Healthy' })).toBeTruthy()
    fireEvent.click(nodeCard)
    expect(await screen.findByRole('heading', { level: 1, name: 'Validator A' })).toBeTruthy()
    fireEvent.click(screen.getByRole('link', { name: /All Networks/ }))
    expect(await screen.findByRole('region', { name: 'Home' })).toBeTruthy()
  })

  it('redirects the removed public Network route to Home', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/public/v1/networks': () => jsonResponse([], 200),
    })

    render(<App />)
    await act(async () => {
      window.history.pushState({}, '', '/networks/mainnet')
      window.dispatchEvent(new PopStateEvent('popstate'))
      await Promise.resolve()
    })

    expect(await screen.findByRole('region', { name: 'Home' })).toBeTruthy()
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
      '/api/public/v1/nodes/node-1/history?limit=2': () => jsonResponse([], 200),
      '/api/public/v1/nodes/node-1/metrics': () => jsonResponse({
        from: '2026-08-19T23:59:00Z', to: '2026-08-20T00:00:00Z', windowSeconds: 60,
        processCpuPercent: [], processMemoryPercent: [], dataDirectoryPercent: [],
        networkRxBytesPerSec: [], networkTxBytesPerSec: [], peerInboundCount: [], peerOutboundCount: [],
        blockIntervalMs: [], transactionCount: [],
      }, 200),
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
    expect(screen.getAllByRole('img', { name: /line chart over the last 60 seconds/ })).toHaveLength(4)
    expect(screen.getAllByRole('img', { name: /bar chart over the last 60 seconds/ })).toHaveLength(2)
    expect(screen.getAllByText('No samples in the last minute')).toHaveLength(6)
    openPeerDisclosure()
    expect(screen.getByRole('heading', { name: 'Peer history' })).toBeTruthy()

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
    expect(screen.getByRole('heading', { name: 'Peer history' })).toBeTruthy()
    expect(screen.getByText(/last successful Node data/i)).toBeTruthy()
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
              nodes: { total: 1, active: 1, healthy: 1, unhealthy: 0, unknown: 0, retired: 0, published: 1 },
        networks: { total: 1, with_identity_mismatch: 0 },
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

  it('refetches authoritative Admin REST after SSE reconnect without clearing the page', async () => {
    let overviewCalls = 0
    const fetchMock = mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/overview': () => {
        overviewCalls += 1
        return jsonResponse({
          generated_at: '2026-08-12T00:00:00Z',
          summary: {
            agents: { total: 0, online: 0, offline: 0, unknown: 0 },
            nodes: { total: 0, active: 0, healthy: 0, unhealthy: 0, unknown: 0, retired: 0, published: 0 },
            networks: { total: 0, with_identity_mismatch: 0 },
          },
          attention: [],
        }, 200)
      },
      '/api/admin/v1/nodes': () => jsonResponse([], 200),
      '/api/admin/v1/agents': () => jsonResponse([], 200),
    })
    vi.stubGlobal('EventSource', FakeEventSource)
    render(<App />)
    await goToAdmin()
    await screen.findByRole('heading', { level: 1, name: 'Overview' })
    await waitFor(() => expect(overviewCalls).toBe(1))
    const stream = FakeEventSource.latest
    expect(stream).toBeTruthy()

    await act(async () => stream?.onopen?.())
    await act(async () => stream?.onerror?.())
    expect(await screen.findByText('Live updates paused')).toBeTruthy()
    await act(async () => stream?.onopen?.())
    await waitFor(() => expect(overviewCalls).toBe(2))
    expect(screen.getByRole('heading', { level: 1, name: 'Overview' })).toBeTruthy()
    expect(fetchMock).toHaveBeenCalled()
  })

  it('refetches Network Registry surfaces and Overview after a Network invalidation', async () => {
    let overviewCalls = 0
    let networkCalls = 0
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/overview': () => {
        overviewCalls += 1
        return jsonResponse(
          {
            generated_at: '2026-08-12T00:00:00Z',
            summary: {
              agents: { total: 0, online: 0, offline: 0, unknown: 0 },
              nodes: { total: 0, active: 0, healthy: 0, unhealthy: 0, unknown: 0, retired: 0, published: 0 },
              networks: { total: 0, with_identity_mismatch: 0 },
            },
            attention: [],
          },
          200,
        )
      },
      '/api/admin/v1/nodes': () => jsonResponse([], 200),
      '/api/admin/v1/agents': () => jsonResponse([], 200),
      '/api/admin/v1/networks': () => {
        networkCalls += 1
        return jsonResponse([], 200)
      },
    })
    vi.stubGlobal('EventSource', FakeEventSource)

    render(<App />)
    await goToAdmin()
    await screen.findByRole('heading', { level: 1, name: 'Overview' })
    await waitFor(() => expect(overviewCalls).toBe(1))

    await act(async () => {
      window.history.pushState({}, '', '/admin/networks')
      window.dispatchEvent(new PopStateEvent('popstate'))
      await Promise.resolve()
    })
    await screen.findByRole('heading', { level: 1, name: 'Networks' })
    await waitFor(() => expect(networkCalls).toBe(1))

    await act(async () => {
      FakeEventSource.latest?.emit(
        'invalidation',
        JSON.stringify({
          version: 1,
          eventId: 3,
          resource: 'network',
          resourceId: 'platon-mainnet',
          revision: 3,
        }),
      )
      await Promise.resolve()
    })
    await waitFor(() => expect(networkCalls).toBe(2))

    await goToAdmin()
    await screen.findByRole('heading', { level: 1, name: 'Overview' })
    await waitFor(() => expect(overviewCalls).toBe(2))
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
                nodes: { total: 1, active: 1, healthy: 1, unhealthy: 0, unknown: 0, retired: 0, published: 1 },
        networks: { total: 1, with_identity_mismatch: 0 },
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
        nodes: { total: 1, active: 1, healthy: 1, unhealthy: 0, unknown: 0, retired: 0, published: 1 },
        networks: { total: 1, with_identity_mismatch: 0 },
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
      expect(screen.getByRole('region', { name: 'Home' })).toBeTruthy(),
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
    await screen.findByRole('region', { name: 'Home' })
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
      expect(screen.getByRole('region', { name: 'Home' })).toBeTruthy(),
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

  // Complete MVP Admin inventory (issues #92 and #111): Overview, Agents,
  // Agent Detail, Nodes, Node Detail, Networks, Network Detail, Settings,
  // Sessions and Audit. Each route renders its own page shell
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
    ['/admin/networks/no-such-network', /Network unavailable/],
    ['/admin/settings', /Settings/],
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
    await screen.findByRole('region', { name: 'Home' })
    await renderAt('/admin')
    await screen.findByRole('heading', { level: 1, name: 'Overview' })

    for (const [path, heading] of MVP_ROUTES) {
      await renderAt(path)
      await screen.findByRole('heading', { level: 1, name: heading })

      // Every retained Admin route shares the same accessible shell. Parent
      // collection links stay current on detail routes so navigation remains
      // oriented without coupling this contract to CSS classes.
      expect(screen.getByRole('main')).toBeTruthy()
      const adminNav = screen.getByRole('navigation', { name: 'Admin' })
      expect(within(adminNav).getAllByRole('link')).toHaveLength(7)
      const activeHref = path.startsWith('/admin/agents')
        ? '/admin/agents'
        : path.startsWith('/admin/nodes')
          ? '/admin/nodes'
          : path.startsWith('/admin/networks')
            ? '/admin/networks'
            : path.startsWith('/admin/access/sessions')
              ? '/admin/access/sessions'
              : path.startsWith('/admin/access/audit')
                ? '/admin/access/audit'
                : path === '/admin/settings'
                  ? '/admin/settings'
                  : '/admin'
      const activeLabel = {
        '/admin': 'Overview',
        '/admin/agents': 'Agents',
        '/admin/nodes': 'Nodes',
        '/admin/networks': 'Networks',
        '/admin/settings': 'Settings',
        '/admin/access/sessions': 'Sessions',
        '/admin/access/audit': 'Audit',
      }[activeHref]
      expect(within(adminNav).getByRole('link', { name: activeLabel }).getAttribute('aria-current')).toBe(
        'page',
      )
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
    ['/admin/history-window', /History Window/],
    ['/admin/site-access', /Site Access/],
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
    await screen.findByRole('region', { name: 'Home' })
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
    await screen.findByRole('region', { name: 'Home' })
    await renderAt('/admin')
    await screen.findByRole('heading', { level: 1, name: 'Overview' })

    const adminNav = screen.getByRole('navigation', { name: 'Admin' })
    // Each retained page group carries one decorative leading SVG icon. The glyph
    // is aria-hidden, so the accessible name stays the page-group label while
    // the visible icon leads the text (webui.md §10.1).
    const expectedLinks = [
      { name: 'Overview', href: '/admin', glyph: 'LayoutDashboard' },
      { name: 'Agents', href: '/admin/agents', glyph: 'Cpu' },
      { name: 'Nodes', href: '/admin/nodes', glyph: 'Server' },
      { name: 'Networks', href: '/admin/networks', glyph: 'Network' },
      { name: 'Settings', href: '/admin/settings', glyph: 'Settings' },
      { name: 'Sessions', href: '/admin/access/sessions', glyph: 'PanelsTopLeft' },
      { name: 'Audit', href: '/admin/access/audit', glyph: 'ListChecks' },
    ]
    expect(within(adminNav).getAllByRole('link')).toHaveLength(expectedLinks.length)
    for (const { name, href, glyph } of expectedLinks) {
      const link = within(adminNav).getByRole('link', { name })
      expect(link.getAttribute('href')).toBe(href)
      expect(link.textContent?.trim()).toBe(name)
      const icon = link.querySelector('[data-slot="admin-nav-icon"]')
      expect(icon?.getAttribute('aria-hidden')).toBe('true')
      expect(icon?.querySelector('svg')?.getAttribute('data-icon')).toBe(glyph)
    }
    for (const removed of ['History Window', 'Site Access', 'Validators', 'People', 'Alert Rules', 'Incidents', 'Silences', 'Maintenance', 'Deliveries', 'Channels', 'Operations', 'Data', 'Retention', 'Backups', 'Restore', 'Doctor', 'Enroll', 'Recover', 'Rotate']) {
      expect(
        Array.from(adminNav.querySelectorAll('a')).some((element) =>
          element.textContent?.includes(removed),
        ),
        'removed page ' + removed + ' must not be linked from Admin navigation',
      ).toBe(false)
    }
  })
})

describe('Theme lifecycle (issue #146)', () => {
  const THEME_KEY = 'platpulse.themeMode'
  let system: { matches: boolean; listeners: Set<() => void> }
  let storage: Storage

  /** jsdom in this setup has no localStorage; install a controllable one. */
  function installLocalStorage() {
    const map = new Map<string, string>()
    const fake = {
      get length() {
        return map.size
      },
      clear: () => map.clear(),
      getItem: (key: string) => (map.has(key) ? map.get(key)! : null),
      key: (index: number) => Array.from(map.keys())[index] ?? null,
      removeItem: (key: string) => {
        map.delete(key)
      },
      setItem: (key: string, value: string) => {
        map.set(key, value)
      },
    }
    Object.defineProperty(window, 'localStorage', { value: fake, configurable: true, writable: true })
    return fake as Storage
  }

  function installSystemTheme(initialDark: boolean) {
    system = { matches: initialDark, listeners: new Set() }
    vi.stubGlobal(
      'matchMedia',
      vi.fn(() => ({
        matches: system.matches,
        media: '(prefers-color-scheme: dark)',
        onchange: null,
        addEventListener: (_type: string, listener: () => void) => system.listeners.add(listener),
        removeEventListener: (_type: string, listener: () => void) => system.listeners.delete(listener),
        addListener: (listener: () => void) => system.listeners.add(listener),
        removeListener: (listener: () => void) => system.listeners.delete(listener),
        dispatchEvent: () => true,
      })),
    )
  }

  function changeSystemTheme(dark: boolean) {
    system.matches = dark
    for (const listener of [...system.listeners]) listener()
  }

  function themeButton() {
    return screen.getByRole('button', { name: /^Theme: / })
  }

  async function renderLogin() {
    mockFetch({ '/api/public/v1/session': () => errorBody('auth_required') })
    render(<App />)
    await screen.findByRole('heading', { level: 1, name: 'Sign in to PlatPulse' })
  }

  beforeEach(() => {
    storage = installLocalStorage()
    // The Site Access cache lives at module scope; force the deterministic
    // private baseline so an unauthenticated render always reaches Login.
    applySiteAccessSettings({ mode: 'private', authorizationGeneration: 0 })
    // The router is created once at module scope; sync it back to Home so a
    // previous test's Admin route cannot leak into this one.
    act(() => {
      window.history.replaceState({}, '', '/')
      window.dispatchEvent(new PopStateEvent('popstate'))
    })
    document.documentElement.classList.remove('dark')
    document.documentElement.removeAttribute('data-theme-mode')
    document.documentElement.style.colorScheme = ''
    installSystemTheme(false)
  })

  it('shows the Login brand header without authenticated controls', async () => {
    await renderLogin()
    const header = screen.getByRole('banner')
    const brand = screen.getByRole('link', { name: 'PlatPulse' })
    expect(header.contains(brand)).toBe(true)
    expect(brand.getAttribute('href')).toBe('/')
    expect(brand.querySelector('img')?.getAttribute('src')).toContain('platpulse-mark')
    expect(brand.querySelector('img')?.getAttribute('alt')).toBe('')
    expect(header.contains(themeButton())).toBe(true)
    expect(screen.queryByRole('link', { name: /^Admin$/ })).toBeNull()
    expect(screen.getAllByRole('main')).toHaveLength(1)
  })

  it('cycles Auto → Light → Dark → Auto, paints the document, and persists', async () => {
    await renderLogin()
    expect(themeButton().getAttribute('aria-label')).toBe('Theme: Auto. Switch to Light')
    expect(document.documentElement.classList.contains('dark')).toBe(false)
    expect(document.documentElement.style.colorScheme).toBe('light')

    fireEvent.click(themeButton())
    expect(themeButton().getAttribute('aria-label')).toBe('Theme: Light. Switch to Dark')
    expect(storage.getItem(THEME_KEY)).toBe('light')

    fireEvent.click(themeButton())
    expect(themeButton().getAttribute('aria-label')).toBe('Theme: Dark. Switch to Auto')
    expect(document.documentElement.classList.contains('dark')).toBe(true)
    expect(document.documentElement.style.colorScheme).toBe('dark')
    expect(storage.getItem(THEME_KEY)).toBe('dark')

    fireEvent.click(themeButton())
    expect(themeButton().getAttribute('aria-label')).toBe('Theme: Auto. Switch to Light')
    expect(storage.getItem(THEME_KEY)).toBe('auto')
  })

  it('follows system changes in Auto and never overrides an explicit choice', async () => {
    await renderLogin()
    expect(document.documentElement.classList.contains('dark')).toBe(false)

    await act(async () => changeSystemTheme(true))
    expect(document.documentElement.classList.contains('dark')).toBe(true)

    await act(async () => changeSystemTheme(false))
    expect(document.documentElement.classList.contains('dark')).toBe(false)

    // An explicit Light choice ignores later system changes.
    fireEvent.click(themeButton())
    await act(async () => changeSystemTheme(true))
    expect(themeButton().getAttribute('aria-label')).toBe('Theme: Light. Switch to Dark')
    expect(document.documentElement.classList.contains('dark')).toBe(false)
  })

  it('defaults an invalid preference to Auto and still switches with unavailable storage', async () => {
    storage.setItem(THEME_KEY, 'sepia')
    await renderLogin()
    expect(themeButton().getAttribute('aria-label')).toBe('Theme: Auto. Switch to Light')

    const blocked = () => {
      throw new Error('storage blocked')
    }
    storage.getItem = blocked
    storage.setItem = blocked
    fireEvent.click(themeButton())
    expect(themeButton().getAttribute('aria-label')).toBe('Theme: Light. Switch to Dark')
    fireEvent.click(themeButton())
    expect(document.documentElement.classList.contains('dark')).toBe(true)
  })

  it('keeps the resolved theme across Home and Admin navigation', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/public/v1/networks': () => jsonResponse([], 200),
    })
    render(<App />)
    await screen.findByRole('region', { name: 'Home' })
    fireEvent.click(themeButton())
    fireEvent.click(themeButton())
    expect(document.documentElement.classList.contains('dark')).toBe(true)

    await goToAdmin()
    await screen.findByRole('heading', { level: 1, name: 'Overview' })
    expect(document.documentElement.classList.contains('dark')).toBe(true)
    expect(themeButton().getAttribute('aria-label')).toBe('Theme: Dark. Switch to Auto')
  })

  describe('Admin dual theme (issue #148)', () => {
    /** Every retained Admin route and the page heading that proves it rendered
     *  without depending on any route-specific fixture. */
    const ADMIN_ROUTES: Array<{ path: string; heading: string }> = [
      { path: '/admin', heading: 'Overview' },
      { path: '/admin/agents', heading: 'Agents' },
      { path: '/admin/nodes', heading: 'Nodes' },
      { path: '/admin/networks', heading: 'Networks' },
      { path: '/admin/settings', heading: 'Settings' },
      { path: '/admin/access/sessions', heading: 'Sessions' },
      { path: '/admin/access/audit', heading: 'Audit log' },
    ]

    async function navigateTo(path: string) {
      await act(async () => {
        window.history.pushState({}, '', path)
        window.dispatchEvent(new PopStateEvent('popstate'))
        await Promise.resolve()
      })
    }

    /** Sign in as Owner, resolve Dark through the production control, and
     *  land on the Admin Overview. */
    async function renderAdminInDark() {
      mockFetch({
        '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
        '/api/public/v1/networks': () => jsonResponse([], 200),
      })
      render(<App />)
      await screen.findByRole('region', { name: 'Home' })
      fireEvent.click(themeButton())
      fireEvent.click(themeButton())
      expect(document.documentElement.classList.contains('dark')).toBe(true)
      await goToAdmin()
      await screen.findByRole('heading', { level: 1, name: 'Overview' })
    }

    it('keeps every retained Admin route on the resolved theme without the public decoration', async () => {
      await renderAdminInDark()

      // The workbench is quiet and undecorated: the Admin shell owns its own
      // navigation, and the public background grid/Home region never leak in.
      expect(document.querySelector('.background-decoration')).toBeNull()
      expect(screen.getByRole('navigation', { name: 'Admin' })).toBeTruthy()
      expect(screen.queryByRole('region', { name: 'Home' })).toBeNull()

      for (const route of ADMIN_ROUTES) {
        await navigateTo(route.path)
        expect(
          await screen.findByRole('heading', { level: 1, name: route.heading }),
        ).toBeTruthy()
        expect(
          document.documentElement.classList.contains('dark'),
          route.path + ' stays on the resolved theme',
        ).toBe(true)
        expect(document.querySelector('.background-decoration'), route.path).toBeNull()
        expect(document.querySelector('[data-slot="admin-shell"]'), route.path).not.toBeNull()
      }

      // One click from Dark reaches Auto, which resolves Light under the test
      // system preference; a second click selects explicit Light. The retained
      // route stays mounted and the workbench keeps no public grid.
      fireEvent.click(themeButton())
      expect(themeButton().getAttribute('aria-label')).toBe('Theme: Auto. Switch to Light')
      expect(document.documentElement.classList.contains('dark')).toBe(false)
      fireEvent.click(themeButton())
      expect(themeButton().getAttribute('aria-label')).toBe('Theme: Light. Switch to Dark')
      expect(document.documentElement.classList.contains('dark')).toBe(false)
      expect(screen.getByRole('heading', { level: 1, name: 'Audit log' })).toBeTruthy()
    })

    it('keeps the Owner-only boundary and Home isolation on the resolved theme', async () => {
      // A Viewer resolves the same theme but never enters the Admin shell.
      mockFetch({
        '/api/public/v1/session': () => jsonResponse(VIEWER_SESSION, 200),
        '/api/public/v1/networks': () => jsonResponse([], 200),
      })
      render(<App />)
      await screen.findByRole('region', { name: 'Home' })
      fireEvent.click(themeButton())
      fireEvent.click(themeButton())
      expect(document.documentElement.classList.contains('dark')).toBe(true)

      await goToAdmin()
      expect(
        await screen.findByRole('heading', { level: 1, name: 'Owner access required' }),
      ).toBeTruthy()
      expect(document.querySelector('[data-slot="admin-shell"]')).toBeNull()

      // Home keeps its single Admin entry link and never adopts the Admin nav.
      await navigateTo('/')
      await screen.findByRole('region', { name: 'Home' })
      expect(screen.queryByRole('link', { name: 'Admin' })).toBeNull()
      expect(screen.queryByRole('navigation', { name: 'Admin' })).toBeNull()
      expect(document.documentElement.classList.contains('dark')).toBe(true)
    })
  })
})
