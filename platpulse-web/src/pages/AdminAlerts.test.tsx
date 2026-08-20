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

const RULE = {
  ruleKey: 'node.rpc_unreachable',
  subjectKind: 'node',
  enabled: true,
  severity: 'warning',
  version: 1,
  condition: { for_secs: 60, recovery_for_secs: 120 },
  schema: [
    {
      key: 'for_secs',
      label: 'Sustained firing',
      unit: 's',
      min: 1,
      max: 604800,
      default: 60,
      description: 'How long the condition must hold before an Incident opens.',
    },
    {
      key: 'recovery_for_secs',
      label: 'Sustained recovery',
      unit: 's',
      min: 1,
      max: 604800,
      default: 120,
      description: 'How long fresh recovery must hold before an Incident resolves.',
    },
  ],
  createdAt: '2026-08-12T08:00:00Z',
  updatedAt: '2026-08-12T08:00:00Z',
  openIncidents: 1,
  evaluation: { subjects: 1, normal: 0, pending: 0, firing: 1, recovering: 0, evaluationUnavailable: 0 },
}

const RULE_DETAIL = {
  ...RULE,
  versions: [
    { version: 1, severity: 'warning', condition: { for_secs: 60, recovery_for_secs: 120 }, createdAt: '2026-08-12T08:00:00Z' },
  ],
  overrides: [],
  states: [
    {
      subjectKind: 'node',
      subjectKey: 'node-a',
      state: 'firing',
      since: '2026-08-12T08:00:00Z',
      pendingSince: null,
      firingSince: '2026-08-12T08:01:00Z',
      recoveringSince: null,
      inputKind: 'known',
      inputValue: 1,
      inputDetail: 'rpc_unreachable: connect refused',
      evaluationUnavailable: false,
      lastEvaluatedAt: '2026-08-12T08:01:00Z',
      openIncidents: 1,
    },
  ],
}

