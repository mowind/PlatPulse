import { act, cleanup, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { client } from '../api/generated/client.gen'
import { AuthProvider, useAuth } from './AuthContext'
import { ServerOutageNotice } from '../components/ServerOutageNotice'
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
  return (
    <>
      <span>{auth.status.state}</span>
      <ServerOutageNotice />
    </>
  )
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
    // is mounted (webui.md §15.25), so the boundary that outlives every shell is
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

/**
 * A Server that cannot answer is not a Server that refused the cookie
 * (issue #267): only an explicit `401` may sign a reader out. Everything else
 * — a restarting Server behind a reverse proxy, a dropped connection, a
 * database failure — leaves the access this document already has untouched,
 * and the probe keeps asking until the Server answers again.
 */
describe('a Server that cannot answer', () => {
  type ProbeAnswer = 'session' | 'unauthenticated' | 'bad_gateway' | 'offline' | 'proxy_401'
  let answer: ProbeAnswer

  function sessionRoute(): Response | Promise<Response> {
    switch (answer) {
      case 'offline':
        return Promise.reject(new TypeError('Failed to fetch'))
      case 'bad_gateway':
        return new Response('<html><body>502 Bad Gateway</body></html>', {
          status: 502,
          headers: { 'Content-Type': 'text/html' },
        })
      case 'proxy_401':
        // An intermediary in front of a restarting Server refuses on its own:
        // a 401 that is not this Server's `auth_required` answer.
        return new Response('<html><body>401 Authorization Required</body></html>', {
          status: 401,
          headers: { 'Content-Type': 'text/html' },
        })
      case 'unauthenticated':
        return jsonResponse(
          {
            error: {
              code: 'auth_required',
              message: 'authentication required',
              requestId: 'request-1',
              fields: [],
            },
          },
          401,
        )
      default:
        return jsonResponse(OWNER_SESSION, 200)
    }
  }

  /** Mount the boundary and let one probe finish; only used with fake timers. */
  async function renderProbe() {
    render(
      <AuthProvider>
        <ReadingProbe />
      </AuthProvider>,
    )
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10)
    })
  }

  beforeEach(() => {
    answer = 'session'
    auth = null
    client.setConfig({ baseUrl: TEST_ORIGIN })
    mockFetch({ '/api/public/v1/session': () => sessionRoute() })
  })

  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
    vi.useRealTimers()
  })

  it('keeps the signed-in reader through a 502 from the reverse proxy', async () => {
    await renderSignedIn()

    answer = 'bad_gateway'
    let outcome = ''
    await act(async () => {
      outcome = await auth!.recheckSession()
    })

    expect(outcome).toBe('unavailable')
    expect(auth!.status.state).toBe('authenticated')
    expect(auth!.accessLost).toBe(false)
  })

  it('keeps the signed-in reader through a dropped connection', async () => {
    await renderSignedIn()

    answer = 'offline'
    let outcome = ''
    await act(async () => {
      outcome = await auth!.recheckSession()
    })

    expect(outcome).toBe('unavailable')
    expect(auth!.status.state).toBe('authenticated')
    expect(auth!.accessLost).toBe(false)
  })

  it('does not declare a Guest while the first probe cannot reach the Server, then recovers', async () => {
    vi.useFakeTimers()
    answer = 'bad_gateway'

    await renderProbe()
    expect(auth!.status.state).toBe('unavailable')
    expect(auth!.accessLost).toBe(false)

    // The Server comes back on the retry: the reader is signed in again
    // without ever having been sent to the login page.
    answer = 'session'
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_000)
    })
    expect(auth!.status.state).toBe('authenticated')
    expect(auth!.accessLost).toBe(false)
    expect(auth!.hadSession).toBe(true)
    expect(screen.getByText('authenticated')).toBeTruthy()
  })

  it('does not declare a Guest while the first probe has no connection, then recovers', async () => {
    vi.useFakeTimers()
    answer = 'offline'

    await renderProbe()
    expect(auth!.status.state).toBe('unavailable')
    expect(auth!.accessLost).toBe(false)
    expect(auth!.hadSession).toBe(false)

    answer = 'session'
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_000)
    })
    expect(auth!.status.state).toBe('authenticated')
    expect(auth!.accessLost).toBe(false)
    expect(auth!.outage).toBe(false)
  })

  it('keeps a rejected login from ending the access question it interrupted', async () => {
    vi.useFakeTimers()
    answer = 'offline'
    mockFetch({
      '/api/public/v1/session': () => sessionRoute(),
      '/api/public/v1/login': () =>
        jsonResponse(
          {
            error: {
              code: 'invalid_credentials',
              message: 'invalid credentials',
              requestId: 'request-2',
              fields: [],
            },
          },
          401,
        ),
    })

    await renderProbe()
    expect(auth!.status.state).toBe('unavailable')

    await act(async () => {
      await expect(auth!.login('admin', 'wrong')).rejects.toThrow()
    })
    // Nobody was signed in, so the unanswered first probe keeps asking: a
    // Server that came back must still reach this reader (issue #267).
    expect(auth!.status.state).toBe('unavailable')
    expect(auth!.outage).toBe(true)

    answer = 'session'
    await act(async () => {
      await vi.advanceTimersByTimeAsync(30_000)
    })
    expect(auth!.status.state).toBe('authenticated')
  })

  it('retries a re-check the Server could not answer until it answers', async () => {
    vi.useFakeTimers()

    await renderProbe()
    expect(auth!.status.state).toBe('authenticated')

    answer = 'offline'
    await act(async () => {
      await auth!.recheckSession()
    })
    expect(auth!.status.state).toBe('authenticated')
    // The access is kept, and the shell says why the reading is the last one
    // the Server gave instead of silently passing it off as current.
    expect(auth!.outage).toBe(true)
    expect(
      screen.getByText('The Server cannot confirm your Session right now. Retrying automatically.'),
    ).toBeTruthy()
    const generationDuringOutage = auth!.generation

    answer = 'session'
    await act(async () => {
      await vi.advanceTimersByTimeAsync(30_000)
    })
    expect(auth!.status.state).toBe('authenticated')
    expect(auth!.accessLost).toBe(false)
    expect(auth!.outage).toBe(false)
    expect(
      screen.queryByText('The Server cannot confirm your Session right now. Retrying automatically.'),
    ).toBeNull()
    // The retried re-check really reached the Server: a confirmed answer
    // starts a new authorization generation.
    expect(auth!.generation).toBeGreaterThan(generationDuringOutage)
  })

  it('keeps the signed-in reader through a 401 that is not the Session refusal', async () => {
    await renderSignedIn()

    answer = 'proxy_401'
    let outcome = ''
    await act(async () => {
      outcome = await auth!.recheckSession()
    })

    expect(outcome).toBe('unavailable')
    expect(auth!.status.state).toBe('authenticated')
    expect(auth!.accessLost).toBe(false)
  })

  it('signs the reader out when the Server refuses the cookie with 401', async () => {
    await renderSignedIn()

    answer = 'unauthenticated'
    let outcome = ''
    await act(async () => {
      outcome = await auth!.recheckSession()
    })

    expect(outcome).toBe('signed_out')
    expect(auth!.status.state).toBe('guest')
    expect(auth!.accessLost).toBe(true)
  })

  it('declares a Guest on the first probe when the Server refuses the cookie', async () => {
    answer = 'unauthenticated'

    await render(
      <AuthProvider>
        <ReadingProbe />
      </AuthProvider>,
    )
    await screen.findByText('guest')
    expect(auth!.accessLost).toBe(false)
    expect(auth!.hadSession).toBe(false)
  })
})
