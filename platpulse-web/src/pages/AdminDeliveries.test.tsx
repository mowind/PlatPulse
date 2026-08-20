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

const EVENT = {
  eventId: 'event-0001',
  eventKind: 'incident',
  incidentId: 'incident-1',
  ruleKey: 'node.rpc_unreachable',
  subjectKind: 'node',
  subjectKey: 'node-a',
  severity: 'warning',
  summary: 'Incident opened: node.rpc_unreachable on node node-a',
  createdAt: '2026-08-12T08:01:00Z',
}

const DELIVERY = {
  deliveryId: 'delivery-0001',
  eventId: 'event-0001',
  channelKind: 'telegram',
  destination: '****6789',
  state: 'retry_scheduled',
  attemptCount: 1,
  nextAttemptAt: '2026-08-12T08:06:00Z',
  lastAttemptAt: '2026-08-12T08:01:30Z',
  lastResult: 'telegram_api_error 429',
  lastErrorKind: 'telegram_api',
  retryAfterSeconds: 5,
  createdAt: '2026-08-12T08:01:00Z',
  updatedAt: '2026-08-12T08:01:30Z',
}

const ATTEMPT = {
  attemptId: 'attempt-1',
  deliveryId: 'delivery-0001',
  attemptNumber: 1,
  attemptedAt: '2026-08-12T08:01:30Z',
  outcome: 'failed',
  providerResult: 'telegram_api_error 429',
  errorKind: 'telegram_api',
  durationMs: 120,
  retryAfterSeconds: 5,
}

const CHANNEL = {
  channelId: 'telegram',
  channelKind: 'telegram',
  enabled: true,
  destination: '****6789',
  providerRef: 'telegram-token',
  maxAttempts: 5,
  retryBaseSeconds: 60,
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
    if (url === '/api/public/v1/access') {
      return jsonResponse({ mode: 'private', authorizationGeneration: 0 }, 200)
    }
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

describe('PAGE-ADMIN-DELIVERIES (Outbox + Events)', () => {
  it('lists Deliveries with per-channel state and redacted destinations', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/notifications/deliveries*': () =>
        jsonResponse({ items: [DELIVERY], next_before: null }, 200),
      '/api/admin/v1/notifications/events*': () =>
        jsonResponse({ items: [{ ...EVENT, deliveries: [DELIVERY] }], next_before: null }, 200),
    })
    renderAt('/admin/alerts/deliveries')

    await screen.findByRole('heading', { level: 1, name: 'Deliveries' })
    const row = await screen.findByRole('row', { name: /telegram/ })
    expect(row.textContent).toContain('deliver')
    expect(row.textContent).toContain('****6789')
    expect(row.textContent).toContain('Retry scheduled')
    expect(row.textContent).toContain('1')
    // The full destination and any token-like value never reach the page.
    expect(document.body.textContent).not.toContain('123456789')
    expect(document.body.textContent).not.toContain('fake-token')
    // Events are distinguishable from Delivery attempts.
    const eventItem = await screen.findByText(/Incident opened: node\.rpc_unreachable/)
    expect(eventItem).toBeTruthy()
  })

  it('filters the Outbox by state and shows the Dead letter filter', async () => {
    let requested: URL | null = null
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(String(input), init)
      requested = new URL(request.url)
      const url = request.url.replace(TEST_ORIGIN, '')
      if (url === '/api/public/v1/session') return jsonResponse(OWNER_SESSION, 200)
      if (url.startsWith('/api/admin/v1/notifications/deliveries')) {
        return jsonResponse(
          { items: [{ ...DELIVERY, state: 'dead_letter', attemptCount: 5 }], next_before: null },
          200,
        )
      }
      if (url.startsWith('/api/admin/v1/notifications/events')) {
        return jsonResponse({ items: [], next_before: null }, 200)
      }
      return jsonResponse({ error: { code: 'not_found' } }, 404)
    })
    vi.stubGlobal('fetch', fetchMock)
    renderAt('/admin/alerts/deliveries')

    await screen.findByRole('heading', { level: 1, name: 'Deliveries' })
    fireEvent.change(await screen.findByLabelText('State'), { target: { value: 'dead_letter' } })
    await waitFor(() => {
      expect(requested?.searchParams.get('state')).toBe('dead_letter')
    })
    expect((await screen.findAllByText('Dead letter')).length).toBeGreaterThan(0)
  })

  it('shows Empty when the Outbox has no rows in the selected state', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/notifications/deliveries*': () =>
        jsonResponse({ items: [], next_before: null }, 200),
      '/api/admin/v1/notifications/events*': () =>
        jsonResponse({ items: [], next_before: null }, 200),
    })
    renderAt('/admin/alerts/deliveries')

    expect(await screen.findByText(/No Deliveries/)).toBeTruthy()
    expect(screen.getByText('No Notification Events yet.')).toBeTruthy()
  })
})

