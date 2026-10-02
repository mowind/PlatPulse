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

const ACTIVE_SILENCE = {
  silenceId: 'silence-1',
  status: 'active',
  matcherKind: 'node',
  matcherValue: 'node-a',
  reason: 'planned node maintenance',
  startsAt: '2026-03-01T00:00:00Z',
  endsAt: '2026-03-01T02:00:00Z',
  createdBy: 'admin',
  createdAt: '2026-02-28T00:00:00Z',
  cancelledAt: null,
  cancelledBy: null,
}

const CANCELLED_SILENCE = {
  silenceId: 'silence-2',
  status: 'cancelled',
  matcherKind: 'all',
  matcherValue: null,
  reason: 'quiet weekend',
  startsAt: '2026-03-01T00:00:00Z',
  endsAt: '2026-03-01T02:00:00Z',
  createdBy: 'owner2',
  createdAt: '2026-02-28T00:00:00Z',
  cancelledAt: '2026-02-28T01:00:00Z',
  cancelledBy: 'owner3',
}

function jsonResponse(body: unknown, status: number): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
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
  vi.unstubAllGlobals()
  adminQueryClient.clear()
})

describe('PAGE-ADMIN-SILENCES (Alerts Silences)', () => {
  it('lists Silences with the matcher scope, reason, window, authorship, and Server-owned status', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/alerts/silences*': () =>
        jsonResponse({ silences: [ACTIVE_SILENCE, CANCELLED_SILENCE] }, 200),
    })
    renderAt('/admin/alerts/silences')

    await screen.findByRole('heading', { level: 1, name: 'Silences' })
    const table = await screen.findByRole('table', { name: 'Silences' })
    expect(table.getAttribute('data-stack')).not.toBeNull()
    expect(table.querySelector('caption')?.textContent).toBe('Silences')
    expect(table.textContent).toContain('node-a')
    expect(table.textContent).toContain('planned node maintenance')
    expect(table.textContent).toContain('quiet weekend')
    expect(table.textContent).toContain('All alerts')
    expect(table.textContent).toContain('Active')
    expect(table.textContent).toContain('Cancelled')
    expect(table.textContent).toContain('by admin')
    expect(table.textContent).toContain('by owner3')

    // Only an active Silence is cancellable.
    expect(screen.getByRole('button', { name: 'Cancel Silence silence-1' })).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Cancel Silence silence-2' })).toBeNull()
  })

  it('drives the Server-owned status filter through URL state', async () => {
    const urls: string[] = []
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/alerts/silences*': (request) => {
        urls.push(request.url.replace(TEST_ORIGIN, ''))
        return jsonResponse({ silences: [ACTIVE_SILENCE] }, 200)
      },
    })
    renderAt('/admin/alerts/silences')

    await screen.findByRole('heading', { level: 1, name: 'Silences' })
    fireEvent.change(screen.getByLabelText('Status'), { target: { value: 'active' } })

    await waitFor(() => {
      expect(urls.some((url) => url.includes('status=active'))).toBe(true)
    })
    expect(window.location.search).toContain('status=active')
  })

  it('creates a Silence under the Server authority and sends CSRF without optimistic mutation', async () => {
    const posts: Array<{ body: Record<string, unknown>; csrf: string | null }> = []
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/alerts/silences*': async (request) => {
        if (request.method === 'POST') {
          posts.push({
            body: (await request.json()) as Record<string, unknown>,
            csrf: request.headers.get('X-CSRF-Token'),
          })
          return jsonResponse({ auditEventId: 3, silence: ACTIVE_SILENCE }, 200)
        }
        return jsonResponse({ silences: [] }, 200)
      },
    })
    renderAt('/admin/alerts/silences')

    await screen.findByRole('heading', { level: 1, name: 'Silences' })
    fireEvent.change(screen.getByLabelText('Matcher kind'), { target: { value: 'node' } })
    fireEvent.change(screen.getByLabelText('Matcher value'), { target: { value: 'node-a' } })
    fireEvent.change(screen.getByLabelText('Reason'), { target: { value: 'planned node maintenance' } })
    fireEvent.change(screen.getByLabelText('Starts at'), { target: { value: '2026-03-01T10:00' } })
    fireEvent.change(screen.getByLabelText('Ends at'), { target: { value: '2026-03-01T12:00' } })
    fireEvent.click(screen.getByRole('button', { name: 'Create Silence' }))

    await screen.findByText('Silence created. The Server is authoritative for it.')
    expect(posts).toHaveLength(1)
    expect(posts[0].csrf).toBe('csrf-token')
    expect(posts[0].body.matcherKind).toBe('node')
    expect(posts[0].body.matcherValue).toBe('node-a')
    expect(posts[0].body.reason).toBe('planned node maintenance')
    expect(posts[0].body.startsAt).toBe(new Date('2026-03-01T10:00').toISOString())
    expect(posts[0].body.endsAt).toBe(new Date('2026-03-01T12:00').toISOString())
  })

  it('treats a transport failure as an unknown outcome instead of a failed mutation', async () => {
    let posts = 0
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/alerts/silences*': async (request) => {
        if (request.method === 'POST') {
          posts += 1
          throw new TypeError('Failed to fetch')
        }
        return jsonResponse({ silences: [] }, 200)
      },
    })
    renderAt('/admin/alerts/silences')

    await screen.findByRole('heading', { level: 1, name: 'Silences' })
    fireEvent.change(screen.getByLabelText('Matcher kind'), { target: { value: 'node' } })
    fireEvent.change(screen.getByLabelText('Matcher value'), { target: { value: 'node-a' } })
    fireEvent.change(screen.getByLabelText('Reason'), { target: { value: 'planned node maintenance' } })
    fireEvent.change(screen.getByLabelText('Starts at'), { target: { value: '2026-03-01T10:00' } })
    fireEvent.change(screen.getByLabelText('Ends at'), { target: { value: '2026-03-01T12:00' } })
    fireEvent.click(screen.getByRole('button', { name: 'Create Silence' }))

    await screen.findByText(/outcome is unknown/)
    expect(posts).toBe(1)
    // The form is retained so the operator can reconcile against Server truth.
    expect((screen.getByLabelText('Reason') as HTMLInputElement).value).toBe(
      'planned node maintenance',
    )
  })
  it('rejects incomplete or reversed forms locally without sending them', async () => {
    const posts: unknown[] = []
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/alerts/silences*': async (request) => {
        if (request.method === 'POST') posts.push(await request.json())
        return jsonResponse({ silences: [] }, 200)
      },
    })
    renderAt('/admin/alerts/silences')

    await screen.findByRole('heading', { level: 1, name: 'Silences' })

    fireEvent.change(screen.getByLabelText('Matcher kind'), { target: { value: 'node' } })
    fireEvent.click(screen.getByRole('button', { name: 'Create Silence' }))
    await screen.findByText('A matcher value is required unless the matcher kind is all.')

    fireEvent.change(screen.getByLabelText('Matcher kind'), { target: { value: 'all' } })
    fireEvent.click(screen.getByRole('button', { name: 'Create Silence' }))
    await screen.findByText('A reason is required for every Silence.')

    fireEvent.change(screen.getByLabelText('Reason'), { target: { value: 'planned node maintenance' } })
    fireEvent.click(screen.getByRole('button', { name: 'Create Silence' }))
    await screen.findByText('A valid start and end time are required.')

    fireEvent.change(screen.getByLabelText('Starts at'), { target: { value: '2026-03-02T10:00' } })
    fireEvent.change(screen.getByLabelText('Ends at'), { target: { value: '2026-03-01T10:00' } })
    fireEvent.click(screen.getByRole('button', { name: 'Create Silence' }))
    await screen.findByText('The end time must be after the start time.')

    expect(posts).toHaveLength(0)
  })

  it('marks the offending field with aria-invalid and an associated message beside the summary', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/alerts/silences*': () => jsonResponse({ silences: [] }, 200),
    })
    renderAt('/admin/alerts/silences')

    await screen.findByRole('heading', { level: 1, name: 'Silences' })
    fireEvent.change(screen.getByLabelText('Matcher kind'), { target: { value: 'node' } })
    fireEvent.click(screen.getByRole('button', { name: 'Create Silence' }))

    const matcher = screen.getByLabelText('Matcher value')
    expect(matcher.getAttribute('aria-invalid')).toBe('true')
    expect(matcher.getAttribute('aria-describedby')).toBe('silence-matcher-value-error')
    expect(screen.getByText('Enter a matcher value for the selected matcher kind.')).toBeTruthy()
    // The page summary stays alongside the field-level message (§10.3).
    expect(
      screen.getByText('A matcher value is required unless the matcher kind is all.'),
    ).toBeTruthy()

    // Correcting the field clears the mark and the message.
    fireEvent.change(matcher, { target: { value: 'node-a' } })
    expect(screen.getByLabelText('Matcher value').getAttribute('aria-invalid')).toBeNull()
    expect(screen.queryByText('Enter a matcher value for the selected matcher kind.')).toBeNull()
  })

  it('requires an explicit confirmation step before cancelling an active Silence', async () => {
    const mutations: Array<{ method: string; csrf: string | null; path: string }> = []
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/alerts/silences': () => jsonResponse({ silences: [ACTIVE_SILENCE] }, 200),
      '/api/admin/v1/alerts/silences/silence-1/cancel': async (request) => {
        mutations.push({
          method: request.method,
          csrf: request.headers.get('X-CSRF-Token'),
          path: new URL(request.url).pathname,
        })
        return jsonResponse(
          { auditEventId: 4, silence: { ...ACTIVE_SILENCE, status: 'cancelled' } },
          200,
        )
      },
    })
    renderAt('/admin/alerts/silences')

    await screen.findByRole('table', { name: 'Silences' })
    fireEvent.click(screen.getByRole('button', { name: 'Cancel Silence silence-1' }))
    expect(
      screen.getByRole('group', { name: 'Confirm cancellation of Silence silence-1' }),
    ).toBeTruthy()
    expect(mutations).toHaveLength(0)

    fireEvent.click(screen.getByRole('button', { name: 'Confirm cancellation' }))
    await screen.findByText('Silence cancelled. Suppressed messages are not replayed.')
    expect(mutations).toEqual([
      { method: 'POST', csrf: 'csrf-token', path: '/api/admin/v1/alerts/silences/silence-1/cancel' },
    ])
    // webui.md §15.5: the confirmed row is gone from an active-only list, so
    // focus moves to the panel heading rather than vanishing with the control.
    await waitFor(() => {
      expect(document.activeElement).toBe(screen.getByRole('heading', { level: 1, name: 'Silences' }))
    })
  })

  it('restores focus to the row control when a cancellation confirmation is dismissed', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/alerts/silences': () => jsonResponse({ silences: [ACTIVE_SILENCE] }, 200),
    })
    renderAt('/admin/alerts/silences')

    await screen.findByRole('table', { name: 'Silences' })
    fireEvent.click(screen.getByRole('button', { name: 'Cancel Silence silence-1' }))
    fireEvent.click(screen.getByRole('button', { name: 'Keep Silence' }))

    await waitFor(() => {
      expect(document.activeElement).toBe(
        screen.getByRole('button', { name: 'Cancel Silence silence-1' }),
      )
    })
  })

  it('shows the sanitized Server error and states the suppression semantics', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/alerts/silences*': async (request) => {
        if (request.method === 'POST') {
          return jsonResponse(
            {
              error: {
                code: 'alert_validation',
                message: 'silence target does not exist',
                requestId: 'req-1',
                fields: [],
              },
            },
            400,
          )
        }
        return jsonResponse({ silences: [] }, 200)
      },
    })
    renderAt('/admin/alerts/silences')

    await screen.findByRole('heading', { level: 1, name: 'Silences' })
    fireEvent.change(screen.getByLabelText('Reason'), { target: { value: 'planned node maintenance' } })
    fireEvent.change(screen.getByLabelText('Starts at'), { target: { value: '2026-03-01T10:00' } })
    fireEvent.change(screen.getByLabelText('Ends at'), { target: { value: '2026-03-01T12:00' } })
    fireEvent.click(screen.getByRole('button', { name: 'Create Silence' }))

    await screen.findByText('silence target does not exist')
    expect(screen.queryByText('Silence created. The Server is authoritative for it.')).toBeNull()

    // The stable semantics copy is present regardless of the list state.
    expect(screen.getByText('What a Silence does and does not do')).toBeTruthy()
    expect(screen.getByText(/never replays a suppressed message/)).toBeTruthy()
    expect(screen.getByText(/non-retractable/)).toBeTruthy()
  })

  it('exposes the filter group, the labelled create form, and an empty state accessibly', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/alerts/silences*': () => jsonResponse({ silences: [] }, 200),
    })
    renderAt('/admin/alerts/silences')

    await screen.findByRole('heading', { level: 1, name: 'Silences' })
    expect(screen.getByRole('group', { name: 'Silence filters' })).toBeTruthy()
    expect(screen.getByRole('form', { name: 'Create Silence' })).toBeTruthy()
    await screen.findByText('No Silences match these filters.')
    expect((screen.getByRole('button', { name: 'Create Silence' }) as HTMLButtonElement).disabled).toBe(
      false,
    )
  })
})
