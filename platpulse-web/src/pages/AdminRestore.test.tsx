import { act, cleanup, render, screen, waitFor } from '@testing-library/react'
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

const ARTIFACT = {
  artifactId: 'artifact-1',
  filename: 'platpulse-artifact-1.db',
  bytes: 2048,
  sha256: 'a'.repeat(64),
  schemaVersion: 23,
  serverVersion: '0.1.0',
  createdAt: '2026-08-12T08:10:00Z',
  dataRangeMin: '2026-08-01T00:00:00Z',
  dataRangeMax: '2026-08-12T08:00:00Z',
  verification: 'ok',
  verifiedAt: '2026-08-12T08:11:00Z',
  createOperationId: 'op-backup-1',
}

const VALIDATION_OK = {
  artifactId: 'artifact-1',
  filename: 'platpulse-artifact-1.db',
  bytes: 2048,
  schemaVersion: 23,
  serverVersion: '0.1.0',
  createdAt: '2026-08-12T08:10:00Z',
  checksumOk: true,
  integrityOk: true,
  schemaCompatible: true,
  currentSchemaVersion: 23,
  error: null,
  message: null,
}

const RESTORE_OPERATION = {
  operationId: 'op-restore-1',
  kind: 'restore',
  status: 'failed',
  progressPercent: 100,
  progressLabel: null,
  requestId: 'req-restore-1',
  createdAt: '2026-08-12T08:20:00Z',
  startedAt: '2026-08-12T08:20:01Z',
  finishedAt: '2026-08-12T08:20:02Z',
  auditEventId: 77,
  cancelRequested: false,
}

const RESTORE_DETAIL = {
  operation: RESTORE_OPERATION,
  warnings: [],
  errors: [
    {
      code: 'restore_requires_stopped_server',
      message:
        'Restore requires an exclusive stopped Server. The current database was not modified.',
    },
  ],
  result: {
    validation: { checksum: 'ok', integrity: 'ok', schemaCompatible: true, schemaVersion: 23 },
    refusal: 'restore_requires_stopped_server',
    artifactId: 'artifact-1',
  },
  cancellable: false,
}

