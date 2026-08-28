import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
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

function mockFetch(routes: Record<string, () => Response | Promise<Response>>) {
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(String(input), init)
    const url = request.url.replace(TEST_ORIGIN, '')
    for (const [pattern, handler] of Object.entries(routes)) {
      if (pattern.endsWith('*')) {
        if (url.startsWith(pattern.slice(0, -1))) return handler()
      } else if (url === pattern) {
        return handler()
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
})