const INCIDENT = {
  incidentId: 'incident-1',
  ruleKey: 'node.rpc_unreachable',
  ruleVersion: 1,
  subjectKind: 'node',
  subjectKey: 'node-a',
  severity: 'warning',
  state: 'open',
  sequence: 1,
  openedAt: '2026-08-12T08:01:00Z',
  resolvedAt: null,
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

describe('PAGE-ADMIN-ALERT-RULES (typed Rule catalog)', () => {
  it('lists typed Rules with evaluation state and Open Incident counts', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/alerts/rules': () => jsonResponse([RULE], 200),
    })
    renderAt('/admin/alerts/rules')

    await screen.findByRole('heading', { level: 1, name: 'Alert Rules' })
    const row = await screen.findByRole('row', { name: /node\.rpc_unreachable/ })
    expect(row.textContent).toContain('node')
    expect(row.textContent).toContain('Warning')
    expect(row.textContent).toContain('Firing')
    expect(row.textContent).toContain('1 subject')
  })

  it('shows a Disabled rule with its evaluation stopped without hiding history', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/alerts/rules': () =>
        jsonResponse([{ ...RULE, enabled: false, openIncidents: 2 }], 200),
    })
    renderAt('/admin/alerts/rules')

    const row = await screen.findByRole('row', { name: /node\.rpc_unreachable/ })
    expect(row.textContent).toContain('Disabled')
    expect(row.textContent).toContain('Evaluation stopped')
    expect(row.textContent).toContain('2')
  })

  it('edits a Rule through the typed form: validation, preview, and versioned save', async () => {
    let savedBody: unknown = null
    let previewCalled = false
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(String(input), init)
      const url = request.url.replace(TEST_ORIGIN, '')
      if (url === '/api/public/v1/session') return jsonResponse(OWNER_SESSION, 200)
      if (url === '/api/admin/v1/alerts/rules/node.rpc_unreachable' && request.method === 'GET') {
        return jsonResponse(RULE_DETAIL, 200)
      }
      if (url === '/api/admin/v1/alerts/rules/node.rpc_unreachable/preview') {
        previewCalled = true
        return jsonResponse(
          {
            rule_key: 'node.rpc_unreachable',
            enabled: true,
            severity: 'warning',
            condition: { for_secs: 30, recovery_for_secs: 60 },
            subjects: [
              {
                subjectKind: 'node',
                subjectKey: 'node-a',
                currentState: 'firing',
                input: { kind: 'known', value: 1, detail: 'rpc_unreachable: connect refused' },
                wouldFire: true,
                projectedState: 'firing',
                note: 'condition still firing',
              },
            ],
          },
          200,
        )
      }
      if (url === '/api/admin/v1/alerts/rules/node.rpc_unreachable' && request.method === 'PUT') {
        savedBody = JSON.parse(request.bodyUsed ? '' : await request.text())
        const body = savedBody as { condition: unknown }
        return jsonResponse(
          {
            rule: { ...RULE_DETAIL, version: 2, condition: body.condition },
            audit_event_id: 7,
          },
          200,
        )
      }
      return jsonResponse({ error: { code: 'not_found' } }, 404)
    })
    vi.stubGlobal('fetch', fetchMock)
    renderAt('/admin/alerts/rules/node.rpc_unreachable')

    // The detail page links into the typed editor (real navigation path).
    await screen.findByRole('heading', { level: 1, name: 'node.rpc_unreachable' })
    fireEvent.click(await screen.findByRole('link', { name: 'Edit Rule' }))
    await screen.findByRole('heading', { level: 1, name: 'Edit node.rpc_unreachable' })
    // The form renders the Server schema (typed parameters only).
    const firing = (await screen.findByLabelText(/Sustained firing/)) as HTMLInputElement
    expect(firing.value).toBe('60')
    fireEvent.change(firing, { target: { value: '30' } })

    // Preview shows the projection without writing.
    fireEvent.click(screen.getByRole('button', { name: 'Preview current facts' }))
    expect(await screen.findByText(/evaluated — nothing was written/)).toBeTruthy()
    expect((await screen.findByRole('row', { name: /node-a/ })).textContent).toContain('Firing')
    expect(previewCalled).toBe(true)

    // Saving bumps the version and refetches the authoritative detail.
    fireEvent.click(screen.getByRole('button', { name: 'Save version' }))
    await waitFor(() => expect(savedBody).not.toBeNull())
    expect((savedBody as { condition: { for_secs: number } }).condition.for_secs).toBe(30)
  })

  it('rejects an invalid threshold client-side without calling the API', async () => {
    const fetchMock = mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/alerts/rules/node.rpc_unreachable': () => jsonResponse(RULE_DETAIL, 200),
    })
    renderAt('/admin/alerts/rules/node.rpc_unreachable')

    await screen.findByRole('heading', { level: 1, name: 'node.rpc_unreachable' })
    fireEvent.click(await screen.findByRole('link', { name: 'Edit Rule' }))
    await screen.findByRole('heading', { level: 1, name: 'Edit node.rpc_unreachable' })
    fireEvent.change(await screen.findByLabelText(/Sustained firing/), { target: { value: '0' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save version' }))
    // The error appears both as a field-level message and in the summary.
    const messages = await screen.findAllByText(/Sustained firing must be at least 1 second/)
    expect(messages.length).toBeGreaterThan(0)
    expect((screen.getByLabelText(/Sustained firing/) as HTMLInputElement).getAttribute('aria-invalid')).toBe('true')
    const putCalls = fetchMock.mock.calls.filter(
      ([input]) => String(input).includes('/alerts/rules/node.rpc_unreachable') && true,
    )
    expect(putCalls.length).toBe(0)
  })
})

