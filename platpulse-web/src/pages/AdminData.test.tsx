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

const OPERATION = {
  operationId: 'op-retention-1',
  kind: 'retention_run',
  status: 'succeeded',
  progressPercent: 100,
  progressLabel: 'raw_block_summary 1/1',
  requestId: 'req-retention-1',
  createdAt: '2026-08-12T08:00:00Z',
  startedAt: '2026-08-12T08:00:05Z',
  finishedAt: '2026-08-12T08:00:07Z',
  auditEventId: 42,
  cancelRequested: false,
}

const OPERATION_DETAIL = {
  operation: OPERATION,
  warnings: [],
  errors: [],
  result: { families: [{ family: 'raw_block_summary', deletedRows: 1 }] },
  cancellable: false,
}

const POLICIES = [
  {
    family: 'raw_block_summary',
    label: 'Raw Block Summaries',
    retentionDays: 7,
    minDays: 1,
    maxDays: 30,
    defaultDays: 7,
    supported: true,
    enabled: true,
    updatedAt: '2026-08-12T08:00:00Z',
    updatedBy: 'admin',
  },
  {
    family: 'one_hour_aggregate',
    label: '1-Hour Aggregates',
    retentionDays: 0,
    minDays: 0,
    maxDays: 0,
    defaultDays: 0,
    supported: false,
    enabled: true,
    updatedAt: '2026-08-12T08:00:00Z',
    updatedBy: 'defaults',
  },
]

const BACKUP = {
  artifactId: 'artifact-1',
  filename: 'platpulse-artifact-1.db',
  bytes: 2048,
  sha256: 'a'.repeat(64),
  schemaVersion: 22,
  serverVersion: '0.1.0',
  createdAt: '2026-08-12T08:10:00Z',
  dataRangeMin: '2026-08-01T00:00:00Z',
  dataRangeMax: '2026-08-12T08:00:00Z',
  verification: 'ok',
  verifiedAt: '2026-08-12T08:11:00Z',
  createOperationId: 'op-backup-1',
}

const BACKUP_DETAIL = { artifact: BACKUP, verificationError: null }

const DOCTOR_CHECKS = [
  { checkId: 'database_integrity', label: 'Database integrity', status: 'pass', detail: 'ok' },
  { checkId: 'web_assets', label: 'Web assets', status: 'not_configured', detail: 'no web root' },
  { checkId: 'latest_backup', label: 'Latest backup', status: 'skipped', detail: 'nothing yet' },
  { checkId: 'backup_storage', label: 'Backup storage', status: 'warning', detail: 'missing' },
]

const DOCTOR_OVERVIEW = {
  lastRun: { ...OPERATION, operationId: 'op-doctor-1', kind: 'doctor_run', status: 'succeeded_with_warnings' },
  checks: DOCTOR_CHECKS,
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

describe('PAGE-ADMIN-OPERATIONS (issue #50)', () => {
  it('lists durable Operations with status, progress, and request ID', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/operations*': () => jsonResponse([OPERATION], 200),
    })
    renderAt('/admin/operations')

    await screen.findByRole('heading', { level: 1, name: 'Operations' })
    const row = await screen.findByRole('row', { name: /Retention run/ })
    expect(row.textContent).toContain('Succeeded')
    expect(row.textContent).toContain('100')
    expect(row.textContent).toContain('req-retention-1')
    // The list shows the correlation id and a shortened operation id.
    expect(row.textContent).toContain('op-reten')
  })

  it('shows detail with warnings, errors, result, and audit link', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/operations/op-retention-1': () =>
        jsonResponse(
          {
            ...OPERATION_DETAIL,
            warnings: [{ code: 'retention_unsupported', message: 'one_minute_aggregate skipped' }],
          },
          200,
        ),
    })
    renderAt('/admin/operations/op-retention-1')

    await screen.findByRole('heading', { level: 1, name: 'Retention run' })
    expect((await screen.findByRole('progressbar')).getAttribute('aria-valuenow')).toBe('100')
    expect(screen.getByText('req-retention-1')).toBeTruthy()
    expect(screen.getByText(/Audit history/).getAttribute('href')).toBe('/admin/access/audit')
    expect(screen.getByText(/retention_unsupported/)).toBeTruthy()
    expect(screen.getAllByText(/raw_block_summary/).length).toBeGreaterThan(0)
  })

  it('cancels a running Operation through the confirmed audited mutation', async () => {
    const requests: string[] = []
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/operations/op-running-1': () => {
        const cancelled = requests.length > 0
        return jsonResponse(
          {
            ...OPERATION_DETAIL,
            operation: {
              ...OPERATION,
              operationId: 'op-running-1',
              status: cancelled ? 'cancelled' : 'running',
              progressPercent: cancelled ? 100 : 40,
            },
            cancellable: !cancelled,
          },
          200,
        )
      },
      '/api/admin/v1/operations/op-running-1/cancel': () => {
        requests.push('/api/admin/v1/operations/op-running-1/cancel')
        return jsonResponse(
          {
            operation: {
              ...OPERATION_DETAIL,
              operation: { ...OPERATION, operationId: 'op-running-1', status: 'cancelled' },
              cancellable: false,
            },
            auditEventId: 43,
          },
          200,
        )
      },
    })
    renderAt('/admin/operations/op-running-1')

    await screen.findByRole('heading', { level: 1, name: 'Retention run' })
    const cancelButton = await screen.findByRole('button', { name: 'Cancel Operation' })
    // Confirmation first: nothing is sent until the Owner confirms.
    fireEvent.click(cancelButton)
    expect(screen.getByText(/Cancel this Operation\?/)).toBeTruthy()
    expect(requests).not.toContain('/api/admin/v1/operations/op-running-1/cancel')
    fireEvent.click(screen.getByRole('button', { name: 'Yes, cancel' }))
    await waitFor(() => {
      expect(requests).toContain('/api/admin/v1/operations/op-running-1/cancel')
    })
    expect(await screen.findByText(/Cancellation requested/)).toBeTruthy()
  })
})

