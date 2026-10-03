import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import App from '../App'
import { adminQueryClient } from '../api/admin'
import { client } from '../api/generated/client.gen'
import { formatObservedAt } from '../components/StatusBadge'

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

const RAW = 'raw_block_summary'
const AGG = 'one_minute_aggregate'
const RAW_LABEL = 'Raw Block Summaries'
const AGG_LABEL = '1-Minute Aggregates'

/** 64-character policy fingerprints whose short form stays readable here. */
const RAW_VERSION = 'a1b2c3d4e5f60718293a4b5c6d7e8f90'.repeat(2).slice(0, 60) + '0001'
const RAW_VERSION_SHORT = 'a1b2c3d4…0001'
const OTHER_VERSION = 'f0f0f0f0e5f60718293a4b5c6d7e8f90'.repeat(2).slice(0, 60) + '0002'
const AGG_VERSION = 'b1b2c3d4e5f60718293a4b5c6d7e8f90'.repeat(2).slice(0, 60) + '0003'

const PREVIEW_ID = '0195f2a1-0400-4100-8100-000000000400'
const SECOND_PREVIEW_ID = '0195f2a1-0401-4101-8101-000000000401'
const RUN_ID = '0195f2a1-0402-4102-8102-000000000402'
const PREVIEW_SHORT = '0195f2a1…0400'
const RUN_SHORT = '0195f2a1…0402'

/** Fixture instants are relative to the real clock so expiry is deterministic. */
const NOW = Date.now()
function iso(offsetMs: number): string {
  return new Date(NOW + offsetMs).toISOString()
}

const PROTECTED = [
  'Audit Events are never released by a retention run.',
  'History gaps are never released by a retention run.',
]

type Policy = {
  family: string
  label: string
  retentionDays: number
  minDays: number
  maxDays: number
  defaultDays: number
  supported: boolean
  enabled: boolean
  updatedAt: string
  updatedBy: string | null
  policyVersion: string
}

function rawPolicy(overrides: Partial<Policy> = {}): Policy {
  return {
    family: RAW,
    label: RAW_LABEL,
    retentionDays: 7,
    minDays: 1,
    maxDays: 30,
    defaultDays: 7,
    supported: true,
    enabled: true,
    updatedAt: '2026-03-01T00:00:00Z',
    updatedBy: 'admin',
    policyVersion: RAW_VERSION,
    ...overrides,
  }
}

/** The consolidation family the Server does not implement cleanup for. */
function aggregatePolicy(): Policy {
  return {
    family: AGG,
    label: AGG_LABEL,
    retentionDays: 90,
    minDays: 7,
    maxDays: 365,
    defaultDays: 90,
    supported: false,
    enabled: true,
    updatedAt: '2026-03-01T00:00:00Z',
    updatedBy: null,
    policyVersion: AGG_VERSION,
  }
}

function livePreview(overrides: Record<string, unknown> = {}) {
  return {
    previewId: PREVIEW_ID,
    createdAt: iso(-60_000),
    createdBy: 'admin',
    expiresAt: iso(3_600_000),
    policyVersion: RAW_VERSION,
    scope: null,
    families: [
      {
        family: RAW,
        retentionDays: 7,
        policyVersion: RAW_VERSION,
        cutoff: iso(-7 * 86_400_000),
        estimatedRows: 1400,
      },
    ],
    skipped: [{ family: AGG, code: 'unsupported', message: 'The Server implements no cleanup for this family.' }],
    estimatedRows: 1400,
    notes: ['Estimates are upper bounds computed when this preview was composed.'],
    protectedState: PROTECTED,
    ...overrides,
  }
}

function runSummary(overrides: Record<string, unknown> = {}) {
  return {
    operationId: RUN_ID,
    kind: 'retention_run',
    status: 'queued',
    progressPercent: 0,
    progressLabel: null,
    requestId: 'req-run',
    createdAt: iso(-1_000),
    startedAt: null,
    finishedAt: null,
    auditEventId: 88,
    cancelRequested: false,
    ...overrides,
  }
}

/** The Server state each mocked route answers from. */
type ServerState = {
  policies: Policy[]
  preview: unknown | null
  lastRun: unknown | null
}

function newState(): ServerState {
  return { policies: [rawPolicy(), aggregatePolicy()], preview: null, lastRun: null }
}

function overviewBody(state: ServerState) {
  return {
    policies: state.policies,
    protectedState: PROTECTED,
    lastRun: state.lastRun,
    preview: state.preview,
  }
}

