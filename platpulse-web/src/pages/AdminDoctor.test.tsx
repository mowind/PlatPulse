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

const LAST_RUN_ID = '0195f2a1-0300-4100-8100-000000000300'
const QUEUED_RUN_ID = '0195f2a1-0301-4101-8101-000000000301'

/** The last Doctor run the Server recorded a report for. */
const LAST_RUN = {
  operationId: LAST_RUN_ID,
  kind: 'doctor_run',
  status: 'succeeded_with_warnings',
  progressPercent: 0,
  progressLabel: null,
  requestId: 'req-1',
  createdAt: '2026-03-01T00:00:00Z',
  startedAt: '2026-03-01T00:00:01Z',
  finishedAt: '2026-03-01T00:00:06Z',
  auditEventId: 31,
  cancelRequested: false,
}

/** A run the Server accepted but has not finished yet. */
const QUEUED_RUN = {
  ...LAST_RUN,
  operationId: QUEUED_RUN_ID,
  status: 'queued',
  createdAt: '2026-03-01T02:00:00Z',
  startedAt: null,
  finishedAt: null,
  auditEventId: 32,
}

const CHECKS = [
  {
    checkId: 'retention.last_run',
    label: 'Retention ran recently',
    status: 'warning',
    detail: 'The last retention run finished with warnings.',
  },
  {
    checkId: 'storage.free_space',
    label: 'Database has free space',
    status: 'pass',
    detail: 'Free space is above the warning threshold.',
  },
]

function jsonResponse(body: unknown, status: number): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

function apiError(code: string, message: string, status: number): Response {
  return jsonResponse({ error: { code, message, requestId: 'req-err' } }, status)
}

const TEST_ORIGIN = 'http://platpulse.test'