describe('PAGE-ADMIN-RETENTION (issue #50)', () => {
  it('lists policies with safety bounds and protected state', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/retention': () =>
        jsonResponse(
          {
            policies: POLICIES,
            protectedState: ['historical high-water marks', 'coverage intervals'],
            lastRun: OPERATION,
          },
          200,
        ),
    })
    renderAt('/admin/data/retention')

    await screen.findByRole('heading', { level: 1, name: 'Retention' })
    const rawRow = await screen.findByRole('row', { name: /Raw Block Summaries/ })
    expect(rawRow.textContent).toContain('7 days')
    expect(rawRow.textContent).toContain('1–30 days')
    const aggregateRow = screen.getByRole('row', { name: /1-Hour Aggregates/ })
    expect(aggregateRow.textContent).toContain('Unsupported')
    expect(aggregateRow.textContent).toContain('Long-term only')
    expect(screen.getByText('historical high-water marks')).toBeTruthy()
    expect(screen.getByText('coverage intervals')).toBeTruthy()
  })

  it('requires typed confirmation before queuing a run', async () => {
    let ran = false
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/retention': () =>
        jsonResponse(
          { policies: POLICIES, protectedState: [], lastRun: null },
          200,
        ),
      '/api/admin/v1/retention/run': () => {
        ran = true
        return jsonResponse(
          { operation: OPERATION_DETAIL, auditEventId: 44 },
          200,
        )
      },
      '/api/admin/v1/operations/op-retention-1': () =>
        jsonResponse(OPERATION_DETAIL, 200),
    })
    renderAt('/admin/data/retention')

    await screen.findByRole('heading', { level: 1, name: 'Retention' })
    fireEvent.click(screen.getByRole('button', { name: 'Run retention now' }))
    // The confirmation prompt appears; nothing is queued until confirmed.
    expect(screen.getByText(/Run retention for every enabled family now\?/)).toBeTruthy()
    expect(ran).toBe(false)
    fireEvent.click(screen.getByRole('button', { name: 'Yes, run now' }))
    await waitFor(() => expect(ran).toBe(true))
  })
})

describe('PAGE-ADMIN-RETENTION-EDIT (issue #50)', () => {
  it('shows a Server-computed impact preview and requires typed confirmation', async () => {
    let saved = false
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/retention': () =>
        jsonResponse({ policies: POLICIES, protectedState: [], lastRun: null }, 200),
      '/api/admin/v1/retention/impact': () =>
        jsonResponse(
          {
            family: 'raw_block_summary',
            retentionDays: 14,
            estimatedRows: 3,
            unsupported: false,
            bounds: { minDays: 1, maxDays: 30 },
            notes: [],
          },
          200,
        ),
      '/api/admin/v1/retention/policies/raw_block_summary': () => {
        saved = true
        return jsonResponse(
          { policy: { ...POLICIES[0], retentionDays: 14 }, auditEventId: 45 },
          200,
        )
      },
    })
    renderAt('/admin/data/retention/edit?family=raw_block_summary')

    await screen.findByRole('heading', { level: 1, name: 'Edit Raw Block Summaries' })
    const input = screen.getByLabelText('Retention (days)')
    fireEvent.change(input, { target: { value: '14' } })
    expect(await screen.findByText(/3 rows/)).toBeTruthy()
    expect(screen.getByText(/no Incidents, coverage, gap, counter/)).toBeTruthy()
    // Out-of-bounds values are rejected client-side against Server bounds.
    fireEvent.change(input, { target: { value: '99' } })
    expect(await screen.findByText(/between 1 and 30 days/)).toBeTruthy()
    fireEvent.change(input, { target: { value: '14' } })
    const saveButton = screen.getByRole('button', { name: 'Save policy' })
    expect((saveButton as HTMLButtonElement).disabled).toBe(true)
    fireEvent.change(screen.getByLabelText(/Type the family and value/), {
      target: { value: 'raw_block_summary 14' },
    })
    await waitFor(() => expect((saveButton as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(saveButton)
    await waitFor(() => expect(saved).toBe(true))
    expect(await screen.findByText(/Audit #45/)).toBeTruthy()
  })

  it('marks unsupported families as unchangeable', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/retention': () =>
        jsonResponse({ policies: POLICIES, protectedState: [], lastRun: null }, 200),
    })
    renderAt('/admin/data/retention/edit?family=one_hour_aggregate')

    await screen.findByRole('heading', { level: 1, name: 'Edit 1-Hour Aggregates' })
    expect(await screen.findByText(/long-term family/)).toBeTruthy()
    expect((screen.getByLabelText('Retention (days)') as HTMLInputElement).disabled).toBe(true)
    expect((screen.getByRole('button', { name: 'Save policy' }) as HTMLButtonElement).disabled).toBe(true)
  })
})

