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

const AGENT_ID = '0195f2a1-0011-4011-8011-000000000011'
const CREDENTIAL_ID = '0195f2a1-0021-4021-8021-000000000021'

const AGENT_DIAGNOSTIC = {
  agent_id: AGENT_ID,
  agent_epoch: 1,
  last_report_sequence: 42,
  active_boot_id: 'boot-1',
  boot_status: 'active',
  previous_boot_id: null,
  close_report_id: null,
  shutdown_state: 'running',
  shutdown_started_at: null,
  shutdown_deadline_at: null,
  shutdown_finished_at: null,
  shutdown_unresolved_range: null,
  shutdown_last_error: null,
  shutdown_forced: false,
  shutdown_report_id: null,
  shutdown_report_sequence: null,
  shutdown_updated_at: null,
  sequence_gap_count: 0,
  security_event_count: 0,
  clock_status: 'ok',
  clock_skew_ms: 12,
  liveness: 'online',
  last_received_at: '2026-08-12T08:00:00Z',
  capabilities: ['host', 'node_chain'],
  credentials: [
    {
      credential_id: CREDENTIAL_ID,
      created_at: '2026-08-12T07:00:00Z',
      revoked_at: null,
      revoke_after: null,
      active: true,
    },
  ],
  host: {
    components: [],
    updated_at: '2026-08-12T08:00:00Z',
    spool_queued_reports: 3,
    spool_in_flight: true,
    spool_dropped_sequence_from: 7,
    spool_dropped_sequence_to: 9,
    spool_last_delivery_error: 'server unavailable',
    spool_store_fatal: false,
  },
  nodes: [
    {
      node_id: 'node-1',
      network_key: 'platon-e2e',
      display_name: 'Node A',
      lifecycle: 'active',
      visibility: 'public',
      health: 'healthy',
      health_reason: 'RPC, sync, and consensus are current',
      freshness: 'current',
      current_head: 12842019,
      historical_high_watermark: 12842019,
      resync_state: 'idle',
      resync_progress: null,
      network_reference_head: null,
      network_reference_confidence: 'unknown',
      rpc: null,
      sync: null,
      consensus: null,
      process: null,
    },
  ],
}

function jsonResponse(body: unknown, status: number): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

function errorBody(code: string, status = 401): Response {
  return jsonResponse({ error: { code, message: code, requestId: 'r1', fields: [] } }, status)
}

const TEST_ORIGIN = 'http://platpulse.test'

type RouteContext = { init?: RequestInit; body: string | null; request: Request }

function mockFetch(routes: Record<string, (ctx: RouteContext) => Response | Promise<Response>>) {
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(String(input), init)
    let body: string | null = null
    try {
      body = await request.clone().text()
    } catch {
      // Non-readable bodies leave `body` as null.
    }
    const url = request.url.replace(TEST_ORIGIN, '')
    for (const [pattern, handler] of Object.entries(routes)) {
      if (pattern.endsWith('*')) {
        if (url.startsWith(pattern.slice(0, -1))) return handler({ init, body, request })
      } else if (url === pattern) {
        return handler({ init, body, request })
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

describe('PAGE-ADMIN-AGENTS (Agent lifecycle)', () => {
  it('lists identity, liveness, boot/report, inventory, credentials, and diagnostics as separate dimensions', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/agents': () => jsonResponse([AGENT_DIAGNOSTIC], 200),
    })
    renderAt('/admin/agents')

    await screen.findByRole('heading', { level: 1, name: 'Agents' })
    const row = await screen.findByRole('row', { name: /0195f2a1/ })
    expect(row.textContent).toContain('Current') // liveness dimension
    expect(row.textContent).toContain('1') // epoch
    expect(row.textContent).toContain('#42') // last report
    expect(row.textContent).toContain('1 Node') // inventory
    expect(row.textContent).toContain('1 active · 0 revoked · 1 total') // credentials
    expect(row.textContent).toContain('0 gaps · 0 security events') // diagnostics
    expect(row.textContent).toContain('Spool: 3 queued · delivery in flight')
    expect(row.textContent).toContain('dropped reports #7–#9')
    expect(row.textContent).toContain('last delivery error: server unavailable')
    expect(screen.getByRole('link', { name: 'Enroll a new Agent' })).toBeTruthy()
  })

  it('shows the Empty state with an enroll action when no Agents exist', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/agents': () => jsonResponse([], 200),
    })
    renderAt('/admin/agents')

    await screen.findByRole('heading', { level: 1, name: 'Agents' })
    expect(await screen.findByText('No Agents enrolled yet.')).toBeTruthy()
    expect(screen.getByRole('link', { name: 'Enroll the first Agent' })).toBeTruthy()
  })


})

