import { act, cleanup, render, screen, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import App from '../App'
import { adminQueryClient } from '../api/admin'
import { client } from '../api/generated/client.gen'

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

const NODE = {
  node_id: 'node-1',
  agent_id: 'agent-1',
  display_name: 'Node A',
  network_key: 'platon-mainnet',
  network_display_name: 'PlatON Mainnet',
  lifecycle: 'active',
  lifecycle_guidance: 'Active Node',
  visibility: 'public',
  inventory_revision: 3,
  first_seen_at: '2026-08-12T08:00:00Z',
  updated_at: '2026-08-12T08:00:00Z',
  rpc_endpoint: 'ws://[REDACTED_IP]:****',
  health: 'healthy',
  health_reason: 'RPC, sync, and consensus are current',
  freshness: 'current',
  current_head: 128,
  resync_state: 'normal',
  identity: { state: 'matched', observed: null, mismatched_fields: [] },
}

const OVERVIEW = {
  generated_at: '2026-08-12T08:00:00Z',
  summary: {
    agents: { total: 1, online: 1, offline: 0, unknown: 0 },
    nodes: { total: 1, healthy: 1, unhealthy: 0, unknown: 0, retired: 0, published: 1 },
  },
  attention: [
    {
      id: 'node_unhealthy:node:node-1',
      kind: 'node_unhealthy',
      severity: 'critical',
      subject_kind: 'node',
      subject_id: 'node-1',
      subject_label: 'Node A',
      message: 'RPC collection failed',
      observed_at: '2026-08-12T08:00:00Z',
    },
  ],
}

const AGENT = {
  agent_id: 'agent-1',
  agent_epoch: 1,
  boot_status: 'running',
  active_boot_id: '0195f2a1-2b3c-4d5e-8f90-123456789abc',
  capabilities: [],
  clock_status: 'ok',
  clock_skew_ms: 0,
  credentials: [],
  host: null,
  last_received_at: '2026-08-12T08:00:00Z',
  last_report_sequence: 42,
  liveness: 'online',
  nodes: [
    {
      node_id: 'node-1',
      display_name: 'Node A',
      network_key: 'platon-mainnet',
      health: 'healthy',
      health_reason: 'RPC, sync, and consensus are current',
      freshness: 'current',
      lifecycle: 'active',
      visibility: 'public',
      inventory_revision: 3,
      resync_state: 'normal',
      network_reference_confidence: 'high',
      data_directory: {
        state: 'ok',
        state_revision: 1,
        value_revision: 1,
        size_bytes: 12_884_901_888,
        received_at: '2026-08-12T08:00:00Z',
      },
      rpc: {
        state: 'ok',
        client_version: 'platon/1.5.1',
        namespaces: ['platon', 'net', 'admin'],
        methods: [],
        received_at: '2026-08-12T08:00:00Z',
      },
    },
  ],
  security_event_count: 0,
  sequence_gap_count: 0,
  shutdown_state: 'running',
  shutdown_forced: false,
  previous_boot_id: null,
}

function jsonResponse(body: unknown, status: number): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

const TEST_ORIGIN = 'http://platpulse.test'

/** Exact-URL route map for the page's REST surface. */
function mockFetch(routes: Record<string, () => Response | Promise<Response>>) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const url = (input instanceof Request ? input.url : String(input)).replace(TEST_ORIGIN, '')
    const handler = routes[url]
    if (handler) return Promise.resolve(handler())
    return Promise.resolve(jsonResponse({ error: { code: 'not_found' } }, 404))
  })
  vi.stubGlobal('fetch', fetchMock)
  return fetchMock
}

/** The Overview page must never request the deferred Geo database status. */
function expectNoGeoRequests(fetchMock: ReturnType<typeof mockFetch>) {
  const geoCalls = fetchMock.mock.calls.filter(([input]) =>
    String(input).includes('/api/admin/v1/geo'),
  )
  expect(geoCalls).toHaveLength(0)
}

async function renderAt(path: string) {
  render(<App />)
  await act(async () => {
    window.history.pushState({}, '', path)
    window.dispatchEvent(new PopStateEvent('popstate'))
    await Promise.resolve()
  })
}

