import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
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

const HISTORY_WINDOW = {
  windowDays: 7,
  defaultDays: 14,
  minDays: 1,
  maxDays: 30,
  updatedAt: '2026-08-20T00:00:00Z',
  updatedBy: 'owner-1',
}

const TEST_ORIGIN = 'http://platpulse.test'

type RouteHandler = (request: Request) => Response | Promise<Response>

function response(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

function apiError(message: string, fields: string[] = [], status = 422): Response {
  return response({ error: { code: 'invalid_request', message, requestId: 'request-1', fields } }, status)
}

function mockFetch(routes: Record<string, RouteHandler>) {
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(String(input), init)
    const path = request.url.replace(TEST_ORIGIN, '').split('?')[0]
    const handler = routes[path]
    return handler ? handler(request) : response({ error: { code: 'not_found' } }, 404)
  })
  vi.stubGlobal('fetch', fetchMock)
  return fetchMock
}

function successfulRoutes(overrides: Record<string, RouteHandler> = {}) {
  return {
    '/api/public/v1/session': () => response(SESSION),
    '/api/admin/v1/history-window': () => response(HISTORY_WINDOW),
    '/api/admin/v1/history-window/impact': async (request: Request) => {
      const body = await request.json() as { windowDays: number }
      return response({
        windowDays: body.windowDays,
        estimatedRows: body.windowDays < HISTORY_WINDOW.windowDays ? 12 : 0,
        minDays: HISTORY_WINDOW.minDays,
        maxDays: HISTORY_WINDOW.maxDays,
        notes: [],
      })
    },
    '/api/admin/v1/access-mode': () => response({ mode: 'private', authorizationGeneration: 0 }),
    ...overrides,
  }
}

