import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import App from '../App'
import { adminQueryClient } from '../api/admin'
import { client } from '../api/generated/client.gen'

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

const PEOPLE = {
  users: [
    {
      userId: 'u-admin',
      username: 'admin',
      role: 'owner',
      disabled: false,
      createdAt: '2026-08-01T00:00:00Z',
      sessionCount: 2,
    },
    {
      userId: 'u-viewer',
      username: 'viewer',
      role: 'viewer',
      disabled: true,
      createdAt: '2026-08-02T00:00:00Z',
      sessionCount: 0,
    },
  ],
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

describe('PAGE-ACCESS-PEOPLE (People and roles)', () => {
  it('lists People with role, status, and session counts but never password material', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/people': () => jsonResponse(PEOPLE, 200),
      '/api/admin/v1/access-mode': () => jsonResponse({ mode: 'private', authorizationGeneration: 0 }, 200),
    })
    renderAt('/admin/access/people')

    await screen.findByRole('heading', { level: 1, name: 'People' })
    const row = await screen.findByRole('row', { name: /admin/ })
    expect(row.textContent).toContain('Owner')
    expect(row.textContent).toContain('Enabled')
    expect(row.textContent).toContain('2')
    expect(screen.getByRole('row', { name: /viewer/ }).textContent).toContain('Disabled')
    const documentText = document.body.textContent ?? ''
    // The page copy may name the word "password", but no password VALUE,
    // hash, token, or CSRF material may be rendered.
    for (const forbidden of ['$argon2id', 'token', 'csrf-token']) {
      expect(documentText.toLowerCase()).not.toContain(forbidden)
    }
  })

  it('creates a user through the authoritative mutation and refetches', async () => {
    let created = false
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/access-mode': () => jsonResponse({ mode: 'private', authorizationGeneration: 0 }, 200),
      '/api/admin/v1/people': ({ method }) => {
        if (method === 'POST') {
          created = true
          return jsonResponse(
            { userId: 'u-new', username: 'newuser', role: 'viewer', disabled: false, createdAt: '2026-08-12T00:00:00Z', sessionCount: 0 },
            200,
          )
        }
        return jsonResponse(PEOPLE, 200)
      },
    })
    renderAt('/admin/access/people')
    await screen.findByRole('heading', { level: 1, name: 'People' })

    fireEvent.change(screen.getByLabelText('Username'), { target: { value: 'newuser' } })
    fireEvent.change(screen.getByLabelText('Password'), { target: { value: 'a long enough password' } })
    fireEvent.click(screen.getByRole('button', { name: 'Create user' }))
    await screen.findByText(/newuser created as a Viewer/)
    expect(created).toBe(true)
  })

  it('surfaces the final-Owner protection conflict without optimistic state', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/people': () => jsonResponse(PEOPLE, 200),
      '/api/admin/v1/access-mode': () => jsonResponse({ mode: 'private', authorizationGeneration: 0 }, 200),
      '/api/admin/v1/people/u-admin/role': () => errorBody('final_owner_protected'),
    })
    renderAt('/admin/access/people')
    await screen.findByRole('heading', { level: 1, name: 'People' })

    const row = await screen.findByRole('row', { name: /admin/ })
    const roleSelect = row.querySelector('select') as HTMLSelectElement
    fireEvent.change(roleSelect, { target: { value: 'viewer' } })
    expect(await screen.findByText('The final valid Owner cannot be demoted.')).toBeTruthy()
    // No optimistic state: the row still shows Owner.
    expect(roleSelect.value).toBe('owner')
  })

  it('toggles anonymous Home through the authoritative mutation', async () => {
    let mode = 'private'
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/people': () => jsonResponse(PEOPLE, 200),
      '/api/admin/v1/access-mode': ({ method }) => {
        if (method === 'PUT') {
          mode = 'public'
          return jsonResponse({ mode, authorizationGeneration: 1 }, 200)
        }
        return jsonResponse({ mode, authorizationGeneration: mode === 'public' ? 1 : 0 }, 200)
      },
    })
    renderAt('/admin/access/people')
    await screen.findByRole('heading', { level: 1, name: 'People' })

    vi.spyOn(window, 'confirm').mockReturnValue(true)
    fireEvent.click(await screen.findByRole('button', { name: 'Make Home Public' }))
    await screen.findByText(/Site Access Mode is now Public/)
    expect(await screen.findByRole('button', { name: 'Make Home Private' })).toBeTruthy()
  })
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
    await screen.findByText(/Session for viewer revoked/)
    expect(revoked).toBe(true)
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
    expect(await screen.findByText(/already_revoked/)).toBeTruthy()
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
    const list = () => document.querySelector('.audit-list')?.textContent ?? ''
    await waitFor(() => expect(list()).toContain('viewer_created'))
    expect(list()).toContain('session_revoked')
    expect(screen.getByRole('link', { name: 'viewer' })).toBeTruthy()

    fireEvent.click(screen.getAllByRole('button', { name: 'Show details' })[0])
    expect(await screen.findByText('username')).toBeTruthy()
    // Redaction: the listing and its details carry no password, token, or
    // credential material (filter labels may name the kinds, so the list
    // container is the assertion boundary).
    const listText = (document.querySelector('.audit-list')?.textContent ?? '').toLowerCase()
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

    const list = () => document.querySelector('.audit-list')?.textContent ?? ''
    const kindSelect = await screen.findByLabelText('Event kind')
    fireEvent.change(kindSelect, { target: { value: 'login_failed' } })
    // The Server receives the filter and the filtered listing renders.
    await waitFor(() => expect(list()).toContain('login_failed'))
    expect(list()).not.toContain('viewer_created')

    fireEvent.change(await screen.findByLabelText('Event kind'), { target: { value: '' } })
    fireEvent.click(await screen.findByRole('button', { name: 'Load older events' }))
    await waitFor(() => expect(queriedBefore).toBe('6'))
    expect(await screen.findByText('ghost')).toBeTruthy()
  })
})

function withinRowButton(row: HTMLElement, name: string): HTMLButtonElement {
  const button = Array.from(row.querySelectorAll('button')).find(
    (element) => element.textContent?.trim() === name,
  )
  if (!button) throw new Error(`button ${name} not found in row`)
  return button
}
