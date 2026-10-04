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
  inventory: {
    accepted_revision: 4,
    accepted_sha256: '0x1111111111111111111111111111111111111111111111111111111111111111',
    accepted_declaration: 'agent_declared',
    last_rejection: null,
  },
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
  attention: [],
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

/** The Diagnostics section, kept apart from the Host history panel that reads the same Host. */
function diagnosticsSection(): HTMLElement {
  const section = document.querySelector(
    'section[aria-labelledby="agent-diagnostics-heading"]',
  )
  if (!section) throw new Error('the Diagnostics section is not rendered')
  return section as HTMLElement
}

/** The Credentials section, kept apart from the Host history panel that also reports errors. */
function credentialsSection(): HTMLElement {
  const section = document.querySelector(
    'section[aria-labelledby="agent-credentials-heading"]',
  )
  if (!section) throw new Error('the Credentials section is not rendered')
  return section as HTMLElement
}

/** The Agent's own section for the Host it observes once, kept apart from the
 * Diagnostics section that reports the same Host's latest observation. */
function agentHostSection(): HTMLElement {
  const section = document.querySelector(
    'section[aria-label="Host resource history"]',
  )
  if (!section)
    throw new Error('the Host resource history section is not rendered')
  return section as HTMLElement
}

async function renderAt(path: string) {
  render(<App />)
  await act(async () => {
    window.history.pushState({}, '', path)
    window.dispatchEvent(new PopStateEvent('popstate'))
    await Promise.resolve()
  })
}

/** Expand one row's Diagnostics disclosure and return the cross-column detail
 * row that must follow it. The item row keeps its six cells. */
