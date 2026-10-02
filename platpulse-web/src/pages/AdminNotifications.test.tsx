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

const TEST_ORIGIN = 'http://platpulse.test'
// The Server only ever sends this redacted summary; the raw chat id and the
// token file contents must never reach the browser (issue #206).
const MASKED_DESTINATION = '****1234'
const BOT_TOKEN = '7712345678:AAF-only-on-the-operator-file'

const EVENT = {
  eventId: 'event-1',
  eventKind: 'test',
  incidentId: null,
  ruleKey: null,
  subjectKind: null,
  subjectKey: null,
  severity: 'info',
  summary: 'Owner test notification',
  createdAt: '2026-03-01T00:00:00Z',
}

const INCIDENT_EVENT = {
  eventId: 'event-2',
  eventKind: 'incident',
  incidentId: 'i-1',
  ruleKey: 'node.rpc_unreachable',
  subjectKind: 'node',
  subjectKey: 'node-a',
  severity: 'critical',
  summary: 'Node a is unreachable',
  createdAt: '2026-02-28T23:00:00Z',
}

const DELIVERY = {
  deliveryId: 'd-1',
  eventId: 'event-1',
  channelKind: 'telegram',
  destination: MASKED_DESTINATION,
  state: 'failed',
  attemptCount: 1,
  nextAttemptAt: null,
  lastAttemptAt: '2026-03-01T00:00:05Z',
  lastResult: 'telegram_api_error 401',
  lastErrorKind: 'api',
  retryAfterSeconds: null,
  createdAt: '2026-03-01T00:00:00Z',
  updatedAt: '2026-03-01T00:00:05Z',
}

const SUCCEEDED_DELIVERY = {
  ...DELIVERY,
  deliveryId: 'd-2',
  state: 'succeeded',
  attemptCount: 2,
  lastResult: 'telegram_ok',
}

const ATTEMPT = {
  attemptId: 'a-1',
  deliveryId: 'd-1',
  attemptNumber: 1,
  attemptedAt: '2026-03-01T00:00:05Z',
  outcome: 'failed',
  providerResult: 'telegram_api_error 401',
  errorKind: 'api',
  durationMs: 120,
  retryAfterSeconds: null,
}

const CHANNEL = {
  channelId: 'telegram',
  channelKind: 'telegram',
  enabled: true,
  destination: MASKED_DESTINATION,
  providerRef: 'telegram-token',
  maxAttempts: 5,
  retryBaseSeconds: 60,
}

const REQUEST_RESULT = {
  requestId: 'req-1',
  commandKind: 'test',
  eventId: 'event-1',
  delivery: DELIVERY,
  auditEventId: 41,
  createdAt: '2026-03-01T00:00:00Z',
  expiresAt: '2026-03-02T00:00:00Z',
}

function jsonResponse(body: unknown, status: number): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

function errorResponse(code: string, message: string, status: number): Response {
  return jsonResponse({ error: { code, message } }, status)
}

type Recorded = { url: string; method: string; body: string; csrf: string | null }

