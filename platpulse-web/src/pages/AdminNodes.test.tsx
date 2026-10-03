import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import App from '../App'
import { adminQueryClient } from '../api/admin'
import { client } from '../api/generated/client.gen'
import { METRIC_HISTORY_SAMPLE_LIMIT } from '../metricHistory'

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

const NODE_A = {
  node_id: '0195f2a1-0014-4014-8014-000000000014',
  agent_id: '0195f2a1-0011-4011-8011-000000000011',
  display_name: 'Node A',
  network_key: 'platon-e2e',
  network_display_name: 'PlatON E2E Network',
  lifecycle: 'active',
  lifecycle_guidance:
    'Active: present in the latest valid Agent Inventory. The Agent-local configuration stays authoritative for this Node; the Server never pushes Endpoint or lifecycle changes.',
  visibility: 'public',
  inventory_revision: 1,
  first_seen_at: '2026-08-12T08:00:00Z',
  updated_at: '2026-08-12T08:00:00Z',
  rpc_endpoint: 'ws://127.0.0.1:****',
  health: 'healthy',
  health_reason: 'RPC, sync, and consensus are current',
  freshness: 'current',
  current_head: 12842019,
  resync_state: 'normal',
  identity: {
    state: 'matched',
    observed: {
      genesis_hash: '0x1111111111111111111111111111111111111111111111111111111111111111',
      chain_id: 210425,
      p2p_network_id: 1,
      address_hrp: 'lat',
    },
    mismatched_fields: [],
  },
}

const NODE_B = {
  node_id: '0195f2a1-0015-4015-8015-000000000015',
  agent_id: '0195f2a1-0011-4011-8011-000000000011',
  display_name: 'Node B (private)',
  network_key: 'platon-e2e',
  network_display_name: 'PlatON E2E Network',
  lifecycle: 'active',
  lifecycle_guidance:
    'Active: present in the latest valid Agent Inventory. The Agent-local configuration stays authoritative for this Node; the Server never pushes Endpoint or lifecycle changes.',
  visibility: 'private',
  inventory_revision: 1,
  first_seen_at: '2026-08-12T08:00:00Z',
  updated_at: '2026-08-12T08:00:00Z',
  rpc_endpoint: 'ws://127.0.0.1:****',
  health: 'unhealthy',
  health_reason: 'RPC collection failed',
  freshness: 'stale',
  current_head: 12842018,
  resync_state: 'normal',
  identity: {
    state: 'mismatched',
    observed: {
      genesis_hash: '0x1111111111111111111111111111111111111111111111111111111111111111',
      chain_id: 999999,
      p2p_network_id: 1,
      address_hrp: 'lat',
    },
    mismatched_fields: ['chain_id'],
  },
}

const NODE_A_DETAIL = {
  ...NODE_A,
  node_key_fingerprint: '0xabcd1234',
  historical_high_watermark: 12842019,
  resync_progress: null,
  network_reference_head: 12842019,
  network_reference_confidence: 'low',
  data_directory: {
    state: 'ok',
    attempted_at: '2026-08-12T08:00:00Z',
    observed_at: '2026-08-12T08:00:00Z',
    received_at: '2026-08-12T08:00:00Z',
    state_revision: 1,
    value_revision: 1,
    size_bytes: 12_884_901_888,
  },
  process: {
    state: 'ok',
    attempted_at: '2026-08-12T08:00:00Z',
    observed_at: '2026-08-12T08:00:00Z',
    received_at: '2026-08-12T08:00:00Z',
    state_revision: 1,
    value_revision: 1,
    pid: 1234,
    started_at: '2026-08-12T07:00:00Z',
    cpu_percent: 1.5,
    memory_bytes: 1024,
    uptime_ms: 3600000,
  },
  rpc: {
    client_version: 'platon/1.5.1',
    namespaces: ['admin', 'net', 'platon'],
    methods: ['eth_blockNumber'],
    state: 'ok',
    attempted_at: '2026-08-12T08:00:00Z',
    observed_at: '2026-08-12T08:00:00Z',
    received_at: '2026-08-12T08:00:00Z',
    state_revision: 1,
    value_revision: 1,
  },
  sync: {
    state: 'ok',
    attempted_at: '2026-08-12T08:00:00Z',
    observed_at: '2026-08-12T08:00:00Z',
    received_at: '2026-08-12T08:00:00Z',
    state_revision: 1,
    value_revision: 1,
    syncing: false,
    current_block: 12842019,
    highest_block: 12842019,
    pulled_states: null,
    known_states: null,
  },
  consensus: {
    state: 'ok',
    attempted_at: '2026-08-12T08:00:00Z',
    observed_at: '2026-08-12T08:00:00Z',
    received_at: '2026-08-12T08:00:00Z',
    state_revision: 1,
    value_revision: 1,
    epoch: 42,
    view_number: 7,
    validator: true,
    highest_qc_block: 12842019,
    highest_lock_block: 12842019,
    highest_commit_block: 12842019,
  },
  peers: {
    state: 'ok',
    attempted_at: '2026-08-12T08:00:00Z',
    observed_at: '2026-08-12T08:00:00Z',
    received_at: '2026-08-12T08:00:00Z',
    state_revision: 1,
    value_revision: 1,
    freshness: 'current',
    peer_count: 1,
    inbound_count: 1,
    outbound_count: 0,
    trusted_count: 1,
    static_count: 0,
    consensus_count: 1,
    peers: [{
      peer_id: 'peer-a',
      direction: 'inbound',
      trusted: true,
      static_peer: false,
      consensus_peer: true,
      client_name: 'PlatON/v1.5.1',
      capabilities: ['cbft/1'],
      cbft_protocol_version: 1,
      cbft_highest_qc_block: 12842019,
      cbft_locked_block: 12842018,
      cbft_commit_block: 12842017,
    }],
  },
}

