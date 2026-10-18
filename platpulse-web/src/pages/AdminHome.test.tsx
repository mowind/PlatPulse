import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import App from '../App'
import { prioritizeAgents } from './AdminHome'
import type { AgentDiagnostic, HostDiagnostic } from '../api/generated'
import { adminQueryClient } from '../api/admin'
import { client } from '../api/generated/client.gen'
import { expectStaticReadOnlySurface } from '../test/static-surface'

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
    nodes: { total: 1, active: 1, healthy: 1, unhealthy: 0, unknown: 0, retired: 0 },
    networks: { total: 2, with_identity_mismatch: 1 },
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
      evidence_key: '',
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
  attention: [],
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
  it.each(['nodes', 'agents'])('keeps Refresh busy after Overview completes while %s still refreshes', async (pendingPanel) => {
    let refreshing = false
    let release!: (response: Response) => void
    const pending = new Promise<Response>((resolve) => { release = resolve })
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/overview': () => jsonResponse(OVERVIEW, 200),
      '/api/admin/v1/nodes': () => refreshing && pendingPanel === 'nodes' ? pending : jsonResponse([NODE], 200),
      '/api/admin/v1/agents': () => refreshing && pendingPanel === 'agents' ? pending : jsonResponse([AGENT], 200),
    })
    await renderAt('/admin')
    const button = await screen.findByRole('button', { name: 'Refresh' })
    refreshing = true
    await act(async () => button.click())
    await waitFor(() => expect(adminQueryClient.getQueryCache().findAll().find((query) =>
      query.queryKey.includes('overview'))?.state.fetchStatus).toBe('idle'))
    expect(screen.getByRole('button', { name: 'Refreshing…' })).toBe(button)
    expect((button as HTMLButtonElement).disabled).toBe(true)
    expect(button.querySelector('svg')).toBeTruthy()
    await act(async () => release(jsonResponse(pendingPanel === 'nodes' ? [NODE] : [AGENT], 200)))
    await waitFor(() => expect((screen.getByRole('button', { name: 'Refresh' }) as HTMLButtonElement).disabled).toBe(false))
  })

  it('distinguishes subjects and Critical items and exposes discarded reports without expanding', async () => {
    const id = '0195f2a1-0011-4011-8011-000000000011'
    const item = { ...OVERVIEW.attention[0], subject_kind: 'agent', subject_id: id, subject_label: id }
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/overview': () => jsonResponse({ ...OVERVIEW, attention: [
        { ...item, id: 'fatal', kind: 'agent_spool_fatal', message: 'Durable reports are at risk' },
        { ...item, id: 'overflow', kind: 'agent_spool_overflow', message: 'The spool discarded queued reports' },
      ] }, 200),
      '/api/admin/v1/nodes': () => jsonResponse([NODE], 200),
      '/api/admin/v1/agents': () => jsonResponse([AGENT], 200),
    })
    await renderAt('/admin')
    const toggle = await screen.findByRole('button', { name: 'Show 1 additional issue · 1 Critical' })
    const group = toggle.closest('[data-slot="attention-item"]') as HTMLElement
    expect(screen.getByText('2 items across 1 subjects · 2 Critical')).toBeTruthy()
    expect(within(group).getAllByText('The spool discarded queued reports').some((element) => !element.closest('[hidden]'))).toBe(true)
    expect(within(group).getByText(id).closest('details')).toBeTruthy()
    expect(within(group).getByRole('link', { name: 'View agent' }).getAttribute('href')).toBe(`/admin/agents/${id}`)
    await act(async () => toggle.click())
    expect(document.getElementById(toggle.getAttribute('aria-controls')!)?.hasAttribute('hidden')).toBe(false)
    const writeText = vi.fn().mockResolvedValue(undefined)
    vi.stubGlobal('navigator', { clipboard: { writeText } })
    const identity = within(group).getByText(id).closest('details')!
    await act(async () => { identity.open = true })
    await act(async () => within(identity).getByRole('button', { name: 'Copy Agent ID' }).click())
    expect(writeText).toHaveBeenCalledWith(id)
    expect(within(identity).getByRole('status').textContent).toBe('Copied')
  })

  it('acknowledges one Agent occurrence by its evidence boundary and refetches', async () => {
    const id = '0195f2a1-0011-4011-8011-000000000011'
    let overviewCalls = 0
    const item = {
      ...OVERVIEW.attention[0],
      id: 'agent_offline:agent:' + id,
      kind: 'agent_offline',
      severity: 'warning',
      subject_kind: 'agent',
      subject_id: id,
      subject_label: id,
      message: 'the Agent has not reported within the liveness window',
      evidence_key: 'offline-1',
    }
    const fetchMock = mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/overview': () => {
        overviewCalls += 1
        return jsonResponse({ ...OVERVIEW, attention: overviewCalls === 1 ? [item] : [] }, 200)
      },
      '/api/admin/v1/nodes': () => jsonResponse([NODE], 200),
      '/api/admin/v1/agents': () => jsonResponse([AGENT], 200),
      [`/api/admin/v1/agents/${id}/attention/acknowledgments`]: () =>
        jsonResponse(
          {
            agent_id: id,
            attention: [],
            acknowledged: [{ kind: 'agent_offline', evidence_key: 'offline-1' }],
            skipped: [],
          },
          200,
        ),
    })
    await renderAt('/admin')
    fireEvent.click(
      await screen.findByRole('button', { name: 'Acknowledge agent_offline for this Agent' }),
    )
    await waitFor(() => expect(overviewCalls).toBeGreaterThanOrEqual(2))
    const ackCall = fetchMock.mock.calls.find(([input]) =>
      (input instanceof Request ? input.url : String(input)).includes('/attention/acknowledgments'),
    )
    expect(ackCall).toBeTruthy()
    const ackInit = (ackCall as unknown as [RequestInfo | URL, RequestInit | undefined])[1]
    const ackInput = ackCall![0]
    const ackRequest =
      ackInput instanceof Request ? ackInput : new Request(String(ackInput), ackInit)
    expect(ackRequest.method).toBe('POST')
    expect(JSON.parse(await ackRequest.clone().text())).toEqual({
      items: [{ kind: 'agent_offline', evidence_key: 'offline-1' }],
    })
    await waitFor(() =>
      expect(
        screen.queryByRole('button', { name: 'Acknowledge agent_offline for this Agent' }),
      ).toBeNull(),
    )
  })

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

    // Node Health Summary is a compact preview: name, Network, independent
    // Health and Freshness, and a link to the Node; no in-place diagnostics.
    const nodePanel = screen.getByRole('heading', { level: 2, name: 'Node Health Summary' }).closest('article') as HTMLElement
    const nodeItem = await within(nodePanel).findByRole('listitem')
    expect(nodeItem.textContent).toContain('healthy')
    expect(nodeItem.textContent).toContain('Current')
    expect(nodeItem.textContent).toContain('PlatON Mainnet')
    // A healthy Node repeats no reason and no teaching copy.
    expect(nodeItem.textContent).not.toContain('RPC, sync, and consensus are current')
    expect(nodeItem.textContent).not.toContain('Server-owned freshness')
    expect(within(nodeItem).getByRole('link', { name: 'Node A' }).getAttribute('href')).toBe('/admin/nodes/node-1')
    expect(within(nodePanel).queryByRole('button')).toBeNull()
    expect(screen.queryByText('Node Data')).toBeNull()

    // Agent inventory stays an independent panel with a compact item.
    const agentPanel = screen.getByRole('heading', { level: 2, name: 'Agent inventory' }).closest('article') as HTMLElement
    const agentItem = await within(agentPanel).findByRole('listitem')
    expect(within(agentItem).getByRole('link', { name: 'agent-1' }).getAttribute('href')).toBe('/admin/agents/agent-1')
    expect(agentItem.textContent).toContain('1 retained Node')
    // Host resources, report sequence and Spool/Clock fields stay on the Agent.
    expect(agentPanel.textContent).not.toMatch(/CPU|Memory|RX \/ TX|Report #|Spool|Clock|Server liveness/)

    // Geo database status is absent from the Overview page (issue #93).
    expect(screen.queryByRole('heading', { level: 2, name: 'Geo database' })).toBeNull()
    expect(screen.queryByText('Cached countries')).toBeNull()

    // Per-Node visibility/publication controls and the legacy Operations
    // panel are absent; Site Access Mode is the single site authority.
    expect(screen.queryByRole('heading', { level: 2, name: 'Operations' })).toBeNull()
    expect(screen.queryByLabelText('Node ID')).toBeNull()
    expect(screen.queryByRole('button', { name: 'Update visibility' })).toBeNull()
    expect(screen.queryByText(/Nodes are visible on Home/)).toBeNull()

    // The browser never asks the Server for Geo status on this page.
    expectNoGeoRequests(fetchMock)
  })

  it('associates additional Attention issues with their disclosure list', async () => {
    const multiItemOverview = {
      ...OVERVIEW,
      attention: [
        { ...OVERVIEW.attention[0], id: 'node_resync:node:node-1', kind: 'node_resync', severity: 'warning', message: 'resync is pending', observed_at: null },
        OVERVIEW.attention[0],
      ],
    }
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/overview': () => jsonResponse(multiItemOverview, 200),
      '/api/admin/v1/nodes': () => jsonResponse([NODE], 200),
      '/api/admin/v1/agents': () => jsonResponse([AGENT], 200),
    })
    await renderAt('/admin')
    const toggle = await screen.findByRole('button', { name: /Show 1 additional issues?/ })
    const detailsId = toggle.getAttribute('aria-controls')
    expect(detailsId).toBeTruthy()
    expect(screen.getByText(/RPC collection failed/)).toBeTruthy()
    expect(screen.queryByText('resync is pending')).toBeNull()
    expect(document.getElementById(detailsId as string)?.hasAttribute('hidden')).toBe(true)
    await act(async () => toggle.click())
    const details = document.getElementById(detailsId as string)
    expect(details).toBeTruthy()
    expect(details?.textContent).toContain('resync is pending')
    // The primary item labels its own observation time; the grouped item has
    // no timestamp and stays explicitly unknown instead of borrowing one.
    const group = toggle.closest('[data-slot="attention-item"]') as HTMLElement
    expect(group.textContent).toContain('Last observed')
    expect(details?.textContent).toContain('Observation time unknown')
  })

  it('keeps variant query parameters on the canonical production Overview', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/overview': () => jsonResponse(OVERVIEW, 200),
      '/api/admin/v1/nodes': () => jsonResponse([NODE], 200),
      '/api/admin/v1/agents': () => jsonResponse([AGENT], 200),
    })

    await renderAt('/admin?variant=C')
    expect(window.location.pathname).toBe('/admin')
    expect(window.location.search).toBe('?variant=C')
    expect(await screen.findByRole('heading', { level: 1, name: 'Overview' })).toBeTruthy()
    expect(screen.queryByRole('navigation', { name: 'Prototype variants' })).toBeNull()
    expect(screen.queryByText('Standalone prototype · no login')).toBeNull()
    expect(screen.queryByText('System posture')).toBeNull()
    expect(screen.queryByText('Fleet overview')).toBeNull()
  })

  it('prioritizes Active Nodes before limiting the preview and keeps deterministic ties stable', async () => {
    const activeNodes = [
      { ...NODE, node_id: 'healthy-1', display_name: 'Healthy 1', health: 'healthy', freshness: 'current' },
      { ...NODE, node_id: 'retired', display_name: 'Retired Node', lifecycle: 'retired' },
      { ...NODE, node_id: 'unhealthy-b', display_name: 'Unhealthy B', health: 'unhealthy', network_key: 'zeta' },
      { ...NODE, node_id: 'healthy-2', display_name: 'Healthy 2', health: 'healthy', freshness: 'current' },
      { ...NODE, node_id: 'unknown', display_name: 'Unknown Node', health: 'unknown', freshness: 'current' },
      { ...NODE, node_id: 'stale', display_name: 'Stale Node', health: 'healthy', freshness: 'stale' },
      { ...NODE, node_id: 'unhealthy-a', display_name: 'Unhealthy A', health: 'unhealthy', network_key: 'alpha' },
      { ...NODE, node_id: 'healthy-3', display_name: 'Healthy 3', health: 'healthy', freshness: 'current' },
      { ...NODE, node_id: 'healthy-4', display_name: 'Healthy 4', health: 'healthy', freshness: 'current' },
      { ...NODE, node_id: 'healthy-5', display_name: 'Healthy 5', health: 'healthy', freshness: 'current' },
      { ...NODE, node_id: 'healthy-6', display_name: 'Healthy 6', health: 'healthy', freshness: 'current' },
      { ...NODE, node_id: 'healthy-7', display_name: 'Healthy 7', health: 'healthy', freshness: 'current' },
      { ...NODE, node_id: 'healthy-8', display_name: 'Healthy 8', health: 'healthy', freshness: 'current' },
    ]
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/overview': () => jsonResponse(OVERVIEW, 200),
      '/api/admin/v1/nodes': () => jsonResponse(activeNodes, 200),
      '/api/admin/v1/agents': () => jsonResponse([], 200),
    })
    await renderAt('/admin')

    const panel = (await screen.findByRole('heading', { name: 'Node Health Summary' })).closest('article') as HTMLElement
    const items = await within(panel).findAllByRole('listitem')
    expect(items).toHaveLength(5)
    expect(items.slice(0, 4).map((item) => item.textContent)).toEqual([
      expect.stringContaining('Unhealthy A'),
      expect.stringContaining('Unhealthy B'),
      expect.stringContaining('Unknown Node'),
      expect.stringContaining('Stale Node'),
    ])
    expect(screen.queryByText('Retired Node')).toBeNull()
    expect(within(panel).getByText(/Showing 5 of 12 Active Nodes/)).toBeTruthy()
    // Only healthy, current Nodes are left out, so nothing exceptional is hidden.
    expect(panel.querySelector('[data-slot="preview-hidden"]')).toBeNull()
    expect(screen.getByRole('link', { name: 'View all Nodes' }).getAttribute('href')).toBe('/admin/nodes')
    expect(within(items[0]).getByRole('link', { name: 'Unhealthy A' }).getAttribute('href')).toBe('/admin/nodes/unhealthy-a')
  })

  it('counts the exceptional Nodes and Agents the five-item previews leave out', async () => {
    const nodes = [
      ...Array.from({ length: 6 }, (_, index) => ({ ...NODE, node_id: 'bad-' + index, display_name: 'Bad ' + index, health: 'unhealthy' })),
      { ...NODE, node_id: 'unknown-1', display_name: 'Unknown 1', health: 'unknown', freshness: 'unknown' },
      { ...NODE, node_id: 'stale-1', display_name: 'Stale 1', health: 'healthy', freshness: 'stale' },
      { ...NODE, node_id: 'mismatch-1', display_name: 'Mismatch 1', health: 'healthy', identity: { state: 'mismatched', observed: null, mismatched_fields: ['chain_id'] } },
    ]
    const active = [{ credential_id: 'c1', active: true, created_at: '2026-08-12T08:00:00Z', revoked_at: null, revoke_after: null }]
    const agents = [
      ...Array.from({ length: 5 }, (_, index) => ({ ...AGENT, agent_id: 'risk-' + index, security_event_count: 1, credentials: active })),
      { ...AGENT, agent_id: 'offline-agent', liveness: 'offline', credentials: active },
      { ...AGENT, agent_id: 'unknown-agent', liveness: 'unknown', credentials: active },
    ]
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/overview': () => jsonResponse(OVERVIEW, 200),
      '/api/admin/v1/nodes': () => jsonResponse(nodes, 200),
      '/api/admin/v1/agents': () => jsonResponse(agents, 200),
    })
    await renderAt('/admin')
    const nodePanel = (await screen.findByRole('heading', { name: 'Node Health Summary' })).closest('article') as HTMLElement
    await within(nodePanel).findAllByRole('listitem')
    expect(within(nodePanel).getByText(/Showing 5 of 9 Active Nodes/)).toBeTruthy()
    expect(nodePanel.querySelector('[data-slot="preview-hidden"]')?.textContent).toBe(
      'Not shown: 1 unhealthy · 1 unknown health · 1 stale · 1 identity mismatch',
    )
    const agentPanel = screen.getByRole('heading', { name: 'Agent inventory' }).closest('article') as HTMLElement
    expect(within(agentPanel).getByText(/Showing 5 of 7 Agents/)).toBeTruthy()
    // Each left-out Agent is counted once, under its most pressing bucket.
    expect(agentPanel.querySelector('[data-slot="preview-hidden"]')?.textContent).toBe(
      'Not shown: 1 offline · 1 unknown reporting',
    )
  })

  it('names identity mismatch and exception reasons on the preview without opening diagnostics in place', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/overview': () => jsonResponse(OVERVIEW, 200),
      '/api/admin/v1/nodes': () => jsonResponse([
        { ...NODE, health: 'unhealthy', health_reason: 'RPC collection failed', identity: { state: 'mismatched', observed: null, mismatched_fields: ['chain_id', 'genesis_hash'] }, resync_state: 'resyncing' },
      ], 200),
      '/api/admin/v1/agents': () => jsonResponse([AGENT], 200),
    })
    await renderAt('/admin')
    const panel = (await screen.findByRole('heading', { name: 'Node Health Summary' })).closest('article') as HTMLElement
    const item = await within(panel).findByRole('listitem')
    expect(item.textContent).toContain('unhealthy')
    expect(item.textContent).toContain('RPC collection failed')
    expect(item.textContent).toContain('Identity mismatched: chain_id, genesis_hash')
    expect(item.textContent).toContain('Resync resyncing')
    expect(within(item).queryByRole('button')).toBeNull()
  })

  it('keeps the preview order stable across a timestamp-only refetch', async () => {
    const peerNode = { ...NODE, node_id: 'node-2', display_name: 'Node B' }
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/overview': () => jsonResponse(OVERVIEW, 200),
      '/api/admin/v1/nodes': () => jsonResponse([peerNode, NODE], 200),
      '/api/admin/v1/agents': () => jsonResponse([AGENT], 200),
    })
    await renderAt('/admin')
    const panel = (await screen.findByRole('heading', { name: 'Node Health Summary' })).closest('article') as HTMLElement
    const before = (await within(panel).findAllByRole('listitem')).map((item) => item.textContent)
    expect(before[0]).toContain('Node A')
    await act(async () => {
      adminQueryClient.setQueriesData({ queryKey: ['admin', 'nodes'] }, [
        { ...peerNode, updated_at: '2026-08-12T08:02:00Z' },
        { ...NODE, updated_at: '2026-08-12T08:03:00Z' },
      ])
    })
    expect(within(panel).getAllByRole('listitem').map((item) => item.textContent)).toEqual(before)
  })

  it('renders one atomic snapshot time and exactly four linked summary cards', async () => {
    const generatedAt = new Date(Date.now() - 2 * 60 * 1000).toISOString()
    const fetchMock = mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/overview': () =>
        jsonResponse({ ...OVERVIEW, generated_at: generatedAt }, 200),
      '/api/admin/v1/nodes': () => jsonResponse([NODE], 200),
      '/api/admin/v1/agents': () => jsonResponse([AGENT], 200),
    })
    await renderAt('/admin')

    const summary = await screen.findByRole('navigation', { name: 'Overview summaries' })
    const links = within(summary).getAllByRole('link')
    expect(links).toHaveLength(4)
    expect(links.map((link) => link.getAttribute('href'))).toEqual([
      '/admin/agents',
      '/admin/nodes?lifecycle=active',
      '/admin/nodes?lifecycle=retired',
      '/admin/networks',
    ])
    for (const link of links) expect(link.querySelector('a, button')).toBeNull()

    expect(within(summary).getByRole('link', { name: /Agents/ }).textContent).toContain(
      '1 online · 0 offline · 0 unknown',
    )
    expect(within(summary).getByRole('link', { name: /Active Nodes/ }).textContent).toContain(
      '1 healthy · 0 unhealthy · 0 unknown',
    )
    expect(within(summary).getByRole('link', { name: /Retired Nodes/ }).textContent).toContain(
      'Excluded from live health buckets',
    )
    expect(within(summary).getByRole('link', { name: /Networks/ }).textContent).toContain(
      '1 with Network Identity Mismatch',
    )

    const snapshotTime = document.querySelector(`time[datetime="${generatedAt}"]`)
    expect(snapshotTime?.textContent).toBe('2 minutes ago')
    expect(snapshotTime?.getAttribute('aria-label')).toContain('UTC')
    expect(snapshotTime?.getAttribute('title')).toContain('UTC')

    const requestedUrls = fetchMock.mock.calls.map(([input]) =>
      input instanceof Request ? input.url : String(input),
    )
    expect(requestedUrls.filter((url) => url.includes('/api/admin/v1/overview'))).toHaveLength(1)
    expect(requestedUrls.some((url) => url.includes('/api/admin/v1/networks'))).toBe(false)
  })

  it('keeps read-only Overview panels static while interactive summary links keep their feedback', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/overview': () => jsonResponse(OVERVIEW, 200),
      '/api/admin/v1/nodes': () => jsonResponse([NODE], 200),
      '/api/admin/v1/agents': () => jsonResponse([AGENT], 200),
    })
    await renderAt('/admin')

    const panels = document.querySelectorAll('[data-slot="overview-panel"]')
    expect(panels).toHaveLength(3)
    for (const panel of panels) {
      expectStaticReadOnlySurface(panel)
    }

    const summary = await screen.findByRole('navigation', { name: 'Overview summaries' })
    for (const link of within(summary).getAllByRole('link')) {
      expect(link.className).toContain('hover:bg-background')
      expect(link.className).toContain('hover:shadow-emerald-600/10')
    }
  })

  it('separates the snapshot refresh time from the attention observation time', async () => {
    const generatedAt = new Date(Date.now() - 30 * 1000).toISOString()
    const observedAt = new Date(Date.now() - 4 * 60 * 1000).toISOString()
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/overview': () =>
        jsonResponse(
          {
            ...OVERVIEW,
            generated_at: generatedAt,
            attention: [
              { ...OVERVIEW.attention[0], observed_at: observedAt },
              {
                ...OVERVIEW.attention[0],
                id: 'agent_security_event:agent:agent-2',
                kind: 'agent_security_event',
                severity: 'warning',
                subject_kind: 'agent',
                subject_id: 'agent-2',
                subject_label: 'Agent Two',
                message: '1 security event was recorded',
                observed_at: null,
              },
            ],
          },
          200,
        ),
      '/api/admin/v1/nodes': () => jsonResponse([NODE], 200),
      '/api/admin/v1/agents': () => jsonResponse([AGENT], 200),
    })
    await renderAt('/admin')

    // A recorded observation renders its own time, explicitly labelled.
    const knownItem = (await screen.findByText(/RPC collection failed/)).closest('[data-slot="attention-item"]') as HTMLElement
    expect(knownItem.textContent).toContain('Last observed')
    const observedTime = knownItem.querySelector('time')
    expect(observedTime?.getAttribute('datetime')).toBe(observedAt)
    expect(observedTime?.textContent).toBe('4 minutes ago')

    // No observation timestamp: Unknown stays Unknown and never borrows the
    // snapshot refresh time.
    const unknownItem = (await screen.findByText(/1 security event was recorded/)).closest('[data-slot="attention-item"]') as HTMLElement
    expect(unknownItem.textContent).toContain('Observation time unknown')
    expect(unknownItem.querySelector('time')).toBeNull()
    expect(unknownItem.textContent).not.toContain('ago')

    const header = screen.getByText(/Last good snapshot/).closest('[data-slot="header-status"]') as HTMLElement
    expect(header.querySelector('time')?.getAttribute('datetime')).toBe(generatedAt)
    expect(header.textContent).toContain('30 seconds ago')
  })

  it('keeps stale, unknown, and never-observed states distinct on the previews', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/overview': () => jsonResponse(OVERVIEW, 200),
      '/api/admin/v1/nodes': () =>
        jsonResponse(
          [{ ...NODE, health: 'unknown', health_reason: 'The Server has not observed enough current components', freshness: 'stale', current_head: null }],
          200,
        ),
      '/api/admin/v1/agents': () =>
        jsonResponse([{ ...AGENT, liveness: 'unknown', last_received_at: null, host: null, credentials: [{ credential_id: 'c1', active: true, created_at: '2026-08-12T08:00:00Z', revoked_at: null, revoke_after: null }] }], 200),
    })
    await renderAt('/admin')

    const nodePanel = (await screen.findByRole('heading', { name: 'Node Health Summary' })).closest('article') as HTMLElement
    const nodeItem = await within(nodePanel).findByRole('listitem')
    expect(nodeItem.textContent).toContain('unknown')
    expect(nodeItem.textContent).toContain('Stale')
    expect(nodeItem.textContent).toContain('The Server has not observed enough current components')
    expect(nodeItem.textContent).not.toContain('healthy')

    const agentPanel = screen.getByRole('heading', { name: 'Agent inventory' }).closest('article') as HTMLElement
    const agentItem = await within(agentPanel).findByRole('listitem')
    expect(agentItem.textContent).toContain('Unknown')
    expect(agentItem.textContent).toContain('Never received')
    expect(agentItem.textContent).toContain('Host not observed yet')
    expect(agentItem.textContent).not.toMatch(/\b0 queued|Normal/)
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
    expect(await screen.findByRole('link', { name: 'Node A' })).toBeTruthy()
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
              nodes: { total: 0, active: 0, healthy: 0, unhealthy: 0, unknown: 0, retired: 0 },
               networks: { total: 0, with_identity_mismatch: 0 },
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
    expect(screen.getByRole('heading', { level: 2, name: 'Set up your first observation' })).toBeTruthy()
    expect(screen.getByRole('link', { name: 'Register the expected Network identity' }).getAttribute('href')).toBe('/admin/networks')
    expect(screen.getByRole('link', { name: "Configure the Agent's local Node Inventory" }).getAttribute('href')).toBe('/admin/settings')
    expect(screen.queryByText(/RPC Endpoint|Enrollment|fake data|one-click/i)).toBeNull()
  })

  it('does not show setup guidance until all three surfaces prove authoritative emptiness', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/overview': () => jsonResponse({ ...OVERVIEW, summary: { ...OVERVIEW.summary, agents: { total: 0, online: 0, offline: 0, unknown: 0 }, nodes: { ...OVERVIEW.summary.nodes, total: 0, active: 0, healthy: 0, unhealthy: 0, unknown: 0, retired: 0 }, networks: { total: 0, with_identity_mismatch: 0 }, }, attention: [] }, 200),
      '/api/admin/v1/nodes': () => jsonResponse([NODE], 200),
      '/api/admin/v1/agents': () => jsonResponse([], 200),
    })
    await renderAt('/admin')
    await screen.findByRole('heading', { level: 1, name: 'Overview' })
    expect(await screen.findByRole('link', { name: 'Node A' })).toBeTruthy()
    expect(screen.queryByRole('heading', { level: 2, name: 'Set up your first observation' })).toBeNull()
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