function mockFetch(
  routes: Record<string, (request: Request) => Response | Promise<Response>>,
  recorded: Recorded[] = [],
) {
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(String(input), init)
    const url = request.url.replace(TEST_ORIGIN, '')
    const clone = request.clone()
    const body = await clone.text().catch(() => '')
    recorded.push({
      url,
      method: request.method,
      body,
      csrf: request.headers.get('X-CSRF-Token'),
    })
    for (const [pattern, handler] of Object.entries(routes)) {
      if (pattern.endsWith('*')) {
        if (url.startsWith(pattern.slice(0, -1))) return handler(request)
      } else if (url === pattern) {
        return handler(request)
      }
    }
    return errorResponse('not_found', 'no route', 404)
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

const SESSION_ROUTE = {
  '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
}

const CHANNEL_ROUTE = {
  '/api/admin/v1/notifications/channels': () => jsonResponse([CHANNEL], 200),
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

describe('PAGE-ADMIN-NOTIFICATIONS (issue #206 notification surface)', () => {
  it('renders the overview with the request-dedup disclaimer and a redacted channel', async () => {
    mockFetch({ ...SESSION_ROUTE, ...CHANNEL_ROUTE })
    await renderAt('/admin/notifications')

    await screen.findByRole('heading', { level: 1, name: 'Notifications' })
    // The distinction between Server request dedup and an external delivery
    // guarantee is stated, not implied.
    const disclaimer = screen.getByText(/Request dedup is not an end-to-end delivery guarantee/i)
    expect(disclaimer).toBeTruthy()
    expect(document.body.textContent).toContain('exactly-once')
    expect(await screen.findByText(MASKED_DESTINATION)).toBeTruthy()
    expect(document.body.textContent).not.toContain(BOT_TOKEN)
    // Every routed section is reachable from the overview.
    for (const href of [
      '/admin/notifications/events',
      '/admin/notifications/deliveries',
      '/admin/notifications/channels',
    ]) {
      expect(screen.getAllByRole('link').some((link) => link.getAttribute('href') === href)).toBe(true)
    }
  })

  it('lists events with their suppression state and per-channel delivery summaries', async () => {
    mockFetch({
      ...SESSION_ROUTE,
      '/api/admin/v1/notifications/events*': (request) =>
        new URL(request.url).searchParams.get('event_kind') === 'incident'
          ? jsonResponse({ items: [{ ...INCIDENT_EVENT, deliveries: [] }], nextBefore: null }, 200)
          : jsonResponse(
              {
                items: [
                  { ...EVENT, deliveries: [{ deliveryId: 'd-1', channelKind: 'telegram', destination: MASKED_DESTINATION, state: 'failed', attemptCount: 1 }] },
                  { ...INCIDENT_EVENT, deliveries: [] },
                ],
                nextBefore: 'event-2',
              },
              200,
            ),
    })
    await renderAt('/admin/notifications/events')

    await screen.findByRole('heading', { level: 1, name: 'Notification Events' })
    const table = await screen.findByRole('table', { name: 'Notification events' })
    expect(table.getAttribute('data-stack')).not.toBeNull()
    expect(table.textContent).toContain('Owner test notification')
    expect(table.textContent).toContain('node · node-a')
    expect(table.textContent).toContain('failed')
    expect(table.textContent).toContain('None')

    // The kind filter narrows the Server query instead of filtering locally.
    fireEvent.change(screen.getByLabelText('Event kind'), { target: { value: 'incident' } })
    await screen.findByText('Node a is unreachable')
    await waitFor(() => {
      expect(screen.queryByText('Owner test notification')).toBeNull()
    })
  })

  it('names the parent page group when an event id is unknown instead of rendering another event', async () => {
    mockFetch({
      ...SESSION_ROUTE,
      '/api/admin/v1/notifications/events/*': () =>
        errorResponse('notification_event_not_found', 'no such event', 404),
    })
    await renderAt('/admin/notifications/events/no-such-event')

    await screen.findByRole('heading', { level: 1, name: 'Notification Event not found' })
    expect(screen.queryByText('Owner test notification')).toBeNull()
  })

  it('renders the event detail with its deliveries and the Audit association', async () => {
    mockFetch({
      ...SESSION_ROUTE,
      '/api/admin/v1/notifications/events/event-1': () =>
        jsonResponse({ ...EVENT, deliveries: [DELIVERY] }, 200),
    })
    await renderAt('/admin/notifications/events/event-1')

    await screen.findByRole('heading', { level: 1, name: 'Notification Event event-1' })
    const table = await screen.findByRole('table', { name: 'Deliveries for this notification event' })
    expect(table.textContent).toContain('telegram')
    expect(table.textContent).toContain(MASKED_DESTINATION)
    expect(table.textContent).toContain('telegram_api_error 401')
  })

  it('lists deliveries and preserves the provider result and attempt count', async () => {
    mockFetch({
      ...SESSION_ROUTE,
      '/api/admin/v1/notifications/deliveries*': (request) =>
        new URL(request.url).searchParams.get('state') === 'succeeded'
          ? jsonResponse({ items: [SUCCEEDED_DELIVERY], nextBefore: null }, 200)
          : jsonResponse({ items: [DELIVERY], nextBefore: 'd-1' }, 200),
    })
    await renderAt('/admin/notifications/deliveries')

    await screen.findByRole('heading', { level: 1, name: 'Notification Deliveries' })
    const table = await screen.findByRole('table', { name: 'Notification deliveries' })
    expect(table.textContent).toContain('telegram_api_error 401')
    expect(table.textContent).toContain('None')

    fireEvent.change(screen.getByLabelText('State'), { target: { value: 'succeeded' } })
    await screen.findByText('telegram_ok')
  })

  it('queues one retry keyed by a single request id and reports the recorded Server result', async () => {
    const recorded: Recorded[] = []
    let deduplicated = false
    mockFetch(
      {
        ...SESSION_ROUTE,
        '/api/admin/v1/notifications/deliveries/d-1/retry': () => {
          deduplicated = true
          return jsonResponse({ ...DELIVERY, deduplicated, requestId: 'req-x', auditEventId: 7 }, 200)
        },
        '/api/admin/v1/notifications/deliveries/d-1': () =>
          jsonResponse({ ...DELIVERY, attempts: [ATTEMPT], event: EVENT }, 200),
      },
      recorded,
    )
    await renderAt('/admin/notifications/deliveries/d-1')

    await screen.findByRole('heading', { level: 1, name: 'Notification Delivery d-1' })
    const attempts = await screen.findByRole('table', { name: 'Delivery attempts' })
    expect(attempts.textContent).toContain('telegram_api_error 401')
    expect(attempts.textContent).toContain('api')

    fireEvent.click(screen.getByRole('button', { name: 'Queue retry' }))
    await waitFor(() => {
      expect(recorded.filter((entry) => entry.method === 'POST')).toHaveLength(1)
    })
    const post = recorded.find((entry) => entry.method === 'POST')
    const body = JSON.parse(post?.body ?? '{}') as { requestId?: string }
    // The Server command identity is the browser-generated opaque request id,
    // and the mutation carries the CSRF token like every other Admin write.
    expect(body.requestId).toMatch(/[0-9a-f-]{36}/)
    expect(post?.csrf).toBe('csrf-token')
    await screen.findByText(/already recorded this request id/i)
  })

  it('treats a retry whose response was lost as indeterminate and never re-sends it', async () => {
    const recorded: Recorded[] = []
    mockFetch(
      {
        ...SESSION_ROUTE,
        '/api/admin/v1/notifications/deliveries/d-1/retry': () => {
          throw new Error('socket closed')
        },
        '/api/admin/v1/notifications/deliveries/d-1': () =>
          jsonResponse({ ...DELIVERY, attempts: [ATTEMPT], event: EVENT }, 200),
        '/api/admin/v1/notifications/requests*': () => jsonResponse(REQUEST_RESULT, 200),
      },
      recorded,
    )
    await renderAt('/admin/notifications/deliveries/d-1')

    await screen.findByRole('heading', { level: 1, name: 'Notification Delivery d-1' })
    fireEvent.click(screen.getByRole('button', { name: 'Queue retry' }))

    await screen.findByText(/outcome is unknown/i)
    // The reconciliation panel is pre-filled with the same request id, so the
    // Owner reads the recorded result instead of issuing a second command.
    expect(await screen.findByText('Reconcile the uncertain retry')).toBeTruthy()
    await screen.findByText('req-1')
    expect(recorded.filter((entry) => entry.method === 'POST')).toHaveLength(1)
  })

  it('asks for a new request id when the Server refuses a conflicting intent', async () => {
    const recorded: Recorded[] = []
    mockFetch(
      {
        ...SESSION_ROUTE,
        '/api/admin/v1/notifications/deliveries/d-1/retry': () =>
          errorResponse(
            'request_id_conflict',
            'this request id is recorded for a different command',
            409,
          ),
        '/api/admin/v1/notifications/deliveries/d-1': () =>
          jsonResponse({ ...DELIVERY, attempts: [ATTEMPT], event: EVENT }, 200),
      },
      recorded,
    )
    await renderAt('/admin/notifications/deliveries/d-1')

    await screen.findByRole('heading', { level: 1, name: 'Notification Delivery d-1' })
    fireEvent.click(screen.getByRole('button', { name: 'Queue retry' }))

    await screen.findByText(/different command.*Start a new request id/is)
    // A refusal is final: the browser must not silently retry under a new id.
    expect(recorded.filter((entry) => entry.method === 'POST')).toHaveLength(1)
  })

  it('sends one controlled test with a request id and shows the redacted Delivery outcome', async () => {
    const recorded: Recorded[] = []
    mockFetch(
      {
        ...SESSION_ROUTE,
        ...CHANNEL_ROUTE,
        '/api/admin/v1/notifications/channels/telegram/test': () =>
          jsonResponse(
            {
              ...DELIVERY,
              eventId: 'event-1',
              requestId: 'req-1',
              auditEventId: 41,
              deduplicated: false,
            },
            200,
          ),
      },
      recorded,
    )
    await renderAt('/admin/notifications/channels')

    await screen.findByRole('heading', { level: 1, name: 'Notification Channels' })
    const channels = await screen.findByRole('table', { name: 'Notification channels' })
    expect(channels.textContent).toContain(MASKED_DESTINATION)
    expect(document.body.textContent).not.toContain(BOT_TOKEN)

    fireEvent.click(screen.getByRole('button', { name: 'Send test notification' }))
    const outcome = await screen.findByText('Deduplicated')
    expect(outcome).toBeTruthy()
    await waitFor(() => {
      expect(recorded.filter((entry) => entry.method === 'POST')).toHaveLength(1)
    })
    const post = recorded.find((entry) => entry.method === 'POST')
    expect(post?.csrf).toBe('csrf-token')
    expect((JSON.parse(post?.body ?? '{}') as { requestId?: string }).requestId).toMatch(
      /[0-9a-f-]{36}/,
    )
    // The provider outcome is reported, and it is never presented as an HTTP
    // failure: the Server accepted the command and recorded the attempt.
    const outcomeCard = screen.getByText('Deduplicated').closest('[data-slot="notification-test-outcome"]')
    expect(outcomeCard?.textContent).toContain('failed')
  })

  it('surfaces the Server test cooldown as a typed retry-after instruction', async () => {
    const recorded: Recorded[] = []
    mockFetch(
      {
        ...SESSION_ROUTE,
        ...CHANNEL_ROUTE,
        '/api/admin/v1/notifications/channels/telegram/test': () =>
          errorResponse(
            'test_cooldown_active',
            'a test notification was sent recently; retry after 12 seconds',
            429,
          ),
      },
      recorded,
    )
    await renderAt('/admin/notifications/channels')

    await screen.findByRole('heading', { level: 1, name: 'Notification Channels' })
    fireEvent.click(screen.getByRole('button', { name: 'Send test notification' }))

    await screen.findByText(/retry after 12 seconds.*cooldown/is)
    expect(recorded.filter((entry) => entry.method === 'POST')).toHaveLength(1)
  })

  it('reconciles an uncertain test by looking up the original request id', async () => {
    const recorded: Recorded[] = []
    mockFetch(
      {
        ...SESSION_ROUTE,
        ...CHANNEL_ROUTE,
        '/api/admin/v1/notifications/channels/telegram/test': () => {
          throw new Error('gateway timeout')
        },
        '/api/admin/v1/notifications/requests*': () => jsonResponse(REQUEST_RESULT, 200),
      },
      recorded,
    )
    await renderAt('/admin/notifications/channels')

    await screen.findByRole('heading', { level: 1, name: 'Notification Channels' })
    fireEvent.click(screen.getByRole('button', { name: 'Send test notification' }))

    await screen.findByText(/outcome is unknown/i)
    await screen.findByText('Reconcile the uncertain test')
    await screen.findByText('req-1')
    // Exactly one command was ever issued; reconciliation is a read.
    expect(recorded.filter((entry) => entry.method === 'POST')).toHaveLength(1)
    expect(recorded.some((entry) => entry.method === 'GET' && entry.url.includes('/requests/'))).toBe(
      true,
    )
  })

  it('explains an unknown or expired request id without claiming a send happened', async () => {
    mockFetch({
      ...SESSION_ROUTE,
      ...CHANNEL_ROUTE,
      '/api/admin/v1/notifications/requests*': () =>
        errorResponse('notification_request_not_found', 'no such request', 404),
    })
    await renderAt('/admin/notifications/channels')

    await screen.findByRole('heading', { level: 1, name: 'Notification Channels' })
    fireEvent.change(screen.getByLabelText('Request id'), { target: { value: 'req-gone' } })
    fireEvent.click(screen.getByRole('button', { name: 'Look up request' }))

    await screen.findByText(/no unexpired record for this request id/i)
  })

  it('does not offer a retry for a terminal suppressed delivery', async () => {
    mockFetch({
      ...SESSION_ROUTE,
      '/api/admin/v1/notifications/deliveries/d-9': () =>
        jsonResponse(
          { ...DELIVERY, deliveryId: 'd-9', state: 'suppressed' , attempts: [], event: EVENT },
          200,
        ),
    })
    await renderAt('/admin/notifications/deliveries/d-9')

    await screen.findByRole('heading', { level: 1, name: 'Notification Delivery d-9' })
    const retry = await screen.findByRole('button', { name: 'Queue retry' })
    expect((retry as HTMLButtonElement).disabled).toBe(true)
    expect(screen.getByText(/suppressed is not a retryable state/i)).toBeTruthy()
  })

  it('keeps the notification pages Owner-only', async () => {
    mockFetch({
      '/api/public/v1/session': () =>
        jsonResponse({ ...OWNER_SESSION, session: { ...OWNER_SESSION.session, role: 'viewer' } }, 200),
    })
    await renderAt('/admin/notifications/channels')

    await screen.findByRole('heading', { level: 1, name: 'Owner access required' })
    expect(screen.queryByRole('heading', { level: 1, name: 'Notification Channels' })).toBeNull()
  })
})

describe('PAGE-ADMIN-NOTIFICATIONS navigation', () => {
  it('marks the single Notifications nav entry current on every notification route', async () => {
    mockFetch({ ...SESSION_ROUTE, ...CHANNEL_ROUTE })
    await renderAt('/admin/notifications/channels')

    await screen.findByRole('heading', { level: 1, name: 'Notification Channels' })
    const adminNav = screen.getByRole('navigation', { name: 'Admin' })
    const link = within(adminNav).getByRole('link', { name: 'Notifications' })
    expect(link.getAttribute('href')).toBe('/admin/notifications')
    expect(link.getAttribute('aria-current')).toBe('page')
    const sections = screen.getByRole('navigation', { name: 'Notification sections' })
    expect(within(sections).getAllByRole('link')).toHaveLength(4)
  })
})