describe('PAGE-ADMIN-DELIVERY (attempts, retry, redaction)', () => {
  it('shows attempt history, Retry-After, and redacted destination', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/notifications/deliveries/delivery-0001': () =>
        jsonResponse({ ...DELIVERY, attempts: [ATTEMPT], event: EVENT }, 200),
    })
    renderAt('/admin/alerts/deliveries/delivery-0001')

    await screen.findByRole('heading', { level: 1, name: /^Delivery deliver/ })
    expect(await screen.findByText('****6789')).toBeTruthy()
    expect((await screen.findAllByText('telegram_api_error 429')).length).toBeGreaterThan(0)
    expect(await screen.findByText('5 s (provider)')).toBeTruthy()
    expect(await screen.findByRole('button', { name: 'Retry delivery' })).toBeTruthy()
    expect(document.body.textContent).toContain('never a new Notification Event')
    expect(document.body.textContent).not.toContain('123456789')
    // The Event is shown separately from the Delivery state.
    expect(await screen.findByText(/Incident opened: node\.rpc_unreachable/)).toBeTruthy()
  })

  it('retries through the mutation and refetches authoritative state', async () => {
    let retried = false
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(String(input), init)
      const url = request.url.replace(TEST_ORIGIN, '')
      if (url === '/api/public/v1/session') return jsonResponse(OWNER_SESSION, 200)
      if (
        url === '/api/admin/v1/notifications/deliveries/delivery-0001/retry' &&
        request.method === 'POST'
      ) {
        retried = true
        return jsonResponse({ ...DELIVERY, state: 'pending' }, 200)
      }
      if (url === '/api/admin/v1/notifications/deliveries/delivery-0001') {
        return jsonResponse(
          { ...DELIVERY, state: retried ? 'pending' : 'dead_letter', attempts: [ATTEMPT], event: EVENT },
          200,
        )
      }
      return jsonResponse({ error: { code: 'not_found' } }, 404)
    })
    vi.stubGlobal('fetch', fetchMock)
    renderAt('/admin/alerts/deliveries/delivery-0001')

    await screen.findByRole('heading', { level: 1, name: /^Delivery deliver/ })
    await screen.findByText('Dead letter')
    vi.spyOn(window, 'confirm').mockReturnValue(true)
    fireEvent.click(await screen.findByRole('button', { name: 'Retry delivery' }))
    await waitFor(() => expect(retried).toBe(true))
    expect(await screen.findByText(/Retry queued/)).toBeTruthy()
  })

  it('surfaces the Server refusal for duplicate parallel retries', async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(String(input), init)
      const url = request.url.replace(TEST_ORIGIN, '')
      if (url === '/api/public/v1/session') return jsonResponse(OWNER_SESSION, 200)
      if (
        url === '/api/admin/v1/notifications/deliveries/delivery-0001/retry' &&
        request.method === 'POST'
      ) {
        return jsonResponse(
          {
            error: {
              code: 'delivery_already_queued',
              message: 'a retry for this Delivery is already queued or in flight',
              requestId: 'req-1',
              fields: ['deliveryId'],
            },
          },
          409,
        )
      }
      if (url === '/api/admin/v1/notifications/deliveries/delivery-0001') {
        return jsonResponse(
          { ...DELIVERY, state: 'pending', attempts: [ATTEMPT], event: EVENT },
          200,
        )
      }
      return jsonResponse({ error: { code: 'not_found' } }, 404)
    })
    vi.stubGlobal('fetch', fetchMock)
    renderAt('/admin/alerts/deliveries/delivery-0001')

    await screen.findByRole('heading', { level: 1, name: /^Delivery deliver/ })
    const button = await screen.findByRole('button', { name: 'Retry delivery' })
    expect((button as HTMLButtonElement).disabled).toBe(true)
    expect(
      await screen.findByText('A retry is already queued — the Server refuses duplicates.'),
    ).toBeTruthy()
  })

  it('does not offer retry for suppressed Deliveries', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/notifications/deliveries/delivery-0001': () =>
        jsonResponse(
          {
            ...DELIVERY,
            state: 'suppressed',
            lastResult: 'suppressed_by_silence:sil-1',
            attempts: [],
            event: EVENT,
          },
          200,
        ),
    })
    renderAt('/admin/alerts/deliveries/delivery-0001')

    await screen.findByRole('heading', { level: 1, name: /^Delivery deliver/ })
    expect(await screen.findByText('Suppressed')).toBeTruthy()
    expect(await screen.findByText(/suppressed by a Silence or Maintenance Window/)).toBeTruthy()
    expect(
      (await screen.findByRole('button', { name: 'Retry delivery' })) as HTMLButtonElement,
    ).toHaveProperty('disabled', true)
  })
})

