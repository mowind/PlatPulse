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

const NETWORK = {
  network_key: 'platon-e2e',
  display_name: 'PlatON E2E Network',
  genesis_hash: '0x1111111111111111111111111111111111111111111111111111111111111111',
  chain_id: 210425,
  p2p_network_id: 1,
  address_hrp: 'lat',
  created_at: '2026-08-12T08:00:00Z',
  updated_at: '2026-08-12T08:00:00Z',
  active_node_count: 1,
  retired_node_count: 0,
  mismatched_node_count: 1,
}

const NETWORK_DETAIL = {
  ...NETWORK,
  nodes: [
    {
      node_id: '0195f2a1-0014-4014-8014-000000000014',
      agent_id: '0195f2a1-0011-4011-8011-000000000011',
      display_name: 'Node A',
      lifecycle: 'active',
      visibility: 'public',
      health: 'healthy',
      health_reason: 'RPC, sync, and consensus are current',
      freshness: 'current',
      current_head: 12842019,
      resync_state: 'normal',
      identity: {
        state: 'matched',
        observed: {
          genesis_hash: NETWORK.genesis_hash,
          chain_id: 210425,
          p2p_network_id: 1,
          address_hrp: 'lat',
        },
        mismatched_fields: [],
      },
    },
    {
      node_id: '0195f2a1-0015-4015-8015-000000000015',
      agent_id: '0195f2a1-0011-4011-8011-000000000011',
      display_name: 'Node B (private)',
      lifecycle: 'active',
      visibility: 'private',
      health: 'unhealthy',
      health_reason: 'RPC collection failed',
      freshness: 'stale',
      current_head: 12842018,
      resync_state: 'normal',
      identity: {
        state: 'mismatched',
        observed: {
          genesis_hash: NETWORK.genesis_hash,
          chain_id: 999999,
          p2p_network_id: 1,
          address_hrp: 'lat',
        },
        mismatched_fields: ['chain_id'],
      },
    },
  ],
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

describe('PAGE-ADMIN-NETWORKS (Network Registry)', () => {
  it('lists the complete validated identity tuple with Node counts and typed mismatch outcomes', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/networks': () => jsonResponse([NETWORK], 200),
    })
    renderAt('/admin/networks')

    await screen.findByRole('heading', { level: 1, name: 'Networks' })
    const row = await screen.findByRole('row', { name: /PlatON E2E Network/ })
    expect(row.textContent).toContain('platon-e2e')
    expect(row.textContent).toContain('210425')
    expect(row.textContent).toContain('lat')
    expect(row.textContent).toContain('1 active · 0 retired')
    expect(row.textContent).toContain('Mismatched')
  })

  it('registers a Network only through the explicit Owner workflow with the full tuple', async () => {
    let created = false
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(String(input), init)
      const url = request.url.replace(TEST_ORIGIN, '')
      if (url === '/api/public/v1/session') return jsonResponse(OWNER_SESSION, 200)
      if (url === '/api/admin/v1/networks' && request.method === 'GET') {
        return jsonResponse([], 200)
      }
      if (url === '/api/admin/v1/networks' && request.method === 'POST') {
        created = true
        return jsonResponse({ networkKey: 'platon-testnet', displayName: 'PlatON Testnet' }, 200)
      }
      return jsonResponse({ error: { code: 'not_found' } }, 404)
    })
    vi.stubGlobal('fetch', fetchMock)
    renderAt('/admin/networks')

    await screen.findByRole('heading', { level: 1, name: 'Networks' })
    fireEvent.click(screen.getByRole('button', { name: 'Register a Network' }))
    fireEvent.change(screen.getByLabelText('Network key'), {
      target: { value: 'platon-testnet' },
    })
    fireEvent.change(screen.getByLabelText('Display name'), {
      target: { value: 'PlatON Testnet' },
    })
    fireEvent.change(screen.getByLabelText('Genesis hash'), {
      target: { value: NETWORK.genesis_hash },
    })
    fireEvent.change(screen.getByLabelText('Chain ID'), {
      target: { value: '210426' },
    })
    fireEvent.change(screen.getByLabelText('P2P network ID'), {
      target: { value: '2' },
    })
    fireEvent.change(screen.getByLabelText('Address HRP'), {
      target: { value: 'lat' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Register Network' }))
    expect(await screen.findByText('Registered PlatON Testnet.')).toBeTruthy()
    expect(created).toBe(true)
  })

  it('surfaces a rejected tuple as a field-level error and preserves the draft', async () => {
    vi.unstubAllGlobals()
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(String(input), init)
      const url = request.url.replace(TEST_ORIGIN, '')
      if (url === '/api/public/v1/session') return jsonResponse(OWNER_SESSION, 200)
      if (url === '/api/admin/v1/networks' && request.method === 'GET') {
        return jsonResponse([], 200)
      }
      if (url === '/api/admin/v1/networks' && request.method === 'POST') {
        return jsonResponse(
          { error: { code: 'invalid_network_tuple', message: 'the Network identity tuple is invalid', requestId: 'r1', fields: [] } },
          400,
        )
      }
      return jsonResponse({ error: { code: 'not_found' } }, 404)
    })
    vi.stubGlobal('fetch', fetchMock)
    renderAt('/admin/networks')

    await screen.findByRole('heading', { level: 1, name: 'Networks' })
    fireEvent.click(screen.getByRole('button', { name: 'Register a Network' }))
    fireEvent.change(screen.getByLabelText('Network key'), {
      target: { value: 'bad key!' },
    })
    fireEvent.change(screen.getByLabelText('Display name'), {
      target: { value: 'Draft Network' },
    })
    fireEvent.change(screen.getByLabelText('Genesis hash'), {
      target: { value: 'not-a-hash' },
    })
    fireEvent.change(screen.getByLabelText('Chain ID'), {
      target: { value: '1' },
    })
    fireEvent.change(screen.getByLabelText('P2P network ID'), {
      target: { value: '1' },
    })
    fireEvent.change(screen.getByLabelText('Address HRP'), {
      target: { value: 'lat' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Register Network' }))
    expect(await screen.findByText('the Network identity tuple is invalid')).toBeTruthy()
    // The draft is preserved after the failed mutation.
    expect(screen.getByLabelText('Display name').getAttribute('value')).toBe('Draft Network')
  })

  it('shows the detail with per-Node identity dispositions and the expected tuple', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/networks/platon-e2e': () => jsonResponse(NETWORK_DETAIL, 200),
    })
    renderAt('/admin/networks/platon-e2e')

    await screen.findByRole('heading', { level: 1, name: /PlatON E2E Network/ })
    expect(screen.getByText('Expected identity tuple')).toBeTruthy()
    expect(screen.getByText('210425')).toBeTruthy()
    expect(screen.getAllByText('lat').length).toBeGreaterThan(0)
    // Per-Node typed mismatch outcome with the contradicting field.
    expect(await screen.findByText(/1 Node observes an identity that contradicts/)).toBeTruthy()
    const rowB = screen.getByRole('row', { name: /Node B \(private\)/ })
    expect(rowB.textContent).toContain('Mismatched')
    expect(rowB.textContent).toContain('chain_id')
    expect(rowB.textContent).toContain('Observed: genesis hash')
    expect(rowB.textContent).toContain('chain id 999999')
  })

  it('updates the Registry tuple with an audited confirmation and refetches', async () => {
    let displayName = 'PlatON E2E Network'
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(String(input), init)
      const url = request.url.replace(TEST_ORIGIN, '')
      if (url === '/api/public/v1/session') return jsonResponse(OWNER_SESSION, 200)
      if (url === '/api/admin/v1/networks/platon-e2e' && request.method === 'GET') {
        return jsonResponse({ ...NETWORK_DETAIL, display_name: displayName }, 200)
      }
      if (url === '/api/admin/v1/networks/platon-e2e' && request.method === 'PUT') {
        displayName = 'PlatON E2E Network v2'
        return jsonResponse({ networkKey: 'platon-e2e', displayName }, 200)
      }
      return jsonResponse({ error: { code: 'not_found' } }, 404)
    })
    vi.stubGlobal('fetch', fetchMock)
    renderAt('/admin/networks/platon-e2e')

    await screen.findByRole('heading', { level: 1, name: /PlatON E2E Network/ })
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    const nameInput = screen.getByLabelText('Display name')
    fireEvent.change(nameInput, { target: { value: 'PlatON E2E Network v2' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save tuple' }))
    // The confirmation step is explicit before the identity tuple mutation.
    expect(
      await screen.findByText(/Update the expected identity tuple\?/),
    ).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Confirm tuple update' }))
    expect(await screen.findByText('Updated PlatON E2E Network v2.')).toBeTruthy()
    // The authoritative refetch shows the new Server-owned name.
    await waitFor(() => {
      expect(displayName).toBe('PlatON E2E Network v2')
    })
  })
})
