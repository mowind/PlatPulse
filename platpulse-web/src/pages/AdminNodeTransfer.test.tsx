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

const SOURCE_AGENT = '0195f2a1-0011-4011-8011-000000000011'
const TARGET_AGENT = '0195f2a1-0019-4019-8019-000000000019'
const NODE_ID = '0195f2a1-0014-4014-8014-000000000014'

const NODE_DETAIL = {
  node_id: NODE_ID,
  agent_id: SOURCE_AGENT,
  display_name: 'Node A',
  network_key: 'platon-e2e',
  network_display_name: 'PlatON E2E Network',
  lifecycle: 'active',
  lifecycle_guidance: 'Active',
  visibility: 'private',
  inventory_revision: 1,
  first_seen_at: '2026-08-12T08:00:00Z',
  updated_at: '2026-08-12T08:00:00Z',
  rpc_endpoint: 'ws://127.0.0.1:****',
  health: 'healthy',
  health_reason: 'RPC, sync, and consensus are current',
  freshness: 'current',
  identity: { state: 'matched', observed: null, mismatched_fields: [] },
  transfer: null,
}

const AGENTS = [
  {
    agent_id: SOURCE_AGENT,
    agent_epoch: 1,
    boot_status: 'active',
    liveness: 'online',
    capabilities: [],
    credentials: [],
    nodes: [],
    clock_status: 'unknown',
    security_event_count: 0,
    sequence_gap_count: 0,
    shutdown_state: 'running',
    shutdown_forced: false,
    active_boot_id: null,
    last_received_at: '2026-08-12T08:00:00Z',
    last_report_sequence: 42,
  },
  {
    agent_id: TARGET_AGENT,
    agent_epoch: 1,
    boot_status: 'active',
    liveness: 'online',
    capabilities: [],
    credentials: [],
    nodes: [],
    clock_status: 'unknown',
    security_event_count: 0,
    sequence_gap_count: 0,
    shutdown_state: 'running',
    shutdown_forced: false,
    active_boot_id: null,
    last_received_at: '2026-08-12T08:00:00Z',
    last_report_sequence: 1,
  },
]

const PENDING_TRANSFER = {
  transfer_id: '0195f2a1-0041-4041-8041-000000000041',
  node_id: NODE_ID,
  source_agent_id: SOURCE_AGENT,
  target_agent_id: TARGET_AGENT,
  status: 'pending',
  operator_reason: 'move the validator host',
  created_at: '2026-08-12T08:00:00Z',
  expires_at: '2026-08-15T08:00:00Z',
  cancelled_at: null,
  completed_at: null,
  rejection_code: null,
  rejection_reason: null,
  mismatched_fields: [],
  updated_at: '2026-08-12T08:00:00Z',
}

function jsonResponse(body: unknown, status: number): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

const TEST_ORIGIN = 'http://platpulse.test'

