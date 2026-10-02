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

const RULE_KEY = 'node.rpc_unreachable'

const RULE_SUMMARY = {
  ruleKey: RULE_KEY,
  subjectKind: 'node',
  enabled: true,
  severity: 'warning',
  version: 3,
  condition: { for_secs: 60, recovery_for_secs: 120 },
  schema: [
    { key: 'for_secs', label: 'Sustained for', unit: 'seconds', min: 0, max: 86400, default: 60, description: 'Firing duration' },
    { key: 'recovery_for_secs', label: 'Recovery for', unit: 'seconds', min: 0, max: 86400, default: 120, description: 'Recovery duration' },
  ],
  createdAt: '2026-03-01T00:00:00Z',
  updatedAt: '2026-03-02T00:00:00Z',
  openIncidents: 1,
  evaluation: {
    subjects: 1,
    normal: 0,
    firing: 1,
    pending: 0,
    recovering: 0,
    evaluationUnavailable: false,
  },
}

const RULE_DETAIL = {
  ...RULE_SUMMARY,
  versions: [
    { version: 3, severity: 'warning', condition: { for_secs: 60, recovery_for_secs: 120 }, createdAt: '2026-03-02T00:00:00Z' },
    { version: 1, severity: 'warning', condition: { for_secs: 30, recovery_for_secs: 60 }, createdAt: '2026-03-01T00:00:00Z' },
  ],
  overrides: [
    {
      scopeKind: 'network',
      scopeValue: 'platon-mainnet',
      enabled: null,
      severity: 'critical',
      condition: null,
      updatedAt: '2026-03-02T00:00:00Z',
    },
  ],
  states: [
    {
      subjectKind: 'node',
      subjectKey: 'node-a',
      state: 'firing',
      evaluationUnavailable: false,
      inputKind: 'boolean',
      inputValue: 1,
      inputDetail: 'connect refused',
      lastEvaluatedAt: '2026-03-02T00:00:00Z',
      since: '2026-03-02T00:00:00Z',
      firingSince: '2026-03-02T00:00:00Z',
      pendingSince: null,
      recoveringSince: null,
      openIncidents: 1,
    },
  ],
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

describe('PAGE-ADMIN-RULES (Alert Rule catalog)', () => {
  it('lists the typed catalog with version, state, severity, open Incidents, and evaluations', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/alerts/rules': () => jsonResponse([RULE_SUMMARY], 200),
    })
    renderAt('/admin/alerts/rules')

    await screen.findByRole('heading', { level: 1, name: 'Alert Rules' })
    const table = await screen.findByRole('table', { name: 'Alert Rules' })
    expect(table.textContent).toContain(RULE_KEY)
    expect(table.textContent).toContain('Enabled')
    expect(table.textContent).toContain('Warning')
    expect(table.textContent).toContain('1 / 0')
    expect(screen.getByRole('link', { name: RULE_KEY }).getAttribute('href')).toBe(
      '/admin/alerts/rules/' + RULE_KEY,
    )
  })
})

