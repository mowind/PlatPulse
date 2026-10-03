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

const RUNNING_OPERATION_ID = '0195f2a1-0200-4100-8100-000000000200'
const QUEUED_OPERATION_ID = '0195f2a1-0201-4101-8101-000000000201'
const WARNED_OPERATION_ID = '0195f2a1-0202-4102-8102-000000000202'

/** A retention run in flight, with the Server's own progress record. */
const RUNNING_OPERATION = {
  operationId: RUNNING_OPERATION_ID,
  kind: 'retention_run',
  status: 'running',
  progressPercent: 40,
  progressLabel: 'Pruning expired read samples',
  requestId: 'req-1',
  createdAt: '2026-03-01T00:00:00Z',
  startedAt: '2026-03-01T00:00:05Z',
  finishedAt: null,
  auditEventId: 11,
  cancelRequested: false,
}

/** A task the Server accepted but has not started. */
const QUEUED_OPERATION = {
  ...RUNNING_OPERATION,
  status: 'queued',
  progressPercent: 0,
  progressLabel: null,
  startedAt: null,
  auditEventId: 10,
  cancelRequested: false,
}
/** A running task whose cancel request the Server already recorded. */
const CANCEL_REQUESTED_OPERATION = {
  ...RUNNING_OPERATION,
  operationId: QUEUED_OPERATION_ID,
  cancelRequested: true,
  auditEventId: 12,
}

/** A finished Doctor run that recorded warnings and a result payload. */
const WARNED_OPERATION = {
  operationId: WARNED_OPERATION_ID,
  kind: 'doctor_run',
  status: 'succeeded_with_warnings',
  progressPercent: 0,
  progressLabel: null,
  requestId: 'req-2',
  createdAt: '2026-03-01T01:00:00Z',
  startedAt: '2026-03-01T01:00:01Z',
  finishedAt: '2026-03-01T01:00:06Z',
  auditEventId: 13,
  cancelRequested: false,
}

const DOCTOR_CHECK = {
  checkId: 'retention.last_run',
  label: 'Retention ran recently',
  status: 'warning',
  detail: 'The last retention run finished with warnings.',
}

/** The Server's own record of a cleanup stopped at a safe checkpoint (Story 40). */
const CANCELLED_RUN_RESULT = {
  cancelled: {
    phase: 'running',
    releasedRows: 384,
    remainingTargets: 2,
    families: [{ family: 'raw_block_summary', deletedRows: 384, estimatedRows: 1400 }],
    note:
      'Cancellation stops the run at a safe checkpoint between bounded batches. Rows already released stay ' +
      'released - the Server never rolls a release back - and the work that remained was not attempted.',
  },
  previewId: '0195f2a1-0400-4100-8100-000000000400',
}

/** A disabled policy: no threshold is declared, so none may be shown. */
const DISABLED_CAPACITY = {
  enabled: false,
  protected: false,
  pauseBelowBytes: null,
  resumeAboveBytes: null,
  sampleIntervalSeconds: 60,
  policyOrigin: null,
  mountPath: '/var/lib/platpulse',
  sample: { mountPath: '/var/lib/platpulse', totalBytes: 107374182400, availableBytes: 85899345920 },
  sampledAt: '2026-08-12T09:00:00Z',
  samplingError: null,
  transitionError: null,
  activeIntervalId: null,
  recentIntervals: [],
}

/** A Server protecting itself: the interval and the skipped series are the
 * Server's own gap record, and the recovery measurement closed it. */