async function renderSettings() {
  render(<App />)
  await act(async () => {
    window.history.pushState({}, '', '/admin/settings')
    window.dispatchEvent(new PopStateEvent('popstate'))
    await Promise.resolve()
  })
  await screen.findByRole('heading', { level: 1, name: 'Settings' })
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

describe('Admin Settings workflows', () => {
  it('renders one Settings page with ordered independent configuration cards', async () => {
    mockFetch(successfulRoutes())

    await renderSettings()

    expect(screen.getAllByRole('heading', { level: 1 })).toHaveLength(1)
    const cards = screen.getAllByRole('article')
    expect(cards).toHaveLength(2)
    expect(within(cards[0]).getByRole('heading', { level: 2, name: 'History Window' })).toBeTruthy()
    expect(within(cards[1]).getByRole('heading', { level: 2, name: 'Site Access Mode' })).toBeTruthy()

    const historyCard = within(cards[0])
    expect(historyCard.getByText('7 days')).toBeTruthy()
    expect(historyCard.getByText('14 days')).toBeTruthy()
    expect(historyCard.getByText('1 day')).toBeTruthy()
    expect(historyCard.getByText('30 days')).toBeTruthy()
    expect(historyCard.getByText(/2026-08-20T00:00:00Z by owner-1/)).toBeTruthy()
    expect(historyCard.getByText(/Shortening removes expired history asynchronously/)).toBeTruthy()
    expect(historyCard.getByText(/Lengthening cannot recover deleted or missed history/)).toBeTruthy()

    const accessCard = within(cards[1])
    expect(accessCard.getByText('Private')).toBeTruthy()
    expect(accessCard.getByText(/Public permits anonymous Home reads/)).toBeTruthy()
    expect(accessCard.getByText(/Private requires Owner login/)).toBeTruthy()
  })


  it.each([
    {
      failedPath: '/api/admin/v1/history-window',
      errorText: 'Unable to load the History Window.',
      otherControl: 'Make Home Public',
    },
    {
      failedPath: '/api/admin/v1/access-mode',
      errorText: 'Unable to load Site Access Mode.',
      otherControl: 'Save History Window',
    },
  ])('keeps the other Settings card usable when $failedPath fails', async ({ failedPath, errorText, otherControl }) => {
    mockFetch(successfulRoutes({
      [failedPath]: () => apiError('Configuration unavailable', [], 503),
    }))

    await renderSettings()

    expect(await screen.findByText(errorText)).toBeTruthy()
    expect(screen.getByRole('button', { name: otherControl })).toBeTruthy()
  })

  it('requires the authoritative impact preview and typed confirmation before saving History Window', async () => {
    let putBody: unknown = null
    let accessReads = 0
    let historyWindow = HISTORY_WINDOW
    mockFetch(successfulRoutes({
      '/api/admin/v1/access-mode': () => {
        accessReads += 1
        return response({ mode: 'private', authorizationGeneration: 0 })
      },
      '/api/admin/v1/history-window': async (request) => {
        if (request.method === 'PUT') {
          putBody = await request.json()
          historyWindow = {
            ...HISTORY_WINDOW,
            windowDays: 14,
            updatedAt: '2026-08-20T01:00:00Z',
          }
          return response({ window: historyWindow, auditEventId: 41 })
        }
        return response(historyWindow)
      },
    }))

    await renderSettings()
    const save = screen.getByRole('button', { name: 'Save History Window' })
    expect((save as HTMLButtonElement).disabled).toBe(true)

    fireEvent.change(screen.getByLabelText('New window (days)'), { target: { value: '14' } })
    fireEvent.change(screen.getByLabelText('Type the change to confirm'), {
      target: { value: 'history-window 14' },
    })
    expect((save as HTMLButtonElement).disabled).toBe(true)
    await screen.findByText('0 rows')
    await waitFor(() => expect((save as HTMLButtonElement).disabled).toBe(false))

    fireEvent.click(save)
    expect(await screen.findByText(/Audit #41/)).toBeTruthy()
    expect(putBody).toEqual({ windowDays: 14, confirmed: true })
    expect(accessReads).toBe(1)
  })

  it('rejects blank, non-integer, and out-of-bounds History Window values without requests or clamping', async () => {
    let putCalls = 0
    let impactCalls = 0
    mockFetch(successfulRoutes({
      '/api/admin/v1/history-window': (request) => {
        if (request.method === 'PUT') putCalls += 1
        return response(HISTORY_WINDOW)
      },
      '/api/admin/v1/history-window/impact': async (request) => {
        impactCalls += 1
        const body = await request.json() as { windowDays: number }
        return response({ windowDays: body.windowDays, estimatedRows: 0, minDays: 1, maxDays: 30, notes: [] })
      },
    }))

    await renderSettings()
    const days = screen.getByLabelText('New window (days)')
    const confirmation = screen.getByLabelText('Type the change to confirm')
    const save = screen.getByRole('button', { name: 'Save History Window' })
    await screen.findByText('0 rows')
    expect(impactCalls).toBe(1)

    fireEvent.change(days, { target: { value: '' } })
    expect(screen.getByRole('alert').textContent).toContain('Enter a number of days')
    expect((days as HTMLInputElement).value).toBe('')
    expect(impactCalls).toBe(1)

    fireEvent.change(days, { target: { value: '1.5' } })
    expect(screen.getByRole('alert').textContent).toContain('whole number')
    fireEvent.change(confirmation, { target: { value: 'history-window 1.5' } })
    expect((save as HTMLButtonElement).disabled).toBe(true)

    fireEvent.change(days, { target: { value: '31' } })
    expect(screen.getByRole('alert').textContent).toContain('between 1 and 30 days')
    fireEvent.change(confirmation, { target: { value: 'history-window 31' } })
    fireEvent.click(save)
    expect((save as HTMLButtonElement).disabled).toBe(true)
    expect(putCalls).toBe(0)
    expect(impactCalls).toBe(1)
  })

  it('keeps History Window disabled when its impact preview fails', async () => {
    mockFetch(successfulRoutes({
      '/api/admin/v1/history-window/impact': async (request) => {
        const body = await request.json() as { windowDays: number }
        return body.windowDays === 14
          ? apiError('Impact service unavailable', [], 503)
          : response({ windowDays: body.windowDays, estimatedRows: 0, minDays: 1, maxDays: 30, notes: [] })
      },
    }))

    await renderSettings()
    fireEvent.change(screen.getByLabelText('New window (days)'), { target: { value: '14' } })
    fireEvent.change(screen.getByLabelText('Type the change to confirm'), {
      target: { value: 'history-window 14' },
    })

    expect(await screen.findByText(/Unable to preview the impact/)).toBeTruthy()
    expect((screen.getByRole('button', { name: 'Save History Window' }) as HTMLButtonElement).disabled).toBe(true)
    expect(screen.getByText('Private')).toBeTruthy()
  })

  it('cancels Site Access Mode without mutation and clears Public cache after confirmation', async () => {
    let putCalls = 0
    let historyReads = 0
    mockFetch(successfulRoutes({
      '/api/admin/v1/history-window': () => {
        historyReads += 1
        return response(HISTORY_WINDOW)
      },
      '/api/admin/v1/access-mode': async (request) => {
        if (request.method === 'PUT') {
          putCalls += 1
          return response({ mode: 'public', authorizationGeneration: 1 })
        }
        return response({ mode: 'private', authorizationGeneration: 0 })
      },
    }))

    await renderSettings()
    const toggle = screen.getByRole('button', { name: 'Make Home Public' })
    const confirm = vi.spyOn(window, 'confirm').mockReturnValueOnce(false).mockReturnValueOnce(true)

    fireEvent.click(toggle)
    expect(putCalls).toBe(0)
    expect(confirm).toHaveBeenCalledWith(expect.stringContaining('Anonymous visitors'))

    publicQueryClient.setQueryData(['public', 'networks', 0], { stale: true })
    fireEvent.click(toggle)
    expect(await screen.findByText(/Site Access Mode is now Public/)).toBeTruthy()
    expect(putCalls).toBe(1)
    expect(historyReads).toBe(1)
    expect(publicQueryClient.getQueryData(['public', 'networks', 0])).toBeUndefined()
  })

  it('shows a History Window page error without changing Site Access state', async () => {
    mockFetch(successfulRoutes({
      '/api/admin/v1/history-window': (request) => request.method === 'PUT'
        ? apiError('History Window save unavailable', [], 500)
        : response(HISTORY_WINDOW),
    }))

    await renderSettings()
    fireEvent.change(screen.getByLabelText('New window (days)'), { target: { value: '14' } })
    fireEvent.change(screen.getByLabelText('Type the change to confirm'), {
      target: { value: 'history-window 14' },
    })
    await screen.findByText('0 rows')
    await waitFor(() => expect(
      (screen.getByRole('button', { name: 'Save History Window' }) as HTMLButtonElement).disabled,
    ).toBe(false))
    fireEvent.click(screen.getByRole('button', { name: 'Save History Window' }))

    expect(await screen.findByText('History Window save unavailable')).toBeTruthy()
    expect(screen.getByText('Private')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Make Home Public' })).toBeTruthy()
  })

  it('reports each mutation error in its own card without hiding the other workflow', async () => {
    mockFetch(successfulRoutes({
      '/api/admin/v1/history-window': (request) => request.method === 'PUT'
        ? apiError('History Window rejected by Server', ['windowDays'])
        : response(HISTORY_WINDOW),
      '/api/admin/v1/access-mode': (request) => request.method === 'PUT'
        ? apiError('Site Access transition failed', [], 500)
        : response({ mode: 'private', authorizationGeneration: 0 }),
    }))

    await renderSettings()
    fireEvent.change(screen.getByLabelText('New window (days)'), { target: { value: '14' } })
    fireEvent.change(screen.getByLabelText('Type the change to confirm'), {
      target: { value: 'history-window 14' },
    })
    await screen.findByText('0 rows')
    await waitFor(() => expect((screen.getByRole('button', { name: 'Save History Window' }) as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(screen.getByRole('button', { name: 'Save History Window' }))
    expect(await screen.findByText('History Window rejected by Server')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Make Home Public' })).toBeTruthy()

    vi.spyOn(window, 'confirm').mockReturnValue(true)
    fireEvent.click(screen.getByRole('button', { name: 'Make Home Public' }))
    expect(await screen.findByText('Site Access transition failed')).toBeTruthy()
    expect((screen.getByLabelText('New window (days)') as HTMLInputElement).value).toBe('14')
    expect(screen.getByRole('heading', { level: 2, name: 'History Window' })).toBeTruthy()
  })
})