function jsonResponse(body: unknown, status: number): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

function apiError(code: string, message: string, status: number, fields: string[] = []): Response {
  return jsonResponse({ error: { code, message, requestId: 'req-err', fields } }, status)
}

const TEST_ORIGIN = 'http://platpulse.test'

type RecordedRequest = { key: string; method: string; path: string; body: unknown }
const requests: RecordedRequest[] = []

/** One mocked Server route: receives the request and its parsed JSON body. */
type Route = (request: Request, body: unknown) => Response | Promise<Response>

/**
 * Route table keyed by "METHOD /path". The handler receives the parsed body, and
 * every request is recorded so a test can assert the exact command the page
 * sent (and that it never re-sent a refused one).
 */
function mockRetention(routes: Record<string, Route>) {
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(String(input), init)
    const url = request.url.replace(TEST_ORIGIN, '')
    const path = url.split('?')[0]
    const text = await request.clone().text()
    let body: unknown = null
    if (text.length > 0) {
      try {
        body = JSON.parse(text)
      } catch {
        body = text
      }
    }
    const key = request.method + ' ' + path
    requests.push({ key, method: request.method, path, body })
    const handler = routes[key]
    if (handler) return handler(request, body)
    return jsonResponse({ error: { code: 'not_found' } }, 404)
  })
  vi.stubGlobal('fetch', fetchMock)
  return fetchMock
}

function calls(key: string): RecordedRequest[] {
  return requests.filter((entry) => entry.key === key)
}

function slot(name: string): HTMLElement {
  const element = document.querySelector<HTMLElement>('[data-slot="' + name + '"]')
  if (!element) throw new Error('missing data-slot ' + name)
  return element
}

const OVERVIEW_KEY = 'GET /api/admin/v1/retention'
const IMPACT_KEY = 'POST /api/admin/v1/retention/impact'
const PREVIEW_KEY = 'POST /api/admin/v1/retention/preview'
const RUN_KEY = 'POST /api/admin/v1/retention/run'
const POLICY_KEY = 'PUT /api/admin/v1/retention/policies/' + RAW

/** The four routes every case needs; individual cases add the mutating ones. */
function baseRoutes(state: ServerState, overrides: Record<string, Route> = {}) {
  return {
    'GET /api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
    [OVERVIEW_KEY]: () => jsonResponse(overviewBody(state), 200),
    ...overrides,
  }
}

function impactRoute(estimatedPerDay = 100) {
  return (_request: Request, body: unknown) => {
    const days = (body as { retentionDays: number }).retentionDays
    return jsonResponse(
      {
        family: RAW,
        retentionDays: days,
        estimatedRows: days * estimatedPerDay,
        unsupported: false,
        bounds: { minDays: 1, maxDays: 30 },
        notes: [],
      },
      200,
    )
  }
}

async function renderAt(path: string) {
  render(<App />)
  await act(async () => {
    window.history.pushState({}, '', path)
    window.dispatchEvent(new PopStateEvent('popstate'))
    await Promise.resolve()
  })
}

async function renderRetention() {
  await renderAt('/admin/retention')
  await screen.findByRole('heading', { level: 1, name: 'Retention' })
}

async function openEditor() {
  fireEvent.click(screen.getByRole('button', { name: 'Edit ' + RAW_LABEL + ' retention' }))
  await screen.findByRole('heading', { level: 2, name: 'Edit ' + RAW_LABEL + ' retention' })
}

function saveButton(): HTMLButtonElement {
  return screen.getByRole('button', { name: 'Save retention' }) as HTMLButtonElement
}

/** Fill the bounded edit and wait until the Server's estimate has settled. */
async function fillEdit(days: number, confirmation: string) {
  fireEvent.change(screen.getByLabelText('New retention (days)'), { target: { value: String(days) } })
  fireEvent.change(screen.getByLabelText('Type the change to confirm'), { target: { value: confirmation } })
  await waitFor(() => expect(saveButton().hasAttribute('disabled')).toBe(false))
}

beforeEach(() => {
  requests.length = 0
  window.history.replaceState({}, '', '/')
  client.setConfig({ baseUrl: TEST_ORIGIN })
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
  vi.unstubAllGlobals()
  adminQueryClient.clear()
})

