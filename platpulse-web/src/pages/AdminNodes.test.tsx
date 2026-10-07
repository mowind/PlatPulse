import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from '@testing-library/react'
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

/** A full 32-byte P2P public key as the Server projects the observed key. */
const P2P_KEY_A = '0x' + 'a1'.repeat(64)
const P2P_KEY_B = '0x' + 'b2'.repeat(64)

/** Automatic Validator identity (issue #218): the correspondence is resolved
 * from this Node's Network and the full P2P public key its Agent reported. It
 * is a chain identity, and the Public projection is a separate decision — a
 * resolved identity is shown in Public only while the Node is Active with an
 * open automatic Link. */
const VALIDATOR_IDENTITY_RESOLVED = {
  nodeId: NODE_A.node_id,
  nodeDisplayName: 'Node A',
  networkKey: 'platon-mainnet',
  lifecycle: 'active',
  state: 'identified',
  reason: null,
  observedValidatorNodeKey: P2P_KEY_A,
  validatorId: 'v-1',
  validatorNodeKey: P2P_KEY_A,
  associationEffective: true,
  evaluatedAt: '2026-08-12T08:00:00Z',
}

const NODE_VALIDATOR_RESOLVED_DETAIL = {
  ...NODE_A_DETAIL,
  validator_identity: VALIDATOR_IDENTITY_RESOLVED,
}

/** A conflict is a named unresolved state carrying the Server's own reason:
 * never an absence, and never a verdict about staking or ownership. */
const VALIDATOR_IDENTITY_MISMATCHED = {
  nodeId: '0195f2a1-0015-4015-8015-000000000015',
  nodeDisplayName: 'Node B (private)',
  networkKey: 'platon-mainnet',
  lifecycle: 'active',
  state: 'network_identity_mismatch',
  reason:
    "The P2P public key this Node's Agent reported was not observed for the Network identity of this Node.",
  observedValidatorNodeKey: P2P_KEY_B,
  validatorId: null,
  validatorNodeKey: null,
  associationEffective: false,
  evaluatedAt: '2026-08-12T08:00:00Z',
}

const NODE_VALIDATOR_MISMATCH_DETAIL = {
  ...NODE_RPC_ERROR_DETAIL,
  validator_identity: VALIDATOR_IDENTITY_MISMATCHED,
}

/** A Node the Server has not evaluated carries no evaluation at all, which is
 * not the assertion that it holds no identity. */