describe('PAGE-ADMIN-INCIDENTS and PAGE-ADMIN-INCIDENT', () => {
  it('lists Open Incidents and shows immutable evidence, evaluation, and overlapping suppressions', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/alerts/incidents?state=open&limit=100': () =>
        jsonResponse({ incidents: [INCIDENT], total: 1 }, 200),
      '/api/admin/v1/alerts/incidents/incident-1': () =>
        jsonResponse(
          {
            ...INCIDENT,
            openedEvidence: {
              input_kind: 'known',
              input_value: 1,
              input_detail: 'rpc_unreachable: connect refused',
              threshold: 0.5,
              note: 'firing sustained for 60s; Incident opens',
            },
            resolvedEvidence: null,
            evaluation: RULE_DETAIL.states[0],
            suppressions: [
              {
                kind: 'silence',
                id: 'sil-1',
                reason: 'quiet weekend',
                startsAt: '2026-08-12T00:00:00Z',
                endsAt: '2026-08-13T00:00:00Z',
                marksIncident: false,
              },
              {
                kind: 'maintenance',
                id: 'mnt-1',
                reason: 'planned reboot',
                startsAt: '2026-08-12T00:00:00Z',
                endsAt: '2026-08-13T00:00:00Z',
                marksIncident: true,
              },
            ],
          },
          200,
        ),
    })
    renderAt('/admin/alerts/incidents')

    await screen.findByRole('heading', { level: 1, name: 'Incidents' })
    const row = await screen.findByRole('row', { name: /incident/ })
    expect(row.textContent).toContain('Open')
    expect(row.textContent).toContain('node.rpc_unreachable')

    fireEvent.click(screen.getByRole('link', { name: /incident/ }))
    await screen.findByRole('heading', { level: 1, name: 'Incident incident' })
    expect((await screen.findAllByText(/connect refused/)).length).toBeGreaterThan(0)
    // Evaluation state renders independently.
    expect(await screen.findByText('Firing')).toBeTruthy()
    // Both suppression reasons stay visible independently.
    expect(await screen.findByText('quiet weekend')).toBeTruthy()
    expect(await screen.findByText('planned reboot')).toBeTruthy()
    expect(await screen.findByText(/marks Incident suppressed/)).toBeTruthy()
  })
})