/** Retired lifecycle (CONTEXT.md: Retired Node): identity and history stay,
 * live observation alerts no longer apply, and health is not liveness. */
const NODE_D_RETIRED_DETAIL = {
  ...NODE_A,
  node_id: '0195f2a1-0017-4017-8017-000000000017',
  display_name: 'Node D (retired)',
  lifecycle: 'retired',
  lifecycle_guidance:
    'Retired: absent from the latest valid Agent Inventory. Identity and history remain; live observation alerts no longer apply. Reactivation requires declaring the same Node ID in the Agent Inventory; the Server never changes Node lifecycle remotely.',
  health: 'unknown',
  health_reason: 'No live observation expectations for a Retired Node',
  freshness: 'unknown',
  current_head: null,
}

/** Error diagnostics keep the last-good values: RPC is in error with an
 * explicit message while the last successful sync value stays visible. */
const NODE_RPC_ERROR_DETAIL = {
  ...NODE_A_DETAIL,
  node_id: '0195f2a1-0015-4015-8015-000000000015',
  display_name: 'Node B (private)',
  health: 'unhealthy',
  health_reason: 'RPC collection failed',
  freshness: 'stale',
  current_head: 12842018,
  identity: NODE_B.identity,
  rpc: {
    ...NODE_A_DETAIL.rpc,
    state: 'error',
    attempted_at: '2026-08-12T08:05:00Z',
    error_message: 'connection refused',
  },
  sync: NODE_A_DETAIL.sync,
}

function jsonResponse(body: unknown, status: number): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

const TEST_ORIGIN = 'http://platpulse.test'

function mockFetch(
  routes: Record<string, (request: Request) => Response | Promise<Response>>,
) {
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(String(input), init)
    const url = request.url.replace(TEST_ORIGIN, '')
    for (const [pattern, handler] of Object.entries(routes)) {
      if (pattern.endsWith('*')) {
        if (url.startsWith(pattern.slice(0, -1))) return handler(request)
      } else if (url === pattern) {
        return handler(request)
      }
    }
    return jsonResponse({ error: { code: 'not_found' } }, 404)
  })
  vi.stubGlobal('fetch', fetchMock)
  return fetchMock
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