describe('PAGE-ADMIN-AGENT-DETAIL', () => {
  it('shows identity, credentials with revoke, inventory, diagnostics, and the redacted audit trail', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      [`/api/admin/v1/agents/${AGENT_ID}`]: () => jsonResponse(AGENT_DIAGNOSTIC, 200),
      [`/api/admin/v1/agents/${AGENT_ID}/audit`]: () =>
        jsonResponse(
          {
            agent_id: AGENT_ID,
            items: [
              {
                audit_event_id: 3,
                event_kind: 'agent_credential_rotated',
                actor_username: 'admin',
                created_at: '2026-08-12T08:00:00Z',
                details: { credential_id: 'x', overlap_hours: 24 },
              },
            ],
          },
          200,
        ),
    })
    renderAt(`/admin/agents/${AGENT_ID}`)

    await screen.findByRole('heading', { level: 1, name: /Agent 0195f2a1/ })
    // The panels arrive with the authoritative REST data.
    await screen.findByText('Identity')
    // Independent dimensions.
    expect(screen.getByText('Identity')).toBeTruthy()
    expect(screen.getByText('Liveness')).toBeTruthy()
    expect(screen.getByText('Boot and report state')).toBeTruthy()
    expect(screen.getByText('Inventory')).toBeTruthy()
    expect(screen.getByText('Credentials')).toBeTruthy()
    expect(screen.getByText('Diagnostics')).toBeTruthy()
    expect(screen.getByText('Audit trail')).toBeTruthy()
    // Credential state is Server-owned; the revoke action is explicit.
    expect(screen.getByText(CREDENTIAL_ID)).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Revoke' })).toBeTruthy()
    // The redacted audit trail renders details without secrets.
    expect(screen.getByText('agent_credential_rotated')).toBeTruthy()
    expect(screen.getByText(/overlap_hours: 24/)).toBeTruthy()
    // Actions are offered as links, not executed remotely.
    expect(screen.getByRole('link', { name: 'Rotate credential' })).toBeTruthy()
    expect(screen.getByRole('link', { name: 'Recover agent' })).toBeTruthy()
  })

  it('revokes a credential through explicit confirmation and refetches state', async () => {
    let revoked = false
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      [`/api/admin/v1/agents/${AGENT_ID}`]: () =>
        jsonResponse(
          {
            ...AGENT_DIAGNOSTIC,
            credentials: revoked
              ? [{ ...AGENT_DIAGNOSTIC.credentials[0], revoked_at: '2026-08-12T09:00:00Z', active: false }]
              : AGENT_DIAGNOSTIC.credentials,
          },
          200,
        ),
      [`/api/admin/v1/agents/${AGENT_ID}/audit`]: () => jsonResponse({ agent_id: AGENT_ID, items: [] }, 200),
      [`/api/admin/v1/agents/${AGENT_ID}/credentials/${CREDENTIAL_ID}/revoke`]: () => {
        revoked = true
        return jsonResponse(
          {
            agent_id: AGENT_ID,
            credential_id: CREDENTIAL_ID,
            revoked_at: '2026-08-12T09:00:00Z',
            request_id: 'req-revoke',
          },
          200,
        )
      },
    })
    renderAt(`/admin/agents/${AGENT_ID}`)

    await screen.findByRole('heading', { level: 1, name: /Agent 0195f2a1/ })
    fireEvent.click(await screen.findByRole('button', { name: 'Revoke' }))
    expect(screen.getByText(/Revoke now\?/)).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Confirm revoke' }))

    expect(await screen.findByText(/Credential revoked at/)).toBeTruthy()
    // The authoritative refetch shows the revoked state; no optimistic flip.
    await waitFor(() => expect(screen.getByText('Revoked')).toBeTruthy())
    expect(screen.queryByRole('button', { name: 'Revoke' })).toBeNull()
  })

  it('shows a typed conflict and reloads the authoritative state', async () => {
    let detailCalls = 0
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      [`/api/admin/v1/agents/${AGENT_ID}`]: () => {
        detailCalls += 1
        // A concurrent operator already revoked the credential: the reload
        // must show the Server's current dimension, not the stale draft.
        return jsonResponse(
          detailCalls === 1
            ? AGENT_DIAGNOSTIC
            : {
                ...AGENT_DIAGNOSTIC,
                credentials: [
                  {
                    ...AGENT_DIAGNOSTIC.credentials[0],
                    revoked_at: '2026-08-12T09:00:00Z',
                    active: false,
                  },
                ],
              },
          200,
        )
      },
      [`/api/admin/v1/agents/${AGENT_ID}/audit`]: () => jsonResponse({ agent_id: AGENT_ID, items: [] }, 200),
      [`/api/admin/v1/agents/${AGENT_ID}/credentials/${CREDENTIAL_ID}/revoke`]: () =>
        errorBody('credential_already_revoked', 409),
    })
    renderAt(`/admin/agents/${AGENT_ID}`)

    await screen.findByRole('heading', { level: 1, name: /Agent 0195f2a1/ })
    fireEvent.click(await screen.findByRole('button', { name: 'Revoke' }))
    fireEvent.click(screen.getByRole('button', { name: 'Confirm revoke' }))

    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toContain('credential_already_revoked')
    // PATTERN-CONFLICT-RELOAD: the authoritative state is refetched and
    // shows the credential as revoked; the typed error remains visible.
    await waitFor(() => expect(detailCalls).toBeGreaterThan(1))
    expect(await screen.findByText('Revoked', { exact: true })).toBeTruthy()
    expect(screen.getByRole('alert').textContent).toContain('credential_already_revoked')
  })

  it('shows the non-leaking unavailable state for an unknown Agent', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/agents/no-such-agent': () => errorBody('agent_not_found', 404),
    })
    renderAt('/admin/agents/no-such-agent')

    await screen.findByRole('heading', { level: 1, name: 'Agent unavailable' })
    expect(screen.getByText('This Agent is no longer available.')).toBeTruthy()
  })
})