describe('PAGE-ADMIN-RULE-DETAIL (inherited configuration and version-safe editing)', () => {
  it('reads the baseline, the inherited override fields, and the immutable history', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      ['/api/admin/v1/alerts/rules/' + RULE_KEY]: () => jsonResponse(RULE_DETAIL, 200),
    })
    renderAt('/admin/alerts/rules/' + RULE_KEY)

    await screen.findByRole('heading', { level: 1, name: 'Alert Rule ' + RULE_KEY })
    expect(screen.getAllByText(/unset override fields inherit the layer below/).length).toBeGreaterThan(0)
    const overrides = await screen.findByRole('table', { name: 'Rule overrides' })
    expect(overrides.textContent).toContain('platon-mainnet')
    expect(overrides.textContent).toContain('Critical')
    // A null override field is shown as inherited, never as a defaulted value.
    expect(overrides.textContent).toContain('Inherited')
    const versions = screen.getByRole('table', { name: 'Rule versions' })
    expect(versions.textContent).toContain('3')
    expect(versions.textContent).toContain('Saved')
  })

  it('saves a typed edit under the composed version and reports the new version', async () => {
    const puts: Array<Record<string, unknown>> = []
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      ['/api/admin/v1/alerts/rules/' + RULE_KEY]: async (request) => {
        if (request.method === 'PUT') {
          puts.push((await request.json()) as Record<string, unknown>)
          return jsonResponse({ auditEventId: 7, rule: { ...RULE_DETAIL, version: 4 } }, 200)
        }
        return jsonResponse(RULE_DETAIL, 200)
      },
    })
    renderAt('/admin/alerts/rules/' + RULE_KEY)

    await screen.findByRole('heading', { level: 1, name: 'Alert Rule ' + RULE_KEY })
    fireEvent.change(screen.getByLabelText('Rule severity'), { target: { value: 'critical' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save Rule' }))

    await screen.findByText('Saved as version 4.')
    expect(puts).toHaveLength(1)
    expect(puts[0].expectedVersion).toBe(3)
    expect(puts[0].enabled).toBe(true)
    expect(puts[0].severity).toBe('critical')
    expect(puts[0].condition).toEqual({ for_secs: 60, recovery_for_secs: 120 })
  })

  it('requires a reload and review when the Server rejects a stale save', async () => {
    let version = 3
    const puts: Array<Record<string, unknown>> = []
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      ['/api/admin/v1/alerts/rules/' + RULE_KEY]: async (request) => {
        if (request.method === 'PUT') {
          puts.push((await request.json()) as Record<string, unknown>)
          return jsonResponse(
            {
              error: {
                code: 'alert_rule_version_conflict',
                message:
                  'the rule changed since it was read; reload the current configuration and review before saving',
                requestId: 'req-1',
                fields: [],
              },
            },
            409,
          )
        }
        return jsonResponse({ ...RULE_DETAIL, version }, 200)
      },
    })
    renderAt('/admin/alerts/rules/' + RULE_KEY)

    await screen.findByRole('heading', { level: 1, name: 'Alert Rule ' + RULE_KEY })
    fireEvent.click(screen.getByRole('button', { name: 'Save Rule' }))

    await screen.findByText('This Rule changed since you read it')
    expect((screen.getByRole('button', { name: 'Save Rule' }) as HTMLButtonElement).disabled).toBe(true)
    expect(puts[0].expectedVersion).toBe(3)

    // Another Owner saved version 4; the reload adopts the new composed version
    // and only then re-enables saving.
    version = 4
    fireEvent.click(screen.getByRole('button', { name: /Reload current configuration/ }))
    await waitFor(() => {
      expect(screen.getByText(/composed version 4/)).toBeTruthy()
    })
    expect((screen.getByRole('button', { name: 'Save Rule' }) as HTMLButtonElement).disabled).toBe(false)
  })

  it('previews unsaved changes without sending a version and without promising an Incident', async () => {
    const previews: Array<Record<string, unknown>> = []
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      ['/api/admin/v1/alerts/rules/' + RULE_KEY + '/preview']: async (request) => {
        previews.push((await request.json()) as Record<string, unknown>)
        return jsonResponse(
          {
            ...RULE_DETAIL,
            subjects: [
              {
                subjectKind: 'node',
                subjectKey: 'node-a',
                currentState: 'normal',
                projectedState: 'pending',
                wouldFire: false,
                note: 'would need 60s sustained before firing',
              },
            ],
          },
          200,
        )
      },
      ['/api/admin/v1/alerts/rules/' + RULE_KEY]: () => jsonResponse(RULE_DETAIL, 200),
    })
    renderAt('/admin/alerts/rules/' + RULE_KEY)

    await screen.findByRole('heading', { level: 1, name: 'Alert Rule ' + RULE_KEY })
    fireEvent.click(screen.getByRole('button', { name: 'Preview unsaved changes' }))

    await screen.findByText('would need 60s sustained before firing')
    expect('expectedVersion' in previews[0]).toBe(false)
    expect(screen.getByText(/It is not a save: it creates, resolves, and acknowledges nothing/)).toBeTruthy()
    const subjects = screen.getByRole('table', { name: 'Previewed subjects' })
    expect(subjects.textContent).toContain('pending')
  })

  it('refuses an override that inherits everything and saves a narrowed one under the version', async () => {
    const upserts: Array<Record<string, unknown>> = []
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      ['/api/admin/v1/alerts/rules/' + RULE_KEY + '/overrides']: async (request) => {
        upserts.push((await request.json()) as Record<string, unknown>)
        return jsonResponse(
          { ruleKey: RULE_KEY, version: 4, overrides: RULE_DETAIL.overrides },
          200,
        )
      },
      ['/api/admin/v1/alerts/rules/' + RULE_KEY]: () => jsonResponse(RULE_DETAIL, 200),
    })
    renderAt('/admin/alerts/rules/' + RULE_KEY)

    await screen.findByRole('heading', { level: 1, name: 'Alert Rule ' + RULE_KEY })
    fireEvent.change(screen.getByLabelText('Override scope value'), { target: { value: 'node-a' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save override' }))
    await screen.findByText(
      'Override at least one field; an override with every field inherited is not allowed.',
    )
    expect(upserts).toHaveLength(0)

    fireEvent.change(screen.getByLabelText('Override severity'), { target: { value: 'critical' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save override' }))
    await screen.findByText(
      'Override saved as composed version 4. A reader holding the previous version must reload before saving.',
    )
    // The override write advanced the composed version, and the baseline form
    // adopts it rather than later sending a version the Server superseded
    // (issue #204 review, Spec FINDING 1).
    expect(screen.getByText(/This save carries composed version 4\./)).toBeTruthy()
    expect(upserts).toHaveLength(1)
    expect(upserts[0].expectedVersion).toBe(3)
    expect(upserts[0].scopeKind).toBe('network')
    expect(upserts[0].scopeValue).toBe('node-a')
    expect(upserts[0].severity).toBe('critical')
    expect(upserts[0].enabled).toBeNull()
    expect(upserts[0].condition).toBeNull()
  })

  it('states that a disabled Rule neither resolves nor acknowledges an existing Incident', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      ['/api/admin/v1/alerts/rules/' + RULE_KEY]: () =>
        jsonResponse(
          {
            ...RULE_DETAIL,
            enabled: false,
            version: 4,
            versions: [
              { version: 4, severity: 'warning', condition: { for_secs: 60, recovery_for_secs: 120 }, createdAt: '2026-03-03T00:00:00Z' },
              ...RULE_DETAIL.versions,
            ],
          },
          200,
        ),
    })
    renderAt('/admin/alerts/rules/' + RULE_KEY)

    await screen.findByRole('heading', { level: 1, name: 'Alert Rule ' + RULE_KEY })
    expect(
      screen.getByText(/Disabling does not resolve, retract, or acknowledge any existing Incident/),
    ).toBeTruthy()
  })

  it('reports an unknown Rule key as not found instead of a blank surface', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      ['/api/admin/v1/alerts/rules/no.such_rule']: () =>
        jsonResponse({ error: { code: 'alert_rule_not_found', message: 'no such rule' } }, 404),
    })
    renderAt('/admin/alerts/rules/no.such_rule')

    await screen.findByRole('heading', { level: 1, name: 'Alert Rule not found' })
    expect(screen.getByRole('link', { name: 'Back to Alert Rules' })).toBeTruthy()
  })
})