describe('PAGE-ADMIN-RETENTION (issue #210)', () => {
  it('reads the policy table and claims no retention for a family the Server does not implement', async () => {
    const state = newState()
    mockRetention(baseRoutes(state))
    await renderRetention()

    const table = await screen.findByRole('table', { name: /Retention policies:/ })
    expect(table.textContent).toContain(RAW_LABEL)
    expect(table.textContent).toContain(RAW)
    expect(table.textContent).toContain('7 days')
    expect(table.textContent).toContain('1 day – 30 days')
    expect(table.textContent).toContain(AGG_LABEL)
    expect(table.textContent).toContain('Unsupported')
    expect(table.textContent).toContain(
      'The Server implements no cleanup for this family, so no retention is claimed for it.',
    )
    // Only a supported, enabled family is actionable.
    expect(screen.getByRole('button', { name: 'Edit ' + RAW_LABEL + ' retention' })).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Edit ' + AGG_LABEL + ' retention' })).toBeNull()
    expect(screen.getByText(PROTECTED[0])).toBeTruthy()
    expect(screen.getByText(PROTECTED[1])).toBeTruthy()
    expect(screen.getByText(/recorded no retention run yet/)).toBeTruthy()
  })

  it("shows the Server's own refusal of a value below the floor and gates the save", async () => {
    const state = newState()
    mockRetention(
      baseRoutes(state, {
        [IMPACT_KEY]: () =>
          apiError(
            'retention_out_of_bounds',
            'Raw Block Summaries cannot be lowered below 1 days (design §11.3 safety bounds)',
            400,
            ['family', 'retentionDays'],
          ),
      }),
    )
    await renderRetention()
    await openEditor()

    fireEvent.change(screen.getByLabelText('New retention (days)'), { target: { value: '0' } })
    // The page mirrors the floor from the bounds the Server sent...
    expect(await screen.findByText('The safety floor for this family is 1 day.')).toBeTruthy()
    // ...and the Server still owns the contract, so its refusal is what is read.
    expect(
      await screen.findByText(
        /The Server refuses this value: Raw Block Summaries cannot be lowered below 1 days/,
      ),
    ).toBeTruthy()
    fireEvent.change(screen.getByLabelText('Type the change to confirm'), {
      target: { value: 'retention ' + RAW + ' 0' },
    })
    expect(saveButton().hasAttribute('disabled')).toBe(true)
  })

  it('submits the version it read with the confirmed value and reports the audit id', async () => {
    const state = newState()
    mockRetention(
      baseRoutes(state, {
        [IMPACT_KEY]: impactRoute(),
        [POLICY_KEY]: (_request, body) => {
          const days = (body as { retentionDays: number }).retentionDays
          state.policies = [rawPolicy({ retentionDays: days }), aggregatePolicy()]
          return jsonResponse({ policy: rawPolicy({ retentionDays: days }), auditEventId: 77 }, 200)
        },
      }),
    )
    await renderRetention()
    await openEditor()

    await fillEdit(14, 'retention ' + RAW + ' 14')
    expect(screen.getByText('About 1400 rows are older than this bound now.', { exact: false })).toBeTruthy()
    fireEvent.click(saveButton())

    expect(await screen.findByText('is now retained for 14 days (Audit #77)', { exact: false })).toBeTruthy()
    const puts = calls(POLICY_KEY)
    expect(puts).toHaveLength(1)
    expect(puts[0].body).toEqual({ retentionDays: 14, expectedPolicyVersion: RAW_VERSION })
    await waitFor(() =>
      expect(screen.getByRole('table', { name: /Retention policies:/ }).textContent).toContain('14 days'),
    )
  })

  it('reports a concurrent policy change, reloads the value, and never retries the write', async () => {
    const state = newState()
    mockRetention(
      baseRoutes(state, {
        [IMPACT_KEY]: impactRoute(),
        [POLICY_KEY]: () => {
          // Another Operator saved first: the version this page read is gone.
          state.policies = [rawPolicy({ retentionDays: 21, policyVersion: OTHER_VERSION }), aggregatePolicy()]
          return apiError(
            'retention_policy_version_conflict',
            'the policy changed since it was read; reload the current value and preview again',
            409,
            ['expectedPolicyVersion'],
          )
        },
      }),
    )
    await renderRetention()
    await openEditor()
    await fillEdit(14, 'retention ' + RAW + ' 14')
    fireEvent.click(saveButton())

    expect(
      await screen.findByText(new RegExp('This policy changed since this page read it \\(fingerprint ' + RAW_VERSION_SHORT)),
    ).toBeTruthy()
    expect(screen.getByText(/The Server refused the write, so nothing changed/)).toBeTruthy()
    expect(calls(POLICY_KEY)).toHaveLength(1)
    // The confirmation was consumed by the refused write: the edit must be
    // re-confirmed against what is now shown.
    await waitFor(() =>
      expect((screen.getByLabelText('Type the change to confirm') as HTMLInputElement).value).toBe(''),
    )
    expect(saveButton().hasAttribute('disabled')).toBe(true)
    await waitFor(() =>
      expect(screen.getByRole('table', { name: /Retention policies:/ }).textContent).toContain('21 days'),
    )
    expect(screen.getAllByText('21 days').length).toBeGreaterThanOrEqual(2)
  })

  it('maps a field-level rejection onto the retention input', async () => {
    const state = newState()
    mockRetention(
      baseRoutes(state, {
        [IMPACT_KEY]: impactRoute(),
        [POLICY_KEY]: () =>
          apiError(
            'retention_out_of_bounds',
            'Raw Block Summaries must be between 1 and 30 days (design §11.3 safety bounds)',
            400,
            ['family', 'retentionDays'],
          ),
      }),
    )
    await renderRetention()
    await openEditor()
    await fillEdit(14, 'retention ' + RAW + ' 14')
    fireEvent.click(saveButton())

    const fieldError = await screen.findByText(
      'Raw Block Summaries must be between 1 and 30 days (design §11.3 safety bounds)',
    )
    expect(fieldError.getAttribute('id')).toBe('retention-days-error')
    expect(screen.getByLabelText('New retention (days)').getAttribute('aria-invalid')).toBe('true')
  })

  it('composes a bound preview and shows the scope, cutoff, estimate, skips and notes', async () => {
    const state = newState()
    mockRetention(
      baseRoutes(state, {
        [PREVIEW_KEY]: () => {
          state.preview = livePreview()
          return jsonResponse(livePreview(), 200)
        },
      }),
    )
    await renderRetention()
    expect(screen.getByText(/No preview is bound on this Server/)).toBeTruthy()

    fireEvent.click(screen.getByRole('button', { name: 'Compose preview' }))
    const table = await screen.findByRole('table', { name: /Families bound by this preview/ })
    expect(calls(PREVIEW_KEY)).toHaveLength(1)
    // No families given: the Server composes over every actionable family.
    expect(calls(PREVIEW_KEY)[0].body).toEqual({})

    const preview = slot('retention-preview')
    expect(preview.textContent).toContain(PREVIEW_SHORT)
    expect(preview.textContent).toContain('admin')
    expect(preview.textContent).toContain('Every enabled, supported family')
    expect(preview.textContent).toContain('1400 (Server estimate, an upper bound)')
    expect(table.textContent).toContain(RAW)
    expect(table.textContent).toContain('7 days')
    expect(table.textContent).toContain('1400')
    expect(table.textContent).toContain(formatObservedAt(livePreview().families[0].cutoff))
    expect(slot('retention-preview-skipped').textContent).toContain(AGG)
    expect(slot('retention-preview-skipped').textContent).toContain('unsupported')
    expect(slot('retention-preview-notes').textContent).toContain(
      'Estimates are upper bounds computed when this preview was composed.',
    )
    expect(
      (screen.getByRole('button', { name: 'Run retention for this preview' }) as HTMLButtonElement).disabled,
    ).toBe(false)
  })

  it('queues the run for the preview it bound and links the recorded task', async () => {
    const state = newState()
    state.preview = livePreview()
    mockRetention(
      baseRoutes(state, {
        [RUN_KEY]: () => {
          state.lastRun = runSummary()
          return jsonResponse(
            {
              auditEventId: 88,
              operation: { cancellable: true, errors: [], warnings: [], result: null, operation: runSummary() },
            },
            200,
          )
        },
      }),
    )
    await renderRetention()

    const run = screen.getByRole('button', { name: 'Run retention for this preview' }) as HTMLButtonElement
    expect(run.disabled).toBe(false)
    expect(slot('retention-run').textContent).toContain('the Server executes the plan it froze')
    fireEvent.click(run)

    expect(await screen.findByText(new RegExp('The Server queued the retention run as ' + RUN_ID))).toBeTruthy()
    expect(calls(RUN_KEY)).toHaveLength(1)
    expect(calls(RUN_KEY)[0].body).toEqual({ previewId: PREVIEW_ID })
    const link = await screen.findByRole('link', { name: RUN_SHORT })
    expect(link.getAttribute('href')).toBe('/admin/operations/' + RUN_ID)
  })

  it('surfaces a refused stale preview, never retries the run, and recovers from a new preview', async () => {
    const state = newState()
    state.preview = livePreview()
    mockRetention(
      baseRoutes(state, {
        [RUN_KEY]: () =>
          apiError(
            'retention_preview_stale',
            'the retention preview no longer matches the current policies (raw_block_summary changed); compose a new preview',
            409,
            ['previewId'],
          ),
        [PREVIEW_KEY]: () => {
          state.preview = livePreview({ previewId: SECOND_PREVIEW_ID, createdAt: iso(-1_000) })
          return jsonResponse(state.preview, 200)
        },
      }),
    )
    await renderRetention()

    const run = screen.getByRole('button', { name: 'Run retention for this preview' }) as HTMLButtonElement
    fireEvent.click(run)

    // The Server's own message is on screen where the run was requested, and
    // the recovery instruction is on screen beside the preview it was refused
    // for.
    await waitFor(() =>
      expect(slot('retention-run').textContent).toContain(
        'no longer matches the current policies (raw_block_summary changed)',
      ),
    )
    expect(slot('retention-preview').textContent).toContain('Nothing was queued')
    expect(slot('retention-preview').textContent).toContain('this page will not retry it')
    await waitFor(() =>
      expect(
        (screen.getByRole('button', { name: 'Run retention for this preview' }) as HTMLButtonElement).disabled,
      ).toBe(true),
    )
    fireEvent.click(screen.getByRole('button', { name: 'Run retention for this preview' }))
    expect(calls(RUN_KEY)).toHaveLength(1)

    fireEvent.click(screen.getByRole('button', { name: 'Compose a new preview' }))
    await waitFor(() => expect(calls(PREVIEW_KEY)).toHaveLength(1))
    await waitFor(() =>
      expect(
        (screen.getByRole('button', { name: 'Run retention for this preview' }) as HTMLButtonElement).disabled,
      ).toBe(false),
    )
    await waitFor(() => expect(slot('retention-preview').textContent).toContain('0195f2a1…0401'))
    expect(screen.queryByText(/this page will not retry it/)).toBeNull()
  })

  it('refuses a run for an expired preview', async () => {
    const state = newState()
    state.preview = livePreview({ expiresAt: iso(-60_000) })
    mockRetention(baseRoutes(state))
    await renderRetention()

    expect(await screen.findByText('Expired: a run for this preview is refused.')).toBeTruthy()
    expect(screen.getByText(/The Server refuses a run for an expired preview/)).toBeTruthy()
    expect(
      (screen.getByRole('button', { name: 'Run retention for this preview' }) as HTMLButtonElement).disabled,
    ).toBe(true)
    fireEvent.click(screen.getByRole('button', { name: 'Run retention for this preview' }))
    expect(calls(RUN_KEY)).toHaveLength(0)
  })

  it('keeps the last successful values when a refresh fails', async () => {
    const state = newState()
    let failed = false
    mockRetention(
      baseRoutes(state, {
        [OVERVIEW_KEY]: () =>
          failed ? apiError('unavailable', 'The Server is unavailable.', 503) : jsonResponse(overviewBody(state), 200),
      }),
    )
    await renderRetention()
    await screen.findByRole('table', { name: /Retention policies:/ })

    failed = true
    await act(async () => {
      await adminQueryClient.invalidateQueries({ queryKey: ['admin', 'retention'] })
    })

    expect(await screen.findByText(/Unable to refresh retention policies/)).toBeTruthy()
    expect(screen.getByText(/nothing below is zeroed or marked Healthy because this refresh failed/)).toBeTruthy()
    const table = screen.getByRole('table', { name: /Retention policies:/ })
    expect(table.textContent).toContain(RAW_LABEL)
    expect(table.textContent).toContain('7 days')
  })

  it('reports an unknown outcome when composing never reaches the Server', async () => {
    const state = newState()
    mockRetention(
      baseRoutes(state, {
        [PREVIEW_KEY]: () => {
          throw new Error('network down')
        },
      }),
    )
    await renderRetention()

    fireEvent.click(screen.getByRole('button', { name: 'Compose preview' }))
    expect(await screen.findByText(/whether a preview was composed is unknown/)).toBeTruthy()
    expect(screen.getByText(/Nothing is bound here until one is composed successfully/)).toBeTruthy()
    expect(screen.getByText(/No preview is bound on this Server/)).toBeTruthy()
    expect((screen.getByRole('button', { name: 'Compose preview' }) as HTMLButtonElement).disabled).toBe(false)
  })
})