const NODE_VALIDATOR_UNEVALUATED_DETAIL = {
  ...NODE_A_DETAIL,
  validator_identity: null,
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

/** The Node's own Process series panel, named apart from the shared Host one. */
function nodeProcessPanel(): HTMLElement {
  const panel = document.querySelector(
    '[data-slot="metric-history-panel"][data-surface="node-process"]',
  )
  if (!panel)
    throw new Error('the Node Process metric history panel is not rendered')
  return panel as HTMLElement
}

/** The shared Host series panel of the Node page, named apart from the Node's
 * own Process series. */
function nodeHostPanel(): HTMLElement {
  const panel = document.querySelector(
    '[data-slot="metric-history-panel"][data-surface="node-host"]',
  )
  if (!panel)
    throw new Error('the shared Host metric history panel is not rendered')
  return panel as HTMLElement
}

/** The Validator identity panel of the Node page (issue #218), scoped by its
 * own heading so an assertion never reads the neighbouring identity panels. */
function validatorIdentityPanel(): HTMLElement {
  const heading = screen.getByRole('heading', { level: 2, name: 'Validator identity' })
  const panel = heading.closest('[data-slot="card-x"]')
  if (!panel) throw new Error('the Validator identity panel is not rendered')
  return panel as HTMLElement
}

/** The Agent's Host series as the Server answers it: observations of the machine
 * the Agent watches, stored once for every Node it serves. */
function hostHistoryFixture(overrides: Record<string, unknown> = {}) {
  const HOUR_MS = 60 * 60 * 1000
  const instant = (offsetMs: number) =>
    new Date(Date.now() + offsetMs).toISOString().replace(/\.\d{3}Z$/, 'Z')
  return {
    scopeKind: 'host',
    scopeKey: NODE_A.agent_id,
    nodeId: NODE_A.node_id,
    metric: 'cpu_percent',
    dimension: '',
    from: instant(-24 * HOUR_MS),
    to: instant(0),
    requestedFrom: instant(-24 * HOUR_MS),
    availability: null,
    rawRetentionDays: 1,
    grain: 'raw',
    aggregateSupported: true,
    historyHorizonDays: 30,
    windowSeconds: 86400,
    truncated: false,
    continuation: null,
    segments: [
      {
        from: instant(-24 * HOUR_MS),
        to: instant(0),
        grain: 'raw',
        source: 'raw',
        pointCount: 2,
        truncated: false,
      },
    ],
    series: {
      observed: true,
      firstObservedAt: instant(-2 * HOUR_MS),
      lastObservedAt: instant(-1 * HOUR_MS),
      lastReceivedAt: instant(-1 * HOUR_MS + 1000),
      observationCount: 2,
      replayedCount: 0,
      correctedCount: 0,
      sampledCount: 2,
      coverageSeconds: 60,
      windowSeconds: 86400,
      latestDelaySeconds: 1,
      latestClockSuspect: false,
    },
    items: [
      {
        observedAt: instant(-2 * HOUR_MS),
        receivedAt: instant(-2 * HOUR_MS + 1000),
        value: 11,
        grain: 'raw',
        source: 'raw',
        minValue: 11,
        maxValue: 11,
        sampleCount: 1,
        lastObservedAt: instant(-2 * HOUR_MS),
        delaySeconds: 1,
        clockSuspect: false,
      },
      {
        observedAt: instant(-1 * HOUR_MS),
        receivedAt: instant(-1 * HOUR_MS + 1000),
        value: 2.5,
        grain: 'raw',
        source: 'raw',
        minValue: 2.5,
        maxValue: 2.5,
        sampleCount: 1,
        lastObservedAt: instant(-1 * HOUR_MS),
        delaySeconds: 1,
        clockSuspect: false,
      },
    ],
    gaps: [],
    ...overrides,
  }
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
    aggregateSupported: true,
    historyHorizonDays: 30,
    windowSeconds: 86400,
    truncated: false,
    continuation: null,
    segments: [
      {
        from: canonical(-24 * HOUR),
        to: canonical(0),
        grain: 'raw',
        source: 'raw',
        pointCount: 3,
        truncated: false,
      },
    ],
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
      { observedAt: canonical(-3 * HOUR), receivedAt: canonical(-3 * HOUR + 1000), value: 2.5, grain: 'raw', source: 'raw', minValue: 2.5, maxValue: 2.5, sampleCount: 1, lastObservedAt: canonical(-3 * HOUR), delaySeconds: 1, clockSuspect: false },
      { observedAt: canonical(-2 * HOUR), receivedAt: canonical(-2 * HOUR + 1000), value: 2.5, grain: 'raw', source: 'raw', minValue: 2.5, maxValue: 2.5, sampleCount: 1, lastObservedAt: canonical(-2 * HOUR), delaySeconds: 1, clockSuspect: false },
      { observedAt: canonical(-1 * HOUR), receivedAt: canonical(-1 * HOUR + 120_000), value: 2.5, grain: 'raw', source: 'raw', minValue: 2.5, maxValue: 2.5, sampleCount: 1, lastObservedAt: canonical(-1 * HOUR), delaySeconds: 120, clockSuspect: true, clockNote: 'the observation is stamped 6s after the Server received it: the Agent clock is ahead' },
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
      segments: [],
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

    // The answer says which tier answered what, and how far back evidence can
    // exist at all: the horizon outlives the raw window.
    expect(screen.getByText('Tiers in this answer')).toBeTruthy()
    expect(screen.getByText(/stored samples · 3 points/)).toBeTruthy()
    expect(screen.getByText(/Investigation horizon/)).toBeTruthy()
    expect(screen.getByText(/30 days · older stretches are answered by buckets/)).toBeTruthy()
    // A raw-only answer explains no tiers and offers no older page.
    expect(document.querySelector('[data-slot="metric-history-tiers"]')).toBeNull()
    expect(document.querySelector('[data-slot="metric-history-bucket"]')).toBeNull()
    expect(screen.queryByRole('button', { name: 'Load older points' })).toBeNull()

    // Another series this Node never reported is named as absent: no chart,
    // no zero line.
    fireEvent.change(
      within(nodeProcessPanel()).getByLabelText('Metric series'),
      {
        target: { value: 'data_directory_percent' },
      },
    )
    expect(
      await screen.findByText(/never reported Data directory/),
    ).toBeTruthy()
    expect(
      document.querySelector('[data-slot="metric-history-chart"]'),
    ).toBeNull()
    expect(
      document.querySelector('[data-slot="metric-history-gap-band"]'),
    ).toBeNull()
    expect(
      document.querySelector('[data-slot="metric-history-segments"]'),
    ).toBeNull()
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
  it('reports a range the raw window no longer holds, and pages an older answer without faking samples', async () => {
    // The coordinate is named once and handed to both sides: the mocked Server
    // answers with this instant and the assertion reads it back. Recomputing it
    // from the wall clock at assertion time would compare two reads of
    // `Date.now()` taken either side of a second boundary.
    const oldestAnswered = canonical(-90 * MINUTE)
    let call = 0
    const olderCalls: string[] = []
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/nodes/0195f2a1-0014-4014-8014-000000000014': () =>
        jsonResponse(NODE_A_DETAIL, 200),
      '/api/admin/v1/nodes/0195f2a1-0014-4014-8014-000000000014/metric-history*': (request) => {
        if (request.url.includes('before=')) {
          olderCalls.push(request.url)
          return jsonResponse(
            metricHistoryFixture({
              to: oldestAnswered,
              truncated: false,
              continuation: null,
            }),
            200,
          )
        }
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
        return jsonResponse(
          metricHistoryFixture({
            truncated: true,
            windowSeconds: 3600,
            continuation: oldestAnswered,
          }),
          200,
        )
      },
    })
    renderAt('/admin/nodes/0195f2a1-0014-4014-8014-000000000014')

    await screen.findByRole('heading', { level: 1, name: /Node A/ })
    expect(
      await screen.findByText(/entirely older than the investigation horizon/),
    ).toBeTruthy()
    expect(screen.getByText(/no stored sample falls inside this window/)).toBeTruthy()
    expect(document.querySelector('[data-slot="metric-history-chart"]')).toBeNull()
    // The ledger outlives the released samples: the state is still reported.
    expect(screen.getByText('1 stored observation(s) since the first one')).toBeTruthy()

    fireEvent.click(
      within(nodeProcessPanel()).getByRole('button', { name: '1 hour' }),
    )
    expect(
      await screen.findByText(/more samples than one answer carries/),
    ).toBeTruthy()
    expect(call).toBeGreaterThan(1)

    // The Server names the coordinate to continue from, and the panel asks for
    // exactly that one: a full stretch is never pretended to fit one answer.
    fireEvent.click(await screen.findByRole('button', { name: 'Load older points' }))
    await waitFor(() => {
      expect(olderCalls.length).toBeGreaterThan(0)
    })
    expect(decodeURIComponent(olderCalls[0])).toContain('before=' + oldestAnswered)
    expect(await screen.findByText(/An older page/)).toBeTruthy()
    expect(screen.getByText(/No point is skipped between two pages/)).toBeTruthy()
    // Returning to the newest points drops the cursor rather than keeping it.
    fireEvent.click(screen.getByRole('button', { name: 'Return to the newest points' }))
    expect(await screen.findByRole('button', { name: 'Load older points' })).toBeTruthy()
  })

  it('pages twice into an older answer while the Server reports it truncated, and returns to the newest points', async () => {
    // The Server names each answer's own oldest instant as the exclusive
    // coordinate the next older page continues from.
    const newestOldest = canonical(-30 * MINUTE)
    const secondPageOldest = canonical(-120 * MINUTE)
    const thirdPageOldest = canonical(-210 * MINUTE)
    const requested: (string | null)[] = []
    const olderRequests = () => requested.filter((value) => value !== null)
    // The chart's axis labels carry percent text of their own, so which page is
    // on screen is read from the newest-observations table alone.
    const sampleText = () =>
      document.querySelector('[data-slot="metric-history-samples"]')?.textContent ?? ''
    const observedLabel = (instant: string) => instant.slice(0, 19).replace('T', ' ')
    const page = (
      to: string,
      continuation: string | null,
      truncated: boolean,
      observedAt: string,
      value: number,
    ) =>
      metricHistoryFixture({
        to,
        truncated,
        continuation,
        items: [
          {
            observedAt,
            receivedAt: observedAt,
            value,
            grain: 'raw',
            source: 'raw',
            minValue: value,
            maxValue: value,
            sampleCount: 1,
            lastObservedAt: observedAt,
            delaySeconds: 1,
            clockSuspect: false,
          },
        ],
      })
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/nodes/0195f2a1-0014-4014-8014-000000000014': () =>
        jsonResponse(NODE_A_DETAIL, 200),
      '/api/admin/v1/nodes/0195f2a1-0014-4014-8014-000000000014/metric-history*': (request) => {
        const before = new URL(request.url).searchParams.get('before')
        requested.push(before)
        if (before === newestOldest) {
          return jsonResponse(page(newestOldest, secondPageOldest, true, secondPageOldest, 33.4), 200)
        }
        if (before === secondPageOldest) {
          return jsonResponse(page(secondPageOldest, null, false, thirdPageOldest, 66.4), 200)
        }
        if (before !== null) return jsonResponse({ error: { code: 'not_found' } }, 404)
        return jsonResponse(page(canonical(0), newestOldest, true, newestOldest, 2.5), 200)
      },
    })
    renderAt('/admin/nodes/0195f2a1-0014-4014-8014-000000000014')

    // The newest window answers for itself alone: it is truncated, so it carries
    // the way into the older pages beside that notice.
    expect(await screen.findByText(/more samples than one answer carries/)).toBeTruthy()
    await waitFor(() => {
      expect(sampleText()).toContain('2.5%')
    })
    expect(sampleText()).toContain(observedLabel(newestOldest))
    expect(requested[0]).toBeNull()

    // First page older: the coordinate is the oldest instant of the newest
    // answer, and the way further back stays available.
    fireEvent.click(screen.getByRole('button', { name: 'Load older points' }))
    await waitFor(() => {
      expect(olderRequests()).toHaveLength(1)
    })
    expect(olderRequests()[0]).toBe(newestOldest)
    await waitFor(() => {
      expect(sampleText()).toContain('33%')
    })
    expect(sampleText()).toContain(observedLabel(secondPageOldest))
    expect(sampleText()).not.toContain('2.5%')
    expect(screen.getByText(/No point is skipped between two pages/)).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Return to the newest points' })).toBeTruthy()

    // Second page older: the cursor is that answer's own oldest instant, so the
    // panel walks further back instead of resetting to the newest window.
    fireEvent.click(screen.getByRole('button', { name: 'Load older points' }))
    await waitFor(() => {
      expect(olderRequests()).toHaveLength(2)
    })
    expect(olderRequests()[1]).toBe(secondPageOldest)
    await waitFor(() => {
      expect(sampleText()).toContain('66%')
    })
    expect(sampleText()).toContain(observedLabel(thirdPageOldest))
    expect(sampleText()).not.toContain('33%')
    // This answer holds everything left, so there is no further page to ask for
    // while the newest window is still one click away.
    expect(screen.queryByRole('button', { name: 'Load older points' })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Return to the newest points' }))
    await waitFor(() => {
      expect(sampleText()).toContain('2.5%')
    })
    expect(sampleText()).toContain(observedLabel(newestOldest))
    expect(sampleText()).not.toContain('66%')
    expect(screen.getByRole('button', { name: 'Load older points' })).toBeTruthy()
    // Returning drops the cursor; it never asks the Server for another page.
    expect(olderRequests()).toHaveLength(2)
  })

  it('names the tier behind each stretch and never calls a bucket a sample', async () => {
    const bucketStart = new Date(Math.floor((Date.now() - 29 * 24 * HOUR) / MINUTE) * MINUTE)
      .toISOString()
      .replace(/\.\d{3}Z$/, 'Z')
    const bucket = {
      observedAt: bucketStart,
      receivedAt: canonical(-29 * 24 * HOUR + 5000),
      value: 42.5,
      grain: '1m',
      source: 'aggregate',
      minValue: 7,
      maxValue: 88.5,
      sampleCount: 6,
      lastObservedAt: new Date(Date.parse(bucketStart) + 4 * MINUTE)
        .toISOString()
        .replace(/\.\d{3}Z$/, 'Z'),
      delaySeconds: 5,
      clockSuspect: false,
    }
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/nodes/0195f2a1-0014-4014-8014-000000000014': () =>
        jsonResponse(NODE_A_DETAIL, 200),
      '/api/admin/v1/nodes/0195f2a1-0014-4014-8014-000000000014/metric-history*': () =>
        jsonResponse(
          metricHistoryFixture({
            from: canonical(-30 * 24 * HOUR),
            requestedFrom: canonical(-30 * 24 * HOUR),
            grain: '1m',
            windowSeconds: 30 * 86400,
            items: [metricHistoryFixture().items[0], bucket],
            segments: [
              {
                from: canonical(-30 * 24 * HOUR),
                to: canonical(-24 * HOUR),
                grain: '1m',
                source: 'aggregate',
                pointCount: 1440,
                truncated: false,
              },
              {
                from: canonical(-24 * HOUR),
                to: canonical(0),
                grain: 'raw',
                source: 'raw',
                pointCount: 1,
                truncated: false,
              },
            ],
          }),
          200,
        ),
    })
    renderAt('/admin/nodes/0195f2a1-0014-4014-8014-000000000014')

    // The answer, not the static panel heading: wait for evidence the tiers
    // were read. Which tier answered which stretch, and what a bucket is worth,
    // so a bucket never reads as one stored sample.
    expect(await screen.findByText(/1 minute buckets · 1440 points/)).toBeTruthy()
    expect(screen.getByText(/answered by 1 minute buckets/)).toBeTruthy()
    expect(screen.getByText('Tiers in this answer')).toBeTruthy()
    expect(screen.getByText(/stored samples · 1 point/)).toBeTruthy()
    expect(screen.getByText(/30 days · older stretches are answered by buckets/)).toBeTruthy()

    // The newest-points table names the grain, the counted evidence, and the
    // range the bucket kept.
    expect(screen.getByText('1 minute bucket')).toBeTruthy()
    expect(screen.getByText(/6 observations over 4 minutes/)).toBeTruthy()
    // The counted maximum of the bucket is on screen: the spike the Server kept
    // is visible even though the bucket is drawn as one marker (the percent
    // formatter drops the fraction above ten, so 88.5 reads as 89).
    expect(screen.getByText(/7\.0% to 89%/)).toBeTruthy()
    const chart = document.querySelector('[data-slot="metric-history-chart"]')
    expect(chart?.getAttribute('aria-label')).toContain('at 1 minute grain')

    // A bucket is drawn as a square marker, and the plot says what it stands for.
    expect(
      document.querySelectorAll('[data-slot="metric-history-bucket"]').length,
    ).toBeGreaterThan(0)
    // The legend explains the square markers (the chart description says the
    // same thing, so the legend is named rather than searched for by text).
    expect(
      document.querySelector('[data-slot="metric-history-buckets"]')?.textContent,
    ).toContain('are aggregate buckets')
  })

  it('breaks the drawn line at a bucket whose own evidence proved a hole, and says so', async () => {
    // Four 1 minute buckets one minute apart, the third of which measured a
    // five minute interval between two of its own observations. The Server kept
    // the evidence that the stretch it stands for was not continuous, so the
    // plot must not join it to either neighbour and the table must not call it
    // a closed stretch.
    const bucket = (offsetMs: number, value: number, gapSeconds: number) => ({
      observedAt: canonical(offsetMs),
      receivedAt: canonical(offsetMs + 5000),
      value,
      grain: '1m',
      source: 'aggregate',
      minValue: value - 1,
      maxValue: value + 1,
      sampleCount: 60,
      firstObservedAt: canonical(offsetMs),
      lastObservedAt: canonical(offsetMs + 59_000),
      maxGapSeconds: gapSeconds,
      delaySeconds: 5,
      clockSuspect: false,
    })
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/nodes/0195f2a1-0014-4014-8014-000000000014': () =>
        jsonResponse(NODE_A_DETAIL, 200),
      '/api/admin/v1/nodes/0195f2a1-0014-4014-8014-000000000014/metric-history*': () =>
        jsonResponse(
          metricHistoryFixture({
            from: canonical(-1 * HOUR),
            to: canonical(0),
            requestedFrom: canonical(-1 * HOUR),
            grain: '1m',
            windowSeconds: 3600,
            gaps: [],
            items: [
              bucket(-4 * MINUTE, 10, 1),
              bucket(-3 * MINUTE, 20, 1),
              bucket(-2 * MINUTE, 30, 300),
              bucket(-1 * MINUTE, 40, 1),
            ],
          }),
          200,
        ),
    })
    renderAt('/admin/nodes/0195f2a1-0014-4014-8014-000000000014')

    // The per-point copy names what the bucket measured and refuses to call the
    // stretch closed.
    const hole = await screen.findByText(/the widest measured interval inside it is 5 minutes/)
    expect(hole.getAttribute('data-slot')).toBe('metric-history-hole')
    expect(hole.textContent).toContain('so no line is drawn across it')
    // Every bucket is drawn, and only the one that proved a hole carries the
    // mark that says so.
    expect(document.querySelectorAll('[data-slot="metric-history-bucket"]').length).toBe(4)
    expect(
      document.querySelectorAll('[data-slot="metric-history-bucket"][data-hole="proved"]').length,
    ).toBe(1)

    // The columns on either side of the holed bucket are not joined across it:
    // one stretch of the two columns before it, and the column after it stands
    // alone (three joined points would be one line of four columns).
    const lines = Array.from(document.querySelectorAll('[data-slot="metric-history-line"]'))
    expect(lines.length).toBe(1)
    expect(lines[0].getAttribute('d')?.match(/L/g)?.length).toBe(1)
    expect(
      document.querySelector('[data-slot="metric-history-buckets"]')?.textContent,
    ).toContain('1 of them measured a hole inside their own stretch')
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

  it("reads the Agent's shared Host series on a Node page and names the Agent it belongs to", async () => {
    const hostCalls: string[] = []
    const processCalls: string[] = []
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/nodes/0195f2a1-0014-4014-8014-000000000014/host-metric-history*':
        (request) => {
          hostCalls.push(request.url)
          return jsonResponse(hostHistoryFixture(), 200)
        },
      '/api/admin/v1/nodes/0195f2a1-0014-4014-8014-000000000014/metric-history*':
        (request) => {
          processCalls.push(request.url)
          return jsonResponse(metricHistoryFixture(), 200)
        },
      '/api/admin/v1/nodes/0195f2a1-0014-4014-8014-000000000014': () =>
        jsonResponse(NODE_A_DETAIL, 200),
    })
    renderAt('/admin/nodes/0195f2a1-0014-4014-8014-000000000014')

    await screen.findByRole('heading', { level: 1, name: /Node A/ })
    // Two panels with two subjects: this Node's own Process series and the
    // machine the Agent observes once for every Node of it.
    expect(
      within(nodeProcessPanel()).getByRole('heading', {
        level: 2,
        name: 'Metric history',
      }),
    ).toBeTruthy()
    expect(
      within(nodeHostPanel()).getByRole('heading', {
        level: 2,
        name: 'Host metric history',
      }),
    ).toBeTruthy()
    expect(
      within(nodeHostPanel()).getByText(/never reported|not one Node process/),
    ).toBeTruthy()

    // The Host panel asks the shared Agent series on its own route, never the
    // Node's own metric, and it says which Agent the series belongs to.
    await waitFor(() => {
      expect(hostCalls.length).toBeGreaterThan(0)
    })
    const asked = decodeURIComponent(hostCalls[0])
    expect(asked).toContain(`/nodes/${NODE_A.node_id}/host-metric-history`)
    expect(asked).toContain('metric=cpu_percent')
    expect(asked).toContain('dimension=')
    expect(
      processCalls.every((url) => url.includes('metric=process_cpu_percent')),
    ).toBe(true)
    const owner = nodeHostPanel().querySelector(
      '[data-slot="host-metric-history-owner"]',
    )
    expect(owner?.textContent).toContain(NODE_A.agent_id)
    expect(owner?.textContent).toContain('does not remove it')

    // The machine's own reading is charted and listed in its own unit.
    const hostPanel = nodeHostPanel()
    expect(
      hostPanel.querySelector('[data-slot="metric-history-chart"]'),
    ).not.toBeNull()
    const samples =
      hostPanel.querySelector('[data-slot="metric-history-samples"]')
        ?.textContent ?? ''
    expect(samples).toContain('11%')
    expect(samples).toContain('2.5%')
    expect(
      within(hostPanel).getByRole('button', { name: '1 hour' }),
    ).toBeTruthy()
  })

  it('asks for the mount path before reading a Host storage series, and reads the series each path names', async () => {
    const hostCalls: string[] = []
    const instant = (offsetMs: number) =>
      new Date(Date.now() + offsetMs).toISOString().replace(/\.\d{3}Z$/, 'Z')
    const storageItem = (value: number) => ({
      observedAt: instant(-60 * 60 * 1000),
      receivedAt: instant(-60 * 60 * 1000 + 1000),
      value,
      grain: 'raw',
      source: 'raw',
      minValue: value,
      maxValue: value,
      sampleCount: 1,
      lastObservedAt: instant(-60 * 60 * 1000),
      delaySeconds: 1,
      clockSuspect: false,
    })
    // '/data' and '/data ' are two mounts the Agent reported, so each path reads
    // the value recorded for that path alone.
    const storageValue = (dimension: string) =>
      dimension === '/data' ? 100 : dimension === '/data ' ? 200 : 900
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/nodes/0195f2a1-0014-4014-8014-000000000014/host-metric-history*':
        (request) => {
          hostCalls.push(request.url)
          const dimension =
            new URL(request.url).searchParams.get('dimension') ?? ''
          return jsonResponse(
            hostHistoryFixture({
              metric: 'disk_used_bytes',
              dimension,
              items: dimension === '' ? [] : [storageItem(storageValue(dimension))],
            }),
            200,
          )
        },
      '/api/admin/v1/nodes/0195f2a1-0014-4014-8014-000000000014/metric-history*':
        () => jsonResponse(metricHistoryFixture(), 200),
      '/api/admin/v1/nodes/0195f2a1-0014-4014-8014-000000000014': () =>
        jsonResponse(NODE_A_DETAIL, 200),
    })
    renderAt('/admin/nodes/0195f2a1-0014-4014-8014-000000000014')

    await screen.findByRole('heading', { level: 1, name: /Node A/ })
    // A storage series is identified by the mount path the Agent reported, so
    // the panel names no series until the Operator gives the path.
    const beforeMetric = hostCalls.length
    fireEvent.change(within(nodeHostPanel()).getByLabelText('Metric series'), {
      target: { value: 'disk_used_bytes' },
    })
    expect(
      await within(nodeHostPanel()).findByText(
        /Enter the mount path the Agent reported/,
      ),
    ).toBeTruthy()
    expect(
      nodeHostPanel().querySelector('[data-slot="metric-history-chart"]'),
    ).toBeNull()
    // An empty path is not a mount, so no query is issued for it: the panel does
    // not ask the Server for the series without an identity and then mask it.
    expect(hostCalls.length).toBe(beforeMetric)

    const mountInput = within(nodeHostPanel()).getByLabelText('Mount path')
    const beforeMount = hostCalls.length
    fireEvent.change(mountInput, { target: { value: '/data' } })
    await waitFor(() => {
      expect(hostCalls.length).toBeGreaterThan(beforeMount)
    })
    const dataCall = decodeURIComponent(hostCalls[hostCalls.length - 1])
    expect(dataCall).toContain('metric=disk_used_bytes')
    expect(dataCall).toContain('dimension=/data')
    const usedBytes = () =>
      nodeHostPanel().querySelector('[data-slot="metric-history-samples"]')
        ?.textContent ?? ''
    await waitFor(() => {
      expect(usedBytes()).toContain('100 B')
    })

    // Another path is another series, not the same one continued.
    fireEvent.change(mountInput, { target: { value: '/mnt/data' } })
    await waitFor(() => {
      expect(decodeURIComponent(hostCalls[hostCalls.length - 1])).toContain(
        'dimension=/mnt/data',
      )
    })
    await waitFor(() => {
      expect(usedBytes()).toContain('900 B')
    })
    expect(usedBytes()).not.toContain('100 B')

    // A mount identity is literal, so a path that differs only in surrounding
    // whitespace is a different mount: '/data ' is read as the series the Agent
    // reported for it, never trimmed into '/data' and read as that one instead.
    fireEvent.change(mountInput, { target: { value: '/data ' } })
    await waitFor(() => {
      expect(
        new URL(hostCalls[hostCalls.length - 1]).searchParams.get('dimension'),
      ).toBe('/data ')
    })
    await waitFor(() => {
      expect(usedBytes()).toContain('200 B')
    })
    expect(usedBytes()).not.toContain('100 B')
  })
  it('shows the automatic Validator identity the Server resolved, with its Public projection', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/nodes/0195f2a1-0014-4014-8014-000000000014': () =>
        jsonResponse(NODE_VALIDATOR_RESOLVED_DETAIL, 200),
    })
    renderAt('/admin/nodes/0195f2a1-0014-4014-8014-000000000014')

    await screen.findByRole('heading', { level: 1, name: /Node A/ })
    const panel = validatorIdentityPanel()
    // The resolved state and the Public projection are named separately: a
    // resolved identity still needs an Active Node and an open Link to appear
    // on Home.
    expect(within(panel).getAllByText('Identified').length).toBeGreaterThan(0)
    expect(
      within(panel).getByText(
        'An automatic Link is open and the Public projection shows this association.',
      ),
    ).toBeTruthy()
    expect(within(panel).getByText('State')).toBeTruthy()
    expect(within(panel).getByTitle(P2P_KEY_A)).toBeTruthy()
    expect(panel.querySelector('a[href="/admin/validators/v-1"]')).not.toBeNull()
    expect(
      within(panel).getByText('Open, and shown in the Public projection'),
    ).toBeTruthy()
    expect(within(panel).getByText('2026-08-12 08:00:00 UTC')).toBeTruthy()
    expect(within(panel).queryByText('Never evaluated')).toBeNull()
    expect(within(panel).getAllByText('Active').length).toBeGreaterThan(0)
  })

  it('keeps an unresolved Validator identity Unknown with the Server reason, never as absence', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/nodes/0195f2a1-0015-4015-8015-000000000015': () =>
        jsonResponse(NODE_VALIDATOR_MISMATCH_DETAIL, 200),
    })
    renderAt('/admin/nodes/0195f2a1-0015-4015-8015-000000000015')

    await screen.findByRole('heading', { level: 1, name: /Node B/ })
    const panel = validatorIdentityPanel()
    expect(within(panel).getAllByText('Network identity mismatch').length).toBeGreaterThan(0)
    expect(
      within(panel).getByText(
        "The P2P public key this Node's Agent reported was not observed for the Network identity of this Node.",
      ),
    ).toBeTruthy()
    // The observed key stays inspectable, and no chain Validator is claimed.
    expect(within(panel).getByTitle(P2P_KEY_B)).toBeTruthy()
    expect(within(panel).getByText('Not established')).toBeTruthy()
    expect(panel.querySelector('a[href^="/admin/validators/"]')).toBeNull()
    expect(within(panel).getByText('No automatic Link to project')).toBeTruthy()
    expect(within(panel).queryByText('Not a Validator')).toBeNull()
  })

  it('shows a Node the Server has not evaluated as Not evaluated, not as absence', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/nodes/0195f2a1-0014-4014-8014-000000000014': () =>
        jsonResponse(NODE_VALIDATOR_UNEVALUATED_DETAIL, 200),
    })
    renderAt('/admin/nodes/0195f2a1-0014-4014-8014-000000000014')

    await screen.findByRole('heading', { level: 1, name: /Node A/ })
    const panel = validatorIdentityPanel()
    expect(within(panel).getAllByText('Not evaluated').length).toBeGreaterThan(0)
    expect(
      within(panel).getByText(
        'No automatic Validator identity evaluation is recorded for this Node yet.',
      ),
    ).toBeTruthy()
    expect(within(panel).getByText('Never evaluated')).toBeTruthy()
    // The observed key and the lifecycle are Unknown, not empty and not a
    // definite lifecycle.
    expect(within(panel).getAllByText('Unknown').length).toBe(2)
    expect(panel.querySelector('a[href^="/admin/validators/"]')).toBeNull()
    expect(within(panel).getByText('No automatic Link to project')).toBeTruthy()
  })
})