function expandDiagnostics(row: HTMLElement): HTMLElement {
  const toggle = within(row).getByRole('button', { name: 'Show diagnostics' })
  fireEvent.click(toggle)
  expect(toggle.getAttribute('aria-expanded')).toBe('true')
  const detail = row.nextElementSibling as HTMLElement | null
  expect(detail?.getAttribute('data-slot')).toBe('detail-row')
  expect(toggle.getAttribute('aria-controls')).toBe(detail?.querySelector('td')?.id)
  expect(row.children).toHaveLength(6)
  return detail as HTMLElement
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
    // One cell per dimension, in header order: no summary field is merged
    // into a neighbouring cell and no placeholder cell is added.
    const cells = Array.from(row.children)
    expect(cells.map((cell) => cell.getAttribute('data-label'))).toEqual([
      'Agent',
      'Reporting status',
      'Last received',
      'Node Inventory',
      'Credentials',
      'Diagnostics',
    ])
    expect(cells[1].textContent).toContain('Server liveness')
    expect(cells[1].textContent).toContain('Current')
    expect(cells[2].textContent).toContain('Server receipt time')
    expect(cells[2].textContent).toContain('2026-08-12 08:00:00 UTC')
    expect(cells[3].textContent).toContain('2 retained Nodes')
    expect(cells[3].textContent).toContain('Active + Retired')
    expect(cells[4].textContent).toContain('Server validity')
    expect(cells[4].textContent).toContain('1 active · 0 revoked · 1 inactive (not revoked) · 2 total')
    expect(cells[5].textContent).toContain('Recorded gap intervals')
    expect(cells[5].textContent).toContain('Accumulated recorded security events')
    expect(cells[5].textContent).toContain('3 queued')
    expect(cells[5].textContent).toContain('delivery in flight')
    expect(cells[5].textContent).toContain('store not fatal')
    expect(cells[5].textContent).toContain('delivery error recorded')
    expect(cells[5].textContent).toContain('dropped sequence range recorded')
    // The full findings stay behind the disclosure, not in the summary cell.
    expect(row.textContent).not.toContain('Dropped sequence range')
    expect(row.textContent).not.toContain('Host snapshot')
    const detail = expandDiagnostics(row as HTMLElement)
    expect(detail.querySelector('td')?.getAttribute('colspan')).toBe('6')
    expect(detail.textContent).toContain('Dropped sequence range')
    expect(detail.textContent).toContain('#7–#9 recorded')
    expect(detail.textContent).toContain('Delivery error')
    expect(detail.textContent).toContain('Host snapshot')
    expect(detail.textContent).not.toContain('server unavailable')
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

  it('offers the Add Agent enrollment entry from the empty state', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/agents': () => jsonResponse([], 200),
    })
    renderAt('/admin/agents')

    await screen.findByRole('heading', { level: 1, name: 'Agents' })
    expect(await screen.findByText(/No Agents enrolled yet\./)).toBeTruthy()
    // Generating a token does not create a placeholder; the entry is always
    // available and never an unavailable action.
    const entries = await screen.findAllByRole('link', { name: 'Add Agent' })
    expect(entries.length).toBeGreaterThan(0)
    expect(entries[0].getAttribute('href')).toBe('/admin/agents/enroll')
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
    const summary = within(row).getByText('Recorded evidence', { exact: true }).parentElement
    expect(summary?.textContent).toContain('0 queued')
    expect(summary?.textContent).toContain('delivery idle')
    expect(summary?.textContent).toContain('store fatal')
    expect(summary?.textContent).toContain('delivery error recorded')
    expect(summary?.textContent).toContain('report too large')
    expect(summary?.textContent).toContain('4 pending history gaps')
    expect(row.textContent).not.toContain(longDeliveryError)
    const detail = expandDiagnostics(row as HTMLElement)
    expect(within(detail).getByText('Queued reports', { exact: true }).parentElement?.textContent).toContain('0')
    expect(within(detail).getByText('Delivery state', { exact: true }).parentElement?.textContent).toContain('Idle')
    expect(within(detail).getByText('Store fatal', { exact: true }).parentElement?.textContent).toContain('Yes')
    expect(within(detail).getByText('Delivery error', { exact: true }).parentElement?.textContent).toContain('Recorded')
    expect(within(detail).getByText('Report size', { exact: true }).parentElement?.textContent).toContain('Too large')
    expect(within(detail).getByText('Pending history gaps', { exact: true }).parentElement?.textContent).toContain('4')
    expect(detail.textContent).toContain('Host snapshot')
    expect(detail.textContent).not.toContain(longDeliveryError)
    expect(detail.textContent).not.toContain('store error retained for detail')
  })

  it('distinguishes no Host observation from an unobserved spool', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/agents': () => jsonResponse([{ ...AGENT_DIAGNOSTIC, host: null }], 200),
    })
    renderAt('/admin/agents')

    const row = await screen.findByRole('row', { name: /0195f2a1/ })
    expect(within(row).getByText('Recorded evidence', { exact: true }).parentElement?.textContent).toContain('No Host observation yet')
    expect(within(row).queryByText('Spool observation', { exact: true })).toBeNull()
    const detail = expandDiagnostics(row as HTMLElement)
    expect(within(detail).getByText('Host observation', { exact: true }).parentElement?.textContent).toContain('Not observed yet')
    expect(within(detail).queryByText('Spool observation', { exact: true })).toBeNull()
  })

  it('distinguishes an unobserved spool from an authoritative zero queue', async () => {
    const host = { components: [], updated_at: '2026-08-12T08:00:00Z' }
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/agents': () => jsonResponse([{ ...AGENT_DIAGNOSTIC, host }], 200),
    })
    renderAt('/admin/agents')

    let row = await screen.findByRole('row', { name: /0195f2a1/ })
    expect(within(row).getByText('Recorded evidence', { exact: true }).parentElement?.textContent).toContain('Spool not observed yet')
    let detail = expandDiagnostics(row as HTMLElement)
    expect(within(detail).getByText('Spool observation', { exact: true }).parentElement?.textContent).toContain('Not observed yet')
    expect(within(detail).queryByText('Queued reports', { exact: true })).toBeNull()

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
    expect(within(row).getByText('Recorded evidence', { exact: true }).parentElement?.textContent).toContain('0 queued')
    expect(within(row).getByText('Recorded evidence', { exact: true }).parentElement?.textContent).toContain('delivery idle')
    detail = expandDiagnostics(row as HTMLElement)
    expect(within(detail).getByText('Queued reports', { exact: true }).parentElement?.textContent).toContain('0')
    expect(within(detail).getByText('Delivery state', { exact: true }).parentElement?.textContent).toContain('Idle')
    expect(within(detail).getByText('Spool observation', { exact: true }).parentElement?.textContent).toContain('Observed')
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
    expect(row.textContent).not.toContain('Spool not observed yet')
    const detail = expandDiagnostics(row as HTMLElement)
    expect(within(detail).getByText('Spool observation', { exact: true }).parentElement?.textContent).toContain('Observed')
    expect(within(detail).getByText('Last delivery', { exact: true }).parentElement?.textContent).toContain('2026-08-12 07:00:00 UTC')
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
    // Contextual shortcut from an Agent detail to its Incident history
    // (issue #202 Story 2): a bound subject filter, not a generic entry.
    expect(
      screen.getByRole('link', { name: 'Incidents for this Agent' }).getAttribute('href'),
    ).toBe(`/admin/alerts/incidents?subject=agent&subject_key=${AGENT_ID}`)
    for (const heading of ['Overview', 'Runtime and reporting', 'Credentials', 'Diagnostics', 'Audit']) {
      expect(screen.getByRole('heading', { level: 2, name: heading })).toBeTruthy()
    }
    // Independent dimensions.
    expect(screen.getByText('Identity')).toBeTruthy()
    expect(screen.getByText('Liveness')).toBeTruthy()
    expect(screen.getByText('Boot and report state')).toBeTruthy()
    expect(screen.getByText('Inventory')).toBeTruthy()
    // The Inventory dimension shows the declaration the Server accepts
    // (issue #181), not an inferred per-Node counter.
    const acceptedRevision = screen.getByText('Accepted declaration', { exact: true }).parentElement
    expect(acceptedRevision?.textContent).toContain('Accepted revision 4')
    expect(acceptedRevision?.textContent).toContain('0x11111111…1111')
    expect(
      screen.getByText('Inventory rejection evidence', { exact: true }).parentElement?.textContent,
    ).toContain('No Inventory rejection recorded')
    expect(screen.getByText('Credentials')).toBeTruthy()
    expect(screen.getByText('Diagnostics')).toBeTruthy()
    expect(within(diagnosticsSection()).getByText('Host CPU')).toBeTruthy()
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

  it('shows the Server-recorded Inventory rejection next to the accepted declaration', async () => {
    const rejected = {
      ...AGENT_DIAGNOSTIC,
      inventory: {
        accepted_revision: 4,
        accepted_sha256: '0x1111111111111111111111111111111111111111111111111111111111111111',
        accepted_declaration: 'agent_declared',
        last_rejection: {
          code: 'inventory_revision_conflict',
          declaration: 'agent_declared',
          reported_revision: 4,
          reported_sha256: '0x2222222222222222222222222222222222222222222222222222222222222222',
          received_at: '2026-08-12T08:30:00Z',
        },
      },
    }
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      [`/api/admin/v1/agents/${AGENT_ID}`]: () => jsonResponse(rejected, 200),
      [`/api/admin/v1/agents/${AGENT_ID}/audit`]: () => jsonResponse({ agent_id: AGENT_ID, items: [] }, 200),
    })
    renderAt(`/admin/agents/${AGENT_ID}`)

    await screen.findByRole('heading', { level: 1, name: /Agent 0195f2a1/ })
    const evidence = (
      await screen.findByText('Inventory rejection evidence', { exact: true })
    ).parentElement
    // Both sides of the comparison, so the remedy (bump the revision) is
    // readable from the page alone.
    expect(evidence?.textContent).toContain('inventory_revision_conflict')
    expect(evidence?.textContent).toContain('Declared revision 4')
    expect(evidence?.textContent).toContain('0x22222222…2222')
    expect(evidence?.textContent).toContain('Accepted revision 4')
    expect(evidence?.textContent).toContain('0x11111111…1111')
    expect(evidence?.textContent).toContain('Received 2026-08-12 08:30:00 UTC')
  })

  it('explains a Server-managed declaration without inventing an Agent revision', async () => {
    const serverManaged = {
      ...AGENT_DIAGNOSTIC,
      inventory: {
        accepted_revision: 3,
        accepted_sha256: '0x3333333333333333333333333333333333333333333333333333333333333333',
        accepted_declaration: 'server_managed',
        last_rejection: {
          code: 'inventory_revision_conflict',
          declaration: 'server_managed',
          reported_revision: null,
          reported_sha256: '0x4444444444444444444444444444444444444444444444444444444444444444',
          received_at: '2026-08-12T08:30:00Z',
        },
      },
    }
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      [`/api/admin/v1/agents/${AGENT_ID}`]: () => jsonResponse(serverManaged, 200),
      [`/api/admin/v1/agents/${AGENT_ID}/audit`]: () => jsonResponse({ agent_id: AGENT_ID, items: [] }, 200),
    })
    renderAt(`/admin/agents/${AGENT_ID}`)

    await screen.findByRole('heading', { level: 1, name: /Agent 0195f2a1/ })
    const inventory = (await screen.findByText('Accepted declaration', { exact: true })).parentElement
    // The Server owns the revision in v2, so the page names it as assigned.
    expect(inventory?.textContent).toContain('Server-assigned revision 3')
    expect(inventory?.textContent).toContain('0x33333333…3333')

    const evidence = (
      await screen.findByText('Inventory rejection evidence', { exact: true })
    ).parentElement
    expect(evidence?.textContent).toContain('inventory_revision_conflict')
    expect(evidence?.textContent).toContain('server-managed declaration')
    // A v2 refusal reports the refused fingerprint, never a fabricated
    // Agent-assigned revision.
    expect(evidence?.textContent).toContain('Declared fingerprint 0x44444444…4444')
    expect(evidence?.textContent).not.toContain('Declared revision')
    expect(evidence?.textContent).toContain('Server-assigned revision 3')
  })

  it('shows an uninitialized Inventory as Unknown rather than revision 0', async () => {
    const neverAccepted = {
      ...AGENT_DIAGNOSTIC,
      inventory: {
        accepted_revision: null,
        accepted_sha256: null,
        accepted_declaration: 'unknown',
        last_rejection: null,
      },
    }
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      [`/api/admin/v1/agents/${AGENT_ID}`]: () => jsonResponse(neverAccepted, 200),
      [`/api/admin/v1/agents/${AGENT_ID}/audit`]: () => jsonResponse({ agent_id: AGENT_ID, items: [] }, 200),
    })
    renderAt(`/admin/agents/${AGENT_ID}`)

    await screen.findByRole('heading', { level: 1, name: /Agent 0195f2a1/ })
    const inventory = (await screen.findByText('Accepted declaration', { exact: true })).parentElement
    expect(inventory?.textContent).toContain('Unknown')
    expect(inventory?.textContent).not.toContain('revision 0')
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
      [`/api/admin/v1/agents/${AGENT_ID}/audit`]: () =>
        jsonResponse({ agent_id: AGENT_ID, items: [] }, 200),
      [`/api/admin/v1/agents/${AGENT_ID}/credentials/${CREDENTIAL_ID}/revoke`]:
        () => errorBody('credential_already_revoked', 409),
    })
    renderAt(`/admin/agents/${AGENT_ID}`)

    await screen.findByRole('heading', { level: 1, name: /Agent 0195f2a1/ })
    fireEvent.click(await screen.findByRole('button', { name: 'Revoke' }))
    fireEvent.click(screen.getByRole('button', { name: 'Confirm revoke' }))

    const alert = await within(credentialsSection()).findByRole('alert')
    expect(alert.textContent).toContain('credential_already_revoked')
    // PATTERN-CONFLICT-RELOAD: the authoritative state is refetched and
    // shows the credential as revoked; the typed error remains visible.
    await waitFor(() => expect(detailCalls).toBeGreaterThan(1))
    expect(await screen.findByText('Revoked', { exact: true })).toBeTruthy()
    expect(
      within(credentialsSection()).getByRole('alert').textContent,
    ).toContain('credential_already_revoked')
  })

  it('saves and reads back the Server-owned display name and notes', async () => {
    let savedBody: unknown = null
    let detailCalls = 0
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      [`/api/admin/v1/agents/${AGENT_ID}`]: () => {
        detailCalls += 1
        return jsonResponse(
          detailCalls === 1
            ? AGENT_DIAGNOSTIC
            : { ...AGENT_DIAGNOSTIC, display_name: 'Host A Agent', notes: 'Primary Host' },
          200,
        )
      },
      [`/api/admin/v1/agents/${AGENT_ID}/audit`]: () => jsonResponse({ agent_id: AGENT_ID, items: [] }, 200),
      [`/api/admin/v1/agents/${AGENT_ID}/metadata`]: ({ body }) => {
        savedBody = JSON.parse(body ?? '{}')
        return jsonResponse(
          { agent_id: AGENT_ID, display_name: 'Host A Agent', notes: 'Primary Host' },
          200,
        )
      },
    })
    renderAt(`/admin/agents/${AGENT_ID}`)

    await screen.findByRole('heading', { level: 1, name: /Agent 0195f2a1/ })
    // A fresh Agent has no name: the stable ID is the visible identifier.
    expect(screen.queryByText('Host A Agent')).toBeNull()

    const displayInput = await screen.findByLabelText('Display name')
    const notesInput = screen.getByLabelText('Notes')
    fireEvent.change(displayInput, { target: { value: 'Host A Agent' } })
    fireEvent.change(notesInput, { target: { value: 'Primary Host' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save name and notes' }))

    expect(await screen.findByText(/Saved\. The name and notes are Server-owned/)).toBeTruthy()
    expect(savedBody).toEqual({ displayName: 'Host A Agent', notes: 'Primary Host' })
    // The authoritative refetch renders the Server-owned name; the stable
    // Agent ID stays visible and copyable.
    await waitFor(() =>
      expect(screen.getByRole('heading', { level: 1, name: /Host A Agent/ })).toBeTruthy(),
    )
    expect(screen.getAllByText(AGENT_ID, { exact: true }).length).toBeGreaterThan(0)
    expect(screen.getByRole('button', { name: 'Copy Agent ID' })).toBeTruthy()
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

  it('permanently removes the Agent after confirming the exact owned Nodes', async () => {
    let removed = false
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      [`/api/admin/v1/agents/${AGENT_ID}`]: () =>
        removed ? errorBody('agent_not_found', 404) : jsonResponse(AGENT_DIAGNOSTIC, 200),
      [`/api/admin/v1/agents/${AGENT_ID}/audit`]: () =>
        jsonResponse({ agent_id: AGENT_ID, items: [] }, 200),
      [`/api/admin/v1/agents/${AGENT_ID}/removal`]: ({ request }) => {
        if (request.method === 'POST') {
          removed = true
          return jsonResponse(
            {
              agent_id: AGENT_ID,
              deleted_at: '2026-08-12T09:00:00Z',
              revoked_credential_count: 1,
              purged_nodes: [
                { node_id: 'node-1', removed: { total_owned_rows: 9 } },
                { node_id: 'node-2', removed: { total_owned_rows: 3 } },
              ],
              removed: { total_owned_rows: 12 },
            },
            200,
          )
        }
        return jsonResponse(
          {
            target: {
              agent_id: AGENT_ID,
              display_name: null,
              notes: null,
              agent_epoch: 1,
              last_received_at: null,
              created_at: '2026-08-01T00:00:00Z',
              updated_at: '2026-08-12T00:00:00Z',
            },
            owned_nodes: [
              {
                node_id: 'node-1',
                network_key: 'platon-e2e',
                network_display_name: 'PlatON E2E',
                display_name: 'Node A',
                lifecycle: 'active',
                visibility: 'public',
                inventory_revision: 1,
              },
              {
                node_id: 'node-2',
                network_key: 'platon-e2e',
                network_display_name: 'PlatON E2E',
                display_name: 'Retired Node',
                lifecycle: 'retired',
                visibility: 'private',
                inventory_revision: 1,
              },
            ],
            pending_transfers: [],
            counts: { total_owned_rows: 12 },
            credential_count: 2,
            active_credential_count: 1,
            can_remove: true,
          },
          200,
        )
      },
    })
    renderAt(`/admin/agents/${AGENT_ID}`)

    await screen.findByRole('heading', { level: 1, name: /Agent 0195f2a1/ })
    await screen.findByRole('heading', { level: 2, name: 'Danger zone' })
    fireEvent.click(screen.getByRole('button', { name: 'Permanently delete Agent' }))

    const dialog = await screen.findByRole('alertdialog', {
      name: 'Confirm permanent Agent removal',
    })
    await within(dialog).findByText(/This Agent owns 2 Node\(s\)/)
    expect(within(dialog).getByText('Node A')).toBeTruthy()
    expect(within(dialog).getByText('Retired Node')).toBeTruthy()
    expect(within(dialog).getByText(/1 active credential\(s\) of 2 will be revoked/)).toBeTruthy()
    expect(
      within(dialog).getByText(/remote process is not stopped; local configuration is not changed/),
    ).toBeTruthy()

    fireEvent.click(within(dialog).getByRole('button', { name: 'Confirm permanent removal' }))

    await screen.findByRole('heading', { level: 1, name: 'Agent removed' })
    expect(screen.getByText(/1 credential\(s\) were revoked/)).toBeTruthy()
    expect(screen.getByText(/2 Node\(s\) were permanently purged/)).toBeTruthy()
    expect(screen.queryByRole('alertdialog')).toBeNull()
  })

  it('blocks removal while an unhandled Transfer is pending', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      [`/api/admin/v1/agents/${AGENT_ID}`]: () => jsonResponse(AGENT_DIAGNOSTIC, 200),
      [`/api/admin/v1/agents/${AGENT_ID}/audit`]: () =>
        jsonResponse({ agent_id: AGENT_ID, items: [] }, 200),
      [`/api/admin/v1/agents/${AGENT_ID}/removal`]: () =>
        jsonResponse(
          {
            target: {
              agent_id: AGENT_ID,
              display_name: null,
              notes: null,
              agent_epoch: 1,
              last_received_at: null,
              created_at: '2026-08-01T00:00:00Z',
              updated_at: '2026-08-12T00:00:00Z',
            },
            owned_nodes: [
              {
                node_id: 'node-1',
                network_key: 'platon-e2e',
                network_display_name: 'PlatON E2E',
                display_name: 'Node A',
                lifecycle: 'active',
                visibility: 'public',
                inventory_revision: 1,
              },
            ],
            pending_transfers: [
              {
                transfer_id: 'transfer-1',
                node_id: 'node-1',
                source_agent_id: AGENT_ID,
                target_agent_id: 'agent-b',
                direction: 'source',
                expires_at: '2026-08-13T00:00:00Z',
              },
            ],
            counts: { total_owned_rows: 9 },
            credential_count: 1,
            active_credential_count: 1,
            can_remove: false,
          },
          200,
        ),
    })
    renderAt(`/admin/agents/${AGENT_ID}`)

    await screen.findByRole('heading', { level: 1, name: /Agent 0195f2a1/ })
    await screen.findByRole('heading', { level: 2, name: 'Danger zone' })
    fireEvent.click(screen.getByRole('button', { name: 'Permanently delete Agent' }))

    const dialog = await screen.findByRole('alertdialog', {
      name: 'Confirm permanent Agent removal',
    })
    await within(dialog).findByText(/blocked by 1 unhandled Transfer\(s\)/)
    const confirm = within(dialog).getByRole('button', {
      name: 'Confirm permanent removal',
    }) as HTMLButtonElement
    expect(confirm.disabled).toBe(true)
  })

  it('acknowledges one or all current Agent items and refetches the authoritative list', async () => {
    const offline = {
      id: 'agent_offline:agent:' + AGENT_ID,
      kind: 'agent_offline',
      severity: 'warning',
      subject_kind: 'agent',
      subject_id: AGENT_ID,
      subject_label: AGENT_ID,
      message: 'the Agent has not reported within the liveness window',
      observed_at: '2026-08-12T08:00:00Z',
      evidence_key: 'offline-1',
    }
    const security = {
      ...offline,
      id: 'agent_security_event:agent:' + AGENT_ID,
      kind: 'agent_security_event',
      severity: 'critical',
      message: '2 security events were recorded',
      observed_at: null,
      evidence_key: 'security-2',
    }
    let attention = [offline, security]
    const ackBodies: string[] = []
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      [`/api/admin/v1/agents/${AGENT_ID}`]: () =>
        jsonResponse({ ...AGENT_DIAGNOSTIC, attention }, 200),
      [`/api/admin/v1/agents/${AGENT_ID}/audit`]: () =>
        jsonResponse({ agent_id: AGENT_ID, items: [] }, 200),
      [`/api/admin/v1/agents/${AGENT_ID}/attention/acknowledgments`]: (ctx) => {
        const body = JSON.parse(ctx.body ?? '{}') as {
          items: { kind: string; evidence_key: string }[]
        }
        ackBodies.push(ctx.body ?? '')
        const acknowledgedKinds = new Set(body.items.map((entry) => entry.kind))
        attention = attention.filter((entry) => !acknowledgedKinds.has(entry.kind))
        return jsonResponse(
          {
            agent_id: AGENT_ID,
            attention,
            acknowledged: body.items,
            skipped: [],
          },
          200,
        )
      },
    })
    renderAt(`/admin/agents/${AGENT_ID}`)
    await screen.findByRole('heading', { level: 1, name: /Agent 0195f2a1/ })

    // Individual acknowledgment sends exactly the displayed evidence boundary.
    fireEvent.click(await screen.findByRole('button', { name: 'Acknowledge agent_offline' }))
    await waitFor(() => expect(ackBodies).toHaveLength(1))
    expect(JSON.parse(ackBodies[0])).toEqual({
      items: [{ kind: 'agent_offline', evidence_key: 'offline-1' }],
    })
    await waitFor(() =>
      expect(screen.queryByRole('button', { name: 'Acknowledge agent_offline' })).toBeNull(),
    )

    // Bulk confirmation covers only the remaining displayed Agent item.
    fireEvent.click(screen.getByRole('button', { name: 'Acknowledge current Agent items' }))
    await waitFor(() => expect(ackBodies).toHaveLength(2))
    expect(JSON.parse(ackBodies[1])).toEqual({
      items: [{ kind: 'agent_security_event', evidence_key: 'security-2' }],
    })
    await waitFor(() =>
      expect(
        screen.queryByRole('button', {
          name: 'Acknowledge current Agent items',
        }),
      ).toBeNull(),
    )
  })