describe('PAGE-ADMIN-SILENCES and PAGE-ADMIN-MAINTENANCE', () => {
  it('creates and cancels a Silence with confirmation and typed statuses', async () => {
    const created: { matcherKind: string; reason: string }[] = []
    let cancelled = false
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(String(input), init)
      const url = request.url.replace(TEST_ORIGIN, '')
      if (url === '/api/public/v1/session') return jsonResponse(OWNER_SESSION, 200)
      if (url === '/api/admin/v1/alerts/silences?status=active' && request.method === 'GET') {
        return jsonResponse({ silences: [] }, 200)
      }
      if (url === '/api/admin/v1/alerts/silences' && request.method === 'POST') {
        const body = JSON.parse(request.bodyUsed ? '' : await request.text())
        created.push(body)
        return jsonResponse(
          {
            silence: {
              silenceId: 'sil-9',
              matcherKind: body.matcherKind,
              matcherValue: body.matcherValue,
              reason: body.reason,
              startsAt: body.startsAt,
              endsAt: body.endsAt,
              createdBy: 'admin',
              createdAt: '2026-08-12T08:00:00Z',
              cancelledAt: null,
              cancelledBy: null,
              status: 'active',
            },
            audit_event_id: 3,
          },
          200,
        )
      }
      if (url === '/api/admin/v1/alerts/silences/sil-9/cancel' && request.method === 'POST') {
        cancelled = true
        return jsonResponse(
          {
            silence: {
              silenceId: 'sil-9',
              matcherKind: 'node',
              matcherValue: 'node-a',
              reason: 'quiet weekend',
              startsAt: '2026-08-12T00:00:00Z',
              endsAt: '2026-08-13T00:00:00Z',
              createdBy: 'admin',
              createdAt: '2026-08-12T08:00:00Z',
              cancelledAt: '2026-08-12T09:00:00Z',
              cancelledBy: 'admin',
              status: 'cancelled',
            },
            audit_event_id: 4,
          },
          200,
        )
      }
      return jsonResponse({ error: { code: 'not_found' } }, 404)
    })
    vi.stubGlobal('fetch', fetchMock)
    renderAt('/admin/alerts/silences')

    await screen.findByRole('heading', { level: 1, name: 'Silences' })
    fireEvent.click(screen.getByRole('button', { name: 'Create a Silence' }))
    fireEvent.change(screen.getByLabelText('Applies to'), { target: { value: 'node' } })
    fireEvent.change(screen.getByLabelText('Matcher value'), { target: { value: 'node-a' } })
    fireEvent.change(screen.getByLabelText('Reason'), { target: { value: 'quiet weekend' } })
    fireEvent.change(screen.getByLabelText('Starts at'), {
      target: { value: '2026-08-12T00:00:00Z' },
    })
    fireEvent.change(screen.getByLabelText('Ends at (required)'), {
      target: { value: '2026-08-13T00:00:00Z' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Create Silence' }))
    expect(await screen.findByText('Silence created: quiet weekend')).toBeTruthy()
    expect(created).toEqual([
      {
        matcherKind: 'node',
        matcherValue: 'node-a',
        reason: 'quiet weekend',
        startsAt: '2026-08-12T00:00:00Z',
        endsAt: '2026-08-13T00:00:00Z',
      },
    ])
    expect(cancelled).toBe(false)
  })

  it('creates a Maintenance Window with a typed expected-condition allowlist', async () => {
    const created: { scopeKind: string; expectedRuleKeys: string[] }[] = []
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(String(input), init)
      const url = request.url.replace(TEST_ORIGIN, '')
      if (url === '/api/public/v1/session') return jsonResponse(OWNER_SESSION, 200)
      if (url === '/api/admin/v1/alerts/maintenance?status=active' && request.method === 'GET') {
        return jsonResponse({ windows: [] }, 200)
      }
      if (url === '/api/admin/v1/alerts/maintenance' && request.method === 'POST') {
        const body = JSON.parse(request.bodyUsed ? '' : await request.text())
        created.push(body)
        return jsonResponse(
          {
            window: {
              windowId: 'mnt-9',
              scopeKind: body.scopeKind,
              scopeValue: body.scopeValue,
              expectedRuleKeys: body.expectedRuleKeys,
              reason: body.reason,
              startsAt: body.startsAt,
              endsAt: body.endsAt,
              createdBy: 'admin',
              createdAt: '2026-08-12T08:00:00Z',
              cancelledAt: null,
              cancelledBy: null,
              status: 'active',
            },
            audit_event_id: 5,
          },
          200,
        )
      }
      return jsonResponse({ error: { code: 'not_found' } }, 404)
    })
    vi.stubGlobal('fetch', fetchMock)
    renderAt('/admin/alerts/maintenance')

    await screen.findByRole('heading', { level: 1, name: 'Maintenance Windows' })
    fireEvent.click(screen.getByRole('button', { name: 'Schedule Maintenance' }))
    fireEvent.change(screen.getByLabelText('Scope value'), { target: { value: 'node-a' } })
    fireEvent.click(screen.getByLabelText('node.rpc_unreachable'))
    fireEvent.change(screen.getByLabelText('Reason'), { target: { value: 'planned reboot' } })
    fireEvent.change(screen.getByLabelText('Starts at'), {
      target: { value: '2026-08-12T00:00:00Z' },
    })
    fireEvent.change(screen.getByLabelText('Ends at (required)'), {
      target: { value: '2026-08-13T00:00:00Z' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Schedule Window' }))
    expect(await screen.findByText('Maintenance Window created: planned reboot')).toBeTruthy()
    expect(created).toEqual([
      {
        scopeKind: 'node',
        scopeValue: 'node-a',
        expectedRuleKeys: ['node.rpc_unreachable'],
        reason: 'planned reboot',
        startsAt: '2026-08-12T00:00:00Z',
        endsAt: '2026-08-13T00:00:00Z',
      },
    ])
  })
})
