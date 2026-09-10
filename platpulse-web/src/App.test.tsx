import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { onlineManager } from '@tanstack/react-query'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import App from './App'
import { adminQueryClient, resetAdminCache } from './api/admin'
import { resetPublicCache } from './api/public'
import { resetRealtimeCursors } from './api/transport'
import { client } from './api/generated/client.gen'
import type { PublicGeoInsight, PublicNode } from './api/generated/types.gen'

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

type PublicPeerCase = {
  name: string
  peers: {
    state: string
    freshness: string
    staleSince?: string
    peerCount: number | null
    inboundCount: number | null
    outboundCount: number | null
    trustedCount: number | null
    staticCount: number | null
    consensusCount: number | null
  }
  expectedCollection: string | RegExp | null
  expectedFreshness: string | null
  expectedCount: string | null
  expectedCurrent: boolean
  expectedNote?: RegExp
}

const PUBLIC_PEER_CASES: PublicPeerCase[] = [
  {
    name: 'successful non-zero data',
    peers: { state: 'ok', freshness: 'current', peerCount: 4, inboundCount: 1, outboundCount: 3, trustedCount: 2, staticCount: 1, consensusCount: 2 },
    expectedCollection: null,
    expectedFreshness: null,
    expectedCount: '4',
    expectedCurrent: true,
  },
  {
    name: 'successful fresh empty data',
    peers: { state: 'ok', freshness: 'current', peerCount: 0, inboundCount: 0, outboundCount: 0, trustedCount: 0, staticCount: 0, consensusCount: 0 },
    expectedCollection: null,
    expectedFreshness: null,
    expectedCount: '0',
    expectedCurrent: true,
    expectedNote: /authoritative empty snapshot/i,
  },
  {
    name: 'missing successful data after collection failure',
    peers: { state: 'error', freshness: 'unknown', peerCount: null, inboundCount: null, outboundCount: null, trustedCount: null, staticCount: null, consensusCount: null },
    expectedCollection: /Collection failed/,
    expectedFreshness: 'Unknown',
    expectedCount: null,
    expectedCurrent: false,
    expectedNote: /no successful Peer snapshot is available/i,
  },
  {
    name: 'failed collection with a fresh retained value',
    peers: { state: 'error', freshness: 'current', peerCount: 4, inboundCount: 1, outboundCount: 3, trustedCount: 2, staticCount: 1, consensusCount: 2 },
    expectedCollection: /Collection failed/,
    expectedFreshness: null,
    expectedCount: '4',
    expectedCurrent: false,
    expectedNote: /Showing last successful snapshot/,
  },
  {
    name: 'failed collection with a stale retained value',
    peers: { state: 'error', freshness: 'stale', staleSince: '2026-08-16T03:02:00Z', peerCount: 4, inboundCount: 1, outboundCount: 3, trustedCount: 2, staticCount: 1, consensusCount: 2 },
    expectedCollection: /Collection failed/,
    expectedFreshness: 'Stale',
    expectedCount: '4',
    expectedCurrent: false,
    expectedNote: /Showing last successful snapshot/,
  },
  {
    name: 'stale retained zero data',
    peers: { state: 'ok', freshness: 'stale', staleSince: '2026-08-16T03:02:00Z', peerCount: 0, inboundCount: 0, outboundCount: 0, trustedCount: 0, staticCount: 0, consensusCount: 0 },
    expectedCollection: null,
    expectedFreshness: 'Stale',
    expectedCount: '0',
    expectedCurrent: false,
    expectedNote: /Showing last successful snapshot/,
  },
  {
    name: 'known values with unknown freshness',
    peers: { state: 'ok', freshness: 'unknown', peerCount: 4, inboundCount: 1, outboundCount: 3, trustedCount: 2, staticCount: 1, consensusCount: 2 },
    expectedCollection: null,
    expectedFreshness: 'Unknown',
    expectedCount: '4',
    expectedCurrent: false,
    expectedNote: /Freshness unknown/i,
  },
  {
    name: 'disabled collection with retained values',
    peers: { state: 'disabled', freshness: 'current', peerCount: 2, inboundCount: 1, outboundCount: 1, trustedCount: 1, staticCount: 0, consensusCount: 1 },
    expectedCollection: 'Disabled',
    expectedFreshness: null,
    expectedCount: '2',
    expectedCurrent: false,
    expectedNote: /Collection disabled.*Showing last successful snapshot/i,
  },
  {
    name: 'unsupported collection with retained values',
    peers: { state: 'unsupported', freshness: 'stale', peerCount: 2, inboundCount: 1, outboundCount: 1, trustedCount: 1, staticCount: 0, consensusCount: 1 },
    expectedCollection: 'Unsupported',
    expectedFreshness: 'Stale',
    expectedCount: '2',
    expectedCurrent: false,
    expectedNote: /Collection unsupported.*Showing last successful snapshot/i,
  },
  {
    name: 'starting collection without a successful value',
    peers: { state: 'starting', freshness: 'unknown', peerCount: null, inboundCount: null, outboundCount: null, trustedCount: null, staticCount: null, consensusCount: null },
    expectedCollection: 'Starting',
    expectedFreshness: 'Unknown',
    expectedCount: null,
    expectedCurrent: false,
    expectedNote: /Collection starting.*No successful Peer snapshot is available/i,
  },
] as const

