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

  function serve(options: { put?: (body: Put) => Response | Promise<Response> } = {}) {
    const state = { detail: { ...NETWORK_DETAIL } as typeof NETWORK_DETAIL, puts: [] as Put[] }
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(String(input), init)
      const url = request.url.replace(TEST_ORIGIN, '')
      if (url === '/api/public/v1/session') return jsonResponse(OWNER_SESSION, 200)
      if (url === NETWORK_URL && request.method === 'GET') return jsonResponse(state.detail, 200)
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
          return jsonResponse({ error: { code: 'unavailable', message: 'server database is unavailable' } }, 503)
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
})
