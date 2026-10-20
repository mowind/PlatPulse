import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import App from '../App'
import { adminQueryClient } from '../api/admin'
import { resetNetworkTupleWrites } from '../api/networkTupleWrites'
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
  resetNetworkTupleWrites()
})

describe('PAGE-ADMIN-NETWORKS (Network Registry)', () => {
  it('lists Network, chain ID, Node counts and typed mismatch outcomes, and opens the complete tuple per row', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/networks': () => jsonResponse([NETWORK], 200),
    })
    renderAt('/admin/networks')

    await screen.findByRole('heading', { level: 1, name: 'Networks' })
    const table = await screen.findByRole('table')
    expect(within(table).getAllByRole('columnheader').map((cell) => cell.textContent)).toEqual([
      'Network', 'Chain ID', 'Nodes', 'Mismatches', 'Details',
    ])
    const row = await screen.findByRole('row', { name: /PlatON E2E Network/ })
    expect(row.textContent).toContain('platon-e2e')
    expect(row.textContent).toContain('210425')
    expect(row.textContent).toContain('1 active · 0 retired')
    expect(row.textContent).toContain('Mismatched')
    // Configuration fields wait behind the row's details.
    expect(within(row).queryByRole('button', { name: /Show full\s*Genesis hash/ })).toBeNull()
    const toggle = within(row).getByRole('button', { name: 'Details for PlatON E2E Network' })
    fireEvent.click(toggle)
    expect(toggle.getAttribute('aria-expanded')).toBe('true')
    const region = document.getElementById(toggle.getAttribute('aria-controls') ?? '') as HTMLElement
    expect(within(region).getByText('P2P network ID')).toBeTruthy()
    expect(within(region).getByText('Address HRP')).toBeTruthy()
    expect(within(region).getByText('lat')).toBeTruthy()
    expect(within(region).getByRole('button', { name: /Show full\s*Genesis hash/ })).toBeTruthy()
  })

  it('does not present zero reported mismatches as Current or verified identity', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/networks': () => jsonResponse([{ ...NETWORK, mismatched_node_count: 0 }], 200),
    })
    renderAt('/admin/networks')

    const row = await screen.findByRole('row', { name: /PlatON E2E Network/ })
    expect(row.textContent).toContain('No mismatch reported')
    expect(row.textContent).not.toContain('Current')
  })

  it('opens the complete Genesis hash on demand, copies it, and keeps it selectable when copy fails', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/networks': () => jsonResponse([NETWORK], 200),
    })
    renderAt('/admin/networks')

    const summary = await screen.findByRole('row', { name: /PlatON E2E Network/ })
    fireEvent.click(within(summary).getByRole('button', { name: 'Details for PlatON E2E Network' }))
    const row = screen.getByRole('row', { name: /Registered identity tuple/ })
    // Short by default: the complete hash is not laid out in the details.
    expect(within(row).getByText('0x111111…1111')).toBeTruthy()
    expect(within(row).queryByText(NETWORK.genesis_hash, { exact: true })).toBeNull()

    const toggle = within(row).getByRole('button', { name: 'Show full Genesis hash' })
    expect(toggle.getAttribute('aria-expanded')).toBe('false')
    const panelId = toggle.getAttribute('aria-controls')
    expect(panelId).toBeTruthy()
    fireEvent.click(toggle)
    expect(toggle.getAttribute('aria-expanded')).toBe('true')
    expect(toggle.textContent).toBe('Hide full Genesis hash')
    const panel = document.getElementById(panelId ?? '')
    expect(panel).toBeTruthy()
    expect(within(panel as HTMLElement).getByText(NETWORK.genesis_hash, { exact: true })).toBeTruthy()

    const writeText = vi.fn().mockResolvedValue(undefined)
    const previousClipboard = Object.getOwnPropertyDescriptor(navigator, 'clipboard')
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
    try {
      fireEvent.click(within(panel as HTMLElement).getByRole('button', { name: 'Copy Genesis hash' }))
      expect(await within(panel as HTMLElement).findByText('Copied to clipboard.')).toBeTruthy()
      expect(writeText).toHaveBeenCalledWith(NETWORK.genesis_hash)

      Object.defineProperty(navigator, 'clipboard', {
        configurable: true,
        value: { writeText: vi.fn().mockRejectedValue(new Error('denied')) },
      })
      fireEvent.click(within(panel as HTMLElement).getByRole('button', { name: 'Copy Genesis hash' }))
      expect(
        await within(panel as HTMLElement).findByText('Copy unavailable; select the full Genesis hash shown here.'),
      ).toBeTruthy()
      expect(within(panel as HTMLElement).queryByText('Copied to clipboard.')).toBeNull()
      expect(within(panel as HTMLElement).getByText(NETWORK.genesis_hash, { exact: true })).toBeTruthy()

      Object.defineProperty(navigator, 'clipboard', { configurable: true, value: undefined })
      fireEvent.click(within(panel as HTMLElement).getByRole('button', { name: 'Copy Genesis hash' }))
      expect(
        await within(panel as HTMLElement).findByText('Copy unavailable; select the full Genesis hash shown here.'),
      ).toBeTruthy()
    } finally {
      if (previousClipboard) Object.defineProperty(navigator, 'clipboard', previousClipboard)
      else delete (navigator as unknown as { clipboard?: unknown }).clipboard
    }

    // Escape inside the opened value collapses it and returns focus to the toggle.
    const copy = within(panel as HTMLElement).getByRole('button', { name: 'Copy Genesis hash' })
    copy.focus()
    fireEvent.keyDown(copy, { key: 'Escape' })
    expect(toggle.getAttribute('aria-expanded')).toBe('false')
    expect(document.getElementById(panelId ?? '')).toBeNull()
    expect(document.activeElement).toBe(toggle)
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

  it('states the whole Node-side remedy, including the Agent restart, as the exact alert copy', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/networks/platon-e2e': () => jsonResponse(NETWORK_DETAIL, 200),
    })
    renderAt('/admin/networks/platon-e2e')

    const remedy = await screen.findByText(/Correct the observation on the Node itself/)
    const banner = remedy.closest('[role="alert"]')
    expect(banner).toBeTruthy()
    expect(banner?.textContent).toContain("restart that Host's Agent")
    // Pinned in full on purpose: this copy is the operator contract (ADR 0011
    // Decision 2 — the Agent restart is the remedy, the tuple is never the fix).
    expect((banner?.textContent ?? '').replace(/\s+/g, ' ').trim()).toBe(
      `Mismatched 1 Node observes an identity that contradicts this Registry tuple. ` +
        `Correct the observation on the Node itself: its build and its configuration, ` +
        `not this Registry, and then restart that Host's Agent — the identity stamp is ` +
        `refreshed only when the Agent re-reads its Node. Rewriting the tuple to match ` +
        `one Node is not a remedy — it would contradict every other Node on this ` +
        `Network. Block History already rejected from those Nodes is never merged and ` +
        `never backfilled, so the affected heights stay a permanent gap. See "Node ` +
        `Network Identity at onboarding and after a Node build upgrade" in ` +
        `docs/deployment.md.`,
    )
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
    // A tuple edit changes no Node state and reclaims no rejected Block History.
    expect(
      screen.getByText(/Block History already rejected on a mismatch is not reclaimed/),
    ).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Confirm tuple update' }))
    expect(await screen.findByText('Updated PlatON E2E Network v2.')).toBeTruthy()
    // The authoritative refetch shows the new Server-owned name.
    await waitFor(() => {
      expect(displayName).toBe('PlatON E2E Network v2')
    })
  })
})