type PublicGeoCase = {
  name: string
  geo: PublicGeoInsight
  expectedStatus: string
  expectedContent?: RegExp
  expectedAttribution?: RegExp
  expectedCountry?: string
  expectedCount?: string
  expectedDetails?: RegExp[]
}

const GEO_ATTRIBUTION = 'This product includes GeoLite Data created by MaxMind, available from https://www.maxmind.com.'
const PUBLIC_GEO_CASES: PublicGeoCase[] = [
  {
    name: 'enabled country data',
    geo: {
      state: 'current',
      countries: [{ countryCode: 'US', count: 3, centroidLat: 37, centroidLon: -95 }],
      attribution: GEO_ATTRIBUTION,
    },
    expectedStatus: 'Current',
    expectedAttribution: /GeoLite Data created by MaxMind/i,
    expectedCountry: 'US',
    expectedCount: '3',
  },
  {
    name: 'enabled empty country data',
    geo: {
      state: 'current',
      countries: [],
      attribution: GEO_ATTRIBUTION,
    },
    expectedStatus: 'Current',
    expectedAttribution: /GeoLite Data created by MaxMind/i,
    expectedContent: /No country observations are available yet/i,
  },
  {
    name: 'without a country observation',
    geo: {
      state: 'unknown',
      countries: null,
      attribution: null,
    },
    expectedStatus: 'Unknown',
    expectedContent: /Country insight is Unknown; no usable Geo projection is available/i,
  },
  {
    name: 'error with retained country data',
    geo: {
      state: 'error',
      countries: [{ countryCode: 'US', count: 3 }],
      attribution: GEO_ATTRIBUTION,
      errorReason: 'Geo database is invalid',
      lastGoodAt: '2026-08-16T03:00:00Z',
      databaseAgeSeconds: 3600,
    },
    expectedStatus: 'Error',
    expectedAttribution: /GeoLite Data created by MaxMind/i,
    expectedCountry: 'US',
    expectedCount: '3',
    expectedContent: /Showing the last-good country projection/i,
    expectedDetails: [
      /Reason: Geo database is invalid/i,
      /Last good database load: 2026-08-16T03:00:00Z/i,
      /Database age: 1 hour/i,
    ],
  },
  {
    name: 'stale with retained country data',
    geo: {
      state: 'stale',
      countries: [{ countryCode: 'DE', count: 2 }],
      attribution: GEO_ATTRIBUTION,
      lastGoodAt: '2026-08-16T03:00:00Z',
      databaseAgeSeconds: 2678400,
      staleSince: '2026-08-17T03:00:00Z',
    },
    expectedStatus: 'Stale',
    expectedAttribution: /GeoLite Data created by MaxMind/i,
    expectedCountry: 'DE',
    expectedCount: '2',
    expectedContent: /Showing the last-good country projection/i,
    expectedDetails: [
      /Last good database load: 2026-08-16T03:00:00Z/i,
      /Database age: 31 days/i,
      /Stale since: 2026-08-17T03:00:00Z/i,
    ],
  },
]

function publicNetworkPayload(peers: (typeof PUBLIC_PEER_CASES)[number]['peers'], geo: PublicGeoInsight = { state: 'disabled' }) {
  return {
    networkKey: 'mainnet',
    displayName: 'Mainnet',
    nodes: [],
    peers,
    geo,
    validators: [],
  }
}

async function renderPublicNetwork(peers: (typeof PUBLIC_PEER_CASES)[number]['peers'], geo: PublicGeoInsight = { state: 'disabled' }) {
  mockFetch({
    '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
    '/api/public/v1/networks': () => jsonResponse([], 200),
    '/api/public/v1/networks/mainnet': () => jsonResponse(publicNetworkPayload(peers, geo), 200),
  })
  render(<App />)
  await act(async () => {
    window.history.pushState({}, '', '/networks/mainnet')
    window.dispatchEvent(new PopStateEvent('popstate'))
    await Promise.resolve()
  })
  return screen.findByRole('region', { name: 'Peer insight' })
}

