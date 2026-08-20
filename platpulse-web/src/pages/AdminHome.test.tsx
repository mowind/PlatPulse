import { act, cleanup, render, screen } from '@testing-library/react'
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

function jsonResponse(body: unknown, status: number): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

const TEST_ORIGIN = 'http://platpulse.test'

function mockFetch() {
  const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
    const request = input instanceof Request ? input : new Request(String(input))
    const url = request.url.replace(TEST_ORIGIN, '')
    if (url === '/api/public/v1/session') return jsonResponse(OWNER_SESSION, 200)
    if (url === '/api/admin/v1/overview') {
      return jsonResponse(
        {
          generated_at: '2026-08-12T08:00:00Z',
          summary: {
            agents: { total: 1, online: 1, offline: 0, unknown: 0 },
            nodes: { total: 1, healthy: 1, unhealthy: 0, unknown: 0, retired: 0, published: 1 },
          },
          attention: [],
        },
        200,
      )
    }
    if (url === '/api/admin/v1/nodes') return jsonResponse([NODE], 200)
    if (url === '/api/admin/v1/agents') {
      return jsonResponse({ error: { code: 'unavailable', message: 'Agent diagnostics unavailable' } }, 503)
    }
    if (url === '/api/admin/v1/geo') return jsonResponse({ error: { code: 'unavailable' } }, 503)
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

describe('PAGE-ADMIN-OVERVIEW', () => {
  it('keeps Node health available when Agent diagnostics fail independently', async () => {
    mockFetch()
    await renderAt('/admin')

    await screen.findByRole('heading', { level: 1, name: 'Overview' })
    expect(await screen.findByRole('row', { name: /Node A/ })).toBeTruthy()
    expect(await screen.findByText('Agent diagnostics unavailable')).toBeTruthy()
    expect(screen.queryByText('Unable to load Nodes')).toBeNull()
  })
})