function mockFetch(routes: Record<string, (init?: RequestInit) => Response | Promise<Response>>) {
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(String(input), init)
    const url = request.url.replace(TEST_ORIGIN, '')
    const method = request.method
    for (const [pattern, handler] of Object.entries(routes)) {
      if (pattern.endsWith('*')) {
        if (url.startsWith(pattern.slice(0, -1))) return handler({ method })
      } else if (url === pattern) {
        return handler({ method })
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

function transferRoutes(options: {
  transfers?: unknown[]
  pending?: unknown
  conflict?: boolean
} = {}) {
  const { transfers = [], pending = null, conflict = false } = options
  const createResponse = conflict
    ? () =>
        jsonResponse(
          {
            error: {
              code: 'transfer_conflict',
              message: 'a transfer for this Node is already pending',
              requestId: 'req-conflict',
              fields: ['audit_event_id:55'],
            },
          },
          409,
        )
    : () =>
        jsonResponse(
          {
            transfer: { ...PENDING_TRANSFER, status: 'pending' },
            request_id: 'req-1',
            audit_event_id: 101,
          },
          200,
        )
  return {
    '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
    '/api/admin/v1/agents': () => jsonResponse(AGENTS, 200),
    ['/api/admin/v1/nodes/' + NODE_ID + '/transfers']: (init?: RequestInit) => {
      const result = init?.method === 'POST' ? createResponse() : jsonResponse(transfers, 200)
      return result
    },
    '/api/admin/v1/nodes*': () =>
      jsonResponse({ ...NODE_DETAIL, transfer: pending ?? null }, 200),
    ['/api/admin/v1/transfers/' + PENDING_TRANSFER.transfer_id + '/cancel']: () =>
      jsonResponse(
        {
          transfer: { ...PENDING_TRANSFER, status: 'cancelled', cancelled_at: '2026-08-12T09:00:00Z' },
          request_id: 'req-2',
          audit_event_id: 102,
        },
        200,
      ),
  }
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

describe('PAGE-ADMIN-NODE-TRANSFER (two-phase workflow)', () => {
  it('shows the create form with registered target Agents only', async () => {
    mockFetch(transferRoutes())
    renderAt(`/admin/nodes/${NODE_ID}/transfer`)

    await screen.findByRole('heading', { level: 1, name: 'Transfer Node ownership' })
    const select = await screen.findByLabelText(/Target Agent/)
    const options = Array.from(select.querySelectorAll('option'))
    // The current owner is excluded; only the registered target is offered
    // (placeholder + one candidate).
    expect(options).toHaveLength(2)
    expect(options[1].getAttribute('value')).toBe(TARGET_AGENT)
    expect(screen.getByText(/This Node has never been transferred/)).toBeTruthy()
  })

  it('creates a pending transfer with confirmation and shows the typed success + Audit reference', async () => {
    const fetchMock = mockFetch(transferRoutes())
    renderAt(`/admin/nodes/${NODE_ID}/transfer`)

    const select = await screen.findByLabelText(/Target Agent/)
    fireEvent.change(select, { target: { value: TARGET_AGENT } })
    fireEvent.change(await screen.findByLabelText(/Expiry/), { target: { value: 48 } })
    fireEvent.change(await screen.findByLabelText(/Operator reason/), {
      target: { value: 'move validator' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Review transfer' }))
    // Explicit high-risk confirmation before the mutation runs.
    await screen.findByText(/Create a pending transfer to 0195f2a1…/)
    fireEvent.click(screen.getByRole('button', { name: 'Confirm transfer' }))

    const success = await screen.findByRole('status')
    expect(success.textContent).toContain('Transfer 0195f2a1… is pending')
    expect(screen.getByText(/Server-authoritative, never auto-extends/)).toBeTruthy()
    expect(screen.getByText('req-1')).toBeTruthy()
    expect(screen.getByText('#101')).toBeTruthy()
    // The mutation went through the CSRF-protected Admin seam.
    const createCall = fetchMock.mock.calls.find(([input]) => {
      const request = input instanceof Request ? input : new Request(String(input))
      return request.method === 'POST'
    })
    expect(createCall).toBeDefined()
  })

  it('surfaces a create conflict with the typed error and the request reference', async () => {
    mockFetch(
      transferRoutes({
        transfers: [],
        pending: null,
        conflict: true,
      }),
    )
    renderAt(`/admin/nodes/${NODE_ID}/transfer`)

    // A concurrent operator (another tab) creates the pending transfer
    // between the form render and the confirm; the failed mutation never
    // renders optimistic pending state and surfaces the typed conflict.
    const select = await screen.findByLabelText(/Target Agent/)
    fireEvent.change(select, { target: { value: TARGET_AGENT } })
    fireEvent.click(screen.getByRole('button', { name: 'Review transfer' }))
    await screen.findByText(/Create a pending transfer to 0195f2a1…/)
    fireEvent.click(screen.getByRole('button', { name: 'Confirm transfer' }))

    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toContain('a transfer for this Node is already pending')
    expect(alert.textContent).toContain('req-conflict')
    expect(alert.textContent).toContain('audit_event_id:55')
  })

  it('shows the pending workflow with expiry and cancels through explicit confirmation', async () => {
    mockFetch(
      transferRoutes({
        transfers: [PENDING_TRANSFER],
        pending: PENDING_TRANSFER,
      }),
    )
    renderAt(`/admin/nodes/${NODE_ID}/transfer`)

    const panel = await screen.findByRole('heading', { level: 2, name: 'Pending transfer' })
    expect(panel).toBeTruthy()
    expect(screen.getByText(/source Agent remains the only authoritative owner/)).toBeTruthy()
    expect(screen.getAllByText(/2026-08-15 08:00:00 UTC/).length).toBeGreaterThan(0)
    // Cancel requires explicit confirmation and never switches ownership.
    fireEvent.click(screen.getByRole('button', { name: 'Cancel transfer' }))
    await screen.findByText(/Cancel this transfer\?/)
    fireEvent.click(screen.getByRole('button', { name: 'Confirm cancel' }))
    const status = await screen.findByText(/cancelled · Audit #102/)
    expect(status).toBeTruthy()
  })

  it('renders identity mismatch as a blocking diagnostic with mismatched fields', async () => {
    const mismatch = {
      ...PENDING_TRANSFER,
      status: 'identity_mismatch',
      mismatched_fields: ['genesis_hash', 'address_hrp'],
      rejection_code: 'identity_mismatch',
      rejection_reason:
        'the target-declared Network identity contradicts the registered Network; ownership stays with the source Agent',
    }
    mockFetch(
      transferRoutes({
        transfers: [mismatch],
        pending: null,
      }),
    )
    renderAt(`/admin/nodes/${NODE_ID}/transfer`)

    await screen.findByRole('heading', { level: 2, name: 'Transfer history' })
    await waitFor(() => {
      expect(screen.getByRole('alert').textContent).toContain('Blocking diagnostic')
    })
    expect(screen.getByText(/genesis_hash, address_hrp/)).toBeTruthy()
    expect(screen.getByText(/no new block history was merged/)).toBeTruthy()
    expect(screen.getAllByText('Identity mismatch').length).toBeGreaterThan(0)
  })

  it('renders completed, cancelled, expired, rejected, and conflict outcomes in the timeline', async () => {
    const base = {
      node_id: NODE_ID,
      source_agent_id: SOURCE_AGENT,
      target_agent_id: TARGET_AGENT,
      operator_reason: null,
      created_at: '2026-08-12T08:00:00Z',
      expires_at: '2026-08-15T08:00:00Z',
      cancelled_at: null,
      completed_at: null,
      rejection_code: null,
      rejection_reason: null,
      mismatched_fields: [],
      updated_at: '2026-08-12T08:00:00Z',
    }
    mockFetch(
      transferRoutes({
        transfers: [
          { ...base, transfer_id: 't-completed', status: 'completed', completed_at: '2026-08-13T08:00:00Z' },
          { ...base, transfer_id: 't-cancelled', status: 'cancelled', cancelled_at: '2026-08-13T09:00:00Z' },
          { ...base, transfer_id: 't-expired', status: 'expired' },
          {
            ...base,
            transfer_id: 't-rejected',
            status: 'rejected',
            rejection_code: 'network_key_mismatch',
            rejection_reason: 'target declared the Node under a different Network key',
          },
          { ...base, transfer_id: 't-conflict', status: 'conflict' },
        ],
      }),
    )
    renderAt(`/admin/nodes/${NODE_ID}/transfer`)

    await screen.findByRole('heading', { level: 2, name: 'Transfer history' })
    expect(screen.getAllByText('Completed', { exact: true }).length).toBeGreaterThan(0)
    expect(screen.getAllByText('Cancelled', { exact: true }).length).toBeGreaterThan(0)
    expect(screen.getAllByText('Expired', { exact: true }).length).toBeGreaterThan(0)
    expect(screen.getAllByText('Rejected', { exact: true }).length).toBeGreaterThan(0)
    expect(screen.getAllByText('Conflict', { exact: true }).length).toBeGreaterThan(0)
    expect(screen.getByText(/network_key_mismatch/)).toBeTruthy()
    expect(screen.getByText(/Ownership switched atomically/)).toBeTruthy()
    expect(screen.getAllByText(/Source ownership is unchanged|ownership is unchanged/).length).toBeGreaterThan(0)
  })

  it('back navigation returns to the Node detail route', async () => {
    mockFetch(transferRoutes())
    renderAt(`/admin/nodes/${NODE_ID}/transfer`)

    await screen.findByRole('heading', { level: 1, name: 'Transfer Node ownership' })
    const back = screen.getByRole('link', { name: 'Back to Node detail' })
    expect(back.getAttribute('href')).toBe(`/admin/nodes/${NODE_ID}`)
  })
})