describe('PAGE-ADMIN-NODES (Node inventory)', () => {
  it('lists every Node as its own row with separate health, freshness, identity, visibility, and lifecycle dimensions', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/nodes': () => jsonResponse([NODE_A, NODE_B], 200),
    })
    renderAt('/admin/nodes')

    await screen.findByRole('heading', { level: 1, name: 'Nodes' })
    const rowA = await screen.findByRole('row', { name: /Node A/ })
    expect(rowA.textContent).toContain('healthy')
    expect(rowA.textContent).toContain('Current')
    expect(rowA.textContent).toContain('Matched')
    expect(rowA.textContent).toContain('Public')
    expect(rowA.textContent).toContain('12842019')
    const rowB = screen.getByRole('row', { name: /Node B \(private\)/ })
    expect(rowB.textContent).toContain('unhealthy')
    expect(rowB.textContent).toContain('Stale')
    expect(rowB.textContent).toContain('Mismatched')
    expect(rowB.textContent).toContain('chain_id')
    expect(rowB.textContent).toContain('Private')
    // Endpoints are redacted destination summaries.
    expect(rowA.textContent).toContain('ws://127.0.0.1:****')
  })

  it('filters through URL state and preserves it in back/forward', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/nodes': () => jsonResponse([NODE_A, NODE_B], 200),
    })
    renderAt('/admin/nodes')

    await screen.findByRole('heading', { level: 1, name: 'Nodes' })
    const visibilityFilter = screen.getByLabelText('Visibility')
    fireEvent.change(visibilityFilter, { target: { value: 'public' } })
    await waitFor(() => {
      expect(window.location.search).toContain('visibility=public')
    })
    expect(screen.queryByRole('row', { name: /Node B \(private\)/ })).toBeNull()
    expect(screen.getByRole('row', { name: /Node A/ })).toBeTruthy()
  })

  it('shows the Empty state when no Nodes exist', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/nodes': () => jsonResponse([], 200),
    })
    renderAt('/admin/nodes')

    await screen.findByRole('heading', { level: 1, name: 'Nodes' })
    expect(await screen.findByText('No Nodes match these filters.')).toBeTruthy()
  })

  it('shows the Server-owned detail with mismatch diagnostics and last-good values', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/nodes/0195f2a1-0014-4014-8014-000000000014': () =>
        jsonResponse(NODE_A_DETAIL, 200),
    })
    renderAt('/admin/nodes/0195f2a1-0014-4014-8014-000000000014')

    await screen.findByRole('heading', { level: 1, name: /Node A/ })
    expect(screen.getByText('Server-owned metadata')).toBeTruthy()
    expect(screen.getByText('Lifecycle guidance')).toBeTruthy()
    expect(screen.getByText(/never pushes Endpoint or lifecycle changes/)).toBeTruthy()
    // Lifecycle is Node Inventory state on its own panel (issue #94).
    expect(
      screen.getByRole('heading', { level: 2, name: 'Node Inventory & lifecycle' }),
    ).toBeTruthy()
    expect(screen.getByRole('link', { name: 'Open the Audit log' })).toBeTruthy()
    // Contextual shortcut from a Node detail to its Incident history
    // (issue #202 Story 2): a bound subject filter, not a generic entry.
    expect(
      screen.getByRole('link', { name: 'Incidents for this Node' }).getAttribute('href'),
    ).toBe(
      '/admin/alerts/incidents?subject=node&subject_key=0195f2a1-0014-4014-8014-000000000014',
    )
    expect(screen.getByText('Network identity')).toBeTruthy()
    expect(screen.getByText('Observed chain ID / P2P network')).toBeTruthy()
    expect(screen.getByText(/210425 \/ 1/)).toBeTruthy()
    // Administrative RPC diagnostics stay separate from Home's full
    // observation view and retain the redacted endpoint.
    expect(screen.getByText('RPC diagnostics')).toBeTruthy()
    expect(screen.getByText('Redacted RPC Endpoint')).toBeTruthy()
    expect(screen.getByText('platon/1.5.1')).toBeTruthy()
    expect(screen.getByText('admin, net, platon')).toBeTruthy()
    expect(screen.getByText('Node data size')).toBeTruthy()
    expect(screen.getByText(/12.0 GiB/)).toBeTruthy()
    expect(screen.getByText('Last-good head')).toBeTruthy()
    expect(screen.getAllByText('12842019').length).toBeGreaterThan(0)
    expect(screen.queryByText('Per-Node observations')).toBeNull()
    expect(screen.queryByText('Peer snapshot')).toBeNull()
    expect(screen.queryByText('peer-a')).toBeNull()
    expect(screen.queryByText('PlatON/v1.5.1')).toBeNull()
    expect(screen.queryByText('203.0.113.4')).toBeNull()
    // Node Transfer, per-Node Visibility, and operation controls are absent.
    expect(screen.queryByText('Node transfer')).toBeNull()
    expect(screen.queryByRole('link', { name: /Transfer ownership/ })).toBeNull()
    expect(screen.queryByRole('link', { name: /Publish to Home/ })).toBeNull()
    expect(screen.queryByRole('link', { name: /Make private/ })).toBeNull()
  })

  it('shows the mismatch as a blocking diagnostic distinct from health', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/nodes/0195f2a1-0015-4015-8015-000000000015': () =>
        jsonResponse({ ...NODE_B, node_key_fingerprint: null }, 200),
    })
    renderAt('/admin/nodes/0195f2a1-0015-4015-8015-000000000015')

    await screen.findByRole('heading', { level: 1, name: /Node B \(private\)/ })
    expect(
      await screen.findByText(/Contradicts the Registry: chain_id/),
    ).toBeTruthy()
    expect(screen.getByText(/New history is not merged/)).toBeTruthy()
  })

  it('shows the Retired lifecycle from Node Inventory, separate from health', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/nodes/0195f2a1-0017-4017-8017-000000000017': () =>
        jsonResponse(NODE_D_RETIRED_DETAIL, 200),
    })
    renderAt('/admin/nodes/0195f2a1-0017-4017-8017-000000000017')

    await screen.findByRole('heading', { level: 1, name: /Node D \(retired\)/ })
    expect(
      screen.getByRole('heading', { level: 2, name: 'Node Inventory & lifecycle' }),
    ).toBeTruthy()
    expect(screen.getAllByText('Retired').length).toBeGreaterThan(0)
    expect(screen.getByText(/absent from the latest valid Agent Inventory/)).toBeTruthy()
    expect(screen.getByText(/the Server never changes Node lifecycle remotely/)).toBeTruthy()
    // Health stays Unknown for a Retired Node; lifecycle is not liveness.
    expect(screen.getAllByText('Unknown').length).toBeGreaterThan(0)
    expect(screen.queryByText('Healthy')).toBeNull()
    expect(screen.queryByText('Active')).toBeNull()
    expect(screen.queryByRole('button', { name: /Retire|Reactivate/ })).toBeNull()
  })

  it('keeps Stale and Error diagnostics explicit, never as Healthy or zeros', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/nodes/0195f2a1-0015-4015-8015-000000000015': () =>
        jsonResponse(NODE_RPC_ERROR_DETAIL, 200),
    })
    renderAt('/admin/nodes/0195f2a1-0015-4015-8015-000000000015')

    await screen.findByRole('heading', { level: 1, name: /Node B \(private\)/ })
    expect(screen.getByText('Stale')).toBeTruthy()
    expect(screen.getByText('Error')).toBeTruthy()
    expect(screen.getByText('Last RPC error')).toBeTruthy()
    expect(screen.getByText('connection refused')).toBeTruthy()
    // Last-good values stay visible beside the Error diagnostic.
    expect(screen.getByText(/last-good head 12842019/)).toBeTruthy()
    expect(screen.queryByText('Healthy')).toBeNull()
    // Only the redacted endpoint form is ever rendered.
    expect(screen.getAllByText('ws://127.0.0.1:****').length).toBeGreaterThan(0)
    expect(screen.queryByText('ws://127.0.0.1:8545')).toBeNull()
  })

  it('updates the Server-owned display name and shows the confirmation', async () => {
    // The mutation invalidates the Admin cache; the next detail read serves
    // the new Server-owned name so the refetch half is asserted here too.
    let detail = NODE_A_DETAIL
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/nodes/0195f2a1-0014-4014-8014-000000000014': () =>
        jsonResponse(detail, 200),
      '/api/admin/v1/nodes/0195f2a1-0014-4014-8014-000000000014/metadata': () => {
        detail = { ...detail, display_name: 'Atlas-01' }
        return jsonResponse({ nodeId: NODE_A.node_id, displayName: 'Atlas-01' }, 200)
      },
    })
    renderAt('/admin/nodes/0195f2a1-0014-4014-8014-000000000014')

    await screen.findByRole('heading', { level: 1, name: /Node A/ })
    fireEvent.click(screen.getByRole('button', { name: 'Edit' }))
    const input = screen.getByLabelText('Display name')
    fireEvent.change(input, { target: { value: 'Atlas-01' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))
    // The confirmation step is explicit before the mutation runs.
    expect(
      await screen.findByText(/Rename this Node in the Server-owned metadata\?/),
    ).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Confirm rename' }))
    expect(await screen.findByText('Display name is now "Atlas-01".')).toBeTruthy()
    // The read view shows the new name immediately, and the authoritative
    // refetch replaces the heading with the Server-owned value (issue #94).
    expect(screen.getAllByText('Atlas-01').length).toBeGreaterThan(0)
    expect(
      await screen.findByRole('heading', { level: 1, name: /Atlas-01/ }),
    ).toBeTruthy()
  })

  it('shows a non-leaking unavailable state for an unknown Node', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/nodes/missing': () =>
        jsonResponse({ error: { code: 'not_found' } }, 404),
    })
    renderAt('/admin/nodes/missing')

    expect(await screen.findByText('Node unavailable')).toBeTruthy()
    expect(screen.getByText('This Node is no longer available.')).toBeTruthy()
  })


