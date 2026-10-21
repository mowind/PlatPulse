import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import App from '../App'
import { adminQueryClient } from '../api/admin'
import { client } from '../api/generated/client.gen'
import { auditQueryState } from '../auditQueryState'

const OWNER_SESSION = {
  session: {
    userId: 'u-admin',
    username: 'admin',
    role: 'owner',
    createdAt: '2026-08-12T00:00:00Z',
    lastSeenAt: '2026-08-12T00:00:00Z',
    expiresAt: '2026-08-19T00:00:00Z',
  },
  csrfToken: 'csrf-token',
}

const SESSIONS = {
  sessions: [
    {
      sessionId: 'session-current',
      userId: 'u-admin',
      username: 'admin',
      role: 'owner',
      clientHint: 'Chrome · desktop',
      createdAt: '2026-08-12T08:00:00Z',
      lastSeenAt: '2026-08-12T09:00:00Z',
      expiresAt: '2026-08-19T08:00:00Z',
      current: true,
    },
    {
      sessionId: 'session-other',
      userId: 'u-viewer',
      username: 'viewer',
      role: 'viewer',
      clientHint: 'Firefox · mobile',
      createdAt: '2026-08-12T07:00:00Z',
      lastSeenAt: '2026-08-12T08:30:00Z',
      expiresAt: '2026-08-19T07:00:00Z',
      current: false,
    },
  ],
}

const AUDIT = {
  items: [
    {
      auditEventId: 7,
      eventKind: 'viewer_created',
      actorUsername: 'admin',
      targetKind: 'user',
      targetId: 'viewer',
      createdAt: '2026-08-12T08:00:00Z',
      details: { username: 'viewer', role: 'viewer' },
    },
    {
      auditEventId: 6,
      eventKind: 'session_revoked',
      actorUsername: 'admin',
      targetKind: 'session',
      targetId: 'session-other',
      createdAt: '2026-08-12T07:00:00Z',
      details: { username: 'viewer', sessionId: 'session-other' },
    },
  ],
  nextBefore: null,
}

function jsonResponse(body: unknown, status: number): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

function errorBody(code: string, status = 409): Response {
  return jsonResponse({ error: { code, message: code, requestId: 'r1', fields: [] } }, status)
}

const TEST_ORIGIN = 'http://platpulse.test'

type RouteContext = { init?: RequestInit; body: string | null; request: Request; method: string }