it.each(PUBLIC_PEER_CASES)('renders $name through the public Network route', async ({ peers, expectedCollection, expectedFreshness, expectedCount, expectedCurrent, expectedNote }) => {
  const peerRegion = await renderPublicNetwork(peers)

  try {
    if (expectedCollection) expect(within(peerRegion).getByText(expectedCollection)).toBeTruthy()
    if (expectedFreshness) expect(within(peerRegion).getAllByText(expectedFreshness, { exact: true }).length).toBeGreaterThan(0)
    if (expectedCount) expect(within(peerRegion).getAllByText(expectedCount, { exact: true }).length).toBeGreaterThan(0)
    if (expectedNote) expect(within(peerRegion).getAllByText(expectedNote).length).toBeGreaterThan(0)
    if (expectedCurrent) {
      expect(within(peerRegion).getByText('Peer data current')).toBeTruthy()
      expect(within(peerRegion).queryByText('Unknown', { exact: true })).toBeNull()
    } else {
      expect(within(peerRegion).queryByText('Peer data current')).toBeNull()
    }
  } finally {
    await act(async () => {
      window.history.pushState({}, '', '/')
      window.dispatchEvent(new PopStateEvent('popstate'))
      await Promise.resolve()
    })
    await screen.findByRole('region', { name: 'Home' })
  }
})

it('renders server-disabled Geo as a neutral notice through the public Network route', async () => {
  const peerRegion = await renderPublicNetwork(PUBLIC_PEER_CASES[0].peers)

  try {
    expect(screen.getByText('Peer countries · Disabled by server', { exact: true })).toBeTruthy()
    expect(screen.queryByRole('region', { name: 'Peer countries' })).toBeNull()
    expect(screen.queryByRole('heading', { name: 'Peer countries' })).toBeNull()
    expect(screen.queryByText(/Country insight is Disabled/i)).toBeNull()
    expect(peerRegion).toBeTruthy()
  } finally {
    await act(async () => {
      window.history.pushState({}, '', '/')
      window.dispatchEvent(new PopStateEvent('popstate'))
      await Promise.resolve()
    })
    await screen.findByRole('region', { name: 'Home' })
  }
})

it.each(PUBLIC_GEO_CASES)('renders $name through the public Network route', async ({ geo, expectedStatus, expectedContent, expectedAttribution, expectedCountry, expectedCount, expectedDetails }) => {
  const peerRegion = await renderPublicNetwork(PUBLIC_PEER_CASES[0].peers, geo)

  try {
    const geoRegion = await screen.findByRole('region', { name: 'Peer countries' })
    expect(within(geoRegion).getByText(expectedStatus, { exact: true })).toBeTruthy()
    if (expectedContent) expect(within(geoRegion).getByText(expectedContent)).toBeTruthy()
    if (expectedCountry) expect(within(geoRegion).getByText(expectedCountry, { exact: true })).toBeTruthy()
    if (expectedCount) expect(within(geoRegion).getByText(expectedCount, { exact: true })).toBeTruthy()
    for (const detail of expectedDetails ?? []) expect(within(geoRegion).getByText(detail)).toBeTruthy()
    if (expectedAttribution) expect(within(geoRegion).getByText(expectedAttribution)).toBeTruthy()
    expect(screen.queryByText(/static centroid|37, -95/i)).toBeNull()
    expect(screen.queryByText('Peer countries · Disabled by server', { exact: true })).toBeNull()
    expect(peerRegion).toBeTruthy()
  } finally {
    await act(async () => {
      window.history.pushState({}, '', '/')
      window.dispatchEvent(new PopStateEvent('popstate'))
      await Promise.resolve()
    })
    await screen.findByRole('region', { name: 'Home' })
  }
})