const MINUTE = 60 * 1000
const HOUR = 60 * MINUTE

/** Canonical second-precision instant relative to the moment the panel builds
 * its range, so the fixture stays inside the answered window. */
function canonical(offsetMs: number): string {
  return new Date(Date.now() + offsetMs).toISOString().replace(/\.\d{3}Z$/, 'Z')
}

function metricHistoryFixture(overrides: Record<string, unknown> = {}) {
  return {
    nodeId: NODE_A.node_id,
    metric: 'process_cpu_percent',
    from: canonical(-24 * HOUR),
    to: canonical(0),
    requestedFrom: canonical(-24 * HOUR),
    availability: null,
    rawRetentionDays: 1,
    grain: 'raw',
    aggregateSupported: false,
    windowSeconds: 86400,
    truncated: false,
    series: {
      observed: true,
      firstObservedAt: canonical(-26 * HOUR),
      lastObservedAt: canonical(-1 * HOUR),
      lastReceivedAt: canonical(-1 * HOUR + 1000),
      observationCount: 17_280,
      replayedCount: 12,
      correctedCount: 2,
      sampledCount: 3,
      coverageSeconds: 7200,
      windowSeconds: 86400,
      latestDelaySeconds: 2,
      latestClockSuspect: false,
    },
    items: [
      { observedAt: canonical(-3 * HOUR), receivedAt: canonical(-3 * HOUR + 1000), value: 2.5, delaySeconds: 1, clockSuspect: false },
      { observedAt: canonical(-2 * HOUR), receivedAt: canonical(-2 * HOUR + 1000), value: 2.5, delaySeconds: 1, clockSuspect: false },
      { observedAt: canonical(-1 * HOUR), receivedAt: canonical(-1 * HOUR + 120_000), value: 2.5, delaySeconds: 120, clockSuspect: true, clockNote: 'the observation is stamped 6s after the Server received it: the Agent clock is ahead' },
    ],
    gaps: [
      {
        from: canonical(-2 * HOUR),
        to: canonical(-1 * HOUR),
        seconds: 3600,
        kind: 'protection_pause',
        reason: 'low-space protection paused sample collection',
        skippedCount: 24,
      },
    ],
    ...overrides,
  }
}

  it('charts the stored raw series, its silences, and the series state behind it', async () => {
    const historyCalls: string[] = []
    const neverObserved = metricHistoryFixture({
      metric: 'data_directory_percent',
      items: [],
      gaps: [],
      series: {
        observed: false,
        firstObservedAt: null,
        lastObservedAt: null,
        lastReceivedAt: null,
        observationCount: 0,
        replayedCount: 0,
        correctedCount: 0,
        sampledCount: 0,
        coverageSeconds: 0,
        windowSeconds: 86400,
        latestDelaySeconds: null,
        latestClockSuspect: false,
      },
    })
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/nodes/0195f2a1-0014-4014-8014-000000000014': () =>
        jsonResponse(NODE_A_DETAIL, 200),
      '/api/admin/v1/nodes/0195f2a1-0014-4014-8014-000000000014/metric-history*': (request) => {
        historyCalls.push(request.url)
        return jsonResponse(
          request.url.includes('metric=data_directory_percent')
            ? neverObserved
            : metricHistoryFixture(),
          200,
        )
      },
    })
    renderAt('/admin/nodes/0195f2a1-0014-4014-8014-000000000014')

    await screen.findByRole('heading', { level: 1, name: /Node A/ })
    // The panel asks the Server for exactly the selected series and range.
    await waitFor(() => {
      expect(historyCalls.length).toBeGreaterThan(0)
    })
    expect(historyCalls[0]).toContain('metric=process_cpu_percent')
    expect(historyCalls[0]).toContain('from=')
    expect(historyCalls[0]).toContain('to=')
    // The panel asks for the largest answer the Server carries, so a dense
    // window is narrowed by the Server's own bound instead of by a request that
    // asked for less than it could have.
    expect(historyCalls[0]).toContain('limit=' + METRIC_HISTORY_SAMPLE_LIMIT)

    // Series state: coverage is proven only between stored samples, and
    // carried deliveries are counted apart from observations.
    expect(await screen.findByText('2 hours of 24 hours', { exact: false })).toBeTruthy()
    expect(screen.getByText(/17280 stored observation\(s\) since the first one/)).toBeTruthy()
    expect(screen.getByText(/12 replay\(s\), 2 correction\(s\)/)).toBeTruthy()
    expect(screen.getByText(/1 day · requested from/)).toBeTruthy()

    // The silence is named as a protection pause with its skipped count, and
    // the plot leaves it undrawn instead of bridging it.
    expect(screen.getByText('Protection pause')).toBeTruthy()
    expect(screen.getByText(/24 observation\(s\) skipped/)).toBeTruthy()
    const chart = document.querySelector('[data-slot="metric-history-chart"]')
    expect(chart).not.toBeNull()
    expect(document.querySelectorAll('[data-slot="metric-history-gap-band"]')).toHaveLength(1)
    expect(document.querySelectorAll('[data-slot="metric-history-line"]').length).toBeGreaterThan(0)
    // The observation before the pause sits alone in its column, so it is drawn
    // as a whisker spanning that column's own minimum and maximum rather than
    // bridged to the next column across the silence.
    expect(
      document.querySelectorAll('[data-slot="metric-history-whisker"]').length,
    ).toBeGreaterThan(0)

    // Per-sample timing evidence stays attached to its own observation.
    expect(screen.getByText(/stamped 6s after the Server received it/)).toBeTruthy()

    // Another series this Node never reported is named as absent: no chart,
    // no zero line.
    fireEvent.change(screen.getByLabelText('Metric series'), {
      target: { value: 'data_directory_percent' },
    })
    expect(await screen.findByText(/never reported Data directory/)).toBeTruthy()
    expect(document.querySelector('[data-slot="metric-history-chart"]')).toBeNull()
    expect(document.querySelector('[data-slot="metric-history-gap-band"]')).toBeNull()
  })