function mockFetch(routes: Record<string, (ctx: RouteContext) => Response | Promise<Response>>) {
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(String(input), init)
    let body: string | null = null
    try {
      body = await request.clone().text()
    } catch {
      // Non-readable bodies leave `body` as null.
    }
    const url = request.url.replace(TEST_ORIGIN, '').split('?')[0]
    for (const [pattern, handler] of Object.entries(routes)) {
      if (pattern.endsWith('*')) {
        if (url.startsWith(pattern.slice(0, -1))) return handler({ init, body, request, method: request.method })
      } else if (url === pattern) {
        return handler({ init, body, request, method: request.method })
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



describe('PAGE-ACCESS-SESSIONS (Human Sessions)', () => {
  it('lists coarse session metadata and marks the current session', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/sessions': () => jsonResponse(SESSIONS, 200),
    })
    renderAt('/admin/access/sessions')

    await screen.findByRole('heading', { level: 1, name: 'Sessions' })
    const row = await screen.findByRole('row', { name: /admin/ })
    expect(row.textContent).toContain('Chrome · desktop')
    expect(row.textContent).toContain('This session')
    expect(screen.getByRole('row', { name: /viewer/ }).textContent).toContain('Firefox · mobile')
    // No tokens, digests, CSRF, or raw identifiers in the listing.
    const text = document.body.textContent ?? ''
    for (const forbidden of ['csrf-token', 'token_digest', 'digest']) {
      expect(text.toLowerCase()).not.toContain(forbidden)
    }
  })

  it('revokes a session after explicit confirmation', async () => {
    let revoked = false
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/sessions': () => jsonResponse(SESSIONS, 200),
      '/api/admin/v1/sessions/session-other/revoke': ({ request }) => {
        revoked = request.headers.get('X-CSRF-Token') === 'csrf-token'
        return jsonResponse(
          { sessionId: 'session-other', revokedAt: '2026-08-12T10:00:00Z' },
          200,
        )
      },
    })
    renderAt('/admin/access/sessions')
    await screen.findByRole('heading', { level: 1, name: 'Sessions' })

    const viewerRow = await screen.findByRole('row', { name: /viewer/ })
    fireEvent.click(withinRowButton(viewerRow, 'Revoke'))
    fireEvent.click(withinRowButton(viewerRow, 'Confirm revoke'))
    const outcome = await screen.findByText(/Session for viewer revoked/)
    expect(revoked).toBe(true)
    // The confirmation that held focus is gone; focus lands on the outcome.
    await waitFor(() => expect(document.activeElement).toBe(outcome))
  })

  it('keeps the current session when revoking all others', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/sessions': () => jsonResponse(SESSIONS, 200),
      '/api/admin/v1/sessions/revoke-others': () =>
        jsonResponse({ revokedCount: 1 }, 200),
    })
    renderAt('/admin/access/sessions')
    await screen.findByRole('heading', { level: 1, name: 'Sessions' })

    fireEvent.click(await screen.findByRole('button', { name: 'Revoke all other Sessions' }))
    fireEvent.click(screen.getByRole('button', { name: 'Confirm revoke all others' }))
    expect(await screen.findByText(/1 other session revoked/)).toBeTruthy()
    expect(screen.getByText('This session')).toBeTruthy()
  })

  it('shows a typed conflict when a session was already revoked', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/sessions': () => jsonResponse(SESSIONS, 200),
      '/api/admin/v1/sessions/session-other/revoke': () => errorBody('session_already_revoked'),
    })
    renderAt('/admin/access/sessions')
    await screen.findByRole('heading', { level: 1, name: 'Sessions' })

    const viewerRow = await screen.findByRole('row', { name: /viewer/ })
    fireEvent.click(withinRowButton(viewerRow, 'Revoke'))
    fireEvent.click(withinRowButton(viewerRow, 'Confirm revoke'))
    const alert = await screen.findByText(/already_revoked/)
    await waitFor(() => expect(document.activeElement).toBe(alert))
  })

  it('uses the shared Admin header and labels every stacked cell for phones', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/sessions': () => jsonResponse(SESSIONS, 200),
    })
    renderAt('/admin/access/sessions')

    const heading = await screen.findByRole('heading', { level: 1, name: 'Sessions' })
    const page = heading.closest('[data-slot="sessions-page"]') as HTMLElement
    expect(heading.closest('[data-slot="admin-page-header"]')).toBeTruthy()
    // The privacy note stays reachable from one place, not repeated in header and panel.
    expect(within(page).getAllByRole('button', { name: 'About Session metadata' })).toHaveLength(1)
    const viewerRow = await screen.findByRole('row', { name: /viewer/ })
    const panel = page.querySelector('[data-slot="sessions-panel"]') as HTMLElement
    expect(within(panel).getByRole('heading', { level: 2, name: 'Active Sessions' })).toBeTruthy()
    expect(panel.querySelector('[data-slot="sessions-count"]')?.textContent).toBe('2 Sessions')

    const table = panel.querySelector('[data-slot="sessions-table"] table') as HTMLElement
    expect(table.hasAttribute('data-stack')).toBe(true)
    expect(
      Array.from(viewerRow.querySelectorAll('[data-label]')).map((cell) => cell.getAttribute('data-label')),
    ).toEqual(['User', 'Client', 'Created', 'Last active', 'Expires', 'Action'])
  })

  it('returns focus to the row Revoke action when its confirmation is cancelled', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/sessions': () => jsonResponse(SESSIONS, 200),
    })
    renderAt('/admin/access/sessions')

    const viewerRow = await screen.findByRole('row', { name: /viewer/ })
    fireEvent.click(withinRowButton(viewerRow, 'Revoke'))
    expect(viewerRow.textContent).toContain("Revoke now? The user's streams close immediately.")
    fireEvent.click(withinRowButton(viewerRow, 'Cancel'))
    expect(document.activeElement).toBe(withinRowButton(viewerRow, 'Revoke'))

    fireEvent.click(await screen.findByRole('button', { name: 'Revoke all other Sessions' }))
    expect(screen.getByText(/The current Session stays active/)).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Revoke all other Sessions' }))
  })
})