it('reveals enabled country content after a public Network invalidation', async () => {
  vi.stubGlobal('EventSource', FakeEventSource)
  let geo: PublicGeoInsight = { state: 'disabled' }
  mockFetch({
    '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
    '/api/public/v1/networks': () => jsonResponse([], 200),
    '/api/public/v1/networks/mainnet': () => jsonResponse(publicNetworkPayload(PUBLIC_PEER_CASES[0].peers, geo), 200),
  })
  render(<App />)
  await act(async () => {
    window.history.pushState({}, '', '/networks/mainnet')
    window.dispatchEvent(new PopStateEvent('popstate'))
    await Promise.resolve()
  })

  try {
    await screen.findByText('Peer countries · Disabled by server', { exact: true })
    const peerRegion = await screen.findByRole('region', { name: 'Peer insight' })
    const transportStatus = screen.getByRole('status', { name: /live updates/i })
    await waitFor(() => expect(FakeEventSource.latest?.url).toBe('/api/public/v1/events?after=0'))

    geo = {
      state: 'current',
      countries: [{ countryCode: 'JP', count: 4 }],
      attribution: GEO_ATTRIBUTION,
    }
    await act(async () => {
      FakeEventSource.latest?.emit(
        'invalidation',
        JSON.stringify({ version: 1, eventId: 1, resource: 'network', resourceId: 'mainnet', revision: 1 }),
      )
      await Promise.resolve()
    })

    const geoRegion = await screen.findByRole('region', { name: 'Peer countries' })
    expect(within(geoRegion).getByText('Current', { exact: true })).toBeTruthy()
    expect(within(geoRegion).getByText('JP', { exact: true })).toBeTruthy()
    expect(within(geoRegion).getByText('4', { exact: true })).toBeTruthy()
    expect(screen.getByRole('region', { name: 'Peer insight' })).toBe(peerRegion)
    expect(screen.getByRole('status', { name: /live updates/i })).toBe(transportStatus)
    expect(screen.queryByText('Peer countries · Disabled by server', { exact: true })).toBeNull()
  } finally {
    await act(async () => {
      window.history.pushState({}, '', '/')
      window.dispatchEvent(new PopStateEvent('popstate'))
      await Promise.resolve()
    })
    await screen.findByRole('region', { name: 'Home' })
  }
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
    expect(screen.getByText('Health')).toBeTruthy()
    expect(screen.getByText('Node status')).toBeTruthy()
    expect(screen.getByText('Producing')).toBeTruthy()
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
    const resources = screen.getByLabelText('Node process and storage resources')
    expect(resources.textContent).toContain('CPU')
    expect(resources.textContent).toContain('12.5%')
    expect(resources.textContent).toContain('Memory')
    expect(resources.textContent).toContain('6.3%')
    expect(resources.textContent).toContain('Node data')
    expect(resources.textContent).toContain('2.00 GiB')
    expect(resources.querySelectorAll('.node-hero-resource-progress')).toHaveLength(3)
    expect(screen.getByRole('heading', { level: 3, name: 'Network' })).toBeTruthy()
    expect(screen.getByText('2.00 KiB/s')).toBeTruthy()
    expect(screen.getByText('4.00 KiB/s')).toBeTruthy()
    expect(screen.getByRole('heading', { level: 3, name: 'Connections' })).toBeTruthy()
    const connectionsLegend = screen.getByLabelText('Connections chart legend')
    expect(connectionsLegend.textContent).toContain('Inbound')
    expect(connectionsLegend.textContent).toContain('Outbound')
    expect(screen.getByRole('heading', { level: 3, name: 'Block time' })).toBeTruthy()
    expect(screen.getByText('2.00 s')).toBeTruthy()
    expect(screen.getByRole('heading', { level: 3, name: 'Transactions' })).toBeTruthy()
    expect(screen.getAllByRole('img', { name: /line chart over the last minute/ })).toHaveLength(2)
    expect(screen.getAllByRole('img', { name: /bar chart over the last minute/ })).toHaveLength(2)
    expect(screen.getAllByText('1m')).toHaveLength(4)
    expect(screen.queryByRole('progressbar')).toBeNull()
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
    expect(await screen.findByRole('region', { name: 'Home' })).toBeTruthy()
    const brand = screen.getByRole('link', { name: 'PlatPulse' })
    expect(brand.querySelector('img')?.getAttribute('src')).toContain('platpulse-mark')
    expect(brand.textContent).toBe('PlatPulse')
    expect(screen.getByRole('link', { name: 'Admin' })).toBeTruthy()
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
      '/api/public/v1/networks/mainnet': () => jsonResponse({
        networkKey: 'mainnet',
        displayName: 'Mainnet',
        nodes: [nodePayload],
        peers: { state: 'ok', freshness: 'current', peerCount: 3 },
        geo: { state: 'disabled' },
        validators: [],
      }, 200),
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
    expect(screen.getByText('Healthy')).toBeTruthy()
    fireEvent.click(nodeCard)
    expect(await screen.findByRole('heading', { level: 1, name: 'Validator A' })).toBeTruthy()
    fireEvent.click(screen.getByRole('link', { name: /← mainnet/ }))
    expect(await screen.findByRole('heading', { level: 1, name: 'Mainnet' })).toBeTruthy()
    expect(screen.getByText('Network overview')).toBeTruthy()
    expect(screen.getByText('PlatON Nodes')).toBeTruthy()
    expect(screen.getByText('Network key')).toBeTruthy()
  })

  it('renders compact Node cards with the earliest Server component receipt time', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    vi.setSystemTime(new Date('2026-08-20T00:10:00Z'))

    const longName = 'Node with a very long name that remains readable on narrow screens'
    const longReason = 'One or more observations are unavailable while this Node is starting and waiting for a complete Server-owned snapshot.'
    const unbrokenReason = 'ServerOwnedHealthReasonThatMustWrapWithoutTruncationAtNarrowWidths0123456789'
    const healthyNode: PublicNode = {
      nodeId: 'node-healthy',
      displayName: 'Healthy Node',
      networkKey: 'mainnet',
      health: 'healthy',
      healthReason: 'RPC, sync, and consensus are current',
      // This is the oldest of the three Server receipt timestamps; the card
      // must not substitute the newer Agent report or component observation.
      freshness: '2026-08-20T00:05:00Z',
      lastReportAt: '2026-08-20T00:09:00Z',
      rpcState: 'ok',
      // A foreign value must not be treated as a successful collection state.
      syncState: 'synced',
      consensusState: 'ok',
      processState: 'running',
      resyncState: 'normal',
      networkReferenceConfidence: 'high',
      currentHead: 100,
      peers: { state: 'ok', freshness: 'current', peerCount: 30, inboundCount: 10, outboundCount: 20 },
      consensus: { state: 'ok', freshness: 'current' },
      validator: null,
    }
    const startingNode: PublicNode = {
      nodeId: 'node-starting',
      displayName: longName,
      networkKey: 'mainnet',
      health: 'unknown',
      healthReason: longReason,
      // A freshness enum is not a component receipt timestamp.
      freshness: 'current',
      rpcState: 'Starting',
      syncState: 'starting',
      consensusState: 'starting',
      processState: 'starting',
      resyncState: 'normal',
      networkReferenceConfidence: 'unknown',
      currentHead: null,
      peers: { state: 'unknown', freshness: 'unknown', peerCount: null, inboundCount: null, outboundCount: null },
      consensus: { state: 'unknown', freshness: 'unknown' },
      validator: null,
    }
    const unbrokenReasonNode = {
      ...startingNode,
      nodeId: 'node-unbroken-reason',
      displayName: 'Node With An Unbroken Health Reason',
      healthReason: unbrokenReason,
    }
    const healthyWithUnknownReceiptNode = {
      ...healthyNode,
      nodeId: 'node-healthy-unknown-receipt',
      displayName: 'Healthy Node With Unknown Receipt',
      freshness: 'not-a-timestamp',
    }

    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/public/v1/networks': () => jsonResponse([], 200),
      '/api/public/v1/networks/mainnet': () => jsonResponse({
        networkKey: 'mainnet',
        displayName: 'Mainnet',
        nodes: [healthyNode, startingNode, unbrokenReasonNode, healthyWithUnknownReceiptNode],
        peers: { state: 'ok', freshness: 'current', peerCount: 30 },
        geo: { state: 'disabled' },
        validators: [],
      }, 200),
    })

    render(<App />)
    await act(async () => {
      window.history.pushState({}, '', '/networks/mainnet')
      window.dispatchEvent(new PopStateEvent('popstate'))
      await Promise.resolve()
    })

    const healthyCard = (await screen.findByRole('heading', { level: 2, name: 'Healthy Node' })).closest('article')
    if (!healthyCard) throw new Error('Healthy Node card is missing')
    expect(within(healthyCard).getByText('Healthy', { exact: true })).toBeTruthy()
    expect(within(healthyCard).getByText('Head', { exact: true })).toBeTruthy()
    expect(within(healthyCard).getByText('100', { exact: true })).toBeTruthy()
    expect(within(healthyCard).getByText('30', { exact: true })).toBeTruthy()
    expect(within(healthyCard).getByText('10 inbound · 20 outbound')).toBeTruthy()
    expect(within(healthyCard).getByText('Oldest component update', { exact: true })).toBeTruthy()
    expect(within(healthyCard).getByText('5 minutes ago', { exact: true })).toBeTruthy()
    expect(within(healthyCard).getByText(/20 Aug 2026.*00:05:00 UTC/)).toBeTruthy()
    expect(within(healthyCard).getByText(/earliest Server receipt across RPC, Sync, and Consensus/i)).toBeTruthy()
    expect(within(healthyCard).queryByText('RPC, sync, and consensus are current')).toBeNull()
    expect(within(healthyCard).queryByText('Current observation')).toBeNull()
    expect(within(healthyCard).getByRole('link', { name: 'Healthy Node' }).getAttribute('href')).toBe('/nodes/node-healthy')
    expect(within(healthyCard).getByRole('link', { name: 'View details' }).getAttribute('href')).toBe('/nodes/node-healthy')
    expect(within(healthyCard).getByRole('link', { name: 'View details' }).getAttribute('target')).toBeNull()
    expect(within(healthyCard).getByRole('link', { name: 'View details' }).textContent).toContain('→')
    expect(within(healthyCard).getByLabelText('Node component status').textContent).toContain('RPC')
    expect(within(healthyCard).getByLabelText('Node component status').textContent).toContain('Sync')
    expect(within(healthyCard).getByLabelText('Node component status').textContent).toContain('Consensus')
    expect(within(healthyCard).getByLabelText('Node component status').textContent).toContain('Current')
    expect(within(healthyCard).getByLabelText('Node component status').textContent).toContain('Unknown')

    const startingCard = (await screen.findByRole('heading', { level: 2, name: longName })).closest('article')
    if (!startingCard) throw new Error('Starting Node card is missing')
    expect(within(startingCard).getByText(longReason, { exact: true })).toBeTruthy()
    expect(within(startingCard).getByText('Oldest component update', { exact: true })).toBeTruthy()
    expect(within(startingCard).getAllByText('Unknown', { exact: true }).length).toBeGreaterThan(0)
    expect(within(startingCard).getByText(/RPC, Sync, and Consensus receipt time is unavailable/i)).toBeTruthy()
    expect(within(startingCard).getAllByText('Starting', { exact: true }).length).toBe(3)
    expect(within(startingCard).getByLabelText('Node component status').textContent).toContain('Starting')

    const unbrokenReasonCard = (await screen.findByRole('heading', { level: 2, name: 'Node With An Unbroken Health Reason' })).closest('article')
    if (!unbrokenReasonCard) throw new Error('Unbroken-reason Node card is missing')
    expect(within(unbrokenReasonCard).getByText(unbrokenReason, { exact: true })).toBeTruthy()

    const healthyWithUnknownReceiptCard = (await screen.findByRole('heading', { level: 2, name: 'Healthy Node With Unknown Receipt' })).closest('article')
    if (!healthyWithUnknownReceiptCard) throw new Error('Healthy Node with unknown receipt card is missing')
    expect(within(healthyWithUnknownReceiptCard).getByText('Healthy', { exact: true })).toBeTruthy()
    expect(within(healthyWithUnknownReceiptCard).getByText('Oldest component update', { exact: true })).toBeTruthy()
    expect(healthyWithUnknownReceiptCard.textContent).toMatch(/Oldest component update\s*Unknown/)
  })

  it('treats an invalid calendar timestamp as unavailable on public Node cards', async () => {
    const node: PublicNode = {
      nodeId: 'node-invalid-time',
      displayName: 'Node With Invalid Receipt Time',
      networkKey: 'mainnet',
      health: 'healthy',
      healthReason: 'routine',
      freshness: '2026-02-30T00:00:00Z',
      rpcState: 'ok',
      syncState: 'ok',
      consensusState: 'ok',
      processState: 'running',
      resyncState: 'normal',
      networkReferenceConfidence: 'unknown',
      currentHead: 1,
      peers: { state: 'ok', freshness: 'current', peerCount: 0, inboundCount: 0, outboundCount: 0 },
      consensus: { state: 'ok', freshness: 'current' },
      validator: null,
    }
    const missingZoneNode = {
      ...node,
      nodeId: 'node-missing-zone',
      displayName: 'Node With Missing Timezone',
      freshness: '2026-08-20T00:05:00',
    }
    const incompleteNode = {
      ...node,
      nodeId: 'node-incomplete-time',
      displayName: 'Node With Incomplete Receipt Time',
      freshness: '2026-08-20T00:05',
    }

    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/public/v1/networks': () => jsonResponse([], 200),
      '/api/public/v1/networks/mainnet': () => jsonResponse({
        networkKey: 'mainnet',
        displayName: 'Mainnet',
        nodes: [node, missingZoneNode, incompleteNode],
        peers: { state: 'ok', freshness: 'current', peerCount: 0 },
        geo: { state: 'disabled' },
        validators: [],
      }, 200),
    })

    render(<App />)
    await act(async () => {
      window.history.pushState({}, '', '/networks/mainnet')
      window.dispatchEvent(new PopStateEvent('popstate'))
      await Promise.resolve()
    })

    const invalidDisplayNames = [
      'Node With Invalid Receipt Time',
      'Node With Missing Timezone',
      'Node With Incomplete Receipt Time',
    ]
    for (const displayName of invalidDisplayNames) {
      const card = (await screen.findByRole('heading', { level: 2, name: displayName })).closest('article')
      if (!card) throw new Error(displayName + ' card is missing')
      expect(within(card).getByText('RPC, Sync, and Consensus receipt time is unavailable.', { exact: true })).toBeTruthy()
      expect(within(card).queryByText('Earliest Server receipt across RPC, Sync, and Consensus', { exact: true })).toBeNull()
    }
  })

  it('falls back to the Node ID when a public Node display name is empty', async () => {
    const node: PublicNode = {
      nodeId: 'node-empty-name',
      displayName: '',
      networkKey: 'mainnet',
      health: 'healthy',
      healthReason: 'routine',
      freshness: null,
      rpcState: 'ok',
      syncState: 'ok',
      consensusState: 'ok',
      processState: 'running',
      resyncState: 'normal',
      networkReferenceConfidence: 'unknown',
      currentHead: 1,
      peers: { state: 'ok', freshness: 'current', peerCount: 0, inboundCount: 0, outboundCount: 0 },
      consensus: { state: 'ok', freshness: 'current' },
      validator: null,
    }

    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/public/v1/networks': () => jsonResponse([], 200),
      '/api/public/v1/networks/mainnet': () => jsonResponse({
        networkKey: 'mainnet',
        displayName: 'Mainnet',
        nodes: [node],
        peers: { state: 'ok', freshness: 'current', peerCount: 0 },
        geo: { state: 'disabled' },
        validators: [],
      }, 200),
    })

    render(<App />)
    await act(async () => {
      window.history.pushState({}, '', '/networks/mainnet')
      window.dispatchEvent(new PopStateEvent('popstate'))
      await Promise.resolve()
    })

    const heading = await screen.findByRole('heading', { level: 2, name: 'node-empty-name' })
    expect(within(heading).getByRole('link', { name: 'node-empty-name' }).getAttribute('href')).toBe('/nodes/node-empty-name')
  })

  it('navigates both Network Node card links to the existing public Node Detail', async () => {
    const node = {
      nodeId: 'node-links',
      displayName: 'Linkable Node',
      networkKey: 'mainnet',
      health: 'healthy',
      healthReason: 'routine',
      freshness: '2026-08-20T00:05:00Z',
      rpcState: 'ok',
      syncState: 'ok',
      consensusState: 'ok',
      processState: 'running',
      resyncState: 'normal',
      currentHead: 1,
      peers: { state: 'ok', freshness: 'current', peerCount: 1, inboundCount: 1, outboundCount: 0 },
      consensus: { state: 'ok', freshness: 'current' },
      validator: null,
    }
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/public/v1/networks': () => jsonResponse([], 200),
      '/api/public/v1/networks/mainnet': () => jsonResponse({ networkKey: 'mainnet', displayName: 'Mainnet', nodes: [node], peers: { state: 'ok', freshness: 'current', peerCount: 1 }, geo: { state: 'disabled' }, validators: [] }, 200),
      '/api/public/v1/nodes/node-links': () => jsonResponse({ ...node, processStartedAt: null, lastReportAt: null }, 200),
      '/api/public/v1/nodes/node-links/history?limit=2': () => jsonResponse([], 200),
      '/api/public/v1/nodes/node-links/metrics': () => jsonResponse({ from: '2026-08-20T00:00:00Z', to: '2026-08-20T00:01:00Z', windowSeconds: 60, processCpuPercent: [], processMemoryPercent: [], dataDirectoryPercent: [], networkRxBytesPerSec: [], networkTxBytesPerSec: [], peerInboundCount: [], peerOutboundCount: [], blockIntervalMs: [], transactionCount: [] }, 200),
      '/api/public/v1/nodes/node-links/peer-history': () => jsonResponse({ state: 'unknown', freshness: 'unknown', fiveMinute: [], hourly: [] }, 200),
    })

    render(<App />)
    await act(async () => {
      window.history.pushState({}, '', '/networks/mainnet')
      window.dispatchEvent(new PopStateEvent('popstate'))
      await Promise.resolve()
    })
    const card = (await screen.findByRole('heading', { level: 2, name: 'Linkable Node' })).closest('article')
    if (!card) throw new Error('Linkable Node card is missing')
    fireEvent.click(within(card).getByRole('link', { name: 'View details' }))
    expect(await screen.findByRole('heading', { level: 1, name: 'Linkable Node' })).toBeTruthy()

    await act(async () => {
      window.history.pushState({}, '', '/networks/mainnet')
      window.dispatchEvent(new PopStateEvent('popstate'))
      await Promise.resolve()
    })
    const cardAgain = (await screen.findByRole('heading', { level: 2, name: 'Linkable Node' })).closest('article')
    if (!cardAgain) throw new Error('Linkable Node card did not return')
    fireEvent.click(within(cardAgain).getByRole('link', { name: 'Linkable Node' }))
    expect(await screen.findByRole('heading', { level: 1, name: 'Linkable Node' })).toBeTruthy()
  })

  it('keeps retained zero and simultaneous collection and freshness failures visible on Node cards', async () => {
    const peers = {
      state: 'error',
      freshness: 'stale',
      staleSince: '2026-08-16T03:02:00Z',
      peerCount: 0,
      inboundCount: 0,
      outboundCount: 0,
      trustedCount: 0,
      staticCount: 0,
      consensusCount: 0,
    }
    const node = {
      nodeId: 'node-1',
      displayName: 'Validator A',
      networkKey: 'mainnet',
      health: 'unhealthy',
      healthReason: 'Peer collection failed',
      freshness: 'stale',
      rpcState: 'ok',
      syncState: 'ok',
      consensusState: 'ok',
      processState: 'ok',
      peers,
    }
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/public/v1/networks': () => jsonResponse([], 200),
      '/api/public/v1/networks/mainnet': () => jsonResponse({
        networkKey: 'mainnet',
        displayName: 'Mainnet',
        nodes: [node],
        peers,
        geo: { state: 'disabled' },
        validators: [],
      }, 200),
    })

    render(<App />)
    await act(async () => {
      window.history.pushState({}, '', '/networks/mainnet')
      window.dispatchEvent(new PopStateEvent('popstate'))
      await Promise.resolve()
    })

    const nodeHeading = await screen.findByRole('heading', { level: 2, name: 'Validator A' })
    const nodeCard = nodeHeading.closest<HTMLElement>('article')
    if (!nodeCard) throw new Error('Node card was not rendered')
    expect(within(nodeCard).getByText('0', { exact: true })).toBeTruthy()
    expect(within(nodeCard).getByText(/Collection failed.*Stale.*Showing last successful snapshot/i)).toBeTruthy()
  })

  it('keeps public transport status visible while Network REST is unavailable', async () => {
    vi.stubGlobal('EventSource', FakeEventSource)
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/public/v1/networks': () => jsonResponse([], 200),
      '/api/public/v1/networks/unavailable-network*': () => errorBody('network_unavailable'),
    })

    render(<App />)
    await act(async () => {
      window.history.pushState({}, '', '/networks/unavailable-network')
      window.dispatchEvent(new PopStateEvent('popstate'))
      await Promise.resolve()
    })

    expect((await screen.findByRole('alert')).textContent).toContain('Network is Error')
    expect(screen.getByRole('status', { name: 'Connecting to live updates' })).toBeTruthy()
    await act(async () => {
      FakeEventSource.latest?.onerror?.()
      await Promise.resolve()
    })
    expect(screen.getByRole('status', { name: 'Live updates paused' })).toBeTruthy()
  })

  it('separates connected transport from stale observations and Unknown Node Health', async () => {
    vi.stubGlobal('EventSource', FakeEventSource)
    let networkCalls = 0
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/public/v1/networks': () => jsonResponse([], 200),
      '/api/public/v1/networks/network-with-a-very-long-key*': () => {
        networkCalls += 1
        return networkCalls === 1 ? jsonResponse({
        networkKey: 'network-with-a-very-long-key',
        displayName: 'A Network Name That Wraps On Narrow Screens',
        peers: {
          state: 'error',
          freshness: 'stale',
          peerCount: 4,
          inboundCount: 1,
          outboundCount: 3,
          trustedCount: 2,
          staticCount: 1,
          consensusCount: 2,
          receivedAt: '2026-08-20T00:00:00Z',
          staleSince: '2026-08-20T00:05:00Z',
        },
        geo: { state: 'disabled' },
        validators: [],
        nodes: [{
          nodeId: 'node-unknown-health',
          displayName: 'Node With Unknown Health',
          networkKey: 'network-with-a-very-long-key',
          health: 'unknown',
          healthReason: 'Health summary unavailable',
          freshness: '2026-08-20T00:00:00Z',
          rpcState: 'ok',
          syncState: 'ok',
          consensusState: 'ok',
          currentHead: 42,
          peers: {
            state: 'error',
            freshness: 'stale',
            peerCount: 4,
            inboundCount: 1,
            outboundCount: 3,
          },
        }],
        }, 200) : errorBody('network_unavailable')
      }
    })

    render(<App />)
    await act(async () => {
      window.history.pushState({}, '', '/networks/network-with-a-very-long-key')
      window.dispatchEvent(new PopStateEvent('popstate'))
      await Promise.resolve()
    })
    expect(await screen.findByRole('heading', { level: 1, name: 'A Network Name That Wraps On Narrow Screens' })).toBeTruthy()
    expect(screen.getByRole('status', { name: 'Connecting to live updates' })).toBeTruthy()
    await act(async () => {
      FakeEventSource.latest?.onopen?.()
      await Promise.resolve()
    })

    const networkPeer = screen.getByRole('region', { name: 'Peer insight' })
    const metadata = screen.getByLabelText('Network identity and live updates')
    expect(within(metadata).getByText('Network key')).toBeTruthy()
    expect(within(metadata).getByText('network-with-a-very-long-key')).toBeTruthy()
    expect(within(metadata).getByRole('status', { name: 'Live updates connected' })).toBeTruthy()
    expect(screen.getByText('Stale', { exact: true })).toBeTruthy()
    const nodeCard = screen.getByText('Node With Unknown Health').closest('article')
    if (!nodeCard) throw new Error('Unknown-health Node card is missing')
    expect(within(nodeCard).getByText('Unknown', { exact: true })).toBeTruthy()
    expect(screen.queryByRole('status', { name: 'Current' })).toBeNull()
    expect(networkCalls).toBe(1)

    await act(async () => {
      FakeEventSource.latest?.emit('invalidation', JSON.stringify({
        eventId: 2,
        resource: 'network',
        resourceId: 'network-with-a-very-long-key',
      }))
      await Promise.resolve()
    })
    expect(await screen.findByText('Network refresh failed; showing the last successful Network data.')).toBeTruthy()
    expect(networkCalls).toBe(2)
    expect(within(networkPeer).getByText('4', { exact: true })).toBeTruthy()
    expect(within(networkPeer).getByText('Collection failed', { exact: false })).toBeTruthy()
    expect(within(networkPeer).getAllByText('Showing last successful snapshot', { exact: false }).length).toBeGreaterThan(0)
    expect(within(networkPeer).getByText('Stale', { exact: true })).toBeTruthy()
    expect(screen.getByRole('heading', { level: 1, name: 'A Network Name That Wraps On Narrow Screens' })).toBeTruthy()
    window.dispatchEvent(new Event('offline'))
    expect(await screen.findByText('You are offline')).toBeTruthy()
    expect(within(metadata).getByRole('status', { name: 'Live updates connected' })).toBeTruthy()
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
    expect(screen.getAllByRole('img', { name: /line chart over the last minute/ })).toHaveLength(2)
    expect(screen.getAllByRole('img', { name: /bar chart over the last minute/ })).toHaveLength(2)
    expect(screen.getAllByText('No samples in the last minute')).toHaveLength(4)
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
      { name: 'Settings', href: '/admin/settings' },
      { name: 'Sessions', href: '/admin/access/sessions' },
      { name: 'Audit', href: '/admin/access/audit' },
    ])
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
