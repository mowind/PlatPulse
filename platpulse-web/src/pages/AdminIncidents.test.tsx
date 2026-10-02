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

const OPEN_INCIDENT_ID = '0195f2a1-0100-4100-8100-000000000100'
const RESOLVED_INCIDENT_ID = '0195f2a1-0101-4101-8101-000000000101'

/** One open, unacknowledged Node occurrence. */
const OPEN_INCIDENT = {
  incidentId: OPEN_INCIDENT_ID,
  ruleKey: 'node.rpc_unreachable',
  ruleVersion: 1,
  subjectKind: 'node',
  subjectKey: 'node-a',
  severity: 'warning',
  state: 'open',
  sequence: 1,
  openedAt: '2026-03-01T00:00:00Z',
  resolvedAt: null,
  subjectDeletedAt: null,
  acknowledgment: null,
}

/** One resolved occurrence that already carries the authoritative first Owner. */
const RESOLVED_INCIDENT = {
  ...OPEN_INCIDENT,
  incidentId: RESOLVED_INCIDENT_ID,
  state: 'resolved',
  sequence: 2,
  resolvedAt: '2026-03-01T05:00:00Z',
  acknowledgment: {
    acknowledgedByUsername: 'admin',
    acknowledgedAt: '2026-03-01T00:10:00Z',
    acknowledgedByUserId: 'u1',
  },
}

const OPEN_INCIDENT_DETAIL = {
  ...OPEN_INCIDENT,
  currentRule: {
    ruleKey: 'node.rpc_unreachable',
    enabled: true,
    severity: 'warning',
    version: 1,
    condition: { for_secs: 60, recovery_for_secs: 60 },
  },
  openedEvidence: { input_kind: 'boolean', input_detail: 'connect refused' },
  resolvedEvidence: null,
  evaluation: {
    subjectKind: 'node',
    subjectKey: 'node-a',
    state: 'firing',
    evaluationUnavailable: false,
    inputKind: 'boolean',
    inputValue: 1,
    inputDetail: 'connect refused',
    lastEvaluatedAt: '2026-03-01T00:00:00Z',
    since: '2026-03-01T00:00:00Z',
    firingSince: '2026-03-01T00:01:00Z',
    pendingSince: null,
    recoveringSince: null,
    openIncidents: 1,
  },
  suppressions: [
    {
      id: 'sil-test',
      kind: 'silence',
      marksIncident: false,
      reason: 'planned maintenance window',
      startsAt: '2026-02-28T00:00:00Z',
      endsAt: '2026-03-02T00:00:00Z',
    },
  ],
}

const SECOND_INCIDENT_ID = '0195f2a1-0102-4102-8102-000000000102'