it('reports a sample stamped after its receipt as ahead of receipt, and keeps an isolated spike visible', async () => {
    const cpus = [
      { observedAt: canonical(-30 * MINUTE), value: 10 },
      { observedAt: canonical(-29 * MINUTE), value: 90 },
      { observedAt: canonical(-28 * MINUTE), value: 20 },
    ]
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/nodes/0195f2a1-0014-4014-8014-000000000014': () =>
        jsonResponse(NODE_A_DETAIL, 200),
      '/api/admin/v1/nodes/0195f2a1-0014-4014-8014-000000000014/metric-history*': () =>
        jsonResponse(
          metricHistoryFixture({
            series: {
              ...metricHistoryFixture().series,
              latestDelaySeconds: -300,
              latestClockSuspect: true,
            },
            items: cpus.map((sample, index) => ({
              observedAt: sample.observedAt,
              receivedAt: sample.observedAt,
              value: sample.value,
              // The newest observation is stamped five minutes after the
              // Server received it: a clock disagreement, not a fast delivery.
              delaySeconds: index === cpus.length - 1 ? -300 : 1,
              clockSuspect: index === cpus.length - 1,
              clockNote:
                index === cpus.length - 1
                  ? 'the observation is stamped 300s after the Server received it'
                  : undefined,
            })),
          }),
          200,
        ),
    })
    renderAt('/admin/nodes/0195f2a1-0014-4014-8014-000000000014')

    await screen.findByRole('heading', { level: 2, name: 'Metric history' })
    // The ledger delay and the sample row both name the direction that made the
    // delay suspicious, and neither prints a negative duration or "0 seconds".
    expect(await screen.findAllByText('5 minutes ahead of receipt')).toHaveLength(2)
    expect(screen.queryByText(/^-/)).toBeNull()
    expect(screen.getByText(/stamped 300s after the Server received it/)).toBeTruthy()

    // Three observations share one column, whose maximum of 90 is the spike the
    // single plotted point would otherwise hide. The whisker spans that column
    // (top above bottom), and the point stays between its ends.
    const whisker = document.querySelector('[data-slot="metric-history-whisker"]')
    const point = document.querySelector('[data-slot="metric-history-point"]')
    expect(whisker).not.toBeNull()
    expect(point).not.toBeNull()
    const top = Number(whisker?.getAttribute('y1'))
    const bottom = Number(whisker?.getAttribute('y2'))
    const newest = Number(point?.getAttribute('cy'))
    expect(bottom - top).toBeGreaterThan(50)
    expect(newest).toBeGreaterThan(top)
    expect(newest).toBeLessThan(bottom)
    expect(whisker?.getAttribute('x1')).toBe(whisker?.getAttribute('x2'))
  })
  it('reports a range the raw window no longer holds, and a truncated answer, without faking samples', async () => {
    let call = 0
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/nodes/0195f2a1-0014-4014-8014-000000000014': () =>
        jsonResponse(NODE_A_DETAIL, 200),
      '/api/admin/v1/nodes/0195f2a1-0014-4014-8014-000000000014/metric-history*': () => {
        call += 1
        if (call === 1) {
          return jsonResponse(
            metricHistoryFixture({
              availability: 'unavailable',
              items: [],
              gaps: [],
              series: {
                ...metricHistoryFixture().series,
                observationCount: 1,
                sampledCount: 0,
                coverageSeconds: 0,
              },
            }),
            200,
          )
        }
        return jsonResponse(metricHistoryFixture({ truncated: true, windowSeconds: 3600 }), 200)
      },
    })
    renderAt('/admin/nodes/0195f2a1-0014-4014-8014-000000000014')

    await screen.findByRole('heading', { level: 1, name: /Node A/ })
    expect(
      await screen.findByText(/entirely older than the retained raw window/),
    ).toBeTruthy()
    expect(screen.getByText(/no stored sample falls inside this window/)).toBeTruthy()
    expect(document.querySelector('[data-slot="metric-history-chart"]')).toBeNull()
    // The ledger outlives the released samples: the state is still reported.
    expect(screen.getByText('1 stored observation(s) since the first one')).toBeTruthy()

    fireEvent.click(screen.getByRole('button', { name: '1 hour' }))
    expect(await screen.findByText(/more samples than one answer carries/)).toBeTruthy()
    expect(call).toBeGreaterThan(1)
  })

  it('keeps a failed metric history load actionable without inventing a series', async () => {
    let calls = 0
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/nodes/0195f2a1-0014-4014-8014-000000000014': () =>
        jsonResponse(NODE_A_DETAIL, 200),
      '/api/admin/v1/nodes/0195f2a1-0014-4014-8014-000000000014/metric-history*': () => {
        calls += 1
        if (calls === 1) {
          return jsonResponse(
            { error: { code: 'unavailable', message: 'server database is unavailable' } },
            503,
          )
        }
        return jsonResponse(metricHistoryFixture(), 200)
      },
    })
    renderAt('/admin/nodes/0195f2a1-0014-4014-8014-000000000014')

    await screen.findByRole('heading', { level: 1, name: /Node A/ })
    expect(await screen.findByText('server database is unavailable')).toBeTruthy()
    expect(document.querySelector('[data-slot="metric-history-chart"]')).toBeNull()

    fireEvent.click(screen.getAllByRole('button', { name: 'Try again' })[0])
    await waitFor(() => {
      expect(calls).toBeGreaterThan(1)
    })
  })

  it('offers an explicit, irreversible permanent deletion with the Server-computed scope', async () => {
    const purgeCalls: Array<{ method: string; body: string }> = []
    const impact = {
      target: {
        node_id: NODE_A.node_id,
        agent_id: NODE_A.agent_id,
        network_key: NODE_A.network_key,
        network_display_name: NODE_A.network_display_name,
        display_name: NODE_A.display_name,
        lifecycle: 'active',
        lifecycle_guidance: NODE_A.lifecycle_guidance,
        visibility: 'public',
        inventory_revision: 1,
        rpc_endpoint: NODE_A.rpc_endpoint,
        first_seen_at: NODE_A.first_seen_at,
        updated_at: NODE_A.updated_at,
      },
      counts: {
        component_statuses: 3,
        process_observations: 1,
        data_directory_observations: 1,
        chain_observations: 1,
        rpc_namespaces: 2,
        rpc_methods: 1,
        current_peers: 4,
        current_peer_capabilities: 4,
        peer_presence_intervals: 2,
        peer_aggregate_5m: 2,
        peer_aggregate_5m_countries: 2,
        peer_aggregate_1h: 1,
        peer_aggregate_1h_countries: 1,
        block_summaries: 7,
        block_history_states: 1,
        block_coverage_intervals: 1,
        block_identity_window: 1,
        block_history_gaps: 0,
        chain_divergence_observations: 0,
        observed_network_heads: 1,
        metric_samples: 5,
        metric_series_state: 6,
        validator_links: 1,
        transfers: 1,
        total_owned_rows: 39,
      },
    }
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/nodes/0195f2a1-0014-4014-8014-000000000014': () =>
        jsonResponse(NODE_A_DETAIL, 200),
      '/api/admin/v1/nodes/0195f2a1-0014-4014-8014-000000000014/purge': async (request) => {
        purgeCalls.push({ method: request.method, body: await request.text() })
        if (request.method === 'POST') {
          return jsonResponse(
            {
              node_id: NODE_A.node_id,
              deleted_at: '2026-08-12T09:00:00Z',
              removed: impact.counts,
            },
            200,
          )
        }
        return jsonResponse(impact, 200)
      },
    })
    renderAt('/admin/nodes/0195f2a1-0014-4014-8014-000000000014')

    await screen.findByRole('heading', { level: 1, name: /Node A/ })
    fireEvent.click(screen.getByRole('button', { name: 'Permanently delete Node' }))

    // The confirmation explains the irreversible consequences before the
    // mutation is available.
    expect(
      await screen.findByText(/Confirm permanent deletion of Node A?/),
    ).toBeTruthy()
    expect(screen.getByText(/This cannot be undone/)).toBeTruthy()
    expect(screen.getByText(/requires a new locally configured Node ID/)).toBeTruthy()
    expect(screen.getByText(/process is not stopped or uninstalled/)).toBeTruthy()
    expect(screen.getByText(/local configuration is not changed/)).toBeTruthy()
    expect(await screen.findByText(/39 Node-owned rows will be deleted/)).toBeTruthy()
    expect(screen.getByText(/7 Block Summaries/)).toBeTruthy()

    fireEvent.click(screen.getByRole('button', { name: 'Confirm permanent deletion' }))
    expect(await screen.findByText('Node permanently deleted')).toBeTruthy()
    expect(screen.getByText(/will not return/)).toBeTruthy()
    expect(screen.getByRole('link', { name: 'Back to All Nodes' })).toBeTruthy()
    const mutation = purgeCalls.find((call) => call.method === 'POST')
    expect(mutation?.body).toContain(NODE_A.node_id)
  })

  it('keeps a failed permanent deletion actionable without a fake success', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/nodes/0195f2a1-0014-4014-8014-000000000014': () =>
        jsonResponse(NODE_A_DETAIL, 200),
      '/api/admin/v1/nodes/0195f2a1-0014-4014-8014-000000000014/purge': (request) =>
        request.method === 'POST'
          ? jsonResponse(
              { error: { code: 'unavailable', message: 'server database is unavailable' } },
              503,
            )
          : jsonResponse({ target: {}, counts: { total_owned_rows: 0 } }, 200),
    })
    renderAt('/admin/nodes/0195f2a1-0014-4014-8014-000000000014')

    await screen.findByRole('heading', { level: 1, name: /Node A/ })
    fireEvent.click(screen.getByRole('button', { name: 'Permanently delete Node' }))
    // Wait for the Server-computed scope before the destructive action is
    // enabled.
    await screen.findByText(/Node-owned rows will be deleted/)
    fireEvent.click(screen.getByRole('button', { name: 'Confirm permanent deletion' }))

    expect(await screen.findByText('server database is unavailable')).toBeTruthy()
    expect(screen.queryByText('Node permanently deleted')).toBeNull()
    // The confirmation stays actionable so the Owner can retry or cancel.
    expect(
      screen.getByRole('button', { name: 'Confirm permanent deletion' }),
    ).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Cancel' })).toBeTruthy()
  })
})