describe('PAGE-ADMIN-CHANNELS / PAGE-ADMIN-CHANNEL', () => {
  it('lists configured channels with redacted destinations and provider references', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/notifications/channels': () => jsonResponse([CHANNEL], 200),
    })
    renderAt('/admin/alerts/channels')

    await screen.findByRole('heading', { level: 1, name: 'Channels' })
    const row = await screen.findByRole('row', { name: /telegram/ })
    expect(row.textContent).toContain('****6789')
    expect(row.textContent).toContain('telegram-token')
    expect(row.textContent).toContain('5 attempts')
    expect(document.body.textContent).not.toContain('123456789')
  })

  it('shows Empty when no channel is configured', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/notifications/channels': () => jsonResponse([], 200),
    })
    renderAt('/admin/alerts/channels')

    expect(await screen.findByText(/No notification channel is configured/)).toBeTruthy()
  })

  it('sends a test notification and reports the Delivery outcome', async () => {
    let tested = false
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(String(input), init)
      const url = request.url.replace(TEST_ORIGIN, '')
      if (url === '/api/public/v1/session') return jsonResponse(OWNER_SESSION, 200)
      if (
        url === '/api/admin/v1/notifications/channels/telegram/test' &&
        request.method === 'POST'
      ) {
        tested = true
        return jsonResponse({ ...DELIVERY, state: 'succeeded', eventId: 'event-0002' }, 200)
      }
      if (url === '/api/admin/v1/notifications/channels/telegram') {
        return jsonResponse(CHANNEL, 200)
      }
      return jsonResponse({ error: { code: 'not_found' } }, 404)
    })
    vi.stubGlobal('fetch', fetchMock)
    renderAt('/admin/alerts/channels/telegram')

    await screen.findByRole('heading', { level: 1, name: 'Channel telegram' })
    vi.spyOn(window, 'confirm').mockReturnValue(true)
    fireEvent.click(await screen.findByRole('button', { name: 'Send test notification' }))
    await waitFor(() => expect(tested).toBe(true))
    expect(await screen.findByText(/Test Event event-00 sent — Delivery deliver/)).toBeTruthy()
  })

  it('shows the Server refusal when the channel is disabled', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/notifications/channels/telegram': () =>
        jsonResponse({ ...CHANNEL, enabled: false }, 200),
      '/api/admin/v1/notifications/channels/telegram/test': () =>
        jsonResponse(
          {
            error: {
              code: 'channel_disabled',
              message: 'this notification channel is disabled',
              requestId: 'req-1',
              fields: ['channelId'],
            },
          },
          409,
        ),
    })
    renderAt('/admin/alerts/channels/telegram')

    await screen.findByRole('heading', { level: 1, name: 'Channel telegram' })
    const button = await screen.findByRole('button', { name: 'Send test notification' })
    expect((button as HTMLButtonElement).disabled).toBe(true)
    expect(screen.getByText(/tests are refused by the Server/)).toBeTruthy()
  })
})