/** A second open occurrence on another Node, unacknowledged. */
const SECOND_INCIDENT_DETAIL = {
  ...OPEN_INCIDENT_DETAIL,
  incidentId: SECOND_INCIDENT_ID,
  subjectKey: 'node-b',
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

describe('PAGE-ADMIN-INCIDENTS (Incident history)', () => {
  it('lists each Incident occurrence with state, severity, subject, and acknowledgment', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/alerts/incidents*': () =>
        jsonResponse({ incidents: [OPEN_INCIDENT, RESOLVED_INCIDENT], total: 2 }, 200),
    })
    renderAt('/admin/alerts/incidents')

    await screen.findByRole('heading', { level: 1, name: 'Incidents' })
    const table = await screen.findByRole('table', { name: /Incident history/ })
    expect(table.textContent).toContain('node.rpc_unreachable')
    expect(table.textContent).toContain('node-a')
    expect(table.textContent).toContain('Warning')
    expect(table.textContent).toContain('Open')
    expect(table.textContent).toContain('Resolved')
    expect(table.textContent).toContain('Awaiting acknowledgment')
    expect(table.textContent).toContain('Acknowledged')
    expect(table.textContent).toContain('admin')
    // The bounded first page is disclosed instead of implying a full history.
    expect(
      screen.getByText(/Showing up to the first 200 matching Incidents/),
    ).toBeTruthy()
  })

  it('filters through URL state so back/forward preserves the filters', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/alerts/incidents*': () =>
        jsonResponse({ incidents: [OPEN_INCIDENT, RESOLVED_INCIDENT], total: 2 }, 200),
    })
    renderAt('/admin/alerts/incidents')

    await screen.findByRole('heading', { level: 1, name: 'Incidents' })
    fireEvent.change(screen.getByLabelText('State'), { target: { value: 'resolved' } })
    await waitFor(() => {
      expect(window.location.search).toContain('state=resolved')
    })
  })

  it('narrows to an exact subject key from a Node or Agent shortcut and can clear it', async () => {
    const listUrls: string[] = []
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/alerts/incidents*': (request) => {
        listUrls.push(request.url)
        return jsonResponse({ incidents: [OPEN_INCIDENT], total: 1 }, 200)
      },
    })
    renderAt('/admin/alerts/incidents?subject=node&subject_key=node-a')

    await screen.findByRole('heading', { level: 1, name: 'Incidents' })
    // The bound Server filter carries the exact subject key, so the shortcut
    // narrows to this subject's occurrences rather than a generic page.
    await waitFor(() => {
      expect(listUrls.some((url) => url.includes('subject_key=node-a'))).toBe(true)
    })
    expect(screen.getByText('Subject key node-a')).toBeTruthy()

    fireEvent.click(screen.getByRole('button', { name: 'Clear subject' }))
    await waitFor(() => {
      expect(window.location.search).not.toContain('subject_key')
    })
    await waitFor(() => {
      expect(listUrls.some((url) => !url.includes('subject_key'))).toBe(true)
    })
  })

  it('acknowledges an open Incident only after an explicit confirmation', async () => {
    const ackRequests: Array<{ method: string; csrf: string | null }> = []
    const fetchMock = mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      ['/api/admin/v1/alerts/incidents/' + OPEN_INCIDENT_ID + '/acknowledgments']: (request) => {
        ackRequests.push({
          method: request.method,
          csrf: request.headers.get('X-CSRF-Token'),
        })
        return jsonResponse(
          {
            incidentId: OPEN_INCIDENT_ID,
            acknowledgment: {
              acknowledgedByUsername: 'admin',
              acknowledgedAt: '2026-03-01T00:20:00Z',
              acknowledgedByUserId: 'u1',
            },
            recorded: true,
            auditEventId: 7,
          },
          200,
        )
      },
      '/api/admin/v1/alerts/incidents*': () =>
        jsonResponse({ incidents: [OPEN_INCIDENT], total: 1 }, 200),
    })
    renderAt('/admin/alerts/incidents')

    await screen.findByRole('heading', { level: 1, name: 'Incidents' })
    fireEvent.click(await screen.findByRole('button', { name: 'Expand Incident ' + OPEN_INCIDENT_ID }))
    fireEvent.click(
      await screen.findByRole('button', { name: 'Acknowledge Incident ' + OPEN_INCIDENT_ID }),
    )
    // The confirmation step must exist before anything is sent.
    expect(await screen.findByRole('group', { name: 'Confirm Incident acknowledgment' })).toBeTruthy()
    expect(ackRequests).toHaveLength(0)
    fireEvent.click(screen.getByRole('button', { name: 'Confirm acknowledgment' }))

    expect(await screen.findByText(/Acknowledgment recorded/)).toBeTruthy()
    expect(ackRequests).toEqual([{ method: 'POST', csrf: 'csrf-token' }])
    expect(fetchMock).toHaveBeenCalled()
  })

  it('reports the stored first-writer identity instead of replacing it', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      ['/api/admin/v1/alerts/incidents/' + OPEN_INCIDENT_ID + '/acknowledgments']: () =>
        jsonResponse(
          {
            incidentId: OPEN_INCIDENT_ID,
            acknowledgment: {
              acknowledgedByUsername: 'other-owner',
              acknowledgedAt: '2026-03-01T00:15:00Z',
              acknowledgedByUserId: 'u2',
            },
            recorded: false,
            auditEventId: null,
          },
          200,
        ),
      '/api/admin/v1/alerts/incidents*': () =>
        jsonResponse({ incidents: [OPEN_INCIDENT], total: 1 }, 200),
    })
    renderAt('/admin/alerts/incidents')

    await screen.findByRole('heading', { level: 1, name: 'Incidents' })
    fireEvent.click(await screen.findByRole('button', { name: 'Expand Incident ' + OPEN_INCIDENT_ID }))
    fireEvent.click(
      await screen.findByRole('button', { name: 'Acknowledge Incident ' + OPEN_INCIDENT_ID }),
    )
    fireEvent.click(await screen.findByRole('button', { name: 'Confirm acknowledgment' }))

    const status = await screen.findByText(/already had an authoritative acknowledgment/)
    expect(status.textContent).toContain('no-op even for the same Owner')
    // The response carries the stored first-writer identity even though the
    // follow-up read still reports the occurrence as unacknowledged.
    expect(await screen.findByText(/Acknowledged by other-owner/)).toBeTruthy()
  })

  it('shows the acknowledgment returned by the write even when the follow-up read fails', async () => {
    let failList = false
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      ['/api/admin/v1/alerts/incidents/' + OPEN_INCIDENT_ID + '/acknowledgments']: () =>
        jsonResponse(
          {
            incidentId: OPEN_INCIDENT_ID,
            acknowledgment: {
              acknowledgedByUsername: 'admin',
              acknowledgedAt: '2026-03-01T00:20:00Z',
              acknowledgedByUserId: 'u1',
            },
            recorded: true,
            auditEventId: 7,
          },
          200,
        ),
      '/api/admin/v1/alerts/incidents*': () =>
        failList
          ? jsonResponse({ error: { code: 'internal', message: 'list unavailable' } }, 500)
          : jsonResponse({ incidents: [OPEN_INCIDENT], total: 1 }, 200),
    })
    renderAt('/admin/alerts/incidents')

    await screen.findByRole('heading', { level: 1, name: 'Incidents' })
    fireEvent.click(await screen.findByRole('button', { name: 'Expand Incident ' + OPEN_INCIDENT_ID }))
    failList = true
    fireEvent.click(
      await screen.findByRole('button', { name: 'Acknowledge Incident ' + OPEN_INCIDENT_ID }),
    )
    fireEvent.click(screen.getByRole('button', { name: 'Confirm acknowledgment' }))

    expect(await screen.findByText(/Acknowledgment recorded/)).toBeTruthy()
    expect(await screen.findByText(/Acknowledged by admin/)).toBeTruthy()
    // The write is final: the confirm control must not remain as if nothing was recorded.
    expect(
      screen.queryByRole('button', { name: 'Acknowledge Incident ' + OPEN_INCIDENT_ID }),
    ).toBeNull()
  })

  it('shows the Server-owned detail with evaluation, suppressions, and evidence', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      ['/api/admin/v1/alerts/incidents/' + OPEN_INCIDENT_ID]: () =>
        jsonResponse(OPEN_INCIDENT_DETAIL, 200),
    })
    renderAt('/admin/alerts/incidents/' + OPEN_INCIDENT_ID)

    await screen.findByRole('heading', { level: 1, name: /Incident 0195f2a1/ })
    expect(screen.getByText('connect refused')).toBeTruthy()
    expect(screen.getByText('planned maintenance window')).toBeTruthy()
    expect(screen.getByText('Does not mark the Incident suppressed')).toBeTruthy()
    expect(screen.getByText('Opened evidence')).toBeTruthy()
    expect(screen.getByText(/Subject deletion is annotated separately from recovery/)).toBeTruthy()
    // The enabled branch of the three-state Rule presentation.
    expect(screen.getByText('Yes')).toBeTruthy()
    expect(screen.queryByText('Current Rule state unknown')).toBeNull()
    expect(screen.queryByText('Rule disabled')).toBeNull()
  })

  it('renders the not-found state for an unknown Incident', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      ['/api/admin/v1/alerts/incidents/' + OPEN_INCIDENT_ID]: () =>
        jsonResponse({ error: { code: 'incident_not_found', message: 'Incident not found' } }, 404),
    })
    renderAt('/admin/alerts/incidents/' + OPEN_INCIDENT_ID)

    await screen.findByRole('heading', { level: 1, name: 'Incident not found' })
    expect(screen.getByRole('link', { name: 'Back to Incidents' })).toBeTruthy()
  })

  it('presents a disabled Rule evaluation as the last recorded assessment', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      ['/api/admin/v1/alerts/incidents/' + OPEN_INCIDENT_ID]: () =>
        jsonResponse(
          {
            ...OPEN_INCIDENT_DETAIL,
            currentRule: { ...OPEN_INCIDENT_DETAIL.currentRule, enabled: false, version: 2 },
          },
          200,
        ),
    })
    renderAt('/admin/alerts/incidents/' + OPEN_INCIDENT_ID)

    await screen.findByRole('heading', { level: 1, name: /Incident 0195f2a1/ })
    expect(screen.getByText('Rule disabled')).toBeTruthy()
    expect(screen.getAllByText(/last recorded assessment/).length).toBeGreaterThan(0)
    expect(screen.getByText('Not currently — the Rule is disabled')).toBeTruthy()
    // A disabled Rule must never claim its stale row is a current, available evaluation.
    expect(screen.queryByText('Yes')).toBeNull()
  })

  it('resyncs the rule-key draft when history navigation changes the filter', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/alerts/incidents*': () =>
        jsonResponse({ incidents: [OPEN_INCIDENT], total: 1 }, 200),
    })
    renderAt('/admin/alerts/incidents?rule=node.rpc_unreachable')

    await screen.findByRole('heading', { level: 1, name: 'Incidents' })
    const input = screen.getByPlaceholderText('e.g. node.rpc_unreachable') as HTMLInputElement
    expect(input.value).toBe('node.rpc_unreachable')

    await act(async () => {
      window.history.pushState({}, '', '/admin/alerts/incidents?rule=node.observation_stale')
      window.dispatchEvent(new PopStateEvent('popstate'))
      await Promise.resolve()
    })
    await waitFor(() => {
      expect(input.value).toBe('node.observation_stale')
    })
  })

  it('moves focus into the confirmation and restores it on cancel and collapse', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/alerts/incidents*': () =>
        jsonResponse({ incidents: [OPEN_INCIDENT], total: 1 }, 200),
    })
    renderAt('/admin/alerts/incidents')

    await screen.findByRole('heading', { level: 1, name: 'Incidents' })
    fireEvent.click(await screen.findByRole('button', { name: 'Expand Incident ' + OPEN_INCIDENT_ID }))
    const disclosure = screen.getByRole('button', {
      name: 'Collapse Incident ' + OPEN_INCIDENT_ID,
    })
    fireEvent.click(
      await screen.findByRole('button', { name: 'Acknowledge Incident ' + OPEN_INCIDENT_ID }),
    )
    const confirm = await screen.findByRole('button', { name: 'Confirm acknowledgment' })
    await waitFor(() => {
      expect(document.activeElement).toBe(confirm)
    })
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    const entry = await screen.findByRole('button', {
      name: 'Acknowledge Incident ' + OPEN_INCIDENT_ID,
    })
    await waitFor(() => {
      expect(document.activeElement).toBe(entry)
    })

    fireEvent.click(screen.getByRole('button', { name: 'Collapse evidence' }))
    await waitFor(() => {
      expect(document.activeElement).toBe(disclosure)
    })
  })

  it('reports an unresolved current Rule state as unknown rather than available', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      ['/api/admin/v1/alerts/incidents/' + OPEN_INCIDENT_ID]: () =>
        jsonResponse({ ...OPEN_INCIDENT_DETAIL, currentRule: null }, 200),
    })
    renderAt('/admin/alerts/incidents/' + OPEN_INCIDENT_ID)

    await screen.findByRole('heading', { level: 1, name: /Incident 0195f2a1/ })
    expect(screen.getByText('Current Rule state unknown')).toBeTruthy()
    expect(screen.getByText('Unknown — the current Rule state could not be resolved')).toBeTruthy()
    // An unresolved Rule state must never be presented as a current, available evaluation.
    expect(screen.queryByText('Yes')).toBeNull()
  })

  it("does not carry one occurrence's acknowledgment onto the next", async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      ['/api/admin/v1/alerts/incidents/' + OPEN_INCIDENT_ID]: () =>
        jsonResponse(OPEN_INCIDENT_DETAIL, 200),
      ['/api/admin/v1/alerts/incidents/' + SECOND_INCIDENT_ID]: () =>
        jsonResponse(SECOND_INCIDENT_DETAIL, 200),
      ['/api/admin/v1/alerts/incidents/' + OPEN_INCIDENT_ID + '/acknowledgments']: () =>
        jsonResponse(
          {
            incidentId: OPEN_INCIDENT_ID,
            acknowledgment: {
              acknowledgedByUsername: 'admin',
              acknowledgedAt: '2026-03-01T00:20:00Z',
              acknowledgedByUserId: 'u1',
            },
            recorded: true,
            auditEventId: 7,
          },
          200,
        ),
    })
    renderAt('/admin/alerts/incidents/' + OPEN_INCIDENT_ID)
    fireEvent.click(
      await screen.findByRole('button', { name: 'Acknowledge Incident ' + OPEN_INCIDENT_ID }),
    )
    fireEvent.click(await screen.findByRole('button', { name: 'Confirm acknowledgment' }))
    expect(await screen.findByText(/Acknowledged by admin/)).toBeTruthy()

    await act(async () => {
      window.history.pushState({}, '', '/admin/alerts/incidents/' + SECOND_INCIDENT_ID)
      window.dispatchEvent(new PopStateEvent('popstate'))
      await Promise.resolve()
    })
    // The second occurrence's own action control, unique to it, must appear.
    expect(
      await screen.findByRole('button', { name: 'Acknowledge Incident ' + SECOND_INCIDENT_ID }),
    ).toBeTruthy()
    expect(screen.queryByText(/Acknowledged by admin/)).toBeNull()
    // Both the header badge and the panel badge report the second occurrence as unacknowledged.
    expect(screen.getAllByText('Awaiting acknowledgment')).toHaveLength(2)
  })

  it('ignores a late response from the occurrence the Owner already left', async () => {
    let releaseAcknowledgment: (() => void) | null = null
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      ['/api/admin/v1/alerts/incidents/' + OPEN_INCIDENT_ID]: () =>
        jsonResponse(OPEN_INCIDENT_DETAIL, 200),
      ['/api/admin/v1/alerts/incidents/' + SECOND_INCIDENT_ID]: () =>
        jsonResponse(SECOND_INCIDENT_DETAIL, 200),
      ['/api/admin/v1/alerts/incidents/' + OPEN_INCIDENT_ID + '/acknowledgments']: () =>
        new Promise<Response>((resolve) => {
          releaseAcknowledgment = () =>
            resolve(
              jsonResponse(
                {
                  incidentId: OPEN_INCIDENT_ID,
                  acknowledgment: {
                    acknowledgedByUsername: 'admin',
                    acknowledgedAt: '2026-03-01T00:20:00Z',
                    acknowledgedByUserId: 'u1',
                  },
                  recorded: true,
                  auditEventId: 7,
                },
                200,
              ),
            )
        }),
    })
    renderAt('/admin/alerts/incidents/' + OPEN_INCIDENT_ID)
    fireEvent.click(
      await screen.findByRole('button', { name: 'Acknowledge Incident ' + OPEN_INCIDENT_ID }),
    )
    fireEvent.click(await screen.findByRole('button', { name: 'Confirm acknowledgment' }))

    // Navigate away while the acknowledgment request is still in flight.
    await act(async () => {
      window.history.pushState({}, '', '/admin/alerts/incidents/' + SECOND_INCIDENT_ID)
      window.dispatchEvent(new PopStateEvent('popstate'))
      await Promise.resolve()
    })
    await screen.findByRole('button', { name: 'Acknowledge Incident ' + SECOND_INCIDENT_ID })

    await act(async () => {
      releaseAcknowledgment?.()
      await Promise.resolve()
    })
    expect(screen.queryByText(/Acknowledged by admin/)).toBeNull()
    expect(
      screen.getByRole('button', { name: 'Acknowledge Incident ' + SECOND_INCIDENT_ID }),
    ).toBeTruthy()
  })

  it('keeps focus stable when an external acknowledgment removes the action', async () => {
    let acknowledged = false
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      ['/api/admin/v1/alerts/incidents/' + OPEN_INCIDENT_ID]: () =>
        jsonResponse(
          acknowledged
            ? {
                ...OPEN_INCIDENT_DETAIL,
                acknowledgment: {
                  acknowledgedByUsername: 'other-owner',
                  acknowledgedAt: '2026-03-01T00:15:00Z',
                  acknowledgedByUserId: 'u2',
                },
              }
            : OPEN_INCIDENT_DETAIL,
          200,
        ),
    })
    renderAt('/admin/alerts/incidents/' + OPEN_INCIDENT_ID)
    await screen.findByRole('heading', { level: 1, name: /Incident 0195f2a1/ })
    const entry = await screen.findByRole('button', {
      name: 'Acknowledge Incident ' + OPEN_INCIDENT_ID,
    })
    entry.focus()
    await waitFor(() => {
      expect(document.activeElement).toBe(entry)
    })

    // An ordinary background refresh must not steal focus.
    await act(async () => {
      await adminQueryClient.invalidateQueries()
    })
    await waitFor(() => {
      expect(document.activeElement).toBe(entry)
    })

    acknowledged = true
    await act(async () => {
      await adminQueryClient.invalidateQueries()
    })
    expect(await screen.findByText(/Acknowledged by other-owner/)).toBeTruthy()
    await waitFor(() => {
      expect(document.activeElement?.textContent).toContain('Incident acknowledgment')
    })
  })

  it('keeps one occurrence\'s confirmation in the panel after returning with a failed refetch', async () => {
    let releaseAcknowledgment: (() => void) | null = null
    let detailCalls = 0
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      ['/api/admin/v1/alerts/incidents/' + OPEN_INCIDENT_ID]: () => {
        detailCalls += 1
        return detailCalls === 1
          ? jsonResponse(OPEN_INCIDENT_DETAIL, 200)
          : jsonResponse(
              { error: { code: 'unavailable', message: 'Server database is unavailable' } },
              503,
            )
      },
      ['/api/admin/v1/alerts/incidents/' + SECOND_INCIDENT_ID]: () =>
        jsonResponse(SECOND_INCIDENT_DETAIL, 200),
      ['/api/admin/v1/alerts/incidents/' + OPEN_INCIDENT_ID + '/acknowledgments']: () =>
        new Promise<Response>((resolve) => {
          releaseAcknowledgment = () =>
            resolve(
              jsonResponse(
                {
                  incidentId: OPEN_INCIDENT_ID,
                  acknowledgment: {
                    acknowledgedByUsername: 'admin',
                    acknowledgedAt: '2026-03-01T00:20:00Z',
                    acknowledgedByUserId: 'u1',
                  },
                  recorded: true,
                  auditEventId: 7,
                },
                200,
              ),
            )
        }),
    })
    renderAt('/admin/alerts/incidents/' + OPEN_INCIDENT_ID)
    fireEvent.click(
      await screen.findByRole('button', { name: 'Acknowledge Incident ' + OPEN_INCIDENT_ID }),
    )
    fireEvent.click(await screen.findByRole('button', { name: 'Confirm acknowledgment' }))

    // Leave A while the request is in flight, then return to A whose refetch fails.
    await act(async () => {
      window.history.pushState({}, '', '/admin/alerts/incidents/' + SECOND_INCIDENT_ID)
      window.dispatchEvent(new PopStateEvent('popstate'))
      await Promise.resolve()
    })
    await screen.findByRole('button', { name: 'Acknowledge Incident ' + SECOND_INCIDENT_ID })
    await act(async () => {
      window.history.pushState({}, '', '/admin/alerts/incidents/' + OPEN_INCIDENT_ID)
      window.dispatchEvent(new PopStateEvent('popstate'))
      await Promise.resolve()
    })
    await screen.findByRole('heading', { level: 1, name: /Incident 0195f2a1/ })

    // The late success is the authoritative result; the panel must agree with the header.
    await act(async () => {
      releaseAcknowledgment?.()
      await Promise.resolve()
    })
    expect(await screen.findByText(/Acknowledged by admin at/)).toBeTruthy()
    expect(
      screen.queryByRole('button', { name: 'Acknowledge Incident ' + OPEN_INCIDENT_ID }),
    ).toBeNull()
    expect(screen.queryByText('Awaiting acknowledgment')).toBeNull()
  })

  it('keeps a recorded acknowledgment across route remounts when reads fail', async () => {
    let confirmed = false
    const unavailable = () =>
      jsonResponse(
        { error: { code: 'unavailable', message: 'Server database is unavailable' } },
        503,
      )
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      ['/api/admin/v1/alerts/incidents/' + OPEN_INCIDENT_ID]: () =>
        confirmed ? unavailable() : jsonResponse(OPEN_INCIDENT_DETAIL, 200),
      ['/api/admin/v1/alerts/incidents/' + OPEN_INCIDENT_ID + '/acknowledgments']: () => {
        confirmed = true
        return jsonResponse(
          {
            incidentId: OPEN_INCIDENT_ID,
            acknowledgment: {
              acknowledgedByUsername: 'admin',
              acknowledgedAt: '2026-03-01T00:20:00Z',
              acknowledgedByUserId: 'u1',
            },
            recorded: true,
            auditEventId: 7,
          },
          200,
        )
      },
      '/api/admin/v1/alerts/incidents*': () =>
        confirmed ? unavailable() : jsonResponse({ incidents: [OPEN_INCIDENT], total: 1 }, 200),
    })

    const navigate = async (destination: string) => {
      await act(async () => {
        window.history.pushState({}, '', destination)
        window.dispatchEvent(new PopStateEvent('popstate'))
        await Promise.resolve()
      })
    }

    renderAt('/admin/alerts/incidents')
    await screen.findByRole('heading', { level: 1, name: 'Incidents' })
    expect(await screen.findByText('Awaiting acknowledgment')).toBeTruthy()

    await navigate('/admin/alerts/incidents/' + OPEN_INCIDENT_ID)
    fireEvent.click(
      await screen.findByRole('button', { name: 'Acknowledge Incident ' + OPEN_INCIDENT_ID }),
    )
    fireEvent.click(await screen.findByRole('button', { name: 'Confirm acknowledgment' }))
    expect(await screen.findByText(/Acknowledged by admin at/)).toBeTruthy()

    // Both reads now fail; the recorded write must survive the route remount.
    await navigate('/admin/alerts/incidents')
    const table = await screen.findByRole('table', { name: /Incident history/ })
    expect(table.textContent).toContain('Acknowledged')
    expect(table.textContent).toContain('admin')
    expect(table.textContent).not.toContain('Awaiting acknowledgment')

    await navigate('/admin/alerts/incidents/' + OPEN_INCIDENT_ID)
    await screen.findByRole('heading', { level: 1, name: /Incident 0195f2a1/ })
    expect((await screen.findAllByText(/Acknowledged by admin at/)).length).toBeGreaterThan(0)
    expect(screen.queryByText('Awaiting acknowledgment')).toBeNull()
    expect(
      screen.queryByRole('button', { name: 'Acknowledge Incident ' + OPEN_INCIDENT_ID }),
    ).toBeNull()
  })

  it('does not move focus when an external acknowledgment arrives without panel focus', async () => {
    let acknowledged = false
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      ['/api/admin/v1/alerts/incidents/' + OPEN_INCIDENT_ID]: () =>
        jsonResponse(
          acknowledged
            ? {
                ...OPEN_INCIDENT_DETAIL,
                acknowledgment: {
                  acknowledgedByUsername: 'other-owner',
                  acknowledgedAt: '2026-03-01T00:15:00Z',
                  acknowledgedByUserId: 'u2',
                },
              }
            : OPEN_INCIDENT_DETAIL,
          200,
        ),
    })
    renderAt('/admin/alerts/incidents/' + OPEN_INCIDENT_ID)
    await screen.findByRole('heading', { level: 1, name: /Incident 0195f2a1/ })

    // The panel was never focused, so the browser's default BODY focus is not
    // evidence that the removed action held focus.
    acknowledged = true
    await act(async () => {
      await adminQueryClient.invalidateQueries()
    })
    expect(await screen.findByText(/Acknowledged by other-owner/)).toBeTruthy()
    expect(document.activeElement?.tagName).toBe('BODY')
  })

  it('keeps the occurrence on screen acknowledged when an earlier response lands late', async () => {
    let releaseAcknowledgment: (() => void) | null = null
    let bConfirmed = false
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      ['/api/admin/v1/alerts/incidents/' + OPEN_INCIDENT_ID]: () =>
        jsonResponse(OPEN_INCIDENT_DETAIL, 200),
      ['/api/admin/v1/alerts/incidents/' + SECOND_INCIDENT_ID]: () =>
        jsonResponse(
          bConfirmed
            ? { error: { code: 'unavailable', message: 'Server database is unavailable' } }
            : SECOND_INCIDENT_DETAIL,
          bConfirmed ? 503 : 200,
        ),
      ['/api/admin/v1/alerts/incidents/' + OPEN_INCIDENT_ID + '/acknowledgments']: () =>
        new Promise<Response>((resolve) => {
          releaseAcknowledgment = () =>
            resolve(
              jsonResponse(
                {
                  incidentId: OPEN_INCIDENT_ID,
                  acknowledgment: {
                    acknowledgedByUsername: 'admin',
                    acknowledgedAt: '2026-03-01T00:20:00Z',
                    acknowledgedByUserId: 'u1',
                  },
                  recorded: true,
                  auditEventId: 7,
                },
                200,
              ),
            )
        }),
      ['/api/admin/v1/alerts/incidents/' + SECOND_INCIDENT_ID + '/acknowledgments']: () => {
        bConfirmed = true
        return jsonResponse(
          {
            incidentId: SECOND_INCIDENT_ID,
            acknowledgment: {
              acknowledgedByUsername: 'admin',
              acknowledgedAt: '2026-03-01T00:21:00Z',
              acknowledgedByUserId: 'u1',
            },
            recorded: true,
            auditEventId: 8,
          },
          200,
        )
      },
    })
    renderAt('/admin/alerts/incidents/' + OPEN_INCIDENT_ID)
    fireEvent.click(
      await screen.findByRole('button', { name: 'Acknowledge Incident ' + OPEN_INCIDENT_ID }),
    )
    fireEvent.click(await screen.findByRole('button', { name: 'Confirm acknowledgment' }))

    // Leave A while its request is in flight, then confirm B.
    await act(async () => {
      window.history.pushState({}, '', '/admin/alerts/incidents/' + SECOND_INCIDENT_ID)
      window.dispatchEvent(new PopStateEvent('popstate'))
      await Promise.resolve()
    })
    fireEvent.click(
      await screen.findByRole('button', { name: 'Acknowledge Incident ' + SECOND_INCIDENT_ID }),
    )
    fireEvent.click(await screen.findByRole('button', { name: 'Confirm acknowledgment' }))
    expect(await screen.findByText(/Acknowledged by admin at 2026-03-01 00:21:00 UTC/)).toBeTruthy()

    // A's late success must not displace B, the occurrence still on screen: the
    // header badge and the panel read the same per-occurrence value.
    await act(async () => {
      releaseAcknowledgment?.()
      await Promise.resolve()
    })
    expect(screen.queryByText('Awaiting acknowledgment')).toBeNull()
    expect(screen.getAllByText(/Acknowledged by admin at 2026-03-01 00:21:00 UTC/).length).toBeGreaterThan(0)
    expect(screen.queryByText(/2026-03-01 00:20:00 UTC/)).toBeNull()
  })

  it('keeps focus at BODY after focus left the panel for an external element', async () => {
    let acknowledged = false
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      ['/api/admin/v1/alerts/incidents/' + OPEN_INCIDENT_ID]: () =>
        jsonResponse(
          acknowledged
            ? {
                ...OPEN_INCIDENT_DETAIL,
                acknowledgment: {
                  acknowledgedByUsername: 'other-owner',
                  acknowledgedAt: '2026-03-01T00:15:00Z',
                  acknowledgedByUserId: 'u2',
                },
              }
            : OPEN_INCIDENT_DETAIL,
          200,
        ),
    })
    renderAt('/admin/alerts/incidents/' + OPEN_INCIDENT_ID)
    await screen.findByRole('heading', { level: 1, name: /Incident 0195f2a1/ })
    const entry = await screen.findByRole('button', {
      name: 'Acknowledge Incident ' + OPEN_INCIDENT_ID,
    })
    entry.focus()
    await waitFor(() => {
      expect(document.activeElement).toBe(entry)
    })

    // Focus moves to an external element and is then released back to BODY.
    const external = document.createElement('input')
    document.body.append(external)
    external.focus()
    await waitFor(() => {
      expect(document.activeElement).toBe(external)
    })
    external.blur()
    await waitFor(() => {
      expect(document.activeElement?.tagName).toBe('BODY')
    })

    // An external acknowledgment arrives; because the most recent focus was
    // outside the panel, focus must not jump to the title.
    acknowledged = true
    await act(async () => {
      await adminQueryClient.invalidateQueries()
    })
    expect(await screen.findByText(/Acknowledged by other-owner/)).toBeTruthy()
    expect(document.activeElement?.tagName).toBe('BODY')
    external.remove()
  })
})