const SUBMIT_RESPONSE = {
  operation: RESTORE_DETAIL,
  auditEventId: 77,
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

describe('PAGE-ADMIN-RESTORE (issue #51)', () => {
  it('renders the prerequisites of the highest-risk workflow', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/backups': () => jsonResponse([ARTIFACT], 200),
      '/api/admin/v1/operations*': () => jsonResponse([], 200),
    })
    renderAt('/admin/data/restore')

    await screen.findByRole('heading', { level: 1, name: 'Restore a backup' })
    expect(screen.getByText('Prerequisites')).toBeTruthy()
    expect(screen.getByText(/exclusive stopped-Server condition/i)).toBeTruthy()
    expect(screen.getByText(/Secrets are never restored/)).toBeTruthy()
    expect(screen.getByText(/Failure preserves the current database/)).toBeTruthy()
    // No generic Operation row can trigger the flow: the confirmation
    // inputs only exist on this dedicated route.
    expect(screen.queryByLabelText(/Type the backup file name/)).toBeNull()
  })

  it('requires validation and typed confirmation before submitting', async () => {
    const fetchMock = mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/backups': () => jsonResponse([ARTIFACT], 200),
      '/api/admin/v1/operations/op-restore-1': () => jsonResponse(RESTORE_DETAIL, 200),
      '/api/admin/v1/operations*': () => jsonResponse([], 200),
      '/api/admin/v1/restore/validate': () => jsonResponse(VALIDATION_OK, 200),
      '/api/admin/v1/restore': () => jsonResponse(SUBMIT_RESPONSE, 200),
    })
    renderAt('/admin/data/restore')

    await screen.findByRole('heading', { level: 1, name: 'Restore a backup' })
    const artifactRow = await screen.findByRole('row', { name: /platpulse-artifact-1\.db/ })
    await act(async () => {
      ;(artifactRow.querySelector('input[type="radio"]') as HTMLInputElement).click()
      await Promise.resolve()
    })

    // The typed confirmation alone is not enough: validation must run.
    const confirm = screen.getByLabelText(/Type the backup file name/)
    const start = screen.getByRole('button', { name: 'Start Restore' })
    expect((start as HTMLButtonElement).disabled).toBe(true)

    await act(async () => {
      screen.getByRole('button', { name: 'Validate this backup' }).click()
      await Promise.resolve()
    })
    await screen.findAllByText('Pass')
    expect(screen.getByText(/backup 23 \/ Server 23/)).toBeTruthy()

    // A wrong phrase keeps the button disabled.
    await act(async () => {
      confirm.focus()
      // Controlled input update through the native setter keeps React state
      // in sync (same approach as the retention edit tests).
      const setter = Object.getOwnPropertyDescriptor(
        HTMLInputElement.prototype,
        'value',
      )!.set!
      setter.call(confirm, 'wrong-name.db')
      confirm.dispatchEvent(new Event('input', { bubbles: true }))
      await Promise.resolve()
    })
    expect((start as HTMLButtonElement).disabled).toBe(true)

    await act(async () => {
      const setter = Object.getOwnPropertyDescriptor(
        HTMLInputElement.prototype,
        'value',
      )!.set!
      setter.call(confirm, 'platpulse-artifact-1.db')
      confirm.dispatchEvent(new Event('input', { bubbles: true }))
      await Promise.resolve()
    })
    await waitFor(() => expect((start as HTMLButtonElement).disabled).toBe(false))
    await act(async () => {
      start.click()
      await Promise.resolve()
    })

    // The mutation returned an Operation reference immediately; the inline
    // Operation shows the typed stopped-Server refusal with the request ID
    // and Audit link — REST-authoritative, no optimistic state.
    await screen.findByRole('heading', { level: 2, name: 'Restore Operation' })
    expect(screen.getByText('Failed')).toBeTruthy()
    expect(screen.getByText(/exclusive stopped-Server condition is required/)).toBeTruthy()
    expect(screen.getByText('req-restore-1')).toBeTruthy()
    expect(screen.getByText('#77 — Audit history')).toBeTruthy()
    const submitCalls = fetchMock.mock.calls.filter(([input]) => {
      const url = (input instanceof Request ? input.url : String(input)).replace(
        TEST_ORIGIN,
        '',
      )
      return url === '/api/admin/v1/restore'
    })
    expect(submitCalls.length).toBe(1)
  })

  it('blocks submission when validation fails and shows the typed reason', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/backups': () => jsonResponse([ARTIFACT], 200),
      '/api/admin/v1/operations*': () => jsonResponse([], 200),
      '/api/admin/v1/restore/validate': () =>
        jsonResponse(
          {
            ...VALIDATION_OK,
            checksumOk: false,
            integrityOk: null,
            schemaCompatible: null,
            error: 'restore_checksum_mismatch',
            message: 'the backup artifact checksum does not match its recorded manifest',
          },
          200,
        ),
    })
    renderAt('/admin/data/restore')

    await screen.findByRole('heading', { level: 1, name: 'Restore a backup' })
    const artifactRow = await screen.findByRole('row', { name: /platpulse-artifact-1\.db/ })
    await act(async () => {
      ;(artifactRow.querySelector('input[type="radio"]') as HTMLInputElement).click()
      await Promise.resolve()
    })
    await act(async () => {
      screen.getByRole('button', { name: 'Validate this backup' }).click()
      await Promise.resolve()
    })
    await screen.findByText(/restore_checksum_mismatch/)
    expect(screen.getByText(/does not match its recorded manifest/)).toBeTruthy()
    // Checks that were never reached are Not checked, never Pass.
    expect(screen.getAllByText('Not checked').length).toBe(2)
    expect((screen.getByRole('button', { name: 'Start Restore' }) as HTMLButtonElement).disabled).toBe(true)
  })

  it('presents a succeeded Restore only after readiness and data refetch', async () => {
    const succeeded = {
      ...RESTORE_OPERATION,
      status: 'succeeded',
      finishedAt: '2026-08-12T09:00:00Z',
    }
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/backups': () => jsonResponse([ARTIFACT], 200),
      '/api/admin/v1/operations*': () => jsonResponse([succeeded], 200),
      '/health/ready': () =>
        jsonResponse({ status: 'ready', components: [{ name: 'sqlite', status: 'ready' }] }, 200),
    })
    renderAt('/admin/data/restore')

    await screen.findByRole('heading', { level: 1, name: 'Restore a backup' })
    await screen.findByText('Restore succeeded.', undefined, { timeout: 5_000 })
    await screen.findByText(/data views were refetched from the restored database/)
    expect(screen.getByText(/1 backup artifacts listed/)).toBeTruthy()
  })
})
