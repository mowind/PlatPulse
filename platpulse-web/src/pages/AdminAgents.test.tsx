import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
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
    {
      credential_id: '0195f2a1-0021-4021-8021-000000000022',
      created_at: '2026-08-10T07:00:00Z',
      revoked_at: null,
      revoke_after: '2026-08-11T07:00:00Z',
      active: false,
    },
  ],
  host: {
    components: [
      {
        component: 'memory',
        state: 'ok',
        error_code: null,
        error_message: null,
        attempted_at: '2026-08-12T07:59:58Z',
        observed_at: '2026-08-12T07:59:59Z',
        received_at: '2026-08-12T08:00:00Z',
        state_revision: 7,
        value_revision: 11,
      },
    ],
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
    {
      node_id: 'node-2',
      network_key: 'platon-e2e',
      display_name: 'Retired Node',
      lifecycle: 'retired',
      visibility: 'private',
      health: 'unknown',
      health_reason: 'Retired Nodes are not evaluated for live health',
      freshness: 'unknown',
      current_head: null,
      historical_high_watermark: 12841000,
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
  it('shows Starting while the Server summary is loading', async () => {
    let resolveAgents!: (response: Response) => void
    const pendingAgents = new Promise<Response>((resolve) => {
      resolveAgents = resolve
    })
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/agents': () => pendingAgents,
    })
    renderAt('/admin/agents')

    await waitFor(() => expect(screen.getByText(/Loading Agent inventory/)).toBeTruthy())
    resolveAgents(jsonResponse([AGENT_DIAGNOSTIC], 200))
    await screen.findByRole('row', { name: /0195f2a1/ })
  })

  it('shows an Error state when the Server summary fails initially', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/agents': () => errorBody('diagnostics_unavailable', 503),
    })
    renderAt('/admin/agents')

    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toContain('diagnostics_unavailable')
    expect(alert.textContent).toContain('Try again')
  })

  it('shows the six-column priority summary with independent evidence', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/agents': () => jsonResponse([AGENT_DIAGNOSTIC], 200),
    })
    renderAt('/admin/agents')

    await screen.findByRole('heading', { level: 1, name: 'Agents' })
    const row = await screen.findByRole('row', { name: /0195f2a1/ })
    const headers = screen.getAllByRole('columnheader').map((header) => header.textContent)
    expect(headers).toEqual([
      'Agent',
      'Reporting status',
      'Last received',
      'Node Inventory',
      'Credentials',
      'Diagnostics',
    ])
    expect(row.textContent).toContain('Server liveness')
    expect(row.textContent).toContain('Current')
    expect(row.textContent).toContain('Server receipt time')
    expect(row.textContent).toContain('2026-08-12 08:00:00 UTC')
    expect(row.textContent).toContain('2 retained Nodes')
    expect(row.textContent).toContain('Active + Retired')
    expect(row.textContent).toContain('Server validity')
    expect(row.textContent).toContain('1 active · 0 revoked · 1 inactive (not revoked) · 2 total')
    const gapEvidence = within(row).getByText('Recorded gap intervals', { exact: true }).parentElement
    const securityEvidence = within(row).getByText('Accumulated recorded security events', { exact: true }).parentElement
    const queuedEvidence = within(row).getByText('Queued reports', { exact: true }).parentElement
    const deliveryEvidence = within(row).getByText('Delivery state', { exact: true }).parentElement
    expect(gapEvidence?.textContent).toContain('0')
    expect(securityEvidence?.textContent).toContain('0')
    expect(queuedEvidence?.textContent).toContain('3')
    expect(deliveryEvidence?.textContent).toContain('In flight')
    expect(row.textContent).toContain('Dropped sequence range')
    expect(row.textContent).toContain('#7–#9 recorded')
    expect(row.textContent).toContain('Delivery error')
    expect(row.textContent).not.toContain('server unavailable')
    expect(row.textContent).not.toContain('#42')
    expect(screen.queryByRole('link', { name: 'Enroll a new Agent' })).toBeNull()
  })

  it('keeps last-good Agent values visible after a failed refresh', async () => {
    let failed = false
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/agents': () =>
        failed ? errorBody('diagnostics_unavailable', 503) : jsonResponse([AGENT_DIAGNOSTIC], 200),
    })
    renderAt('/admin/agents')

    const row = await screen.findByRole('row', { name: /0195f2a1/ })
    failed = true
    await act(async () => {
      await adminQueryClient.refetchQueries({ queryKey: ['admin', 'diagnostics'] })
    })

    await waitFor(() => expect(screen.getByRole('alert').textContent).toContain('last successful'))
    expect(row.textContent).toContain('Current')
    expect(row.textContent).toContain('2 retained Nodes')
  })

  it('distinguishes an omitted Server receipt time as Unknown', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/agents': () =>
        jsonResponse([{ ...AGENT_DIAGNOSTIC, last_received_at: undefined }], 200),
    })
    renderAt('/admin/agents')

    const row = await screen.findByRole('row', { name: /0195f2a1/ })
    const receipt = within(row).getByText('Server receipt time', { exact: true }).parentElement
    expect(receipt?.textContent).toContain('Unknown')
  })

  it('distinguishes an explicit null Server receipt time as Never received', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/agents': () =>
        jsonResponse([{ ...AGENT_DIAGNOSTIC, liveness: 'unknown', last_received_at: null }], 200),
    })
    renderAt('/admin/agents')

    const row = await screen.findByRole('row', { name: /0195f2a1/ })
    const liveness = within(row).getByText('Server liveness', { exact: true }).parentElement
    expect(liveness?.textContent).toContain('Unknown')
    const receipt = within(row).getByText('Server receipt time', { exact: true }).parentElement
    expect(receipt?.textContent).toContain('Never received')
  })

  it('reveals the complete Agent ID and reports clipboard failure without claiming success', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/agents': () => jsonResponse([AGENT_DIAGNOSTIC], 200),
    })
    renderAt('/admin/agents')

    const row = await screen.findByRole('row', { name: /0195f2a1/ })
    expect(within(row).getByRole('link', { name: '0195f2a1…0011' })).toBeTruthy()
    fireEvent.click(within(row).getByRole('button', { name: 'Show full Agent ID' }))
    expect(within(row).getByText(AGENT_ID, { exact: true })).toBeTruthy()

    const previousClipboard = Object.getOwnPropertyDescriptor(navigator, 'clipboard')
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText: vi.fn().mockRejectedValue(new Error('clipboard denied')) },
    })
    try {
      fireEvent.click(within(row).getByRole('button', { name: 'Copy Agent ID' }))
      expect(await within(row).findByText('Copy unavailable; select the full Agent ID above.')).toBeTruthy()
      expect(within(row).queryByText('Copied to clipboard.')).toBeNull()
    } finally {
      if (previousClipboard) Object.defineProperty(navigator, 'clipboard', previousClipboard)
      else delete (navigator as unknown as { clipboard?: unknown }).clipboard
    }
  })

  it('shows the Empty state without an unavailable enrollment action', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/agents': () => jsonResponse([], 200),
    })
    renderAt('/admin/agents')

    await screen.findByRole('heading', { level: 1, name: 'Agents' })
    expect(await screen.findByText(/No Agents enrolled yet\./)).toBeTruthy()
    expect(screen.queryByRole('link', { name: 'Enroll the first Agent' })).toBeNull()
  })


  it('keeps Current reporting separate from multiple recorded diagnostics', async () => {
    const longDeliveryError =
      'delivery attempt retained after a bounded retry exhausted the configured server timeout'
    const diagnostic = {
      ...AGENT_DIAGNOSTIC,
      sequence_gap_count: 2,
      security_event_count: 3,
      host: {
        ...AGENT_DIAGNOSTIC.host,
        spool_queued_reports: 0,
        spool_in_flight: false,
        spool_store_fatal: true,
        spool_report_too_large: true,
        spool_pending_history_gaps: 4,
        spool_last_delivery_error: longDeliveryError,
        spool_store_error: 'store error retained for detail',
      },
    }
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/agents': () => jsonResponse([diagnostic], 200),
    })
    renderAt('/admin/agents')

    const row = await screen.findByRole('row', { name: /0195f2a1/ })
    expect(row.textContent).toContain('Current')
    expect(within(row).getByText('Recorded gap intervals', { exact: true }).parentElement?.textContent).toContain('2')
    expect(within(row).getByText('Accumulated recorded security events', { exact: true }).parentElement?.textContent).toContain('3')
    expect(within(row).getByText('Queued reports', { exact: true }).parentElement?.textContent).toContain('0')
    expect(within(row).getByText('Delivery state', { exact: true }).parentElement?.textContent).toContain('Idle')
    expect(within(row).getByText('Store fatal', { exact: true }).parentElement?.textContent).toContain('Yes')
    expect(within(row).getByText('Delivery error', { exact: true }).parentElement?.textContent).toContain('Recorded')
    expect(within(row).getByText('Report size', { exact: true }).parentElement?.textContent).toContain('Too large')
    expect(within(row).getByText('Pending history gaps', { exact: true }).parentElement?.textContent).toContain('4')
    expect(row.textContent).toContain('Host snapshot')
    expect(row.textContent).not.toContain(longDeliveryError)
    expect(row.textContent).not.toContain('store error retained for detail')
  })

  it('distinguishes no Host observation from an unobserved spool', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/agents': () => jsonResponse([{ ...AGENT_DIAGNOSTIC, host: null }], 200),
    })
    renderAt('/admin/agents')

    const row = await screen.findByRole('row', { name: /0195f2a1/ })
    expect(within(row).getByText('Host observation', { exact: true }).parentElement?.textContent).toContain('Not observed yet')
    expect(within(row).queryByText('Spool observation', { exact: true })).toBeNull()
  })

  it('distinguishes an unobserved spool from an authoritative zero queue', async () => {
    const host = { components: [], updated_at: '2026-08-12T08:00:00Z' }
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/agents': () => jsonResponse([{ ...AGENT_DIAGNOSTIC, host }], 200),
    })
    renderAt('/admin/agents')

    let row = await screen.findByRole('row', { name: /0195f2a1/ })
    expect(within(row).getByText('Spool observation', { exact: true }).parentElement?.textContent).toContain('Not observed yet')
    expect(within(row).queryByText('Queued reports', { exact: true })).toBeNull()

    cleanup()
    adminQueryClient.clear()
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/agents': () =>
        jsonResponse(
          [
            {
              ...AGENT_DIAGNOSTIC,
              host: { ...host, spool_queued_reports: 0, spool_in_flight: false },
            },
          ],
          200,
        ),
    })
    await renderAt('/admin/agents')

    row = await screen.findByRole('row', { name: /0195f2a1/ })
    expect(within(row).getByText('Queued reports', { exact: true }).parentElement?.textContent).toContain('0')
    expect(within(row).getByText('Delivery state', { exact: true }).parentElement?.textContent).toContain('Idle')
    expect(within(row).getByText('Spool observation', { exact: true }).parentElement?.textContent).toContain('Observed')
  })

  it('qualifies retained delivery timestamps and metadata-only spool evidence', async () => {
    const host = {
      components: [],
      updated_at: '2026-08-12T08:00:00Z',
      spool_capacity_bytes: 1024,
      spool_last_delivery_at: '2026-08-12T07:00:00Z',
      spool_oldest_queued_age_ms: 120000,
    }
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/agents': () => jsonResponse([{ ...AGENT_DIAGNOSTIC, host }], 200),
    })
    renderAt('/admin/agents')

    const row = await screen.findByRole('row', { name: /0195f2a1/ })
    expect(row.textContent).toContain('Current')
    expect(within(row).getByText('Spool observation', { exact: true }).parentElement?.textContent).toContain('Observed')
    expect(within(row).getByText('Last delivery', { exact: true }).parentElement?.textContent).toContain('2026-08-12 07:00:00 UTC')
    expect(row.textContent).not.toContain('Spool not observed yet')
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
    for (const heading of ['Overview', 'Runtime and reporting', 'Credentials', 'Diagnostics', 'Audit']) {
      expect(screen.getByRole('heading', { level: 2, name: heading })).toBeTruthy()
    }
    // Independent dimensions.
    expect(screen.getByText('Identity')).toBeTruthy()
    expect(screen.getByText('Liveness')).toBeTruthy()
    expect(screen.getByText('Boot and report state')).toBeTruthy()
    expect(screen.getByText('Inventory')).toBeTruthy()
    expect(screen.getByText('Credentials')).toBeTruthy()
    expect(screen.getByText('Diagnostics')).toBeTruthy()
    expect(screen.getByText('Host CPU')).toBeTruthy()
    expect(screen.getByText('Host memory used / total')).toBeTruthy()
    expect(screen.getByText('memory', { exact: true })).toBeTruthy()
    expect(screen.getByText(/state revision 7 · value revision 11/)).toBeTruthy()
    expect(screen.getByText('Audit trail')).toBeTruthy()
    expect(screen.getAllByText(AGENT_ID, { exact: true }).length).toBeGreaterThan(0)
    expect(screen.getAllByRole('button', { name: 'Copy Agent ID' }).length).toBeGreaterThan(0)
    expect(screen.getByText('sequence #42')).toBeTruthy()
    expect(screen.getByText('Server receipt time')).toBeTruthy()
    expect(screen.getByText('Full active boot ID')).toBeTruthy()
    expect(screen.getByText('boot-1', { exact: true })).toBeTruthy()
    expect(screen.getByText('Recorded gap intervals')).toBeTruthy()
    const droppedSequence = screen.getByText('Dropped sequence range', { exact: true }).parentElement
    expect(droppedSequence?.textContent).toContain('7–9')
    const deliveryError = screen.getByText('Last delivery error', { exact: true }).parentElement
    expect(deliveryError?.textContent).toContain('server unavailable')
    expect(screen.getByText(/Recorded evidence from the latest Host observation/)).toBeTruthy()
    const storeError = screen.getByText('Spool store error', { exact: true }).parentElement
    expect(storeError?.textContent).toContain('Unknown')
    const reportSize = screen.getByText('Report size state', { exact: true }).parentElement
    expect(reportSize?.textContent).toContain('Unknown')
    // Credential state is Server-owned; the revoke action is explicit.
    expect(screen.getByText(CREDENTIAL_ID)).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Revoke' })).toBeTruthy()
    // The redacted audit trail renders details without secrets.
    expect(screen.getByText('agent_credential_rotated')).toBeTruthy()
    expect(screen.getByText(/overlap_hours: 24/)).toBeTruthy()
    // Actions are offered as links, not executed remotely.
    expect(screen.queryByRole('link', { name: 'Rotate credential' })).toBeNull()
    expect(screen.queryByRole('link', { name: 'Recover agent' })).toBeNull()
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

  it('preserves Cancel and blocks duplicate revoke requests while busy', async () => {
    let resolveRevoke!: (response: Response) => void
    let revokeCalls = 0
    const pendingRevoke = new Promise<Response>((resolve) => {
      resolveRevoke = resolve
    })
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      [`/api/admin/v1/agents/${AGENT_ID}`]: () => jsonResponse(AGENT_DIAGNOSTIC, 200),
      [`/api/admin/v1/agents/${AGENT_ID}/audit`]: () => jsonResponse({ agent_id: AGENT_ID, items: [] }, 200),
      [`/api/admin/v1/agents/${AGENT_ID}/credentials/${CREDENTIAL_ID}/revoke`]: () => {
        revokeCalls += 1
        return pendingRevoke
      },
    })
    renderAt(`/admin/agents/${AGENT_ID}`)

    await screen.findByRole('heading', { level: 1, name: /Agent 0195f2a1/ })
    fireEvent.click(await screen.findByRole('button', { name: 'Revoke' }))
    expect(screen.getByRole('button', { name: 'Confirm revoke' })).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Cancel' })).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    expect(screen.queryByRole('button', { name: 'Confirm revoke' })).toBeNull()

    fireEvent.click(screen.getByRole('button', { name: 'Revoke' }))
    const confirm = screen.getByRole('button', { name: 'Confirm revoke' })
    fireEvent.click(confirm)
    await waitFor(() => expect((screen.getByRole('button', { name: 'Revoking…' }) as HTMLButtonElement).disabled).toBe(true))
    expect((screen.getByRole('button', { name: 'Cancel' }) as HTMLButtonElement).disabled).toBe(true)
    fireEvent.click(confirm)
    expect(revokeCalls).toBe(1)

    resolveRevoke(jsonResponse({ agent_id: AGENT_ID, credential_id: CREDENTIAL_ID, revoked_at: '2026-08-12T09:00:00Z', request_id: 'req-revoke' }, 200))
    await screen.findByText(/Credential revoked at/)
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