const PROTECTED_CAPACITY = {
  ...DISABLED_CAPACITY,
  enabled: true,
  protected: true,
  pauseBelowBytes: 5368709120,
  resumeAboveBytes: 10737418240,
  policyOrigin: '/etc/platpulse/server.toml',
  sample: { mountPath: '/var/lib/platpulse', totalBytes: 107374182400, availableBytes: 1073741824 },
  activeIntervalId: '0195f2a1-0500-4500-8500-000000000500',
  recentIntervals: [
    {
      intervalId: '0195f2a1-0500-4500-8500-000000000500',
      sourceMount: '/var/lib/platpulse',
      startedAt: '2026-03-01T02:00:00Z',
      startedReason: 'low_space',
      openedTotalBytes: 107374182400,
      openedAvailableBytes: 1073741824,
      pauseBelowBytes: 5368709120,
      resumeAboveBytes: 10737418240,
      endedAt: null,
      endedReason: null,
      resumedTotalBytes: null,
      resumedAvailableBytes: null,
      updatedAt: '2026-03-01T02:05:00Z',
      skippedSampleCount: 4,
      skippedSeriesTotal: 3,
      skippedSeries: [
        {
          scopeKind: 'host',
          scopeKey: '0195f2a1-0011-4011-8011-000000000011',
          metric: 'network_rx_bytes_per_sec',
          skippedCount: 2,
          firstSkippedAt: '2026-03-01T02:00:30Z',
          lastSkippedAt: '2026-03-01T02:04:30Z',
        },
        {
          scopeKind: 'node',
          scopeKey: '0195f2a1-0014-4014-8014-000000000014',
          metric: 'process_cpu_percent',
          skippedCount: 1,
          firstSkippedAt: '2026-03-01T02:00:30Z',
          lastSkippedAt: '2026-03-01T02:00:30Z',
        },
      ],
    },
  ],
}

function detailOf(operation: Record<string, unknown>, overrides: Record<string, unknown> = {}) {
  return {
    operation,
    warnings: [],
    errors: [],
    result: null,
    cancellable: operation.status === 'queued' || operation.status === 'running',
    ...overrides,
  }
}

function jsonResponse(body: unknown, status: number): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

/** Sanitized ApiErrorBody, as the Server sends it. */
function apiError(code: string, message: string, status: number): Response {
  return jsonResponse(
    { error: { code, message, requestId: 'req-err' } },
    status,
  )
}