describe('PAGE-ADMIN-BACKUPS (issue #50)', () => {
  it('lists sanitized artifact metadata and never database contents', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/backups': () => jsonResponse([BACKUP], 200),
    })
    renderAt('/admin/data/backups')

    await screen.findByRole('heading', { level: 1, name: 'Backups' })
    const row = await screen.findByRole('row', { name: /platpulse-artifact-1\.db/ })
    expect(row.textContent).toContain('2.0 KiB')
    expect(row.textContent).toContain('Verified')
    expect(row.textContent).toContain('22')
    expect(screen.getByText('Create a backup').getAttribute('href')).toBe('/admin/data/backups/create')
  })

  it('requires the typed phrase before queuing a backup creation', async () => {
    let created = false
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/backups/create': () => jsonResponse({ error: { code: 'not_found' } }, 404),
      '/api/admin/v1/backups': () => {
        created = true
        return jsonResponse(
          { operation: { ...OPERATION_DETAIL, operation: { ...OPERATION, operationId: 'op-backup-new' } }, auditEventId: 46 },
          200,
        )
      },
      '/api/admin/v1/operations/op-backup-new': () => jsonResponse(OPERATION_DETAIL, 200),
    })
    renderAt('/admin/data/backups/create')

    await screen.findByRole('heading', { level: 1, name: 'Create a backup' })
    const button = screen.getByRole('button', { name: 'Queue backup' })
    expect((button as HTMLButtonElement).disabled).toBe(true)
    fireEvent.change(screen.getByLabelText(/Type the confirmation phrase/), {
      target: { value: 'create backup' },
    })
    await waitFor(() => expect((button as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(button)
    await waitFor(() => expect(created).toBe(true))
  })

  it('verifies an artifact through the Operation flow', async () => {
    let verified = false
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/backups/artifact-1': () => jsonResponse(BACKUP_DETAIL, 200),
      '/api/admin/v1/backups/artifact-1/verify': () => {
        verified = true
        return jsonResponse(
          { operation: { ...OPERATION_DETAIL, operation: { ...OPERATION, operationId: 'op-verify-1', kind: 'backup_verify' } }, auditEventId: 47 },
          200,
        )
      },
      '/api/admin/v1/operations/op-verify-1': () => jsonResponse(OPERATION_DETAIL, 200),
    })
    renderAt('/admin/data/backups/artifact-1')

    await screen.findByRole('heading', { level: 1, name: 'platpulse-artifact-1.db' })
    expect(screen.getByText('a'.repeat(64))).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Verify artifact' }))
    await waitFor(() => expect(verified).toBe(true))
  })
})

describe('PAGE-ADMIN-DOCTOR (issue #50)', () => {
  it('renders distinct check statuses and never offers fixes', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/doctor': () => jsonResponse(DOCTOR_OVERVIEW, 200),
    })
    renderAt('/admin/data/doctor')

    await screen.findByRole('heading', { level: 1, name: 'Doctor' })
    const row = await screen.findByRole('row', { name: /Database integrity/ })
    expect(row.textContent).toContain('Pass')
    expect(screen.getByRole('row', { name: /Web assets/ }).textContent).toContain('Not configured')
    expect(screen.getByRole('row', { name: /Latest backup/ }).textContent).toContain('Skipped')
    expect(screen.getByRole('row', { name: /Backup storage/ }).textContent).toContain('Warning')
    // Read-only: the only action is running the checks again.
    expect(screen.getByRole('button', { name: 'Run Doctor' })).toBeTruthy()
    expect(screen.queryByRole('button', { name: /fix/i })).toBeNull()
    expect(screen.getByText(/never auto-fixes/)).toBeTruthy()
  })
})