describe('PAGE-ACCESS-AUDIT (Audit review)', () => {
  it('lists immutable redacted events with detail expansion and target links', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/audit': () => jsonResponse(AUDIT, 200),
    })
    renderAt('/admin/access/audit')

    await screen.findByRole('heading', { level: 1, name: 'Audit log' })
    const list = () => document.querySelector('[data-slot="audit-list"]')?.textContent ?? ''
    await waitFor(() => expect(list()).toContain('viewer_created'))
    expect(list()).toContain('session_revoked')
    expect(screen.getByRole('table')).toBeTruthy()
    for (const heading of ['Time', 'Event', 'Actor', 'Target', 'Details']) {
      expect(screen.getByRole('columnheader', { name: heading })).toBeTruthy()
    }
    expect(screen.getByRole('link', { name: 'viewer' })).toBeTruthy()

    const disclosure = screen.getAllByRole('button', { name: 'Show details' })[0]
    fireEvent.click(disclosure)
    expect(disclosure.getAttribute('aria-controls')).toMatch(/^audit-details-/)
    expect(await screen.findByRole('region', { name: /Redacted details for Audit event/ })).toBeTruthy()
    expect(await screen.findByText('username')).toBeTruthy()
    // Redaction: the listing and its details carry no password, token, or
    // credential material (filter labels may name the kinds, so the list
    // container is the assertion boundary).
    const listText = (document.querySelector('[data-slot="audit-list"]')?.textContent ?? '').toLowerCase()
    for (const forbidden of ['password', 'credential', 'token', 'csrf']) {
      expect(listText).not.toContain(forbidden)
    }
  })

  it('filters by event kind and loads older pages with the cursor', async () => {
    let queriedBefore: string | null = null
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/audit': ({ request }) => {
        const url = new URL(request.url)
        queriedBefore = url.searchParams.get('before')
        const kind = url.searchParams.get('event_kind')
        const filtered =
          kind === 'login_failed' || queriedBefore !== null
        return jsonResponse(
          filtered
            ? { items: [{ auditEventId: 5, eventKind: 'login_failed', actorUsername: null, targetKind: 'user', targetId: 'ghost', createdAt: '2026-08-11T00:00:00Z', details: { username: 'ghost' } }], nextBefore: null }
            : { items: AUDIT.items, nextBefore: 6 },
          200,
        )
      },
    })
    renderAt('/admin/access/audit')
    await screen.findByRole('heading', { level: 1, name: 'Audit log' })

    const list = () => document.querySelector('[data-slot="audit-list"]')?.textContent ?? ''
    const kindSelect = await screen.findByLabelText('Event kind')
    fireEvent.change(kindSelect, { target: { value: 'login_failed' } })
    // The Server receives the filter and the filtered listing renders.
    await waitFor(() => expect(list()).toContain('login_failed'))
    expect(list()).not.toContain('viewer_created')

    fireEvent.change(await screen.findByLabelText('Event kind'), { target: { value: '' } })
    fireEvent.click(await screen.findByRole('button', { name: 'Load older events' }))
    await waitFor(() => expect(queriedBefore).toBe('6'))
    expect(await screen.findByText('ghost')).toBeTruthy()
    // Loading an older cursor appends to the current page; it must not erase
    // the newest events that are still part of this Audit view.
    expect(list()).toContain('viewer_created')
  })

  // Generous waits: these flows chain several fetches and renders, which a
  // loaded full-suite run can stretch past the 1s default.
  const SLOW = { timeout: 5000 }
  const OLDER = {
    auditEventId: 5,
    eventKind: 'login_failed',
    actorUsername: null,
    targetKind: 'user',
    targetId: 'ghost',
    createdAt: '2026-08-11T00:00:00Z',
    details: { username: 'ghost' },
  }
  const rowIds = () =>
    Array.from(document.querySelectorAll('[data-slot="audit-table"] tbody tr[id^="event-"]')).map((row) => row.id)
  const count = () => document.querySelector('[data-slot="audit-count"]')?.textContent

  it('keeps loaded events while an older page loads, ignores repeat presses, and appends without duplicates', async () => {
    const befores: (string | null)[] = []
    let resolveOlder: (response: Response) => void = () => {}
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/audit': ({ request }) => {
        const before = new URL(request.url).searchParams.get('before')
        befores.push(before)
        if (before === null) return jsonResponse({ items: AUDIT.items, nextBefore: 6 }, 200)
        return new Promise<Response>((resolve) => {
          resolveOlder = resolve
        })
      },
    })
    renderAt('/admin/access/audit')
    await waitFor(() => expect(rowIds()).toEqual(['event-7', 'event-6']))
    // An expanded event stays expanded through the append.
    fireEvent.click(screen.getAllByRole('button', { name: 'Show details' })[0])
    expect(screen.getByRole('region', { name: 'Redacted details for Audit event 7' })).toBeTruthy()

    const older = screen.getByRole('button', { name: 'Load older events' })
    older.focus()
    fireEvent.click(older)
    const loading = await screen.findByRole('button', { name: 'Loading older events…' })
    expect(loading).toBe(older)
    expect(loading.getAttribute('aria-disabled')).toBe('true')
    expect(document.activeElement).toBe(older)
    // No page-wide loading state; the loaded events and their count remain.
    expect(screen.queryByText('Loading the Audit log…')).toBeNull()
    expect(rowIds()).toEqual(['event-7', 'event-6'])
    expect(count()).toBe('2 loaded')
    fireEvent.click(loading)
    fireEvent.click(loading)
    expect(befores.filter((before) => before === '6')).toHaveLength(1)

    // The Server's page overlaps the loaded one by one event.
    resolveOlder(jsonResponse({ items: [AUDIT.items[1], OLDER], nextBefore: null }, 200))
    await waitFor(() => expect(rowIds()).toEqual(['event-7', 'event-6', 'event-5']))
    expect(count()).toBe('3 loaded')
    expect(screen.getByRole('region', { name: 'Redacted details for Audit event 7' })).toBeTruthy()
    // The last page arrived: the button is gone and focus is not dropped.
    expect(screen.queryByRole('button', { name: /older events/ })).toBeNull()
    const end = screen.getByText('No older events for the current filters.')
    await waitFor(() => expect(document.activeElement).toBe(end))
  })

  it('reports a failed older page beside the list and retries the same cursor', async () => {
    const befores: (string | null)[] = []
    let olderCalls = 0
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/audit': ({ request }) => {
        const before = new URL(request.url).searchParams.get('before')
        befores.push(before)
        if (before === null) return jsonResponse({ items: AUDIT.items, nextBefore: 6 }, 200)
        olderCalls += 1
        if (olderCalls === 1) {
          return jsonResponse({ error: { code: 'unavailable', message: 'Older page unavailable', requestId: 'r1', fields: [] } }, 503)
        }
        return jsonResponse({ items: [OLDER], nextBefore: 4 }, 200)
      },
    })
    renderAt('/admin/access/audit')
    await waitFor(() => expect(rowIds()).toEqual(['event-7', 'event-6']))
    fireEvent.click(screen.getByRole('button', { name: 'Load older events' }))

    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toContain('Older page unavailable')
    expect(alert.textContent).toContain('The 2 loaded events stay listed.')
    // At the bottom of the list, not in place of it.
    expect(alert.closest('[data-slot="audit-older"]')).toBeTruthy()
    expect(rowIds()).toEqual(['event-7', 'event-6'])
    expect(screen.queryByText('Loading the Audit log…')).toBeNull()

    fireEvent.click(screen.getByRole('button', { name: 'Try loading older events again' }))
    await waitFor(() => expect(rowIds()).toEqual(['event-7', 'event-6', 'event-5']))
    // The retry asked for the failed page again instead of skipping its cursor.
    expect(befores).toEqual([null, '6', '6'])
    expect(screen.queryByRole('alert')).toBeNull()
    expect(screen.getByRole('button', { name: 'Load older events' })).toBeTruthy()
  })

  it('says a linked event is not among the loaded results without calling it older, missing or deleted', async () => {
    let resolveFirst: (response: Response) => void = () => {}
    let first = true
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/audit': ({ request }) => {
        const url = new URL(request.url)
        if (url.searchParams.get('event_kind') === 'logout') return jsonResponse({ items: [], nextBefore: null }, 200)
        if (url.searchParams.get('before') === '6') return jsonResponse({ items: [OLDER], nextBefore: null }, 200)
        if (first) {
          first = false
          return new Promise<Response>((resolve) => {
            resolveFirst = resolve
          })
        }
        return jsonResponse({ items: AUDIT.items, nextBefore: 6 }, 200)
      },
    })
    renderAt('/admin/access/audit#event-5')
    const notice = () => document.querySelector('[data-slot="audit-deep-link-notice"]')?.textContent ?? ''

    // Not loaded yet.
    await waitFor(() => expect(notice()).toBe('Looking for event 5 in the Audit log…'), SLOW)

    // Loaded, but outside the loaded range.
    resolveFirst(jsonResponse({ items: AUDIT.items, nextBefore: 6 }, 200))
    await waitFor(() => expect(notice()).toContain('Event 5 is not in the 2 loaded events.'), SLOW)
    expect(notice()).toContain('It may not be loaded yet.')
    for (const text of [notice()]) expect(text).not.toMatch(/older than|does not exist|deleted|removed/i)

    // A filter with no matching events.
    fireEvent.change(screen.getByLabelText('Event kind'), { target: { value: 'logout' } })
    await waitFor(() =>
      expect(notice()).toBe(
        'Event 5 is not in the current results: no events match the current filters. Changing the filters may bring it into view.',
      ),
      SLOW,
    )
    expect(notice()).not.toMatch(/older than|does not exist|deleted/i)

    // Back to every event, then the older page brings the target in.
    fireEvent.change(screen.getByLabelText('Event kind'), { target: { value: '' } })
    fireEvent.click(await screen.findByRole('button', { name: 'Load older events' }, SLOW))
    await waitFor(() => expect(rowIds()).toContain('event-5'), SLOW)
    expect(document.querySelector('[data-slot="audit-deep-link-notice"]')).toBeNull()
    // The reveal opens the event in the effect after it is listed.
    expect(await screen.findByRole('region', { name: 'Redacted details for Audit event 5' }, SLOW)).toBeTruthy()
  })

  it('names the current filters when a linked event is not in a filtered result', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/audit': ({ request }) => {
        const kind = new URL(request.url).searchParams.get('event_kind')
        return jsonResponse(kind ? { items: [AUDIT.items[0]], nextBefore: null } : { items: AUDIT.items, nextBefore: null }, 200)
      },
    })
    renderAt('/admin/access/audit#event-6')
    await waitFor(() => expect(rowIds()).toEqual(['event-7', 'event-6']))
    expect(document.querySelector('[data-slot="audit-deep-link-notice"]')).toBeNull()
    fireEvent.change(screen.getByLabelText('Event kind'), { target: { value: 'viewer_created' } })
    await waitFor(() => expect(rowIds()).toEqual(['event-7']))
    expect(document.querySelector('[data-slot="audit-deep-link-notice"]')?.textContent).toBe(
      'Event 6 is not in the 1 loaded event. It may be excluded by the current filters; no further events are available for them.',
    )
  })

  it('opens a linked event again when a filter change brings it back into the results', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/audit': ({ request }) => {
        const kind = new URL(request.url).searchParams.get('event_kind')
        return jsonResponse(kind ? { items: [AUDIT.items[0]], nextBefore: null } : { items: AUDIT.items, nextBefore: null }, 200)
      },
    })
    renderAt('/admin/access/audit#event-6')
    const region = () => screen.queryByRole('region', { name: 'Redacted details for Audit event 6' })
    await waitFor(() => expect(region()).toBeTruthy(), SLOW)
    fireEvent.click(screen.getByRole('button', { name: 'Hide details' }))
    expect(region()).toBeNull()
    fireEvent.change(screen.getByLabelText('Event kind'), { target: { value: 'viewer_created' } })
    await waitFor(() => expect(rowIds()).toEqual(['event-7']), SLOW)
    fireEvent.change(screen.getByLabelText('Event kind'), { target: { value: '' } })
    await waitFor(() => expect(region()).toBeTruthy(), SLOW)
    expect(document.querySelector('[data-slot="audit-deep-link-notice"]')).toBeNull()
  })

  // A refresh is what an invalidation (an SSE signal, a reconnect, a
  // mutation elsewhere in the tab) asks of the shown Audit query.
  const refreshAudit = () =>
    act(async () => {
      await adminQueryClient.invalidateQueries({ queryKey: ['admin', 'audit'] })
    })
  const refreshRow = () => document.querySelector('[data-slot="audit-refresh-error"]')
  const unavailable = (message: string) =>
    jsonResponse({ error: { code: 'unavailable', message, requestId: 'r1', fields: [] } }, 503)

  it('A: keeps the first-page retry button and its focus in place while the retry runs', async () => {
    let calls = 0
    let resolveRetry: (response: Response) => void = () => {}
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/audit': () => {
        calls += 1
        if (calls === 1) return unavailable('Audit log unavailable')
        return new Promise<Response>((resolve) => {
          resolveRetry = resolve
        })
      },
    })
    renderAt('/admin/access/audit')
    const alert = await screen.findByRole('alert', {}, SLOW)
    expect(alert.textContent).toContain('Audit log unavailable')
    const retry = screen.getByRole('button', { name: 'Try again' })
    retry.focus()
    fireEvent.click(retry)
    // Never a blank panel: the same row says the retry is running.
    const busy = await screen.findByRole('button', { name: 'Trying again…' })
    expect(busy).toBe(retry)
    expect(busy.getAttribute('aria-disabled')).toBe('true')
    expect(document.activeElement).toBe(busy)
    expect(document.querySelector('[data-slot="audit-initial-error"] [role="status"]')?.textContent).toContain(
      'Loading the Audit log again…',
    )
    fireEvent.click(busy)
    fireEvent.click(busy)
    expect(calls).toBe(2)

    resolveRetry(jsonResponse({ items: AUDIT.items, nextBefore: null }, 200))
    await waitFor(() => expect(rowIds()).toEqual(['event-7', 'event-6']), SLOW)
    expect(screen.queryByRole('alert')).toBeNull()
    expect(screen.queryByRole('button', { name: /Try again|Trying again/ })).toBeNull()
    // Focus lands on the events the retry brought back. Focus is moved by an
    // effect, so it is waited for instead of read in the same tick: a slow
    // flush would otherwise fail a page that behaves correctly.
    await waitFor(
      () => expect(document.activeElement).toBe(document.querySelector('[data-slot="audit-table"]')),
      SLOW,
    )
  })

  it('A: moves focus to the empty result when a retry succeeds with no events', async () => {
    let calls = 0
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/audit': () => {
        calls += 1
        return calls === 1 ? unavailable('Audit log unavailable') : jsonResponse({ items: [], nextBefore: null }, 200)
      },
    })
    renderAt('/admin/access/audit')
    const retry = await screen.findByRole('button', { name: 'Try again' }, SLOW)
    retry.focus()
    fireEvent.click(retry)
    expect(await screen.findByText('No matching Audit events.', {}, SLOW)).toBeTruthy()
    await waitFor(() => expect(document.activeElement).toBe(document.querySelector('[data-slot="audit-empty"]')))
  })

  it('B: keeps the loaded first page when its refresh fails and says so once, above the list', async () => {
    let failing = false
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/audit': () =>
        failing ? unavailable('Refresh unavailable') : jsonResponse({ items: AUDIT.items, nextBefore: null }, 200),
    })
    renderAt('/admin/access/audit')
    await waitFor(() => expect(rowIds()).toEqual(['event-7', 'event-6']), SLOW)
    fireEvent.click(screen.getAllByRole('button', { name: 'Show details' })[0])

    failing = true
    await refreshAudit()
    await waitFor(() => expect(refreshRow()).toBeTruthy(), SLOW)
    const alerts = screen.getAllByRole('alert')
    expect(alerts).toHaveLength(1)
    expect(alerts[0].textContent).toBe(
      'ErrorRefreshing the Audit log failed: Refresh unavailable. Showing the 2 events loaded before.',
    )
    // Not a page-wide error and not an older-page error.
    expect(screen.queryByRole('button', { name: 'Try again' })).toBeNull()
    expect(document.querySelector('[data-slot="audit-older-error"]')).toBeNull()
    expect(rowIds()).toEqual(['event-7', 'event-6'])
    expect(count()).toBe('2 loaded')
    expect(screen.getByRole('region', { name: 'Redacted details for Audit event 7' })).toBeTruthy()
    // The list follows the row, not replaced by it.
    expect(refreshRow()!.compareDocumentPosition(document.querySelector('[data-slot="audit-list"]')!)).toBe(
      Node.DOCUMENT_POSITION_FOLLOWING,
    )
  })

  it('C, D, E: a failed refresh after several pages keeps every page, retries the same cursor once, and clears on success', async () => {
    const befores: (string | null)[] = []
    let mode: 'ok' | 'fail' | 'hold' = 'ok'
    let olderCalls = 0
    let resolveHeld: (response: Response) => void = () => {}
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/audit': ({ request }) => {
        const before = new URL(request.url).searchParams.get('before')
        befores.push(before)
        if (before === null) return jsonResponse({ items: AUDIT.items, nextBefore: 6 }, 200)
        olderCalls += 1
        // D: the newly appended page fails first and is retried by its cursor.
        if (olderCalls === 1) return unavailable('Older page unavailable')
        if (mode === 'fail') return unavailable('Refresh unavailable')
        if (mode === 'hold') {
          return new Promise<Response>((resolve) => {
            resolveHeld = resolve
          })
        }
        return jsonResponse({ items: [AUDIT.items[1], OLDER], nextBefore: 4 }, 200)
      },
    })
    renderAt('/admin/access/audit')
    await waitFor(() => expect(rowIds()).toEqual(['event-7', 'event-6']), SLOW)
    fireEvent.click(screen.getByRole('button', { name: 'Load older events' }))
    await screen.findByRole('button', { name: 'Try loading older events again' }, SLOW)
    expect(refreshRow()).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Try loading older events again' }))
    await waitFor(() => expect(rowIds()).toEqual(['event-7', 'event-6', 'event-5']), SLOW)
    expect(befores).toEqual([null, '6', '6'])
    fireEvent.click(screen.getAllByRole('button', { name: 'Show details' })[0])
    expect(screen.getByRole('region', { name: 'Redacted details for Audit event 7' })).toBeTruthy()

    // C: the shown query (the cursor-6 page) fails its refresh.
    mode = 'fail'
    await refreshAudit()
    await waitFor(() => expect(refreshRow()).toBeTruthy(), SLOW)
    expect(befores).toEqual([null, '6', '6', '6'])
    expect(refreshRow()!.textContent).toContain('Showing the 3 events loaded before.')
    expect(screen.getAllByRole('alert')).toHaveLength(1)
    expect(document.querySelector('[data-slot="audit-older-error"]')).toBeNull()
    expect(rowIds()).toEqual(['event-7', 'event-6', 'event-5'])
    expect(count()).toBe('3 loaded')
    // The cursor for the next page is still the one the last success gave.
    expect(screen.getByRole('button', { name: 'Load older events' })).toBeTruthy()

    // E: the retry asks for the same cursor once, says it is running, and
    // clears the row when it succeeds.
    mode = 'hold'
    const retry = screen.getByRole('button', { name: 'Try refreshing again' })
    retry.focus()
    fireEvent.click(retry)
    const busy = await screen.findByRole('button', { name: 'Refreshing…' })
    expect(busy.getAttribute('aria-disabled')).toBe('true')
    expect(document.activeElement).toBe(busy)
    fireEvent.click(busy)
    expect(befores).toEqual([null, '6', '6', '6', '6'])
    // While it runs the row says so (and still says what is shown); a new
    // failure mounts a fresh alert, so it is announced again.
    expect(within(refreshRow() as HTMLElement).getByRole('status').textContent).toBe(
      'StartingRefreshing the Audit log again. Showing the 3 events loaded before.',
    )
    expect(within(refreshRow() as HTMLElement).queryByRole('alert')).toBeNull()
    resolveHeld(jsonResponse({ items: [AUDIT.items[1], OLDER], nextBefore: 4 }, 200))
    await waitFor(() => expect(refreshRow()).toBeNull(), SLOW)
    expect(screen.queryByRole('alert')).toBeNull()
    expect(rowIds()).toEqual(['event-7', 'event-6', 'event-5'])
    expect(count()).toBe('3 loaded')
    expect(screen.getByRole('region', { name: 'Redacted details for Audit event 7' })).toBeTruthy()
    // As above: focus moves in an effect once the retry's result is shown.
    await waitFor(
      () => expect(document.activeElement).toBe(document.querySelector('[data-slot="audit-table"]')),
      SLOW,
    )
  })
})