function slot(name: string): HTMLElement {
  const element = document.querySelector<HTMLElement>('[data-slot="' + name + '"]')
  if (!element) throw new Error('missing data-slot ' + name)
  return element
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

describe('PAGE-ADMIN-OPERATIONS (task ledger)', () => {
  it('lists each recorded task with its status, kind, and progress', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/capacity': () => jsonResponse(DISABLED_CAPACITY, 200),
      '/api/admin/v1/operations*': () =>
        jsonResponse([WARNED_OPERATION, RUNNING_OPERATION, CANCEL_REQUESTED_OPERATION], 200),
    })
    await renderAt('/admin/operations')

    await screen.findByRole('heading', { level: 1, name: 'Operations' })
    const table = await screen.findByRole('table', { name: /Recorded Operations/ })
    expect(table.textContent).toContain('Doctor')
    expect(table.textContent).toContain('Retention run')
    expect(table.textContent).toContain('Succeeded with warnings')
    expect(table.textContent).toContain('Running')
    // A recorded cancel request is shown as a request, never as an outcome.
    expect(table.textContent).toContain('Cancel requested')
    // A finished task carries an outcome instead of a stale 0%.
    expect(screen.getByText('Not applicable')).toBeTruthy()
    expect(table.textContent).toContain('40%')
  })

  it('asks the Server for the filtered window and keeps the filter in the URL', async () => {
    const fetchMock = mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/capacity': () => jsonResponse(DISABLED_CAPACITY, 200),
      '/api/admin/v1/operations*': () => jsonResponse([RUNNING_OPERATION], 200),
    })
    await renderAt('/admin/operations?status=running&kind=retention_run')

    await screen.findByRole('table', { name: /Recorded Operations/ })
    await waitFor(() => {
      const url = fetchMock.mock.calls
        .map((call) => String((call[0] as Request).url))
        .find((url) => url.includes('/api/admin/v1/operations'))
      expect(url).toContain('status=running')
      expect(url).toContain('kind=retention_run')
    })
  })

  it('asks the Server for the chosen page size instead of the default window', async () => {
    const fetchMock = mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/capacity': () => jsonResponse(DISABLED_CAPACITY, 200),
      '/api/admin/v1/operations*': () => jsonResponse([RUNNING_OPERATION], 200),
    })
    await renderAt('/admin/operations?limit=25')

    await screen.findByRole('table', { name: /Recorded Operations/ })
    // The Page size control is a real Server bound, not a local slice, so the
    // outgoing request must carry the chosen limit.
    await waitFor(() => {
      const url = fetchMock.mock.calls
        .map((call) => String((call[0] as Request).url))
        .find((url) => url.includes('/api/admin/v1/operations'))
      expect(url).toContain('limit=25')
    })

    fireEvent.change(screen.getByLabelText('Page size'), { target: { value: '200' } })
    await waitFor(() => {
      const urls = fetchMock.mock.calls.map((call) => String((call[0] as Request).url))
      expect(urls.some((url) => url.includes('limit=200'))).toBe(true)
    })
  })
  it('names the recorded state when a task id is unknown', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/operations/*': () => apiError('operation_not_found', 'Operation not found', 404),
    })
    await renderAt('/admin/operations/op-missing')

    await screen.findByRole('heading', { level: 1, name: 'Operation not found' })
  })

  it('shows the recorded outcome, the Audit link, and no delete control', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/operations/*': () =>
        jsonResponse(
          detailOf(WARNED_OPERATION, {
            warnings: [{ code: 'doctor_check_failed', message: 'node-1 disk usage is above 90%' }],
            result: { checks: [DOCTOR_CHECK] },
          }),
          200,
        ),
    })
    await renderAt('/admin/operations/' + WARNED_OPERATION_ID)

    await screen.findByRole('heading', { level: 1, name: 'Operation detail' })
    expect(screen.getByText('doctor_check_failed')).toBeTruthy()
    expect(screen.getByText('node-1 disk usage is above 90%')).toBeTruthy()
    expect(
      screen.getByRole('link', { name: /Audit log|event-13|13/ }).getAttribute('href'),
    ).toContain('/admin/access/audit')
    expect(
      screen.getByRole('region', { name: 'Operation result payload' }).textContent,
    ).toContain('retention.last_run')
    // Reading an outcome never deletes anything, so no delete control exists.
    expect(screen.queryByRole('button', { name: /Delete|Remove/ })).toBeNull()
  })

  it('shows what a stopped cleanup already released and what it stopped', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/operations/*': () =>
        jsonResponse(
          detailOf(
            { ...RUNNING_OPERATION, status: 'cancelled', finishedAt: '2026-03-01T01:00:10Z', cancelRequested: true },
            { result: CANCELLED_RUN_RESULT },
          ),
          200,
        ),
    })
    await renderAt('/admin/operations/' + RUNNING_OPERATION_ID)

    await screen.findByRole('heading', { level: 1, name: 'Operation detail' })
    expect(slot('operation-cancellation-phase').textContent).toContain('Cancelled while running')
    expect(screen.getByText(/stopped at a safe checkpoint between batches/)).toBeTruthy()
    expect(slot('operation-cancellation-released').textContent).toBe('384')
    expect(slot('operation-cancellation-remaining').textContent).toBe('2')
    expect(slot('operation-cancellation-families').textContent).toContain('raw_block_summary')
    expect(slot('operation-cancellation-families').textContent).toContain('384 of an estimated 1400 rows released')
    expect(slot('operation-cancellation-note').textContent).toContain('never rolls a release back')
    expect(screen.getByText('0195f2a1-0400-4100-8100-000000000400')).toBeTruthy()
    // The raw Server payload is still recorded below the summary, unchanged.
    expect(screen.getByRole('region', { name: 'Operation result payload' }).textContent).toContain('releasedRows')
  })

  it('reports a cleanup cancelled before it ran as having executed nothing', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/operations/*': () =>
        jsonResponse(
          detailOf(QUEUED_OPERATION, {
            result: {
              cancelled: {
                phase: 'queued',
                releasedRows: 0,
                remainingTargets: 1,
                families: [],
                note:
                  'The task was cancelled while it was still queued, so it executed nothing: nothing was ' +
                  'released and nothing is left running.',
              },
            },
          }),
          200,
        ),
    })
    await renderAt('/admin/operations/' + QUEUED_OPERATION_ID)

    await screen.findByRole('heading', { level: 1, name: 'Operation detail' })
    expect(slot('operation-cancellation-phase').textContent).toContain('Cancelled while queued')
    expect(screen.getByText(/no batch ever ran and nothing was released/)).toBeTruthy()
    expect(slot('operation-cancellation-released').textContent).toBe('0')
    expect(slot('operation-cancellation-remaining').textContent).toBe('1')
    expect(document.querySelector('[data-slot="operation-cancellation-families"]')).toBeNull()
    expect(slot('operation-cancellation-note').textContent).toContain('executed nothing')
  })

  it('never invents a cancellation outcome for a payload it cannot vouch for', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/operations/*': () =>
        jsonResponse(
          detailOf(RUNNING_OPERATION, {
            result: { cancelled: { phase: 'halfway', releasedRows: 3 } },
          }),
          200,
        ),
    })
    await renderAt('/admin/operations/' + RUNNING_OPERATION_ID)

    await screen.findByRole('heading', { level: 1, name: 'Operation detail' })
    expect(document.querySelector('[data-slot="operation-cancellation"]')).toBeNull()
    expect(screen.getByRole('region', { name: 'Operation result payload' }).textContent).toContain('halfway')
  })

  it('omits a family the Server recorded no counts for instead of inventing zeros', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/operations/*': () =>
        jsonResponse(
          detailOf(
            { ...RUNNING_OPERATION, status: 'cancelled', cancelRequested: true },
            {
              result: {
                cancelled: {
                  phase: 'running',
                  releasedRows: 5,
                  remainingTargets: 1,
                  families: [
                    { family: 'raw_block_summary', deletedRows: 5, estimatedRows: 40 },
                    { family: 'aggregate_rollup', note: 'counts were never recorded' },
                  ],
                  note: 'stopping at a safe checkpoint leaves the rest unattempted',
                },
              },
            },
          ),
          200,
        ),
    })
    await renderAt('/admin/operations/' + RUNNING_OPERATION_ID)

    await screen.findByRole('heading', { level: 1, name: 'Operation detail' })
    const families = slot('operation-cancellation-families').textContent ?? ''
    expect(families).toContain('5 of an estimated 40 rows released')
    // An incomplete entry is left out rather than shown as a fabricated
    // "0 of an estimated 0" (webui.md §15.11); the raw payload keeps it visible.
    expect(families).not.toContain('aggregate_rollup')
    expect(families).not.toContain('0 of an estimated 0')
    expect(screen.getByRole('region', { name: 'Operation result payload' }).textContent).toContain(
      'aggregate_rollup',
    )
  })

  it('never offers a cancel control for a running task that already recorded one', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/operations/*': () =>
        jsonResponse(detailOf(CANCEL_REQUESTED_OPERATION, { cancellable: false }), 200),
    })
    await renderAt('/admin/operations/' + QUEUED_OPERATION_ID)

    await screen.findByRole('heading', { level: 1, name: 'Operation detail' })
    expect(screen.getByText(/Cancel requested — the task stops/)).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Cancel task' }).hasAttribute('disabled')).toBe(true)
    expect(
      screen.getByText(/will not accept a cancel request for this task/),
    ).toBeTruthy()
  })

  it('confirms a cancel request in two steps and renders only the recorded outcome', async () => {
    let cancelled = false
    const fetchMock = mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/operations/*': (request) => {
        // A queued task is cancelled by the Server itself, so the page may
        // only render what the recorded DTO says.
        const record = cancelled
          ? { ...QUEUED_OPERATION, status: 'cancelled', finishedAt: '2026-03-01T00:00:07Z' }
          : QUEUED_OPERATION
        if (request.method === 'POST' && request.url.endsWith('/cancel')) {
          cancelled = true
          return jsonResponse(
            {
              auditEventId: 21,
              operation: detailOf(
                { ...QUEUED_OPERATION, status: 'cancelled', finishedAt: '2026-03-01T00:00:07Z' },
                { cancellable: false },
              ),
            },
            200,
          )
        }
        return jsonResponse(detailOf(record, { cancellable: !cancelled }), 200)
      },
    })
    await renderAt('/admin/operations/' + QUEUED_OPERATION_ID)
    await screen.findByRole('heading', { level: 1, name: 'Operation detail' })
    expect(screen.getAllByText('Queued').length).toBeGreaterThan(0)

    // The first click only arms the request; nothing is sent yet.
    fireEvent.click(screen.getByRole('button', { name: 'Cancel task' }))
    const sent = () =>
      fetchMock.mock.calls.filter((call) => String((call[0] as Request).url).includes('/cancel'))
    expect(sent()).toHaveLength(0)
    expect(screen.getByText(/Request cancellation of/)).toBeTruthy()

    fireEvent.click(screen.getByRole('button', { name: 'Confirm cancel request' }))
    await screen.findByText(/The Server recorded the cancel request/)
    const cancelCalls = sent()
    expect(cancelCalls).toHaveLength(1)
    expect((cancelCalls[0][0] as Request).method).toBe('POST')
    expect((cancelCalls[0][0] as Request).headers.get('X-CSRF-Token')).toBe('csrf-token')

    // The terminal status and the finished time come from the Server record,
    // never from an optimistic local guess.
    await screen.findByText('Cancelled')
    expect(screen.queryByText('Queued')).toBeNull()
    expect(screen.getByRole('button', { name: 'Cancel task' }).hasAttribute('disabled')).toBe(true)
  })
  it('explains a refused cancel without claiming the task changed', async () => {
    const fetchMock = mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/operations/*': (request) => {
        if (request.method === 'POST') {
          return apiError(
            'operation_not_cancellable',
            'only queued or running Operations can be cancelled',
            409,
          )
        }
        return jsonResponse(detailOf(RUNNING_OPERATION), 200)
      },
    })
    await renderAt('/admin/operations/' + RUNNING_OPERATION_ID)
    await screen.findByRole('heading', { level: 1, name: 'Operation detail' })

    // A running task is offered the control, and the Server may still refuse
    // it between the read and the command. Its conflict answer is the only
    // thing the Owner learns; nothing local is invented.
    fireEvent.click(screen.getByRole('button', { name: 'Cancel task' }))
    fireEvent.click(screen.getByRole('button', { name: 'Confirm cancel request' }))
    await screen.findByText(/only queued or running Operations can be cancelled/)
    expect(screen.getByText(/The recorded status is authoritative/)).toBeTruthy()
    expect(screen.getAllByText('Running').length).toBeGreaterThan(0)
    expect(screen.queryByText('Cancelled')).toBeNull()
    expect(
      fetchMock.mock.calls.filter((call) => String((call[0] as Request).url).includes('/cancel')),
    ).toHaveLength(1)
  })
  it('names an unknown outcome when a cancel request may not have reached the Server', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/operations/*': (request) => {
        if (request.method === 'POST') throw new TypeError('Failed to fetch')
        return jsonResponse(detailOf(RUNNING_OPERATION), 200)
      },
    })
    await renderAt('/admin/operations/' + RUNNING_OPERATION_ID)
    await screen.findByRole('heading', { level: 1, name: 'Operation detail' })

    fireEvent.click(screen.getByRole('button', { name: 'Cancel task' }))
    fireEvent.click(screen.getByRole('button', { name: 'Confirm cancel request' }))
    await screen.findByText(/The request may not have reached the Server/)
  })

  it('offers a retry when the ledger cannot be loaded', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/capacity': () => jsonResponse(DISABLED_CAPACITY, 200),
      '/api/admin/v1/operations*': () => apiError('database_unavailable', 'Server database is unavailable', 503),
    })
    await renderAt('/admin/operations')

    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toContain('Server database is unavailable')
    expect(screen.getByRole('button', { name: 'Try again' })).toBeTruthy()
  })

  it('shows a disabled capacity policy without inventing a threshold', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/capacity': () => jsonResponse(DISABLED_CAPACITY, 200),
      '/api/admin/v1/operations*': () => jsonResponse([RUNNING_OPERATION], 200),
    })
    await renderAt('/admin/operations')

    const title = await screen.findByText('Storage capacity and low-space protection')
    const card = title.closest('[data-slot="card-x"]')
    expect(card).not.toBeNull()
    const text = card?.textContent ?? ''
    expect(text).toContain('Disabled')
    // A disabled policy declares no floor, so the card says so twice instead
    // of rendering an absent threshold as a number.
    expect(text.match(/Not declared/g)).toHaveLength(2)
    expect(text).toContain('never paused')
    // The filesystem is still measured while the policy is off.
    expect(text).toContain('/var/lib/platpulse')
    expect(text).toContain('80.0 GiB available of 100 GiB')
    expect(text).toContain('2026-08-12 09:00:00 UTC')
    expect(text).toContain('never been paused here')
  })

  it('shows the recorded gap while low-space protection is active', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/capacity': () => jsonResponse(PROTECTED_CAPACITY, 200),
      '/api/admin/v1/operations*': () => jsonResponse([RUNNING_OPERATION], 200),
    })
    await renderAt('/admin/operations')

    await screen.findByText('Storage capacity and low-space protection')
    const card = screen.getByText('Storage capacity and low-space protection').closest(
      '[data-slot="card-x"]',
    )
    const text = card?.textContent ?? ''
    expect(text).toContain('Protecting')
    expect(text).toContain('5.00 GiB')
    expect(text).toContain('10.0 GiB')
    expect(text).toContain('/etc/platpulse/server.toml')
    // Protecting is a hysteresis, so the note names both declared levels: a
    // volume already above the pause floor must not be described as below it.
    expect(text).toContain(
      'Optional history is paused while available space is at or below 5.00 GiB, ' +
        'resumes at 10.0 GiB available, and every skipped sample is recorded below.',
    )

    const intervals = slot('capacity-intervals-table')
    expect(intervals.textContent).toContain('Low space')
    expect(intervals.textContent).toContain('Still active')
    // A historical row names its own volume and both boundary totals: a mount
    // can be relocated, and the floor alone does not say how full it was.
    expect(intervals.textContent).toContain('/var/lib/platpulse')
    expect(intervals.textContent).toContain('1.00 GiB of 100 GiB')

    // The gap is visible per series, not only as a count.
    const gap = slot('capacity-skipped-series')
    expect(gap.textContent).toContain('host:0195f2a1-0011-4011-8011-000000000011')
    expect(gap.textContent).toContain('network_rx_bytes_per_sec')
    expect(gap.textContent).toContain('2 skipped')
    expect(gap.textContent).toContain('node:0195f2a1-0014-4014-8014-000000000014')
    expect(gap.textContent).toContain('process_cpu_percent')
    // The worst-N bound is named instead of pretending the list is complete.
    expect(text).toContain('3 series lost samples in total')
    // Optional history only: the Operation ledger is unaffected.
    expect(screen.getByRole('table', { name: /Recorded Operations/ })).toBeTruthy()
  })

  it('keeps an enabled policy Unknown until a reading arrives', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/capacity': () =>
        jsonResponse(
          {
            ...PROTECTED_CAPACITY,
            protected: false,
            sample: null,
            sampledAt: null,
            activeIntervalId: null,
            recentIntervals: [],
          },
          200,
        ),
      '/api/admin/v1/operations*': () => jsonResponse([RUNNING_OPERATION], 200),
    })
    await renderAt('/admin/operations')

    await screen.findByText('Storage capacity and low-space protection')
    const card = screen
      .getByText('Storage capacity and low-space protection')
      .closest('[data-slot="card-x"]')
    const text = card?.textContent ?? ''
    // Enabling protection does not make an unmeasured filesystem healthy.
    expect(text).toContain('Unknown')
    expect(text).not.toContain('Monitoring')
    expect(text).toContain('has not been measured yet')
    expect(text).toContain('Never measured')
  })

  it('reports a failed capacity measurement as Unknown, not Monitoring', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/capacity': () =>
        jsonResponse(
          {
            ...PROTECTED_CAPACITY,
            protected: false,
            activeIntervalId: null,
            recentIntervals: [],
            samplingError: 'Permission denied (os error 13)',
          },
          200,
        ),
      '/api/admin/v1/operations*': () => jsonResponse([RUNNING_OPERATION], 200),
    })
    await renderAt('/admin/operations')

    await screen.findByText('Storage capacity and low-space protection')
    const card = screen
      .getByText('Storage capacity and low-space protection')
      .closest('[data-slot="card-x"]')
    const text = card?.textContent ?? ''
    expect(text).toContain('Unknown')
    expect(text).not.toContain('Monitoring')
    expect(text).toContain('the reading above is the last successful one')
    expect(text).toContain('The state filesystem could not be measured: Permission denied (os error 13)')
    // The retained last-good measurement is still shown, and dated.
    expect(text).toContain('1.00 GiB available of 100 GiB')
    expect(text).toContain('2026-08-12 09:00:00 UTC')
  })

  it('reports a failed protection transition as Unknown, not Monitoring', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/capacity': () =>
        jsonResponse(
          {
            ...PROTECTED_CAPACITY,
            protected: false,
            activeIntervalId: null,
            recentIntervals: [],
            transitionError: 'database is locked',
          },
          200,
        ),
      '/api/admin/v1/operations*': () => jsonResponse([RUNNING_OPERATION], 200),
    })
    await renderAt('/admin/operations')

    await screen.findByText('Storage capacity and low-space protection')
    const card = screen
      .getByText('Storage capacity and low-space protection')
      .closest('[data-slot="card-x"]')
    const text = card?.textContent ?? ''
    expect(text).toContain('Unknown')
    expect(text).not.toContain('Monitoring')
    expect(text).toContain('The last protection transition could not be recorded: database is locked')
  })
})