function mockFetch(
  routes: Record<string, (request: Request) => Response | Promise<Response>>,
) {
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(String(input), init)
    const url = request.url.replace(TEST_ORIGIN, '')
    for (const [pattern, handler] of Object.entries(routes)) {
      if (pattern.endsWith('*')) {
        if (url.startsWith(pattern.slice(0, -1))) return handler(request)
      } else if (url === pattern) {
        return handler(request)
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
  vi.useRealTimers()
  vi.unstubAllGlobals()
  adminQueryClient.clear()
})

describe('PAGE-ADMIN-DOCTOR (diagnostics)', () => {
  it('shows the last recorded report and every recorded check', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/doctor': () =>
        jsonResponse({ lastRun: LAST_RUN, currentRun: null, checks: CHECKS }, 200),
    })
    await renderAt('/admin/doctor')

    await screen.findByRole('heading', { level: 1, name: 'Doctor' })
    const table = await screen.findByRole('table', { name: /Checks from the last recorded Doctor report/ })
    expect(table.textContent).toContain('Retention ran recently')
    expect(table.textContent).toContain('retention.last_run')
    expect(table.textContent).toContain('Warning')
    expect(table.textContent).toContain('Pass')
    expect(table.textContent).toContain('The last retention run finished with warnings.')
    expect(screen.getByText('Succeeded with warnings')).toBeTruthy()
    expect(screen.getByText('Report age')).toBeTruthy()
    expect(
      screen.getAllByRole('link', { name: /0195f2a1/ })[0].getAttribute('href'),
    ).toBe('/admin/operations/' + LAST_RUN_ID)
  })

  it('keeps a queued run visible beside the previous report after a refresh', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/doctor': () =>
        jsonResponse({ lastRun: LAST_RUN, currentRun: QUEUED_RUN, checks: CHECKS }, 200),
    })
    await renderAt('/admin/doctor')

    await screen.findByRole('heading', { level: 1, name: 'Doctor' })
    // The in-flight run is a real state after a reload, not a finished one.
    expect(screen.getByText('Run in flight')).toBeTruthy()
    expect(screen.getByText(/This run is Queued:/)).toBeTruthy()
    const inFlightLink = screen
      .getAllByRole('link', { name: /0195f2a1/ })
      .find((link) => link.getAttribute('href') === '/admin/operations/' + QUEUED_RUN_ID)
    expect(inFlightLink).toBeTruthy()
    // The report below is named as the previous one until the run finishes.
    expect(screen.getAllByText(/still the previous one/).length).toBeGreaterThan(0)
    expect(
      screen.getByText(/A newer run is in flight and will replace it/),
    ).toBeTruthy()
  })

  it('shows a running run in flight and links to its task', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/doctor': () =>
        jsonResponse(
          {
            lastRun: LAST_RUN,
            currentRun: { ...QUEUED_RUN, status: 'running', startedAt: '2026-03-01T02:00:02Z' },
            checks: CHECKS,
          },
          200,
        ),
    })
    await renderAt('/admin/doctor')

    await screen.findByRole('heading', { level: 1, name: 'Doctor' })
    expect(screen.getByText(/This run is Running\./)).toBeTruthy()
    expect(screen.getByText('Running')).toBeTruthy()
  })

  it('follows a run from queued to running to its finished report without a reload', async () => {
    let phase: 'queued' | 'running' | 'done' = 'queued'
    let doctorCalls = 0
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/doctor': () => {
        doctorCalls += 1
        if (phase === 'running') {
          return jsonResponse(
            {
              lastRun: LAST_RUN,
              currentRun: { ...QUEUED_RUN, status: 'running', startedAt: '2026-03-01T02:00:02Z' },
              checks: CHECKS,
            },
            200,
          )
        }
        if (phase === 'done') {
          return jsonResponse(
            {
              lastRun: { ...QUEUED_RUN, status: 'succeeded', finishedAt: '2026-03-01T02:00:09Z' },
              currentRun: null,
              checks: CHECKS,
            },
            200,
          )
        }
        return jsonResponse({ lastRun: LAST_RUN, currentRun: QUEUED_RUN, checks: CHECKS }, 200)
      },
    })
    vi.useFakeTimers({ shouldAdvanceTime: true })
    await renderAt('/admin/doctor')
    await screen.findByText(/This run is Queued:/)
    expect(doctorCalls).toBe(1)

    // Operation progress events invalidate the Operations ledger, not the
    // Doctor report, so the page has to poll while a run is in flight.
    phase = 'running'
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2200)
    })
    await screen.findByText(/This run is Running\./)
    expect(doctorCalls).toBeGreaterThan(1)

    // Once the run is terminal the page settles on the finished report and
    // stops asking for more.
    phase = 'done'
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2200)
    })
    await waitFor(() => expect(screen.queryByText('Run in flight')).toBeNull())
    const callsAtTerminal = doctorCalls
    await act(async () => {
      await vi.advanceTimersByTimeAsync(8000)
    })
    expect(doctorCalls).toBe(callsAtTerminal)
  })
  it('queues a run with one read-only command and no confirmation', async () => {
    const fetchMock = mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/doctor': (request) => {
        if (request.method === 'POST') {
          return jsonResponse(
            { auditEventId: 33, operation: { operation: QUEUED_RUN, warnings: [], errors: [], result: null, cancellable: true } },
            200,
          )
        }
        return jsonResponse({ lastRun: LAST_RUN, currentRun: null, checks: CHECKS }, 200)
      },
    })
    await renderAt('/admin/doctor')
    await screen.findByRole('heading', { level: 1, name: 'Doctor' })

    fireEvent.click(screen.getByRole('button', { name: 'Run Doctor' }))

    // One click, one request: a read-only diagnostic has nothing to confirm.
    await screen.findByText(new RegExp('The Server queued a Doctor run as ' + QUEUED_RUN_ID))
    expect(screen.queryByRole('dialog')).toBeNull()
    const post = fetchMock.mock.calls
      .map((call) => call[0] as Request)
      .filter((request) => request.url.endsWith('/api/admin/v1/doctor') && request.method === 'POST')
    expect(post).toHaveLength(1)
    expect(post[0]?.headers.get('X-CSRF-Token')).toBe('csrf-token')
  })

  it('never offers a repair, delete, or credential control', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/doctor': () =>
        jsonResponse({ lastRun: LAST_RUN, currentRun: null, checks: CHECKS }, 200),
    })
    await renderAt('/admin/doctor')
    await screen.findByRole('heading', { level: 1, name: 'Doctor' })

    expect(screen.getByText(/It never repairs anything/)).toBeTruthy()
    for (const name of ['Repair', 'Fix', 'Delete', 'Rotate credential']) {
      expect(screen.queryByRole('button', { name: new RegExp(name, 'i') })).toBeNull()
    }
  })

  it('names an unknown outcome when the run request may not have reached the Server', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/doctor': (request) => {
        if (request.method === 'POST') throw new TypeError('Failed to fetch')
        return jsonResponse({ lastRun: LAST_RUN, currentRun: null, checks: CHECKS }, 200)
      },
    })
    await renderAt('/admin/doctor')
    await screen.findByRole('heading', { level: 1, name: 'Doctor' })

    fireEvent.click(screen.getByRole('button', { name: 'Run Doctor' }))
    await screen.findByText(/The request may not have reached the Server/)
  })

  it('shows the empty report state before the first run', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/doctor': () => jsonResponse({ lastRun: null, currentRun: null, checks: [] }, 200),
    })
    await renderAt('/admin/doctor')

    await screen.findByRole('heading', { level: 1, name: 'Doctor' })
    expect(screen.getByText(/Doctor has not produced a report on this Server yet/)).toBeTruthy()
    expect(screen.getByText(/No checks are recorded in the last report/)).toBeTruthy()
  })

  it('reads an unrecognised check status as Unknown, not as a pass', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/doctor': () =>
        jsonResponse(
          {
            lastRun: LAST_RUN,
            currentRun: null,
            checks: [{ checkId: 'future.check', label: 'A newer check', status: 'brand_new_state', detail: 'From a newer Server.' }],
          },
          200,
        ),
    })
    await renderAt('/admin/doctor')

    await screen.findByRole('heading', { level: 1, name: 'Doctor' })
    expect(screen.getByText('Unknown')).toBeTruthy()
    expect(screen.queryByText('Pass')).toBeNull()
  })

  it('offers a retry when the report cannot be loaded', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/doctor': () => apiError('database_unavailable', 'Server database is unavailable', 503),
    })
    await renderAt('/admin/doctor')

    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toContain('Server database is unavailable')
    expect(screen.getByRole('button', { name: 'Try again' })).toBeTruthy()
  })
})