/**
 * The mount paths the Agent's stored storage evidence holds (issue #216), in the
 * shape the Server can actually answer: the cadence belongs to the whole answer,
 * so a measured cadence makes every path of it either reported or silent, and the
 * two series behind a path are written by the one Report that observed it, so a
 * path never carries a series nothing was ever observed for. One path's newest
 * reading was released by retention, which is a different statement from a path
 * that is simply still observed.
 */
function storageMountsAnswer(overrides: Record<string, unknown> = {}) {
  const series = (
    metric: string,
    fields: Record<string, unknown> = {},
  ) => ({
    metric,
    observed: true,
    firstObservedAt: '2026-08-12T00:00:00Z',
    lastObservedAt: '2026-08-12T08:00:00Z',
    lastReceivedAt: '2026-08-12T08:00:01Z',
    observationCount: 12,
    replayedCount: 0,
    correctedCount: 0,
    releasedBefore: null,
    latestValue: 0,
    latestObservedAt: '2026-08-12T08:00:00Z',
    latestReceivedAt: '2026-08-12T08:00:01Z',
    latestDelaySeconds: 1,
    latestClockSuspect: false,
    ...fields,
  })
  return {
    agentId: AGENT_ID,
    answeredAt: '2026-08-12T08:00:05Z',
    cadenceSeconds: 900,
    silenceThresholdSeconds: 900,
    usedMetric: 'disk_used_bytes',
    capacityMetric: 'disk_total_bytes',
    mountLimit: 256,
    truncated: true,
    collectionPaused: false,
    mounts: [
      {
        mountPath: '/data',
        observationState: 'reported',
        silentSeconds: 60,
        used: series('disk_used_bytes', { latestValue: 512 }),
        capacity: series('disk_total_bytes', { latestValue: 1024 }),
      },
      {
        mountPath: '/bulk-299',
        observationState: 'silent',
        silentSeconds: 1200,
        used: series('disk_used_bytes', {
          observationCount: 4,
          latestValue: null,
          releasedBefore: '2026-08-12T06:00:00Z',
        }),
        capacity: series('disk_total_bytes', { observationCount: 4, latestValue: 1024 * 1024 }),
      },
      {
        mountPath: '/logs',
        observationState: 'silent',
        silentSeconds: 3000,
        used: series('disk_used_bytes', { observationCount: 2, latestValue: 4096 }),
        capacity: series('disk_total_bytes', { observationCount: 2, latestValue: 8192 }),
      },
    ],
    ...overrides,
  }
}

  it('reads the Host resource history from the Agent that observed it once, apart from its Nodes', async () => {
    const hostCalls: string[] = []
    const instant = (offsetMs: number) =>
      new Date(Date.now() + offsetMs).toISOString().replace(/\.\d{3}Z$/, 'Z')
    const hourAgo = instant(-60 * 60 * 1000)
    const sample = (value: number) => ({
      observedAt: hourAgo,
      receivedAt: instant(-60 * 60 * 1000 + 1000),
      value,
      grain: 'raw',
      source: 'raw',
      minValue: value,
      maxValue: value,
      sampleCount: 1,
      lastObservedAt: hourAgo,
      delaySeconds: 1,
      clockSuspect: false,
    })
    const historyResponse = (
      metric: string,
      dimension: string,
      items: unknown[],
    ) => ({
      scopeKind: 'host',
      scopeKey: AGENT_ID,
      nodeId: null,
      metric,
      dimension,
      from: instant(-24 * 60 * 60 * 1000),
      to: instant(0),
      requestedFrom: instant(-24 * 60 * 60 * 1000),
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
          from: instant(-24 * 60 * 60 * 1000),
          to: instant(0),
          grain: 'raw',
          source: 'raw',
          pointCount: items.length,
          truncated: false,
        },
      ],
      series: {
        observed: items.length > 0,
        firstObservedAt: items.length > 0 ? hourAgo : null,
        lastObservedAt: items.length > 0 ? hourAgo : null,
        lastReceivedAt:
          items.length > 0 ? instant(-60 * 60 * 1000 + 1000) : null,
        observationCount: items.length,
        replayedCount: 0,
        correctedCount: 0,
        sampledCount: items.length,
        coverageSeconds: 60,
        windowSeconds: 86400,
        latestDelaySeconds: 1,
        latestClockSuspect: false,
      },
      items,
      gaps: [],
    })
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      [`/api/admin/v1/agents/${AGENT_ID}`]: () =>
        jsonResponse(AGENT_DIAGNOSTIC, 200),
      [`/api/admin/v1/agents/${AGENT_ID}/audit`]: () =>
        jsonResponse({ agent_id: AGENT_ID, items: [] }, 200),
      [`/api/admin/v1/agents/${AGENT_ID}/storage-mounts`]: () =>
        jsonResponse(storageMountsAnswer(), 200),
      [`/api/admin/v1/agents/${AGENT_ID}/metric-history*`]: (ctx) => {
        hostCalls.push(ctx.request.url)
        const dimension =
          new URL(ctx.request.url).searchParams.get('dimension') ?? ''
        if (dimension === '')
          return jsonResponse(
            historyResponse('cpu_percent', '', [sample(11), sample(2.5)]),
            200,
          )
        return jsonResponse(
          historyResponse('disk_used_bytes', dimension, [
            // '/data' and '/data ' are two mounts the Agent reported, so each
            // path reads the value recorded for that path alone.
            sample(dimension === '/data' ? 100 : dimension === '/data ' ? 200 : 900),
          ]),
          200,
        )
      },
    })
    renderAt(`/admin/agents/${AGENT_ID}`)

    await screen.findByRole('heading', { level: 1, name: /Agent 0195f2a1/ })
    // 06: the shared Host series is its own section, not a row inside the
    // Diagnostics section that reports the same Host's latest observation.
    await screen.findByRole('heading', {
      level: 2,
      name: 'Host resource history',
    })
    const section = agentHostSection()
    expect(section.textContent).toContain('06')
    expect(
      within(section).getByRole('heading', {
        level: 2,
        name: 'Host resource history',
      }),
    ).toBeTruthy()
    expect(
      within(section).getByText(/belongs to the Agent, not to a Node/),
    ).toBeTruthy()
    // A shared reading names whose evidence it is and how many Nodes read it.
    const owner = section.querySelector(
      '[data-slot="host-metric-history-owner"]',
    )
    expect(owner?.textContent).toContain(AGENT_ID)
    expect(owner?.textContent).toContain('2 retained Nodes')
    // The Agent's Host series is never the Node Process series.
    expect(within(section).queryByText('Process CPU')).toBeNull()

    await waitFor(() => expect(hostCalls.length).toBeGreaterThan(0))
    const asked = decodeURIComponent(hostCalls[0])
    expect(asked).toContain(`/agents/${AGENT_ID}/metric-history`)
    expect(asked).toContain('metric=cpu_percent')
    const samples = () =>
      section.querySelector('[data-slot="metric-history-samples"]')
        ?.textContent ?? ''
    await waitFor(() => expect(samples()).toContain('11%'))
    expect(samples()).toContain('2.5%')
    expect(
      section.querySelector('[data-slot="metric-history-chart"]'),
    ).not.toBeNull()

    // A storage series is named by the mount path the Agent reported, so the
    // panel asks for the path before it claims any series.
    const beforeMetric = hostCalls.length
    fireEvent.change(within(section).getByLabelText('Metric series'), {
      target: { value: 'disk_used_bytes' },
    })
    expect(
      await within(section).findByText(
        /Enter the mount path the Agent reported/,
      ),
    ).toBeTruthy()
    expect(
      section.querySelector('[data-slot="metric-history-chart"]'),
    ).toBeNull()
    // An empty path is not a mount, so no query is issued for it: the panel does
    // not ask the Server for a series without an identity and then mask it.
    expect(hostCalls.length).toBe(beforeMetric)
    const beforeMount = hostCalls.length
    fireEvent.change(within(section).getByLabelText('Mount path'), {
      target: { value: '/data' },
    })
    await waitFor(() => expect(hostCalls.length).toBeGreaterThan(beforeMount))
    expect(decodeURIComponent(hostCalls[hostCalls.length - 1])).toContain(
      'dimension=/data',
    )
    await waitFor(() => expect(samples()).toContain('100 B'))

    // A mount identity is literal, so a path that differs only in surrounding
    // whitespace is a different mount: '/data ' is read as the series the Agent
    // reported for it, never trimmed into '/data' and read as that one instead.
    fireEvent.change(within(section).getByLabelText('Mount path'), {
      target: { value: '/data ' },
    })
    await waitFor(() =>
      expect(
        new URL(hostCalls[hostCalls.length - 1]).searchParams.get('dimension'),
      ).toBe('/data '),
    )
    await waitFor(() => expect(samples()).toContain('200 B'))
    expect(samples()).not.toContain('100 B')

    // The Host family never offers the Node Process series.
    const options = Array.from(
      within(section)
        .getByLabelText('Metric series')
        .querySelectorAll('option'),
    ).map((option) => (option as HTMLOptionElement).value)
    expect(options).toContain('disk_total_bytes')
    expect(options).not.toContain('process_cpu_percent')
  })

  it('names the mount paths the stored storage evidence holds and reads one on demand', async () => {
    const instant = (offsetMs: number) =>
      new Date(Date.now() + offsetMs).toISOString().replace(/\.\d{3}Z$/, 'Z')
    const hourAgo = instant(-60 * 60 * 1000)
    const historyCalls: string[] = []
    const historyResponse = (metric: string, dimension: string, value: number) => ({
      scopeKind: 'host',
      scopeKey: AGENT_ID,
      nodeId: null,
      metric,
      dimension,
      from: instant(-24 * 60 * 60 * 1000),
      to: instant(0),
      requestedFrom: instant(-24 * 60 * 60 * 1000),
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
          from: instant(-24 * 60 * 60 * 1000),
          to: instant(0),
          grain: 'raw',
          source: 'raw',
          pointCount: 1,
          truncated: false,
        },
      ],
      series: {
        observed: true,
        firstObservedAt: hourAgo,
        lastObservedAt: hourAgo,
        lastReceivedAt: instant(-60 * 60 * 1000 + 1000),
        observationCount: 1,
        replayedCount: 0,
        correctedCount: 0,
        sampledCount: 1,
        coverageSeconds: 60,
        windowSeconds: 86400,
        latestDelaySeconds: 1,
        latestClockSuspect: false,
      },
      items: [
        {
          observedAt: hourAgo,
          receivedAt: instant(-60 * 60 * 1000 + 1000),
          value,
          grain: 'raw',
          source: 'raw',
          minValue: value,
          maxValue: value,
          sampleCount: 1,
          lastObservedAt: hourAgo,
          delaySeconds: 1,
          clockSuspect: false,
        },
      ],
      gaps: [],
    })
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      [`/api/admin/v1/agents/${AGENT_ID}`]: () => jsonResponse(AGENT_DIAGNOSTIC, 200),
      [`/api/admin/v1/agents/${AGENT_ID}/audit`]: () =>
        jsonResponse({ agent_id: AGENT_ID, items: [] }, 200),
      [`/api/admin/v1/agents/${AGENT_ID}/storage-mounts`]: () =>
        jsonResponse(storageMountsAnswer(), 200),
      [`/api/admin/v1/agents/${AGENT_ID}/metric-history*`]: (ctx) => {
        historyCalls.push(ctx.request.url)
        const asked = new URL(ctx.request.url).searchParams
        return jsonResponse(
          historyResponse(
            asked.get('metric') ?? '',
            asked.get('dimension') ?? '',
            4242,
          ),
          200,
        )
      },
    })
    renderAt(`/admin/agents/${AGENT_ID}`)

    const section = await screen.findByRole('heading', {
      level: 2,
      name: 'Storage by mount path',
    })
    expect(agentHostSection().contains(section)).toBe(true)
    const list = await within(agentHostSection()).findByRole('region', {
      name: 'Storage mount paths',
    })

    // The list is the Server's own order, and each row carries the path exactly
    // as the Agent reported it: nothing here normalizes or renames a path.
    const rows = Array.from(
      list.querySelectorAll('[data-slot="storage-mount-row"]'),
    ) as HTMLElement[]
    expect(rows.map((row) => row.dataset.mountPath)).toEqual([
      '/data',
      '/bulk-299',
      '/logs',
    ])
    expect(rows.map((row) => row.dataset.mountState)).toEqual([
      'reported',
      'silent',
      'silent',
    ])

    // A path still observed shows its newest reading as a share of capacity.
    expect(rows[0].textContent).toContain('512 B of 1.00 KiB · 50%')
    expect(within(rows[0]).getByText('Current')).toBeTruthy()

    // A path that stopped being reported is silent against the measured cadence
    // only, and the readings already stored are stated to stay.
    expect(within(rows[1]).getByText('Stale')).toBeTruthy()
    expect(rows[1].textContent).toContain(
      'longer than the 15 minutes a silence is judged against on this Host',
    )
    expect(rows[1].textContent).toContain('The readings already stored stay on the series.')
    // Retention released its newest reading: that is not the same statement as a
    // series nothing was ever observed for.
    expect(rows[1].textContent).toContain(
      'retention released this series before 2026-08-12T06:00:00Z',
    )

    // The measured cadence judges the whole answer: a path that stopped being
    // reported is silent, and no path of a measured answer is answered unknown.
    expect(within(rows[2]).getByText('Stale')).toBeTruthy()
    expect(rows[2].textContent).toContain('Silent for 50 minutes')
    expect(rows[2].textContent).toContain('2 observations recorded')
    expect(rows.every((row) => !row.textContent?.includes('No cadence could be measured'))).toBe(
      true,
    )

    // Every path in the list is readable, and the Server's answer states what the
    // list left out instead of hiding older paths silently.
    expect(within(list).getAllByRole('button', { name: /^Read the storage series of/ })).toHaveLength(3)
    const panel = agentHostSection().querySelector('[data-slot="storage-mounts-panel"]')!
    expect(panel.textContent).toContain('at most 256 mount paths')
    expect(panel.textContent).toContain('The oldest of them are not in this answer')
    expect(panel.textContent).toContain('every 15 minutes')
    expect(panel.textContent).not.toContain('No cadence could be measured from the stored Host')
    expect(panel.querySelector('[data-slot="storage-mounts-pause"]')).toBeNull()

    // Reading a path chooses the storage series that path names, so the Operator
    // never types the path from memory.
    fireEvent.click(within(rows[0]).getByRole('button', { name: 'Read the storage series of /data' }))
    await waitFor(() =>
      expect(historyCalls.some((url) => {
        const params = new URL(url).searchParams
        return params.get('metric') === 'disk_used_bytes' && params.get('dimension') === '/data'
      })).toBe(true),
    )
    await waitFor(() =>
      expect(
        agentHostSection().querySelector('[data-slot="metric-history-samples"]')?.textContent,
      ).toContain('4.14 KiB'),
    )
  })

  it('states the pause the Server is holding instead of calling a path silent', async () => {
    const stored = storageMountsAnswer({ collectionPaused: true })
    const paused = {
      ...stored,
      mounts: stored.mounts.map((mount) => ({
        ...mount,
        observationState: 'unknown',
        silentSeconds: null,
      })),
    }
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      [`/api/admin/v1/agents/${AGENT_ID}`]: () => jsonResponse(AGENT_DIAGNOSTIC, 200),
      [`/api/admin/v1/agents/${AGENT_ID}/audit`]: () =>
        jsonResponse({ agent_id: AGENT_ID, items: [] }, 200),
      [`/api/admin/v1/agents/${AGENT_ID}/storage-mounts`]: () => jsonResponse(paused, 200),
    })
    renderAt(`/admin/agents/${AGENT_ID}`)
    await screen.findByRole('heading', { level: 2, name: 'Storage by mount path' })

    const list = await within(agentHostSection()).findByRole('region', {
      name: 'Storage mount paths',
    })
    const panel = agentHostSection().querySelector('[data-slot="storage-mounts-panel"]')!
    const rows = Array.from(
      list.querySelectorAll('[data-slot="storage-mount-row"]'),
    ) as HTMLElement[]

    // Every path of the answer is unknown, and the card says the Server is the one
    // holding the readings back: a path that keeps being reported is never called
    // stale for a pause it did not cause.
    expect(rows.map((row) => row.dataset.mountState)).toEqual(['unknown', 'unknown', 'unknown'])
    expect(
      rows.every((row) =>
        row.textContent?.includes('not called stale for a pause it did not cause'),
      ),
    ).toBe(true)
    expect(panel.textContent).not.toContain('Silent for')
    const pause = panel.querySelector('[data-slot="storage-mounts-pause"]')!
    expect(pause.textContent).toContain('Low-space protection is holding optional history back')
    expect(pause.textContent).toContain('no path is judged silent')
    // The readings already stored are still answered, with their own instants.
    expect(rows[0].textContent).toContain('512 B of 1.00 KiB · 50%')
    expect(rows[0].textContent).toContain('12 observations recorded')
  })

  it('keeps the last successful mount paths and says the refresh failed', async () => {
    let failed = false
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      [`/api/admin/v1/agents/${AGENT_ID}`]: () => jsonResponse(AGENT_DIAGNOSTIC, 200),
      [`/api/admin/v1/agents/${AGENT_ID}/audit`]: () =>
        jsonResponse({ agent_id: AGENT_ID, items: [] }, 200),
      [`/api/admin/v1/agents/${AGENT_ID}/storage-mounts`]: () =>
        failed ? errorBody('unavailable', 503) : jsonResponse(storageMountsAnswer(), 200),
    })
    renderAt(`/admin/agents/${AGENT_ID}`)
    await screen.findByRole('heading', { level: 2, name: 'Storage by mount path' })

    const list = await within(agentHostSection()).findByRole('region', {
      name: 'Storage mount paths',
    })
    const panel = agentHostSection().querySelector('[data-slot="storage-mounts-panel"]')!
    expect(list.querySelectorAll('[data-slot="storage-mount-row"]')).toHaveLength(3)
    expect(panel.querySelector('[data-slot="storage-mounts-refresh-error"]')).toBeNull()

    failed = true
    await act(async () => {
      await adminQueryClient.invalidateQueries({
        queryKey: ['admin', 'agents', AGENT_ID, 'storage-mounts'],
      })
    })

    // The stored answer stays on the card, and the card states that it is the last
    // good one instead of showing it as if the refresh had succeeded.
    const alert = await within(panel as HTMLElement).findByRole('alert')
    expect(alert.getAttribute('data-slot')).toBe('storage-mounts-refresh-error')
    expect(alert.textContent).toContain('Failed to refresh the mount paths')
    expect(alert.textContent).toContain('2026-08-12T08:00:05Z')
    expect(list.querySelectorAll('[data-slot="storage-mount-row"]')).toHaveLength(3)

    // And the retry it offers reads the answer again.
    failed = false
    fireEvent.click(within(alert).getByRole('button', { name: 'Try again' }))
    await waitFor(() =>
      expect(panel.querySelector('[data-slot="storage-mounts-refresh-error"]')).toBeNull(),
    )
    expect(list.querySelectorAll('[data-slot="storage-mount-row"]')).toHaveLength(3)
  })
})
