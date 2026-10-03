import { StrictMode } from 'react'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
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

const ARTIFACT_ID = '0195f2a1-0300-4100-8100-000000000300'
const OTHER_ARTIFACT_ID = '0195f2a1-0303-4103-8103-000000000303'
const VERIFY_OPERATION_ID = '0195f2a1-0301-4101-8101-000000000301'
const CREATE_OPERATION_ID = '0195f2a1-0302-4102-8102-000000000302'

/** A recorded artifact no verification task has examined yet. */
const PENDING_ARTIFACT = {
  artifactId: ARTIFACT_ID,
  filename: 'platpulse-' + ARTIFACT_ID + '.db',
  bytes: 2_097_152,
  sha256: 'a'.repeat(64),
  schemaVersion: 42,
  serverVersion: '0.2.0',
  createdAt: '2026-03-01T00:00:00Z',
  dataRangeMin: '2026-02-28T00:00:00Z',
  dataRangeMax: '2026-03-01T00:00:00Z',
  verification: 'pending',
  verifiedAt: null,
  createOperationId: CREATE_OPERATION_ID,
  verifyOperationId: null,
}

/** An artifact whose last verification compared the file to its checksum. */
const CORRUPT_ARTIFACT = {
  ...PENDING_ARTIFACT,
  artifactId: OTHER_ARTIFACT_ID,
  filename: 'platpulse-' + OTHER_ARTIFACT_ID + '.db',
  verification: 'failed',
  verifiedAt: '2026-03-01T02:00:00Z',
  verifyOperationId: VERIFY_OPERATION_ID,
}
const CHECKSUM_REASON = 'artifact checksum mismatch'

function detailOf(artifact: Record<string, unknown>, verificationError: string | null = null) {
  return { artifact, verificationError }
}

function queuedVerification() {
  return {
    auditEventId: 31,
    operation: {
      operation: {
        operationId: VERIFY_OPERATION_ID,
        kind: 'backup_verify',
        status: 'queued',
        progressPercent: 0,
        progressLabel: null,
        requestId: 'req-verify',
        createdAt: '2026-03-01T03:00:00Z',
        startedAt: null,
        finishedAt: null,
        auditEventId: 31,
        cancelRequested: false,
      },
      warnings: [],
      errors: [],
      result: null,
      cancellable: true,
    },
  }
}

/** The detail of one task, as the Server records it for its own ledger. */
function taskDetailOf(operation: Record<string, unknown>, detail: Record<string, unknown> = {}) {
  const base = queuedVerification().operation
  return {
    ...base,
    operation: { ...base.operation, ...operation },
    ...detail,
  }
}

function jsonResponse(body: unknown, status: number): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

function apiError(code: string, message: string, status: number): Response {
  return jsonResponse({ error: { code, message, requestId: 'req-err' } }, status)
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

describe('PAGE-ADMIN-BACKUPS (recorded backup artifacts)', () => {
  it('lists each recorded artifact with the manifest facts the Server wrote', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/backups*': () => jsonResponse([CORRUPT_ARTIFACT, PENDING_ARTIFACT], 200),
    })
    await renderAt('/admin/backups')

    await screen.findByRole('heading', { level: 1, name: 'Backups' })
    const table = document.querySelector('table[data-slot="backups-table"]')
    expect(table).not.toBeNull()
    expect(screen.getByText('Recorded backup artifacts · 2')).toBeTruthy()

    // The artifact name links to its own detail page; the row shows size,
    // schema, and the verification state as the Server recorded it.
    const pendingLink = screen.getByRole('link', { name: PENDING_ARTIFACT.filename })
    expect(pendingLink.getAttribute('href')).toBe('/admin/backups/' + ARTIFACT_ID)
    expect(screen.getAllByText('2.0 MiB')).toHaveLength(2)
    expect(screen.getAllByText('42').length).toBeGreaterThan(0)
    expect(screen.getAllByText('Not verified').length).toBeGreaterThan(0)
    expect(screen.getByText('Verification failed')).toBeTruthy()
    expect(screen.getByText('No verification outcome recorded')).toBeTruthy()

    // The manifest facts the Server wrote include the version that created
    // the artifact and the task that wrote it, both reachable from the row.
    expect(screen.getByRole('columnheader', { name: 'Server' })).toBeTruthy()
    expect(screen.getByRole('columnheader', { name: 'Creating task' })).toBeTruthy()
    expect(screen.getAllByText('0.2.0')).toHaveLength(2)
    const createLinks = screen.getAllByRole('link', { name: /0302/ })
    expect(createLinks).toHaveLength(2)
    expect(createLinks[0].getAttribute('href')).toBe('/admin/operations/' + CREATE_OPERATION_ID)

    // A backup surface that only reads and requests: no creation, restore,
    // deletion, or schedule control is offered.
    expect(screen.queryByRole('button', { name: /Delete|Remove|Restore|Create|Schedule/i })).toBeNull()
    expect(screen.getByText(/it never creates, restores, or deletes a backup/)).toBeTruthy()
  })

  it('states that no artifact is recorded instead of showing an empty table', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/backups*': () => jsonResponse([], 200),
    })
    await renderAt('/admin/backups')

    await screen.findByText(/No backup artifact is recorded yet/)
    expect(document.querySelector('table[data-slot="backups-table"]')).toBeNull()
  })

  it('offers a retry when the recorded artifacts cannot be loaded', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/backups*': () =>
        apiError('database_unavailable', 'Server database is unavailable', 503),
    })
    await renderAt('/admin/backups')

    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toContain('Server database is unavailable')
    expect(screen.getByRole('button', { name: 'Try again' })).toBeTruthy()
  })
})