describe('Expected identity tuple editor', () => {
  const NETWORK_URL = '/api/admin/v1/networks/platon-e2e'

  function tuplePanel(): HTMLElement {
    return screen
      .getByRole('heading', { level: 2, name: 'Expected identity tuple' })
      .closest('[data-slot="card-x"]') as HTMLElement
  }

  function field(label: string): HTMLInputElement {
    return within(tuplePanel()).getByLabelText(label) as HTMLInputElement
  }

  async function backgroundRefresh() {
    await act(async () => {
      await adminQueryClient.invalidateQueries()
    })
  }

  type Put = { displayName: string; genesisHash: string; chainId: number; p2pNetworkId: number; addressHrp: string }

  const OTHER_NETWORK_URL = '/api/admin/v1/networks/platon-other'
  const OTHER_NETWORK_DETAIL = {
    ...NETWORK_DETAIL,
    network_key: 'platon-other',
    display_name: 'PlatON Other Network',
    nodes: [],
  }

  function serve(options: { put?: (body: Put) => Response | Promise<Response> } = {}) {
    const state = {
      detail: { ...NETWORK_DETAIL } as typeof NETWORK_DETAIL,
      puts: [] as Put[],
      // While set, reading the Network detail fails.
      unreadable: false,
      // While set, a read takes its snapshot at once but answers only when
      // the gate opens (a read begun before a write, answered after it).
      readGate: null as Promise<void> | null,
      // While set, the next Admin read answers that the session is gone.
      authExpired: false,
      other: { ...OTHER_NETWORK_DETAIL } as typeof OTHER_NETWORK_DETAIL,
      otherPuts: [] as Put[],
      // Every request other than a read, in order (method and path).
      writes: [] as string[],
    }
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(String(input), init)
      const url = request.url.replace(TEST_ORIGIN, '')
      if (request.method !== 'GET' && request.method !== 'HEAD') state.writes.push(request.method + ' ' + url)
      if (url === '/api/public/v1/session') return jsonResponse(OWNER_SESSION, 200)
      if (state.authExpired && url.startsWith('/api/admin/') && request.method === 'GET') {
        state.authExpired = false
        return jsonResponse({ error: { code: 'auth_required', message: 'sign in again', requestId: 'r-auth' } }, 401)
      }
      if (url === NETWORK_URL && request.method === 'GET') {
        if (state.unreadable) {
          return jsonResponse({ error: { code: 'unavailable', message: 'read failed' } }, 503)
        }
        const snapshot = state.detail
        if (state.readGate) await state.readGate
        return jsonResponse(snapshot, 200)
      }
      if (url === OTHER_NETWORK_URL && request.method === 'GET') return jsonResponse(state.other, 200)
      if (url === OTHER_NETWORK_URL && request.method === 'PUT') {
        const body = (await request.json()) as Put
        state.otherPuts.push(body)
        state.other = { ...state.other, display_name: body.displayName }
        return jsonResponse({ networkKey: 'platon-other', displayName: body.displayName }, 200)
      }
      if (url === NETWORK_URL && request.method === 'PUT') {
        const body = (await request.json()) as Put
        state.puts.push(body)
        if (options.put) return options.put(body)
        state.detail = {
          ...state.detail,
          display_name: body.displayName,
          genesis_hash: body.genesisHash,
          chain_id: body.chainId,
          p2p_network_id: body.p2pNetworkId,
          address_hrp: body.addressHrp,
        }
        return jsonResponse({ networkKey: 'platon-e2e', displayName: body.displayName }, 200)
      }
      return jsonResponse({ error: { code: 'not_found' } }, 404)
    })
    vi.stubGlobal('fetch', fetchMock)
    return state
  }

  async function open() {
    renderAt('/admin/networks/platon-e2e')
    await screen.findByRole('heading', { level: 1, name: /PlatON E2E Network/ })
  }

  it('discards a cancelled edit, keeps a draft through a refresh, and starts the next edit from the Server', async () => {
    const state = serve()
    await open()
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    expect(document.activeElement).toBe(field('Display name'))
    // One way out of the editor: Cancel, which discards the round.
    expect(screen.queryByRole('button', { name: 'Close editor' })).toBeNull()
    fireEvent.change(field('Display name'), { target: { value: 'Draft name' } })
    fireEvent.change(field('Chain ID'), { target: { value: '7' } })

    // A refresh with a new Server tuple keeps the draft and says so.
    state.detail = { ...state.detail, display_name: 'Renamed elsewhere', address_hrp: 'atp' }
    await backgroundRefresh()
    await screen.findByRole('heading', { level: 1, name: /Renamed elsewhere/ })
    expect(field('Display name').value).toBe('Draft name')
    expect(field('Chain ID').value).toBe('7')
    expect(field('Address HRP').value).toBe('lat')
    expect(within(tuplePanel()).getByText(/changed on the Server while you were editing/)).toBeTruthy()
    // The Server's current tuple stays readable beside the kept draft.
    const moved = tuplePanel().querySelector('[data-slot="tuple-server-moved"]') as HTMLElement
    expect(moved.textContent).toContain('Server now: Renamed elsewhere')
    expect(moved.textContent).toContain('Address HRP atp')

    fireEvent.click(within(tuplePanel()).getByRole('button', { name: 'Cancel' }))
    // The read view is the Server tuple; the cancelled draft is gone.
    expect(within(tuplePanel()).getByText('Renamed elsewhere')).toBeTruthy()
    expect(within(tuplePanel()).getByText('210425')).toBeTruthy()
    expect(within(tuplePanel()).getByText('atp')).toBeTruthy()
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Edit tuple' }))

    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    expect(field('Display name').value).toBe('Renamed elsewhere')
    expect(field('Chain ID').value).toBe('210425')
    expect(field('Address HRP').value).toBe('atp')
    expect(within(tuplePanel()).queryByText(/while you were editing/)).toBeNull()
    expect(state.puts).toHaveLength(0)
  })

  it('needs a fresh confirmation after any field changes, and sends exactly the confirmed tuple', async () => {
    const state = serve()
    await open()
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    fireEvent.change(field('Display name'), { target: { value: '  PlatON E2E v2  ' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save tuple' }))
    expect(screen.getByRole('button', { name: 'Confirm tuple update' })).toBeTruthy()

    for (const [label, value] of [
      ['Display name', 'PlatON E2E v3'],
      ['Genesis hash', '0x' + '2'.repeat(64)],
      ['Chain ID', '210426'],
      ['P2P network ID', '2'],
      ['Address HRP', 'atp'],
    ] as const) {
      fireEvent.change(field(label), { target: { value } })
      expect(screen.queryByRole('button', { name: 'Confirm tuple update' })).toBeNull()
      expect(screen.queryByText(/Update the expected identity tuple\?/)).toBeNull()
      fireEvent.click(screen.getByRole('button', { name: 'Save tuple' }))
      expect(screen.getByRole('button', { name: 'Confirm tuple update' })).toBeTruthy()
    }
    // The risk statement is complete beside the confirmation.
    expect(
      screen.getByText(/Existing Nodes whose observed identity\s+contradicts the new tuple surface as typed mismatches; no Node state changes,\s+and Block History already rejected on a mismatch is not reclaimed\./),
    ).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Confirm tuple update' }))
    expect(await screen.findByText('Updated PlatON E2E v3.')).toBeTruthy()
    expect(state.puts).toEqual([
      {
        displayName: 'PlatON E2E v3',
        genesisHash: '0x' + '2'.repeat(64),
        chainId: 210426,
        p2pNetworkId: 2,
        addressHrp: 'atp',
      },
    ])
    // Success closes the round on the Server's tuple.
    expect(screen.getByRole('heading', { level: 1, name: /PlatON E2E v3/ })).toBeTruthy()
    expect(within(tuplePanel()).getByText('210426')).toBeTruthy()
    expect(within(tuplePanel()).queryByRole('textbox')).toBeNull()
  })

  it('never reopens with an earlier confirmation', async () => {
    serve()
    await open()
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    fireEvent.change(field('Display name'), { target: { value: 'Confirmed once' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save tuple' }))
    expect(screen.getByRole('button', { name: 'Confirm tuple update' })).toBeTruthy()
    fireEvent.click(within(tuplePanel()).getByRole('button', { name: 'Cancel' }))
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    expect(screen.queryByRole('button', { name: 'Confirm tuple update' })).toBeNull()
    expect(screen.getByRole('button', { name: 'Save tuple' })).toBeTruthy()
    expect(field('Display name').value).toBe('PlatON E2E Network')
  })

  it('keeps the existing validity rules at the final step', async () => {
    const state = serve()
    await open()
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    fireEvent.click(screen.getByRole('button', { name: 'Save tuple' }))
    expect(screen.getByRole('button', { name: 'Confirm tuple update' })).toBeTruthy()
    // An invalid value after Save withdraws the confirmation; Save then
    // refuses it, so nothing invalid reaches Confirm.
    fireEvent.change(field('Chain ID'), { target: { value: '' } })
    expect(screen.queryByRole('button', { name: 'Confirm tuple update' })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Save tuple' }))
    expect(screen.queryByRole('button', { name: 'Confirm tuple update' })).toBeNull()
    fireEvent.change(field('Chain ID'), { target: { value: '-1' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save tuple' }))
    expect(screen.queryByRole('button', { name: 'Confirm tuple update' })).toBeNull()
    expect(state.puts).toHaveLength(0)
  })

  it('re-runs the form validity at Confirm instead of trusting the Save step', async () => {
    const state = serve()
    await open()
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    fireEvent.click(screen.getByRole('button', { name: 'Save tuple' }))
    // The control no longer holds a valid value even though no change event
    // reached the editor (autofill, extensions, a browser quirk).
    field('Chain ID').value = ''
    fireEvent.click(screen.getByRole('button', { name: 'Confirm tuple update' }))
    await act(async () => {
      await Promise.resolve()
    })
    expect(state.puts).toHaveLength(0)
    expect(screen.queryByRole('button', { name: 'Confirm tuple update' })).toBeNull()
    expect(screen.getByRole('button', { name: 'Save tuple' })).toBeTruthy()
  })

  it('locks only this editor while the update is in flight and sends it once', async () => {
    let release: () => void = () => {}
    const state = serve({
      put: async (body) => {
        await new Promise<void>((resolve) => {
          release = resolve
        })
        state.detail = { ...state.detail, display_name: body.displayName }
        return jsonResponse({ networkKey: 'platon-e2e', displayName: body.displayName }, 200)
      },
    })
    await open()
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    fireEvent.change(field('Display name'), { target: { value: 'Slow tuple' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save tuple' }))
    fireEvent.click(screen.getByRole('button', { name: 'Confirm tuple update' }))

    const saving = await screen.findByRole('button', { name: 'Saving…' })
    await waitFor(() => expect(state.puts).toHaveLength(1))
    const form = saving.closest('form') as HTMLFormElement
    expect(form.getAttribute('aria-busy')).toBe('true')
    expect(saving.getAttribute('aria-disabled')).toBe('true')
    for (const label of ['Display name', 'Genesis hash', 'Chain ID', 'P2P network ID', 'Address HRP']) {
      expect(field(label).disabled).toBe(true)
    }
    expect((within(tuplePanel()).getByRole('button', { name: 'Cancel' }) as HTMLButtonElement).disabled).toBe(true)
    expect((screen.getByRole('button', { name: 'Keep editing' }) as HTMLButtonElement).disabled).toBe(true)
    fireEvent.click(saving)
    fireEvent.click(saving)
    fireEvent.submit(form)
    // The rest of the page stays usable.
    expect(screen.getByRole('link', { name: 'All Networks' })).toBeTruthy()
    await act(async () => {
      await Promise.resolve()
    })
    expect(state.puts).toHaveLength(1)

    await act(async () => {
      release()
    })
    expect(await screen.findByText('Updated Slow tuple.')).toBeTruthy()
    expect(state.puts).toHaveLength(1)
  })

  it('keeps the draft and the confirmation after a failed update so it can be retried', async () => {
    let fail = true
    const state = serve({
      put: (body) => {
        if (fail) {
          // The Server's own refusal: its status, code and complete envelope.
          return jsonResponse(
            { error: { code: 'unavailable', message: 'server database is unavailable', requestId: 'r-503', fields: [] } },
            503,
          )
        }
        state.detail = { ...state.detail, display_name: body.displayName }
        return jsonResponse({ networkKey: 'platon-e2e', displayName: body.displayName }, 200)
      },
    })
    await open()
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    fireEvent.change(field('Display name'), { target: { value: 'Retry tuple' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save tuple' }))
    fireEvent.click(screen.getByRole('button', { name: 'Confirm tuple update' }))

    expect(await within(tuplePanel()).findByText('server database is unavailable')).toBeTruthy()
    expect(screen.queryByText(/^Updated/)).toBeNull()
    expect(field('Display name').value).toBe('Retry tuple')
    expect(field('Display name').disabled).toBe(false)
    expect((within(tuplePanel()).getByRole('button', { name: 'Cancel' }) as HTMLButtonElement).disabled).toBe(false)
    const retry = screen.getByRole('button', { name: 'Confirm tuple update' })
    expect(retry.getAttribute('aria-disabled')).toBeNull()

    fail = false
    fireEvent.click(retry)
    expect(await screen.findByText('Updated Retry tuple.')).toBeTruthy()
    expect(within(tuplePanel()).queryByText('server database is unavailable')).toBeNull()
    expect(state.puts).toHaveLength(2)
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    expect(screen.queryByText('Updated Retry tuple.')).toBeNull()
    expect(field('Display name').value).toBe('Retry tuple')
  })

  function readBackNotice(): HTMLElement | null {
    return tuplePanel().querySelector('[data-slot="read-back-failed"]')
  }

  it('disables Save tuple, with the reason beside it, while the Server holds another tuple than the draft', async () => {
    const state = serve()
    await open()
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    fireEvent.change(field('Display name'), { target: { value: 'Draft B' } })
    state.detail = { ...state.detail, display_name: 'Server C' }
    await backgroundRefresh()
    await screen.findByRole('heading', { level: 1, name: /Server C/ })

    expect(field('Display name').value).toBe('Draft B')
    const save = within(tuplePanel()).getByRole('button', { name: 'Save tuple' }) as HTMLButtonElement
    expect(save.disabled).toBe(true)
    const reason = document.getElementById(save.getAttribute('aria-describedby') ?? '') as HTMLElement
    expect(tuplePanel().contains(reason)).toBe(true)
    expect(reason.textContent).toMatch(/changed on the Server while you were editing/)
    expect(reason.textContent).toMatch(/Save tuple is unavailable/)
    expect(reason.textContent).toContain('Server now: Server C')

    fireEvent.click(save)
    fireEvent.submit(field('Display name').closest('form') as HTMLFormElement)
    expect(screen.queryByRole('button', { name: 'Confirm tuple update' })).toBeNull()
    expect(state.puts).toHaveLength(0)
    // Cancel still starts again from the Server.
    fireEvent.click(within(tuplePanel()).getByRole('button', { name: 'Cancel' }))
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    expect(field('Display name').value).toBe('Server C')
    expect(within(tuplePanel()).getByRole('button', { name: 'Save tuple' })).not.toHaveProperty('disabled', true)
  })

  it('offers only Close editor once the Server holds the draft tuple, and sends nothing', async () => {
    const state = serve()
    await open()
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    fireEvent.change(field('Display name'), { target: { value: ' Landed tuple ' } })
    fireEvent.change(field('Chain ID'), { target: { value: '210426' } })
    // Compared as the update would be sent: trimmed text, numeric IDs.
    state.detail = { ...state.detail, display_name: 'Landed tuple', chain_id: 210426 }
    await backgroundRefresh()
    await screen.findByRole('heading', { level: 1, name: /Landed tuple/ })

    expect(within(tuplePanel()).getByText(/Nothing is left to save/)).toBeTruthy()
    expect(within(tuplePanel()).queryByRole('button', { name: 'Save tuple' })).toBeNull()
    expect(within(tuplePanel()).queryByRole('button', { name: 'Cancel' })).toBeNull()
    fireEvent.submit(field('Display name').closest('form') as HTMLFormElement)
    expect(screen.queryByRole('button', { name: 'Confirm tuple update' })).toBeNull()

    fireEvent.click(within(tuplePanel()).getByRole('button', { name: 'Close editor' }))
    expect(within(tuplePanel()).queryByRole('textbox')).toBeNull()
    expect(within(tuplePanel()).getByText('Landed tuple')).toBeTruthy()
    expect(within(tuplePanel()).getByText('210426')).toBeTruthy()
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Edit tuple' }))
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    expect(within(tuplePanel()).queryByText(/Nothing is left to save/)).toBeNull()
    expect(screen.queryByRole('button', { name: 'Close editor' })).toBeNull()
    expect(state.puts).toHaveLength(0)
  })

  it('states an applied update apart from a failed read-back, and lifts the notice once the Network is read again', async () => {
    const state = serve({
      put: (body) => {
        state.detail = { ...state.detail, display_name: body.displayName }
        state.unreadable = true
        return jsonResponse({ networkKey: 'platon-e2e', displayName: body.displayName }, 200)
      },
    })
    await open()
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    fireEvent.change(field('Display name'), { target: { value: 'Written tuple' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save tuple' }))
    fireEvent.click(screen.getByRole('button', { name: 'Confirm tuple update' }))

    const written = await within(tuplePanel()).findByText('Updated Written tuple.')
    expect(written.className).toContain('text-success')
    const notice = readBackNotice() as HTMLElement
    expect(notice.textContent).toMatch(/could not be read back/)
    expect(notice.closest('.text-success')).toBeNull()
    expect(notice.getAttribute('role')).toBeNull()
    // A warning beside the write's result, in the warning tone.
    expect(notice.className).toMatch(/border-warning/)
    expect(notice.querySelector('[data-slot="status-badge"]')?.getAttribute('data-tone')).toBe('warning')
    expect(screen.getAllByText(/Failed to refresh; showing the last/)).toHaveLength(1)
    // The read view stays the last Server tuple, and the editor is closed.
    expect(within(tuplePanel()).getByText('PlatON E2E Network')).toBeTruthy()
    expect(within(tuplePanel()).queryByRole('textbox')).toBeNull()
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Edit tuple' }))

    await backgroundRefresh()
    expect(readBackNotice()).not.toBeNull()

    // A new round begun meanwhile is not disturbed by the later recovery.
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    fireEvent.change(field('Address HRP'), { target: { value: 'atp' } })
    state.unreadable = false
    fireEvent.click(within(readBackNotice() as HTMLElement).getByRole('button', { name: 'Read the Network again' }))
    await screen.findByRole('heading', { level: 1, name: /Written tuple/ })
    await waitFor(() => expect(readBackNotice()).toBeNull())
    expect(field('Address HRP').value).toBe('atp')
    expect(screen.queryByText(/Failed to refresh; showing the last/)).toBeNull()
    expect(state.puts).toHaveLength(1)
  })

  it('keeps a draft begun before the read-back but sends nothing until the Network is read, then needs the latest tuple', async () => {
    const state = serve({
      put: (body) => {
        state.detail = { ...state.detail, display_name: body.displayName, chain_id: body.chainId }
        state.unreadable = true
        return jsonResponse({ networkKey: 'platon-e2e', displayName: body.displayName }, 200)
      },
    })
    await open()
    // First round: an identity field changes, the write lands, the read-back fails.
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    fireEvent.change(field('Chain ID'), { target: { value: '210426' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save tuple' }))
    fireEvent.click(screen.getByRole('button', { name: 'Confirm tuple update' }))
    expect(await within(tuplePanel()).findByText('Updated PlatON E2E Network.')).toBeTruthy()
    expect(readBackNotice()).not.toBeNull()
    await backgroundRefresh()
    expect(readBackNotice()).not.toBeNull()

    // A new round starts from the tuple this page last read: only the name changes.
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    expect(field('Chain ID').value).toBe('210425')
    fireEvent.change(field('Display name'), { target: { value: 'Renamed after write' } })
    const save = within(tuplePanel()).getByRole('button', { name: 'Save tuple' }) as HTMLButtonElement
    expect(save.disabled).toBe(true)
    const reason = document.getElementById(save.getAttribute('aria-describedby') ?? '') as HTMLElement
    expect(reason).toBeTruthy()
    expect(tuplePanel().contains(reason)).toBe(true)
    expect(reason.closest('form')).toBe(save.closest('form'))
    expect(reason.textContent).toMatch(/Save tuple is unavailable until the latest Network is read successfully/)
    expect(reason.textContent).toMatch(/Read the Network again/)

    fireEvent.click(save)
    // Enter in a field is the form's implicit submission (jsdom has none, so
    // it is dispatched directly; the Playwright case presses the real key).
    fireEvent.submit(field('Display name').closest('form') as HTMLFormElement)
    await act(async () => {
      await Promise.resolve()
    })
    expect(screen.queryByRole('button', { name: 'Confirm tuple update' })).toBeNull()
    expect(state.puts).toHaveLength(1)
    expect(field('Display name').value).toBe('Renamed after write')

    // The Network is read again (a read only): the draft stays, and the
    // existing Server-moved handling takes over.
    state.unreadable = false
    fireEvent.click(within(readBackNotice() as HTMLElement).getByRole('button', { name: 'Read the Network again' }))
    await waitFor(() => expect(readBackNotice()).toBeNull())
    expect(state.puts).toHaveLength(1)
    expect(field('Display name').value).toBe('Renamed after write')
    expect(field('Chain ID').value).toBe('210425')
    const moved = tuplePanel().querySelector('[data-slot="tuple-server-moved"]') as HTMLElement
    expect(moved.textContent).toMatch(/changed on the Server while you were editing/)
    expect(moved.textContent).toContain('Chain ID 210426')
    const blocked = within(tuplePanel()).getByRole('button', { name: 'Save tuple' }) as HTMLButtonElement
    expect(blocked.disabled).toBe(true)
    expect(blocked.getAttribute('aria-describedby')).toBe(moved.id)
    expect(tuplePanel().textContent).not.toMatch(/until the latest Network is read/)
    fireEvent.submit(field('Display name').closest('form') as HTMLFormElement)
    expect(screen.queryByRole('button', { name: 'Confirm tuple update' })).toBeNull()
    expect(state.puts).toHaveLength(1)

    // Starting again from the latest tuple keeps the first update's identity fields.
    fireEvent.click(within(tuplePanel()).getByRole('button', { name: 'Cancel' }))
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    expect(field('Chain ID').value).toBe('210426')
    fireEvent.change(field('Display name'), { target: { value: 'Renamed after write' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save tuple' }))
    fireEvent.click(screen.getByRole('button', { name: 'Confirm tuple update' }))
    expect(await within(tuplePanel()).findByText('Updated Renamed after write.')).toBeTruthy()
    expect(state.puts).toHaveLength(2)
    expect(state.puts[1]).toEqual({
      displayName: 'Renamed after write',
      genesisHash: NETWORK.genesis_hash,
      chainId: 210426,
      p2pNetworkId: 1,
      addressHrp: 'lat',
    })
  })

  it('still holds the save after leaving the Network and coming back while it cannot be read', async () => {
    const state = serve({
      put: (body) => {
        state.detail = { ...state.detail, display_name: body.displayName, chain_id: body.chainId }
        state.unreadable = true
        return jsonResponse({ networkKey: 'platon-e2e', displayName: body.displayName }, 200)
      },
    })
    await open()
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    fireEvent.change(field('Chain ID'), { target: { value: '210426' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save tuple' }))
    fireEvent.click(screen.getByRole('button', { name: 'Confirm tuple update' }))
    expect(await within(tuplePanel()).findByText('Updated PlatON E2E Network.')).toBeTruthy()
    expect(readBackNotice()).not.toBeNull()

    // Another Network, then back: the editor starts afresh, but this
    // Network's latest read still failed and the page shows the earlier tuple.
    await act(async () => {
      window.history.pushState({}, '', '/admin/networks/platon-other')
      window.dispatchEvent(new PopStateEvent('popstate'))
      await Promise.resolve()
    })
    await screen.findByRole('heading', { level: 1, name: /PlatON Other Network/ })
    await act(async () => {
      window.history.pushState({}, '', '/admin/networks/platon-e2e')
      window.dispatchEvent(new PopStateEvent('popstate'))
      await Promise.resolve()
    })
    await screen.findByRole('heading', { level: 1, name: /PlatON E2E Network/ })
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0))
    })

    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    expect(field('Chain ID').value).toBe('210425')
    fireEvent.change(field('Display name'), { target: { value: 'Renamed after return' } })
    const save = within(tuplePanel()).getByRole('button', { name: 'Save tuple' }) as HTMLButtonElement
    expect(save.disabled).toBe(true)
    const reason = document.getElementById(save.getAttribute('aria-describedby') ?? '') as HTMLElement
    expect(reason).toBeTruthy()
    expect(reason.closest('form')).toBe(save.closest('form'))
    expect(reason.textContent).toMatch(/Save tuple is unavailable until the latest Network is read successfully/)
    fireEvent.click(save)
    fireEvent.submit(field('Display name').closest('form') as HTMLFormElement)
    await act(async () => {
      await Promise.resolve()
    })
    expect(screen.queryByRole('button', { name: 'Confirm tuple update' })).toBeNull()
    expect(state.puts).toHaveLength(1)

    // Reading again only reads; then the Server-moved rule takes the kept draft.
    state.unreadable = false
    fireEvent.click(within(tuplePanel()).getByRole('button', { name: 'Read the Network again' }))
    await waitFor(() => expect(tuplePanel().querySelector('[data-slot="tuple-server-moved"]')).not.toBeNull())
    expect(tuplePanel().querySelector('[data-slot="tuple-read-back-pending"]')).toBeNull()
    expect(field('Display name').value).toBe('Renamed after return')
    expect((within(tuplePanel()).getByRole('button', { name: 'Save tuple' }) as HTMLButtonElement).disabled).toBe(true)
    expect(state.puts).toHaveLength(1)

    fireEvent.click(within(tuplePanel()).getByRole('button', { name: 'Cancel' }))
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    expect(field('Chain ID').value).toBe('210426')
  })

  it('holds the save while a refresh of the Network fails, and releases it once a read succeeds unchanged', async () => {
    const state = serve()
    await open()
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    fireEvent.change(field('Display name'), { target: { value: 'Draft during outage' } })
    state.unreadable = true
    await backgroundRefresh()
    await screen.findByText(/Failed to refresh; showing the last/)

    const save = within(tuplePanel()).getByRole('button', { name: 'Save tuple' }) as HTMLButtonElement
    expect(save.disabled).toBe(true)
    const reason = document.getElementById(save.getAttribute('aria-describedby') ?? '') as HTMLElement
    expect(reason.textContent).toMatch(/last read of this Network failed/)
    expect(reason.textContent).not.toMatch(/Read the Network again/)
    expect(readBackNotice()).toBeNull()
    fireEvent.submit(field('Display name').closest('form') as HTMLFormElement)
    expect(screen.queryByRole('button', { name: 'Confirm tuple update' })).toBeNull()

    state.unreadable = false
    const readAgain = within(tuplePanel()).getByRole('button', { name: 'Read the Network again' })
    readAgain.focus()
    fireEvent.click(readAgain)
    await waitFor(() => expect(tuplePanel().querySelector('[data-slot="tuple-read-back-pending"]')).toBeNull())
    // The Server did not move: the kept draft can be saved as it stands.
    expect(tuplePanel().querySelector('[data-slot="tuple-server-moved"]')).toBeNull()
    expect(field('Display name').value).toBe('Draft during outage')
    await waitFor(() => expect(document.activeElement).toBe(field('Display name')))
    const released = within(tuplePanel()).getByRole('button', { name: 'Save tuple' }) as HTMLButtonElement
    expect(released.disabled).toBe(false)
    expect(released.getAttribute('aria-describedby')).toBeNull()
    fireEvent.click(released)
    fireEvent.click(screen.getByRole('button', { name: 'Confirm tuple update' }))
    expect(await within(tuplePanel()).findByText('Updated Draft during outage.')).toBeTruthy()
    expect(state.puts).toHaveLength(1)
    expect(state.puts[0]).toMatchObject({ displayName: 'Draft during outage', chainId: 210425 })
  })

  async function go(path: string) {
    await act(async () => {
      window.history.pushState({}, '', path)
      window.dispatchEvent(new PopStateEvent('popstate'))
      await Promise.resolve()
    })
  }

  async function settle() {
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0))
    })
  }

  /** Leave for another Network and come back inside the app: the editor
   *  remounts from the cached (still fresh) detail of this Network. */
  async function leaveAndReturn() {
    await go('/admin/networks/platon-other')
    await screen.findByRole('heading', { level: 1, name: /PlatON Other Network/ })
    await go('/admin/networks/platon-e2e')
    await screen.findByText('platon-e2e', { selector: 'h1 span' })
    await screen.findByRole('heading', { level: 2, name: 'Expected identity tuple' })
    await settle()
  }

  function gate() {
    let release: () => void = () => {}
    const promise = new Promise<void>((resolve) => {
      release = resolve
    })
    return { promise, release: () => release() }
  }

  function applied(state: ReturnType<typeof serve>, body: Put) {
    state.detail = {
      ...state.detail,
      display_name: body.displayName,
      genesis_hash: body.genesisHash,
      chain_id: body.chainId,
      p2p_network_id: body.p2pNetworkId,
      address_hrp: body.addressHrp,
    }
  }

  function saveButton(): HTMLButtonElement {
    return within(tuplePanel()).getByRole('button', { name: 'Save tuple' }) as HTMLButtonElement
  }

  function saveReason(): HTMLElement {
    const save = saveButton()
    const ids = (save.getAttribute('aria-describedby') ?? '').split(' ').filter(Boolean)
    expect(ids.length).toBeGreaterThan(0)
    const reasons = ids.map((id) => document.getElementById(id) as HTMLElement)
    for (const reason of reasons) {
      expect(reason).toBeTruthy()
      // Beside the button, inside the same form.
      expect(reason.closest('form')).toBe(save.closest('form'))
    }
    const element = document.createElement('div')
    element.textContent = reasons.map((reason) => reason.textContent).join(' ')
    return element
  }

  /** Every way the form could be submitted: the Save click, Enter (the
   *  implicit submission jsdom dispatches as submit), and requestSubmit(). */
  async function trySubmitEveryWay() {
    fireEvent.click(saveButton())
    const form = field('Display name').closest('form') as HTMLFormElement
    fireEvent.submit(form)
    form.requestSubmit()
    await settle()
    expect(screen.queryByRole('button', { name: 'Confirm tuple update' })).toBeNull()
  }

  async function confirmChain(chainId: string) {
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    fireEvent.change(field('Chain ID'), { target: { value: chainId } })
    fireEvent.click(saveButton())
    fireEvent.click(screen.getByRole('button', { name: 'Confirm tuple update' }))
  }

  it('holds a second update while the first is unanswered, across leaving and reopening the Network', async () => {
    const answer = gate()
    const state = serve({
      put: async (body) => {
        await answer.promise
        applied(state, body)
        return jsonResponse({ networkKey: 'platon-e2e', displayName: body.displayName }, 200)
      },
    })
    await open()
    await confirmChain('210426')
    await screen.findByRole('button', { name: 'Saving…' })
    await waitFor(() => expect(state.puts).toHaveLength(1))

    await leaveAndReturn()
    // The new editor starts from the cached, fresh, error-free detail.
    expect(screen.queryByText(/Failed to refresh/)).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    expect(field('Chain ID').value).toBe('210425')
    fireEvent.change(field('Display name'), { target: { value: 'Renamed while saving' } })
    expect(field('Display name').disabled).toBe(false)
    expect(saveButton().disabled).toBe(true)
    expect(saveReason().textContent).toMatch(/still being saved/)
    await trySubmitEveryWay()
    expect(state.puts).toHaveLength(1)

    // The first update is answered and read back: the kept draft now meets
    // the existing Server-moved rule instead of being sent.
    await act(async () => {
      answer.release()
    })
    await waitFor(() => expect(tuplePanel().querySelector('[data-slot="tuple-server-moved"]')).not.toBeNull())
    expect(field('Display name').value).toBe('Renamed while saving')
    expect(saveButton().disabled).toBe(true)
    await trySubmitEveryWay()
    expect(state.puts).toHaveLength(1)

    fireEvent.click(within(tuplePanel()).getByRole('button', { name: 'Cancel' }))
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    expect(field('Chain ID').value).toBe('210426')
  })

  it('holds the save across reopening while the Server has applied the update but its answer is delayed', async () => {
    const answer = gate()
    const state = serve({
      put: async (body) => {
        // The Server applies the update at once; only the answer is late.
        applied(state, body)
        await answer.promise
        return jsonResponse({ networkKey: 'platon-e2e', displayName: body.displayName }, 200)
      },
    })
    await open()
    await confirmChain('210426')
    await waitFor(() => expect(state.puts).toHaveLength(1))

    await leaveAndReturn()
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    expect(field('Chain ID').value).toBe('210425')
    fireEvent.change(field('Display name'), { target: { value: 'Renamed meanwhile' } })
    expect(saveButton().disabled).toBe(true)
    expect(saveReason().textContent).toMatch(/still being saved/)
    await trySubmitEveryWay()

    // Even a page refresh that already shows the applied tuple does not
    // release it: only the answer and the read after it do.
    await backgroundRefresh()
    await screen.findByRole('heading', { level: 1, name: /PlatON E2E Network/ })
    expect(tuplePanel().querySelector('[data-slot="tuple-write-pending"]')).not.toBeNull()
    await trySubmitEveryWay()
    expect(state.puts).toHaveLength(1)

    await act(async () => {
      answer.release()
    })
    await waitFor(() => expect(tuplePanel().querySelector('[data-slot="tuple-write-pending"]')).toBeNull())
    expect(field('Display name').value).toBe('Renamed meanwhile')
    expect(tuplePanel().querySelector('[data-slot="tuple-server-moved"]')?.textContent).toContain('Chain ID 210426')
    expect(saveButton().disabled).toBe(true)
    expect(state.puts).toHaveLength(1)
  })

  it('treats a lost answer as unknown, checks by reading, and then starts from the applied tuple', async () => {
    const state = serve({
      put: (body) => {
        // Applied on the Server first; then the connection drops.
        applied(state, body)
        throw new TypeError('Failed to fetch')
      },
    })
    await open()
    await confirmChain('210426')

    // The one read-only check finds the Server holding the update.
    await waitFor(() => expect(within(tuplePanel()).getByText(/Nothing is left to save/)).toBeTruthy())
    expect(tuplePanel().querySelector('[data-slot="tuple-write-uncertain"]')).toBeNull()
    expect(within(tuplePanel()).queryByRole('alert')).toBeNull()
    expect(screen.queryByText(/^Updated/)).toBeNull()
    fireEvent.click(within(tuplePanel()).getByRole('button', { name: 'Close editor' }))
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    expect(field('Chain ID').value).toBe('210426')
    fireEvent.change(field('Display name'), { target: { value: 'Renamed after loss' } })
    fireEvent.click(saveButton())
    fireEvent.click(screen.getByRole('button', { name: 'Confirm tuple update' }))
    await waitFor(() => expect(state.puts).toHaveLength(2))
    expect(state.puts[1]).toMatchObject({ displayName: 'Renamed after loss', chainId: 210426 })
  })

  it('keeps a lost answer unknown while the Network cannot be read, across Cancel and reopening, then recovers by reading only', async () => {
    const state = serve({
      put: (body) => {
        applied(state, body)
        if (state.puts.length > 1) {
          return jsonResponse({ networkKey: 'platon-e2e', displayName: body.displayName }, 200)
        }
        state.unreadable = true
        throw new TypeError('Failed to fetch')
      },
    })
    await open()
    await confirmChain('210426')

    const notice = await waitFor(() => {
      const element = tuplePanel().querySelector('[data-slot="tuple-write-uncertain"]') as HTMLElement
      expect(element).toBeTruthy()
      return element
    })
    expect(notice.textContent).toMatch(/may or may\s+not have been applied/)
    expect(notice.textContent).toMatch(/not sent again/)
    await waitFor(() => expect(notice.textContent).toMatch(/could not be read/))
    // The lost answer is not reported as a failed save, and nothing is retried.
    expect(within(tuplePanel()).queryByText('Unable to update the Network')).toBeNull()
    expect(screen.queryByRole('button', { name: 'Confirm tuple update' })).toBeNull()
    expect(field('Chain ID').value).toBe('210426')
    expect(saveButton().disabled).toBe(true)
    await trySubmitEveryWay()
    expect(state.puts).toHaveLength(1)

    // Cancel and a new round from the page's tuple (from before the update).
    fireEvent.click(within(tuplePanel()).getByRole('button', { name: 'Cancel' }))
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    expect(field('Chain ID').value).toBe('210425')
    fireEvent.change(field('Display name'), { target: { value: 'Renamed while unknown' } })
    expect(saveButton().disabled).toBe(true)
    expect(saveReason().textContent).toMatch(/Server is seen holding the earlier update/)
    await trySubmitEveryWay()

    // Leaving and reopening keeps it unknown as well.
    await leaveAndReturn()
    expect(tuplePanel().querySelector('[data-slot="tuple-write-uncertain"]')).not.toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    fireEvent.change(field('Display name'), { target: { value: 'Renamed while unknown' } })
    await trySubmitEveryWay()
    expect(state.puts).toHaveLength(1)

    // Checking reads only; once the Server is seen holding the update, the
    // kept draft meets the Server-moved rule.
    state.unreadable = false
    const check = within(tuplePanel()).getByRole('button', { name: 'Check the Network' })
    check.focus()
    fireEvent.click(check)
    await waitFor(() => expect(tuplePanel().querySelector('[data-slot="tuple-write-uncertain"]')).toBeNull())
    expect(field('Display name').value).toBe('Renamed while unknown')
    const moved = tuplePanel().querySelector('[data-slot="tuple-server-moved"]') as HTMLElement
    expect(moved.textContent).toContain('Chain ID 210426')
    expect(saveButton().disabled).toBe(true)
    await waitFor(() => expect(document.activeElement).toBe(field('Display name')))
    expect(state.puts).toHaveLength(1)

    fireEvent.click(within(tuplePanel()).getByRole('button', { name: 'Cancel' }))
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    expect(field('Chain ID').value).toBe('210426')
    fireEvent.change(field('Display name'), { target: { value: 'Renamed while unknown' } })
    fireEvent.click(saveButton())
    fireEvent.click(screen.getByRole('button', { name: 'Confirm tuple update' }))
    expect(await within(tuplePanel()).findByText('Updated Renamed while unknown.')).toBeTruthy()
    expect(state.puts[1]).toMatchObject({ displayName: 'Renamed while unknown', chainId: 210426 })
  })

  it.each([
    ['504', 'Gateway timed out', () => jsonResponse({ error: { code: 'upstream_timeout', message: 'Gateway timed out' } }, 504)],
    ['502', 'Unable to update the Network', () => jsonResponse({ error: { code: 'bad_gateway' } }, 502)],
  ])(
    'keeps an update applied behind a JSON gateway %s unknown across Cancel, a new round and reopening, then recovers by reading only',
    async (_status, gatewayMessage, gatewayAnswer) => {
      const state = serve({
        put: (body) => {
          // The Server applies the update; a gateway in front of it answers
          // with its own JSON error, which carries an `error.code` too.
          applied(state, body)
          state.unreadable = true
          return gatewayAnswer()
        },
      })
      await open()
      await confirmChain('210426')

      const notice = await waitFor(() => {
        const element = tuplePanel().querySelector('[data-slot="tuple-write-uncertain"]') as HTMLElement
        expect(element).toBeTruthy()
        return element
      })
      expect(notice.textContent).toMatch(/may or may\s+not have been applied/)
      expect(notice.textContent).toMatch(/not sent again/)
      await waitFor(() => expect(notice.textContent).toMatch(/could not be read/))
      // Not reported as a refused save, and nothing is left to confirm again.
      expect(within(tuplePanel()).queryByText(gatewayMessage)).toBeNull()
      expect(screen.queryByRole('button', { name: 'Confirm tuple update' })).toBeNull()
      expect(field('Chain ID').value).toBe('210426')
      expect(saveButton().disabled).toBe(true)
      await trySubmitEveryWay()

      // Cancel, and a new round from the tuple this page last read.
      fireEvent.click(within(tuplePanel()).getByRole('button', { name: 'Cancel' }))
      fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
      expect(field('Chain ID').value).toBe('210425')
      fireEvent.change(field('Display name'), { target: { value: 'Renamed behind gateway' } })
      expect(saveButton().disabled).toBe(true)
      expect(saveReason().textContent).toMatch(/Server is seen holding the earlier update/)
      await trySubmitEveryWay()

      // Leaving and reopening keeps it unknown as well.
      await leaveAndReturn()
      expect(tuplePanel().querySelector('[data-slot="tuple-write-uncertain"]')).not.toBeNull()
      fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
      expect(field('Chain ID').value).toBe('210425')
      fireEvent.change(field('Display name'), { target: { value: 'Renamed behind gateway' } })
      expect(saveButton().disabled).toBe(true)
      await trySubmitEveryWay()
      expect(state.puts).toHaveLength(1)

      // Checking reads only; the Server is seen holding the update, and the
      // kept draft meets the Server-moved rule instead of being sent.
      state.unreadable = false
      fireEvent.click(within(tuplePanel()).getByRole('button', { name: 'Check the Network' }))
      await waitFor(() => expect(tuplePanel().querySelector('[data-slot="tuple-write-uncertain"]')).toBeNull())
      expect(field('Display name').value).toBe('Renamed behind gateway')
      expect(tuplePanel().querySelector('[data-slot="tuple-server-moved"]')?.textContent).toContain('Chain ID 210426')
      expect(saveButton().disabled).toBe(true)
      await trySubmitEveryWay()
      expect(state.writes).toEqual(['PUT ' + NETWORK_URL])
    },
  )

  it.each([
    ['an unknown code in a complete envelope', 'locked', () =>
      jsonResponse({ error: { code: 'tuple_locked', message: 'locked', requestId: 'r9', fields: [] } }, 409)],
    ['a known code in an incomplete envelope', 'Service Unavailable', () =>
      jsonResponse({ error: { code: 'unavailable', message: 'Service Unavailable' } }, 503)],
  ])('keeps %s unknown rather than refused, and sends nothing more', async (_name, answerMessage, answer) => {
    const state = serve({ put: () => answer() })
    await open()
    await confirmChain('210426')
    await waitFor(() => expect(tuplePanel().querySelector('[data-slot="tuple-write-uncertain"]')).not.toBeNull())
    expect(within(tuplePanel()).queryByText(answerMessage)).toBeNull()
    expect(screen.queryByRole('button', { name: 'Confirm tuple update' })).toBeNull()
    await trySubmitEveryWay()
    expect(state.writes).toEqual(['PUT ' + NETWORK_URL])
  })

  it('does not take a read of the earlier tuple as proof that a lost update never applied', async () => {
    // The update never reaches the Server here; the browser cannot tell.
    const state = serve({ put: () => Promise.reject(new TypeError('Failed to fetch')) })
    await open()
    await confirmChain('210426')
    const notice = await waitFor(() => {
      const element = tuplePanel().querySelector('[data-slot="tuple-write-uncertain"]') as HTMLElement
      expect(element.textContent).toMatch(/does not show this update/)
      return element
    })
    expect(notice.textContent).toMatch(/does not prove it was not applied/)
    fireEvent.click(within(notice).getByRole('button', { name: 'Check the Network' }))
    await settle()
    await settle()
    expect(tuplePanel().querySelector('[data-slot="tuple-write-uncertain"]')).not.toBeNull()
    expect(saveButton().disabled).toBe(true)
    await trySubmitEveryWay()
    expect(state.puts).toHaveLength(1)
  })

  it('never takes a read begun before the update was answered for its read-back', async () => {
    const answer = gate()
    const lateRead = gate()
    const state = serve({
      put: async (body) => {
        await answer.promise
        applied(state, body)
        return jsonResponse({ networkKey: 'platon-e2e', displayName: body.displayName }, 200)
      },
    })
    await open()
    await confirmChain('210426')
    await waitFor(() => expect(state.puts).toHaveLength(1))
    // While the update is unanswered, a read of the earlier tuple starts; it
    // is answered only after the update.
    state.readGate = lateRead.promise
    await act(async () => {
      void adminQueryClient.invalidateQueries()
    })
    await settle()

    // The update is applied and answered; every read after it fails.
    state.readGate = null
    state.unreadable = true
    await act(async () => {
      answer.release()
    })
    expect(await within(tuplePanel()).findByText('Updated PlatON E2E Network.')).toBeTruthy()
    // The earlier read now answers, with the tuple from before the update.
    await act(async () => {
      lateRead.release()
    })
    await settle()
    expect(readBackNotice()).not.toBeNull()
    expect(within(tuplePanel()).getByText('210425')).toBeTruthy()

    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    expect(field('Chain ID').value).toBe('210425')
    fireEvent.change(field('Display name'), { target: { value: 'Renamed after late read' } })
    expect(saveButton().disabled).toBe(true)
    expect(saveReason().textContent).toMatch(/until the latest Network is read successfully/)
    await trySubmitEveryWay()
    // Nor across reopening.
    await leaveAndReturn()
    expect(readBackNotice()).not.toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    fireEvent.change(field('Display name'), { target: { value: 'Renamed after late read' } })
    expect(saveButton().disabled).toBe(true)
    await trySubmitEveryWay()
    expect(state.puts).toHaveLength(1)

    // A read after the update releases it; the next round starts from it.
    state.unreadable = false
    fireEvent.click(within(readBackNotice() as HTMLElement).getByRole('button', { name: 'Read the Network again' }))
    await waitFor(() => expect(readBackNotice()).toBeNull())
    fireEvent.click(within(tuplePanel()).getByRole('button', { name: 'Cancel' }))
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    expect(field('Chain ID').value).toBe('210426')
  })

  it('returns a refused update to an editable, re-confirmable draft, and holds an answer that is not the Server’s', async () => {
    let answer: 'refuse' | 'proxy' | 'apply' = 'refuse'
    const state = serve({
      put: (body) => {
        if (answer === 'refuse') {
          return jsonResponse(
            { error: { code: 'invalid_network_tuple', message: 'the Network identity tuple is invalid', requestId: 'r1', fields: [] } },
            400,
          )
        }
        if (answer === 'proxy') {
          // A gateway page: not the Server's answer, so the outcome is unknown.
          return new Response('<html>502 Bad Gateway</html>', { status: 502, headers: { 'Content-Type': 'text/html' } })
        }
        applied(state, body)
        return jsonResponse({ networkKey: 'platon-e2e', displayName: body.displayName }, 200)
      },
    })
    await open()
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    fireEvent.change(field('Display name'), { target: { value: 'Refused draft' } })
    fireEvent.click(saveButton())
    fireEvent.click(screen.getByRole('button', { name: 'Confirm tuple update' }))
    expect(await within(tuplePanel()).findByText('the Network identity tuple is invalid')).toBeTruthy()
    expect(field('Display name').value).toBe('Refused draft')
    expect(field('Display name').disabled).toBe(false)
    expect(tuplePanel().querySelector('[data-slot^="tuple-write"]')).toBeNull()
    const again = screen.getByRole('button', { name: 'Confirm tuple update' })
    expect(again.getAttribute('aria-disabled')).toBeNull()

    // Not locked for good: it can be changed and confirmed again.
    fireEvent.change(field('Display name'), { target: { value: 'Second draft' } })
    expect(saveButton().disabled).toBe(false)
    answer = 'proxy'
    fireEvent.click(saveButton())
    fireEvent.click(screen.getByRole('button', { name: 'Confirm tuple update' }))
    await waitFor(() => expect(tuplePanel().querySelector('[data-slot="tuple-write-uncertain"]')).not.toBeNull())
    expect(state.puts).toHaveLength(2)
    expect(field('Display name').value).toBe('Second draft')
    expect(saveButton().disabled).toBe(true)
    await trySubmitEveryWay()
    expect(state.puts).toHaveLength(2)
  })

  it('does not hold another Network, and a late answer leaves its state, notices and focus alone', async () => {
    const answer = gate()
    const state = serve({
      put: async (body) => {
        await answer.promise
        applied(state, body)
        state.unreadable = true
        return jsonResponse({ networkKey: 'platon-e2e', displayName: body.displayName }, 200)
      },
    })
    await open()
    await confirmChain('210426')
    await waitFor(() => expect(state.puts).toHaveLength(1))

    await go('/admin/networks/platon-other')
    await screen.findByRole('heading', { level: 1, name: /PlatON Other Network/ })
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    fireEvent.change(field('Display name'), { target: { value: 'Other renamed' } })
    expect(saveButton().disabled).toBe(false)
    expect(saveButton().getAttribute('aria-describedby')).toBeNull()
    fireEvent.click(saveButton())
    fireEvent.click(screen.getByRole('button', { name: 'Confirm tuple update' }))
    expect(await within(tuplePanel()).findByText('Updated Other renamed.')).toBeTruthy()
    expect(state.otherPuts).toHaveLength(1)
    const edit = screen.getByRole('button', { name: 'Edit tuple' })
    expect(document.activeElement).toBe(edit)

    await act(async () => {
      answer.release()
    })
    await settle()
    expect(within(tuplePanel()).getByText('Updated Other renamed.')).toBeTruthy()
    expect(readBackNotice()).toBeNull()
    expect(tuplePanel().querySelector('[data-slot^="tuple-write"]')).toBeNull()
    expect(document.activeElement).toBe(edit)
    fireEvent.click(edit)
    expect(saveButton().disabled).toBe(false)

    // Network A keeps its own read-back hold.
    await go('/admin/networks/platon-e2e')
    await screen.findByRole('heading', { level: 1, name: /PlatON E2E Network/ })
    expect(readBackNotice()).not.toBeNull()
  })

  it('ends with the authorization: an earlier answer neither pollutes nor releases a write of the new one', async () => {
    const answers = [gate(), gate()]
    const state = serve({
      put: async (body) => {
        const index = state.puts.length - 1
        await answers[index].promise
        applied(state, body)
        return jsonResponse({ networkKey: 'platon-e2e', displayName: body.displayName }, 200)
      },
    })
    await open()
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    fireEvent.change(field('Display name'), { target: { value: 'Old session tuple' } })
    fireEvent.click(saveButton())
    fireEvent.click(screen.getByRole('button', { name: 'Confirm tuple update' }))
    await waitFor(() => expect(state.puts).toHaveLength(1))

    // The session is re-established: a new access generation.
    state.authExpired = true
    await act(async () => {
      void adminQueryClient.invalidateQueries()
    })
    await waitFor(() => expect(screen.queryByRole('button', { name: 'Saving…' })).toBeNull())
    await screen.findByRole('heading', { level: 1, name: /PlatON E2E Network/ })
    await settle()
    expect(tuplePanel().querySelector('[data-slot^="tuple-write"]')).toBeNull()

    // A write of the new generation.
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    fireEvent.change(field('Display name'), { target: { value: 'New session tuple' } })
    expect(saveButton().disabled).toBe(false)
    fireEvent.click(saveButton())
    fireEvent.click(screen.getByRole('button', { name: 'Confirm tuple update' }))
    await waitFor(() => expect(state.puts).toHaveLength(2))
    await screen.findByRole('button', { name: 'Saving…' })

    // The earlier generation's answer arrives: it changes nothing here.
    await act(async () => {
      answers[0].release()
    })
    await settle()
    expect(screen.getByRole('button', { name: 'Saving…' })).toBeTruthy()
    expect(screen.queryByText('Updated Old session tuple.')).toBeNull()
    await leaveAndReturn()
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    fireEvent.change(field('P2P network ID'), { target: { value: '5' } })
    expect(saveReason().textContent).toMatch(/still being saved/)
    await trySubmitEveryWay()
    expect(state.puts).toHaveLength(2)

    await act(async () => {
      answers[1].release()
    })
    await waitFor(() => expect(tuplePanel().querySelector('[data-slot="tuple-write-pending"]')).toBeNull())
  })

  it('carries no feedback of an update that lands after switching to another Network', async () => {
    let release: () => void = () => {}
    serve({
      put: async (body) => {
        await new Promise<void>((resolve) => {
          release = resolve
        })
        return jsonResponse({ networkKey: 'platon-e2e', displayName: body.displayName }, 200)
      },
    })
    await open()
    fireEvent.click(screen.getByRole('button', { name: 'Edit tuple' }))
    fireEvent.change(field('Display name'), { target: { value: 'Late tuple' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save tuple' }))
    fireEvent.click(screen.getByRole('button', { name: 'Confirm tuple update' }))
    await screen.findByRole('button', { name: 'Saving…' })
    await act(async () => {
      window.history.pushState({}, '', '/admin/networks/platon-other')
      window.dispatchEvent(new PopStateEvent('popstate'))
      await Promise.resolve()
    })
    await screen.findByRole('heading', { level: 1, name: /PlatON Other Network/ })
    ;(document.activeElement as HTMLElement | null)?.blur()
    await act(async () => {
      release()
    })
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0))
    })
    expect(screen.queryByText(/Updated Late tuple/)).toBeNull()
    expect(readBackNotice()).toBeNull()
    expect(within(tuplePanel()).queryByRole('textbox')).toBeNull()
    expect(document.activeElement).not.toBe(screen.getByRole('button', { name: 'Edit tuple' }))
  })
})
