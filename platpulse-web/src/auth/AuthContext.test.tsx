import { act, cleanup, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { client } from '../api/generated/client.gen'
import { AuthProvider, useAuth } from './AuthContext'
import {
  homeReturnState,
  homeReturnTab,
  readHomeReturn,
  type HomeReturnState,
} from '../homeReturn'

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

const VIEWER_SESSION = {
  ...OWNER_SESSION,
  session: { ...OWNER_SESSION.session, userId: 'u2', username: 'viewer1', role: 'viewer' },
}

function jsonResponse(body: unknown, status: number): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

type RouteHandler = (init?: RequestInit) => Response | Promise<Response>

const TEST_ORIGIN = 'http://platpulse.test'

/** Stub global fetch with per-URL handlers; `*`-suffixed keys match prefixes. */
function mockFetch(routes: Record<string, RouteHandler>) {
  const fetchMock = vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
    const url = (input instanceof Request ? input.url : String(input)).replace(TEST_ORIGIN, '')
    for (const [pattern, handler] of Object.entries(routes)) {
      if (pattern.endsWith('*')) {
        if (url.startsWith(pattern.slice(0, -1))) return Promise.resolve(handler(init))
      } else if (url === pattern) {
        return Promise.resolve(handler(init))
      }
    }
    return Promise.resolve(jsonResponse({ error: { code: 'not_found' } }, 404))
  })
  vi.stubGlobal('fetch', fetchMock)
  return fetchMock
}

/** The reading a reader leaves behind in a history entry: filters, sort, view
 *  and the place they had, written the way the Home shell writes it, under the
 *  token this document holds for the reader signed in at that moment. */
/** The session the Server would report right now, so a re-check answers for
 *  the reader who is actually signed in. */
let session: unknown = OWNER_SESSION
let departure: HomeReturnState | null = null
let auth: ReturnType<typeof useAuth> | null = null

function ReadingProbe() {
  auth = useAuth()
  return <span>{auth.status.state}</span>
}

function depart() {
  departure = homeReturnState(
    'node-l',
    new URLSearchParams('view=list&sort=peers'),
    0,
    2,
    homeReturnTab(),
  )
}

/** Whether the entry recorded above is still the one this document reads back. */
function accepted(): boolean {
  return departure !== null && readHomeReturn(departure, homeReturnTab()) !== null
}

async function renderSignedIn() {
  render(
    <AuthProvider>
      <ReadingProbe />
    </AuthProvider>,
  )
  await screen.findByText('authenticated')
}

describe('the reader a departure belongs to', () => {
  beforeEach(() => {
    session = OWNER_SESSION
    departure = null
    auth = null
    client.setConfig({ baseUrl: TEST_ORIGIN })
  })

  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
  })

  it('gives a reader their own return, and starts a new reading for the next one', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(session, 200),
      '/api/public/v1/login': () => {
        session = VIEWER_SESSION
        return jsonResponse(VIEWER_SESSION, 200)
      },
      '/api/public/v1/logout': () => new Response(null, { status: 204 }),
    })
    await renderSignedIn()

    depart()
    expect(accepted()).toBe(true)

    // Signing out and signing in as somebody else happens while no Home shell
    // is mounted (design §15.25), so the boundary that outlives every shell is
    // the one that has to notice: the entry the first reader left behind must
    // never be read back in front of the reader who replaced them.
    await act(async () => {
      await auth!.login('viewer1', 'correct horse battery')
    })
    await waitFor(() => expect(accepted()).toBe(false))

    depart()
    expect(accepted()).toBe(true)

    // A successful access re-check republishes the same reader's data at a new
    // authorization generation. It is not a new reader, so the reading they
    // are owed survives it.
    await act(async () => {
      await auth!.recheckSession()
    })
    expect(accepted()).toBe(true)

    await act(async () => {
      await auth!.logout()
    })
    await waitFor(() => expect(accepted()).toBe(false))
  })
})