beforeEach(() => {
  window.history.replaceState({}, '', '/')
  client.setConfig({ baseUrl: TEST_ORIGIN })
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  adminQueryClient.clear()
})
describe('PAGE-ADMIN-OVERVIEW', () => {
  it('prioritizes attention, Node health and Agent inventory; legacy Geo and Operations content is absent', async () => {
    const fetchMock = mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/overview': () => jsonResponse(OVERVIEW, 200),
      '/api/admin/v1/nodes': () => jsonResponse([NODE], 200),
      '/api/admin/v1/agents': () => jsonResponse([AGENT], 200),
    })
    await renderAt('/admin')

    await screen.findByRole('heading', { level: 1, name: 'Overview' })

    // The attention queue is the first panel and renders Server-owned items.
    const panels = screen.getAllByRole('heading', { level: 2 })
    expect(panels[0].textContent).toContain('Attention queue')
    expect(screen.getByText(/RPC collection failed/)).toBeTruthy()
    expect(screen.getByText('Critical')).toBeTruthy()

    // Node Health Summary with its own table row per Node.
    expect(screen.getByRole('heading', { level: 2, name: 'Node Health Summary' })).toBeTruthy()
    const nodeRow = await screen.findByRole('row', { name: /Node A/ })
    expect(nodeRow.textContent).toContain('healthy')
    expect(nodeRow.textContent).toContain('Current')
    await act(async () => {
      screen.getByRole('button', { name: /Node A/ }).click()
    })
    expect(screen.getByText('Node Data')).toBeTruthy()
    expect(screen.getByText('12.0 GiB')).toBeTruthy()

    // Agent inventory stays an independent panel (one Agent, its own card).
    expect(screen.getByRole('heading', { level: 2, name: 'Agent inventory' })).toBeTruthy()
    expect(screen.getByRole('heading', { level: 3, name: 'agent-1' })).toBeTruthy()

    // Geo database status is absent from the Overview page (issue #93).
    expect(screen.queryByRole('heading', { level: 2, name: 'Geo database' })).toBeNull()
    expect(screen.queryByText('Cached countries')).toBeNull()

    // Per-Node visibility/publication controls and the legacy Operations
    // panel are absent; Site Access Mode is the single site authority.
    expect(screen.queryByRole('heading', { level: 2, name: 'Operations' })).toBeNull()
    expect(screen.queryByLabelText('Node ID')).toBeNull()
    expect(screen.queryByRole('button', { name: 'Update visibility' })).toBeNull()
    expect(screen.queryByText(/Nodes are visible on Home/)).toBeNull()
    expect(screen.getByText(/Site Access Mode/)).toBeTruthy()

    // The browser never asks the Server for Geo status on this page.
    expectNoGeoRequests(fetchMock)
  })

  it('keeps stale, unknown, never-observed, disabled, unsupported, and last-good states distinct', async () => {
    const diagnosticNode = {
      ...AGENT.nodes[0],
      health: 'unknown',
      health_reason: 'The Server has not observed enough current components',
      freshness: 'stale',
      rpc: undefined,
      sync: {
        state: 'error',
        error_message: 'sync collector timed out',
        current_block: 120,
        highest_block: 128,
        received_at: '2026-08-12T07:55:00Z',
      },
      consensus: { state: 'unsupported', received_at: null },
      peers: undefined,
      process: { state: 'disabled', received_at: null },
      data_directory: { state: 'starting', received_at: null },
    }
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/overview': () => jsonResponse(OVERVIEW, 200),
      '/api/admin/v1/nodes': () =>
        jsonResponse(
          [{ ...NODE, health: 'unknown', freshness: 'stale', current_head: null }],
          200,
        ),
      '/api/admin/v1/agents': () =>
        jsonResponse([{ ...AGENT, liveness: 'unknown', nodes: [diagnosticNode] }], 200),
    })
    await renderAt('/admin')

    const nodeRow = await screen.findByRole('row', { name: /Node A/ })
    expect(nodeRow.textContent).toContain('unknown')
    expect(nodeRow.textContent).toContain('Stale')
    expect(nodeRow.textContent).toContain('Unknown')

    const nodeToggle = screen.getByRole('button', { name: /Node A/ })
    await act(async () => {
      nodeToggle.click()
    })
    const detailId = nodeToggle.getAttribute('aria-controls')
    const details = detailId ? document.getElementById(detailId) : null
    expect(details).toBeTruthy()
    expect(details?.textContent).toContain('Error')
    expect(details?.textContent).toContain('sync collector timed out')
    expect(details?.textContent).toContain('last-good head 120')
    expect(details?.textContent).toContain('Disabled')
    expect(details?.textContent).toContain('Unsupported')
    expect(details?.textContent).toContain('Unknown')
    expect(details?.textContent).toContain('Never observed')
  })

  it('keeps Node health available when Agent diagnostics fail independently', async () => {
    const fetchMock = mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/overview': () => jsonResponse(OVERVIEW, 200),
      '/api/admin/v1/nodes': () => jsonResponse([NODE], 200),
      '/api/admin/v1/agents': () =>
        jsonResponse({ error: { code: 'unavailable', message: 'Agent diagnostics unavailable' } }, 503),
    })
    await renderAt('/admin')

    await screen.findByRole('heading', { level: 1, name: 'Overview' })
    expect(await screen.findByRole('row', { name: /Node A/ })).toBeTruthy()
    expect(await screen.findByText('Agent diagnostics unavailable')).toBeTruthy()
    expect(screen.queryByText('Unable to load Nodes')).toBeNull()
    expectNoGeoRequests(fetchMock)
  })

  it('keeps explicit empty states distinct from unknown or zero values', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/overview': () =>
        jsonResponse(
          {
            generated_at: '2026-08-12T08:00:00Z',
            summary: {
              agents: { total: 0, online: 0, offline: 0, unknown: 0 },
              nodes: { total: 0, healthy: 0, unhealthy: 0, unknown: 0, retired: 0, published: 0 },
            },
            attention: [],
          },
          200,
        ),
      '/api/admin/v1/nodes': () => jsonResponse([], 200),
      '/api/admin/v1/agents': () => jsonResponse([], 200),
    })
    await renderAt('/admin')

    await screen.findByRole('heading', { level: 1, name: 'Overview' })
    expect(
      await screen.findByText('No attention items. Nothing needs an Owner right now.'),
    ).toBeTruthy()
    expect(screen.getByText('No Nodes observed yet.')).toBeTruthy()
    expect(screen.getByText('No Agents enrolled yet.')).toBeTruthy()
  })

  it('shows an explicit loading state while the overview is in flight', async () => {
    let resolveOverview: (value: Response) => void = () => {}
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/overview': () =>
        new Promise<Response>((resolve) => {
          resolveOverview = resolve
        }),
    })
    await renderAt('/admin')

    await screen.findByRole('heading', { level: 1, name: 'Overview' })
    expect(screen.getByText('Checking the Server for attention…')).toBeTruthy()

    resolveOverview(jsonResponse(OVERVIEW, 200))
    expect(await screen.findByText(/RPC collection failed/)).toBeTruthy()
  })

  it('shows an explicit error state with a retry when the overview fails', async () => {
    let overviewCalls = 0
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/overview': () => {
        overviewCalls += 1
        return overviewCalls === 1
          ? jsonResponse(
              { error: { code: 'unavailable', message: 'Unable to load attention' } },
              503,
            )
          : jsonResponse(OVERVIEW, 200)
      },
    })
    await renderAt('/admin')

    await screen.findByRole('heading', { level: 1, name: 'Overview' })
    const attentionPanel = (await screen.findByText('Unable to load attention')).closest('article')
    expect(attentionPanel).toBeTruthy()
    expect(screen.queryByText(/RPC collection failed/)).toBeNull()

    await act(async () => {
      within(attentionPanel as HTMLElement).getByRole('button', { name: 'Try again' }).click()
      await Promise.resolve()
    })
    expect(await screen.findByText(/RPC collection failed/)).toBeTruthy()
  })
})