describe('Agent inventory risk ordering and evidence', () => {
  it('sorts critical, offline, unknown, and online Agents before limiting to six', () => {
    const agent = (id: string, liveness: string, extra: Partial<AgentDiagnostic> = {}): AgentDiagnostic => ({
      ...AGENT, agent_id: id, liveness, ...extra,
    }) as AgentDiagnostic
    const criticalHost: HostDiagnostic = { components: [], updated_at: '2026-08-12T08:00:00Z', spool_store_fatal: true }
    const ordered = prioritizeAgents([
      agent('online-z', 'online'),
      agent('offline', 'offline'),
      agent('critical', 'online', { security_event_count: 1 }),
      agent('unknown', 'unknown'),
      agent('critical-spool', 'online', { host: criticalHost }),
      agent('online-a', 'online'),
    ])
    expect(ordered.map((entry) => entry.agent_id)).toEqual([
      'critical', 'critical-spool', 'offline', 'unknown', 'online-a', 'online-z',
    ])
  })

  it('prioritizes before slicing and keeps the fifth-item boundary deterministic', async () => {
    const agents = [
      ...Array.from({ length: 4 }, (_, index) => ({ ...AGENT, agent_id: 'online-' + index, liveness: 'online' })),
      { ...AGENT, agent_id: 'offline-a', liveness: 'offline' },
      { ...AGENT, agent_id: 'unknown-a', liveness: 'unknown' },
      { ...AGENT, agent_id: 'critical-a', security_event_count: 1 },
    ]
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/overview': () => jsonResponse(OVERVIEW, 200),
      '/api/admin/v1/nodes': () => jsonResponse([], 200),
      '/api/admin/v1/agents': () => jsonResponse(agents, 200),
    })
    await renderAt('/admin')
    await screen.findByRole('link', { name: 'critical-a' })
    const agentPanel = screen.getByRole('heading', { level: 2, name: 'Agent inventory' }).closest('article') as HTMLElement
    const items = within(agentPanel).getAllByRole('listitem')
    expect(items).toHaveLength(5)
    expect(items.map((item) => item.textContent)).toEqual([
      expect.stringContaining('critical-a'),
      expect.stringContaining('offline-a'),
      expect.stringContaining('unknown-a'),
      expect.stringContaining('online-0'),
      expect.stringContaining('online-1'),
    ])
    expect(screen.queryByText('online-2', { exact: true })).toBeNull()
    expect(within(agentPanel).getByText(/Showing 5 of 7 Agents/)).toBeTruthy()
    expect(screen.getByRole('link', { name: 'View all Agents' }).getAttribute('href')).toBe('/admin/agents')
  })

  it('separates current attention from recorded history and leaves Host resources to the Agent detail', async () => {
    const host: HostDiagnostic = {
      components: [], updated_at: '2026-08-12T08:00:00Z', cpu_percent: 37.5,
      memory_used_bytes: 4 * 1024 ** 3, memory_total_bytes: 8 * 1024 ** 3,
      spool_queued_reports: 3, spool_capacity_bytes: 16 * 1024 ** 3,
      spool_dropped_sequence_from: 10, spool_dropped_sequence_to: 12, spool_pending_history_gaps: 2,
    }
    const diagnostic = {
      ...AGENT, host, security_event_count: 2, sequence_gap_count: 1, clock_status: 'unknown', clock_skew_ms: null,
      credentials: [{ credential_id: 'c1', active: true, created_at: '2026-08-12T08:00:00Z', revoked_at: null, revoke_after: null }],
      attention: [{ ...OVERVIEW.attention[0], id: 'a1', kind: 'agent_spool_overflow', subject_kind: 'agent', subject_id: 'agent-1', message: 'the Agent spool overflowed and discarded queued reports' }],
    }
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/overview': () => jsonResponse(OVERVIEW, 200),
      '/api/admin/v1/nodes': () => jsonResponse([{ ...NODE, display_name: 'Joined Node', health: 'unhealthy' }], 200),
      '/api/admin/v1/agents': () => jsonResponse([diagnostic], 200),
    })
    await renderAt('/admin')
    const link = await screen.findByRole('link', { name: 'agent-1' })
    expect(link.getAttribute('href')).toBe('/admin/agents/agent-1')
    const item = link.closest('li') as HTMLElement
    expect(item.querySelector('[data-slot="agent-preview-current"]')?.textContent).toBe(
      'the Agent spool overflowed and discarded queued reports',
    )
    expect(item.querySelector('[data-slot="agent-preview-recorded"]')?.textContent).toBe(
      'Recorded history: 1 gap interval · 2 security events · +2 more on the Agent',
    )
    expect(item.textContent).toContain('1 retained Node')
    expect(item.textContent).toContain('1 unhealthy')
    expect(item.textContent).not.toMatch(/37\.5%|4\.00 GiB|3 queued|capacity|Clock/)
    expect(item.textContent).not.toContain('0195f2a1-2b3c-4d5e-8f90-123456789abc')
  })

  it('keeps Agent items visible when Nodes fail, with the retained count from the Agent itself', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/overview': () => jsonResponse(OVERVIEW, 200),
      '/api/admin/v1/nodes': () => jsonResponse({ error: { code: 'unavailable', message: 'Nodes unavailable' } }, 503),
      '/api/admin/v1/agents': () => jsonResponse([{ ...AGENT, nodes: [] }], 200),
    })
    await renderAt('/admin')
    const item = (await screen.findByRole('link', { name: 'agent-1' })).closest('li') as HTMLElement
    expect(item.textContent).toContain('0 retained Nodes')
    expect(await screen.findByText('Nodes unavailable')).toBeTruthy()
  })

})