describe('auditQueryState', () => {
  const base = { settled: true, appending: false, hasData: true, isError: false, isFetching: false, retrying: false }
  it('keeps the first-page, older-page and refresh failures apart', () => {
    expect(auditQueryState({ ...base, settled: false, hasData: false, isFetching: true })).toBe('initial-loading')
    expect(auditQueryState({ ...base, settled: false, hasData: false, isError: true, retrying: true })).toBe('initial-failed')
    expect(auditQueryState({ ...base, settled: false, hasData: false, isFetching: true, retrying: true })).toBe('initial-retrying')
    // Never a state without feedback before the first page is merged.
    expect(auditQueryState({ ...base, settled: false, hasData: false, isError: true, isFetching: true })).toBe('initial-retrying')
    expect(auditQueryState({ ...base, settled: false, hasData: true, isError: true })).toBe('initial-loading')
    expect(auditQueryState({ ...base, appending: true, hasData: false, isFetching: true })).toBe('older-loading')
    expect(auditQueryState({ ...base, appending: true, hasData: false, isError: true, retrying: true })).toBe('older-failed')
    expect(auditQueryState({ ...base, appending: true, hasData: false, isFetching: true, retrying: true })).toBe('older-loading')
    expect(auditQueryState({ ...base, isError: true, retrying: true })).toBe('refresh-failed')
    expect(auditQueryState({ ...base, appending: true, isError: true, retrying: true })).toBe('refresh-failed')
    expect(auditQueryState({ ...base, isError: true, isFetching: true, retrying: true })).toBe('refresh-retrying')
    expect(auditQueryState({ ...base, isFetching: true })).toBe('ready')
    // An automatic load of a selection that failed earlier is a plain load.
    expect(auditQueryState({ ...base, settled: false, hasData: false, isFetching: true })).toBe('initial-loading')
    // The same selection under a new key (generation change) with the old
    // events listed: its failure and its retry are a refresh's.
    expect(auditQueryState({ ...base, hasData: false, isError: true })).toBe('refresh-failed')
    expect(auditQueryState({ ...base, hasData: false, isFetching: true, retrying: true })).toBe('refresh-retrying')
    expect(auditQueryState({ ...base, hasData: false, isFetching: true })).toBe('ready')
    expect(auditQueryState(base)).toBe('ready')
  })
})

function withinRowButton(row: HTMLElement, name: string): HTMLButtonElement {
  const button = Array.from(row.querySelectorAll('button')).find(
    (element) => element.textContent?.trim() === name,
  )
  if (!button) throw new Error(`button ${name} not found in row`)
  return button
}