describe('PAGE-ADMIN-BACKUP-DETAIL (inspect and verify)', () => {
  it('renders the recorded manifest and the verification outcome with its reason', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/backups/*': () => jsonResponse(detailOf(CORRUPT_ARTIFACT, CHECKSUM_REASON), 200),
      // The task ledger is unreadable here: the page must say so rather than
      // quietly dropping the task it links to.
      '/api/admin/v1/operations/*': () =>
        apiError('database_unavailable', 'Server database is unavailable', 503),
    })
    await renderAt('/admin/backups/' + OTHER_ARTIFACT_ID)

    await screen.findByRole('heading', { level: 1, name: CORRUPT_ARTIFACT.filename })
    expect(screen.getByText(OTHER_ARTIFACT_ID)).toBeTruthy()
    expect(screen.getByText('a'.repeat(64))).toBeTruthy()
    expect(screen.getByText('0.2.0')).toBeTruthy()
    expect(screen.getByText('2.0 MiB')).toBeTruthy()

    // The Server's own reason is shown verbatim next to a plain reading, and
    // the reading never claims a cause the recorded text does not name.
    const reason = document.querySelector('[data-slot="backup-verification-reason"]')
    expect(reason?.textContent).toBe(CHECKSUM_REASON)
    expect(screen.getByText(/no longer matches the checksum recorded when the artifact was created/)).toBeTruthy()

    // The verification task that recorded this outcome stays reachable.
    const taskLink = screen.getByRole('link', { name: /0301/ })
    expect(taskLink.getAttribute('href')).toBe('/admin/operations/' + VERIFY_OPERATION_ID)
    // A task record this page could not read is named as unread, so the
    // artifact state above is never presented as the task's own answer.
    expect(await screen.findByText(/could not read the task/)).toBeTruthy()

    // A passing-or-failing check is never sold as a restore guarantee.
    expect(screen.getByText(/It is not a restore rehearsal/)).toBeTruthy()
  })

  it('says a pending artifact has no recorded outcome and offers no verdict', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/backups/*': () => jsonResponse(detailOf(PENDING_ARTIFACT), 200),
    })
    await renderAt('/admin/backups/' + ARTIFACT_ID)

    await screen.findByRole('heading', { level: 1, name: PENDING_ARTIFACT.filename })
    expect(screen.getAllByText('Not verified').length).toBeGreaterThan(0)
    expect(screen.getByText(/nothing here says the file is readable/)).toBeTruthy()
    expect(screen.getByText(/No verification has recorded an outcome for this artifact yet/)).toBeTruthy()
    // No task has verified it, so no task block is fabricated at all — the
    // assertion is on the rendered block, not on a label this page could
    // rename without the test noticing.
    expect(document.querySelector('[data-slot="backup-verify-task"]')).toBeNull()
    expect(screen.queryByText(/does not point at it/)).toBeNull()
  })

  it('names an unknown artifact instead of rendering an empty page', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/backups/*': () =>
        apiError('backup_artifact_not_found', 'unknown backup artifact', 404),
    })
    await renderAt('/admin/backups/' + ARTIFACT_ID)

    await screen.findByRole('heading', { level: 1, name: 'Backup artifact not found' })
    const back = screen.getByRole('link', { name: 'Back to Backups' })
    expect(back.getAttribute('href')).toBe('/admin/backups')
    // A page that cannot read the artifact never offers to act on it.
    expect(screen.queryByRole('button', { name: 'Request verification' })).toBeNull()
  })

  it('requests a verification with CSRF and separates acceptance from the result', async () => {
    const fetchMock = mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/backups/*': (request) => {
        if (request.method === 'POST') return jsonResponse(queuedVerification(), 200)
        return jsonResponse(detailOf(PENDING_ARTIFACT), 200)
      },
      '/api/admin/v1/operations/*': () => jsonResponse(taskDetailOf({ status: 'queued' }), 200),
    })
    await renderAt('/admin/backups/' + ARTIFACT_ID)
    await screen.findByRole('heading', { level: 1, name: PENDING_ARTIFACT.filename })

    fireEvent.click(screen.getByRole('button', { name: 'Request verification' }))

    await screen.findByText(/Acceptance is not a result/)
    const posts = fetchMock.mock.calls.filter(
      (call) => String((call[0] as Request).url).endsWith('/verify'),
    )
    expect(posts).toHaveLength(1)
    const posted = posts[0][0] as Request
    expect(posted.method).toBe('POST')
    expect(posted.headers.get('X-CSRF-Token')).toBe('csrf-token')

    // Acceptance is not an outcome: the recorded state the Server sent is
    // still the only verdict on the page, and the in-flight task is named.
    expect(screen.getAllByText('Not verified').length).toBeGreaterThan(0)
    expect(screen.queryByText('Verified')).toBeNull()
    // The card reads the task's own status from the Server before it claims
    // anything is in flight, so the note is awaited rather than assumed.
    expect(await screen.findByText(/is queued or running/)).toBeTruthy()
    expect(screen.getByText(/still the one the Server last recorded/)).toBeTruthy()
    expect(
      screen.getByRole('button', { name: 'Request verification' }).hasAttribute('disabled'),
    ).toBe(false)
  })

  it('renders the outcome the Server records, and clears the in-flight note', async () => {
    let recorded = false
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/backups/*': (request) => {
        if (request.method === 'POST') {
          recorded = true
          return jsonResponse(queuedVerification(), 200)
        }
        return jsonResponse(
          detailOf(
            recorded
              ? {
                  ...PENDING_ARTIFACT,
                  verification: 'ok',
                  verifiedAt: '2026-03-01T03:00:05Z',
                  verifyOperationId: VERIFY_OPERATION_ID,
                }
              : PENDING_ARTIFACT,
          ),
          200,
        )
      },
      // The task itself is read from the Server, so its own terminal status
      // (and never a timer) is what ends the in-flight reading.
      '/api/admin/v1/operations/*': () =>
        jsonResponse(
          taskDetailOf({
            status: recorded ? 'succeeded' : 'queued',
            progressPercent: recorded ? 100 : 0,
            finishedAt: recorded ? '2026-03-01T03:00:05Z' : null,
          }),
          200,
        ),
    })
    await renderAt('/admin/backups/' + ARTIFACT_ID)
    await screen.findByRole('heading', { level: 1, name: PENDING_ARTIFACT.filename })

    fireEvent.click(screen.getByRole('button', { name: 'Request verification' }))
    await screen.findByText(/Acceptance is not a result/)

    // The Server wrote the outcome: the badge follows the recorded DTO, and
    // the acceptance note is replaced by the real result.
    await screen.findByText('Verified')
    expect(screen.getByText(/every check it runs passed/)).toBeTruthy()
    expect(await screen.findByText(/finished and recorded the outcome shown above/)).toBeTruthy()
    expect(screen.queryByText(/is queued or running/)).toBeNull()
  })

  it('explains a refused request without changing the recorded state', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/backups/*': (request) => {
        if (request.method === 'POST') {
          return apiError('backup_artifact_not_found', 'unknown backup artifact', 404)
        }
        return jsonResponse(detailOf(PENDING_ARTIFACT), 200)
      },
    })
    await renderAt('/admin/backups/' + ARTIFACT_ID)
    await screen.findByRole('heading', { level: 1, name: PENDING_ARTIFACT.filename })

    fireEvent.click(screen.getByRole('button', { name: 'Request verification' }))

    await screen.findByText(/no recorded artifact with this id, so no verification could be queued/)
    expect(screen.getAllByText('Not verified').length).toBeGreaterThan(0)
    expect(screen.queryByText('Verified')).toBeNull()
  })

  it('names an unknown outcome when the request may not have reached the Server', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/backups/*': (request) => {
        if (request.method === 'POST') throw new TypeError('Failed to fetch')
        return jsonResponse(detailOf(PENDING_ARTIFACT), 200)
      },
    })
    await renderAt('/admin/backups/' + ARTIFACT_ID)
    await screen.findByRole('heading', { level: 1, name: PENDING_ARTIFACT.filename })

    fireEvent.click(screen.getByRole('button', { name: 'Request verification' }))

    await screen.findByText(/The request may not have reached the Server/)
    expect(screen.getByText(/the outcome is unknown/)).toBeTruthy()
    // Nothing was recorded as a result, so the page keeps the pending state.
    expect(screen.queryByText('Verified')).toBeNull()
  })

  it('reports a cancelled task without claiming the artifact was checked', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/backups/*': (request) => {
        if (request.method === 'POST') return jsonResponse(queuedVerification(), 200)
        return jsonResponse(detailOf(PENDING_ARTIFACT), 200)
      },
      '/api/admin/v1/operations/*': () =>
        jsonResponse(
          taskDetailOf({
            status: 'cancelled',
            cancelRequested: true,
            finishedAt: '2026-03-01T03:01:00Z',
          }),
          200,
        ),
    })
    await renderAt('/admin/backups/' + ARTIFACT_ID)
    await screen.findByRole('heading', { level: 1, name: PENDING_ARTIFACT.filename })

    fireEvent.click(screen.getByRole('button', { name: 'Request verification' }))
    await screen.findByText(/Acceptance is not a result/)

    // A cancelled task writes no artifact outcome, and the artifact keeps its
    // last recorded one. The card reads the task's own record and says the
    // request ended without a verdict, instead of reporting it in flight
    // forever (webui.md §5.5).
    await screen.findByText('Cancelled')
    expect(screen.getByText(/recorded this task as cancelled, so it wrote no outcome/)).toBeTruthy()
    expect(screen.queryByText(/is queued or running/)).toBeNull()
    expect(screen.getAllByText('Not verified').length).toBeGreaterThan(0)
    expect(screen.queryByText('Verified')).toBeNull()
  })

  it('re-reads the artifact and names the gap until the task outcome is linked', async () => {
    // The task finishes, but the artifact record the page first read still
    // holds no outcome from it. The card must re-read the artifact and may
    // only claim correspondence once the Server's own record agrees.
    let terminalServed = false
    let detailReads = 0
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/backups/*': (request) => {
        if (request.method === 'POST') return jsonResponse(queuedVerification(), 200)
        detailReads += 1
        return jsonResponse(
          detailOf(
            terminalServed
              ? {
                  ...PENDING_ARTIFACT,
                  verification: 'ok',
                  verifiedAt: '2026-03-01T03:00:05Z',
                  verifyOperationId: VERIFY_OPERATION_ID,
                }
              : PENDING_ARTIFACT,
          ),
          200,
        )
      },
      '/api/admin/v1/operations/*': () => {
        terminalServed = true
        return jsonResponse(
          taskDetailOf({
            status: 'succeeded',
            progressPercent: 100,
            finishedAt: '2026-03-01T03:00:05Z',
          }),
          200,
        )
      },
    })
    await renderAt('/admin/backups/' + ARTIFACT_ID)
    await screen.findByRole('heading', { level: 1, name: PENDING_ARTIFACT.filename })

    fireEvent.click(screen.getByRole('button', { name: 'Request verification' }))
    await screen.findByText(/Acceptance is not a result/)

    // The gap is named first — the record does not point at this task, and the
    // page says only that rather than promising a link that may never come —
    // then the re-read artifact closes it.
    expect(
      await screen.findByText(/does not point at it, so this task’s result is not attributed/),
    ).toBeTruthy()
    expect(screen.queryByText(/recorded the outcome shown above/)).toBeNull()

    await screen.findByText(/recorded the outcome shown above/)
    expect(detailReads).toBeGreaterThanOrEqual(3)
    expect(screen.queryByText(/does not point at it/)).toBeNull()
  })

  it('shows the task’s own recorded errors and result beside the artifact state', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/backups/*': () => jsonResponse(detailOf(CORRUPT_ARTIFACT, CHECKSUM_REASON), 200),
      '/api/admin/v1/operations/*': () =>
        jsonResponse(
          taskDetailOf(
            { status: 'failed', finishedAt: '2026-03-01T02:00:05Z' },
            {
              errors: [{ code: 'backup_verification_failed', message: CHECKSUM_REASON }],
              result: {
                artifactId: OTHER_ARTIFACT_ID,
                verification: 'failed',
                checkedAt: '2026-03-01T02:00:05Z',
              },
            },
          ),
          200,
        ),
    })
    await renderAt('/admin/backups/' + OTHER_ARTIFACT_ID)
    await screen.findByRole('heading', { level: 1, name: CORRUPT_ARTIFACT.filename })

    // The authoritative record of the task is readable without leaving the
    // artifact page: the failure it wrote, and the payload it wrote with it.
    await screen.findByText('Errors the Server recorded for this task')
    expect(screen.getByText('backup_verification_failed')).toBeTruthy()
    // The artifact record already points at this task and carries a failed
    // outcome, so the task's failure and the recorded failure are the same
    // fact: a checksum failure ends the task as `failed` only after it wrote
    // that outcome (backup_verification_flow.rs).
    expect(screen.getByText(/holds the failure it recorded/)).toBeTruthy()
    expect(screen.queryByText(/failed before it recorded an artifact outcome/)).toBeNull()
    const result = document.querySelector('[data-slot="operation-result"]')
    expect(result?.textContent).toContain('"verification": "failed"')
    expect(result?.textContent).toContain(OTHER_ARTIFACT_ID)
  })

  it('keeps a failed task apart from the outcome the Server recorded for the artifact', async () => {
    // The Server can write an outcome and still lose the task before it
    // finalizes it: a restart fails the running task while the outcome it wrote
    // stands. The page must not read that as a failed artifact.
    const verifiedArtifact = {
      ...CORRUPT_ARTIFACT,
      verification: 'ok',
      verifiedAt: '2026-03-01T02:00:05Z',
    }
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/backups/*': () => jsonResponse(detailOf(verifiedArtifact, null), 200),
      '/api/admin/v1/operations/*': () =>
        jsonResponse(
          taskDetailOf(
            { status: 'failed', finishedAt: '2026-03-01T02:00:05Z' },
            {
              errors: [
                {
                  code: 'interrupted_by_restart',
                  message: 'the Server restarted while this task was running',
                },
              ],
              result: {
                artifactId: OTHER_ARTIFACT_ID,
                verification: 'ok',
                checkedAt: '2026-03-01T02:00:05Z',
              },
            },
          ),
          200,
        ),
    })
    await renderAt('/admin/backups/' + OTHER_ARTIFACT_ID)
    await screen.findByRole('heading', { level: 1, name: verifiedArtifact.filename })

    expect(screen.getByText('Verified')).toBeTruthy()
    expect(await screen.findByText(/the artifact record above is the outcome this task wrote/)).toBeTruthy()
    expect(screen.queryByText(/holds the failure it recorded/)).toBeNull()
    expect(screen.queryByText(/Verification failed/)).toBeNull()
    expect(screen.getByText('interrupted_by_restart')).toBeTruthy()
  })

  it('keeps the accepted task when React re-runs the card’s effects', async () => {
    // React re-runs effect setup after its own cleanup (StrictMode does this in
    // development), so a card that only cleared its "still mounted" flag on
    // cleanup would drop every response: the button would stay on
    // `Requesting verification…` and the accepted task would never be read.
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/backups/*': (request) => {
        if (request.method === 'POST') return jsonResponse(queuedVerification(), 200)
        return jsonResponse(detailOf(PENDING_ARTIFACT), 200)
      },
      '/api/admin/v1/operations/*': () => jsonResponse(taskDetailOf({ status: 'queued' }), 200),
    })
    render(
      <StrictMode>
        <App />
      </StrictMode>,
    )
    await act(async () => {
      window.history.pushState({}, '', '/admin/backups/' + ARTIFACT_ID)
      window.dispatchEvent(new PopStateEvent('popstate'))
      await Promise.resolve()
    })
    await screen.findByRole('heading', { level: 1, name: PENDING_ARTIFACT.filename })

    fireEvent.click(screen.getByRole('button', { name: 'Request verification' }))
    expect(await screen.findByText(/Acceptance is not a result/)).toBeTruthy()
    expect(await screen.findByText(/is queued or running/)).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Request verification' })).toBeTruthy()
    expect(screen.queryByText('Requesting verification…')).toBeNull()
  })
})
