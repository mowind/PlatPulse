import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import App from '../App'
import { adminQueryClient } from '../api/admin'
import { client } from '../api/generated/client.gen'
import { publicQueryClient } from '../api/public'

const SESSION = {
  session: {
    userId: 'owner-1',
    username: 'owner',
    role: 'owner',
    createdAt: '2026-08-20T00:00:00Z',
    lastSeenAt: '2026-08-20T00:00:00Z',
    expiresAt: '2026-08-27T00:00:00Z',
  },
  csrfToken: 'csrf-test',
}

const TEST_ORIGIN = 'http://platpulse.test'

function response(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

function mockFetch(routes: Record<string, (request: Request) => Response | Promise<Response>>) {
  vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(String(input), init)
    const path = request.url.replace(TEST_ORIGIN, '').split('?')[0]
    const handler = routes[path]
    return handler ? handler(request) : response({ error: { code: 'not_found' } }, 404)
  }))
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
  publicQueryClient.clear()
})

describe('Admin configuration workflows', () => {
  it('previews and saves the bounded global History Window through the dedicated seam', async () => {
    let windowDays = 7
    mockFetch({
      '/api/public/v1/session': () => response(SESSION),
      '/api/admin/v1/access-mode': () => response({ mode: 'private', authorizationGeneration: 0 }),
      '/api/admin/v1/history-window': async (request) => {
        if (request.method === 'PUT') {
          windowDays = 14
          return response({ window: { windowDays, defaultDays: 7, minDays: 1, maxDays: 30, updatedAt: '2026-08-20T01:00:00Z', updatedBy: 'owner-1' }, auditEventId: 41 })
        }
        return response({ windowDays, defaultDays: 7, minDays: 1, maxDays: 30, updatedAt: '2026-08-20T00:00:00Z', updatedBy: 'defaults' })
      },
      '/api/admin/v1/history-window/impact': () => response({ windowDays: 14, estimatedRows: 0, minDays: 1, maxDays: 30, notes: [] }),
    })
    await renderAt('/admin/history-window')
    await screen.findByRole('heading', { level: 1, name: 'History Window' })
    fireEvent.change(screen.getByLabelText('New window (days)'), { target: { value: '14' } })
    await screen.findByText('0 rows')
    fireEvent.change(screen.getByLabelText('Type the change to confirm'), { target: { value: 'history-window 14' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save History Window' }))
    expect(await screen.findByText(/Audit #41/)).toBeTruthy()
  })

  it('shows Site Access as a separate audited Owner workflow', async () => {
    let mode = 'private'
    mockFetch({
      '/api/public/v1/session': () => response(SESSION),
      '/api/admin/v1/access-mode': async (request) => {
        if (request.method === 'PUT') mode = 'public'
        return response({ mode, authorizationGeneration: mode === 'public' ? 1 : 0 })
      },
    })
    await renderAt('/admin/site-access')
    await screen.findByRole('heading', { level: 1, name: 'Site Access' })
    publicQueryClient.setQueryData(['public', 'networks', 0], { stale: true })
    vi.spyOn(window, 'confirm').mockReturnValue(true)
    fireEvent.click(screen.getByRole('button', { name: 'Make Home Public' }))
    expect(await screen.findByText(/now Public/)).toBeTruthy()
    expect(publicQueryClient.getQueryData(['public', 'networks', 0])).toBeUndefined()
  })
})
